use super::*;
use crate::worktrees::{DeleteBranch, ReapRequest, WorktreeInfo, WorktreeManager, WorktreeState};

#[derive(Clone, Copy)]
enum RemovalMode {
    Safe,
    Regardless,
}

impl RemovalMode {
    fn needs_confirmation(self, dirty: bool, ignored_only: bool) -> bool {
        matches!(self, Self::Safe) && (dirty || ignored_only)
    }
}

/// The offer to remove a worktree along with its last session, while it is open.
#[derive(Clone)]
pub(super) struct ClosePrompt {
    dialog: adw::AlertDialog,
    status: gtk::Label,
    delete_branch: gtk::CheckButton,
    /// The inventory row the removal is guarded against; None until the scan lands.
    preview: Rc<RefCell<Option<WorktreeInfo>>>,
    requested: Rc<RefCell<Option<RemovalMode>>>,
    closed: Rc<std::cell::Cell<bool>>,
}

impl ClosePrompt {
    pub(super) fn inspection_node(&self, window: &adw::ApplicationWindow) -> UiNode {
        UiNode {
            id: "close-prompt".to_owned(),
            role: "dialog".to_owned(),
            label: Some(self.dialog.heading().map(String::from).unwrap_or_default()),
            is_visible: self.dialog.is_mapped(),
            is_enabled: self.dialog.is_sensitive(),
            is_selected: false,
            bounds: widget_bounds(&self.dialog, window),
            children: vec![
                UiNode {
                    id: "close-prompt-status".to_owned(),
                    role: "status".to_owned(),
                    label: Some(self.status.text().to_string()),
                    is_visible: self.status.is_visible(),
                    is_enabled: true,
                    is_selected: false,
                    bounds: widget_bounds(&self.status, window),
                    children: Vec::new(),
                },
                UiNode {
                    id: "close-prompt-delete-branch".to_owned(),
                    role: "checkbox".to_owned(),
                    label: Some(
                        self.delete_branch
                            .label()
                            .map(String::from)
                            .unwrap_or_default(),
                    ),
                    is_visible: self.delete_branch.is_visible(),
                    is_enabled: self.delete_branch.is_sensitive(),
                    is_selected: self.delete_branch.is_active(),
                    bounds: widget_bounds(&self.delete_branch, window),
                    children: Vec::new(),
                },
                UiNode {
                    id: "close-prompt-remove".to_owned(),
                    role: "button".to_owned(),
                    label: Some(self.dialog.response_label("remove").to_string()),
                    is_visible: self.dialog.is_mapped(),
                    is_enabled: self.dialog.is_response_enabled("remove"),
                    is_selected: false,
                    bounds: widget_bounds(&self.dialog, window),
                    children: Vec::new(),
                },
                UiNode {
                    id: "close-prompt-remove-regardless".to_owned(),
                    role: "button".to_owned(),
                    label: Some(self.dialog.response_label("remove-regardless").to_string()),
                    is_visible: self.dialog.is_mapped(),
                    is_enabled: self.dialog.is_response_enabled("remove-regardless"),
                    is_selected: false,
                    bounds: widget_bounds(&self.dialog, window),
                    children: Vec::new(),
                },
            ],
        }
    }
}

impl Workspace {
    /// Closes a session from the UI. The last session in a linked worktree is asked
    /// whether the worktree goes too; everything else stops right away.
    pub(super) fn close_session(&self, id: &str) {
        if !self.offer_worktree_removal(id) {
            self.stop_session(id, None);
        }
    }

    /// Opens the close prompt when the session is the last one in its worktree.
    pub(super) fn offer_worktree_removal(&self, id: &str) -> bool {
        let target = {
            let sessions = self.sessions.borrow();
            let Some(record) = sessions.get(id).map(|s| s.record.clone()) else {
                return false;
            };
            let others = sessions
                .values()
                .map(|s| &s.record)
                .filter(|other| other.id != record.id);
            abandoned_worktree(&record, others).map(|worktree| (record, worktree))
        };
        let Some((record, worktree)) = target else {
            return false;
        };
        self.present_close_prompt(record, worktree);
        true
    }

