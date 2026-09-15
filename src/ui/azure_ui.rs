use super::*;
use crate::azure::{
    AzureClient, PrAttention, PrContext, PrItem, PrPreferences, acknowledge_attention,
    reconcile_attention,
};
use crate::control::{
    PrAcknowledgeParams, PrLaunchReviewParams, PrListParams, PrSetAutoReviewParams,
};
use crate::worktrees::WorktreeManager;

#[derive(Debug)]
struct LoadedPrContext {
    preferences: PrPreferences,
    context: PrContext,
    changed: Vec<u64>,
}

impl Workspace {
    pub(super) fn open_pr_for_project(&self, root: &str) {
        self.pr_root.set_text(root);
        if self.pr_popover.is_mapped() {
            self.refresh_pr_panel(None);
        } else {
            self.pr_popover.popup();
        }
    }

    pub(super) fn prepare_pr_panel(&self) {
        if self.pr_root.text().trim().is_empty()
            && let Some(root) = self.preferred_project_root()
        {
            self.pr_root.set_text(&root);
        }
        self.refresh_pr_panel(None);
    }

    pub(super) fn handle_pr_control(&self, pending: PendingRequest) {
        match pending.request.command.clone() {
            ControlCommand::PrList(params) => self.list_prs(params, Some(pending)),
            ControlCommand::PrAcknowledge(params) => self.acknowledge_pr(params, Some(pending)),
            ControlCommand::PrSetAutoReview(params) => {
                self.set_auto_review(params, Some(pending));
            }
            ControlCommand::PrLaunchReview(params) => self.launch_review_control(params, pending),
            _ => unreachable!("caller filters PR commands"),
        }
    }

    pub(super) fn refresh_pr_panel(&self, pending: Option<PendingRequest>) {
        let root = self.pr_root.text().trim().to_owned();
        if root.is_empty() {
            self.render_pr_message("SELECT A PROJECT WITH AN AZURE DEVOPS ORIGIN");
            if let Some(pending) = pending {
                respond_failure(
                    pending,
                    ErrorCode::InvalidParams,
                    "Project root is required",
                    None,
                );
            }
            return;
        }
        self.list_prs(
            PrListParams {
                project_root: PathBuf::from(root),
            },
            pending,
        );
    }

