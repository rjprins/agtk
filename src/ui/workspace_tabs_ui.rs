use super::*;
use crate::changes::DiffScope;
use crate::workspace_tabs::{DiffTabKey, WorkspaceTabId};

impl Workspace {
    pub(super) fn attach_workspace_session(&self, session_id: &str) {
        let context = self.context_key_for_session(session_id);
        self.workspace_tabs
            .borrow_mut()
            .attach_session(session_id.to_owned(), context);
        self.render_workspace_tabs();
    }

    pub(super) fn activate_session(&self, session_id: &str) {
        let Some(record) = self
            .sessions
            .borrow()
            .get(session_id)
            .map(|session| session.record.clone())
        else {
            return;
        };
        if self.workspace_tabs.borrow().selected_session() != Some(session_id) {
            if self
                .workspace_tabs
                .borrow()
                .context_tabs(&self.context_key_for_session(session_id))
                .is_empty()
            {
                self.attach_workspace_session(session_id);
            }
        }
        if self
            .workspace_tabs
            .borrow_mut()
            .select_session(session_id)
            .is_none()
        {
            self.attach_workspace_session(session_id);
            self.workspace_tabs.borrow_mut().select_session(session_id);
        }

        if self.stack.visible_child_name().as_deref() == Some("changes-diff") {
            if let Some(viewer) = self.diff_viewer.borrow().as_ref() {
                viewer.save_view_state();
                viewer.clear();
            }
        }
        *self.selected_session.borrow_mut() = Some(session_id.to_owned());
        self.stack.set_visible_child_name(session_id);
        self.session_context_bar.set_visible(true);
        self.context_pr.set_visible(true);
        self.search_bar.set_search_mode(false);
        self.refresh_content_title();
        if let Some(session) = self.sessions.borrow().get(session_id) {
            session.terminal.grab_focus();
        }
        if self.window.is_active() {
            self.acknowledge_session(session_id);
        }
        self.render_history(Some(session_id));
        self.refresh_selected_pr_context(session_id);
        self.search_button.set_sensitive(true);
        self.save_preference("selectedSessionId", serde_json::json!(session_id));
        self.render_workspace_tabs();
        self.refresh_changes();
        self.update_emacs_actions();
        self.update_claude_actions();
        self.set_diff_actions_enabled(false);
        let _ = record;
    }

    pub(super) fn open_changed_file(&self, file: ChangedFile) {
        let Some(owner) = self.selected_session_id() else {
            return;
        };
        let Some(snapshot) = self.changes.state.borrow().snapshot.clone() else {
            return;
        };
        let _ = self.open_changed_file_in_context(file, snapshot.context, owner);
    }

    fn open_changed_file_in_context(
        &self,
        file: ChangedFile,
        context: WorktreeContext,
        owner: String,
    ) -> Option<String> {
        let worktree_root = context.root.to_string_lossy().into_owned();
        let key = DiffTabKey {
            worktree_root,
            scope: file.scope.clone(),
            old_path: file.old_path.clone(),
            new_path: file.new_path.clone(),
        };
        if self
            .workspace_tabs
            .borrow_mut()
            .open_diff(key.clone(), &owner)
            .is_none()
        {
            self.show_error("Could not open this diff in the selected worktree");
            return None;
        }
        let tab_id = crate::workspace_tabs::WorkspaceTabs::diff_tab_id(&key);
        self.diff_tab_data
            .borrow_mut()
            .insert(key.clone(), (file, context));
        self.render_workspace_tabs();
        self.activate_diff_tab(&key);
        Some(tab_id)
    }

