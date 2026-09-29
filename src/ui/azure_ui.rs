use super::*;
use crate::azure::{
    AzureClient, AzurePrList, PrAttention, PrContext, PrItem, PrPreferences, PrProjectState,
    acknowledge_attention, reconcile_attention,
};
use crate::control::{
    PrAcknowledgeParams, PrLaunchReviewParams, PrListParams, PrSetAutoReviewParams,
};
use crate::persist::now_millis;
use crate::providers::{AgentProvider, ProviderDiscovery, recent_mutated_paths};
use crate::worktrees::{WorktreeInfo, WorktreeManager};
use std::collections::BTreeSet;

#[derive(Debug)]
struct LoadedPrContext {
    preferences: PrPreferences,
    context: PrContext,
    changed: Vec<u64>,
}

struct PreloadedPrContext {
    selected: Option<SelectedPrContext>,
    context: PrContext,
}

/// The pull request dialog for one Azure DevOps project.
#[derive(Clone)]
pub(super) struct PrDialog {
    pub(super) modal: modal::Modal,
    pub(super) root: adw::EntryRow,
    pub(super) auto_review: adw::SwitchRow,
    pub(super) list: gtk::ListBox,
    pub(super) loading: gtk::Box,
    pub(super) updating_toggle: Rc<Cell<bool>>,
    refresh: gtk::Button,
}

impl PrDialog {
    pub(super) fn build(parent: &adw::ApplicationWindow) -> Self {
        let root = adw::EntryRow::builder()
            .title("Project Root")
            .show_apply_button(true)
            .build();
        let auto_review = adw::SwitchRow::builder()
            .title("Auto-Review New Attention")
            .subtitle("Launch a Codex review when a colleague's PR needs attention")
            .build();
        let project = adw::PreferencesGroup::new();
        project.add(&root);
        project.add(&auto_review);

        let loading = gtk::Box::new(gtk::Orientation::Horizontal, 8);
        loading.append(&adw::Spinner::new());
        let loading_label = gtk::Label::new(Some("Loading active pull requests…"));
        loading_label.add_css_class("dim-label");
        loading.append(&loading_label);
        loading.set_visible(false);
        let list = gtk::ListBox::new();
        list.add_css_class("boxed-list");
        list.set_selection_mode(gtk::SelectionMode::None);
        let active = adw::PreferencesGroup::builder()
            .title("Active Pull Requests")
            .build();
        active.set_header_suffix(Some(&loading));
        active.add(&list);

        let page = adw::PreferencesPage::new();
        page.add(&project);
        page.add(&active);
        let refresh = gtk::Button::builder()
            .icon_name("view-refresh-symbolic")
            .tooltip_text("Refresh active pull requests")
            .build();
        let modal = modal::Modal::new(parent, "Pull Requests", 900, 720, &page);
        modal.header().pack_start(&refresh);
        Self {
            modal,
            root,
            auto_review,
            list,
            loading,
            updating_toggle: Rc::new(Cell::new(false)),
            refresh,
        }
    }
}

impl Workspace {
    pub(super) fn connect_prs(&self) {
        let workspace = self.clone();
        self.prs
            .refresh
            .connect_clicked(move |_| workspace.refresh_pr_panel(None));
        let workspace = self.clone();
        self.prs
            .root
            .connect_apply(move |_| workspace.refresh_pr_panel(None));
        let workspace = self.clone();
        self.prs
            .modal
            .connect_show(move || workspace.prepare_pr_panel());
        let workspace = self.clone();
        self.prs.auto_review.connect_active_notify(move |row| {
            if workspace.prs.updating_toggle.get() {
                return;
            }
            let root = workspace.prs.root.text().trim().to_owned();
            if root.is_empty() {
                workspace.show_error("Select a project before changing auto-review");
                return;
            }
            workspace.set_auto_review(
                crate::control::PrSetAutoReviewParams {
                    project_root: PathBuf::from(root),
                    enabled: row.is_active(),
                },
                None,
            );
        });
    }
}