    fn list_prs(&self, params: PrListParams, pending: Option<PendingRequest>) {
        let Some(store) = self.store.borrow().clone() else {
            self.report_failure(
                pending,
                ErrorCode::InternalError,
                "Workspace is loading",
                "PR state is not available yet".to_owned(),
            );
            return;
        };
        if !self.pr_popover.is_mapped() {
            self.render_pr_message("LOADING ACTIVE PULL REQUESTS...");
        }
        let preferences = self.pr_preferences.borrow().clone();
        let attic = self.paths.attic_dir();
        let live_paths = self.live_worktree_paths();
        self.run_io(
            move || {
                let project_root = params.project_root.canonicalize()?;
                let root_key = project_root.to_string_lossy().to_string();
                let Some(list) = AzureClient::from_environment().list_active(&project_root)? else {
                    return Err("project origin is not an Azure DevOps repository".into());
                };
                let previous = preferences.projects.get(&root_key);
                let reconciliation = reconcile_attention(previous, &list.pull_requests);
                let changed = reconciliation.changed.iter().copied().collect::<Vec<_>>();
                let mut preferences = preferences;
                preferences.set_project(root_key, reconciliation.state.clone());
                let value = serde_json::to_value(&preferences)?;
                store.set_preference("pullRequests", &value)?;

                let worktrees = WorktreeManager::new(attic)
                    .list(&project_root, &live_paths)
                    .ok()
                    .map(|inventory| inventory.worktrees)
                    .unwrap_or_default();
                let pull_requests = list
                    .pull_requests
                    .into_iter()
                    .map(|pull_request| {
                        let worktree_path = worktrees
                            .iter()
                            .find(|worktree| {
                                worktree.branch.as_deref()
                                    == Some(pull_request.source_branch.as_str())
                            })
                            .map(|worktree| worktree.path.clone());
                        let attention = reconciliation
                            .state
                            .attention
                            .get(&pull_request.id)
                            .copied();
                        PrItem {
                            pull_request,
                            attention,
                            worktree_path,
                        }
                    })
                    .collect();
                Ok(LoadedPrContext {
                    preferences,
                    context: PrContext {
                        project_root,
                        repository: list.repository,
                        current_user: list.current_user,
                        auto_review: reconciliation.state.auto_review,
                        pull_requests,
                    },
                    changed,
                })
            },
            move |workspace, result| match result {
                Ok(loaded) => {
                    *workspace.pr_preferences.borrow_mut() = loaded.preferences;
                    workspace.update_pr_indicator();
                    workspace.updating_pr_toggle.set(true);
                    workspace
                        .pr_auto_toggle
                        .set_active(loaded.context.auto_review);
                    workspace.updating_pr_toggle.set(false);
                    workspace.render_pr_context(&loaded.context);

                    if let Some(pending) = pending {
                        let id = pending.request.id.clone();
                        match serde_json::to_value(&loaded.context) {
                            Ok(value) => {
                                let _ = pending.respond(ControlResponse::success(id, value));
                            }
                            Err(error) => workspace.report_launch_failure(
                                Some(pending),
                                "Could not describe pull requests",
                                error.to_string(),
                            ),
                        }
                    }

                    if loaded.context.auto_review {
                        for item in loaded.context.pull_requests.iter().filter(|item| {
                            loaded.changed.contains(&item.pull_request.id)
                                && !item.pull_request.is_draft
                                && !item.pull_request.is_own_author
                        }) {
                            workspace.launch_pr_review(&loaded.context.project_root, item, None);
                        }
                    }
                }
                Err(error) => {
                    workspace.render_pr_message(&format!("ERROR  {error}"));
                    workspace.report_failure(
                        pending,
                        ErrorCode::OperationRefused,
                        "Could not list pull requests",
                        error.to_string(),
                    );
                }
            },
        );
    }

    fn render_pr_message(&self, text: &str) {
        while let Some(child) = self.pr_list.first_child() {
            self.pr_list.remove(&child);
        }
        let label = gtk::Label::new(Some(text));
        label.set_xalign(0.0);
        label.add_css_class("tui-muted");
        self.pr_list.append(&label);
    }

    fn render_pr_context(&self, context: &PrContext) {
        while let Some(child) = self.pr_list.first_child() {
            self.pr_list.remove(&child);
        }
        if context.pull_requests.is_empty() {
            self.render_pr_message("NO ACTIVE PULL REQUESTS");
            return;
        }
        for item in &context.pull_requests {
            self.pr_list.append(&self.pr_row(context, item));
        }
    }