    pub(super) fn open_diff_control(&self, pending: PendingRequest) {
        let ControlCommand::UiOpenDiff(params) = pending.request.command.clone() else {
            return;
        };
        let request_id = pending.request.id.clone();
        let Some(record) = self
            .sessions
            .borrow()
            .get(&params.session_id)
            .map(|session| session.record.clone())
        else {
            let _ = pending.respond(ControlResponse::failure(
                request_id,
                ErrorCode::SessionNotFound,
                "No session exists with that ID",
                Some(serde_json::json!({ "sessionId": params.session_id })),
            ));
            return;
        };
        let path = record.worktree_path.clone().or(record.cwd.clone());
        let Some(path) = path else {
            let _ = pending.respond(ControlResponse::failure(
                request_id,
                ErrorCode::OperationRefused,
                "This session has no worktree path",
                None,
            ));
            return;
        };
        if record
            .worktree_path
            .as_ref()
            .is_some_and(|worktree| !worktree.exists())
        {
            let _ = pending.respond(ControlResponse::failure(
                request_id,
                ErrorCode::OperationRefused,
                "The session worktree is unavailable",
                None,
            ));
            return;
        }
        let preferred_base = self
            .changes
            .state
            .borrow()
            .base_ref_by_context
            .get(&self.context_key_for_session(&params.session_id))
            .cloned();
        let request_generation = self.diff_request_sequence.get().wrapping_add(1);
        let changes_io = self.changes_io.clone();
        let request_path = path.clone();
        let worker_params = params.clone();
        let result = changes_io.submit(move || {
            let snapshot = crate::changes::read_changes_snapshot(
                &request_path,
                preferred_base.as_deref(),
                request_generation,
                0,
            )?
            .ok_or_else(|| "This session is outside a Git worktree".to_owned())?;
            let wanted_path = worker_params.path.as_bytes();
            let file = match worker_params.scope {
                crate::control::UiDiffScope::All => snapshot.all_changes,
                crate::control::UiDiffScope::Staged => snapshot.staged,
                crate::control::UiDiffScope::Unstaged => snapshot.unstaged,
                crate::control::UiDiffScope::Untracked => snapshot.untracked,
                crate::control::UiDiffScope::Committed => snapshot.branch_changes,
                crate::control::UiDiffScope::Commit => {
                    let commit_id = worker_params
                        .commit_id
                        .as_deref()
                        .ok_or_else(|| "Commit scope requires a commit ID".to_owned())?;
                    if !crate::changes::commit_is_in_branch(&snapshot.context, commit_id)? {
                        return Err("Commit is not in this worktree's branch history".to_owned());
                    }
                    crate::changes::read_commit_files(&snapshot.context, commit_id)?
                }
            }
            .into_iter()
            .find(|file| {
                file.old_path.as_deref() == Some(wanted_path)
                    || file.new_path.as_deref() == Some(wanted_path)
            })
            .ok_or_else(|| {
                "No changed file with that path exists in the requested scope".to_owned()
            })?;
            Ok((file, snapshot.context))
        });
        let workspace = self.clone();
        glib::spawn_future_local(async move {
            let result = result
                .await
                .unwrap_or_else(|_| Err("Changes worker stopped".to_owned()));
            match result {
                Ok((file, context)) => {
                    if !workspace.sessions.borrow().contains_key(&params.session_id) {
                        let _ = pending.respond(ControlResponse::failure(
                            request_id,
                            ErrorCode::SessionNotFound,
                            "The session closed before its diff could be opened",
                            Some(serde_json::json!({ "sessionId": params.session_id })),
                        ));
                        return;
                    }
                    workspace.select_session_for_diff(&params.session_id);
                    if let Some(tab_id) =
                        workspace.open_changed_file_in_context(file, context, params.session_id)
                    {
                        let _ = pending.respond(ControlResponse::success(
                            request_id,
                            serde_json::json!({ "opened": true, "tabId": tab_id }),
                        ));
                    } else {
                        let _ = pending.respond(ControlResponse::failure(
                            request_id,
                            ErrorCode::OperationRefused,
                            "Could not open the requested diff tab",
                            None,
                        ));
                    }
                }
                Err(error) => {
                    let _ = pending.respond(ControlResponse::failure(
                        request_id,
                        ErrorCode::OperationRefused,
                        error,
                        None,
                    ));
                }
            }
        });
    }

    fn select_session_for_diff(&self, session_id: &str) {
        let Some(row) = self
            .sessions
            .borrow()
            .get(session_id)
            .map(|session| session.row.clone())
        else {
            return;
        };
        let context = self.context_key_for_session(session_id);
        self.workspace_tabs
            .borrow_mut()
            .attach_session(session_id.to_owned(), context);
        let _ = self.workspace_tabs.borrow_mut().select_session(session_id);
        self.suppress_session_activation.set(true);
        self.list.select_row(Some(&row));
        self.suppress_session_activation.set(false);
        *self.selected_session.borrow_mut() = Some(session_id.to_owned());
        self.save_preference("selectedSessionId", serde_json::json!(session_id));
        self.render_workspace_tabs();
        self.render_history(Some(session_id));
        self.refresh_selected_pr_context(session_id);
        self.search_button.set_sensitive(true);
        self.refresh_changes();
        self.update_emacs_actions();
        self.update_claude_actions();
    }

