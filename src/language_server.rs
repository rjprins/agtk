//! Go to definition and find references, answered by language servers.
//!
//! agtk starts one server per worktree and language on the first lookup and
//! stops it once it sits idle. Each server has its own thread, so a server that
//! is still indexing only delays its own lookups.

use std::collections::{HashMap, HashSet};
use std::ffi::OsString;
use std::fs;
use std::io::{self, BufRead, BufReader, Write};
use std::os::unix::ffi::{OsStrExt, OsStringExt};
use std::os::unix::process::CommandExt;
use std::path::{Path, PathBuf};
use std::process::{Child, ChildStderr, ChildStdin, ChildStdout, Command, Stdio};
use std::sync::mpsc::{self, RecvTimeoutError};
use std::sync::{Arc, Mutex, PoisonError};
use std::thread;
use std::time::{Duration, Instant};

use futures_channel::oneshot;
use nix::sys::signal::{Signal, killpg};
use nix::unistd::Pid;
use serde::Deserialize;
use serde_json::{Value, json};

const IDLE_TIMEOUT: Duration = Duration::from_secs(10 * 60);
const START_TIMEOUT: Duration = Duration::from_secs(30);
const DEFINITION_TIMEOUT: Duration = Duration::from_secs(30);
/// References need the whole workspace analyzed, which takes a while the first time.
const REFERENCES_TIMEOUT: Duration = Duration::from_secs(90);
const STOP_TIMEOUT: Duration = Duration::from_secs(2);
const MAX_MESSAGE_BYTES: usize = 64 * 1024 * 1024;
const MAX_LOCATIONS: usize = 1000;
const MAX_PREVIEW_FILES: usize = 100;
const MAX_PREVIEW_FILE_BYTES: u64 = 1024 * 1024;
const MAX_PREVIEW_BYTES: usize = 8 * 1024 * 1024;

/// A family of languages that one server answers for.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub enum Language {
    Python,
    TypeScript,
}

impl Language {
    /// The server family and LSP language ID for a file.
    pub fn for_path(path: &Path) -> Option<(Self, &'static str)> {
        let extension = path.extension()?.to_str()?.to_ascii_lowercase();
        Some(match extension.as_str() {
            "py" | "pyi" => (Self::Python, "python"),
            "ts" | "mts" | "cts" => (Self::TypeScript, "typescript"),
            "tsx" => (Self::TypeScript, "typescriptreact"),
            "js" | "mjs" | "cjs" => (Self::TypeScript, "javascript"),
            "jsx" => (Self::TypeScript, "javascriptreact"),
            _ => return None,
        })
    }

    fn name(self) -> &'static str {
        match self {
            Self::Python => "Python",
            Self::TypeScript => "TypeScript",
        }
    }

    /// Servers to look for, in order of preference. All of them take `--stdio`.
    fn commands(self) -> &'static [&'static str] {
        match self {
            Self::Python => &["basedpyright-langserver", "pyright-langserver"],
            Self::TypeScript => &["typescript-language-server", "vtsls"],
        }
    }

    fn missing_message(self) -> &'static str {
        match self {
            Self::Python => "Install pyright to look up Python code",
            Self::TypeScript => {
                "Install typescript-language-server to look up TypeScript and JavaScript code"
            }
        }
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Deserialize)]
#[serde(rename_all = "camelCase")]
pub enum Lookup {
    Definition,
    References,
}

#[derive(Debug, Clone)]
pub struct Query {
    pub lookup: Lookup,
    pub root: PathBuf,
    pub path: PathBuf,
    /// The text the user sees, which may differ from the file on disk.
    pub text: Arc<str>,
    /// Zero-based line and UTF-16 offset, as LSP counts them.
    pub line: u32,
    pub character: u32,
}

/// A range in a file, with zero-based lines and UTF-16 offsets.
#[derive(Debug, Clone, PartialEq, Eq, Hash)]
pub struct Location {
    pub path: PathBuf,
    pub start: (u32, u32),
    pub end: (u32, u32),
    /// Whether the range is in the queried text rather than another file.
    pub in_query: bool,
}

