use super::*;
use crate::control::{WorktreeCreateParams, WorktreeListParams, WorktreeReapParams};
use crate::worktrees::{
    DeleteBranch, ReapRequest, WorktreeInfo, WorktreeInventory, WorktreeManager,
};

impl Workspace {
    pub(super) fn preferred_project_root(&self) -> Option<String> {
        if let Some(selected) = self.selected_session_id()
            && let Some(root) = self
                .sessions
                .borrow()
                .get(&selected)
                .and_then(|session| session.record.project_root.as_ref())
        {
            return Some(root.to_string_lossy().to_string());
        }
        self.project_summaries()
            .into_iter()
            .map(|project| project.root)
            .next()
    }

    pub(super) fn prepare_worktree_panel(&self) {
        if self.worktree_root.text().trim().is_empty()
            && let Some(root) = self.preferred_project_root()
        {
            self.worktree_root.set_text(&root);
        }
        self.refresh_worktree_panel();
    }

    pub(super) fn open_worktrees_for_project(&self, root: &str) {
        self.worktree_root.set_text(root);
        if self.worktree_popover.is_mapped() {
            self.refresh_worktree_panel();
        } else {
            self.worktree_popover.popup();
        }
    }

    pub(super) fn refresh_worktree_panel(&self) {
        let root = self.worktree_root.text().trim().to_owned();
        if root.is_empty() {
            self.render_worktree_message("Enter an absolute project root");
            return;
        }
        self.render_worktree_message("SCANNING GIT STATE...");
        let manager = WorktreeManager::new(self.paths.attic_dir());
        let live_paths = self.live_worktree_paths();
        self.run_io(
            move || manager.list(Path::new(&root), &live_paths),
            |workspace, result| match result {
                Ok(inventory) => workspace.render_worktree_inventory(inventory),
                Err(error) => {
                    workspace.render_worktree_message(&format!("ERROR  {error}"));
                    workspace.show_error(&format!("Could not list worktrees: {error}"));
                }
            },
        );
    }

    pub(super) fn create_worktree_from_panel(&self) {
        let project_root = PathBuf::from(self.worktree_root.text().trim());
        let branch = self.worktree_branch.text().trim().to_owned();
        let base = self.worktree_base.text().trim().to_owned();
        let purpose = self.worktree_purpose.text().trim().to_owned();
        let manager = WorktreeManager::new(self.paths.attic_dir());
        self.render_worktree_message("CREATING WORKTREE...");
        self.run_io(
            move || {
                manager.create(
                    &project_root,
                    &branch,
                    (!base.is_empty()).then_some(base.as_str()),
                    &purpose,
                )
            },
            |workspace, result| match result {
                Ok(created) => {
                    workspace.worktree_branch.set_text("");
                    workspace.worktree_purpose.set_text("");
                    workspace.show_error(&format!("Created worktree {}", created.path.display()));
                    workspace.refresh_worktree_panel();
                }
                Err(error) => {
                    workspace.render_worktree_message(&format!("ERROR  {error}"));
                    workspace.show_error(&format!("Could not create worktree: {error}"));
                }
            },
        );
    }

    pub(super) fn handle_worktree_control(&self, pending: PendingRequest) {
        match pending.request.command.clone() {
            ControlCommand::WorktreeList(params) => self.list_worktrees(params, pending),
            ControlCommand::WorktreeCreate(params) => self.create_worktree(params, pending),
            ControlCommand::WorktreeReap(params) => self.reap_worktree(params, pending),
            _ => unreachable!("caller filters worktree commands"),
        }
    }

    fn list_worktrees(&self, params: WorktreeListParams, pending: PendingRequest) {
        let manager = WorktreeManager::new(self.paths.attic_dir());
        let live_paths = self.live_worktree_paths();
        self.run_io(
            move || manager.list(&params.project_root, &live_paths),
            move |workspace, result| {
                workspace.respond_worktree_result(pending, "Could not list worktrees", result)
            },
        );
    }

    fn create_worktree(&self, params: WorktreeCreateParams, pending: PendingRequest) {
        let manager = WorktreeManager::new(self.paths.attic_dir());
        self.run_io(
            move || {
                manager.create(
                    &params.project_root,
                    &params.branch,
                    params.base_branch.as_deref(),
                    &params.purpose,
                )
            },
            move |workspace, result| {
                if let Ok(created) = &result {
                    workspace
                        .launch_project
                        .set_text(created.repo_root.to_string_lossy().as_ref());
                    workspace
                        .launch_worktree
                        .set_text(created.path.to_string_lossy().as_ref());
                    workspace
                        .launch_cwd
                        .set_text(created.path.to_string_lossy().as_ref());
                }
                workspace.respond_worktree_result(pending, "Could not create worktree", result);
            },
        );
    }

    fn reap_worktree(&self, params: WorktreeReapParams, pending: PendingRequest) {
        let manager = WorktreeManager::new(self.paths.attic_dir());
        let live_paths = self.live_worktree_paths();
        let request = ReapRequest {
            path: params.path,
            expected_head: params.expected_head,
            expected_status_hash: params.expected_status_hash,
            delete_branch: params.delete_branch,
        };
        self.run_io(
            move || manager.reap(request, &live_paths),
            move |workspace, result| {
                workspace.respond_worktree_result(pending, "Worktree operation was refused", result)
            },
        );
    }

