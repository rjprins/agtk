use std::cmp::Reverse;
use std::collections::{BTreeMap, BTreeSet};
use std::fs::{self, File};
use std::io::{BufRead, BufReader, Read, Seek, SeekFrom};
use std::path::{Path, PathBuf};
use std::sync::{LazyLock, Mutex};
use std::time::{SystemTime, UNIX_EPOCH};

use memchr::memmem;

use serde::{Deserialize, Serialize};
use serde_json::{Map, Value};

use crate::control::{CreateSessionParams, SessionKind};
use crate::history::submitted_prompt;
use crate::persist::PersistResult;

const LOG_HEAD_BYTES: u64 = 1024 * 1024;
const LOG_PREVIEW_BYTES: u64 = 4 * 1024 * 1024;
const MAX_MESSAGE_CHARS: usize = 2_000;
const MAX_TITLE_CHARS: usize = 160;

pub fn recent_mutated_paths(path: &Path, limit: usize) -> PersistResult<Vec<PathBuf>> {
    let content = read_tail(path, LOG_PREVIEW_BYTES)?;
    let entries = content
        .text
        .lines()
        .filter_map(|line| serde_json::from_str::<Value>(line).ok())
        .collect::<Vec<_>>();
    let mut paths = Vec::new();
    let mut seen = BTreeSet::new();
    for entry in entries.iter().rev() {
        let Some(entry) = entry.as_object() else {
            continue;
        };
        if entry.get("type").and_then(Value::as_str) != Some("assistant") {
            continue;
        }
        let Some(blocks) = entry
            .get("message")
            .and_then(Value::as_object)
            .and_then(|message| message.get("content"))
            .and_then(Value::as_array)
        else {
            continue;
        };
        for block in blocks {
            let Some(block) = block.as_object() else {
                continue;
            };
            if block.get("type").and_then(Value::as_str) != Some("tool_use") {
                continue;
            }
            let name = block
                .get("name")
                .and_then(Value::as_str)
                .unwrap_or_default()
                .to_ascii_lowercase();
            if !matches!(
                name.as_str(),
                "edit" | "write" | "multiedit" | "notebookedit"
            ) {
                continue;
            }
            let Some(input) = block.get("input").and_then(Value::as_object) else {
                continue;
            };
            let Some(path) = input
                .get("file_path")
                .or_else(|| input.get("notebook_path"))
                .and_then(Value::as_str)
                .map(str::trim)
                .filter(|path| !path.is_empty())
            else {
                continue;
            };
            if seen.insert(path.to_owned()) {
                paths.push(PathBuf::from(path));
                if paths.len() >= limit.max(1) {
                    return Ok(paths);
                }
            }
        }
    }
    Ok(paths)
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord, Serialize, Deserialize)]
#[serde(rename_all = "lowercase")]
pub enum AgentProvider {
    Codex,
    Claude,
}

impl AgentProvider {
    pub const fn command(self) -> &'static str {
        match self {
            Self::Codex => "codex",
            Self::Claude => "claude",
        }
    }

    pub const fn kind(self) -> SessionKind {
        match self {
            Self::Codex => SessionKind::Codex,
            Self::Claude => SessionKind::Claude,
        }
    }

    pub fn resume_args(self, provider_session_id: &str) -> Vec<String> {
        match self {
            Self::Codex => vec!["resume".to_owned(), provider_session_id.to_owned()],
            Self::Claude => vec!["--resume".to_owned(), provider_session_id.to_owned()],
        }
    }

    /// Starts a new conversation from a copy of `provider_session_id` and
    /// leaves the original alone, so both can carry on.
    pub fn fork_plan(self, provider_session_id: &str) -> PersistResult<ForkPlan> {
        Ok(match self {
            Self::Codex => ForkPlan {
                args: vec!["fork".to_owned(), provider_session_id.to_owned()],
                conversation_id: None,
            },
            Self::Claude => {
                let conversation_id = random_uuid()?;
                ForkPlan {
                    args: vec![
                        "--resume".to_owned(),
                        provider_session_id.to_owned(),
                        "--fork-session".to_owned(),
                        "--session-id".to_owned(),
                        conversation_id.clone(),
                    ],
                    conversation_id: Some(conversation_id),
                }
            }
        })
    }
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ForkPlan {
    pub args: Vec<String>,
    /// The copy's conversation, when agtk chooses it. Codex picks its own,
    /// which agtk finds in the logs like any other Codex conversation.
    pub conversation_id: Option<String>,
}