#[derive(Debug, Default)]
pub struct Answer {
    pub locations: Vec<Location>,
    /// The text of other files that locations point into, for previews.
    pub files: HashMap<PathBuf, String>,
}

pub type LookupResult = Result<Answer, String>;

struct Job {
    query: Query,
    reply: oneshot::Sender<LookupResult>,
}

/// The running servers, keyed by worktree root and language.
#[derive(Default)]
pub struct LanguageServers {
    servers: Mutex<HashMap<(PathBuf, Language), mpsc::Sender<Job>>>,
}

impl LanguageServers {
    pub fn lookup(&self, query: Query) -> oneshot::Receiver<LookupResult> {
        let (reply, receiver) = oneshot::channel();
        let Some((language, _)) = Language::for_path(&query.path) else {
            let _ = reply.send(Err(
                "Lookups are not available for this kind of file".to_owned()
            ));
            return receiver;
        };
        if !query.root.is_absolute() || !query.root.is_dir() {
            let _ = reply.send(Err("Lookups only work in a project folder".to_owned()));
            return receiver;
        }
        let key = (query.root.clone(), language);
        let mut job = Job { query, reply };
        let mut servers = self.servers.lock().unwrap_or_else(PoisonError::into_inner);
        if let Some(sender) = servers.get(&key) {
            match sender.send(job) {
                Ok(()) => return receiver,
                // The server stopped since, so start a new one.
                Err(mpsc::SendError(returned)) => job = returned,
            }
        }
        let (sender, jobs) = mpsc::channel();
        let _ = sender.send(job);
        let root = key.0.clone();
        let spawned = thread::Builder::new()
            .name(format!(
                "agtk-{}-server",
                language.name().to_ascii_lowercase()
            ))
            .spawn(move || serve(language, root, jobs));
        if spawned.is_ok() {
            servers.insert(key, sender);
        }
        receiver
    }

    /// Stops the servers of a worktree, for example once it is removed.
    pub fn stop(&self, root: &Path) {
        self.servers
            .lock()
            .unwrap_or_else(PoisonError::into_inner)
            .retain(|(server_root, _), _| server_root != root);
    }
}

fn serve(language: Language, root: PathBuf, jobs: mpsc::Receiver<Job>) {
    let mut server = match Server::start(language, &root) {
        Ok(server) => server,
        Err(error) => {
            while let Ok(job) = jobs.try_recv() {
                let _ = job.reply.send(Err(error.clone()));
            }
            return;
        }
    };
    // Dropping every sender, or sitting idle, ends the loop.
    while let Ok(job) = jobs.recv_timeout(IDLE_TIMEOUT) {
        let result = server.answer(&job.query);
        let _ = job.reply.send(result);
        if !server.is_running() {
            return;
        }
    }
    server.stop();
}

struct OpenDocument {
    uri: String,
    version: i64,
    text: Arc<str>,
}

type Response = (i64, Result<Value, String>);

struct Server {
    language: Language,
    child: Child,
    stdin: Arc<Mutex<ChildStdin>>,
    responses: mpsc::Receiver<Response>,
    /// The last line the server wrote to stderr, to explain a crash.
    last_error: Arc<Mutex<String>>,
    next_id: i64,
    /// The one document agtk keeps open, so other files are read from disk.
    open: Option<OpenDocument>,
}

