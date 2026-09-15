use std::path::PathBuf;

use serde::{Deserialize, Serialize};

use crate::control::SessionKind;

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub struct QuickLaunchPreferences {
    pub kind: SessionKind,
    #[serde(default)]
    pub args: Vec<String>,
    pub cwd: Option<PathBuf>,
    pub project_root: Option<PathBuf>,
    pub worktree_path: Option<PathBuf>,
}

impl Default for QuickLaunchPreferences {
    fn default() -> Self {
        Self {
            kind: SessionKind::Shell,
            args: Vec::new(),
            cwd: None,
            project_root: None,
            worktree_path: None,
        }
    }
}