    pub(super) fn refresh_changes(&self) {
        if !self.changes.split.shows_sidebar()
            && self.stack.visible_child_name().as_deref() != Some("changes-diff")
        {
            return;
        }
        let Some(session_id) = self.selected_session_id() else {
            self.invalidate_changes("session", "Select an agent to inspect its worktree");
            self.changes
                .set_error("Select an agent to inspect its worktree");
            return;
        };
        let record = self
            .sessions
            .borrow()
            .get(&session_id)
            .map(|session| session.record.clone());
        let Some(record) = record else {
            self.invalidate_changes("session", "The selected session is unavailable");
            self.changes
                .set_error("The selected session is unavailable");
            return;
        };
        let path = record.worktree_path.clone().or(record.cwd.clone());
        let Some(path) = path else {
            self.invalidate_changes(
                &format!("session:{session_id}"),
                "This session has no recorded worktree path",
            );
            self.changes
                .set_error("This session has no recorded worktree path");
            return;
        };
        if record
            .worktree_path
            .as_ref()
            .is_some_and(|worktree| !worktree.exists())
        {
            self.invalidate_changes(
                &format!("unavailable:{}", path.display()),
                "The recorded worktree is unavailable",
            );
            self.changes
                .set_error("The recorded worktree is unavailable");
            return;
        }

        let context_key = self
            .workspace_tabs
            .borrow()
            .active_context()
            .unwrap_or("session")
            .to_owned();
        let (generation, should_start, show_loading) = {
            let mut state = self.changes.state.borrow_mut();
            let context_changed = state.context_key.as_deref() != Some(context_key.as_str());
            state.context_key = Some(context_key.clone());
            if state.refresh_in_flight {
                if context_changed {
                    state.request_generation = state.request_generation.wrapping_add(1);
                }
                state.refresh_pending = true;
                (state.request_generation, false, context_changed)
            } else {
                state.request_generation = state.request_generation.wrapping_add(1);
                state.refresh_in_flight = true;
                (
                    state.request_generation,
                    true,
                    context_changed || state.snapshot.is_none(),
                )
            }
        };
        if show_loading {
            self.changes.set_loading("Reading worktree changes…");
        }
        if !should_start {
            return;
        }
        let preferred_base = self
            .changes
            .state
            .borrow()
            .base_ref_by_context
            .get(&context_key)
            .cloned();
        let changes_io = self.changes_io.clone();
        let request_path = path.clone();
        let result = changes_io.submit(move || {
            let snapshot = crate::changes::read_changes_snapshot(
                &request_path,
                preferred_base.as_deref(),
                generation,
                0,
            )?;
            let Some(snapshot) = snapshot else {
                return Ok(None);
            };
            let refs = crate::changes::list_base_refs(&snapshot.context.root)?;
            Ok(Some((snapshot, refs)))
        });
        let workspace = self.clone();
        glib::spawn_future_local(async move {
            let result = result
                .await
                .unwrap_or_else(|_| Err("Changes worker stopped".to_owned()));
            let (still_current, refresh_again) = {
                let mut state = workspace.changes.state.borrow_mut();
                state.refresh_in_flight = false;
                let current = state.request_generation == generation
                    && state.context_key.as_deref() == Some(context_key.as_str());
                let refresh_again = std::mem::take(&mut state.refresh_pending);
                (current, refresh_again)
            };
            if !still_current {
                if refresh_again {
                    workspace.refresh_changes();
                }
                return;
            }
            match result {
                Ok(None) => workspace
                    .changes
                    .set_error("This session is outside a Git worktree"),
                Ok(Some((mut snapshot, refs))) => {
                    if let Some(previous) = workspace.changes.state.borrow().snapshot.as_ref() {
                        preserve_loaded_commit_pages(&mut snapshot, previous);
                    }
                    let selected = snapshot.context.base_ref.clone();
                    workspace.changes.set_base_refs(refs, selected.as_deref());
                    let changed = {
                        let mut state = workspace.changes.state.borrow_mut();
                        let changed = state
                            .snapshot
                            .as_ref()
                            .is_none_or(|previous| !same_snapshot_content(previous, &snapshot));
                        state.snapshot = Some(snapshot.clone());
                        changed
                    };
                    if changed {
                        let open_file_workspace = workspace.clone();
                        let load_workspace = workspace.clone();
                        workspace.changes.render(
                            snapshot.clone(),
                            move |file| open_file_workspace.open_changed_file(file),
                            move |oid, container| load_workspace.load_commit_files(&oid, container),
                            {
                                let load_more_workspace = workspace.clone();
                                move || load_more_workspace.load_more_commits()
                            },
                        );
                    }
                    workspace.check_visible_diff_for_staleness(&snapshot);
                }
                Err(error) => workspace.changes.set_error(&error),
            }
            if refresh_again {
                workspace.refresh_changes();
            }
        });
    }

    pub(super) fn invalidate_changes(&self, context_key: &str, _message: &str) {
        let mut state = self.changes.state.borrow_mut();
        state.context_key = Some(context_key.to_owned());
        state.request_generation = state.request_generation.wrapping_add(1);
        state.refresh_pending = false;
    }

    pub(super) fn select_changes_base(&self) {
        if self.changes.updating_base.get() {
            return;
        }
        let Some(context_key) = self
            .workspace_tabs
            .borrow()
            .active_context()
            .map(str::to_owned)
        else {
            return;
        };
        let selected = self.changes.selected_base();
        self.changes
            .set_base_preference(&context_key, selected.clone());
        self.save_preference(
            "changesBaseRefs",
            serde_json::to_value(self.changes.base_preferences()).unwrap_or_default(),
        );
        self.refresh_changes();
    }

