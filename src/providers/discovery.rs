//! Finds agent conversation logs and caches the facts read from them.

use std::cmp::Reverse;
use std::collections::{BTreeMap, BTreeSet};
use std::fs;
use std::path::{Path, PathBuf};
use std::sync::{LazyLock, Mutex};
use std::time::{SystemTime, UNIX_EPOCH};

use serde::{Deserialize, Serialize};

use super::codex::codex_session_meta;
use super::conversation::{
    conversation_messages, jsonl_entries, read_tail, recent_mutated_paths, string_field,
    validate_provider_session_id,
};
use super::facts::{LogFacts, read_log_facts, session_from_facts};
use super::{AgentProvider, DiscoveryRoots, LOG_PREVIEW_BYTES, ProviderPreview, ProviderSession};
use crate::persist::PersistResult;

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
            let Some(session) = self.parse_session_or_skip(&candidate, &codex_titles) else {
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
            let Some(session) = self.parse_session_or_skip(&candidate, &codex_titles) else {
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

    /// A log can vanish or turn unreadable between the scan and the read, since
    /// the agents rewrite and prune them, so one bad log skips only itself.
    fn parse_session_or_skip(
        &self,
        candidate: &LogCandidate,
        codex_titles: &BTreeMap<String, String>,
    ) -> Option<ProviderSession> {
        self.parse_session(candidate, codex_titles)
            .unwrap_or_else(|error| {
                eprintln!("agtk: skipping {}: {error}", candidate.path.display());
                None
            })
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
pub(super) struct LogCandidate {
    pub(super) provider: AgentProvider,
    pub(super) path: PathBuf,
    pub(super) modified_at: u64,
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

fn system_millis(time: SystemTime) -> u64 {
    time.duration_since(UNIX_EPOCH)
        .unwrap_or_default()
        .as_millis() as u64
}
