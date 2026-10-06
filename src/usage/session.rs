//! Incremental token accounting from local agent transcripts.

use std::collections::HashMap;
use std::fs::File;
use std::io::{self, Read, Seek, SeekFrom};
use std::os::unix::fs::MetadataExt;
use std::path::PathBuf;
use std::time::{Duration, Instant};

use crate::providers::AgentProvider;
use serde_json::Value;

const CHUNK: u64 = 4 * 1024 * 1024;

#[derive(Debug, Clone, Default)]
pub struct SessionUsage {
    pub input: u64,
    pub output: u64,
    /// Already included in input; never add this a second time.
    pub cached: u64,
    pub context_used: Option<u64>,
    pub context_capacity: Option<u64>,
    pub available: bool,
    pub catching_up: bool,
    pub incomplete: bool,
}

impl SessionUsage {
    pub fn total(&self) -> u64 {
        self.input.saturating_add(self.output)
    }

    pub fn context_remaining(&self) -> Option<f64> {
        let capacity = self.context_capacity.filter(|value| *value > 0)?;
        Some(100.0 * capacity.saturating_sub(self.context_used?) as f64 / capacity as f64)
    }
}

pub struct UsageReader {
    path: PathBuf,
    provider: AgentProvider,
    offset: u64,
    identity: Option<(u64, u64)>,
    modified: Option<std::time::SystemTime>,
    known_len: u64,
    checkpoint: Vec<u8>,
    verify_after: Instant,
    needs_verification: bool,
    pending: Vec<u8>,
    skipping_line: bool,
    messages: HashMap<String, (u64, u64, u64)>,
    usage: SessionUsage,
}

impl UsageReader {
    pub fn new(path: PathBuf, provider: AgentProvider) -> Self {
        Self {
            path,
            provider,
            offset: 0,
            identity: None,
            modified: None,
            known_len: 0,
            checkpoint: Vec::new(),
            verify_after: Instant::now() + Duration::from_secs(60),
            needs_verification: false,
            pending: Vec::new(),
            skipping_line: false,
            messages: HashMap::new(),
            usage: SessionUsage::default(),
        }
    }

    pub fn read_new(&mut self) -> io::Result<SessionUsage> {
        let mut file = File::open(&self.path)?;
        let metadata = file.metadata()?;
        let identity = (metadata.dev(), metadata.ino());
        let changed = metadata.modified().ok() != self.modified || metadata.len() != self.known_len;
        // Cheap checkpoints cannot detect edits to the middle of a growing file.
        // Reconcile changed logs once a minute through the same bounded reader,
        // rather than rescanning every historical byte on every append.
        let verify = self.needs_verification && Instant::now() >= self.verify_after;
        // A same-length write is a rewrite. For growing files, check the consumed
        // prefix and append boundary so a truncate-and-rewrite doesn't mix totals.
        let rewritten = changed
            && self.offset > 0
            && (metadata.len() == self.known_len
                || checkpoint(&mut file, self.offset)? != self.checkpoint);
        if self.identity.is_some_and(|previous| previous != identity)
            || metadata.len() < self.offset
            || rewritten
            || verify
        {
            *self = Self::new(self.path.clone(), self.provider);
        } else if changed && self.offset > 0 {
            self.needs_verification = true;
        }
        self.identity = Some(identity);
        self.modified = metadata.modified().ok();
        self.known_len = metadata.len();
        file.seek(SeekFrom::Start(self.offset))?;
        let mut chunk = Vec::new();
        (&mut file).take(CHUNK).read_to_end(&mut chunk)?;
        self.offset += chunk.len() as u64;
        self.usage.catching_up = self.offset < metadata.len();
        let mut start = 0;
        for end in memchr::memchr_iter(b'\n', &chunk) {
            if !self.skipping_line {
                self.pending.extend_from_slice(&chunk[start..end]);
                // Avoid JSON allocation for unrelated and enormous tool output.
                let needle: &[u8] = match self.provider {
                    AgentProvider::Codex => b"token_count",
                    AgentProvider::Claude => b"\"usage\"",
                };
                let relevant = memchr::memmem::find(&self.pending, needle).is_some();
                if relevant && let Ok(entry) = serde_json::from_slice::<Value>(&self.pending) {
                    self.observe(&entry);
                }
            }
            self.pending.clear();
            self.skipping_line = false;
            start = end + 1;
        }
        if !self.skipping_line {
            self.pending.extend_from_slice(&chunk[start..]);
            if self.pending.len() > CHUNK as usize {
                self.pending.clear();
                self.skipping_line = true;
                self.usage.incomplete = true;
            }
        }
        self.checkpoint = checkpoint(&mut file, self.offset)?;
        Ok(self.usage.clone())
    }