    fn live_worktree_paths(&self) -> Vec<PathBuf> {
        self.sessions
            .borrow()
            .values()
            .filter(|session| session.record.state != SessionState::Exited)
            .filter_map(|session| session.record.worktree_path.clone())
            .collect()
    }

    fn render_worktree_message(&self, text: &str) {
        while let Some(child) = self.worktree_list.first_child() {
            self.worktree_list.remove(&child);
        }
        let label = gtk::Label::new(Some(text));
        label.set_xalign(0.0);
        label.add_css_class("tui-muted");
        self.worktree_list.append(&label);
    }

    fn render_worktree_inventory(&self, inventory: WorktreeInventory) {
        while let Some(child) = self.worktree_list.first_child() {
            self.worktree_list.remove(&child);
        }
        let summary = gtk::Label::new(Some(&format!(
            "DEFAULT  {}    {} WORKTREES",
            inventory.default_branch,
            inventory.worktrees.len()
        )));
        summary.set_xalign(0.0);
        summary.add_css_class("tui-muted");
        self.worktree_list.append(&summary);
        for worktree in inventory.worktrees {
            self.worktree_list
                .append(&self.worktree_inventory_row(&inventory.repo_root, worktree));
        }
    }

    fn worktree_inventory_row(&self, project_root: &Path, worktree: WorktreeInfo) -> gtk::Box {
        let row = gtk::Box::new(gtk::Orientation::Vertical, 1);
        row.add_css_class("tui-worktree-row");
        let headline = gtk::Box::new(gtk::Orientation::Horizontal, 5);
        let branch = worktree.branch.as_deref().unwrap_or("(detached)");
        let dirty = if worktree.dirty {
            " DIRTY"
        } else if worktree.ignored_only {
            " IGNORED"
        } else {
            ""
        };
        let name = gtk::Label::new(Some(&format!(
            "[{}] {}{}",
            format!("{:?}", worktree.state).to_lowercase(),
            branch,
            dirty
        )));
        name.set_xalign(0.0);
        name.set_hexpand(true);
        headline.append(&name);
        let launch = gtk::Button::with_label("[launch]");
        launch.add_css_class("tui-button");
        launch.set_tooltip_text(Some("Open quick launch in this worktree"));
        let launch_workspace = self.clone();
        let launch_project = project_root.to_owned();
        let launch_path = worktree.path.clone();
        launch.connect_clicked(move |_| {
            launch_workspace.open_launch_for_worktree(&launch_project, &launch_path)
        });
        headline.append(&launch);
        if worktree.reap_class.is_some() && !worktree.is_primary && worktree.live_session_count == 0
        {
            let reap = gtk::Button::with_label("[reap?]");
            reap.add_css_class("tui-danger-button");
            reap.set_tooltip_text(Some("Arm, then confirm guarded reap using this preview"));
            let armed = Rc::new(Cell::new(false));
            let reap_workspace = self.clone();
            let reap_worktree = worktree.clone();
            reap.connect_clicked(move |button| {
                if armed.replace(true) {
                    button.set_sensitive(false);
                    reap_workspace.reap_worktree_from_panel(reap_worktree.clone());
                } else {
                    button.set_label("[confirm]");
                }
            });
            headline.append(&reap);
        }
        row.append(&headline);
        let details = gtk::Label::new(Some(&format!(
            "{}    {}",
            worktree.path.display(),
            worktree.evidence
        )));
        details.set_xalign(0.0);
        details.set_ellipsize(gtk::pango::EllipsizeMode::Middle);
        details.add_css_class("tui-muted");
        details.set_tooltip_text(Some(&format!(
            "{}\n{}",
            worktree.path.display(),
            worktree.evidence
        )));
        row.append(&details);
        row
    }

    fn reap_worktree_from_panel(&self, worktree: WorktreeInfo) {
        let manager = WorktreeManager::new(self.paths.attic_dir());
        let live_paths = self.live_worktree_paths();
        let request = ReapRequest {
            path: worktree.path,
            expected_head: worktree.head.unwrap_or_default(),
            expected_status_hash: worktree.status_hash,
            delete_branch: DeleteBranch::Auto,
        };
        self.render_worktree_message("REAPING WITH PREVIEW GUARDS...");
        self.run_io(
            move || manager.reap(request, &live_paths),
            |workspace, result| match result {
                Ok(result) if result.ok => {
                    workspace
                        .show_error(result.reason.as_deref().unwrap_or("Worktree reaped safely"));
                    workspace.refresh_worktree_panel();
                }
                Ok(result) => {
                    let reason = result.reason.as_deref().unwrap_or("reap aborted");
                    workspace.render_worktree_message(&format!("ABORTED  {reason}"));
                    workspace.show_error(reason);
                }
                Err(error) => {
                    workspace.render_worktree_message(&format!("REFUSED  {error}"));
                    workspace.show_error(&format!("Worktree reap refused: {error}"));
                }
            },
        );
    }

    fn respond_worktree_result<T: serde::Serialize>(
        &self,
        pending: PendingRequest,
        message: &'static str,
        result: PersistResult<T>,
    ) {
        match result.and_then(|value| serde_json::to_value(value).map_err(Into::into)) {
            Ok(value) => {
                let id = pending.request.id.clone();
                let _ = pending.respond(ControlResponse::success(id, value));
            }
            Err(error) => self.report_failure(
                Some(pending),
                ErrorCode::OperationRefused,
                message,
                error.to_string(),
            ),
        }
    }
}