    fn pr_row(&self, context: &PrContext, item: &PrItem) -> gtk::Box {
        let row = gtk::Box::new(gtk::Orientation::Vertical, 1);
        row.add_css_class("tui-worktree-row");
        let headline = gtk::Box::new(gtk::Orientation::Horizontal, 4);
        let marker = item
            .attention
            .map(|attention| format!("[{}] ", attention_name(attention)))
            .unwrap_or_default();
        let draft = if item.pull_request.is_draft {
            " [draft]"
        } else {
            ""
        };
        let title = gtk::Label::new(Some(&format!(
            "{marker}#{} {}{draft}",
            item.pull_request.id, item.pull_request.title
        )));
        title.set_xalign(0.0);
        title.set_hexpand(true);
        title.set_ellipsize(gtk::pango::EllipsizeMode::End);
        title.set_tooltip_text(Some(&item.pull_request.title));
        headline.append(&title);

        let view = gtk::Button::builder()
            .icon_name("web-browser-symbolic")
            .build();
        view.add_css_class("tui-button");
        view.set_tooltip_text(Some(
            "Open in the browser and mark current attention viewed",
        ));
        let view_workspace = self.clone();
        let url = item.pull_request.url.clone();
        let project_root = context.project_root.clone();
        let pull_request_id = item.pull_request.id;
        let attention = item.attention;
        view.connect_clicked(move |_| {
            let launcher = gtk::UriLauncher::new(&url);
            let launch_workspace = view_workspace.clone();
            launcher.launch(
                Some(&view_workspace.window),
                None::<&gio::Cancellable>,
                move |result| {
                    if let Err(error) = result {
                        launch_workspace.show_error(&format!("Could not open PR: {error}"));
                    }
                },
            );
            if let Some(marker) = attention {
                view_workspace.acknowledge_pr(
                    PrAcknowledgeParams {
                        project_root: project_root.clone(),
                        pull_request_id,
                        marker,
                    },
                    None,
                );
            }
        });
        headline.append(&view);

        let review = gtk::Button::builder()
            .icon_name("document-edit-symbolic")
            .build();
        review.add_css_class("tui-button");
        review.set_tooltip_text(Some("Launch Codex with the review-pr workflow"));
        let review_workspace = self.clone();
        let review_root = context.project_root.clone();
        let review_item = item.clone();
        review.connect_clicked(move |_| {
            review_workspace.launch_pr_review(&review_root, &review_item, None)
        });
        headline.append(&review);
        row.append(&headline);

        let location = item
            .worktree_path
            .as_ref()
            .map_or("project root".to_owned(), |path| path.display().to_string());
        let details = gtk::Label::new(Some(&format!(
            "{}    comments:{}    {} -> {}",
            item.pull_request.author,
            item.pull_request.unresolved_threads,
            item.pull_request.source_branch,
            item.pull_request.target_branch,
        )));
        details.set_xalign(0.0);
        details.set_ellipsize(gtk::pango::EllipsizeMode::End);
        details.add_css_class("tui-muted");
        details.set_tooltip_text(Some(&format!(
            "{} -> {} by {}, {} unresolved thread(s)",
            item.pull_request.source_branch,
            item.pull_request.target_branch,
            item.pull_request.author,
            item.pull_request.unresolved_threads
        )));
        row.append(&details);
        let location = gtk::Label::new(Some(&format!("cwd  {location}")));
        location.set_xalign(0.0);
        location.set_ellipsize(gtk::pango::EllipsizeMode::Middle);
        location.add_css_class("tui-muted");
        row.append(&location);
        row
    }

    fn acknowledge_pr(&self, params: PrAcknowledgeParams, pending: Option<PendingRequest>) {
        let Some(store) = self.store.borrow().clone() else {
            self.report_failure(
                pending,
                ErrorCode::InternalError,
                "Workspace is loading",
                "PR state is not available yet".to_owned(),
            );
            return;
        };
        let root = params.project_root.to_string_lossy().to_string();
        let mut preferences = self.pr_preferences.borrow().clone();
        let current = preferences.project(&root);
        let acknowledged = current.attention.get(&params.pull_request_id) == Some(&params.marker);
        let updated = acknowledge_attention(&current, &[(params.pull_request_id, params.marker)]);
        preferences.set_project(root, updated);
        let value = match serde_json::to_value(&preferences) {
            Ok(value) => value,
            Err(error) => {
                self.report_launch_failure(pending, "Could not encode PR state", error.to_string());
                return;
            }
        };
        self.run_io(
            move || store.set_preference("pullRequests", &value),
            move |workspace, result| match result {
                Ok(()) => {
                    *workspace.pr_preferences.borrow_mut() = preferences;
                    workspace.update_pr_indicator();
                    if let Some(pending) = pending {
                        let id = pending.request.id.clone();
                        let _ = pending.respond(ControlResponse::success(
                            id,
                            serde_json::json!({ "acknowledged": acknowledged }),
                        ));
                    } else if workspace.pr_popover.is_mapped() {
                        workspace.refresh_pr_panel(None);
                    }
                }
                Err(error) => workspace.report_launch_failure(
                    pending,
                    "Could not save PR state",
                    error.to_string(),
                ),
            },
        );
    }

