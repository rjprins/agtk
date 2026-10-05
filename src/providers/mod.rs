//! The agent CLIs agtk launches, and the conversations they log.

use std::fs::File;
use std::io::Read;
use std::path::PathBuf;

use serde::{Deserialize, Serialize};

use crate::control::CreateSessionParams;

use crate::persist::PersistResult;
use crate::session::SessionKind;

mod codex;
mod conversation;
mod discovery;
mod facts;

pub use codex::PromptLogReader;
pub use conversation::recent_mutated_paths;
pub use discovery::ProviderDiscovery;

const LOG_HEAD_BYTES: u64 = 1024 * 1024;
const LOG_PREVIEW_BYTES: u64 = 4 * 1024 * 1024;
const MAX_MESSAGE_CHARS: usize = 2_000;
const MAX_TITLE_CHARS: usize = 160;

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

    /// The provider whose conversations a session of this kind resumes, if any.
    pub const fn from_kind(kind: SessionKind) -> Option<Self> {
        match kind {
            SessionKind::Codex => Some(Self::Codex),
            SessionKind::Claude => Some(Self::Claude),
            _ => None,
        }
    }

    pub fn resume_args(self, provider_session_id: &str) -> Vec<String> {
        match self {
            Self::Codex => vec!["resume".to_owned(), provider_session_id.to_owned()],
            Self::Claude => vec!["--resume".to_owned(), provider_session_id.to_owned()],
        }
    }

    /// Starts a new conversation from a copy of `provider_session_id` and
    /// leaves the original alone, so both can carry on.
    pub fn fork_plan(self, provider_session_id: &str) -> PersistResult<ForkPlan> {
        Ok(match self {
            Self::Codex => ForkPlan {
                args: vec!["fork".to_owned(), provider_session_id.to_owned()],
                conversation_id: None,
            },
            Self::Claude => {
                let conversation_id = random_uuid()?;
                ForkPlan {
                    args: vec![
                        "--resume".to_owned(),
                        provider_session_id.to_owned(),
                        "--fork-session".to_owned(),
                        "--session-id".to_owned(),
                        conversation_id.clone(),
                    ],
                    conversation_id: Some(conversation_id),
                }
            }
        })
    }
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ForkPlan {
    pub args: Vec<String>,
    /// The copy's conversation, when agtk chooses it. Codex picks its own,
    /// which agtk finds in the logs like any other Codex conversation.
    pub conversation_id: Option<String>,
}

/// A random version 4 UUID, the form Claude requires for a session ID.
fn random_uuid() -> PersistResult<String> {
    let mut bytes = [0_u8; 16];
    File::open("/dev/urandom")?.read_exact(&mut bytes)?;
    bytes[6] = (bytes[6] & 0x0f) | 0x40;
    bytes[8] = (bytes[8] & 0x3f) | 0x80;
    let hex = bytes
        .iter()
        .map(|byte| format!("{byte:02x}"))
        .collect::<String>();
    Ok(format!(
        "{}-{}-{}-{}-{}",
        &hex[..8],
        &hex[8..12],
        &hex[12..16],
        &hex[16..20],
        &hex[20..]
    ))
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct ProviderSession {
    pub provider: AgentProvider,
    pub provider_session_id: String,
    /// The best title: the given name, the provider's title, the command, or the first prompt.
    pub name: String,
    /// A name the user gave with the provider's rename command.
    pub custom_title: Option<String>,
    /// The title the provider generated.
    pub ai_title: Option<String>,
    pub first_prompt: Option<String>,
    pub last_prompt: Option<String>,
    pub branch: Option<String>,
    pub prompt_count: u32,
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
    /// Files the agent edited, newest first.
    pub files_changed: Vec<PathBuf>,
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

#[cfg(test)]
mod tests {
    use crate::timestamps::parse_utc_millis;

    #[test]
    fn provider_times_parse_as_utc_milliseconds() {
        assert_eq!(parse_utc_millis(b"1970-01-01T00:00:01.5Z"), Some(1_500));
        assert_eq!(
            parse_utc_millis(b"2026-09-29T14:46:47.853Z"),
            Some(1_790_693_207_853)
        );
        assert_eq!(
            parse_utc_millis(b"2024-02-29T00:00:00Z"),
            Some(1_709_164_800_000)
        );
        assert_eq!(parse_utc_millis(b"2026-09-29T14:46:47+02:00"), None);
    }

    #[test]
    fn provider_times_reject_invalid_dates_clocks_and_fraction_digits() {
        for invalid in [
            "2026-09-29T24:00:00Z",
            "2026-02-29T00:00:00Z",
            "2026-09-31T00:00:00Z",
            "2026-09-29T00:60:00Z",
            "2026-09-29T00:00:61Z",
            "2026-09-29T00:00:00.123junkZ",
            "9223372036854775807-01-01T00:00:00Z",
        ] {
            assert_eq!(parse_utc_millis(invalid.as_bytes()), None, "{invalid}");
        }
    }
}
