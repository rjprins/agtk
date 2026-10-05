//! Turns log entries into the messages and changed files a preview shows.

use std::collections::BTreeSet;
use std::fs::File;
use std::io::{Read, Seek, SeekFrom};
use std::path::{Path, PathBuf};

use serde_json::{Map, Value};

use super::{ConversationMessage, ConversationRole, LOG_PREVIEW_BYTES, MAX_MESSAGE_CHARS};
use crate::persist::PersistResult;
use crate::text::truncate_chars;
use crate::timestamps::parse_utc_millis;

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

pub(super) fn conversation_messages(content: &str) -> Vec<ConversationMessage> {
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

pub(super) fn message(entry: &Map<String, Value>) -> Option<(ConversationRole, String)> {
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

pub(super) fn jsonl_entries(content: &str) -> Vec<Map<String, Value>> {
    content
        .lines()
        .filter_map(|line| serde_json::from_str::<Value>(line.trim()).ok())
        .filter_map(|value| value.as_object().cloned())
        .collect()
}

pub(super) fn string_field(object: &Map<String, Value>, fields: &[&str]) -> Option<String> {
    fields.iter().find_map(|field| {
        object
            .get(*field)
            .and_then(Value::as_str)
            .map(str::trim)
            .filter(|value| !value.is_empty())
            .map(str::to_owned)
    })
}

pub(super) fn skip_user_text(text: &str) -> bool {
    text.starts_with("# AGENTS.md")
        || text.starts_with("# INSTRUCTIONS")
        || text.starts_with('<')
        || text.contains("<environment_context>")
}

pub(super) fn first_line(text: &str, max_chars: usize) -> String {
    let line = text.lines().next().unwrap_or(text).trim();
    let line = line
        .strip_prefix("Please ")
        .or_else(|| line.strip_prefix("please "))
        .unwrap_or(line);
    truncate_chars(line, max_chars)
}

pub(super) fn validate_provider_session_id(value: &str) -> PersistResult<()> {
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

pub(super) struct TailContent {
    pub(super) text: String,
    pub(super) is_truncated: bool,
}

pub(super) fn read_tail(path: &Path, limit: u64) -> PersistResult<TailContent> {
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
