//! Go to definition and find references in the code viewer, and back and
//! forward through the jumps they make.

use super::*;
use crate::language_server::{Answer, LanguageServers, Lookup, Query};
use crate::navigation_history::NavigationHistory;
use crate::workspace_tabs::{DiffTabKey, WorkspaceTabId};

/// A place in a file or diff tab, with a one-based line and column.
#[derive(Debug, Clone, PartialEq, Eq)]
pub(super) struct Place {
    tab: WorkspaceTabId,
    line: u32,
    column: u32,
}

/// The text the viewer shows, which lookups send to the language server.
struct ShownText {
    document_id: String,
    tab: WorkspaceTabId,
    root: PathBuf,
    path: PathBuf,
    text: Arc<str>,
}

#[derive(Clone, Default)]
pub(super) struct CodeNavigation {
    servers: Rc<LanguageServers>,
    shown: Rc<RefCell<Option<ShownText>>>,
    /// Files that answers for the shown text pointed to, the only ones a jump opens.
    targets: Rc<RefCell<HashSet<PathBuf>>>,
    /// Back and forward per worktree, since each worktree has its own buffers.
    histories: Rc<RefCell<HashMap<String, NavigationHistory<Place>>>>,
    /// Where to put the cursor once a diff tab shows again.
    diff_position: Rc<RefCell<Option<(DiffTabKey, u32, u32)>>>,
}

impl Workspace {
    /// Remembers the text a diff or file tab shows, when its language has a server.
    pub(super) fn remember_shown_text(
        &self,
        document_id: &str,
        tab: WorkspaceTabId,
        root: &str,
        path: PathBuf,
        text: Arc<str>,
    ) {
        let navigation = &self.code_navigation;
        navigation.targets.borrow_mut().clear();
        let root = PathBuf::from(root);
        *navigation.shown.borrow_mut() =
            (crate::language_server::Language::for_path(&path).is_some() && root.is_absolute())
                .then(|| ShownText {
                    document_id: document_id.to_owned(),
                    tab,
                    root,
                    path,
                    text,
                });
    }

    /// The cursor position to restore when `key` shows, if going back asked for one.
    pub(super) fn take_diff_position(&self, key: &DiffTabKey) -> Option<(u32, u32)> {
        let mut pending = self.code_navigation.diff_position.borrow_mut();
        let (pending_key, line, column) = pending.take()?;
        (&pending_key == key).then_some((line, column))
    }

    pub(super) fn stop_language_servers(&self, worktree: &Path) {
        let root = worktree
            .canonicalize()
            .unwrap_or_else(|_| worktree.to_owned());
        self.code_navigation.servers.stop(&root);
    }

    pub(super) fn look_up_code(
        &self,
        request_id: u32,
        document_id: &str,
        lookup: Lookup,
        line: u32,
        column: u32,
    ) {
        let query = self
            .code_navigation
            .shown
            .borrow()
            .as_ref()
            .filter(|shown| shown.document_id == document_id)
            .map(|shown| Query {
                lookup,
                root: shown.root.clone(),
                path: shown.path.clone(),
                text: shown.text.clone(),
                line: line - 1,
                character: column - 1,
            });
        let Some(query) = query else {
            self.answer_lookup(
                request_id,
                &serde_json::json!({ "error": "Lookups are not available here" }),
            );
            return;
        };
        let answer = self.code_navigation.servers.lookup(query);
        let workspace = self.clone();
        let document_id = document_id.to_owned();
        glib::spawn_future_local(async move {
            let result = answer
                .await
                .unwrap_or_else(|_| Err("The language server stopped".to_owned()));
            let still_shown = workspace
                .code_navigation
                .shown
                .borrow()
                .as_ref()
                .is_some_and(|shown| shown.document_id == document_id);
            // The viewer drops pending lookups when it shows something else.
            if !still_shown {
                return;
            }
            let payload = match result {
                Ok(answer) => {
                    workspace.code_navigation.targets.borrow_mut().extend(
                        answer
                            .locations
                            .iter()
                            .filter(|location| !location.in_query)
                            .map(|location| location.path.clone()),
                    );
                    answer_payload(&answer)
                }
                Err(error) => serde_json::json!({ "error": error }),
            };
            workspace.answer_lookup(request_id, &payload);
        });
    }