impl Workspace {
    pub(super) fn refresh_selected_pr_context(&self, session_id: &str) {
        self.clear_selected_pr_context();
        let Some((project_root, worktree_path, kind, conversation_id, cwd)) =
            self.sessions.borrow().get(session_id).and_then(|session| {
                Some((
                    session.record.project_root.clone(),
                    session
                        .record
                        .worktree_path
                        .clone()
                        .or(session.record.cwd.clone())?,
                    session.record.kind,
                    session.record.conversation_id.clone(),
                    session.record.cwd.clone(),
                ))
            })
        else {
            return;
        };
        let session_id = session_id.to_owned();
        let attic = self.paths.attic_dir();
        let preferences = self.pr_preferences.borrow().clone();
        let live_paths = self.live_worktree_paths();
        let generation = self.pr_context_generation.clone();
        let requested = generation.fetch_add(1, Ordering::Relaxed) + 1;
        self.run_slow(
            move || {
                if generation.load(Ordering::Relaxed) != requested {
                    return Err("a newer session was selected".into());
                }
                let manager = WorktreeManager::new(attic);
                let project_root = match project_root {
                    Some(project_root) => project_root,
                    None => manager.repository_root(&worktree_path)?,
                };
                let branch = manager.branch_for_worktree(&worktree_path)?;
                let Some(list) = AzureClient::from_environment().list_active(&project_root)? else {
                    return Err("project origin is not an Azure DevOps repository".into());
                };
                let root_key = project_root.to_string_lossy().to_string();
                let state = preferences.project(&root_key);
                let worktrees = manager
                    .list(&project_root, &live_paths)
                    .ok()
                    .map(|inventory| inventory.worktrees)
                    .unwrap_or_default();
                let context = pr_context_from_list(project_root, list, &state, &worktrees);
                let mut selected = branch.as_deref().and_then(|branch| {
                    context
                        .pull_requests
                        .iter()
                        .find(|item| item.pull_request.source_branch == branch)
                        .map(|item| SelectedPrContext {
                            session_id: session_id.clone(),
                            pull_request: item.pull_request.clone(),
                        })
                });

                if selected.is_none() {
                    let provider = match kind {
                        SessionKind::Claude => Some(AgentProvider::Claude),
                        SessionKind::Codex => Some(AgentProvider::Codex),
                        _ => None,
                    };
                    if let Some(provider) = provider {
                        let discovered = ProviderDiscovery::from_environment()
                            .discover(now_millis(), &BTreeSet::new())?;
                        let agent_session = discovered.into_iter().find(|candidate| {
                            candidate.provider == provider
                                && conversation_id.as_ref().map_or_else(
                                    || paths_match(candidate.cwd.as_deref(), cwd.as_deref()),
                                    |conversation_id| {
                                        candidate.provider_session_id == *conversation_id
                                    },
                                )
                        });
                        if let Some(agent_session) = agent_session {
                            for path in recent_mutated_paths(&agent_session.log_path, 20)? {
                                let Some(edit_branch) = manager
                                    .branch_for_path_in_repo(&context.project_root, &path)?
                                else {
                                    continue;
                                };
                                if let Some(item) = context
                                    .pull_requests
                                    .iter()
                                    .find(|item| item.pull_request.source_branch == edit_branch)
                                {
                                    selected = Some(SelectedPrContext {
                                        session_id: session_id.clone(),
                                        pull_request: item.pull_request.clone(),
                                    });
                                    break;
                                }
                            }
                        }
                    }
                }
                Ok(PreloadedPrContext { selected, context })
            },
            |workspace, result| {
                let Ok(loaded) = result else {
                    return;
                };
                workspace.cache_pr_context(&loaded.context);
                if workspace.prs.modal.is_visible()
                    && pr_cache_key(Path::new(workspace.prs.root.text().trim()))
                        == pr_cache_key(&loaded.context.project_root)
                {
                    workspace.render_pr_context(&loaded.context);
                }
                if let Some(selected) = loaded.selected
                    && workspace.selected_session_id().as_deref()
                        == Some(selected.session_id.as_str())
                {
                    workspace.render_selected_pr_context(selected);
                }
            },
        );
    }

    pub(super) fn clear_selected_pr_context(&self) {
        self.selected_pr.borrow_mut().take();
        self.context_pr.set_visible(false);
    }

    fn render_selected_pr_context(&self, context: SelectedPrContext) {
        let pull_request = &context.pull_request;
        self.context_pr_number
            .set_label(&format!("PR #{}", pull_request.id));
        self.context_pr_title.set_text(&pull_request.title);
        self.context_pr_title
            .set_tooltip_text(Some(&pull_request.title));
        self.context_pr_author
            .set_text(&format!("by {}", pull_request.author));
        self.context_pr_threads
            .set_text(&format!("● {} unresolved", pull_request.unresolved_threads));
        *self.selected_pr.borrow_mut() = Some(context);
        self.context_pr.set_visible(true);
    }

    pub(super) fn open_selected_pr_context(&self) {
        let Some(context) = self.selected_pr.borrow().clone() else {
            return;
        };
        let url = context.pull_request.url;
        let launcher = gtk::UriLauncher::new(&url);
        let workspace = self.clone();
        launcher.launch(
            Some(&self.window),
            None::<&gio::Cancellable>,
            move |result| {
                if let Err(error) = result {
                    workspace.show_error(&format!("Could not open PR: {error}"));
                }
            },
        );
    }