    fn observe(&mut self, entry: &Value) {
        match self.provider {
            AgentProvider::Codex => {
                if entry["type"] != "event_msg" || entry["payload"]["type"] != "token_count" {
                    return;
                }
                let info = &entry["payload"]["info"];
                let totals = &info["total_token_usage"];
                if let (Some(input), Some(output)) = (
                    totals["input_tokens"].as_u64(),
                    totals["output_tokens"].as_u64(),
                ) {
                    self.usage.input = input;
                    self.usage.output = output;
                    self.usage.cached = count(totals, "cached_input_tokens").min(input);
                    self.usage.available = true;
                }
                if let Some(used) = info["last_token_usage"]["total_tokens"].as_u64() {
                    self.usage.context_used = Some(used);
                    self.usage.context_capacity = info["model_context_window"].as_u64();
                }
            }
            AgentProvider::Claude => {
                if entry["type"] != "assistant" || entry["isSidechain"] == true {
                    return;
                }
                let message = &entry["message"];
                let usage = &message["usage"];
                if !usage.is_object() {
                    return;
                }
                let Some(id) = message["id"].as_str() else {
                    return;
                };
                let cached = count(usage, "cache_read_input_tokens");
                let input = count(usage, "input_tokens")
                    .saturating_add(cached)
                    .saturating_add(count(usage, "cache_creation_input_tokens"));
                let output = count(usage, "output_tokens");
                let previous = self.messages.entry(id.to_owned()).or_default();
                let next = (
                    previous.0.max(input),
                    previous.1.max(output),
                    previous.2.max(cached),
                );
                self.usage.input = self.usage.input.saturating_add(next.0 - previous.0);
                self.usage.output = self.usage.output.saturating_add(next.1 - previous.1);
                self.usage.cached = self.usage.cached.saturating_add(next.2 - previous.2);
                *previous = next;
                self.usage.available = true;
                self.usage.context_used = Some(input.saturating_add(output));
                // Capacity varies by model, plan and flags. Never infer it from the name.
                self.usage.context_capacity = usage["context_window"].as_u64();
            }
        }
    }
}

fn count(value: &Value, key: &str) -> u64 {
    value[key].as_u64().unwrap_or(0)
}

fn checkpoint(file: &mut File, offset: u64) -> io::Result<Vec<u8>> {
    let mut bytes = Vec::new();
    file.seek(SeekFrom::Start(0))?;
    (&mut *file).take(offset.min(128)).read_to_end(&mut bytes)?;
    file.seek(SeekFrom::Start(offset.saturating_sub(128)))?;
    (&mut *file).take(offset.min(128)).read_to_end(&mut bytes)?;
    Ok(bytes)
}

#[cfg(test)]
mod tests {
    use super::*;
    use serde_json::json;
    use std::io::Write;

    #[test]
    fn in_place_rewrites_discard_the_previous_totals() {
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("log");
        let row = |input| {
            json!({"type":"assistant","message":{"id":"a","usage":{
            "input_tokens":input,"output_tokens":0}}})
            .to_string()
                + "\n"
        };
        std::fs::write(&path, row(90)).unwrap();
        let mut reader = UsageReader::new(path.clone(), AgentProvider::Claude);
        assert_eq!(reader.read_new().unwrap().total(), 90);
        std::fs::write(&path, row(10)).unwrap();
        assert_eq!(reader.read_new().unwrap().total(), 10);
        std::fs::write(&path, row(20) + &"{}\n".repeat(100)).unwrap();
        assert_eq!(reader.read_new().unwrap().total(), 20);
    }

