use super::*;
use crate::session_names::{follows_worktree_name, next_worktree_session_name};
use crate::worktrees::{PorcelainWorktree, WorktreeManager};

impl Workspace {
    /// Re-associates a session with a worktree. The process keeps its own
    /// directory; only what agtk derives from the worktree changes.
    pub(super) fn set_session_worktree(
        &self,
        id: &str,
        path: PathBuf,
        pending: Option<PendingRequest>,
    ) {
        if !self.sessions.borrow().contains_key(id) {
            self.report_failure(
                pending,
                ErrorCode::SessionNotFound,
                "No session exists with that ID",
                id.to_owned(),
            );
            return;
        }
        let known_roots = self
            .project_summaries()
            .into_iter()
            .map(|project| PathBuf::from(project.root))
            .collect::<Vec<_>>();
        let manager = WorktreeManager::new(self.paths.attic_dir());
        let id = id.to_owned();
        self.run_slow(
            move || {
                if !path.is_dir() {
                    return Err(format!("{} is not a directory", path.display()).into());
                }
                let location = manager.resolve_session_location(&path, &known_roots);
                let Some(worktree) = location.worktree_path else {
                    return Err(format!("{} is not inside a Git worktree", path.display()).into());
                };
                Ok((worktree, location.project_root))
            },
            move |workspace, result| match result {
                Ok((worktree, project_root)) => {
                    workspace.save_session_worktree(&id, worktree, project_root, pending)
                }
                Err(error) => workspace.report_failure(
                    pending,
                    ErrorCode::OperationRefused,
                    "Could not change worktree",
                    error.to_string(),
                ),
            },
        );
    }

    fn save_session_worktree(
        &self,
        id: &str,
        worktree: PathBuf,
        project_root: PathBuf,
        pending: Option<PendingRequest>,
    ) {
        // A rename or state hook may have arrived while git was resolving the path.
        let record = self.sessions.borrow().get(id).map(|s| s.record.clone());
        let Some(mut record) = record else {
            self.report_failure(
                pending,
                ErrorCode::SessionNotFound,
                "Session closed before its worktree could be changed",
                id.to_owned(),
            );
            return;
        };
        let Some(store) = self.store.borrow().clone() else {
            self.report_launch_failure(
                pending,
                "Could not change worktree",
                "workspace is loading".to_owned(),
            );
            return;
        };
        self.run_io(
            move || {
                // A name taken from the old worktree follows the session to the new one.
                let old_worktree = record.worktree_path.as_deref().or(record.cwd.as_deref());
                if !record.renamed && follows_worktree_name(&record.name, old_worktree) {
                    record.name = next_worktree_session_name(
                        &record.name,
                        Some(&worktree),
                        &store.sessions()?,
                    );
                }
                record.worktree_path = Some(worktree);
                record.project_root = Some(project_root);
                store.save_session(&record)?;
                Ok(record)
            },
            move |workspace, result| match result {
                Ok(record) => {
                    let id = record.id.clone();
                    workspace.apply_session_worktree(record);
                    if let Some(pending) = pending {
                        match workspace.session_summary(&id).and_then(|summary| {
                            serde_json::to_value(summary).map_err(|error| error.to_string())
                        }) {
                            Ok(summary) => {
                                let id = pending.request.id.clone();
                                let _ = pending.respond(ControlResponse::success(id, summary));
                            }
                            Err(error) => workspace.report_launch_failure(
                                Some(pending),
                                "Could not describe session",
                                error,
                            ),
                        }
                    }
                }
                Err(error) => workspace.report_failure(
                    pending,
                    ErrorCode::OperationRefused,
                    "Could not change worktree",
                    error.to_string(),
                ),
            },
        );
    }

    fn apply_session_worktree(&self, record: SessionRecord) {
        let id = record.id.clone();
        let project_root = record.project_root.clone();
        let name_changed = {
            let mut sessions = self.sessions.borrow_mut();
            let Some(session) = sessions.get_mut(&id) else {
                return;
            };
            let name_changed = session.record.name != record.name;
            session.record.worktree_path = record.worktree_path;
            session.record.project_root = record.project_root;
            session.record.name = record.name.clone();
            sessions::show_worktree_caption(&session.worktree_label, &session.record);
            if name_changed {
                session.label.set_text(&record.name);
                if matches!(
                    session.record.kind,
                    SessionKind::Claude | SessionKind::Codex
                ) {
                    session.pending_agent_name = Some(record.name);
                }
            }
            name_changed
        };
        if name_changed {
            self.send_pending_agent_name(&id);
        }
        // Tabs follow the worktree, so the session leaves its old context.
        self.attach_workspace_session(&id);
        self.remember_project(project_root.as_deref());
        self.rebuild_sidebar();
        if self.selected_session_id().as_deref() == Some(id.as_str()) {
            self.workspace_tabs.borrow_mut().select_session(&id);
            self.refresh_content_title();
            self.refresh_selected_pr_context(&id);
            self.render_workspace_tabs();
            self.refresh_changes();
            self.update_emacs_actions();
        }
    }