    fn load_commit_files(&self, oid: &str, container: gtk::Box) {
        let snapshot = self.changes.state.borrow().snapshot.clone();
        let Some(snapshot) = snapshot else {
            return;
        };
        let context_key = snapshot.context.root.to_string_lossy().into_owned();
        let oid = oid.to_owned();
        let changes_io = self.changes_io.clone();
        let context = snapshot.context.clone();
        let expected_base = context.base_oid.clone();
        let expected_head = context.head_oid.clone();
        let request_oid = oid.clone();
        let result =
            changes_io.submit(move || crate::changes::read_commit_files(&context, &request_oid));
        let workspace = self.clone();
        glib::spawn_future_local(async move {
            let result = result
                .await
                .unwrap_or_else(|_| Err("Changes worker stopped".to_owned()));
            let current = {
                let state = workspace.changes.state.borrow();
                state.context_key.as_deref() == Some(context_key.as_str())
                    && state.snapshot.as_ref().is_some_and(|current| {
                        current.context.base_oid == expected_base
                            && current.context.head_oid == expected_head
                            && current.commits.iter().any(|commit| commit.oid == oid)
                    })
            };
            if !current {
                return;
            }
            match result {
                Ok(files) => {
                    workspace
                        .changes
                        .state
                        .borrow_mut()
                        .loaded_commits
                        .insert(oid.clone(), files.clone());
                    while let Some(child) = container.first_child() {
                        container.remove(&child);
                    }
                    for file in files {
                        let open_workspace = workspace.clone();
                        changes_ui::append_file_button(
                            &container,
                            file,
                            DiffScope::Commit { oid: oid.clone() },
                            move |file| open_workspace.open_changed_file(file),
                        );
                    }
                }
                Err(error) => {
                    while let Some(child) = container.first_child() {
                        container.remove(&child);
                    }
                    let message =
                        gtk::Label::new(Some(&format!("Could not load commit files: {error}")));
                    message.set_wrap(true);
                    container.append(&message);
                }
            }
        });
    }

    fn load_more_commits(&self) {
        let Some(snapshot) = self.changes.state.borrow().snapshot.clone() else {
            return;
        };
        let context_key = snapshot.context.root.to_string_lossy().into_owned();
        let offset = snapshot.commits.len();
        let context = snapshot.context.clone();
        let expected_base = context.base_oid.clone();
        let expected_head = context.head_oid.clone();
        let changes_io = self.changes_io.clone();
        let result = changes_io.submit(move || crate::changes::read_more_commits(&context, offset));
        let workspace = self.clone();
        glib::spawn_future_local(async move {
            let result = result
                .await
                .unwrap_or_else(|_| Err("Changes worker stopped".to_owned()));
            let current = {
                let state = workspace.changes.state.borrow();
                state.context_key.as_deref() == Some(context_key.as_str())
                    && state.snapshot.as_ref().is_some_and(|current| {
                        current.context.base_oid == expected_base
                            && current.context.head_oid == expected_head
                            && current.commits.len() == offset
                    })
            };
            if !current {
                return;
            }
            match result {
                Ok((commits, has_more)) => {
                    let mut snapshot = snapshot;
                    snapshot.commits.extend(commits);
                    snapshot.has_more_commits = has_more;
                    workspace.changes.state.borrow_mut().snapshot = Some(snapshot.clone());
                    let open_workspace = workspace.clone();
                    let load_workspace = workspace.clone();
                    let more_workspace = workspace.clone();
                    workspace.changes.render(
                        snapshot,
                        move |file| open_workspace.open_changed_file(file),
                        move |oid, container| load_workspace.load_commit_files(&oid, container),
                        move || more_workspace.load_more_commits(),
                    );
                }
                Err(error) => workspace.changes.set_error(&error),
            }
        });
    }

    fn activate_diff_tab(&self, key: &DiffTabKey) {
        let tab = WorkspaceTabId::Diff(key.clone());
        if !self.workspace_tabs.borrow_mut().select_tab(&tab) {
            return;
        }
        if !matches!(&key.scope, DiffScope::Commit { .. })
            && let Some(snapshot) = self.changes.state.borrow().snapshot.clone()
            && snapshot.context.root.to_string_lossy() == key.worktree_root
        {
            if let Some(file) = file_from_snapshot(&snapshot, key) {
                self.diff_tab_data
                    .borrow_mut()
                    .insert(key.clone(), (file, snapshot.context));
                self.stale_diff_tabs.borrow_mut().remove(key);
            } else {
                self.stale_diff_tabs.borrow_mut().insert(key.clone());
            }
        }
        self.diff_document_signatures.borrow_mut().remove(key);
        if self.stack.visible_child_name().as_deref() == Some("changes-diff")
            && let Some(viewer) = self.diff_viewer.borrow().as_ref()
        {
            viewer.save_view_state();
        }
        self.stack.set_visible_child_name("changes-diff");
        self.session_context_bar.set_visible(false);
        self.context_pr.set_visible(false);
        self.search_bar.set_search_mode(false);
        let path = key
            .new_path
            .as_deref()
            .or(key.old_path.as_deref())
            .map(escape_path)
            .unwrap_or_else(|| "Diff".to_owned());
        self.diff_heading.set_text(&path);
        self.content_title.set_title(&path);
        self.diff_placeholder.set_text("Loading diff…");
        self.diff_placeholder.set_visible(true);
        self.diff_viewer_host.set_visible(false);
        self.render_workspace_tabs();
        self.set_diff_actions_enabled(true);
        self.update_diff_stale_banner(key);
        self.load_diff_tab(key.clone());
    }

