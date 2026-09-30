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

/// A file shown read-only. `path` is absolute so terminal links outside the
/// worktree still open in the context (worktree) of the session that printed them.
#[derive(Debug, Clone, PartialEq, Eq, Hash)]
pub struct FileTabKey {
    pub worktree_root: String,
    pub path: std::path::PathBuf,
}

#[derive(Debug, Clone, PartialEq, Eq, Hash)]
pub enum WorkspaceTabId {
    Session(String),
    Diff(DiffTabKey),
    File(FileTabKey),
}

#[derive(Debug, Default)]
struct ContextTabs {
    sessions: Vec<String>,
    diffs: Vec<DiffTabKey>,
    files: Vec<FileTabKey>,
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
    /// Sessions in the order they were last selected, most recent last.
    session_history: Vec<String>,
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
        self.record_visit(session_id);
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
        tabs.diffs.clear();
        tabs.diffs.push(key.clone());
        tabs.recent
            .retain(|tab| !matches!(tab, WorkspaceTabId::Diff(_)));
        self.selected_session = Some(context_owner.to_owned());
        self.record_visit(context_owner);
        self.active_context = Some(context.clone());
        let tab = WorkspaceTabId::Diff(key);
        self.activate(&context, tab.clone());
        Some(tab)
    }

    /// Shows `key` in the context's single reusable file buffer, like `open_diff`.
    pub fn open_file(&mut self, key: FileTabKey, context_owner: &str) -> Option<WorkspaceTabId> {
        let context = self.session_context.get(context_owner)?.clone();
        if context != key.worktree_root {
            return None;
        }
        let tabs = self.contexts.entry(context.clone()).or_default();
        tabs.files.clear();
        tabs.files.push(key.clone());
        tabs.recent
            .retain(|tab| !matches!(tab, WorkspaceTabId::File(_)));
        self.selected_session = Some(context_owner.to_owned());
        self.record_visit(context_owner);
        self.active_context = Some(context.clone());
        let tab = WorkspaceTabId::File(key);
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
            WorkspaceTabId::Diff(DiffTabKey { worktree_root, .. })
            | WorkspaceTabId::File(FileTabKey { worktree_root, .. }) => {
                let Some(context) = self.active_context.clone() else {
                    return false;
                };
                if &context != worktree_root
                    || !self
                        .contexts
                        .get(&context)
                        .is_some_and(|tabs| tab_exists(tabs, tab))
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
        self.close_buffer(&key.worktree_root, WorkspaceTabId::Diff(key.clone()))
    }

    pub fn close_file(&mut self, key: &FileTabKey) -> Option<WorkspaceTabId> {
        let tabs = self.contexts.get_mut(&key.worktree_root)?;
        tabs.files.retain(|open| open != key);
        self.close_buffer(&key.worktree_root, WorkspaceTabId::File(key.clone()))
    }

    /// After a diff or file tab closes, falls back to the most recent remaining tab.
    fn close_buffer(&mut self, context: &str, closed: WorkspaceTabId) -> Option<WorkspaceTabId> {
        let selected_session = self.selected_session.clone();
        let tabs = self.contexts.get_mut(context)?;
        tabs.recent.retain(|tab| tab != &closed);
        if tabs.active == Some(closed) {
            tabs.active = tabs
                .recent
                .iter()
                .rev()
                .find(|tab| tab_exists(tabs, tab))
                .cloned()
                .or_else(|| {
                    tabs.sessions
                        .iter()
                        .find(|id| selected_session.as_deref() == Some(id.as_str()))
                        .or_else(|| tabs.sessions.first())
                        .map(|id| WorkspaceTabId::Session(id.clone()))
                })
                .or_else(|| tabs.diffs.last().cloned().map(WorkspaceTabId::Diff))
                .or_else(|| tabs.files.last().cloned().map(WorkspaceTabId::File));
        }
        self.visible_tab = tabs.active.clone();
        tabs.active.clone()
    }

    pub fn remove_session(&mut self, session_id: &str) -> Option<String> {
        let context = self.session_context.remove(session_id)?;
        self.session_history.retain(|id| id != session_id);
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
                .or_else(|| tabs.diffs.last().cloned().map(WorkspaceTabId::Diff))
                .or_else(|| tabs.files.last().cloned().map(WorkspaceTabId::File));
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
            .chain(tabs.files.iter().cloned().map(WorkspaceTabId::File))
            .collect()
    }

    pub fn active_context(&self) -> Option<&str> {
        self.active_context.as_deref()
    }

    pub fn selected_session(&self) -> Option<&str> {
        self.selected_session.as_deref()
    }

    /// The most recently selected session other than the current one.
    pub fn last_session(&self) -> Option<&str> {
        self.session_history
            .iter()
            .rev()
            .map(String::as_str)
            .find(|id| Some(*id) != self.selected_session.as_deref())
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

    pub fn file_tab_id(key: &FileTabKey) -> String {
        let mut hasher = std::collections::hash_map::DefaultHasher::new();
        key.hash(&mut hasher);
        format!("file-{:016x}", hasher.finish())
    }

    fn record_visit(&mut self, session_id: &str) {
        self.session_history.retain(|id| id != session_id);
        self.session_history.push(session_id.to_owned());
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
        WorkspaceTabId::File(key) => tabs.files.contains(key),
    }
}

#[cfg(test)]
mod tests {
    use super::{FileTabKey, WorkspaceTabId, WorkspaceTabs};
    use std::path::PathBuf;

    fn tabs_with(sessions: &[&str]) -> WorkspaceTabs {
        let mut tabs = WorkspaceTabs::default();
        for id in sessions {
            tabs.attach_session(*id, "/repo");
        }
        tabs
    }

    fn file(path: &str) -> FileTabKey {
        FileTabKey {
            worktree_root: "/repo".to_owned(),
            path: PathBuf::from(path),
        }
    }

    #[test]
    fn one_file_buffer_per_context_replaces_the_previous_file() {
        let mut tabs = tabs_with(&["a"]);
        tabs.select_session("a");
        assert!(tabs.open_file(file("/repo/one.rs"), "a").is_some());
        assert!(tabs.open_file(file("/repo/two.rs"), "a").is_some());
        assert_eq!(
            tabs.context_tabs("/repo"),
            [
                WorkspaceTabId::Session("a".to_owned()),
                WorkspaceTabId::File(file("/repo/two.rs")),
            ]
        );
        assert_eq!(
            tabs.visible_tab(),
            Some(&WorkspaceTabId::File(file("/repo/two.rs")))
        );
        // A file from another context cannot open in this session's buffer.
        let mut elsewhere = file("/other/x.rs");
        elsewhere.worktree_root = "/other".to_owned();
        assert!(tabs.open_file(elsewhere, "a").is_none());
    }

    #[test]
    fn closing_a_file_tab_returns_to_the_last_visited_tab() {
        let mut tabs = tabs_with(&["a", "b"]);
        tabs.select_session("a");
        tabs.select_session("b");
        tabs.open_file(file("/repo/one.rs"), "b");
        assert_eq!(
            tabs.close_file(&file("/repo/one.rs")),
            Some(WorkspaceTabId::Session("b".to_owned()))
        );
        assert!(tabs.context_tabs("/repo").iter().all(|tab| matches!(tab, WorkspaceTabId::Session(_))));
    }

    #[test]
    fn last_session_toggles_between_the_two_most_recent() {
        let mut tabs = tabs_with(&["a", "b", "c"]);
        assert_eq!(tabs.last_session(), None);
        tabs.select_session("a");
        assert_eq!(tabs.last_session(), None);
        tabs.select_session("b");
        tabs.select_session("c");
        assert_eq!(tabs.last_session(), Some("b"));
        tabs.select_session("b");
        assert_eq!(tabs.last_session(), Some("c"));
    }

    #[test]
    fn last_session_skips_closed_sessions() {
        let mut tabs = tabs_with(&["a", "b", "c"]);
        tabs.select_session("a");
        tabs.select_session("b");
        tabs.select_session("c");
        tabs.remove_session("b");
        assert_eq!(tabs.last_session(), Some("a"));
    }
}
