//! Reads a log's identity, titles and prompts while scanning as little as possible.

use std::collections::BTreeMap;
use std::fs::File;
use std::io::{BufRead, BufReader, Read};
use std::path::PathBuf;
use std::sync::LazyLock;

use memchr::memmem;
use serde::{Deserialize, Serialize};
use serde_json::{Map, Value};

use super::conversation::{
    first_line, message, skip_user_text, string_field, validate_provider_session_id,
};
use super::discovery::LogCandidate;
use super::{AgentProvider, ConversationRole, LOG_HEAD_BYTES, MAX_TITLE_CHARS, ProviderSession};
use crate::persist::PersistResult;
use crate::text::truncate_chars;
use crate::timestamps::parse_utc_millis;

/// What one log says about its session. Cached, so keep it small.
#[derive(Debug, Clone, Default, Serialize, Deserialize)]
pub(super) struct LogFacts {
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

pub(super) fn session_from_facts(
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
pub(super) fn read_log_facts(candidate: &LogCandidate) -> PersistResult<Option<LogFacts>> {
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

pub(super) fn contains(line: &[u8], needle: &str) -> bool {
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

pub(super) fn entry_session_id(entry: &Map<String, Value>) -> Option<String> {
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

pub(super) fn entry_cwd(entry: &Map<String, Value>) -> Option<String> {
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
