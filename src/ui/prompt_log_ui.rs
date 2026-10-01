//! Prompt history for Codex, read from its rollout log.
//!
//! Codex has no hook that reports a submitted prompt, but it appends every
//! prompt to its rollout. Each poll reads what was appended since the last one
//! and lets the logged text replace the keystroke guess for that turn.

use std::collections::BTreeSet;
use std::time::{Duration, Instant};

use super::*;
use crate::providers::PromptLogReader;

/// Codex creates the rollout at the first prompt, so a miss retries with backoff.
const FIRST_RETRY: Duration = Duration::from_secs(1);
const MAX_RETRY: Duration = Duration::from_secs(60);

#[derive(Debug, Default)]
pub(super) struct PromptLogFollower {
    reader: Option<PromptLogReader>,
    /// The reader is out on the I/O worker, or a search for the log is running.
    in_flight: bool,
    retry_at: Option<Instant>,
    misses: u32,
}

impl PromptLogFollower {
    fn note_miss(&mut self, now: Instant) {
        self.misses = self.misses.saturating_add(1);
        let delay = FIRST_RETRY
            .checked_mul(1 << (self.misses - 1).min(6))
            .map_or(MAX_RETRY, |delay| delay.min(MAX_RETRY));
        self.retry_at = Some(now + delay);
    }
}

impl Workspace {
    /// One poll of every Codex session: read new prompts from a known log, or
    /// look for the log of a session that is busy without one.
    pub(super) fn follow_prompt_logs(&self) {
        let ids = self.sessions.borrow().keys().cloned().collect::<Vec<_>>();
        let now = Instant::now();
        for id in ids {
            let action = {
                let mut sessions = self.sessions.borrow_mut();
                let Some(session) = sessions.get_mut(&id) else {
                    continue;
                };
                if session.record.kind != SessionKind::Codex
                    || session.record.state == SessionState::Exited
                    || session.prompt_log.in_flight
                {
                    continue;
                }
                if let Some(reader) = session.prompt_log.reader.take() {
                    session.prompt_log.in_flight = true;
                    Action::Read(reader)
                } else if session.record.state == SessionState::Busy
                    && session.prompt_log.retry_at.is_none_or(|at| at <= now)
                {
                    let Some(cwd) = session.record.cwd.clone() else {
                        continue;
                    };
                    session.prompt_log.in_flight = true;
                    Action::Locate {
                        conversation_id: session.record.conversation_id.clone(),
                        cwd,
                        launched_at: session.record.created_at,
                    }
                } else {
                    continue;
                }
            };
            match action {
                Action::Read(reader) => self.read_prompt_log(id, reader),
                Action::Locate {
                    conversation_id,
                    cwd,
                    launched_at,
                } => self.locate_prompt_log(id, conversation_id, cwd, launched_at),
            }
        }
    }

    fn read_prompt_log(&self, id: String, mut reader: PromptLogReader) {
        self.run_io(
            move || {
                let prompts = reader.read_new();
                Ok((reader, prompts))
            },
            move |workspace, result| {
                let Ok((reader, prompts)) = result else {
                    return;
                };
                let prompts = {
                    let mut sessions = workspace.sessions.borrow_mut();
                    let Some(session) = sessions.get_mut(&id) else {
                        return;
                    };
                    session.prompt_log.in_flight = false;
                    match prompts {
                        Ok(prompts) => {
                            session.prompt_log.reader = Some(reader);
                            prompts
                        }
                        Err(error) => {
                            // The log went away; look for it again later.
                            eprintln!("agtk: prompt log {}: {error}", reader.path().display());
                            session.prompt_log.note_miss(Instant::now());
                            Vec::new()
                        }
                    }
                };
                for prompt in prompts {
                    workspace.record_submitted_prompt(&id, prompt);
                }
            },
        );
    }

    fn locate_prompt_log(
        &self,
        id: String,
        conversation_id: Option<String>,
        cwd: PathBuf,
        launched_at: u64,
    ) {
        let discovery = self.provider_discovery();
        // Logs other live Codex sessions own are not this one's.
        let claimed = self
            .sessions
            .borrow()
            .values()
            .filter(|session| {
                session.record.id != id && session.record.state != SessionState::Exited
            })
            .filter_map(|session| session.record.conversation_id.clone())
            .collect::<BTreeSet<_>>();
        self.run_slow(
            move || {
                discovery.locate_codex_log(conversation_id.as_deref(), &cwd, launched_at, &claimed)
            },
            move |workspace, result| {
                let changed_record = {
                    let mut sessions = workspace.sessions.borrow_mut();
                    let Some(session) = sessions.get_mut(&id) else {
                        return;
                    };
                    session.prompt_log.in_flight = false;
                    match result {
                        Ok(Some((conversation_id, path))) => {
                            session.prompt_log.reader =
                                Some(PromptLogReader::new(path, session.record.created_at));
                            session.prompt_log.misses = 0;
                            session.prompt_log.retry_at = None;
                            // The log also tells which conversation to resume later.
                            if session.record.conversation_id.is_none() {
                                session.record.conversation_id = Some(conversation_id);
                                Some(session.record.clone())
                            } else {
                                None
                            }
                        }
                        Ok(None) => {
                            session.prompt_log.note_miss(Instant::now());
                            None
                        }
                        Err(error) => {
                            eprintln!("agtk: could not find the Codex log: {error}");
                            session.prompt_log.note_miss(Instant::now());
                            None
                        }
                    }
                };
                if let Some(record) = changed_record {
                    workspace.persist_record(record);
                }
            },
        );
    }
}

enum Action {
    Read(PromptLogReader),
    Locate {
        conversation_id: Option<String>,
        cwd: PathBuf,
        launched_at: u64,
    },
}
