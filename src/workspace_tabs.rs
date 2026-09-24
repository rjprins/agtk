use std::collections::HashMap;
use std::hash::{Hash, Hasher};

use crate::changes::DiffScope;

#[derive(Debug, Clone, PartialEq, Eq, Hash)]
pub struct DiffTabKey {
    pub worktree_root: String,
    pub scope: DiffScope,
    pub old_path: Option<Vec<u8>>,
    pub new_path: Option<Vec<u8>>,
}

#[derive(Debug, Clone, PartialEq, Eq, Hash)]
pub enum WorkspaceTabId {
    Session(String),
    Diff(DiffTabKey),
}

#[derive(Debug, Default)]
struct ContextTabs {
    sessions: Vec<String>,
    diffs: Vec<DiffTabKey>,
    active: Option<WorkspaceTabId>,
    recent: Vec<WorkspaceTabId>,
}

#[derive(Debug, Default)]
pub struct WorkspaceTabs {
    session_context: HashMap<String, String>,
    contexts: HashMap<String, ContextTabs>,
    active_context: Option<String>,
    selected_session: Option<String>,
    visible_tab: Option<WorkspaceTabId>,
}

impl WorkspaceTabs {
    pub fn attach_session(&mut self, session_id: impl Into<String>, context: impl Into<String>) {
        let session_id = session_id.into();
        let context = context.into();
        if let Some(old_context) = self
            .session_context
            .insert(session_id.clone(), context.clone())
            && old_context != context
            && let Some(tabs) = self.contexts.get_mut(&old_context)
        {
            tabs.sessions.retain(|id| id != &session_id);
            tabs.recent
                .retain(|tab| tab != &WorkspaceTabId::Session(session_id.clone()));
        }
        let tabs = self.contexts.entry(context).or_default();
        if !tabs.sessions.contains(&session_id) {
            tabs.sessions.push(session_id);
        }
    }

    pub fn select_session(&mut self, session_id: &str) -> Option<WorkspaceTabId> {
        let context = self.session_context.get(session_id)?.clone();
        self.selected_session = Some(session_id.to_owned());
        self.active_context = Some(context.clone());
        let tab = WorkspaceTabId::Session(session_id.to_owned());
        self.activate(&context, tab.clone());
        Some(tab)
    }

    pub fn open_diff(&mut self, key: DiffTabKey, context_owner: &str) -> Option<WorkspaceTabId> {
        let context = self.session_context.get(context_owner)?.clone();
        if context != key.worktree_root {
            return None;
        }
        let tabs = self.contexts.entry(context.clone()).or_default();
        if !tabs.diffs.contains(&key) {
            tabs.diffs.push(key.clone());
        }
        self.selected_session = Some(context_owner.to_owned());
        self.active_context = Some(context.clone());
        let tab = WorkspaceTabId::Diff(key);
        self.activate(&context, tab.clone());
        Some(tab)
    }

    pub fn select_diff(&mut self, key: &DiffTabKey) -> Option<WorkspaceTabId> {
        let context = self
            .session_context
            .get(&self.selected_session.clone()?)?
            .clone();
        if context != key.worktree_root || !self.contexts.get(&context)?.diffs.contains(key) {
            return None;
        }
        let tab = WorkspaceTabId::Diff(key.clone());
        self.active_context = Some(context.clone());
        self.activate(&context, tab.clone());
        Some(tab)
    }

    pub fn select_tab(&mut self, tab: &WorkspaceTabId) -> bool {
        match tab {
            WorkspaceTabId::Session(id) => self.select_session(id).is_some(),
            WorkspaceTabId::Diff(key) => {
                let Some(context) = self.active_context.clone() else {
                    return false;
                };
                if context != key.worktree_root
                    || !self
                        .contexts
                        .get(&context)
                        .is_some_and(|tabs| tabs.diffs.contains(key))
                {
                    return false;
                }
                self.activate(&context, tab.clone());
                true
            }
        }
    }