/// A random version 4 UUID, the form Claude requires for a session ID.
fn random_uuid() -> PersistResult<String> {
    let mut bytes = [0_u8; 16];
    File::open("/dev/urandom")?.read_exact(&mut bytes)?;
    bytes[6] = (bytes[6] & 0x0f) | 0x40;
    bytes[8] = (bytes[8] & 0x3f) | 0x80;
    let hex = bytes
        .iter()
        .map(|byte| format!("{byte:02x}"))
        .collect::<String>();
    Ok(format!(
        "{}-{}-{}-{}-{}",
        &hex[..8],
        &hex[8..12],
        &hex[12..16],
        &hex[16..20],
        &hex[20..]
    ))
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct ProviderSession {
    pub provider: AgentProvider,
    pub provider_session_id: String,
    /// The best title: the given name, the provider's title, the command, or the first prompt.
    pub name: String,
    /// A name the user gave with the provider's rename command.
    pub custom_title: Option<String>,
    /// The title the provider generated.
    pub ai_title: Option<String>,
    pub first_prompt: Option<String>,
    pub last_prompt: Option<String>,
    pub branch: Option<String>,
    pub prompt_count: u32,
    pub cwd: Option<PathBuf>,
    pub created_at: u64,
    pub last_seen_at: u64,
    pub log_path: PathBuf,
}

impl ProviderSession {
    pub fn restore_plan(&self, target: RestoreTarget) -> RestorationPlan {
        let cwd = target
            .cwd
            .or_else(|| target.worktree_path.clone())
            .or_else(|| self.cwd.clone());
        RestorationPlan {
            params: CreateSessionParams {
                kind: self.provider.kind(),
                command: None,
                args: self.provider.resume_args(&self.provider_session_id),
                cwd,
                name: target.name.or_else(|| Some(self.name.clone())),
                project_root: target.project_root,
                worktree_path: target.worktree_path,
                initial_input: None,
            },
            conversation_id: self.provider_session_id.clone(),
        }
    }
}

#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct RestoreTarget {
    pub cwd: Option<PathBuf>,
    pub project_root: Option<PathBuf>,
    pub worktree_path: Option<PathBuf>,
    pub name: Option<String>,
}

#[derive(Debug, Clone, PartialEq)]
pub struct RestorationPlan {
    pub params: CreateSessionParams,
    pub conversation_id: String,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct ConversationMessage {
    pub role: ConversationRole,
    pub text: String,
    pub timestamp: Option<u64>,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize)]
