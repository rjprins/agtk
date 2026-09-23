use super::*;
use crate::control::{WorktreeCreateParams, WorktreeListParams, WorktreeReapParams};
use crate::worktrees::{
    DeleteBranch, ReapRequest, WorktreeInfo, WorktreeInventory, WorktreeManager, WorktreeState,
};

/// The worktree dialog: inventory, guarded reap, and creation.
#[derive(Clone)]
pub(super) struct WorktreeDialog {
    pub(super) modal: modal::Modal,
    pub(super) root: adw::EntryRow,
    pub(super) branch: adw::EntryRow,
    pub(super) base: adw::EntryRow,
    pub(super) purpose: adw::EntryRow,
    pub(super) list: gtk::ListBox,
    inventory: adw::PreferencesGroup,
    refresh: gtk::Button,
    create: adw::ButtonRow,
}

impl WorktreeDialog {
    pub(super) fn build(parent: &adw::ApplicationWindow) -> Self {
        let root = adw::EntryRow::builder()
            .title("Project Root")
            .show_apply_button(true)
            .build();
        let project = adw::PreferencesGroup::new();
        project.add(&root);

        let list = gtk::ListBox::new();
        list.add_css_class("boxed-list");
        list.set_selection_mode(gtk::SelectionMode::None);
        let inventory = adw::PreferencesGroup::builder().title("Worktrees").build();
        inventory.add(&list);

        let branch = adw::EntryRow::builder().title("Branch").build();
        branch.set_tooltip_text(Some("A concise kebab-case slug"));
        let base = adw::EntryRow::builder()
            .title("Base (default branch when empty)")
            .build();
        let purpose = adw::EntryRow::builder().title("Purpose").build();
        let create = adw::ButtonRow::builder()
            .title("Create Worktree")
            .start_icon_name("list-add-symbolic")
            .build();
        let new_group = adw::PreferencesGroup::builder()
            .title("New Worktree")
            .build();
        new_group.add(&branch);
        new_group.add(&base);
        new_group.add(&purpose);
        new_group.add(&create);

        let page = adw::PreferencesPage::new();
        page.add(&project);
        page.add(&inventory);
        page.add(&new_group);

        let refresh = gtk::Button::builder()
            .icon_name("view-refresh-symbolic")
            .tooltip_text("Scan worktrees again")
            .build();
        let modal = modal::Modal::new(parent, "Worktrees", 760, 720, &page);
        modal.header().pack_start(&refresh);
        Self {
            modal,
            root,
            branch,
            base,
            purpose,
            list,
            inventory,
            refresh,
            create,
        }
    }
}

