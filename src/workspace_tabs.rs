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
    /// Each context has one reusable diff buffer and one file buffer.
    diff: Option<DiffTabKey>,
    file: Option<FileTabKey>,
    active: Option<WorkspaceTabId>,
    recent: Vec<WorkspaceTabId>,
    /// Previously viewed files and diffs in first-opened order, without duplicates.
    buffer_history: Vec<WorkspaceTabId>,
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

    /// Shows `key` in the context's reusable diff buffer.
    pub fn open_diff(&mut self, key: DiffTabKey, context_owner: &str) -> Option<WorkspaceTabId> {
        let worktree_root = key.worktree_root.clone();
        self.open_buffer(WorkspaceTabId::Diff(key), &worktree_root, context_owner)
    }

    /// Shows `key` in the context's reusable file buffer.
    pub fn open_file(&mut self, key: FileTabKey, context_owner: &str) -> Option<WorkspaceTabId> {
        let worktree_root = key.worktree_root.clone();
        self.open_buffer(WorkspaceTabId::File(key), &worktree_root, context_owner)
    }

    fn open_buffer(
        &mut self,
        tab: WorkspaceTabId,
        worktree_root: &str,
        context_owner: &str,
    ) -> Option<WorkspaceTabId> {
        let context = self.session_context.get(context_owner)?.clone();
        if context != worktree_root {
            return None;
        }
        let tabs = self.contexts.entry(context.clone()).or_default();
        replace_buffer(tabs, &tab);
        self.selected_session = Some(context_owner.to_owned());
        self.record_visit(context_owner);
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
                if &context != worktree_root {
                    return false;
                }
                let Some(tabs) = self.contexts.get_mut(&context) else {
                    return false;
                };
                if !tab_exists(tabs, tab) && !tabs.buffer_history.contains(tab) {
                    return false;
                }
                replace_buffer(tabs, tab);
                self.activate(&context, tab.clone());
                true
            }
        }
    }

    pub fn close_diff(&mut self, key: &DiffTabKey) -> Option<WorkspaceTabId> {
        let tabs = self.contexts.get_mut(&key.worktree_root)?;
        if tabs.diff.as_ref() == Some(key) {
            tabs.diff = None;
        }
        self.close_buffer(&key.worktree_root, WorkspaceTabId::Diff(key.clone()))
    }

    pub fn close_file(&mut self, key: &FileTabKey) -> Option<WorkspaceTabId> {
        let tabs = self.contexts.get_mut(&key.worktree_root)?;
        if tabs.file.as_ref() == Some(key) {
            tabs.file = None;
        }
        self.close_buffer(&key.worktree_root, WorkspaceTabId::File(key.clone()))
    }

    /// After a diff or file tab closes, falls back to the most recent remaining tab.
    fn close_buffer(&mut self, context: &str, closed: WorkspaceTabId) -> Option<WorkspaceTabId> {
        let selected_session = self.selected_session.clone();
        let tabs = self.contexts.get_mut(context)?;
        tabs.recent.retain(|tab| tab != &closed);
        tabs.buffer_history.retain(|tab| tab != &closed);
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
                .or_else(|| tabs.diff.clone().map(WorkspaceTabId::Diff))
                .or_else(|| tabs.file.clone().map(WorkspaceTabId::File));
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
                .or_else(|| tabs.diff.clone().map(WorkspaceTabId::Diff))
                .or_else(|| tabs.file.clone().map(WorkspaceTabId::File));
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
            .chain(tabs.diff.clone().map(WorkspaceTabId::Diff))
            .chain(tabs.file.clone().map(WorkspaceTabId::File))
            .collect()
    }

    pub fn active_context(&self) -> Option<&str> {
        self.active_context.as_deref()
    }

    /// History for this viewer and worktree, newest first-opened file first.
    /// Revisiting a file keeps its position in the list.
    pub fn buffer_history(&self, tab: &WorkspaceTabId) -> Vec<WorkspaceTabId> {
        let context = match tab {
            WorkspaceTabId::Diff(key) => &key.worktree_root,
            WorkspaceTabId::File(key) => &key.worktree_root,
            WorkspaceTabId::Session(_) => return Vec::new(),
        };
        self.contexts
            .get(context)
            .into_iter()
            .flat_map(|tabs| tabs.buffer_history.iter().rev())
            .filter(|previous| std::mem::discriminant(*previous) == std::mem::discriminant(tab))
            .cloned()
            .collect()
    }

    pub fn selected_session(&self) -> Option<&str> {
        self.selected_session.as_deref()
    }

    /// The most recently selected session other than the current one.
    pub fn last_session(&self) -> Option<&str> {
        self.last_session_matching(|_| true)
    }

    /// The most recently selected eligible session other than the current one.
    pub(crate) fn last_session_matching(
        &self,
        mut eligible: impl FnMut(&str) -> bool,
    ) -> Option<&str> {
        self.session_history
            .iter()
            .rev()
            .map(String::as_str)
            .find(|id| Some(*id) != self.selected_session.as_deref() && eligible(id))
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
            if !matches!(tab, WorkspaceTabId::Session(_)) && !tabs.buffer_history.contains(&tab) {
                tabs.buffer_history.push(tab.clone());
            }
            tabs.recent.retain(|previous| previous != &tab);
            tabs.recent.push(tab.clone());
            tabs.active = Some(tab.clone());
            self.visible_tab = Some(tab);
        }
    }
}

fn replace_buffer(tabs: &mut ContextTabs, tab: &WorkspaceTabId) {
    match tab {
        WorkspaceTabId::Diff(key) => tabs.diff = Some(key.clone()),
        WorkspaceTabId::File(key) => tabs.file = Some(key.clone()),
        WorkspaceTabId::Session(_) => return,
    }
    tabs.recent
        .retain(|open| std::mem::discriminant(open) != std::mem::discriminant(tab));
}