impl Server {
    fn start(language: Language, root: &Path) -> Result<Self, String> {
        let search_path = crate::session::login_shell_path().or_else(|| std::env::var_os("PATH"));
        let home = std::env::var_os("HOME");
        let program = language
            .commands()
            .iter()
            .find_map(|command| {
                crate::session::resolve_executable_from(
                    command,
                    None,
                    search_path.as_deref(),
                    home.as_deref(),
                )
            })
            .ok_or_else(|| language.missing_message().to_owned())?;
        let mut command = Command::new(&program);
        command
            .arg("--stdio")
            .current_dir(root)
            .stdin(Stdio::piped())
            .stdout(Stdio::piped())
            .stderr(Stdio::piped())
            // Its own group, so stopping it also stops the processes it starts.
            .process_group(0);
        if let Some(path) = &search_path {
            command.env("PATH", path);
        }
        let mut child = command.spawn().map_err(|error| {
            format!(
                "Could not start {}: {error}",
                program.file_name().unwrap_or_default().to_string_lossy()
            )
        })?;
        let (Some(stdin), Some(stdout), Some(stderr)) =
            (child.stdin.take(), child.stdout.take(), child.stderr.take())
        else {
            let _ = child.kill();
            let _ = child.wait();
            return Err("Could not connect to the language server".to_owned());
        };
        let stdin = Arc::new(Mutex::new(stdin));
        let (sender, responses) = mpsc::channel();
        let settings = Settings::new(language, root);
        let reader_stdin = stdin.clone();
        thread::spawn(move || read_messages(stdout, reader_stdin, sender, settings));
        let last_error = Arc::new(Mutex::new(String::new()));
        let stderr_tail = last_error.clone();
        thread::spawn(move || keep_last_error(stderr, stderr_tail));

        let mut server = Self {
            language,
            child,
            stdin,
            responses,
            last_error,
            next_id: 0,
            open: None,
        };
        let root_uri = path_to_uri(root);
        let name = root
            .file_name()
            .map(|name| name.to_string_lossy().into_owned())
            .unwrap_or_default();
        server.request(
            "initialize",
            json!({
                "processId": std::process::id(),
                "clientInfo": { "name": "agtk", "version": env!("CARGO_PKG_VERSION") },
                "rootPath": root.to_string_lossy(),
                "rootUri": root_uri,
                "workspaceFolders": [{ "uri": root_uri, "name": name }],
                "capabilities": {
                    "general": { "positionEncodings": ["utf-16"] },
                    "workspace": { "configuration": true, "workspaceFolders": true },
                    "textDocument": {
                        "synchronization": { "dynamicRegistration": false },
                        "definition": { "dynamicRegistration": false, "linkSupport": true },
                        "references": { "dynamicRegistration": false },
                    },
                },
            }),
            START_TIMEOUT,
        )?;
        server.notify("initialized", json!({}))?;
        // Pyright reads its settings again when told they changed.
        server.notify(
            "workspace/didChangeConfiguration",
            json!({ "settings": {} }),
        )?;
        Ok(server)
    }

    fn answer(&mut self, query: &Query) -> LookupResult {
        let Some((_, language_id)) = Language::for_path(&query.path) else {
            return Err("Lookups are not available for this kind of file".to_owned());
        };
        let uri = path_to_uri(&query.path);
        self.show_document(&uri, language_id, &query.text)?;
        let mut params = json!({
            "textDocument": { "uri": uri },
            "position": { "line": query.line, "character": query.character },
        });
        let (method, timeout) = match query.lookup {
            Lookup::Definition => ("textDocument/definition", DEFINITION_TIMEOUT),
            Lookup::References => {
                params["context"] = json!({ "includeDeclaration": true });
                ("textDocument/references", REFERENCES_TIMEOUT)
            }
        };
        let result = self.request(method, params, timeout)?;
        Ok(collect_answer(parse_locations(&result), &query.path))
    }

