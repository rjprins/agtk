use std::fmt;
use std::str::FromStr;

use serde::{Deserialize, Serialize};

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "lowercase")]
pub enum SessionKind {
    Shell,
    Codex,
    Claude,
    Gemini,
    Custom,
}

impl SessionKind {
    /// The name the protocol, the CLI and the launch dialog use for this kind.
    pub const fn as_str(self) -> &'static str {
        match self {
            Self::Shell => "shell",
            Self::Codex => "codex",
            Self::Claude => "claude",
            Self::Gemini => "gemini",
            Self::Custom => "custom",
        }
    }
}

impl fmt::Display for SessionKind {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        formatter.write_str(self.as_str())
    }
}

impl FromStr for SessionKind {
    type Err = String;

    fn from_str(value: &str) -> Result<Self, Self::Err> {
        [
            Self::Shell,
            Self::Codex,
            Self::Claude,
            Self::Gemini,
            Self::Custom,
        ]
        .into_iter()
        .find(|kind| kind.as_str() == value)
        .ok_or_else(|| format!("unknown session kind: {value}"))
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "lowercase")]
pub enum SessionState {
    Running,
    Busy,
    Ready,
    Waiting,
    /// Nothing is running, or the finished turn has been viewed.
    Idle,
    Exited,
    Reconnecting,
}

#[cfg(test)]
mod tests {
    use super::SessionKind;

    #[test]
    fn kind_names_round_trip_and_match_serde() {
        for kind in [
            SessionKind::Shell,
            SessionKind::Codex,
            SessionKind::Claude,
            SessionKind::Gemini,
            SessionKind::Custom,
        ] {
            assert_eq!(kind.as_str().parse::<SessionKind>(), Ok(kind));
            assert_eq!(
                serde_json::to_value(kind).unwrap(),
                serde_json::json!(kind.as_str())
            );
        }
        assert!("bash".parse::<SessionKind>().is_err());
    }
}