fn tab_exists(tabs: &ContextTabs, tab: &WorkspaceTabId) -> bool {
    match tab {
        WorkspaceTabId::Session(id) => tabs.sessions.contains(id),
        WorkspaceTabId::Diff(key) => tabs.diff.as_ref() == Some(key),
        WorkspaceTabId::File(key) => tabs.file.as_ref() == Some(key),
    }
}

#[cfg(test)]
mod tests {
    use super::{DiffTabKey, FileTabKey, WorkspaceTabId, WorkspaceTabs};
    use crate::changes::DiffScope;
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
        assert!(
            tabs.context_tabs("/repo")
                .iter()
                .all(|tab| matches!(tab, WorkspaceTabId::Session(_)))
        );
    }

    #[test]
    fn history_reopens_files_without_reordering_and_lists_new_files_first() {
        let mut tabs = tabs_with(&["a"]);
        let first = WorkspaceTabId::File(file("/repo/src/main.rs"));
        let second = WorkspaceTabId::File(file("/repo/examples/main.rs"));
        tabs.open_file(file("/repo/src/main.rs"), "a");
        tabs.open_file(file("/repo/examples/main.rs"), "a");
        assert_eq!(
            tabs.buffer_history(&second),
            [second.clone(), first.clone()]
        );

        tabs.select_session("a");
        assert!(tabs.select_tab(&first));
        assert_eq!(tabs.buffer_history(&first), [second.clone(), first.clone()]);
        assert_eq!(tabs.visible_tab(), Some(&first));
        assert_eq!(
            tabs.context_tabs("/repo"),
            [WorkspaceTabId::Session("a".to_owned()), first.clone()]
        );
        assert!(tabs.select_tab(&first));
        assert_eq!(tabs.buffer_history(&first), [second.clone(), first.clone()]);
        tabs.open_file(file("/repo/examples/main.rs"), "a");
        assert_eq!(
            tabs.buffer_history(&second),
            [second.clone(), first.clone()]
        );
        let third = WorkspaceTabId::File(file("/repo/README.md"));
        tabs.open_file(file("/repo/README.md"), "a");
        assert!(tabs.select_tab(&first));
        assert_eq!(tabs.buffer_history(&first), [third, second, first.clone()]);
        assert_eq!(
            tabs.close_file(&file("/repo/src/main.rs")),
            Some(WorkspaceTabId::Session("a".to_owned()))
        );
        assert!(!tabs.select_tab(&first));
        assert!(!tabs.select_tab(&WorkspaceTabId::File(file("/repo/unopened.rs"))));
    }

    #[test]
    fn diff_history_preserves_scopes_and_stays_separate_from_files() {
        let mut tabs = tabs_with(&["a"]);
        let staged = DiffTabKey {
            worktree_root: "/repo".to_owned(),
            scope: DiffScope::Staged,
            old_path: Some(b"main.rs".to_vec()),
            new_path: Some(b"main.rs".to_vec()),
        };
        let unstaged = DiffTabKey {
            scope: DiffScope::Unstaged,
            ..staged.clone()
        };
        let staged_tab = WorkspaceTabId::Diff(staged.clone());
        let unstaged_tab = WorkspaceTabId::Diff(unstaged.clone());
        let file_tab = WorkspaceTabId::File(file("/repo/main.rs"));
        tabs.open_diff(staged.clone(), "a");
        tabs.open_file(file("/repo/main.rs"), "a");
        tabs.open_diff(unstaged, "a");
        assert_eq!(
            tabs.buffer_history(&file_tab),
            std::slice::from_ref(&file_tab)
        );
        assert_eq!(
            tabs.buffer_history(&unstaged_tab),
            [unstaged_tab.clone(), staged_tab.clone()]
        );
        assert!(tabs.select_tab(&staged_tab));
        assert_eq!(
            tabs.context_tabs("/repo"),
            [
                WorkspaceTabId::Session("a".to_owned()),
                staged_tab.clone(),
                file_tab.clone()
            ]
        );
        assert_eq!(
            tabs.buffer_history(&staged_tab),
            [unstaged_tab, staged_tab.clone()]
        );
        assert_eq!(tabs.close_diff(&staged), Some(file_tab));
        assert!(!tabs.select_tab(&staged_tab));
    }

    #[test]
    fn buffer_history_is_shared_within_a_worktree_and_removed_with_its_last_session() {
        let mut tabs = tabs_with(&["a", "b"]);
        let first = WorkspaceTabId::File(file("/repo/one.rs"));
        let second = WorkspaceTabId::File(file("/repo/two.rs"));
        tabs.open_file(file("/repo/one.rs"), "a");
        tabs.open_file(file("/repo/two.rs"), "b");
        tabs.attach_session("other", "/other");
        let other = FileTabKey {
            worktree_root: "/other".to_owned(),
            path: PathBuf::from("/other/main.rs"),
        };
        tabs.open_file(other.clone(), "other");
        let other_tab = WorkspaceTabId::File(other);
        assert_eq!(
            tabs.buffer_history(&other_tab),
            std::slice::from_ref(&other_tab)
        );
        assert!(!tabs.select_tab(&first));
        tabs.select_session("a");
        assert_eq!(tabs.buffer_history(&first), [second, first.clone()]);
        tabs.remove_session("a");
        assert!(tabs.select_tab(&first));
        tabs.remove_session("b");
        assert!(tabs.buffer_history(&first).is_empty());
        assert_eq!(tabs.buffer_history(&other_tab), [other_tab]);
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