#[serde(rename_all = "lowercase")]
pub enum ConversationRole {
    User,
    Assistant,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct ProviderPreview {
    pub session: ProviderSession,
    pub messages: Vec<ConversationMessage>,
    pub is_truncated: bool,
    /// Files the agent edited, newest first.
    pub files_changed: Vec<PathBuf>,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct DiscoveryRoots {
    pub claude_config_dir: PathBuf,
    pub codex_home_dir: PathBuf,
}

impl DiscoveryRoots {
    pub fn from_environment() -> Self {
        let home = std::env::var_os("HOME")
            .map(PathBuf::from)
            .unwrap_or_else(std::env::temp_dir);
        Self {
            claude_config_dir: std::env::var_os("CLAUDE_CONFIG_DIR")
                .map(PathBuf::from)
                .unwrap_or_else(|| home.join(".claude")),
            codex_home_dir: std::env::var_os("CODEX_HOME")
                .map(PathBuf::from)
                .unwrap_or_else(|| home.join(".codex")),
        }
    }
}

#[derive(Debug, Clone)]
pub struct ProviderDiscovery {
    roots: DiscoveryRoots,
    scan_limit: usize,
    max_age_millis: u64,
    cache_path: Option<PathBuf>,
}

impl ProviderDiscovery {
    pub fn new(roots: DiscoveryRoots, scan_limit: usize, max_age_millis: u64) -> Self {
        Self {
            roots,
            scan_limit: scan_limit.clamp(1, 2_000),
            max_age_millis,
            cache_path: None,
        }
    }

    /// Keeps parsed log facts in this file so a restart does not rescan every log.
    pub fn with_cache(mut self, path: PathBuf) -> Self {
        self.cache_path = Some(path);
        self
    }

    pub fn from_environment() -> Self {
        Self::new(
            DiscoveryRoots::from_environment(),
            500,
            90 * 24 * 60 * 60 * 1_000,
        )
    }

    pub fn discover(
        &self,
        now_millis: u64,
        live: &BTreeSet<(AgentProvider, String)>,
    ) -> PersistResult<Vec<ProviderSession>> {
        let mut candidates = self.candidates()?;
        candidates.retain(|candidate| {
            now_millis.saturating_sub(candidate.modified_at) <= self.max_age_millis
        });
        candidates.sort_by_key(|candidate| Reverse(candidate.modified_at));
        candidates.truncate(self.scan_limit);
        let codex_titles = self.codex_titles();
        let mut sessions = BTreeMap::<(AgentProvider, String), ProviderSession>::new();
        for candidate in candidates {
            let Some(session) = self.parse_session(&candidate, &codex_titles)? else {
                continue;
            };
            let key = (session.provider, session.provider_session_id.clone());
            if live.contains(&key) {
                continue;
            }
            if sessions
                .get(&key)
                .is_none_or(|previous| session.last_seen_at > previous.last_seen_at)
            {
                sessions.insert(key, session);
            }
        }
        let mut sessions = sessions.into_values().collect::<Vec<_>>();
        sessions.sort_by_key(|session| Reverse(session.last_seen_at));
        self.save_cache();
        Ok(sessions)
    }

    /// Whether a conversation has a log to resume. Both providers name the file
    /// after it, so reading the logs is only the fallback.
    pub fn has_log(
        &self,
        provider: AgentProvider,
        provider_session_id: &str,
    ) -> PersistResult<bool> {
        validate_provider_session_id(provider_session_id)?;
        let named = self.candidates()?.iter().any(|candidate| {
            candidate.provider == provider
                && candidate
                    .path
                    .file_stem()
                    .and_then(|stem| stem.to_str())
                    .is_some_and(|stem| stem.ends_with(provider_session_id))
        });
        Ok(named || self.preview(provider, provider_session_id, 1).is_ok())
    }

    /// The rollout a running Codex session writes: by conversation when known,
    /// else the earliest log in its directory that started after launch and no
    /// other live session claims. Codex creates the file at the first prompt,
    /// so this finds nothing before then.
    pub fn locate_codex_log(
        &self,
        conversation_id: Option<&str>,
        cwd: &Path,
        launched_at: u64,
        claimed: &BTreeSet<String>,
    ) -> PersistResult<Option<(String, PathBuf)>> {
        // Clocks agree on one machine, but Codex notes its start a little late.
        const CLOCK_SLACK_MILLIS: u64 = 5_000;
        let mut candidates = self.candidates()?;
        candidates.retain(|candidate| candidate.provider == AgentProvider::Codex);
        if let Some(conversation_id) = conversation_id {
            validate_provider_session_id(conversation_id)?;
            return Ok(candidates
                .into_iter()
                .find(|candidate| {
                    candidate
                        .path
                        .file_stem()
                        .and_then(|stem| stem.to_str())
                        .is_some_and(|stem| stem.ends_with(conversation_id))
                })
                .map(|candidate| (conversation_id.to_owned(), candidate.path)));
        }
        candidates.retain(|candidate| candidate.modified_at + CLOCK_SLACK_MILLIS >= launched_at);
        let mut best: Option<(u64, String, PathBuf)> = None;
        for candidate in candidates {
            let Some(meta) = codex_session_meta(&candidate.path) else {
                continue;
            };
            if meta.cwd != cwd
                || meta.started_at + CLOCK_SLACK_MILLIS < launched_at
                || claimed.contains(&meta.session_id)
            {
                continue;
            }
            if best
                .as_ref()
                .is_none_or(|(started, _, _)| meta.started_at < *started)
            {
                best = Some((meta.started_at, meta.session_id, candidate.path));
            }
        }
        Ok(best.map(|(_, session_id, path)| (session_id, path)))
    }

    pub fn preview(
        &self,
        provider: AgentProvider,
        provider_session_id: &str,
        max_messages: usize,
    ) -> PersistResult<ProviderPreview> {
        validate_provider_session_id(provider_session_id)?;
        if !(1..=100).contains(&max_messages) {
            return Err("max messages must be between 1 and 100".into());
        }
        let mut candidates = self.candidates()?;
        candidates.sort_by_key(|candidate| Reverse(candidate.modified_at));
        candidates.truncate(self.scan_limit);
        let codex_titles = self.codex_titles();
        for candidate in candidates
            .into_iter()
            .filter(|candidate| candidate.provider == provider)
        {
            let Some(session) = self.parse_session(&candidate, &codex_titles)? else {
                continue;
            };
            if session.provider_session_id != provider_session_id {
                continue;
            }
            let content = read_tail(&candidate.path, LOG_PREVIEW_BYTES)?;
            let mut messages = conversation_messages(&content.text);
            let is_truncated = content.is_truncated || messages.len() > max_messages;
            if messages.len() > max_messages {
                messages.drain(..messages.len() - max_messages);
            }
            let files_changed = match provider {
                AgentProvider::Claude => recent_mutated_paths(&candidate.path, 8)?,
                AgentProvider::Codex => Vec::new(),
            };
            return Ok(ProviderPreview {
                session,
                messages,
                is_truncated,
                files_changed,
            });
        }
        Err(format!(
            "no {} log was found for session {provider_session_id}",
            provider.command()
        )
        .into())
    }

    fn candidates(&self) -> PersistResult<Vec<LogCandidate>> {
        let mut candidates = Vec::new();
        scan_jsonl(
            AgentProvider::Claude,
            &self.roots.claude_config_dir.join("projects"),
            3,
            &mut candidates,
        )?;
        scan_jsonl(
            AgentProvider::Codex,
            &self.roots.codex_home_dir.join("sessions"),
            4,
            &mut candidates,
        )?;
        Ok(candidates)
    }

    /// Codex keeps thread names in an index beside its sessions; the last line wins.
    fn codex_titles(&self) -> BTreeMap<String, String> {
        let Ok(content) = fs::read_to_string(self.roots.codex_home_dir.join("session_index.jsonl"))
        else {
            return BTreeMap::new();
        };
        let mut titles = BTreeMap::new();
        for entry in jsonl_entries(&content) {
            if let (Some(id), Some(name)) = (
                string_field(&entry, &["id"]),
                string_field(&entry, &["thread_name"]),
            ) {
                titles.insert(id, name);
            }
        }
        titles
    }

    fn parse_session(
        &self,
        candidate: &LogCandidate,
        codex_titles: &BTreeMap<String, String>,
    ) -> PersistResult<Option<ProviderSession>> {
        let Some(facts) = self.cached_facts(candidate)? else {
            return Ok(None);
        };
        Ok(session_from_facts(candidate, facts, codex_titles))
    }

    fn cached_facts(&self, candidate: &LogCandidate) -> PersistResult<Option<LogFacts>> {
        let len = fs::metadata(&candidate.path)?.len();
        let mut cache = LOG_CACHE
            .lock()
            .map_err(|_| "provider cache lock poisoned")?;
        if let Some(path) = &self.cache_path
            && cache.loaded.insert(path.clone())
        {
            cache.entries.extend(load_cache(path));
        }
        if let Some(entry) = cache.entries.get(&candidate.path)
            && entry.len == len
            && entry.modified_at == candidate.modified_at
        {
            return Ok(entry.facts.clone());
        }
        drop(cache);
        let facts = read_log_facts(candidate)?;
        let mut cache = LOG_CACHE
            .lock()
            .map_err(|_| "provider cache lock poisoned")?;
        cache.entries.insert(
            candidate.path.clone(),
            CachedFacts {
                len,
                modified_at: candidate.modified_at,
                facts: facts.clone(),
            },
        );
        cache.dirty = true;
        Ok(facts)
    }

    fn save_cache(&self) {
        let Some(path) = &self.cache_path else {
            return;
        };
        let Ok(mut cache) = LOG_CACHE.lock() else {
            return;
        };
        if !cache.dirty {
            return;
        }
        // Forget logs that are gone, so the file does not grow forever.
        cache.entries.retain(|log, _| log.exists());
        let file = CacheFile {
            version: CACHE_VERSION,
            entries: std::mem::take(&mut cache.entries),
        };
        let data = serde_json::to_vec(&file);
        cache.entries = file.entries;
        let Ok(data) = data else {
            return;
        };
        let temporary = path.with_extension("tmp");
        if fs::write(&temporary, data).is_ok() && fs::rename(&temporary, path).is_ok() {
            cache.dirty = false;
        }
    }
}

#[derive(Default)]
struct LogCache {
    entries: BTreeMap<PathBuf, CachedFacts>,
    loaded: BTreeSet<PathBuf>,
    dirty: bool,
}

static LOG_CACHE: LazyLock<Mutex<LogCache>> = LazyLock::new(Mutex::default);

#[derive(Debug, Clone, Serialize, Deserialize)]
struct CachedFacts {
    len: u64,
    modified_at: u64,
    facts: Option<LogFacts>,
}

/// Bump when LogFacts changes meaning, so old files are reread.
const CACHE_VERSION: u32 = 1;

#[derive(Serialize, Deserialize)]
struct CacheFile {
    version: u32,
    entries: BTreeMap<PathBuf, CachedFacts>,
}

fn load_cache(path: &Path) -> BTreeMap<PathBuf, CachedFacts> {
    fs::read(path)
        .ok()
        .and_then(|data| serde_json::from_slice::<CacheFile>(&data).ok())
        .filter(|file| file.version == CACHE_VERSION)
        .map(|file| file.entries)
        .unwrap_or_default()
}

#[derive(Debug)]
struct LogCandidate {
    provider: AgentProvider,
    path: PathBuf,
    modified_at: u64,
}

fn scan_jsonl(
    provider: AgentProvider,
    root: &Path,
    max_depth: usize,
    target: &mut Vec<LogCandidate>,
) -> PersistResult<()> {
    if !root.is_dir() {
        return Ok(());
    }
    let mut pending = vec![(root.to_owned(), 0_usize)];
    let mut visited = 0_usize;
    while let Some((directory, depth)) = pending.pop() {
        visited += 1;
        if visited > 10_000 {
            return Err("provider log directory limit exceeded".into());
        }
        let entries = match fs::read_dir(directory) {
            Ok(entries) => entries,
            Err(_) => continue,
        };
        for entry in entries.flatten() {
            let file_type = match entry.file_type() {
                Ok(file_type) => file_type,
                Err(_) => continue,
            };
            if file_type.is_symlink() {
                continue;
            }
            if file_type.is_dir() {
                if depth < max_depth && entry.file_name() != "subagents" {
                    pending.push((entry.path(), depth + 1));
                }
                continue;
            }
            if !file_type.is_file()
                || entry.path().extension().and_then(|value| value.to_str()) != Some("jsonl")
            {
                continue;
            }
            let Ok(metadata) = entry.metadata() else {
                continue;
            };
            target.push(LogCandidate {
                provider,
                path: entry.path(),
                modified_at: system_millis(metadata.modified().unwrap_or(UNIX_EPOCH)),
            });
        }
    }
    Ok(())
}

/// What one log says about its session. Cached, so keep it small.
#[derive(Debug, Clone, Default, Serialize, Deserialize)]
struct LogFacts {
    session_id: Option<String>,
    cwd: Option<String>,
    first_prompt: Option<String>,
    command: Option<String>,
    custom_title: Option<String>,
    ai_title: Option<String>,
    last_prompt: Option<String>,
    branch: Option<String>,
    prompt_count: u32,
    /// First and last entry times in Unix milliseconds. The file time is no
    /// substitute: Claude rewrites old logs when it lists them.
    first_activity: Option<u64>,
    last_activity: Option<u64>,
    /// Newer Codex logs only record prompts as response items.
    #[serde(skip)]
    codex_item_prompts: u32,
}

fn session_from_facts(
    candidate: &LogCandidate,
    facts: LogFacts,
    codex_titles: &BTreeMap<String, String>,
) -> Option<ProviderSession> {
    let provider_session_id = facts.session_id.unwrap_or_else(|| {
        candidate
            .path
            .file_stem()
            .and_then(|value| value.to_str())
            .unwrap_or("session")
            .to_owned()
    });
    if validate_provider_session_id(&provider_session_id).is_err() {
        return None;
    }
    let cwd = facts.cwd.map(PathBuf::from);
    let ai_title = facts
        .ai_title
        .or_else(|| codex_titles.get(&provider_session_id).cloned());
    let name = [
        facts.custom_title.as_deref(),
        ai_title.as_deref(),
        facts.command.as_deref(),
        facts.first_prompt.as_deref(),
    ]
    .into_iter()
    .flatten()
    .map(|text| first_line(text, MAX_TITLE_CHARS))
    .find(|text| !text.is_empty())
    .unwrap_or_else(|| {
        cwd.as_ref()
            .and_then(|path| path.file_name())
            .and_then(|name| name.to_str())
            .map(|leaf| format!("{}:{leaf}", candidate.provider.command()))
            .unwrap_or_else(|| {
                format!(
                    "{}:{}",
                    candidate.provider.command(),
                    provider_session_id.chars().take(8).collect::<String>()
                )
            })
    });
    Some(ProviderSession {
        provider: candidate.provider,
        provider_session_id,
        name,
        custom_title: facts.custom_title,
        ai_title,
        first_prompt: facts.first_prompt,
        last_prompt: facts.last_prompt,
        branch: facts.branch.filter(|branch| branch != "HEAD"),
        prompt_count: facts.prompt_count,
        cwd,
        created_at: facts.first_activity.unwrap_or(candidate.modified_at),
        last_seen_at: facts.last_activity.unwrap_or(candidate.modified_at),
        log_path: candidate.path.clone(),
    })
}

/// Reads the head for identity and the first prompt, then streams the whole log
/// for what changes over time: titles, the last prompt, the branch, and the count.
fn read_log_facts(candidate: &LogCandidate) -> PersistResult<Option<LogFacts>> {
    let Some(mut facts) = read_head_facts(candidate)? else {
        return Ok(None);
    };
    let reader = BufReader::new(File::open(&candidate.path)?);
    for line in reader.split(b'\n') {
        scan_line(candidate.provider, &line?, &mut facts);
    }
    facts.prompt_count = facts.prompt_count.max(facts.codex_item_prompts);
    Ok(Some(facts))
}

static CLAUDE_TYPED_PROMPT: LazyLock<memmem::Finder<'static>> =
    LazyLock::new(|| memmem::Finder::new(br#""message":{"role":"user","content":"#));
static TOOL_RESULT: LazyLock<memmem::Finder<'static>> =
    LazyLock::new(|| memmem::Finder::new(br#""tool_use_id""#));
static TIMESTAMP: LazyLock<memmem::Finder<'static>> =
    LazyLock::new(|| memmem::Finder::new(br#""timestamp":""#));
static GIT_BRANCH: LazyLock<memmem::Finder<'static>> =
    LazyLock::new(|| memmem::Finder::new(br#""gitBranch":""#));

fn contains(line: &[u8], needle: &str) -> bool {
    memmem::find(line, needle.as_bytes()).is_some()
}

/// Cheap byte checks first; only the few matching lines are parsed as JSON.
fn scan_line(provider: AgentProvider, line: &[u8], facts: &mut LogFacts) {
    if let Some(start) = TIMESTAMP.find(line)
        && let Some(time) = line
            .get(start + TIMESTAMP.needle().len()..)
            .and_then(|value| value.get(..value.iter().position(|byte| *byte == b'"')?))
            .and_then(parse_utc_millis)
    {
        facts.first_activity.get_or_insert(time);
        facts.last_activity = facts.last_activity.max(Some(time));
    }
    match provider {
        AgentProvider::Claude => {
            if let Some(start) = CLAUDE_TYPED_PROMPT.find(line) {
                let content = &line[start + CLAUDE_TYPED_PROMPT.needle().len()..];
                let typed = match content.first() {
                    Some(b'"') => {
                        content.get(1) != Some(&b'<') || is_session_command(&content[1..])
                    }
                    Some(b'[') => TOOL_RESULT.find(line).is_none(),
                    _ => false,
                };
                if typed && !contains(line, r#""isMeta":true"#) {
                    facts.prompt_count += 1;
                }
            }
            if let Some(start) = GIT_BRANCH.find(line) {
                let value = &line[start + GIT_BRANCH.needle().len()..];
                if let Some(end) = memchr::memchr(b'"', value)
                    && end > 0
                {
                    facts.branch = Some(String::from_utf8_lossy(&value[..end]).into_owned());
                }
            }
            let field = if contains(line, r#""type":"custom-title""#) {
                "customTitle"
            } else if contains(line, r#""type":"ai-title""#) {
                "aiTitle"
            } else if contains(line, r#""type":"last-prompt""#) {
                "lastPrompt"
            } else {
                return;
            };
            let Ok(Value::Object(entry)) = serde_json::from_slice::<Value>(line) else {
                return;
            };
            let value = string_field(&entry, &[field]);
            match field {
                "customTitle" => facts.custom_title = value,
                "aiTitle" => facts.ai_title = value,
                _ => {
                    if let Some(prompt) = value.filter(|prompt| !skip_user_text(prompt)) {
                        facts.last_prompt = Some(truncate_chars(&prompt, 400));
                    }
                }
            }
        }
        AgentProvider::Codex => {
            let is_event = contains(line, r#""type":"user_message""#);
            if !is_event && !contains(line, r#""role":"user""#) {
                return;
            }
            let Ok(Value::Object(entry)) = serde_json::from_slice::<Value>(line) else {
                return;
            };
            let prompt = if is_event {
                entry
                    .get("payload")
                    .and_then(Value::as_object)
                    .and_then(|payload| string_field(payload, &["message"]))
            } else {
                message(&entry)
                    .filter(|(role, _)| *role == ConversationRole::User)
                    .map(|(_, text)| text)
            };
            let Some(prompt) = prompt.filter(|prompt| !skip_user_text(prompt.trim())) else {
                return;
            };
            if is_event {
                facts.prompt_count += 1;
            } else {
                facts.codex_item_prompts += 1;
            }
            facts.last_prompt = Some(truncate_chars(prompt.trim(), 400));
        }
    }
}

/// Parses the `2026-09-29T14:46:47.853Z` times both providers write.
fn parse_utc_millis(value: &[u8]) -> Option<u64> {
    let text = std::str::from_utf8(value).ok()?.strip_suffix('Z')?;
    let (date, time) = text.split_once('T')?;
    let mut date = date.splitn(3, '-').map(str::parse::<i64>);
    let (year, month, day) = (date.next()?.ok()?, date.next()?.ok()?, date.next()?.ok()?);
    let (clock, fraction) = time.split_once('.').unwrap_or((time, "0"));
    let mut clock = clock.splitn(3, ':').map(str::parse::<i64>);
    let (hour, minute, second) = (
        clock.next()?.ok()?,
        clock.next()?.ok()?,
        clock.next()?.ok()?,
    );
    let millis = format!("{fraction:0<3}").get(..3)?.parse::<i64>().ok()?;
    if !(1..=12).contains(&month) || !(1..=31).contains(&day) {
        return None;
    }
    // Days from the civil date, after Howard Hinnant's algorithm.
    let year = if month <= 2 { year - 1 } else { year };
    let era = year.div_euclid(400);
    let year_of_era = year - era * 400;
    let day_of_year = (153 * ((month + 9) % 12) + 2) / 5 + day - 1;
    let day_of_era = year_of_era * 365 + year_of_era / 4 - year_of_era / 100 + day_of_year;
    let days = era * 146_097 + day_of_era - 719_468;
    let seconds = days * 86_400 + hour * 3_600 + minute * 60 + second;
    u64::try_from(seconds * 1_000 + millis).ok()
}

/// Reads the log head line by line and stops once every fact is known, which
/// is usually within the first few lines. None for empty or ancillary logs.
fn read_head_facts(candidate: &LogCandidate) -> PersistResult<Option<LogFacts>> {
    let reader = BufReader::new(File::open(&candidate.path)?.take(LOG_HEAD_BYTES));
    let mut facts = LogFacts::default();
    let mut has_conversation = false;
    let mut is_first = true;
    for line in reader.split(b'\n') {
        let line = line?;
        let Ok(Value::Object(entry)) =
            serde_json::from_str::<Value>(String::from_utf8_lossy(&line).trim())
        else {
            continue;
        };
        if is_first && is_ancillary_codex_head(candidate.provider, &entry) {
            return Ok(None);
        }
        is_first = false;
        // A Claude log of only snapshots and summaries holds no conversation.
        has_conversation |= candidate.provider != AgentProvider::Claude
            || !matches!(
                entry.get("type").and_then(Value::as_str),
                Some("file-history-snapshot" | "summary")
            );
        if facts.session_id.is_none() {
            facts.session_id = entry_session_id(&entry);
        }
        if facts.cwd.is_none() {
            facts.cwd = entry_cwd(&entry);
        }
        if facts.branch.is_none() {
            facts.branch = entry
                .get("payload")
                .and_then(|payload| payload.get("git"))
                .and_then(Value::as_object)
                .and_then(|git| string_field(git, &["branch"]));
        }
        if facts.command.is_none() && facts.first_prompt.is_none() {
            facts.command = entry_command(&entry);
        }
        if facts.first_prompt.is_none() {
            facts.first_prompt = entry_user_text(&entry).map(|text| truncate_chars(&text, 600));
        }
        if facts.session_id.is_some()
            && facts.cwd.is_some()
            && facts.first_prompt.is_some()
            && has_conversation
        {
            break;
        }
    }
    Ok((!is_first && has_conversation).then_some(facts))
}

const QUIET_COMMANDS: [&str; 7] = [
    "/clear", "/effort", "/model", "/compact", "/resume", "/config", "/login",
];

/// A slash command that is real work, like a skill, not a setting change.
fn is_session_command(content: &[u8]) -> bool {
    const TAG: &[u8] = b"<command-name>";
    let head = &content[..content.len().min(300)];
    let Some(start) = memmem::find(head, TAG) else {
        return false;
    };
    let rest = &content[start + TAG.len()..];
    rest.first() == Some(&b'/')
        && !QUIET_COMMANDS.iter().any(|command| {
            rest.strip_prefix(command.as_bytes())
                .is_some_and(|after| after.first() == Some(&b'<'))
        })
}

/// A session started with a slash command, such as `/review-pr 103601`.
fn entry_command(entry: &Map<String, Value>) -> Option<String> {
    let (role, text) = message(entry)?;
    if role != ConversationRole::User {
        return None;
    }
    let tag = |name: &str| {
        let open = format!("<{name}>");
        let start = text.find(&open)? + open.len();
        let end = text[start..].find(&format!("</{name}>"))? + start;
        Some(text[start..end].trim().to_owned())
    };
    let command = tag("command-name").filter(|command| command.starts_with('/'))?;
    // Built-in commands like /clear and /effort say nothing about the session.
    if QUIET_COMMANDS.contains(&command.as_str()) {
        return None;
    }
    let args = tag("command-args").unwrap_or_default();
    Some(if args.is_empty() {
        command
    } else {
        format!("{command} {args}")
    })
}

fn is_ancillary_codex_head(provider: AgentProvider, entry: &Map<String, Value>) -> bool {
    provider == AgentProvider::Codex
        && entry.get("type").and_then(Value::as_str) == Some("session_meta")
        && entry
            .get("payload")
            .and_then(Value::as_object)
            .and_then(|payload| payload.get("source"))
            .is_some_and(Value::is_object)
}

fn entry_session_id(entry: &Map<String, Value>) -> Option<String> {
    string_field(entry, &["sessionId", "session_id"])
        .or_else(|| {
            (entry.get("type").and_then(Value::as_str) == Some("session"))
                .then(|| string_field(entry, &["id"]))
                .flatten()
        })
        .or_else(|| {
            entry
                .get("payload")
                .and_then(Value::as_object)
                .and_then(|payload| string_field(payload, &["id", "sessionId", "session_id"]))
        })
}

fn entry_cwd(entry: &Map<String, Value>) -> Option<String> {
    string_field(entry, &["cwd"]).or_else(|| {
        entry
            .get("payload")
            .and_then(Value::as_object)
            .and_then(|payload| string_field(payload, &["cwd", "working_directory"]))
    })
}

fn entry_user_text(entry: &Map<String, Value>) -> Option<String> {
    let text =
        message(entry).and_then(|(role, text)| (role == ConversationRole::User).then_some(text))?;
    let trimmed = text.trim();
    if trimmed.len() < 10 || skip_user_text(trimmed) {
        None
    } else {
        Some(trimmed.to_owned())
    }
}

fn conversation_messages(content: &str) -> Vec<ConversationMessage> {
    jsonl_entries(content)
        .into_iter()
        .filter_map(|entry| {
            let (role, text) = message(&entry)?;
            let text = text.trim();
            if text.is_empty() || (role == ConversationRole::User && skip_user_text(text)) {
                return None;
            }
            Some(ConversationMessage {
                role,
                text: truncate_chars(text, MAX_MESSAGE_CHARS),
                timestamp: entry
                    .get("timestamp")
                    .and_then(Value::as_str)
                    .and_then(|stamp| parse_utc_millis(stamp.as_bytes())),
            })
        })
        .collect()
}

fn message(entry: &Map<String, Value>) -> Option<(ConversationRole, String)> {
    match entry.get("type").and_then(Value::as_str) {
        Some("user" | "assistant") => {
            let role = if entry.get("type").and_then(Value::as_str) == Some("user") {
                ConversationRole::User
            } else {
                ConversationRole::Assistant
            };
            let content = entry.get("message")?.as_object()?.get("content")?;
            extract_text(content).map(|text| (role, text))
        }
        Some("response_item") => {
            let payload = entry.get("payload")?.as_object()?;
            let role = match payload.get("role").and_then(Value::as_str) {
                Some("user") => ConversationRole::User,
                Some("assistant") => ConversationRole::Assistant,
                _ => return None,
            };
            extract_text(payload.get("content")?).map(|text| (role, text))
        }
        _ => None,
    }
}

fn extract_text(content: &Value) -> Option<String> {
    if let Some(text) = content.as_str() {
        return Some(text.to_owned());
    }
    content
        .as_array()?
        .iter()
        .find_map(|block| block.as_object()?.get("text")?.as_str().map(str::to_owned))
}

fn jsonl_entries(content: &str) -> Vec<Map<String, Value>> {
    content
        .lines()
        .filter_map(|line| serde_json::from_str::<Value>(line.trim()).ok())
        .filter_map(|value| value.as_object().cloned())
        .collect()
}

fn string_field(object: &Map<String, Value>, fields: &[&str]) -> Option<String> {
    fields.iter().find_map(|field| {
        object
            .get(*field)
            .and_then(Value::as_str)
            .map(str::trim)
            .filter(|value| !value.is_empty())
            .map(str::to_owned)
    })
}

fn skip_user_text(text: &str) -> bool {
    text.starts_with("# AGENTS.md")
        || text.starts_with("# INSTRUCTIONS")
        || text.starts_with('<')
        || text.contains("<environment_context>")
}

fn first_line(text: &str, max_chars: usize) -> String {
    let line = text.lines().next().unwrap_or(text).trim();
    let line = line
        .strip_prefix("Please ")
        .or_else(|| line.strip_prefix("please "))
        .unwrap_or(line);
    truncate_chars(line, max_chars)
}

fn truncate_chars(text: &str, max_chars: usize) -> String {
    if text.chars().count() <= max_chars {
        return text.to_owned();
    }
    let mut value = text.chars().take(max_chars).collect::<String>();
    value.push('…');
    value
}

fn validate_provider_session_id(value: &str) -> PersistResult<()> {
    // Claude and Codex use UUIDs. The ID goes into argv after --resume, so a
    // leading '-' would read as a flag.
    if !(1..=240).contains(&value.len())
        || value.starts_with(['-', '.'])
        || !value
            .bytes()
            .all(|byte| byte.is_ascii_alphanumeric() || matches!(byte, b'-' | b'_' | b'.'))
    {
        return Err("provider session ID is invalid".into());
    }
    Ok(())
}

struct TailContent {
    text: String,
    is_truncated: bool,
}

fn read_tail(path: &Path, limit: u64) -> PersistResult<TailContent> {
    let mut file = File::open(path)?;
    let size = file.metadata()?.len();
    let start = size.saturating_sub(limit);
    file.seek(SeekFrom::Start(start))?;
    let mut bytes = Vec::new();
    file.take(limit).read_to_end(&mut bytes)?;
    let mut text = String::from_utf8_lossy(&bytes).into_owned();
    if start > 0
        && let Some(newline) = text.find('\n')
    {
        text.drain(..=newline);
    }
    Ok(TailContent {
        text,
        is_truncated: start > 0,
    })
}

struct CodexSessionMeta {
    session_id: String,
    cwd: PathBuf,
    started_at: u64,
}

/// The first line of a rollout names the session, its directory, and when
/// Codex started. None for ancillary or foreign files.
fn codex_session_meta(path: &Path) -> Option<CodexSessionMeta> {
    let mut reader = BufReader::new(File::open(path).ok()?.take(LOG_HEAD_BYTES));
    let mut line = String::new();
    reader.read_line(&mut line).ok()?;
    let Value::Object(entry) = serde_json::from_str::<Value>(line.trim()).ok()? else {
        return None;
    };
    if entry.get("type").and_then(Value::as_str) != Some("session_meta") {
        return None;
    }
    let payload = entry.get("payload")?.as_object()?;
    // The payload time is the process start; the line is only written at the first prompt.
    let started_at = string_field(payload, &["timestamp"])
        .or_else(|| string_field(&entry, &["timestamp"]))
        .and_then(|time| parse_utc_millis(time.as_bytes()))?;
    Some(CodexSessionMeta {
        session_id: entry_session_id(&entry)?,
        cwd: PathBuf::from(entry_cwd(&entry)?),
        started_at,
    })
}

/// A prompt the user sent, as a Codex rollout records it. Older logs record
/// each prompt twice, as a response item and then as a user_message event.
fn codex_user_prompt(entry: &Map<String, Value>) -> Option<String> {
    let text = match entry.get("type").and_then(Value::as_str) {
        Some("event_msg") => {
            let payload = entry.get("payload")?.as_object()?;
            if payload.get("type").and_then(Value::as_str) != Some("user_message") {
                return None;
            }
            string_field(payload, &["message"])?
        }
        Some("response_item") => message(entry)
            .filter(|(role, _)| *role == ConversationRole::User)
            .map(|(_, text)| text)?,
        _ => return None,
    };
    if skip_user_text(text.trim()) {
        return None;
    }
    submitted_prompt(&text)
}

/// Follows one Codex rollout for the prompts the user submits, reading only
/// what was appended since the last call.
#[derive(Debug)]
pub struct PromptLogReader {
    path: PathBuf,
    offset: u64,
    since_millis: u64,
    last: Option<String>,
}

impl PromptLogReader {
    /// Prompts logged before `since_millis` belong to an earlier life of the
    /// conversation and are left out.
    pub fn new(path: PathBuf, since_millis: u64) -> Self {
        Self {
            path,
            offset: 0,
            since_millis,
            last: None,
        }
    }

    pub fn path(&self) -> &Path {
        &self.path
    }

    pub fn read_new(&mut self) -> PersistResult<Vec<String>> {
        let mut file = File::open(&self.path)?;
        let size = file.metadata()?.len();
        if size < self.offset {
            // Rewritten from scratch; start over.
            self.offset = 0;
        }
        if size == self.offset {
            return Ok(Vec::new());
        }
        let mut start = self.offset;
        let mut skip_partial = false;
        if self.offset == 0 && size > LOG_PREVIEW_BYTES {
            start = size - LOG_PREVIEW_BYTES;
            skip_partial = true;
        }
        file.seek(SeekFrom::Start(start))?;
        let mut bytes = Vec::new();
        file.read_to_end(&mut bytes)?;
        if skip_partial {
            let Some(newline) = memchr::memchr(b'\n', &bytes) else {
                self.offset = size;
                return Ok(Vec::new());
            };
            bytes.drain(..=newline);
            start += newline as u64 + 1;
        }
        // A line still being written waits for the next read.
        let complete = memchr::memrchr(b'\n', &bytes).map_or(0, |index| index + 1);
        bytes.truncate(complete);
        self.offset = start + complete as u64;
        let mut prompts = Vec::new();
        for line in bytes.split(|byte| *byte == b'\n') {
            // Cheap skip for the many lines that cannot hold a prompt.
            if !contains(line, r#""user""#) && !contains(line, "user_message") {
                continue;
            }
            let Ok(Value::Object(entry)) = serde_json::from_slice::<Value>(line) else {
                continue;
            };
            let logged_at = entry
                .get("timestamp")
                .and_then(Value::as_str)
                .and_then(|time| parse_utc_millis(time.as_bytes()));
            if logged_at.is_some_and(|time| time < self.since_millis) {
                continue;
            }
            let Some(prompt) = codex_user_prompt(&entry) else {
                continue;
            };
            if self.last.as_ref() == Some(&prompt) {
                continue;
            }
            self.last = Some(prompt.clone());
            prompts.push(prompt);
        }
        Ok(prompts)
    }
}

fn system_millis(time: SystemTime) -> u64 {
    time.duration_since(UNIX_EPOCH)
        .unwrap_or_default()
        .as_millis() as u64
}

#[cfg(test)]
mod tests {
    use super::parse_utc_millis;

    #[test]
    fn provider_times_parse_as_utc_milliseconds() {
        assert_eq!(parse_utc_millis(b"1970-01-01T00:00:01.5Z"), Some(1_500));
        assert_eq!(
            parse_utc_millis(b"2026-09-29T14:46:47.853Z"),
            Some(1_790_693_207_853)
        );
        assert_eq!(
            parse_utc_millis(b"2024-02-29T00:00:00Z"),
            Some(1_709_164_800_000)
        );
        assert_eq!(parse_utc_millis(b"2026-09-29T14:46:47+02:00"), None);
    }
}
