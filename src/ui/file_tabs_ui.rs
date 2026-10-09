//! Read-only file tabs: one reusable buffer per worktree in the shared code viewer.

use super::*;
use crate::changes::FileDocumentResult;
use crate::file_links::MarkdownLink;
use crate::workspace_tabs::{FileTabKey, WorkspaceTabId, WorkspaceTabs};

pub(super) const MARKDOWN_PREVIEW_PREFERENCE: &str = "markdownPreview";

/// Whether Markdown shows rendered, remembered apart for file tabs and diff tabs.
#[derive(Debug, Clone, Copy, PartialEq, Eq, serde::Serialize, serde::Deserialize)]
#[serde(default)]
pub(super) struct MarkdownPreview {
    pub files: bool,
    pub diffs: bool,
}

impl Default for MarkdownPreview {
    fn default() -> Self {
        Self {
            files: true,
            diffs: false,
        }
    }
}

#[derive(Debug, Default, Clone)]
pub(super) struct FileTabState {
    /// Line and column to put the cursor on once the text shows.
    pub pending_position: Option<(u32, Option<u32>)>,
    /// Identity of the content last shown, to notice changes on disk.
    pub signature: Option<String>,
    pub stale: bool,
}

/// The path as the tab shows it: relative inside the worktree, `~` elsewhere.
pub(super) fn file_tab_title(key: &FileTabKey) -> String {
    if let Ok(relative) = key.path.strip_prefix(&key.worktree_root)
        && !relative.as_os_str().is_empty()
    {
        return relative.to_string_lossy().into_owned();
    }
    let home = glib::home_dir();
    match key.path.strip_prefix(&home) {
        Ok(rest) if !rest.as_os_str().is_empty() => format!("~/{}", rest.to_string_lossy()),
        _ => key.path.to_string_lossy().into_owned(),
    }
}

pub(super) fn file_tab_label(key: &FileTabKey) -> String {
    key.path
        .file_name()
        .map(|name| name.to_string_lossy().into_owned())
        .unwrap_or_else(|| file_tab_title(key))
}

impl Workspace {
    /// Shows `key` in the owner's file buffer, or just moves the cursor when it is already shown.
    pub(super) fn open_file_tab(
        &self,
        key: FileTabKey,
        owner: String,
        position: Option<(u32, Option<u32>)>,
    ) -> Option<String> {
        let tab = WorkspaceTabId::File(key.clone());
        let tab_id = WorkspaceTabs::file_tab_id(&key);
        let same_file_is_visible = self.workspace_tabs.borrow().visible_tab() == Some(&tab)
            && self.stack.visible_child_name().as_deref() == Some("changes-diff");
        let stale = self
            .file_tabs
            .borrow()
            .get(&key)
            .is_some_and(|state| state.stale);
        if same_file_is_visible && !stale {
            if let Some(viewer) = self.code_viewer.borrow().as_ref() {
                if let Some((line, column)) = position {
                    viewer.reveal(line, column);
                }
                viewer.focus();
            }
            return Some(tab_id);
        }
        if self
            .workspace_tabs
            .borrow_mut()
            .open_file(key.clone(), &owner)
            .is_none()
        {
            self.show_error("Could not open this file in the selected worktree");
            return None;
        }
        self.file_tabs.borrow_mut().insert(
            key.clone(),
            FileTabState {
                pending_position: position,
                signature: None,
                stale: false,
            },
        );
        self.render_workspace_tabs();
        self.activate_file_tab(&key);
        Some(tab_id)
    }

    /// Opens a path in the context of the session that mentioned it, or the selected one.
    pub(super) fn open_file_from_session(
        &self,
        session_id: Option<String>,
        path: PathBuf,
        line: Option<u32>,
        column: Option<u32>,
    ) -> Option<String> {
        let owner = session_id
            .filter(|id| self.sessions.borrow().contains_key(id))
            .or_else(|| self.selected_session_id())?;
        let context = self.context_key_for_session(&owner);
        self.workspace_tabs
            .borrow_mut()
            .attach_session(owner.clone(), context.clone());
        let key = FileTabKey {
            worktree_root: context,
            path,
        };
        self.open_file_tab(key, owner, line.map(|line| (line, column)))
    }

