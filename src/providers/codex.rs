//! Codex session metadata and the reader that follows a live Codex prompt log.

use std::fs::File;
use std::io::{BufRead, BufReader, Read, Seek, SeekFrom};
use std::path::{Path, PathBuf};

use serde_json::{Map, Value};

use super::conversation::{message, skip_user_text, string_field};
use super::facts::{contains, entry_cwd, entry_session_id, parse_utc_millis};
use super::{ConversationRole, LOG_HEAD_BYTES, LOG_PREVIEW_BYTES};
use crate::history::submitted_prompt;
use crate::persist::PersistResult;

pub(super) struct CodexSessionMeta {
    pub(super) session_id: String,
    pub(super) cwd: PathBuf,
    pub(super) started_at: u64,
}

/// The first line of a rollout names the session, its directory, and when
/// Codex started. None for ancillary or foreign files.
pub(super) fn codex_session_meta(path: &Path) -> Option<CodexSessionMeta> {
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