impl Workspace {
    pub(super) fn connect_worktrees(&self) {
        let workspace = self.clone();
        self.worktrees
            .refresh
            .connect_clicked(move |_| workspace.refresh_worktree_panel());
        let workspace = self.clone();
        self.worktrees
            .root
            .connect_apply(move |_| workspace.refresh_worktree_panel());
        let workspace = self.clone();
        self.worktrees
            .create
            .connect_activated(move |_| workspace.create_worktree_from_panel());
        let workspace = self.clone();
        self.worktrees
            .modal
            .connect_show(move || workspace.prepare_worktree_panel());
    }
}

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
        if self.worktrees.root.text().trim().is_empty()
            && let Some(root) = self.preferred_project_root()
        {
            self.worktrees.root.set_text(&root);
        }
        self.refresh_worktree_panel();
    }

    pub(super) fn open_worktrees_for_project(&self, root: &str) {
        self.worktrees.root.set_text(root);
        if self.worktrees.modal.is_visible() {
            self.refresh_worktree_panel();
        } else {
            self.worktrees.modal.present();
        }
    }

    pub(super) fn refresh_worktree_panel(&self) {
        let root = self.worktrees.root.text().trim().to_owned();
        if root.is_empty() {
            self.render_worktree_message("Enter an absolute project root");
            return;
        }
        self.render_worktree_message("Scanning Git state…");
        let manager = WorktreeManager::new(self.paths.attic_dir());
        let live_paths = self.live_worktree_paths();
        self.run_io(
            move || manager.list(Path::new(&root), &live_paths),
            |workspace, result| match result {
                Ok(inventory) => workspace.render_worktree_inventory(inventory),
                Err(error) => {
                    workspace.render_worktree_message(&format!("Error: {error}"));
                    workspace.show_error(&format!("Could not list worktrees: {error}"));
                }
            },
        );
    }

    pub(super) fn create_worktree_from_panel(&self) {
        let project_root = PathBuf::from(self.worktrees.root.text().trim());
        let branch = self.worktrees.branch.text().trim().to_owned();
        let base = self.worktrees.base.text().trim().to_owned();
        let purpose = self.worktrees.purpose.text().trim().to_owned();
        let manager = WorktreeManager::new(self.paths.attic_dir());
        self.render_worktree_message("Creating worktree…");
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
                    workspace.worktrees.branch.set_text("");
                    workspace.worktrees.purpose.set_text("");
                    workspace.show_error(&format!("Created worktree {}", created.path.display()));
                    workspace.refresh_worktree_panel();
                }
                Err(error) => {
                    workspace.render_worktree_message(&format!("Error: {error}"));
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
                        .launch
                        .project
                        .set_text(created.repo_root.to_string_lossy().as_ref());
                    workspace
                        .launch
                        .worktree
                        .set_text(created.path.to_string_lossy().as_ref());
                    workspace
                        .launch
                        .cwd
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

    pub(super) fn live_worktree_paths(&self) -> Vec<PathBuf> {
        self.sessions
            .borrow()
            .values()
            .filter(|session| session.record.state != SessionState::Exited)
            .filter_map(|session| session.record.worktree_path.clone())
            .collect()
    }

    fn render_worktree_message(&self, text: &str) {
        self.clear_worktree_list();
        let row = adw::ActionRow::builder().title(text).build();
        row.add_css_class("dim-label");
        self.worktrees.list.append(&row);
    }

    fn clear_worktree_list(&self) {
        while let Some(child) = self.worktrees.list.first_child() {
            self.worktrees.list.remove(&child);
        }
    }

    fn render_worktree_inventory(&self, inventory: WorktreeInventory) {
        self.clear_worktree_list();
        self.worktrees.inventory.set_description(Some(&format!(
            "{} on {}, default branch {}",
            inventory.worktrees.len(),
            project_label(&inventory.repo_root),
            inventory.default_branch
        )));
        for worktree in inventory.worktrees {
            self.worktrees
                .list
                .append(&self.worktree_inventory_row(&inventory.repo_root, worktree));
        }
    }

    fn worktree_inventory_row(
        &self,
        project_root: &Path,
        worktree: WorktreeInfo,
    ) -> adw::ActionRow {
        let branch = worktree.branch.as_deref().unwrap_or("(detached)");
        let row = adw::ActionRow::builder()
            .title(branch)
            .subtitle(format!(
                "{} · {}",
                worktree.path.display(),
                worktree.evidence
            ))
            .subtitle_lines(1)
            .build();
        row.set_tooltip_text(Some(&format!(
            "{}\n{}",
            worktree.path.display(),
            worktree.evidence
        )));
        let (state_text, state_class) = state_presentation(worktree.state);
        let state = gtk::Label::new(Some(state_text));
        state.add_css_class("caption-heading");
        state.add_css_class(state_class);
        state.set_valign(gtk::Align::Center);
        state.set_width_chars(8);
        state.set_xalign(0.0);
        row.add_prefix(&state);
        if worktree.dirty || worktree.ignored_only {
            let changes = gtk::Label::new(Some(if worktree.dirty {
                "uncommitted changes"
            } else {
                "ignored files only"
            }));
            changes.add_css_class("caption");
            changes.add_css_class(if worktree.dirty {
                "warning"
            } else {
                "dim-label"
            });
            changes.set_valign(gtk::Align::Center);
            row.add_suffix(&changes);
        }
        let launch = gtk::Button::builder()
            .icon_name("list-add-symbolic")
            .valign(gtk::Align::Center)
            .tooltip_text("Launch a session in this worktree")
            .build();
        launch.add_css_class("flat");
        let launch_workspace = self.clone();
        let launch_project = project_root.to_owned();
        let launch_path = worktree.path.clone();
        launch.connect_clicked(move |_| {
            launch_workspace.open_launch_for_worktree(&launch_project, &launch_path)
        });
        row.add_suffix(&launch);
        if worktree.reap_class.is_some() && !worktree.is_primary && worktree.live_session_count == 0
        {
            let reap = gtk::Button::builder()
                .icon_name("user-trash-symbolic")
                .valign(gtk::Align::Center)
                .tooltip_text("Remove safely: click once to arm, again to confirm")
                .build();
            reap.add_css_class("flat");
            reap.add_css_class("destructive-action");
            let armed = Rc::new(Cell::new(false));
            let reap_workspace = self.clone();
            let reap_worktree = worktree.clone();
            reap.connect_clicked(move |button| {
                if armed.replace(true) {
                    button.set_sensitive(false);
                    reap_workspace.reap_worktree_from_panel(reap_worktree.clone());
                } else {
                    button.remove_css_class("flat");
                    button.set_label("Remove");
                }
            });
            row.add_suffix(&reap);
        }
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
        self.render_worktree_message("Removing with preview guards…");
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
                    workspace.render_worktree_message(&format!("Aborted: {reason}"));
                    workspace.show_error(reason);
                }
                Err(error) => {
                    workspace.render_worktree_message(&format!("Refused: {error}"));
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

fn project_label(root: &Path) -> String {
    root.file_name()
        .map(|name| name.to_string_lossy().into_owned())
        .unwrap_or_else(|| root.display().to_string())
}

/// A short state label and the libadwaita status class that colors it.
const fn state_presentation(state: WorktreeState) -> (&'static str, &'static str) {
    match state {
        WorktreeState::Active => ("active", "accent"),
        WorktreeState::Open => ("open", "accent"),
        WorktreeState::Merged => ("merged", "success"),
        WorktreeState::LocalOnly => ("local only", "dim-label"),
        WorktreeState::Review => ("review", "warning"),
        WorktreeState::Stale => ("stale", "warning"),
        WorktreeState::Ephemeral => ("ephemeral", "dim-label"),
        WorktreeState::Unknown => ("unknown", "dim-label"),
    }
}