    fn load_diff_tab(&self, key: DiffTabKey) {
        let Some((file, context)) = self.diff_tab_data.borrow().get(&key).cloned() else {
            return;
        };
        let request = self.diff_request_sequence.get().wrapping_add(1);
        self.diff_request_sequence.set(request);
        *self.diff_current_request.borrow_mut() = None;
        *self.diff_rendered.borrow_mut() = None;
        *self.diff_error.borrow_mut() = None;
        self.diff_placeholder.set_text("Loading diff…");
        self.diff_placeholder.set_visible(true);
        self.diff_viewer_host.set_visible(false);
        let changes_io = self.changes_io.clone();
        let result = changes_io.submit(move || crate::changes::read_diff_document(&context, &file));
        let workspace = self.clone();
        glib::spawn_future_local(async move {
            let result = result
                .await
                .unwrap_or_else(|_| Err("Changes worker stopped".to_owned()));
            if workspace.workspace_tabs.borrow().visible_tab()
                != Some(&WorkspaceTabId::Diff(key.clone()))
            {
                return;
            }
            match result {
                Ok(document) => {
                    workspace
                        .diff_document_signatures
                        .borrow_mut()
                        .insert(key.clone(), diff_result_signature(&document));
                    workspace.present_diff_result(&key, document, request);
                }
                Err(error) => {
                    workspace.show_diff_placeholder(&key, "Could not load diff", &error, None, &[])
                }
            }
        });
    }

    fn check_visible_diff_for_staleness(&self, snapshot: &crate::changes::ChangesSnapshot) {
        let active = self.workspace_tabs.borrow().visible_tab().cloned();
        if let Some(WorkspaceTabId::Diff(key)) = active {
            self.check_diff_for_staleness(&key, snapshot);
        }
    }

    fn check_diff_for_staleness(
        &self,
        key: &DiffTabKey,
        snapshot: &crate::changes::ChangesSnapshot,
    ) {
        if matches!(&key.scope, DiffScope::Commit { .. })
            || key.worktree_root != snapshot.context.root.to_string_lossy().into_owned()
        {
            return;
        }
        let Some(expected_signature) = self.diff_document_signatures.borrow().get(key).cloned()
        else {
            return;
        };
        let Some(file) = file_from_snapshot(snapshot, key) else {
            self.stale_diff_tabs.borrow_mut().insert(key.clone());
            self.update_diff_stale_banner(key);
            return;
        };
        let context = snapshot.context.clone();
        let changes_io = self.changes_io.clone();
        let result = changes_io.submit(move || crate::changes::read_diff_document(&context, &file));
        let workspace = self.clone();
        let key = key.clone();
        glib::spawn_future_local(async move {
            let result = result
                .await
                .unwrap_or_else(|_| Err("Changes worker stopped".to_owned()));
            let cache_still_matches = workspace
                .diff_document_signatures
                .borrow()
                .get(&key)
                .is_some_and(|current| current == &expected_signature);
            if !cache_still_matches {
                return;
            }
            let stale = match result {
                Ok(current) => diff_result_signature(&current) != expected_signature,
                Err(_) => true,
            };
            if stale {
                workspace.stale_diff_tabs.borrow_mut().insert(key.clone());
            } else {
                workspace.stale_diff_tabs.borrow_mut().remove(&key);
            }
            workspace.update_diff_stale_banner(&key);
        });
    }

    fn update_diff_stale_banner(&self, key: &DiffTabKey) {
        let active = self.workspace_tabs.borrow().visible_tab().cloned();
        let stale = self.stale_diff_tabs.borrow().contains(key);
        let visible = self.stack.visible_child_name().as_deref() == Some("changes-diff")
            && active == Some(WorkspaceTabId::Diff(key.clone()))
            && stale;
        self.diff_stale_label
            .set_text("The worktree or comparison base changed. Reload to update this diff.");
        self.diff_reload_button.set_sensitive(stale);
        self.diff_stale_banner.set_visible(visible);
    }

    pub(super) fn reload_active_diff(&self) {
        let Some(WorkspaceTabId::Diff(key)) = self.workspace_tabs.borrow().visible_tab().cloned()
        else {
            return;
        };
        let snapshot = self.changes.state.borrow().snapshot.clone();
        let Some(snapshot) = snapshot
            .filter(|snapshot| snapshot.context.root.to_string_lossy() == key.worktree_root)
        else {
            self.show_diff_placeholder(
                &key,
                &key.new_path
                    .as_deref()
                    .or(key.old_path.as_deref())
                    .map(escape_path)
                    .unwrap_or_else(|| "Diff".to_owned()),
                "Refresh the Changes sidebar before reloading this diff",
                None,
                &[],
            );
            return;
        };
        let Some(file) = file_from_snapshot(&snapshot, &key) else {
            self.diff_document_signatures.borrow_mut().remove(&key);
            self.stale_diff_tabs.borrow_mut().remove(&key);
            self.update_diff_stale_banner(&key);
            let path = key
                .new_path
                .as_deref()
                .or(key.old_path.as_deref())
                .map(escape_path)
                .unwrap_or_else(|| "Diff".to_owned());
            self.show_diff_placeholder(
                &key,
                &path,
                "This file is no longer present in the current scope",
                None,
                &[],
            );
            return;
        };
        self.diff_tab_data
            .borrow_mut()
            .insert(key.clone(), (file, snapshot.context));
        self.diff_document_signatures.borrow_mut().remove(&key);
        self.stale_diff_tabs.borrow_mut().remove(&key);
        self.update_diff_stale_banner(&key);
        self.load_diff_tab(key);
    }