    fn present_close_prompt(&self, record: SessionRecord, worktree: PathBuf) {
        let worktree_name = worktree
            .file_name()
            .and_then(|name| name.to_str())
            .map(str::to_owned)
            .unwrap_or_else(|| worktree.to_string_lossy().into_owned());
        let dialog = adw::AlertDialog::new(
            Some("Close the Last Session in This Worktree?"),
            Some(&format!(
                "{} is the last session in {worktree_name}. The worktree can be removed along with it.",
                record.name
            )),
        );
        let status = gtk::Label::builder()
            .label("Checking the worktree…")
            .wrap(true)
            .xalign(0.0)
            .build();
        let delete_branch = gtk::CheckButton::builder()
            .visible(false)
            .tooltip_text("The branch tip is saved as an attic tag first, so it stays recoverable.")
            .build();
        let content = gtk::Box::new(gtk::Orientation::Vertical, 8);
        content.append(&status);
        content.append(&delete_branch);
        dialog.set_extra_child(Some(&content));
        dialog.add_responses(&[
            ("cancel", "_Cancel"),
            ("close", "Close _Session"),
            ("remove", "_Remove Worktree Safely"),
            ("remove-regardless", "Remove Worktree _Regardless"),
        ]);
        dialog.set_response_appearance("close", adw::ResponseAppearance::Suggested);
        dialog.set_response_appearance("remove", adw::ResponseAppearance::Destructive);
        dialog.set_response_appearance("remove-regardless", adw::ResponseAppearance::Destructive);
        dialog.set_default_response(Some("close"));
        dialog.set_close_response("cancel");
        let prompt = ClosePrompt {
            dialog: dialog.clone(),
            status,
            delete_branch,
            preview: Rc::default(),
            requested: Rc::default(),
            closed: Rc::default(),
        };

        let workspace = self.clone();
        let id = record.id.clone();
        dialog.connect_response(Some("close"), move |_, _| {
            workspace.stop_session(&id, None);
        });
        for (response, mode) in [
            ("remove", RemovalMode::Safe),
            ("remove-regardless", RemovalMode::Regardless),
        ] {
            let workspace = self.clone();
            let id = record.id.clone();
            prompt.connect_removal_response(response, mode, move |prompt| {
                workspace.resolve_worktree_removal(prompt, &id);
            });
        }
        let workspace = self.clone();
        let closed = prompt.clone();
        let id = record.id.clone();
        dialog.connect_closed(move |_| {
            closed.closed.set(true);
            workspace.close_prompt.borrow_mut().take();
            workspace.resolve_worktree_removal(closed.clone(), &id);
        });
        *self.close_prompt.borrow_mut() = Some(prompt.clone());
        dialog.present(Some(&self.window));
        self.scan_closing_worktree(prompt, record, worktree);
    }

    /// Fetches the guarded inventory row so the removal checks the state shown here.
    /// The closing session is left out, so the row describes the worktree after the close.
    fn scan_closing_worktree(&self, prompt: ClosePrompt, record: SessionRecord, worktree: PathBuf) {
        let manager = WorktreeManager::new(self.paths.attic_dir());
        let live_paths = self
            .sessions
            .borrow()
            .values()
            .filter(|session| session.record.id != record.id)
            .filter(|session| session.record.state != SessionState::Exited)
            .filter_map(|session| session.record.worktree_path.clone())
            .collect::<Vec<_>>();
        let repo = record.project_root.clone();
        self.run_slow(
            move || {
                let repo = match repo {
                    Some(repo) => repo,
                    None => manager.repository_root(&worktree)?,
                };
                let inventory = manager.list(&repo, &live_paths)?;
                let wanted = worktree.canonicalize().unwrap_or(worktree);
                Ok(inventory
                    .worktrees
                    .into_iter()
                    .find(|row| row.path.canonicalize().ok().as_ref() == Some(&wanted)))
            },
            move |workspace, result| {
                match result {
                    Ok(Some(row)) => prompt.show_scan(row),
                    Ok(None) => {
                        prompt.refuse("The worktree is not listed by its repository, so it stays.")
                    }
                    Err(error) => prompt.refuse(&format!(
                        "The worktree could not be checked, so it stays: {error}"
                    )),
                }
                if prompt.preview.borrow().is_some() {
                    workspace.resolve_worktree_removal(prompt, &record.id);
                } else if prompt.requested.borrow_mut().take().is_some() {
                    workspace.show_error(&prompt.status.text());
                }
            },
        );
    }

    fn resolve_worktree_removal(&self, prompt: ClosePrompt, id: &str) {
        if !prompt.closed.get() {
            return; // Wait for the response's close animation before presenting again.
        }
        let Some(preview) = prompt.preview.borrow().clone() else {
            return; // The scan will resolve the queued choice.
        };
        let Some(mode) = prompt.requested.borrow_mut().take() else {
            return;
        };
        if mode.needs_confirmation(preview.dirty, preview.ignored_only) {
            prompt
                .dialog
                .set_heading(Some("This Worktree Has Uncommitted Files"));
            prompt.dialog.set_body(
                "Keep the worktree by closing only the session, or remove it regardless. Files are saved to the attic before removal.",
            );
            prompt.dialog.set_response_enabled("remove", false);
            prompt.closed.set(false);
            *self.close_prompt.borrow_mut() = Some(prompt.clone());
            prompt.dialog.present(Some(&self.window));
            return;
        }
        let delete_branch = match preview.branch {
            Some(_) if prompt.delete_branch.is_active() => DeleteBranch::Force,
            Some(_) => DeleteBranch::Never,
            None => DeleteBranch::Auto,
        };
        self.stop_session_then(id, None, move |workspace, _| {
            workspace.remove_closed_worktree(preview, delete_branch);
        });
    }