    #[test]
    fn codex_usage_survives_a_large_tail_of_tool_output() {
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("log");
        let row = json!({"type":"event_msg","payload":{"type":"token_count","info":{
            "total_token_usage":{"input_tokens":100,"output_tokens":10}}}})
        .to_string()
            + "\n";
        std::fs::write(&path, row + &"{}\n".repeat(CHUNK as usize / 3 + 100)).unwrap();
        let mut reader = UsageReader::new(path, AgentProvider::Codex);
        let mut usage = reader.read_new().unwrap();
        while usage.catching_up {
            usage = reader.read_new().unwrap();
        }
        assert_eq!(usage.total(), 110);
    }

    #[test]
    fn periodic_verification_reconciles_middle_edits_in_a_growing_log() {
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("log");
        let padding = "{}\n".repeat(100);
        let row = |input| {
            padding.clone()
                + &json!({"type":"assistant","message":{"id":"a","usage":{
            "input_tokens":input,"output_tokens":0}}})
                .to_string()
                + "\n"
                + &padding
        };
        std::fs::write(&path, row(90)).unwrap();
        let mut reader = UsageReader::new(path.clone(), AgentProvider::Claude);
        assert_eq!(reader.read_new().unwrap().total(), 90);
        std::fs::write(&path, row(10) + "{}\n").unwrap();
        reader.read_new().unwrap();
        reader.verify_after = Instant::now();
        assert_eq!(reader.read_new().unwrap().total(), 10);
    }

    #[test]
    fn codex_totals_are_snapshots_and_partial_lines_wait() {
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("log");
        let entry = json!({"type":"event_msg","payload":{"type":"token_count","info":{
            "total_token_usage":{"input_tokens":1000,"output_tokens":200,"cached_input_tokens":800},
            "last_token_usage":{"total_tokens":250},"model_context_window":1000}}})
        .to_string();
        std::fs::write(&path, &entry).unwrap();
        let mut reader = UsageReader::new(path.clone(), AgentProvider::Codex);
        assert!(!reader.read_new().unwrap().available);
        let mut file = std::fs::OpenOptions::new()
            .append(true)
            .open(&path)
            .unwrap();
        writeln!(file, "\n{entry}").unwrap();
        let usage = reader.read_new().unwrap();
        assert_eq!(usage.total(), 1200);
        assert_eq!(usage.cached, 800);
        assert_eq!(usage.context_remaining(), Some(75.0));
        assert_eq!(reader.read_new().unwrap().total(), 1200);
        std::fs::write(&path, "{}\n").unwrap();
        assert!(!reader.read_new().unwrap().available);
    }

    #[test]
    fn claude_counts_streamed_messages_once_and_includes_cache_in_input() {
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("log");
        let row = |id, output| {
            json!({"type":"assistant","message":{"id":id,"usage":{
            "input_tokens":10,"cache_read_input_tokens":100,"cache_creation_input_tokens":20,
            "output_tokens":output}}})
            .to_string()
        };
        std::fs::write(
            &path,
            format!("{}\n{}\n{}\n", row("a", 5), row("b", 3), row("a", 15)),
        )
        .unwrap();
        let mut reader = UsageReader::new(path, AgentProvider::Claude);
        let usage = reader.read_new().unwrap();
        assert_eq!(usage.total(), 278);
        assert_eq!(usage.cached, 200);
        assert_eq!(usage.context_remaining(), None);
    }

    #[test]
    fn replacing_a_log_resets_accounting_even_when_its_size_grows() {
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("log");
        let row = |input| {
            json!({"type":"event_msg","payload":{"type":"token_count","info":{
            "total_token_usage":{"input_tokens":input,"output_tokens":1}}}})
            .to_string()
                + "\n"
        };
        std::fs::write(&path, row(10)).unwrap();
        let mut reader = UsageReader::new(path.clone(), AgentProvider::Codex);
        assert_eq!(reader.read_new().unwrap().total(), 11);
        let replacement = dir.path().join("replacement");
        std::fs::write(&replacement, row(100)).unwrap();
        std::fs::rename(replacement, path).unwrap();
        assert_eq!(reader.read_new().unwrap().total(), 101);
    }
}