    fn present_diff_result(&self, key: &DiffTabKey, result: DiffDocumentResult, request: u64) {
        match result {
            DiffDocumentResult::Text(document) => self.show_diff_document(key, document, request),
            DiffDocumentResult::Placeholder(placeholder) => self.show_diff_placeholder(
                key,
                &placeholder.path,
                &placeholder.reason,
                placeholder.byte_size,
                &placeholder.metadata,
            ),
        }
    }

    fn show_diff_document(
        &self,
        key: &DiffTabKey,
        document: crate::changes::DiffDocument,
        request: u64,
    ) {
        self.diff_document_signatures.borrow_mut().insert(
            key.clone(),
            diff_result_signature(&DiffDocumentResult::Text(document.clone())),
        );
        if document.original == document.modified && document.metadata.is_empty() {
            self.show_diff_placeholder(key, &document.path, "No content changes", None, &[]);
            return;
        }
        let tab_id = crate::workspace_tabs::WorkspaceTabs::diff_tab_id(key);
        let request_id = format!("{tab_id}-{request}");
        *self.diff_current_request.borrow_mut() = Some(request_id.clone());
        *self.diff_rendered.borrow_mut() = None;
        *self.diff_error.borrow_mut() = None;
        if self.diff_viewer.borrow().is_none() {
            let view_states = self.diff_view_states.clone();
            let current_request = self.diff_current_request.clone();
            let rendered = self.diff_rendered.clone();
            let error_state = self.diff_error.clone();
            let viewer = diff_viewer::DiffViewer::new(move |event| match event {
                diff_viewer::ViewerEvent::ViewState { tab_id, state } => {
                    view_states.borrow_mut().insert(tab_id, state);
                }
                diff_viewer::ViewerEvent::DiffRendered {
                    request_id,
                    line_changes,
                    character_changes,
                    ..
                } => {
                    if current_request.borrow().as_deref() == Some(request_id.as_str()) {
                        *rendered.borrow_mut() =
                            Some((request_id, line_changes, character_changes));
                    }
                }
                diff_viewer::ViewerEvent::Error {
                    request_id: Some(request_id),
                    message,
                } => {
                    if current_request.borrow().as_deref() == Some(request_id.as_str()) {
                        *error_state.borrow_mut() = Some((request_id, message));
                    }
                }
                diff_viewer::ViewerEvent::Ready { .. }
                | diff_viewer::ViewerEvent::Error {
                    request_id: None, ..
                } => {}
            });
            self.diff_viewer_host.append(&viewer.widget());
            *self.diff_viewer.borrow_mut() = Some(viewer);
        }
        let appearance = self.appearance.borrow().clone();
        let description = FontDescription::from_string(&appearance.font);
        let font_family = description
            .family()
            .map(|family| family.to_string())
            .unwrap_or_else(|| "monospace".to_owned());
        let font_size = (description.size() as f64 / gtk::pango::SCALE as f64)
            .round()
            .clamp(8.0, 48.0) as u32;
        if let Some(viewer) = self.diff_viewer.borrow().as_ref() {
            viewer.set_appearance(
                if matches!(
                    self.effective_terminal_theme(),
                    ThemeKey::NeutralLight | ThemeKey::SolarizedLight | ThemeKey::Light
                ) {
                    "light"
                } else {
                    "dark"
                },
                &font_family,
                font_size,
            );
            let state = self.diff_view_states.borrow().get(&tab_id).cloned();
            let payload = serde_json::json!({
                "requestId": request_id,
                "tabId": tab_id,
                "path": document.path,
                "original": document.original,
                "modified": document.modified,
                "originalLabel": document.original_label,
                "modifiedLabel": document.modified_label,
                "language": document.language,
                "metadata": document.metadata,
                "viewState": state,
            });
            if let Err(error) = viewer.show_diff(&payload) {
                self.show_diff_placeholder(key, &document.path, &error, None, &[]);
                return;
            }
            self.diff_placeholder.set_visible(false);
            self.diff_viewer_host.set_visible(true);
            viewer.focus();
        }
    }