    /// Opens the queried text in the server, closing the document opened before.
    fn show_document(
        &mut self,
        uri: &str,
        language_id: &str,
        text: &Arc<str>,
    ) -> Result<(), String> {
        if let Some(open) = &self.open
            && open.uri == uri
        {
            if *open.text == **text {
                return Ok(());
            }
            let version = open.version + 1;
            self.notify(
                "textDocument/didChange",
                json!({
                    "textDocument": { "uri": uri, "version": version },
                    "contentChanges": [{ "text": &**text }],
                }),
            )?;
            self.open = Some(OpenDocument {
                uri: uri.to_owned(),
                version,
                text: text.clone(),
            });
            return Ok(());
        }
        if let Some(open) = self.open.take() {
            self.notify(
                "textDocument/didClose",
                json!({ "textDocument": { "uri": open.uri } }),
            )?;
        }
        self.notify(
            "textDocument/didOpen",
            json!({
                "textDocument": {
                    "uri": uri,
                    "languageId": language_id,
                    "version": 1,
                    "text": &**text,
                },
            }),
        )?;
        self.open = Some(OpenDocument {
            uri: uri.to_owned(),
            version: 1,
            text: text.clone(),
        });
        Ok(())
    }

    fn request(&mut self, method: &str, params: Value, timeout: Duration) -> Result<Value, String> {
        self.next_id += 1;
        let id = self.next_id;
        self.send(&json!({ "jsonrpc": "2.0", "id": id, "method": method, "params": params }))?;
        let deadline = Instant::now() + timeout;
        loop {
            match self
                .responses
                .recv_timeout(deadline.saturating_duration_since(Instant::now()))
            {
                Ok((answered, result)) if answered == id => return result,
                // A late answer to a request that timed out.
                Ok(_) => {}
                Err(RecvTimeoutError::Timeout) => {
                    let _ = self.notify("$/cancelRequest", json!({ "id": id }));
                    return Err(format!(
                        "The {} language server did not answer within {} seconds",
                        self.language.name(),
                        timeout.as_secs()
                    ));
                }
                Err(RecvTimeoutError::Disconnected) => return Err(self.stopped_message()),
            }
        }
    }

    fn notify(&self, method: &str, params: Value) -> Result<(), String> {
        self.send(&json!({ "jsonrpc": "2.0", "method": method, "params": params }))
    }

    fn send(&self, message: &Value) -> Result<(), String> {
        let mut stdin = self.stdin.lock().unwrap_or_else(PoisonError::into_inner);
        write_message(&mut *stdin, message).map_err(|_| self.stopped_message())
    }

    fn stopped_message(&self) -> String {
        let last_error = self
            .last_error
            .lock()
            .unwrap_or_else(PoisonError::into_inner)
            .clone();
        if last_error.is_empty() {
            format!("The {} language server stopped", self.language.name())
        } else {
            format!(
                "The {} language server stopped: {last_error}",
                self.language.name()
            )
        }
    }

    fn is_running(&mut self) -> bool {
        matches!(self.child.try_wait(), Ok(None))
    }

    /// Asks the server to exit, and kills it if it does not.
    fn stop(mut self) {
        if self.request("shutdown", Value::Null, STOP_TIMEOUT).is_ok() {
            let _ = self.notify("exit", Value::Null);
        }
        let deadline = Instant::now() + STOP_TIMEOUT;
        while self.is_running() && Instant::now() < deadline {
            thread::sleep(Duration::from_millis(50));
        }
    }
}

impl Drop for Server {
    fn drop(&mut self) {
        if self.is_running()
            && let Ok(pid) = i32::try_from(self.child.id())
        {
            let _ = killpg(Pid::from_raw(pid), Signal::SIGKILL);
        }
        let _ = self.child.wait();
    }
}

/// What agtk answers when a server asks for its settings.
struct Settings {
    language: Language,
    root_uri: String,
    name: String,
    python: Option<PathBuf>,
}

impl Settings {
    fn new(language: Language, root: &Path) -> Self {
        Self {
            language,
            root_uri: path_to_uri(root),
            name: root
                .file_name()
                .map(|name| name.to_string_lossy().into_owned())
                .unwrap_or_default(),
            python: (language == Language::Python)
                .then(|| python_interpreter(root))
                .flatten(),
        }
    }

