use std::collections::BTreeMap;

use serde::{Deserialize, Serialize};

#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord, Serialize, Deserialize)]
#[serde(rename_all = "kebab-case")]
pub enum ShortcutAction {
    NewShell,
    CloseSession,
    ToggleSidebar,
    NextSession,
    PreviousSession,
    NextReadySession,
    ReopenPrList,
    ClaudeModelPreset,
}

impl ShortcutAction {
    pub const ALL: [Self; 8] = [
        Self::NewShell,
        Self::CloseSession,
        Self::ToggleSidebar,
        Self::NextSession,
        Self::PreviousSession,
        Self::NextReadySession,
        Self::ReopenPrList,
        Self::ClaudeModelPreset,
    ];

    pub const fn as_str(self) -> &'static str {
        match self {
            Self::NewShell => "new-shell",
            Self::CloseSession => "close-session",
            Self::ToggleSidebar => "toggle-sidebar",
            Self::NextSession => "next-session",
            Self::PreviousSession => "previous-session",
            Self::NextReadySession => "next-ready-session",
            Self::ReopenPrList => "reopen-pr-list",
            Self::ClaudeModelPreset => "claude-model-preset",
        }
    }

    pub const fn label(self) -> &'static str {
        match self {
            Self::NewShell => "New shell",
            Self::CloseSession => "Close selected session",
            Self::ToggleSidebar => "Toggle sidebar",
            Self::NextSession => "Next session",
            Self::PreviousSession => "Previous session",
            Self::NextReadySession => "Next ready session",
            Self::ReopenPrList => "Reopen PR list",
            Self::ClaudeModelPreset => "Switch Claude model",
        }
    }

    pub const fn default_accelerator(self) -> &'static str {
        match self {
            Self::NewShell => "<Control><Shift>grave",
            Self::CloseSession => "<Control><Shift>q",
            Self::ToggleSidebar => "<Control><Shift>backslash",
            Self::NextSession => "<Control><Shift>bracketright",
            Self::PreviousSession => "<Control><Shift>bracketleft",
            Self::NextReadySession => "<Control><Shift>space",
            Self::ReopenPrList => "<Alt><Shift>p",
            Self::ClaudeModelPreset => "<Control><Shift>m",
        }
    }

    pub const fn is_active(self) -> bool {
        matches!(
            self,
            Self::NewShell
                | Self::CloseSession
                | Self::ToggleSidebar
                | Self::NextSession
                | Self::PreviousSession
        )
    }
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub struct ShortcutPreferences {
    pub bindings: BTreeMap<ShortcutAction, String>,
}

impl ShortcutPreferences {
    pub fn binding(&self, action: ShortcutAction) -> &str {
        self.bindings
            .get(&action)
            .map(String::as_str)
            .unwrap_or_else(|| action.default_accelerator())
    }

    pub fn set(&mut self, action: ShortcutAction, accelerator: String) {
        self.bindings.insert(action, accelerator);
    }

    pub fn reset(&mut self, action: ShortcutAction) {
        self.bindings
            .insert(action, action.default_accelerator().to_owned());
    }
}

impl Default for ShortcutPreferences {
    fn default() -> Self {
        Self {
            bindings: ShortcutAction::ALL
                .into_iter()
                .map(|action| (action, action.default_accelerator().to_owned()))
                .collect(),
        }
    }
}