    fn show_diff_placeholder(
        &self,
        _key: &DiffTabKey,
        path: &str,
        reason: &str,
        byte_size: Option<u64>,
        metadata: &[String],
    ) {
        *self.diff_current_request.borrow_mut() = None;
        *self.diff_rendered.borrow_mut() = None;
        let size = byte_size
            .map(|size| format!(" ({size} bytes)"))
            .unwrap_or_default();
        let metadata = if metadata.is_empty() {
            String::new()
        } else {
            format!("\n{}", metadata.join("\n"))
        };
        self.diff_heading.set_text(path);
        self.diff_placeholder
            .set_text(&format!("{path}\n{reason}{size}{metadata}"));
        self.diff_placeholder.set_visible(true);
        self.diff_viewer_host.set_visible(false);
        if let Some(viewer) = self.diff_viewer.borrow().as_ref() {
            viewer.clear();
        }
    }

    fn close_diff_tab(&self, key: &DiffTabKey) {
        if let Some(viewer) = self.diff_viewer.borrow().as_ref() {
            viewer.save_view_state();
        }
        self.diff_tab_data.borrow_mut().remove(key);
        self.diff_document_signatures.borrow_mut().remove(key);
        self.stale_diff_tabs.borrow_mut().remove(key);
        let tab_id = crate::workspace_tabs::WorkspaceTabs::diff_tab_id(key);
        self.diff_view_states.borrow_mut().remove(&tab_id);
        let next = self.workspace_tabs.borrow_mut().close_diff(key);
        self.render_workspace_tabs();
        match next {
            Some(WorkspaceTabId::Session(id)) => self.activate_session(&id),
            Some(WorkspaceTabId::Diff(key)) => self.activate_diff_tab(&key),
            None => {
                if let Some(viewer) = self.diff_viewer.borrow().as_ref() {
                    viewer.clear();
                }
                self.stack.set_visible_child_name(EMPTY_PAGE);
                self.diff_viewer_host.set_visible(false);
                self.diff_placeholder.set_visible(true);
                self.session_context_bar.set_visible(false);
                self.context_pr.set_visible(false);
                self.refresh_content_title();
                self.set_diff_actions_enabled(false);
            }
        }
    }

    pub(super) fn render_workspace_tabs(&self) {
        while let Some(child) = self.center_tabs.first_child() {
            self.center_tabs.remove(&child);
        }
        let Some(context) = self
            .workspace_tabs
            .borrow()
            .active_context()
            .map(str::to_owned)
        else {
            return;
        };
        let tabs = self.workspace_tabs.borrow().context_tabs(&context);
        let active = self.workspace_tabs.borrow().visible_tab().cloned();
        for tab in tabs {
            match &tab {
                WorkspaceTabId::Session(session_id) => {
                    let Some(session_name) = self
                        .sessions
                        .borrow()
                        .get(session_id)
                        .map(|session| session.record.name.clone())
                    else {
                        continue;
                    };
                    let button = gtk::ToggleButton::with_label(&session_name);
                    button.set_active(active.as_ref() == Some(&tab));
                    button.set_tooltip_text(Some(&session_name));
                    button.set_accessible_role(gtk::AccessibleRole::Tab);
                    let workspace = self.clone();
                    let session_id = session_id.clone();
                    button.connect_clicked(move |_| {
                        if let Some(row) = workspace
                            .sessions
                            .borrow()
                            .get(&session_id)
                            .map(|s| s.row.clone())
                        {
                            workspace.list.select_row(Some(&row));
                        }
                        workspace.activate_session(&session_id);
                    });
                    self.center_tabs.append(&button);
                }
                WorkspaceTabId::Diff(key) => {
                    let row = gtk::Box::new(gtk::Orientation::Horizontal, 0);
                    row.add_css_class("linked");
                    let label = diff_tab_label(key);
                    let button = gtk::ToggleButton::with_label(&label);
                    button.set_active(active.as_ref() == Some(&tab));
                    button.set_tooltip_text(Some(&diff_tab_tooltip(key)));
                    button.set_accessible_role(gtk::AccessibleRole::Tab);
                    let workspace = self.clone();
                    let key_for_click = key.clone();
                    button.connect_clicked(move |_| workspace.activate_diff_tab(&key_for_click));
                    let close = gtk::Button::builder()
                        .icon_name("window-close-symbolic")
                        .tooltip_text("Close diff tab")
                        .build();
                    close.add_css_class("flat");
                    let workspace = self.clone();
                    let key_for_close = key.clone();
                    close.connect_clicked(move |_| workspace.close_diff_tab(&key_for_close));
                    row.append(&button);
                    row.append(&close);
                    self.center_tabs.append(&row);
                }
            }
        }
    }

