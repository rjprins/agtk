use std::collections::BTreeMap;

use serde::{Deserialize, Serialize};

#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord, Serialize, Deserialize)]
#[serde(rename_all = "kebab-case")]
pub enum ShortcutAction {
    // Saved bindings still use the key of the shell shortcut this replaced.
    #[serde(alias = "new-shell")]
    LaunchInProject,
    CloseSession,
    ToggleSidebar,
    NextSession,
    PreviousSession,
    NextReadySession,
    LastSession,
    ResumeSession,
    ReopenPrList,
    ClaudeModelPreset,
    IncreaseFontSize,
    DecreaseFontSize,
}

impl ShortcutAction {
    pub const ALL: [Self; 12] = [
        Self::LaunchInProject,
        Self::CloseSession,
        Self::ToggleSidebar,
        Self::NextSession,
        Self::PreviousSession,
        Self::NextReadySession,
        Self::LastSession,
        Self::ResumeSession,
        Self::ReopenPrList,
        Self::ClaudeModelPreset,
        Self::IncreaseFontSize,
        Self::DecreaseFontSize,
    ];

    pub const fn as_str(self) -> &'static str {
        match self {
            Self::LaunchInProject => "launch-in-project",
            Self::CloseSession => "close-session",
            Self::ToggleSidebar => "toggle-sidebar",
            Self::NextSession => "next-session",
            Self::PreviousSession => "previous-session",
            Self::NextReadySession => "next-ready-session",
            Self::LastSession => "last-session",
            Self::ResumeSession => "resume-session",
            Self::ReopenPrList => "reopen-pr-list",
            Self::ClaudeModelPreset => "claude-model-preset",
            Self::IncreaseFontSize => "increase-font-size",
            Self::DecreaseFontSize => "decrease-font-size",
        }
    }

    pub const fn label(self) -> &'static str {
        match self {
            Self::LaunchInProject => "Launch in current project",
            Self::CloseSession => "Close selected session",
            Self::ToggleSidebar => "Toggle sidebar",
            Self::NextSession => "Next session",
            Self::PreviousSession => "Previous session",
            Self::NextReadySession => "Next ready session",
            Self::LastSession => "Back to last visited session",
            Self::ResumeSession => "Resume a closed agent session",
            Self::ReopenPrList => "Reopen PR list",
            Self::ClaudeModelPreset => "Switch Claude model",
            Self::IncreaseFontSize => "Increase UI and terminal font size",
            Self::DecreaseFontSize => "Decrease UI and terminal font size",
        }
    }

    pub const fn default_accelerator(self) -> &'static str {
        match self {
            Self::LaunchInProject => "<Control><Shift>grave",
            Self::CloseSession => "<Control><Shift>q",
            Self::ToggleSidebar => "<Control><Shift>backslash",
            Self::NextSession => "<Control><Shift>bracketright",
            Self::PreviousSession => "<Control><Shift>bracketleft",
            Self::NextReadySession => "<Control><Shift>space",
            Self::LastSession => "<Control><Shift>l",
            Self::ResumeSession => "<Control><Shift>r",
            Self::ReopenPrList => "<Alt><Shift>p",
            Self::ClaudeModelPreset => "<Control><Shift>m",
            Self::IncreaseFontSize => "<Control>plus",
            Self::DecreaseFontSize => "<Control>minus",
        }
    }

    pub const fn is_active(self) -> bool {
        matches!(
            self,
            Self::LaunchInProject
                | Self::CloseSession
                | Self::ToggleSidebar
                | Self::NextSession
                | Self::PreviousSession
                | Self::NextReadySession
                | Self::LastSession
                | Self::ResumeSession
                | Self::ReopenPrList
                | Self::ClaudeModelPreset
                | Self::IncreaseFontSize
                | Self::DecreaseFontSize
        )
    }
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
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