    pub(super) fn open_file_control(&self, pending: PendingRequest) {
        let ControlCommand::UiOpenFile(params) = pending.request.command.clone() else {
            return;
        };
        let request_id = pending.request.id.clone();
        let Some(record) = self
            .sessions
            .borrow()
            .get(&params.session_id)
            .map(|session| session.record.clone())
        else {
            let _ = pending.respond(session_not_found(request_id, &params.session_id));
            return;
        };
        let requested = PathBuf::from(&params.path);
        let path = if requested.is_absolute() {
            requested
        } else {
            match record.worktree_path.clone().or(record.cwd.clone()) {
                Some(base) => base.join(requested),
                None => {
                    let _ = pending.respond(ControlResponse::failure(
                        request_id,
                        ErrorCode::OperationRefused,
                        "This session has no worktree path",
                        None,
                    ));
                    return;
                }
            }
        };
        if !path.is_file() {
            let _ = pending.respond(ControlResponse::failure(
                request_id,
                ErrorCode::OperationRefused,
                "No file exists at that path",
                Some(serde_json::json!({ "path": path.to_string_lossy() })),
            ));
            return;
        }
        self.select_session_for_diff(&params.session_id);
        match self.open_file_from_session(Some(params.session_id), path, params.line, params.column)
        {
            Some(tab_id) => {
                let _ = pending.respond(ControlResponse::success(
                    request_id,
                    serde_json::json!({ "opened": true, "tabId": tab_id }),
                ));
            }
            None => {
                let _ = pending.respond(ControlResponse::failure(
                    request_id,
                    ErrorCode::OperationRefused,
                    "Could not open the requested file tab",
                    None,
                ));
            }
        }
    }

    pub(super) fn activate_file_tab(&self, key: &FileTabKey) {
        let tab = WorkspaceTabId::File(key.clone());
        if !self.workspace_tabs.borrow_mut().select_tab(&tab) {
            return;
        }
        self.stack.set_visible_child_name("changes-diff");
        self.session_context_bar.set_visible(false);
        self.context_pr.set_visible(false);
        self.search_bar.set_search_mode(false);
        let title = file_tab_title(key);
        self.diff_heading.set_text(&title);
        self.content_title.set_title(&title);
        self.diff_navigation.set_visible(false);
        self.show_markdown_presentation(&key.path, false);
        self.open_in_emacs_button.set_visible(true);
        self.diff_placeholder.set_text("Loading file…");
        self.diff_placeholder.set_visible(true);
        self.diff_viewer_host.set_visible(false);
        self.render_workspace_tabs();
        self.set_diff_actions_enabled(true);
        self.update_open_in_emacs_button();
        self.update_file_stale_banner(key);
        self.load_file_tab(key.clone());
    }

    fn load_file_tab(&self, key: FileTabKey) {
        let request = self.diff_request_sequence.get().wrapping_add(1);
        self.diff_request_sequence.set(request);
        *self.diff_current_request.borrow_mut() = None;
        *self.file_rendered.borrow_mut() = None;
        *self.diff_error.borrow_mut() = None;
        let path = key.path.clone();
        let worktree = key.worktree_root.clone();
        let title = file_tab_title(&key);
        let result = self.changes_io.submit(move || {
            let document = crate::changes::read_file_document(&path, &title)?;
            Ok((
                document,
                crate::changes::link_target_outside(&path, Path::new(&worktree)),
            ))
        });
        let workspace = self.clone();
        glib::spawn_future_local(async move {
            let result = result
                .await
                .unwrap_or_else(|_| Err("Changes worker stopped".to_owned()));
            if workspace.diff_request_sequence.get() != request
                || workspace.workspace_tabs.borrow().visible_tab()
                    != Some(&WorkspaceTabId::File(key.clone()))
            {
                return;
            }
            match result {
                Ok((document, link_target)) => {
                    let identity = document.identity().to_owned();
                    if let Some(state) = workspace.file_tabs.borrow_mut().get_mut(&key) {
                        state.signature = Some(identity);
                        state.stale = false;
                    }
                    workspace.update_file_stale_banner(&key);
                    match document {
                        FileDocumentResult::Placeholder(placeholder) => workspace
                            .show_viewer_placeholder(
                                &placeholder.path,
                                &placeholder.reason,
                                placeholder.byte_size,
                                &[],
                            ),
                        document => {
                            workspace.show_file_document(&key, document, link_target, request)
                        }
                    }
                }
                Err(error) => workspace.show_viewer_placeholder(
                    &file_tab_title(&key),
                    "Could not load file",
                    None,
                    &[error],
                ),
            }
        });
    }