    fn remove_closed_worktree(&self, preview: WorktreeInfo, delete_branch: DeleteBranch) {
        let name = preview
            .path
            .file_name()
            .and_then(|name| name.to_str())
            .map(str::to_owned)
            .unwrap_or_else(|| preview.path.to_string_lossy().into_owned());
        let manager = WorktreeManager::new(self.paths.attic_dir());
        let live_paths = self.live_worktree_paths();
        let request = ReapRequest {
            path: preview.path,
            expected_head: preview.head.unwrap_or_default(),
            expected_status_hash: preview.status_hash,
            delete_branch,
            confirmed: true,
        };
        self.run_slow(
            move || manager.reap(request, &live_paths),
            move |workspace, result| {
                let message = match result {
                    Ok(result) if result.ok => match result.reason {
                        Some(reason) => format!("Worktree {name} removed, {reason}"),
                        None if result.branch_deleted => {
                            format!("Worktree {name} and its branch removed")
                        }
                        None => format!("Worktree {name} removed"),
                    },
                    Ok(result) => format!(
                        "Worktree {name} kept: {}",
                        result.reason.as_deref().unwrap_or("removal aborted")
                    ),
                    Err(error) => format!("Worktree {name} kept: {error}"),
                };
                workspace.show_error(&message);
                if workspace.worktrees.modal.is_visible() {
                    workspace.refresh_worktree_panel();
                }
            },
        );
    }
}

impl ClosePrompt {
    fn connect_removal_response(
        &self,
        response: &str,
        mode: RemovalMode,
        resolve: impl Fn(Self) + 'static,
    ) {
        let prompt = self.clone();
        self.dialog.connect_response(Some(response), move |_, _| {
            *prompt.requested.borrow_mut() = Some(mode);
            // AlertDialog can emit closed before response; the choice must resolve
            // from either event, and the preview scan handles a choice made early.
            resolve(prompt.clone());
        });
    }

    fn show_scan(&self, row: WorktreeInfo) {
        let blocked = if row.is_primary {
            Some("This is the main checkout of the project, so it stays.")
        } else if row.live_session_count > 0 {
            Some("Another session still uses this worktree, so it stays.")
        } else {
            None
        };
        if let Some(reason) = blocked {
            self.refuse(reason);
            return;
        }
        let mut lines = vec![if row.dirty || row.ignored_only {
            "The worktree has uncommitted changes. They are saved to the attic before removal."
                .to_owned()
        } else {
            "The worktree is clean.".to_owned()
        }];
        if row.state != WorktreeState::Unknown {
            lines.push(format!("Lifecycle: {}.", row.evidence));
        }
        self.status.set_text(&lines.join("\n"));
        if let Some(branch) = &row.branch {
            self.delete_branch
                .set_label(Some(&format!("Also delete branch {branch}")));
            self.delete_branch
                .set_active(row.state == WorktreeState::Merged);
            self.delete_branch.set_visible(true);
        }
        *self.preview.borrow_mut() = Some(row);
    }

    fn refuse(&self, reason: &str) {
        self.status.set_text(reason);
        self.delete_branch.set_visible(false);
        self.dialog.set_response_enabled("remove", false);
        self.dialog.set_response_enabled("remove-regardless", false);
    }
}

/// The worktree a closing session leaves behind: a linked checkout no other open
/// session uses. The project's main checkout is never offered.
pub(super) fn abandoned_worktree<'a>(
    record: &SessionRecord,
    others: impl IntoIterator<Item = &'a SessionRecord>,
) -> Option<PathBuf> {
    let worktree = record.worktree_path.clone()?;
    if record.project_root.as_deref() == Some(worktree.as_path()) {
        return None;
    }
    let shared = others.into_iter().any(|other| {
        other.state != SessionState::Exited
            && other.worktree_path.as_deref() == Some(worktree.as_path())
    });
    (!shared).then_some(worktree)
}

#[cfg(test)]
mod tests {
    use super::abandoned_worktree;
    use crate::control::SessionState;
    use crate::persist::SessionRecord;
    use std::path::PathBuf;