    fn context_key_for_session(&self, session_id: &str) -> String {
        let Some(record) = self
            .sessions
            .borrow()
            .get(session_id)
            .map(|session| session.record.clone())
        else {
            return format!("session:{session_id}");
        };
        let recorded_worktree = record.worktree_path.as_ref();
        let Some(path) = recorded_worktree.or(record.cwd.as_ref()) else {
            return format!("session:{session_id}");
        };
        let Some(path) = path.canonicalize().ok() else {
            return if recorded_worktree.is_some() {
                format!("unavailable:{}", path.to_string_lossy())
            } else {
                format!("session:{session_id}")
            };
        };
        match crate::changes::resolve_worktree_context(&path, None, 0) {
            Ok(Some(context)) => context.root.to_string_lossy().into_owned(),
            _ if recorded_worktree.is_some() => format!("unavailable:{}", path.to_string_lossy()),
            _ => format!("session:{session_id}"),
        }
    }

    fn set_diff_actions_enabled(&self, enabled: bool) {
        if let Some(action) = self
            .window
            .lookup_action("close-session")
            .and_downcast::<gio::SimpleAction>()
        {
            action.set_enabled(!enabled);
        }
        if let Some(action) = self
            .window
            .lookup_action("session-close")
            .and_downcast::<gio::SimpleAction>()
        {
            action.set_enabled(!enabled);
        }
        self.history_button
            .set_sensitive(!enabled && self.selected_session_id().is_some());
        self.search_button
            .set_sensitive(!enabled && self.selected_session_id().is_some());
        self.claude_model_button
            .set_sensitive(!enabled && self.selected_session_id().is_some());
    }
}

fn diff_tab_label(key: &DiffTabKey) -> String {
    let path = key
        .new_path
        .as_deref()
        .or(key.old_path.as_deref())
        .map(escape_path)
        .unwrap_or_else(|| "Diff".to_owned());
    let basename = path.rsplit('/').next().unwrap_or(&path);
    format!("{} · {}", basename, scope_label(&key.scope))
}

fn diff_tab_tooltip(key: &DiffTabKey) -> String {
    let path = key
        .new_path
        .as_deref()
        .or(key.old_path.as_deref())
        .map(escape_path)
        .unwrap_or_else(|| "Diff".to_owned());
    format!("{} · {}", scope_label(&key.scope), path)
}

fn scope_label(scope: &DiffScope) -> &'static str {
    match scope {
        DiffScope::All => "All changes",
        DiffScope::Staged => "Staged",
        DiffScope::Unstaged => "Unstaged",
        DiffScope::Untracked => "Untracked",
        DiffScope::Committed => "Branch",
        DiffScope::Commit { .. } => "Commit",
    }
}

fn escape_path(raw: &[u8]) -> String {
    let mut output = String::new();
    for byte in raw {
        if byte.is_ascii_graphic() && *byte != b'\\' {
            output.push(char::from(*byte));
        } else if *byte == b' ' {
            output.push(' ');
        } else if *byte == b'\\' {
            output.push_str("\\\\");
        } else {
            output.push_str(&format!("\\x{byte:02X}"));
        }
    }
    output
}

fn file_from_snapshot(
    snapshot: &crate::changes::ChangesSnapshot,
    key: &DiffTabKey,
) -> Option<ChangedFile> {
    let files = match &key.scope {
        DiffScope::All => &snapshot.all_changes,
        DiffScope::Staged => &snapshot.staged,
        DiffScope::Unstaged => &snapshot.unstaged,
        DiffScope::Untracked => &snapshot.untracked,
        DiffScope::Committed => &snapshot.branch_changes,
        DiffScope::Commit { .. } => return None,
    };
    files
        .iter()
        .find(|file| file.old_path == key.old_path && file.new_path == key.new_path)
        .cloned()
}

fn same_snapshot_content(
    left: &crate::changes::ChangesSnapshot,
    right: &crate::changes::ChangesSnapshot,
) -> bool {
    let mut left = left.clone();
    let mut right = right.clone();
    left.context.generation = 0;
    right.context.generation = 0;
    left == right
}

fn diff_result_signature(result: &DiffDocumentResult) -> String {
    match result {
        DiffDocumentResult::Text(document) => format!(
            "text|{}|{}|{}|{}|{}",
            document.identity,
            document.original_label,
            document.modified_label,
            document.metadata.join("\u{1f}"),
            document.language.as_deref().unwrap_or_default(),
        ),
        DiffDocumentResult::Placeholder(placeholder) => format!(
            "placeholder|{}|{}|{}|{}|{}|{}",
            placeholder.identity,
            placeholder.reason,
            placeholder.original_label,
            placeholder.modified_label,
            placeholder.metadata.join("\u{1f}"),
            placeholder.byte_size.unwrap_or_default(),
        ),
    }
}

fn preserve_loaded_commit_pages(
    refreshed: &mut crate::changes::ChangesSnapshot,
    previous: &crate::changes::ChangesSnapshot,
) {
    let same_range = refreshed.context.root == previous.context.root
        && refreshed.context.base_oid == previous.context.base_oid
        && refreshed.context.head_oid == previous.context.head_oid;
    if same_range
        && previous.commits.len() >= refreshed.commits.len()
        && previous.commits.starts_with(&refreshed.commits)
    {
        refreshed.commits.clone_from(&previous.commits);
        refreshed.has_more_commits = previous.has_more_commits;
    }
}
