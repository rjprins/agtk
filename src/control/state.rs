use serde::Serialize;
use std::path::PathBuf;

use crate::appearance::ThemeKey;
use crate::session::{SessionKind, SessionState};
use crate::shortcuts::ShortcutAction;

#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct AppState {
    pub instance: String,
    pub protocol_version: u16,
    pub selected_session_id: Option<String>,
    pub appearance: AppearanceSummary,
    pub shortcuts: Vec<ShortcutSummary>,
    pub window: WindowState,
    pub projects: Vec<ProjectSummary>,
    pub worktree_groups: Vec<WorktreeGroupSummary>,
    pub sessions: Vec<SessionSummary>,
    pub attention: AttentionSummary,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct ShortcutSummary {
    pub action: ShortcutAction,
    pub label: String,
    pub accelerator: String,
    pub default_accelerator: String,
    pub is_active: bool,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct AppearanceSummary {
    pub theme: ThemeKey,
    pub effective_theme: ThemeKey,
    pub follow_system: bool,
    pub font: String,
    pub ui_font_size: u8,
    pub viewer_font_size: u8,
    pub available_themes: Vec<ThemeKey>,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct WindowState {
    pub width: i32,
    pub height: i32,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct ProjectSummary {
    pub root: String,
    pub name: String,
    pub is_pinned: bool,
    pub is_collapsed: bool,
    /// Pinned or with sessions. Other projects wait under Inactive Projects.
    pub is_active: bool,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct WorktreeGroupSummary {
    pub project_root: String,
    pub path: String,
    pub branch: String,
    pub session_ids: Vec<String>,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct SessionSummary {
    pub id: String,
    pub name: String,
    pub kind: SessionKind,
    pub state: SessionState,
    pub is_selected: bool,
    pub history_count: usize,
    pub cwd: Option<PathBuf>,
    pub project_root: Option<PathBuf>,
    pub worktree_path: Option<PathBuf>,
    pub created_at: u64,
    pub state_changed_at: u64,
    pub position: i64,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct AttentionSummary {
    pub count: usize,
}

#[derive(Debug, Clone, PartialEq, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct UiInspection {
    pub root: UiNode,
    pub is_truncated: bool,
}

#[derive(Debug, Clone, PartialEq, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct UiNode {
    pub id: String,
    pub role: String,
    pub label: Option<String>,
    pub is_visible: bool,
    pub is_enabled: bool,
    pub is_selected: bool,
    pub bounds: Bounds,
    pub children: Vec<UiNode>,
}

#[derive(Debug, Clone, Copy, Default, PartialEq, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct Bounds {
    pub x: f32,
    pub y: f32,
    pub width: f32,
    pub height: f32,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct TextSnapshot {
    pub session_id: String,
    pub text: String,
    pub lines: usize,
    pub is_truncated: bool,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct CaptureResult {
    pub path: PathBuf,
    pub width: i32,
    pub height: i32,
    pub sha256: String,
}
