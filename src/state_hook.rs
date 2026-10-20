//! A command of the user's that agtk runs when an agent changes state, so
//! something outside agtk hears about a finished turn without polling.

use std::ffi::OsString;
use std::path::{Path, PathBuf};
use std::process::Command;
use std::time::Duration;

use serde::{Deserialize, Serialize};

use crate::command_runner::run_bounded;
use crate::control::AgentSignalState;
use crate::session::{SessionKind, SessionState};
use crate::text::truncate_chars;

pub const PREFERENCE_KEY: &str = "stateHook";
pub const MAX_COMMAND_CHARS: usize = 4_096;
/// A hook that hangs is killed, with its process group, after this long.
pub const TIMEOUT: Duration = Duration::from_secs(30);
const OUTPUT_LIMIT: usize = 64 * 1024;

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct StateHookPreferences {
    /// Run with `sh -c`. No command, no hook.
    pub command: Option<String>,
    #[serde(default = "default_states")]
    pub states: Vec<AgentSignalState>,
}

impl Default for StateHookPreferences {
    fn default() -> Self {
        Self {
            command: None,
            states: default_states(),
        }
    }
}

/// The states that need someone: a finished turn and a blocked one.
pub fn default_states() -> Vec<AgentSignalState> {
    vec![AgentSignalState::Ready, AgentSignalState::Waiting]
}

impl StateHookPreferences {
    /// The command to run for a session that just entered `state`, if any.
    pub fn command_for(&self, state: SessionState) -> Option<&str> {
        let state = match state {
            SessionState::Busy => AgentSignalState::Busy,
            SessionState::Ready => AgentSignalState::Ready,
            SessionState::Waiting => AgentSignalState::Waiting,
            SessionState::Idle => AgentSignalState::Idle,
            SessionState::Running | SessionState::Exited | SessionState::Reconnecting => {
                return None;
            }
        };
        self.command
            .as_deref()
            .filter(|_| self.states.contains(&state))
    }
}

/// One session's move from one state to another.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct StateChange {
    pub session_id: String,
    pub name: String,
    pub kind: SessionKind,
    pub cwd: Option<PathBuf>,
    pub conversation_id: Option<String>,
    pub old_state: SessionState,
    pub new_state: SessionState,
}

/// Where the hook can reach agtk, for example with `agtkctl`.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct HookContext {
    pub instance: String,
    pub control_socket: PathBuf,
    pub path: Option<OsString>,
}

/// The hook's process: `sh -c command`, with the change in its environment.
/// Unknown values are empty.
pub fn hook_command(command: &str, change: &StateChange, context: &HookContext) -> Command {
    let mut process = Command::new("/bin/sh");
    process
        .arg("-c")
        .arg(command)
        .env("AGTK_INSTANCE", &context.instance)
        .env("AGTK_CONTROL_SOCKET", &context.control_socket)
        .env("AGTK_SESSION_ID", &change.session_id)
        .env("AGTK_SESSION_NAME", &change.name)
        .env("AGTK_SESSION_KIND", change.kind.as_str())
        .env(
            "AGTK_SESSION_CWD",
            change.cwd.as_deref().unwrap_or(Path::new("")),
        )
        .env(
            "AGTK_CONVERSATION_ID",
            change.conversation_id.as_deref().unwrap_or_default(),
        )
        .env("AGTK_OLD_STATE", state_name(change.old_state))
        .env("AGTK_NEW_STATE", state_name(change.new_state));
    if let Some(path) = &context.path {
        process.env("PATH", path);
    }
    process
}

/// Runs the hook to completion. Err describes a failure for the log.
pub fn run_hook(command: &str, change: &StateChange, context: &HookContext) -> Result<(), String> {
    let output = run_bounded(
        hook_command(command, change, context),
        TIMEOUT,
        OUTPUT_LIMIT,
    )
    .map_err(|error| error.to_string())?;
    if output.status.success() {
        return Ok(());
    }
    let stderr = output.stderr.trim();
    Err(if stderr.is_empty() {
        output.status.to_string()
    } else {
        format!("{}: {}", output.status, truncate_chars(stderr, 500))
    })
}

pub fn state_name(state: SessionState) -> &'static str {
    match state {
        SessionState::Running => "running",
        SessionState::Busy => "busy",
        SessionState::Ready => "ready",
        SessionState::Waiting => "waiting",
        SessionState::Idle => "idle",
        SessionState::Exited => "exited",
        SessionState::Reconnecting => "reconnecting",
    }
}