    fn show_file_document(
        &self,
        key: &FileTabKey,
        document: FileDocumentResult,
        link_target: Option<PathBuf>,
        request: u64,
    ) {
        let tab_id = WorkspaceTabs::file_tab_id(key);
        let request_id = format!("{tab_id}-{request}");
        *self.diff_current_request.borrow_mut() = Some(request_id.clone());
        *self.file_rendered.borrow_mut() = None;
        *self.diff_error.borrow_mut() = None;
        self.ensure_code_viewer();
        let (theme, font_family, font_size) = self.viewer_appearance();
        let Some(viewer) = self.code_viewer.borrow().as_ref().cloned() else {
            return;
        };
        viewer.set_appearance(theme, &font_family, font_size);
        let state = self.diff_view_states.borrow().get(&tab_id).cloned();
        let position = self
            .file_tabs
            .borrow_mut()
            .get_mut(key)
            .and_then(|state| state.pending_position.take());
        let inside_worktree = key.path.starts_with(&key.worktree_root) && link_target.is_none();
        let (mut payload, details) = match document {
            FileDocumentResult::Text(document) => {
                let lines = if document.line_count == 1 {
                    "1 line".to_owned()
                } else {
                    format!("{} lines", document.line_count)
                };
                (
                    serde_json::json!({
                        "path": document.path,
                        "text": document.text,
                        "language": document.language,
                    }),
                    lines,
                )
            }
            FileDocumentResult::Image(document) => (
                serde_json::json!({
                    "path": document.path,
                    "dataUrl": document.data_url,
                }),
                glib::format_size(document.byte_size).to_string(),
            ),
            FileDocumentResult::Placeholder(_) => return,
        };
        let mut metadata = vec![details];
        if let Some(target) = &link_target {
            metadata.push(format!(
                "Links outside the worktree to {}",
                target.display()
            ));
        }
        let common = serde_json::json!({
            "requestId": request_id,
            "tabId": tab_id,
            "label": if inside_worktree { "Working tree" } else { "" },
            "metadata": metadata,
            "viewState": state,
            "line": position.map(|(line, _)| line),
            "column": position.and_then(|(_, column)| column),
            "presentation": self.markdown_presentation_for(&key.path, false),
        });
        payload
            .as_object_mut()
            .expect("file payload is an object")
            .extend(
                common
                    .as_object()
                    .expect("common payload is an object")
                    .clone(),
            );
        if let Err(error) = viewer.show_file(&payload) {
            self.show_viewer_placeholder(&file_tab_title(key), &error, None, &[]);
            return;
        }
        self.diff_placeholder.set_visible(false);
        self.diff_viewer_host.set_visible(true);
        viewer.focus();
    }

    /// Re-reads the shown file on each refresh and offers a reload when it changed.
    pub(super) fn check_file_for_staleness(&self, key: &FileTabKey) {
        let Some(expected) = self
            .file_tabs
            .borrow()
            .get(key)
            .and_then(|state| state.signature.clone())
        else {
            return;
        };
        let path = key.path.clone();
        let title = file_tab_title(key);
        let result = self
            .changes_io
            .submit(move || crate::changes::read_file_document(&path, &title));
        let workspace = self.clone();
        let key = key.clone();
        glib::spawn_future_local(async move {
            let result = result
                .await
                .unwrap_or_else(|_| Err("Changes worker stopped".to_owned()));
            let still_expected = workspace
                .file_tabs
                .borrow()
                .get(&key)
                .is_some_and(|state| state.signature.as_deref() == Some(expected.as_str()));
            if !still_expected {
                return;
            }
            let stale = match result {
                Ok(document) => document.identity() != expected,
                Err(_) => true,
            };
            if let Some(state) = workspace.file_tabs.borrow_mut().get_mut(&key) {
                state.stale = stale;
            }
            workspace.update_file_stale_banner(&key);
        });
    }

    pub(super) fn update_file_stale_banner(&self, key: &FileTabKey) {
        let active = self.workspace_tabs.borrow().visible_tab().cloned();
        let stale = self
            .file_tabs
            .borrow()
            .get(key)
            .is_some_and(|state| state.stale);
        let visible = self.stack.visible_child_name().as_deref() == Some("changes-diff")
            && active == Some(WorkspaceTabId::File(key.clone()))
            && stale;
        self.diff_stale_label
            .set_text("This file changed on disk. Reload to see the latest content.");
        self.diff_reload_button.set_label("Reload file");
        self.diff_reload_button.set_sensitive(stale);
        self.diff_stale_banner.set_visible(visible);
    }

    pub(super) fn reload_active_file(&self, key: &FileTabKey) {
        if let Some(state) = self.file_tabs.borrow_mut().get_mut(key) {
            state.stale = false;
            state.signature = None;
        }
        self.update_file_stale_banner(key);
        self.load_file_tab(key.clone());
    }

    pub(super) fn close_file_tab(&self, key: &FileTabKey) {
        self.file_tabs.borrow_mut().remove(key);
        self.diff_view_states
            .borrow_mut()
            .remove(&WorkspaceTabs::file_tab_id(key));
        let next = self.workspace_tabs.borrow_mut().close_file(key);
        self.render_workspace_tabs();
        self.show_tab_after_close(next);
    }

    /// The file behind the visible diff or file tab, if it exists on disk.
    fn visible_document_path(&self) -> Option<PathBuf> {
        self.visible_document()
            .map(|(path, _, _)| path)
            .filter(|path| path.is_file())
    }