    /// The result for a request from the server, or `None` when agtk does not handle it.
    fn answer_request(&self, method: &str, params: Option<&Value>) -> Option<Value> {
        match method {
            "workspace/configuration" => {
                let items = params
                    .and_then(|params| params.get("items"))
                    .and_then(Value::as_array)
                    .map(Vec::as_slice)
                    .unwrap_or_default();
                Some(Value::Array(
                    items
                        .iter()
                        .map(|item| self.section(item.get("section").and_then(Value::as_str)))
                        .collect(),
                ))
            }
            "workspace/workspaceFolders" => {
                Some(json!([{ "uri": self.root_uri, "name": self.name }]))
            }
            "window/workDoneProgress/create"
            | "client/registerCapability"
            | "client/unregisterCapability"
            | "window/showMessageRequest" => Some(Value::Null),
            _ => None,
        }
    }

    fn section(&self, section: Option<&str>) -> Value {
        match (self.language, section) {
            (Language::Python, Some("python")) => match &self.python {
                Some(python) => json!({ "pythonPath": python }),
                None => json!({}),
            },
            (Language::Python, Some("python.analysis" | "basedpyright.analysis")) => json!({
                "autoSearchPaths": true,
                "useLibraryCodeForTypes": true,
                "diagnosticMode": "openFilesOnly",
                "logLevel": "Error",
            }),
            (Language::Python, Some("pyright" | "basedpyright")) => json!({}),
            _ => Value::Null,
        }
    }
}

/// The worktree's virtual environment, or the one in the main checkout, since
/// worktrees that agents create often have none of their own.
fn python_interpreter(root: &Path) -> Option<PathBuf> {
    let mut roots = vec![root.to_owned()];
    roots.extend(main_worktree(root).filter(|main| main != root));
    roots
        .iter()
        .flat_map(|root| [".venv", "venv"].map(|name| root.join(name).join("bin/python")))
        .find(|python| python.is_file())
}

/// The main checkout of a linked worktree, whose `.git` is a file pointing into it.
fn main_worktree(root: &Path) -> Option<PathBuf> {
    let link = fs::read_to_string(root.join(".git")).ok()?;
    let git_dir = root.join(link.strip_prefix("gitdir:")?.trim());
    let common = fs::read_to_string(git_dir.join("commondir")).ok()?;
    let common = git_dir.join(common.trim()).canonicalize().ok()?;
    if common.file_name()? != ".git" {
        return None;
    }
    common.parent().map(Path::to_owned)
}

fn read_messages(
    stdout: ChildStdout,
    stdin: Arc<Mutex<ChildStdin>>,
    responses: mpsc::Sender<Response>,
    settings: Settings,
) {
    let mut reader = BufReader::new(stdout);
    while let Ok(Some(message)) = read_message(&mut reader) {
        let method = message.get("method").and_then(Value::as_str);
        match (message.get("id"), method) {
            // A request from the server, answered here so it never waits on a lookup.
            (Some(id), Some(method)) => {
                let reply = match settings.answer_request(method, message.get("params")) {
                    Some(result) => json!({ "jsonrpc": "2.0", "id": id, "result": result }),
                    None => json!({
                        "jsonrpc": "2.0",
                        "id": id,
                        "error": { "code": -32601, "message": "Method not found" },
                    }),
                };
                let mut stdin = stdin.lock().unwrap_or_else(PoisonError::into_inner);
                if write_message(&mut *stdin, &reply).is_err() {
                    return;
                }
            }
            (Some(id), None) => {
                let Some(id) = id.as_i64() else {
                    continue;
                };
                let result = match message.get("error") {
                    Some(error) => Err(error
                        .get("message")
                        .and_then(Value::as_str)
                        .unwrap_or("The language server reported an error")
                        .to_owned()),
                    None => Ok(message.get("result").cloned().unwrap_or(Value::Null)),
                };
                if responses.send((id, result)).is_err() {
                    return;
                }
            }
            // Diagnostics, progress and log messages are not shown.
            _ => {}
        }
    }
}