    pub(super) fn open_pr_for_project(&self, root: &str) {
        self.prs.root.set_text(root);
        self.render_cached_pr_context(root);
        if self.prs.modal.is_visible() {
            self.refresh_pr_panel(None);
        } else {
            self.prs.modal.present();
        }
    }

    pub(super) fn prepare_pr_panel(&self) {
        if self.prs.root.text().trim().is_empty()
            && let Some(root) = self.preferred_project_root()
        {
            self.prs.root.set_text(&root);
        }
        self.render_cached_pr_context(self.prs.root.text().trim());
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
        let root = self.prs.root.text().trim().to_owned();
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
        let preferences = self.pr_preferences.borrow().clone();
        let attic = self.paths.attic_dir();
        let live_paths = self.live_worktree_paths();
        let requested_key = pr_cache_key(&params.project_root);
        let had_cached = self.pr_context_cache.borrow().contains_key(&requested_key);
        self.prs.loading.set_visible(!had_cached);
        let finish = move |workspace: &Self, result: PersistResult<LoadedPrContext>| match result {
            Ok(loaded) => {
                workspace.prs.loading.set_visible(false);
                *workspace.pr_preferences.borrow_mut() = loaded.preferences;
                workspace.update_pr_indicator();
                workspace.prs.updating_toggle.set(true);
                workspace
                    .prs
                    .auto_review
                    .set_active(loaded.context.auto_review);
                workspace.prs.updating_toggle.set(false);
                workspace.cache_pr_context(&loaded.context);
                if pr_cache_key(Path::new(workspace.prs.root.text().trim()))
                    == pr_cache_key(&loaded.context.project_root)
                {
                    workspace.render_pr_context(&loaded.context);
                }

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
                workspace.prs.loading.set_visible(false);
                if !had_cached
                    && pr_cache_key(Path::new(workspace.prs.root.text().trim())) == requested_key
                {
                    workspace.render_pr_message(&format!("ERROR  {error}"));
                }
                workspace.report_failure(
                    pending,
                    ErrorCode::OperationRefused,
                    "Could not list pull requests",
                    error.to_string(),
                );
            }
        };
        self.run_slow(
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
                let loaded = LoadedPrContext {
                    preferences,
                    context: PrContext {
                        project_root,
                        repository: list.repository,
                        current_user: list.current_user,
                        auto_review: reconciliation.state.auto_review,
                        pull_requests,
                    },
                    changed,
                };
                Ok((loaded, value))
            },
            move |workspace, result| match result {
                // The preference write stays on the durable queue with the other writes.
                Ok((loaded, value)) => workspace.run_io(
                    move || {
                        store
                            .set_preference("pullRequests", &value)
                            .map(|()| loaded)
                    },
                    finish,
                ),
                Err(error) => finish(workspace, Err(error)),
            },
        );
    }

    fn render_pr_message(&self, text: &str) {
        while let Some(child) = self.prs.list.first_child() {
            self.prs.list.remove(&child);
        }
        let row = adw::ActionRow::builder().title(text).build();
        row.add_css_class("dim-label");
        self.prs.list.append(&row);
    }

    fn render_pr_context(&self, context: &PrContext) {
        while let Some(child) = self.prs.list.first_child() {
            self.prs.list.remove(&child);
        }
        if context.pull_requests.is_empty() {
            self.render_pr_message("No active pull requests");
            return;
        }
        for item in &context.pull_requests {
            self.prs.list.append(&self.pr_row(context, item));
        }
    }

    fn cache_pr_context(&self, context: &PrContext) {
        self.pr_context_cache
            .borrow_mut()
            .insert(pr_cache_key(&context.project_root), context.clone());
    }

    fn render_cached_pr_context(&self, root: &str) -> bool {
        let key = pr_cache_key(Path::new(root));
        let context = self.pr_context_cache.borrow().get(&key).cloned();
        let Some(context) = context else {
            return false;
        };
        self.prs.updating_toggle.set(true);
        self.prs.auto_review.set_active(context.auto_review);
        self.prs.updating_toggle.set(false);
        self.render_pr_context(&context);
        true
    }

    fn pr_row(&self, context: &PrContext, item: &PrItem) -> adw::ActionRow {
        let pull_request = &item.pull_request;
        let location = item
            .worktree_path
            .as_ref()
            .map_or("project root".to_owned(), |path| path.display().to_string());
        let row = adw::ActionRow::builder()
            .title(glib::markup_escape_text(&pull_request.title))
            .subtitle(glib::markup_escape_text(&format!(
                "{} · {} → {} · {} unresolved",
                pull_request.author,
                pull_request.source_branch,
                pull_request.target_branch,
                pull_request.unresolved_threads
            )))
            .subtitle_lines(1)
            .build();
        row.set_tooltip_text(Some(&format!("{}\nRuns in {location}", pull_request.title)));

        let open = {
            let workspace = self.clone();
            let url = pull_request.url.clone();
            let project_root = context.project_root.clone();
            let pull_request_id = pull_request.id;
            let attention = item.attention;
            move || {
                let launcher = gtk::UriLauncher::new(&url);
                let launch_workspace = workspace.clone();
                launcher.launch(
                    Some(&workspace.window),
                    None::<&gio::Cancellable>,
                    move |result| {
                        if let Err(error) = result {
                            launch_workspace.show_error(&format!("Could not open PR: {error}"));
                        }
                    },
                );
                if let Some(marker) = attention {
                    workspace.acknowledge_pr(
                        PrAcknowledgeParams {
                            project_root: project_root.clone(),
                            pull_request_id,
                            marker,
                        },
                        None,
                    );
                }
            }
        };
        let number = gtk::Button::with_label(&format!("#{}", pull_request.id));
        number.add_css_class("flat");
        number.add_css_class("accent");
        number.set_valign(gtk::Align::Center);
        number.set_tooltip_text(Some(
            "Open in Azure DevOps and mark current attention viewed",
        ));
        let open_number = open.clone();
        number.connect_clicked(move |_| open_number());
        row.add_prefix(&number);

        if let Some(attention) = item.attention {
            row.add_suffix(&badge(attention_name(attention), "accent"));
        }
        if pull_request.is_draft {
            row.add_suffix(&badge("draft", "dim-label"));
        }
        let view = gtk::Button::builder()
            .icon_name("adw-external-link-symbolic")
            .valign(gtk::Align::Center)
            .tooltip_text("Open in the browser and mark current attention viewed")
            .build();
        view.add_css_class("flat");
        view.connect_clicked(move |_| open());
        row.add_suffix(&view);
        let review = gtk::Button::builder()
            .label("Review")
            .valign(gtk::Align::Center)
            .tooltip_text("Launch Codex with the review-pr workflow")
            .build();
        let review_workspace = self.clone();
        let review_root = context.project_root.clone();
        let review_item = item.clone();
        review.connect_clicked(move |_| {
            review_workspace.launch_pr_review(&review_root, &review_item, None)
        });
        row.add_suffix(&review);
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
                    } else if workspace.prs.modal.is_visible() {
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
        self.run_slow(
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
        self.menus.set_pull_request_attention(count);
    }
}