    pub fn close_diff(&mut self, key: &DiffTabKey) -> Option<WorkspaceTabId> {
        let tabs = self.contexts.get_mut(&key.worktree_root)?;
        tabs.diffs.retain(|open| open != key);
        tabs.recent
            .retain(|tab| tab != &WorkspaceTabId::Diff(key.clone()));
        if tabs.active == Some(WorkspaceTabId::Diff(key.clone())) {
            tabs.active = tabs
                .recent
                .iter()
                .rev()
                .find(|tab| tab_exists(tabs, tab))
                .cloned()
                .or_else(|| {
                    tabs.sessions
                        .iter()
                        .find(|id| self.selected_session.as_deref() == Some(id.as_str()))
                        .or_else(|| tabs.sessions.first())
                        .map(|id| WorkspaceTabId::Session(id.clone()))
                })
                .or_else(|| tabs.diffs.last().cloned().map(WorkspaceTabId::Diff));
        }
        self.visible_tab = tabs.active.clone();
        tabs.active.clone()
    }

    pub fn remove_session(&mut self, session_id: &str) -> Option<String> {
        let context = self.session_context.remove(session_id)?;
        let tabs = self.contexts.get_mut(&context)?;
        tabs.sessions.retain(|id| id != session_id);
        let session_tab = WorkspaceTabId::Session(session_id.to_owned());
        tabs.recent.retain(|tab| tab != &session_tab);
        if tabs.active == Some(session_tab) {
            tabs.active = tabs
                .recent
                .iter()
                .rev()
                .find(|tab| tab_exists(tabs, tab))
                .cloned()
                .or_else(|| tabs.sessions.first().cloned().map(WorkspaceTabId::Session))
                .or_else(|| tabs.diffs.last().cloned().map(WorkspaceTabId::Diff));
        }
        if self.selected_session.as_deref() == Some(session_id) {
            self.selected_session = tabs.sessions.first().cloned();
        }
        if self.active_context.as_deref() == Some(context.as_str()) {
            self.visible_tab = tabs.active.clone();
        }
        if tabs.sessions.is_empty() {
            self.contexts.remove(&context);
            if self.active_context.as_deref() == Some(context.as_str()) {
                self.active_context = None;
                self.visible_tab = None;
                self.selected_session = None;
            }
        }
        self.selected_session.clone()
    }

    pub fn context_tabs(&self, context: &str) -> Vec<WorkspaceTabId> {
        let Some(tabs) = self.contexts.get(context) else {
            return Vec::new();
        };
        tabs.sessions
            .iter()
            .cloned()
            .map(WorkspaceTabId::Session)
            .chain(tabs.diffs.iter().cloned().map(WorkspaceTabId::Diff))
            .collect()
    }

    pub fn active_context(&self) -> Option<&str> {
        self.active_context.as_deref()
    }

    pub fn selected_session(&self) -> Option<&str> {
        self.selected_session.as_deref()
    }

    pub fn session_context(&self, session_id: &str) -> Option<&str> {
        self.session_context.get(session_id).map(String::as_str)
    }

    pub fn visible_tab(&self) -> Option<&WorkspaceTabId> {
        self.visible_tab.as_ref()
    }

    pub fn diff_tab_id(key: &DiffTabKey) -> String {
        let mut hasher = std::collections::hash_map::DefaultHasher::new();
        key.hash(&mut hasher);
        format!("diff-{:016x}", hasher.finish())
    }

    fn activate(&mut self, context: &str, tab: WorkspaceTabId) {
        if let Some(tabs) = self.contexts.get_mut(context) {
            tabs.recent.retain(|previous| previous != &tab);
            tabs.recent.push(tab.clone());
            tabs.active = Some(tab.clone());
            self.visible_tab = Some(tab);
        }
    }
}

fn tab_exists(tabs: &ContextTabs, tab: &WorkspaceTabId) -> bool {
    match tab {
        WorkspaceTabId::Session(id) => tabs.sessions.contains(id),
        WorkspaceTabId::Diff(key) => tabs.diffs.contains(key),
    }
}
