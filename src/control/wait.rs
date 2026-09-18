use serde_json::Value;

use super::{ControlCommand, GetTextParams, SessionState};

#[derive(Debug, Clone, PartialEq, Eq)]
pub enum WaitCondition {
    SessionExists(String),
    SelectedSession(String),
    SessionState {
        session_id: String,
        state: SessionState,
    },
    TerminalText {
        session_id: String,
        literal: String,
        lines: u32,
    },
}

impl WaitCondition {
    pub fn observation(&self) -> ControlCommand {
        match self {
            Self::TerminalText {
                session_id, lines, ..
            } => ControlCommand::SessionGetText(GetTextParams {
                session_id: session_id.clone(),
                lines: *lines,
            }),
            Self::SessionExists(_) | Self::SelectedSession(_) | Self::SessionState { .. } => {
                ControlCommand::AppGetState
            }
        }
    }

    pub fn is_satisfied(&self, observation: &Value) -> bool {
        match self {
            Self::SessionExists(session_id) => session(observation, session_id).is_some(),
            Self::SelectedSession(session_id) => {
                observation["selectedSessionId"].as_str() == Some(session_id)
            }
            Self::SessionState { session_id, state } => {
                session(observation, session_id).and_then(|session| session["state"].as_str())
                    == Some(state_name(*state))
            }
            Self::TerminalText { literal, .. } => observation["text"]
                .as_str()
                .is_some_and(|text| text.contains(literal)),
        }
    }
}

fn session<'a>(observation: &'a Value, session_id: &str) -> Option<&'a Value> {
    observation["sessions"]
        .as_array()?
        .iter()
        .find(|session| session["id"].as_str() == Some(session_id))
}

const fn state_name(state: SessionState) -> &'static str {
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