fn keep_last_error(stderr: ChildStderr, last_error: Arc<Mutex<String>>) {
    let mut reader = BufReader::new(stderr);
    let mut line = Vec::new();
    loop {
        line.clear();
        match reader.read_until(b'\n', &mut line) {
            Ok(0) | Err(_) => return,
            Ok(_) => {
                let text = String::from_utf8_lossy(&line);
                let text = text.trim();
                if !text.is_empty() {
                    let mut last = last_error.lock().unwrap_or_else(PoisonError::into_inner);
                    *last = text.chars().take(300).collect();
                }
            }
        }
    }
}

fn write_message(writer: &mut impl Write, message: &Value) -> io::Result<()> {
    let body = serde_json::to_vec(message)?;
    write!(writer, "Content-Length: {}\r\n\r\n", body.len())?;
    writer.write_all(&body)?;
    writer.flush()
}

/// Reads one framed message, or `None` once the server closed its output.
fn read_message(reader: &mut impl BufRead) -> io::Result<Option<Value>> {
    let mut length = None;
    let mut line = String::new();
    loop {
        line.clear();
        if reader.read_line(&mut line)? == 0 {
            return Ok(None);
        }
        let header = line.trim_end();
        if header.is_empty() {
            break;
        }
        if let Some((name, value)) = header.split_once(':')
            && name.trim().eq_ignore_ascii_case("content-length")
        {
            length = value.trim().parse::<usize>().ok();
        }
    }
    let length = length
        .filter(|length| *length <= MAX_MESSAGE_BYTES)
        .ok_or_else(|| io::Error::new(io::ErrorKind::InvalidData, "bad message length"))?;
    let mut body = vec![0; length];
    reader.read_exact(&mut body)?;
    serde_json::from_slice(&body)
        .map(Some)
        .map_err(io::Error::other)
}

/// Reads a definition or references result: a location, a list of locations or links, or null.
fn parse_locations(result: &Value) -> Vec<Location> {
    let items = match result {
        Value::Array(items) => items.iter().collect(),
        Value::Object(_) => vec![result],
        _ => Vec::new(),
    };
    items
        .into_iter()
        .filter_map(|item| {
            let (uri, range) = match item.get("targetUri") {
                Some(uri) => (
                    uri,
                    item.get("targetSelectionRange")
                        .or_else(|| item.get("targetRange"))?,
                ),
                None => (item.get("uri")?, item.get("range")?),
            };
            Some(Location {
                path: uri_to_path(uri.as_str()?)?,
                start: position(range.get("start")?)?,
                end: position(range.get("end")?)?,
                in_query: false,
            })
        })
        .collect()
}

fn position(value: &Value) -> Option<(u32, u32)> {
    Some((
        u32::try_from(value.get("line")?.as_u64()?).ok()?,
        u32::try_from(value.get("character")?.as_u64()?).ok()?,
    ))
}

/// Marks locations in the queried file and reads the other files for previews.
fn collect_answer(locations: Vec<Location>, query_path: &Path) -> Answer {
    let query_file = query_path.canonicalize().ok();
    let mut seen = HashSet::new();
    let mut answer = Answer::default();
    let mut unreadable = HashSet::new();
    let mut preview_bytes = 0;
    for mut location in locations {
        if !seen.insert(location.clone()) {
            continue;
        }
        if answer.locations.len() == MAX_LOCATIONS {
            break;
        }
        location.in_query = location.path == query_path
            || query_file.is_some() && location.path.canonicalize().ok() == query_file;
        if !location.in_query
            && !answer.files.contains_key(&location.path)
            && !unreadable.contains(&location.path)
        {
            match read_preview(&location.path, MAX_PREVIEW_BYTES - preview_bytes) {
                Some(text) if answer.files.len() < MAX_PREVIEW_FILES => {
                    preview_bytes += text.len();
                    answer.files.insert(location.path.clone(), text);
                }
                _ => {
                    unreadable.insert(location.path.clone());
                }
            }
        }
        answer.locations.push(location);
    }
    answer
}