fn paths_match(left: Option<&Path>, right: Option<&Path>) -> bool {
    match (left, right) {
        (Some(left), Some(right)) => {
            left.canonicalize().unwrap_or_else(|_| left.to_path_buf())
                == right.canonicalize().unwrap_or_else(|_| right.to_path_buf())
        }
        _ => false,
    }
}

fn pr_context_from_list(
    project_root: PathBuf,
    list: AzurePrList,
    state: &PrProjectState,
    worktrees: &[WorktreeInfo],
) -> PrContext {
    let pull_requests = list
        .pull_requests
        .into_iter()
        .map(|pull_request| {
            let worktree_path = worktrees
                .iter()
                .find(|worktree| {
                    worktree.branch.as_deref() == Some(pull_request.source_branch.as_str())
                })
                .map(|worktree| worktree.path.clone());
            PrItem {
                attention: state.attention.get(&pull_request.id).copied(),
                pull_request,
                worktree_path,
            }
        })
        .collect();
    PrContext {
        project_root,
        repository: list.repository,
        current_user: list.current_user,
        auto_review: state.auto_review,
        pull_requests,
    }
}

fn pr_cache_key(root: &Path) -> String {
    root.canonicalize()
        .unwrap_or_else(|_| root.to_path_buf())
        .to_string_lossy()
        .into_owned()
}

const fn attention_name(attention: PrAttention) -> &'static str {
    match attention {
        PrAttention::New => "new",
        PrAttention::Published => "published",
        PrAttention::Review => "review",
    }
}

fn badge(text: &str, class: &str) -> gtk::Label {
    let label = gtk::Label::new(Some(text));
    label.add_css_class("caption-heading");
    label.add_css_class(class);
    label.set_valign(gtk::Align::Center);
    label
}