    pub(super) fn set_auto_review(
        &self,
        params: PrSetAutoReviewParams,
        pending: Option<PendingRequest>,
    ) {
        let Some(store) = self.store.borrow().clone() else {
            self.report_failure(
                pending,
                ErrorCode::InternalError,
                "Workspace is loading",
                "PR state is not available yet".to_owned(),
            );
            return;
        };
        let root = params.project_root.to_string_lossy().to_string();
        let mut preferences = self.pr_preferences.borrow().clone();
        let mut state = preferences.project(&root);
        state.auto_review = params.enabled;
        preferences.set_project(root, state);
        let value = match serde_json::to_value(&preferences) {
            Ok(value) => value,
            Err(error) => {
                self.report_launch_failure(
                    pending,
                    "Could not encode PR settings",
                    error.to_string(),
                );
                return;
            }
        };
        self.run_io(
            move || store.set_preference("pullRequests", &value),
            move |workspace, result| match result {
                Ok(()) => {
                    *workspace.pr_preferences.borrow_mut() = preferences;
                    workspace.update_pr_indicator();
                    if let Some(pending) = pending {
                        let id = pending.request.id.clone();
                        let _ = pending.respond(ControlResponse::success(
                            id,
                            serde_json::json!({ "enabled": params.enabled }),
                        ));
                    }
                }
                Err(error) => workspace.report_launch_failure(
                    pending,
                    "Could not save PR settings",
                    error.to_string(),
                ),
            },
        );
    }

    fn launch_review_control(&self, params: PrLaunchReviewParams, pending: PendingRequest) {
        let project_root = params.project_root;
        let load_root = project_root.clone();
        let pull_request_id = params.pull_request_id;
        let attic = self.paths.attic_dir();
        let live_paths = self.live_worktree_paths();
        self.run_io(
            move || {
                let Some(list) = AzureClient::from_environment().list_active(&load_root)? else {
                    return Err("project origin is not an Azure DevOps repository".into());
                };
                let pull_request = list
                    .pull_requests
                    .into_iter()
                    .find(|pull_request| pull_request.id == pull_request_id)
                    .ok_or("active pull request was not found")?;
                let worktree_path = WorktreeManager::new(attic)
                    .list(&load_root, &live_paths)
                    .ok()
                    .and_then(|inventory| {
                        inventory
                            .worktrees
                            .into_iter()
                            .find(|worktree| {
                                worktree.branch.as_deref()
                                    == Some(pull_request.source_branch.as_str())
                            })
                            .map(|worktree| worktree.path)
                    });
                Ok(PrItem {
                    pull_request,
                    attention: None,
                    worktree_path,
                })
            },
            move |workspace, result| match result {
                Ok(item) => workspace.launch_pr_review(&project_root, &item, Some(pending)),
                Err(error) => workspace.report_failure(
                    Some(pending),
                    ErrorCode::OperationRefused,
                    "Could not launch PR review",
                    error.to_string(),
                ),
            },
        );
    }

    fn launch_pr_review(
        &self,
        project_root: &Path,
        item: &PrItem,
        pending: Option<PendingRequest>,
    ) {
        let cwd = item
            .worktree_path
            .clone()
            .unwrap_or_else(|| project_root.to_path_buf());
        self.launch_controlled(
            CreateSessionParams {
                kind: SessionKind::Codex,
                command: None,
                args: Vec::new(),
                cwd: Some(cwd),
                name: Some(format!("review: PR #{}", item.pull_request.id)),
                project_root: Some(project_root.to_path_buf()),
                worktree_path: item.worktree_path.clone(),
                initial_input: Some(format!("/review-pr {}", item.pull_request.id)),
            },
            pending,
        );
    }

    pub(super) fn pr_attention_count(&self) -> usize {
        self.pr_preferences
            .borrow()
            .projects
            .values()
            .map(|state| state.attention.len())
            .sum()
    }

    pub(super) fn update_pr_indicator(&self) {
        let count = self.pr_attention_count();
        if count == 0 {
            self.pr_button.set_label("PR");
            self.pr_button
                .set_tooltip_text(Some("Active Azure DevOps pull requests"));
        } else {
            self.pr_button.set_label("PR!");
            self.pr_button
                .set_tooltip_text(Some(&format!("{count} pull request attention marker(s)")));
        }
    }
}

const fn attention_name(attention: PrAttention) -> &'static str {
    match attention {
        PrAttention::New => "new",
        PrAttention::Published => "published",
        PrAttention::Review => "review",
    }
}