    #[test]
    fn safe_removal_requires_confirmation_for_dirty_or_ignored_contents() {
        assert!(!super::RemovalMode::Safe.needs_confirmation(false, false));
        assert!(super::RemovalMode::Safe.needs_confirmation(true, false));
        assert!(super::RemovalMode::Safe.needs_confirmation(false, true));
        assert!(!super::RemovalMode::Regardless.needs_confirmation(true, true));
    }

    #[test]
    #[ignore = "requires a private display"]
    fn selecting_remove_regardless_dispatches_after_dialog_closes() {
        use super::{ClosePrompt, RemovalMode};
        use adw::prelude::*;
        use std::{
            cell::Cell,
            rc::Rc,
            time::{Duration, Instant},
        };
        adw::init().unwrap();
        let window = adw::ApplicationWindow::builder()
            .default_width(900)
            .default_height(700)
            .build();
        let dialog = adw::AlertDialog::new(Some("Close worktree"), None);
        dialog.add_responses(&[
            ("cancel", "Cancel"),
            ("remove-regardless", "Remove Regardless"),
        ]);
        dialog.set_close_response("cancel");
        let prompt = ClosePrompt {
            dialog: dialog.clone(),
            status: gtk::Label::new(None),
            delete_branch: gtk::CheckButton::new(),
            preview: Rc::default(),
            requested: Rc::default(),
            closed: Rc::default(),
        };
        let dispatched = Rc::new(Cell::new(0));
        let dispatch = {
            let dispatched = dispatched.clone();
            move |prompt: ClosePrompt| {
                if prompt.closed.get() && prompt.requested.borrow_mut().take().is_some() {
                    dispatched.set(dispatched.get() + 1);
                }
            }
        };
        prompt.connect_removal_response(
            "remove-regardless",
            RemovalMode::Regardless,
            dispatch.clone(),
        );
        let closing = prompt.clone();
        dialog.connect_closed(move |_| {
            closing.closed.set(true);
            dispatch(closing.clone());
        });
        window.present();
        dialog.present(Some(&window));
        fn button(widget: &gtk::Widget) -> Option<gtk::Button> {
            if let Some(button) = widget.downcast_ref::<gtk::Button>()
                && button.label().as_deref() == Some("Remove Regardless")
            {
                return Some(button.clone());
            }
            let mut child = widget.first_child();
            while let Some(widget) = child {
                if let Some(found) = button(&widget) {
                    return Some(found);
                }
                child = widget.next_sibling();
            }
            None
        }
        let context = glib::MainContext::default();
        while context.pending() {
            context.iteration(false);
        }
        button(dialog.upcast_ref())
            .expect("removal button")
            .emit_clicked();
        let deadline = Instant::now() + Duration::from_secs(2);
        while !prompt.closed.get() && Instant::now() < deadline {
            context.iteration(false);
            std::thread::sleep(Duration::from_millis(5));
        }
        window.destroy();
        assert!(prompt.closed.get());
        assert_eq!(
            dispatched.get(),
            1,
            "the removal choice must be dispatched, regardless of signal order"
        );
    }

    fn record(id: &str, worktree: Option<&str>, state: SessionState) -> SessionRecord {
        let mut record = SessionRecord::discovered(id, PathBuf::from(format!("/run/{id}.sock")));
        record.project_root = Some(PathBuf::from("/repo"));
        record.worktree_path = worktree.map(PathBuf::from);
        record.state = state;
        record
    }

    #[test]
    fn only_the_last_open_session_in_a_linked_worktree_leaves_it_behind() {
        let closing = record("a", Some("/repo-fix"), SessionState::Running);
        assert_eq!(
            abandoned_worktree(&closing, []),
            Some(PathBuf::from("/repo-fix"))
        );
        let exited = record("b", Some("/repo-fix"), SessionState::Exited);
        let elsewhere = record("c", Some("/repo-other"), SessionState::Running);
        assert_eq!(
            abandoned_worktree(&closing, [&exited, &elsewhere]),
            Some(PathBuf::from("/repo-fix"))
        );
        let open = record("d", Some("/repo-fix"), SessionState::Idle);
        assert_eq!(abandoned_worktree(&closing, [&open]), None);
    }

    #[test]
    fn the_main_checkout_and_plain_directories_are_never_offered() {
        let main = record("a", Some("/repo"), SessionState::Running);
        assert_eq!(abandoned_worktree(&main, []), None);
        let plain = record("b", None, SessionState::Running);
        assert_eq!(abandoned_worktree(&plain, []), None);
    }
}