fn read_preview(path: &Path, budget: usize) -> Option<String> {
    let size = fs::metadata(path)
        .ok()
        .filter(|metadata| metadata.is_file())?
        .len();
    if size > MAX_PREVIEW_FILE_BYTES || size as usize > budget {
        return None;
    }
    fs::read_to_string(path).ok()
}

pub fn path_to_uri(path: &Path) -> String {
    let mut uri = String::from("file://");
    for &byte in path.as_os_str().as_bytes() {
        if byte.is_ascii_alphanumeric() || b"/-._~".contains(&byte) {
            uri.push(char::from(byte));
        } else {
            uri.push_str(&format!("%{byte:02X}"));
        }
    }
    uri
}

pub fn uri_to_path(uri: &str) -> Option<PathBuf> {
    let rest = uri.strip_prefix("file://")?;
    // Skip an authority such as `localhost`.
    let rest = &rest[rest.find('/')?..];
    let mut bytes = Vec::with_capacity(rest.len());
    let mut input = rest.bytes();
    while let Some(byte) = input.next() {
        if byte == b'%' {
            let high = char::from(input.next()?).to_digit(16)?;
            let low = char::from(input.next()?).to_digit(16)?;
            bytes.push(u8::try_from(high * 16 + low).ok()?);
        } else {
            bytes.push(byte);
        }
    }
    Some(PathBuf::from(OsString::from_vec(bytes)))
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn picks_a_server_by_extension() {
        assert_eq!(
            Language::for_path(Path::new("app/views.py")),
            Some((Language::Python, "python"))
        );
        assert_eq!(
            Language::for_path(Path::new("src/App.TSX")),
            Some((Language::TypeScript, "typescriptreact"))
        );
        assert_eq!(
            Language::for_path(Path::new("main.mjs")),
            Some((Language::TypeScript, "javascript"))
        );
        assert_eq!(Language::for_path(Path::new("Cargo.toml")), None);
        assert_eq!(Language::for_path(Path::new("Makefile")), None);
    }

    #[test]
    fn converts_paths_to_uris_and_back() {
        let path = Path::new("/home/me/my project/naïve#1.py");
        let uri = path_to_uri(path);
        assert_eq!(uri, "file:///home/me/my%20project/na%C3%AFve%231.py");
        assert_eq!(uri_to_path(&uri).as_deref(), Some(path));
        assert_eq!(
            uri_to_path("file://localhost/tmp/a%3Ab.ts").as_deref(),
            Some(Path::new("/tmp/a:b.ts"))
        );
        assert_eq!(uri_to_path("https://example.com/a.py"), None);
        assert_eq!(uri_to_path("file:///bad%zz"), None);
    }

    #[test]
    fn reads_and_writes_framed_messages() {
        let mut buffer = Vec::new();
        write_message(&mut buffer, &json!({ "id": 1, "result": "é" })).unwrap();
        write_message(&mut buffer, &json!({ "method": "exit" })).unwrap();
        assert!(buffer.starts_with(b"Content-Length: 22\r\n\r\n"));
        let mut reader = io::Cursor::new(buffer);
        assert_eq!(
            read_message(&mut reader).unwrap(),
            Some(json!({ "id": 1, "result": "é" }))
        );
        assert_eq!(
            read_message(&mut reader).unwrap(),
            Some(json!({ "method": "exit" }))
        );
        assert_eq!(read_message(&mut reader).unwrap(), None);
    }

    #[test]
    fn reads_extra_headers_and_refuses_bad_lengths() {
        let message = b"Content-Type: application/vscode-jsonrpc\r\ncontent-length: 2\r\n\r\n{}";
        let mut reader = io::Cursor::new(message.to_vec());
        assert_eq!(read_message(&mut reader).unwrap(), Some(json!({})));
        let mut reader = io::Cursor::new(b"Content-Length: nope\r\n\r\n{}".to_vec());
        assert!(read_message(&mut reader).is_err());
    }

    #[test]
    fn parses_locations_and_location_links() {
        let range =
            json!({ "start": { "line": 3, "character": 4 }, "end": { "line": 3, "character": 9 } });
        let single = json!({ "uri": "file:///a.py", "range": range });
        assert_eq!(
            parse_locations(&single),
            [Location {
                path: PathBuf::from("/a.py"),
                start: (3, 4),
                end: (3, 9),
                in_query: false,
            }]
        );
        let links = json!([{
            "targetUri": "file:///b.ts",
            "targetRange": { "start": { "line": 1, "character": 0 }, "end": { "line": 8, "character": 1 } },
            "targetSelectionRange": range,
        }]);
        assert_eq!(parse_locations(&links)[0].start, (3, 4));
        assert!(parse_locations(&Value::Null).is_empty());
        assert!(parse_locations(&json!([{ "uri": "file:///c.py" }])).is_empty());
    }

    #[test]
    fn marks_the_queried_file_and_reads_other_files_once() {
        let folder = tempfile::tempdir().unwrap();
        let shown = folder.path().join("shown.py");
        let other = folder.path().join("other.py");
        fs::write(&shown, "x = 1\n").unwrap();
        fs::write(&other, "def x(): pass\n").unwrap();
        let at = |path: &Path, line| Location {
            path: path.to_owned(),
            start: (line, 0),
            end: (line, 1),
            in_query: false,
        };
        let answer = collect_answer(
            vec![at(&shown, 0), at(&other, 0), at(&other, 0), at(&other, 2)],
            &shown,
        );
        assert_eq!(answer.locations.len(), 3);
        assert!(answer.locations[0].in_query);
        assert!(!answer.locations[1].in_query);
        assert_eq!(answer.files.len(), 1);
        assert_eq!(answer.files[&other], "def x(): pass\n");
    }

    #[test]
    fn finds_the_virtual_environment_of_the_main_checkout() {
        let folder = tempfile::tempdir().unwrap();
        let main = folder.path().join("main");
        let linked = folder.path().join("linked");
        let git_dir = main.join(".git/worktrees/linked");
        fs::create_dir_all(&git_dir).unwrap();
        fs::create_dir_all(main.join(".venv/bin")).unwrap();
        fs::create_dir_all(&linked).unwrap();
        fs::write(main.join(".venv/bin/python"), "").unwrap();
        fs::write(git_dir.join("commondir"), "../..\n").unwrap();
        fs::write(
            linked.join(".git"),
            format!("gitdir: {}\n", git_dir.display()),
        )
        .unwrap();
        let main = main.canonicalize().unwrap();
        assert_eq!(main_worktree(&linked).as_deref(), Some(main.as_path()));
        assert_eq!(
            python_interpreter(&linked),
            Some(main.join(".venv/bin/python"))
        );
        fs::create_dir_all(linked.join(".venv/bin")).unwrap();
        fs::write(linked.join(".venv/bin/python"), "").unwrap();
        assert_eq!(
            python_interpreter(&linked),
            Some(linked.join(".venv/bin/python"))
        );
    }

    #[test]
    fn answers_configuration_requests_per_section() {
        let settings = Settings {
            language: Language::Python,
            root_uri: "file:///repo".to_owned(),
            name: "repo".to_owned(),
            python: Some(PathBuf::from("/repo/.venv/bin/python")),
        };
        let result = settings
            .answer_request(
                "workspace/configuration",
                Some(&json!({ "items": [
                    { "section": "python" },
                    { "section": "python.analysis" },
                    { "section": "files" },
                ] })),
            )
            .unwrap();
        assert_eq!(result[0], json!({ "pythonPath": "/repo/.venv/bin/python" }));
        assert_eq!(result[1]["useLibraryCodeForTypes"], json!(true));
        assert_eq!(result[2], Value::Null);
        assert_eq!(
            settings.answer_request("window/workDoneProgress/create", None),
            Some(Value::Null)
        );
        assert_eq!(settings.answer_request("workspace/applyEdit", None), None);
    }
}