    /// The path, worktree root and tab ID of the visible diff or file tab.
    fn visible_document(&self) -> Option<(PathBuf, PathBuf, String)> {
        match self.workspace_tabs.borrow().visible_tab()? {
            WorkspaceTabId::File(key) => Some((
                key.path.clone(),
                PathBuf::from(&key.worktree_root),
                WorkspaceTabs::file_tab_id(key),
            )),
            WorkspaceTabId::Diff(key) => {
                use std::os::unix::ffi::OsStrExt;
                let relative = key.new_path.as_deref().or(key.old_path.as_deref())?;
                let root = PathBuf::from(&key.worktree_root);
                Some((
                    root.join(std::ffi::OsStr::from_bytes(relative)),
                    root,
                    WorkspaceTabs::diff_tab_id(key),
                ))
            }
            WorkspaceTabId::Session(_) => None,
        }
    }

    fn markdown_preview_for(&self, diff: bool) -> bool {
        let preview = self.markdown_preview.get();
        if diff { preview.diffs } else { preview.files }
    }

    /// What the viewer shows first for `path`: rendered Markdown or its source.
    pub(super) fn markdown_presentation_for(&self, path: &Path, diff: bool) -> &'static str {
        if crate::file_links::is_markdown_path(path) && self.markdown_preview_for(diff) {
            "preview"
        } else {
            "source"
        }
    }

    /// Shows the Source and Preview switch for Markdown, set to how this kind of tab shows it.
    pub(super) fn show_markdown_presentation(&self, path: &Path, diff: bool) {
        let markdown = crate::file_links::is_markdown_path(path);
        let preview = markdown && self.markdown_preview_for(diff);
        // Hide first, so setting the switch is not taken as the user's choice.
        self.markdown_presentation.set_visible(false);
        self.markdown_presentation.set_active_name(Some(if preview {
            "preview"
        } else {
            "source"
        }));
        self.markdown_presentation.set_visible(markdown);
        self.diff_navigation.set_sensitive(!preview);
    }

    /// The user switched the visible Markdown between source and rendered.
    pub(super) fn set_markdown_preview(&self, preview: bool) {
        if !self.markdown_presentation.is_visible() {
            return;
        }
        let diff = match self.workspace_tabs.borrow().visible_tab() {
            Some(WorkspaceTabId::Diff(_)) => true,
            Some(WorkspaceTabId::File(_)) => false,
            _ => return,
        };
        let mut preference = self.markdown_preview.get();
        let slot = if diff {
            &mut preference.diffs
        } else {
            &mut preference.files
        };
        if *slot == preview {
            return;
        }
        *slot = preview;
        self.markdown_preview.set(preference);
        if let Ok(value) = serde_json::to_value(preference) {
            self.save_preference(MARKDOWN_PREVIEW_PREFERENCE, value);
        }
        self.diff_navigation.set_sensitive(!preview);
        if let Some(viewer) = self.code_viewer.borrow().as_ref() {
            viewer.set_presentation(preview);
            viewer.focus();
        }
    }

    /// Follows a link clicked in rendered Markdown: files open in the file tab, web links in the browser.
    pub(super) fn open_markdown_link(&self, tab_id: &str, href: &str) {
        let Some((document, root, visible_tab_id)) = self.visible_document() else {
            return;
        };
        if visible_tab_id != tab_id {
            return;
        }
        match crate::file_links::resolve_markdown_link(href, &document, &root) {
            Some(MarkdownLink::Web(url)) => {
                let workspace = self.clone();
                gtk::UriLauncher::new(&url).launch(
                    Some(&self.window),
                    None::<&gio::Cancellable>,
                    move |result| {
                        if let Err(error) = result {
                            workspace.show_error(&format!("Could not open link: {error}"));
                        }
                    },
                );
            }
            Some(MarkdownLink::File { path, line }) => {
                // A folder link opens its README, as GitHub shows it below the listing.
                let path = if path.is_dir() {
                    ["README.md", "readme.md", "Readme.md"]
                        .iter()
                        .map(|name| path.join(name))
                        .find(|readme| readme.is_file())
                        .unwrap_or(path)
                } else {
                    path
                };
                if path.is_file() {
                    self.open_file_from_session(None, path, line, None);
                } else {
                    self.show_error(&format!("No file at {}", path.display()));
                }
            }
            None => self.show_error(&format!("This link does not open in agtk: {href}")),
        }
    }

    pub(super) fn open_visible_document_in_emacs(&self) {
        match self.visible_document_path() {
            Some(path) => self.open_file_in_emacs(path, None, None),
            None => self.show_error("This file is not available on disk"),
        }
    }

    pub(super) fn update_open_in_emacs_button(&self) {
        self.open_in_emacs_button
            .set_sensitive(self.visible_document_path().is_some());
    }
}
