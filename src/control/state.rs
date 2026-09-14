use serde::Serialize;

use super::SessionKind;

#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct AppState {
    pub instance: String,
    pub protocol_version: u16,
    pub selected_session_id: Option<String>,
    pub window: WindowState,
    pub projects: Vec<ProjectSummary>,
    pub worktree_groups: Vec<WorktreeGroupSummary>,
    pub sessions: Vec<SessionSummary>,
    pub attention: AttentionSummary,
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
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct WorktreeGroupSummary {
    pub path: String,
    pub branch: String,
    pub session_ids: Vec<String>,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize)]
#[serde(rename_all = "lowercase")]
pub enum SessionState {
    Running,
    Busy,
    Ready,
    Waiting,
    Exited,
    Reconnecting,
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