    /// Lists the checkouts of the session's repository, then asks which one to use.
    pub(super) fn start_session_worktree_change(&self, id: &str) {
        let record = self.sessions.borrow().get(id).map(|s| s.record.clone());
        let Some(record) = record else {
            return;
        };
        let current = record.worktree_path.clone().or(record.cwd.clone());
        let Some(current) = current else {
            self.show_error("This session has no working directory");
            return;
        };
        let manager = WorktreeManager::new(self.paths.attic_dir());
        let repo = record.project_root.clone();
        let id = id.to_owned();
        self.run_slow(
            move || {
                let repo = match repo {
                    Some(repo) => repo,
                    None => manager.repository_root(&current)?,
                };
                Ok((manager.linked_worktrees(&repo)?, current))
            },
            move |workspace, result| match result {
                Ok((worktrees, current)) => {
                    workspace.present_worktree_choice(&id, worktrees, &current)
                }
                Err(error) => workspace.show_error(&format!("Could not list worktrees: {error}")),
            },
        );
    }

    fn present_worktree_choice(&self, id: &str, worktrees: Vec<PorcelainWorktree>, current: &Path) {
        if worktrees.is_empty() {
            self.show_error("This repository has no worktrees");
            return;
        }
        let current = current
            .canonicalize()
            .unwrap_or_else(|_| current.to_path_buf());
        let selected = worktrees
            .iter()
            .position(|worktree| worktree.path == current)
            .unwrap_or(0);
        let labels = worktrees
            .iter()
            .map(worktree_choice_label)
            .collect::<Vec<_>>();
        let labels = labels.iter().map(String::as_str).collect::<Vec<_>>();
        let dropdown = gtk::DropDown::from_strings(&labels);
        dropdown.set_selected(selected as u32);
        let dialog = adw::AlertDialog::new(
            Some("Change Worktree"),
            Some(
                "The sidebar, changes and Git actions follow the chosen worktree. The running program keeps its own directory.",
            ),
        );
        dialog.set_extra_child(Some(&dropdown));
        dialog.add_responses(&[("cancel", "_Cancel"), ("change", "_Change")]);
        dialog.set_response_appearance("change", adw::ResponseAppearance::Suggested);
        dialog.set_default_response(Some("change"));
        dialog.set_close_response("cancel");
        let workspace = self.clone();
        let id = id.to_owned();
        let paths = worktrees
            .into_iter()
            .map(|worktree| worktree.path)
            .collect::<Vec<_>>();
        dialog.connect_response(Some("change"), move |_, _| {
            if let Some(path) = paths.get(dropdown.selected() as usize) {
                workspace.set_session_worktree(&id, path.clone(), None);
            }
        });
        dialog.present(Some(&self.window));
    }
}

/// The directory name, with the branch when it says something more.
fn worktree_choice_label(worktree: &PorcelainWorktree) -> String {
    let name = worktree
        .path
        .file_name()
        .and_then(|name| name.to_str())
        .filter(|name| !name.is_empty())
        .map(str::to_owned)
        .unwrap_or_else(|| worktree.path.to_string_lossy().into_owned());
    match worktree.branch.as_deref() {
        Some(branch) if branch != name => format!("{name} ({branch})"),
        Some(_) => name,
        None => format!("{name} (detached)"),
    }
}

#[cfg(test)]
mod tests {
    use super::worktree_choice_label;
    use crate::worktrees::PorcelainWorktree;
    use std::path::PathBuf;

    fn worktree(path: &str, branch: Option<&str>) -> PorcelainWorktree {
        PorcelainWorktree {
            path: PathBuf::from(path),
            head: None,
            branch: branch.map(str::to_owned),
            detached: branch.is_none(),
            locked: false,
            prunable: false,
        }
    }

    #[test]
    fn labels_mention_the_branch_only_when_it_differs_from_the_directory() {
        assert_eq!(
            worktree_choice_label(&worktree("/work/agtk", Some("main"))),
            "agtk (main)"
        );
        assert_eq!(
            worktree_choice_label(&worktree("/work/agtk-fix", Some("fix"))),
            "agtk-fix (fix)"
        );
        assert_eq!(
            worktree_choice_label(&worktree("/work/fix", Some("fix"))),
            "fix"
        );
        assert_eq!(
            worktree_choice_label(&worktree("/work/fix", None)),
            "fix (detached)"
        );
    }
}