    fn answer_lookup(&self, request_id: u32, answer: &serde_json::Value) {
        if let Some(viewer) = self.code_viewer.borrow().as_ref() {
            viewer.answer_lookup(request_id, answer);
        }
    }

    /// Follows a definition or reference that Monaco opened, remembering where it started.
    pub(super) fn follow_code_jump(
        &self,
        document_id: &str,
        path: Option<String>,
        target: (u32, u32),
        from: (u32, u32),
    ) {
        let Some(from) = self.shown_place(document_id, from) else {
            return;
        };
        let Some(path) = path.map(PathBuf::from) else {
            // Monaco moves the cursor within the shown text itself.
            self.record_jump(from);
            return;
        };
        if !self.code_navigation.targets.borrow().contains(&path) {
            return;
        }
        if !path.is_file() {
            self.show_error(&format!("No file at {}", path.display()));
            return;
        }
        self.record_jump(from);
        self.open_file_from_session(None, path, Some(target.0), Some(target.1));
    }

    /// Goes back or forward from the cursor at `current`.
    pub(super) fn navigate_code_history(
        &self,
        document_id: &str,
        forward: bool,
        current: (u32, u32),
    ) {
        let Some(current) = self.shown_place(document_id, current) else {
            return;
        };
        let target = {
            let mut histories = self.code_navigation.histories.borrow_mut();
            let history = histories.entry(place_context(&current)).or_default();
            if forward {
                history.forward(current)
            } else {
                history.back(current)
            }
        };
        if let Some(target) = target
            && !self.go_to_place(&target)
        {
            self.show_error("That diff is no longer open");
        }
    }

    fn shown_place(&self, document_id: &str, (line, column): (u32, u32)) -> Option<Place> {
        self.code_navigation
            .shown
            .borrow()
            .as_ref()
            .filter(|shown| shown.document_id == document_id)
            .map(|shown| Place {
                tab: shown.tab.clone(),
                line,
                column,
            })
    }

    fn record_jump(&self, from: Place) {
        self.code_navigation
            .histories
            .borrow_mut()
            .entry(place_context(&from))
            .or_default()
            .record(from);
    }

    fn go_to_place(&self, place: &Place) -> bool {
        match &place.tab {
            WorkspaceTabId::File(key) => self
                .open_file_from_session(
                    None,
                    key.path.clone(),
                    Some(place.line),
                    Some(place.column),
                )
                .is_some(),
            WorkspaceTabId::Diff(key) => {
                let visible = self.workspace_tabs.borrow().visible_tab() == Some(&place.tab)
                    && self.stack.visible_child_name().as_deref() == Some("changes-diff");
                if visible {
                    if let Some(viewer) = self.code_viewer.borrow().as_ref() {
                        viewer.reveal(place.line, Some(place.column));
                        viewer.focus();
                    }
                    return true;
                }
                *self.code_navigation.diff_position.borrow_mut() =
                    Some((key.clone(), place.line, place.column));
                self.activate_diff_tab(key);
                let shown = self.workspace_tabs.borrow().visible_tab() == Some(&place.tab);
                if !shown {
                    self.code_navigation.diff_position.borrow_mut().take();
                }
                shown
            }
            WorkspaceTabId::Session(_) => false,
        }
    }
}

fn place_context(place: &Place) -> String {
    match &place.tab {
        WorkspaceTabId::File(key) => key.worktree_root.clone(),
        WorkspaceTabId::Diff(key) => key.worktree_root.clone(),
        WorkspaceTabId::Session(id) => id.clone(),
    }
}

/// Locations with one-based lines and columns, as Monaco counts them.
fn answer_payload(answer: &Answer) -> serde_json::Value {
    let locations = answer
        .locations
        .iter()
        .map(|location| {
            serde_json::json!({
                "path": location.path.to_string_lossy(),
                "inShownText": location.in_query,
                "startLine": location.start.0 + 1,
                "startColumn": location.start.1 + 1,
                "endLine": location.end.0 + 1,
                "endColumn": location.end.1 + 1,
            })
        })
        .collect::<Vec<_>>();
    let files = answer
        .files
        .iter()
        .map(|(path, text)| {
            (
                path.to_string_lossy().into_owned(),
                serde_json::Value::String(text.clone()),
            )
        })
        .collect::<serde_json::Map<_, _>>();
    serde_json::json!({ "locations": locations, "files": files })
}
