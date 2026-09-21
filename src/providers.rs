use std::cmp::Reverse;
use std::collections::{BTreeMap, BTreeSet};
use std::fs::{self, File};
use std::io::{BufRead, BufReader, Read, Seek, SeekFrom};
use std::path::{Path, PathBuf};
use std::time::{SystemTime, UNIX_EPOCH};

use serde::{Deserialize, Serialize};
use serde_json::{Map, Value};

use crate::control::{CreateSessionParams, SessionKind};
use crate::persist::PersistResult;

const LOG_HEAD_BYTES: u64 = 1024 * 1024;
const LOG_PREVIEW_BYTES: u64 = 4 * 1024 * 1024;
const MAX_MESSAGE_CHARS: usize = 2_000;

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
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct ProviderSession {
    pub provider: AgentProvider,
    pub provider_session_id: String,
    pub name: String,
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
}

impl ProviderDiscovery {
    pub fn new(roots: DiscoveryRoots, scan_limit: usize, max_age_millis: u64) -> Self {
        Self {
            roots,
            scan_limit: scan_limit.clamp(1, 2_000),
            max_age_millis,
        }
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
        let mut sessions = BTreeMap::<(AgentProvider, String), ProviderSession>::new();
        for candidate in candidates {
            let Some(session) = parse_session(&candidate)? else {
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
        Ok(sessions)
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
        for candidate in candidates
            .into_iter()
            .filter(|candidate| candidate.provider == provider)
        {
            let Some(session) = parse_session(&candidate)? else {
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
            return Ok(ProviderPreview {
                session,
                messages,
                is_truncated,
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

fn parse_session(candidate: &LogCandidate) -> PersistResult<Option<ProviderSession>> {
    let Some(facts) = read_session_facts(candidate)? else {
        return Ok(None);
    };
    let provider_session_id = facts.session_id.unwrap_or_else(|| {
        candidate
            .path
            .file_stem()
            .and_then(|value| value.to_str())
            .unwrap_or("session")
            .to_owned()
    });
    if validate_provider_session_id(&provider_session_id).is_err() {
        return Ok(None);
    }
    let cwd = facts.cwd.map(PathBuf::from);
    let name = facts
        .user_text
        .map(|text| first_line(&text, 160))
        .filter(|text| !text.is_empty())
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
    Ok(Some(ProviderSession {
        provider: candidate.provider,
        provider_session_id,
        name,
        cwd,
        created_at: candidate.modified_at,
        last_seen_at: candidate.modified_at,
        log_path: candidate.path.clone(),
    }))
}

#[derive(Default)]
struct SessionFacts {
    session_id: Option<String>,
    cwd: Option<String>,
    user_text: Option<String>,
    has_conversation: bool,
}

impl SessionFacts {
    fn is_complete(&self) -> bool {
        self.session_id.is_some()
            && self.cwd.is_some()
            && self.user_text.is_some()
            && self.has_conversation
    }
}

/// Reads the log head line by line and stops once every fact is known, which
/// is usually within the first few lines. None for empty or ancillary logs.
fn read_session_facts(candidate: &LogCandidate) -> PersistResult<Option<SessionFacts>> {
    let reader = BufReader::new(File::open(&candidate.path)?.take(LOG_HEAD_BYTES));
    let mut facts = SessionFacts::default();
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
        facts.has_conversation |= candidate.provider != AgentProvider::Claude
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
        if facts.user_text.is_none() {
            facts.user_text = entry_user_text(&entry);
        }
        if facts.is_complete() {
            break;
        }
    }
    Ok((!is_first && facts.has_conversation).then_some(facts))
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
                timestamp: entry.get("timestamp").and_then(Value::as_u64),
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
    if value.trim() != value
        || !(1..=240).contains(&value.chars().count())
        || value.chars().any(char::is_whitespace)
        || value.contains('\0')
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

fn system_millis(time: SystemTime) -> u64 {
    time.duration_since(UNIX_EPOCH)
        .unwrap_or_default()
        .as_millis() as u64
}
