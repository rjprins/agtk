use super::*;
use crate::azure::{
    AzureClient, AzurePrList, PrAttention, PrContext, PrItem, PrProjectState,
    acknowledge_attention, reconcile_attention,
};
use crate::control::{
    PrAcknowledgeParams, PrLaunchReviewParams, PrListParams, PrSetAutoReviewParams,
};
use crate::persist::now_millis;
use crate::providers::{AgentProvider, ProviderDiscovery, recent_mutated_paths};
use crate::worktrees::{PorcelainWorktree, WorktreeLayout, WorktreeManager};
use std::collections::BTreeSet;

#[derive(Debug)]
struct LoadedPrContext {
    root_key: String,
    state: PrProjectState,
    context: PrContext,
    changed: Vec<u64>,
}

struct PreloadedPrContext {
    session_id: String,
    selected: Option<SelectedPrContext>,
    context: PrContext,
}

/// How often the sidebar's PR buttons look for new activity.
const PR_POLL_INTERVAL: Duration = Duration::from_secs(120);

/// Widest the PR list grows inside the dialog.
const PR_CONTENT_WIDTH: i32 = 1400;

/// Keep the header and every PR aligned, including rows with status badges.
struct PrColumns {
    number: gtk::SizeGroup,
    pbis: gtk::SizeGroup,
    author: gtk::SizeGroup,
    comments: gtk::SizeGroup,
    actions: gtk::SizeGroup,
}

impl PrColumns {
    fn new() -> Self {
        Self {
            number: gtk::SizeGroup::new(gtk::SizeGroupMode::Horizontal),
            pbis: gtk::SizeGroup::new(gtk::SizeGroupMode::Horizontal),
            author: gtk::SizeGroup::new(gtk::SizeGroupMode::Horizontal),
            comments: gtk::SizeGroup::new(gtk::SizeGroupMode::Horizontal),
            actions: gtk::SizeGroup::new(gtk::SizeGroupMode::Horizontal),
        }
    }

    fn header(&self) -> adw::ActionRow {
        let row = adw::ActionRow::builder().title("Pull request").build();
        row.add_css_class("dim-label");
        let number = gtk::Label::new(Some("PR"));
        self.number.add_widget(&number);
        row.add_prefix(&number);
        row.add_suffix(&Self::label("PBI", &self.pbis, 0.5));
        row.add_suffix(&Self::label("Author", &self.author, 0.0));
        row.add_suffix(&Self::label(
            "Comments\nresolved/total",
            &self.comments,
            0.5,
        ));
        let actions = gtk::Box::new(gtk::Orientation::Horizontal, 0);
        self.actions.add_widget(&actions);
        row.add_suffix(&actions);
        row
    }

    fn label(text: &str, group: &gtk::SizeGroup, xalign: f32) -> gtk::Label {
        let label = gtk::Label::builder()
            .label(text)
            .xalign(xalign)
            .valign(gtk::Align::Center)
            .margin_start(12)
            .margin_end(12)
            .build();
        group.add_widget(&label);
        label
    }
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
    settings: gtk::Button,
    pub(super) review_settings: Rc<RefCell<Option<modal::Modal>>>,
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

        // A preferences page caps its content at 600px, which wraps the PR titles.
        let groups = gtk::Box::new(gtk::Orientation::Vertical, 24);
        groups.append(&project);
        groups.append(&active);
        let clamp = adw::Clamp::builder()
            .maximum_size(PR_CONTENT_WIDTH)
            .tightening_threshold(PR_CONTENT_WIDTH)
            .margin_top(24)
            .margin_bottom(24)
            .margin_start(12)
            .margin_end(12)
            .child(&groups)
            .build();
        let page = gtk::ScrolledWindow::builder()
            .hscrollbar_policy(gtk::PolicyType::Never)
            .child(&clamp)
            .build();
        let refresh = gtk::Button::builder()
            .icon_name("view-refresh-symbolic")
            .tooltip_text("Refresh active pull requests")
            .build();
        let settings = gtk::Button::builder()
            .icon_name("emblem-system-symbolic")
            .tooltip_text("PR review settings")
            .build();
        let modal = modal::Modal::new(parent, "Pull Requests", 1200, 720, &page);
        modal.header().pack_end(&settings);
        modal.header().pack_start(&refresh);
        Self {
            modal,
            root,
            auto_review,
            list,
            loading,
            updating_toggle: Rc::new(Cell::new(false)),
            refresh,
            settings,
            review_settings: Rc::default(),
        }
    }
}

impl Workspace {
    pub(super) fn connect_prs(&self) {
        let workspace = self.clone();
        self.prs
            .settings
            .connect_clicked(move |_| workspace.show_review_settings());
        let workspace = self.clone();
        // Tick faster than the interval so a poll is never a whole round late.
        glib::timeout_add_local(PR_POLL_INTERVAL / 4, move || {
            workspace.poll_pull_requests_if_focused();
            glib::ControlFlow::Continue
        });
        // Catch up when the user comes back after the polls were paused.
        let workspace = self.clone();
        self.window
            .connect_is_active_notify(move |_| workspace.poll_pull_requests_if_focused());
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
    pub(super) fn show_review_settings(&self) {
        if let Some(dialog) = self.prs.review_settings.borrow().as_ref()
            && dialog.is_visible()
        {
            dialog.window().present();
            return;
        }
        if let Some(previous) = self.prs.review_settings.borrow_mut().take() {
            previous.window().destroy();
        }
        let current = self.pr_preferences.borrow().review.clone();
        let group = adw::PreferencesGroup::builder()
            .title("Codex PR Reviews")
            .description("Applies to automatic reviews and reviews launched through agtkctl or MCP. The Launch dialog uses its selected agent and options.")
            .build();
        let model = adw::EntryRow::builder()
            .title("Model (empty uses Codex default)")
            .text(&current.model)
            .build();
        group.add(&model);
        fn choice(
            group: &adw::PreferencesGroup,
            title: &str,
            values: &[&str],
            selected: &str,
        ) -> adw::ComboRow {
            let labels: Vec<&str> = values
                .iter()
                .map(|value| {
                    if value.is_empty() {
                        "Codex default"
                    } else {
                        value
                    }
                })
                .collect();
            let row = adw::ComboRow::builder()
                .title(title)
                .model(&gtk::StringList::new(&labels))
                .selected(
                    values
                        .iter()
                        .position(|value| *value == selected)
                        .unwrap_or(0) as u32,
                )
                .build();
            group.add(&row);
            row
        }
        const EFFORTS: &[&str] = &[
            "", "minimal", "low", "medium", "high", "xhigh", "max", "ultra",
        ];
        const APPROVALS: &[&str] = &["", "on-request", "never"];
        const SANDBOXES: &[&str] = &["", "read-only", "workspace-write", "danger-full-access"];
        let effort = choice(&group, "Reasoning effort", EFFORTS, &current.effort);
        let approval = choice(&group, "Approval policy", APPROVALS, &current.approval);
        approval.set_subtitle(
            "never prevents approval prompts; commands outside the sandbox will fail.",
        );
        let sandbox = choice(&group, "Sandbox permissions", SANDBOXES, &current.sandbox);
        sandbox.set_subtitle("danger-full-access allows commands without sandbox restrictions.");
        let prompt_group = adw::PreferencesGroup::builder()
            .title("Review Prompt")
            .description("Use {pr_id} for the PR number. Default: /review-pr {pr_id}")
            .build();
        let prompt = gtk::TextView::builder()
            .wrap_mode(gtk::WrapMode::WordChar)
            .top_margin(12)
            .bottom_margin(12)
            .left_margin(12)
            .right_margin(12)
            .accessible_role(gtk::AccessibleRole::TextBox)
            .build();
        prompt.update_property(&[gtk::accessible::Property::Label("Review prompt")]);
        prompt.buffer().set_text(&current.prompt);
        let prompt_scroll = gtk::ScrolledWindow::builder()
            .min_content_height(140)
            .hscrollbar_policy(gtk::PolicyType::Never)
            .child(&prompt)
            .build();
        prompt_scroll.add_css_class("card");
        prompt_group.add(&prompt_scroll);
        let page = gtk::Box::new(gtk::Orientation::Vertical, 24);
        page.set_margin_top(24);
        page.set_margin_bottom(24);
        page.set_margin_start(24);
        page.set_margin_end(24);
        page.append(&group);
        page.append(&prompt_group);
        let scroll = gtk::ScrolledWindow::builder()
            .hscrollbar_policy(gtk::PolicyType::Never)
            .child(&page)
            .build();
        let dialog = modal::Modal::new(&self.window, "PR Review Settings", 680, 640, &scroll);
        let save = gtk::Button::with_label("Save");
        save.add_css_class("suggested-action");
        dialog.header().pack_end(&save);
        let workspace = self.clone();
        let closing = dialog.window().downgrade();
        save.connect_clicked(move |save| {
            let buffer = prompt.buffer();
            let text = buffer
                .text(&buffer.start_iter(), &buffer.end_iter(), false)
                .to_string();
            if text.trim().is_empty() {
                workspace.show_error("Review prompt cannot be empty");
                return;
            }
            let Some(store) = workspace.store.borrow().clone() else {
                workspace.show_error("Workspace is loading");
                return;
            };
            let settings = crate::azure::PrReviewSettings {
                model: model.text().trim().to_owned(),
                effort: EFFORTS[effort.selected() as usize].to_owned(),
                approval: APPROVALS[approval.selected() as usize].to_owned(),
                sandbox: SANDBOXES[sandbox.selected() as usize].to_owned(),
                prompt: text,
            };
            let mut preferences = workspace.pr_preferences.borrow().clone();
            preferences.review = settings.clone();
            let value = match serde_json::to_value(&preferences) {
                Ok(value) => value,
                Err(error) => {
                    workspace.show_error(&error.to_string());
                    return;
                }
            };
            save.set_sensitive(false);
            let save = save.clone();
            let closing = closing.clone();
            workspace.run_io(
                move || store.set_preference("pullRequests", &value),
                move |workspace, result| {
                    save.set_sensitive(true);
                    match result {
                        Ok(()) => {
                            workspace.pr_preferences.borrow_mut().review = settings;
                            if let Some(window) = closing.upgrade() {
                                window.set_visible(false);
                            }
                        }
                        Err(error) => workspace
                            .show_error(&format!("Could not save review settings: {error}")),
                    }
                },
            );
        });
        *self.prs.review_settings.borrow_mut() = Some(dialog.clone());
        dialog.present();
    }

    pub(super) fn refresh_selected_pr_context(&self, session_id: &str) {
        self.clear_selected_pr_context();
        let Some((project_root, worktree_path, kind, conversation_id, cwd, name)) =
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
                    session.record.name.clone(),
                ))
            })
        else {
            return;
        };
        let cached = self.session_pr_cache.borrow().get(session_id).cloned();
        if let Some(pull_request) = cached {
            self.render_selected_pr_context(SelectedPrContext {
                session_id: session_id.to_owned(),
                pull_request,
            });
        }
        let session_id = session_id.to_owned();
        let attic = self.paths.attic_dir();
        let preferences = self.pr_preferences.borrow().clone();
        let generation = self.pr_context_generation.clone();
        let requested = generation.load(Ordering::Relaxed);
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
                let worktrees = manager.linked_worktrees(&project_root).unwrap_or_default();
                let layout = manager.layout(&project_root).ok();
                let context =
                    pr_context_from_list(project_root, list, &state, &worktrees, layout.as_ref());
                // A review runs in its own detached pr-<id> checkout, so its
                // name is what links it to the PR.
                let reviewed = review_pr_id(&name).and_then(|id| {
                    context
                        .pull_requests
                        .iter()
                        .find(|item| item.pull_request.id == id)
                });
                let on_branch = branch.as_deref().and_then(|branch| {
                    context
                        .pull_requests
                        .iter()
                        .find(|item| item.pull_request.source_branch == branch)
                });
                let mut selected = reviewed.or(on_branch).map(|item| SelectedPrContext {
                    session_id: session_id.clone(),
                    pull_request: item.pull_request.clone(),
                });

                if selected.is_none()
                    && let Some(provider) = AgentProvider::from_kind(kind)
                {
                    let discovered = ProviderDiscovery::from_environment()
                        .discover(now_millis(), &BTreeSet::new())?;
                    let agent_session = discovered.into_iter().find(|candidate| {
                        candidate.provider == provider
                            && conversation_id.as_ref().map_or_else(
                                || paths_match(candidate.cwd.as_deref(), cwd.as_deref()),
                                |conversation_id| candidate.provider_session_id == *conversation_id,
                            )
                    });
                    if let Some(agent_session) = agent_session {
                        for path in recent_mutated_paths(&agent_session.log_path, 20)? {
                            let Some(edit_branch) =
                                manager.branch_for_path_in_repo(&context.project_root, &path)?
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
                Ok(PreloadedPrContext {
                    session_id,
                    selected,
                    context,
                })
            },
            move |workspace, result| {
                if workspace.pr_context_generation.load(Ordering::Relaxed) != requested {
                    return;
                }
                let Ok(loaded) = result else {
                    return;
                };
                if workspace.selected_session_id().as_deref() != Some(loaded.session_id.as_str()) {
                    return;
                }
                workspace.cache_pr_context(&loaded.context);
                if workspace.prs.modal.is_visible()
                    && pr_cache_key(Path::new(workspace.prs.root.text().trim()))
                        == pr_cache_key(&loaded.context.project_root)
                {
                    workspace.render_pr_context(&loaded.context);
                }
                if let Some(selected) = loaded.selected {
                    workspace
                        .session_pr_cache
                        .borrow_mut()
                        .insert(selected.session_id.clone(), selected.pull_request.clone());
                    workspace.render_selected_pr_context(selected);
                } else {
                    workspace
                        .session_pr_cache
                        .borrow_mut()
                        .remove(&loaded.session_id);
                    workspace.clear_selected_pr_context();
                }
                workspace.save_session_pr_cache();
                workspace.refresh_session_pr_button(&loaded.session_id);
            },
        );
    }

    pub(super) fn clear_selected_pr_context(&self) {
        self.pr_context_generation.fetch_add(1, Ordering::Relaxed);
        self.selected_pr.borrow_mut().take();
        self.context_pr.set_visible(false);
    }

    pub(super) fn save_session_pr_cache(&self) {
        if let Ok(value) = serde_json::to_value(&*self.session_pr_cache.borrow()) {
            self.save_preference("sessionPullRequests", value);
        }
    }

    /// Links every session of the project to its PR, so the sidebar shows a
    /// PR button without the session ever being selected.
    fn match_sessions_to_prs(&self, context: &PrContext) {
        let records = self
            .sessions
            .borrow()
            .values()
            .map(|session| session.record.clone())
            .collect::<Vec<_>>();
        let mut changed = Vec::new();
        for record in records {
            let mut cache = self.session_pr_cache.borrow_mut();
            match session_pr_from_context(&record, context) {
                Some(pull_request) => {
                    if cache.get(&record.id) != Some(&pull_request) {
                        cache.insert(record.id.clone(), pull_request);
                        changed.push(record.id);
                    }
                }
                // A PR matched from the agent's edits stays until it leaves the list.
                None if in_project(&record, context)
                    && cache.get(&record.id).is_some_and(|cached| {
                        !context
                            .pull_requests
                            .iter()
                            .any(|item| item.pull_request.id == cached.id)
                    }) =>
                {
                    cache.remove(&record.id);
                    changed.push(record.id);
                }
                None => {}
            }
        }
        if changed.is_empty() {
            return;
        }
        self.save_session_pr_cache();
        let selected = self.selected_session_id();
        for id in &changed {
            self.refresh_session_pr_button(id);
            if selected.as_deref() == Some(id.as_str()) {
                self.refresh_selected_pr_context(id);
            }
        }
    }

    /// Shows the row's PR button when the session has a PR, matching it from
    /// the project's PR list if it was not linked yet.
    pub(super) fn refresh_session_pr_button(&self, id: &str) {
        let Some((record, button, label, dot)) = self.sessions.borrow().get(id).map(|session| {
            (
                session.record.clone(),
                session.pr_button.clone(),
                session.pr_label.clone(),
                session.pr_attention_dot.clone(),
            )
        }) else {
            return;
        };
        let mut pull_request = self.session_pr_cache.borrow().get(id).cloned();
        if pull_request.is_none()
            && let Some(project) = record.project_root.as_deref()
            && let Some(context) = self.pr_context_cache.borrow().get(&pr_cache_key(project))
            && let Some(found) = session_pr_from_context(&record, context)
        {
            self.session_pr_cache
                .borrow_mut()
                .insert(id.to_owned(), found.clone());
            self.save_session_pr_cache();
            pull_request = Some(found);
        }
        match pull_request {
            Some(pull_request) => {
                let has_new_comments = record
                    .project_root
                    .as_deref()
                    .and_then(|root| self.pr_attention(root, pull_request.id))
                    == Some(PrAttention::Review);
                label.set_text(&format!("#{}", pull_request.id));
                let mut description =
                    format!("Open PR #{}: {}", pull_request.id, pull_request.title);
                if has_new_comments {
                    description.push_str("\nNew review comments; open to mark viewed");
                }
                button.set_tooltip_text(Some(&description));
                button.update_property(&[gtk::accessible::Property::Label(&description)]);
                dot.set_visible(has_new_comments);
                button.set_visible(true);
            }
            None => {
                dot.set_visible(false);
                button.set_visible(false);
            }
        }
    }

    pub(super) fn open_session_pr(&self, id: &str) {
        let pull_request = self.session_pr_cache.borrow().get(id).cloned();
        match pull_request {
            Some(pull_request) => {
                self.open_pr_link(&pull_request.url);
                let project_root = self
                    .sessions
                    .borrow()
                    .get(id)
                    .and_then(|session| session.record.project_root.clone());
                if let Some(project_root) = project_root
                    && let Some(marker) = self.pr_attention(&project_root, pull_request.id)
                {
                    self.acknowledge_pr(
                        PrAcknowledgeParams {
                            project_root: PathBuf::from(pr_cache_key(&project_root)),
                            pull_request_id: pull_request.id,
                            marker,
                        },
                        None,
                    );
                }
            }
            None => self
                .overlay
                .add_toast(adw::Toast::new("No pull request is linked to this session")),
        }
    }

    fn render_selected_pr_context(&self, context: SelectedPrContext) {
        let pull_request = &context.pull_request;
        self.context_pr_number
            .set_label(&format!("PR #{}", pull_request.id));
        while let Some(child) = self.context_pr_pbis.first_child() {
            self.context_pr_pbis.remove(&child);
        }
        for pbi in &pull_request.linked_pbis {
            let button = gtk::Button::with_label(&format!("PBI #{}", pbi.id));
            button.add_css_class("flat");
            button.add_css_class("accent");
            button.set_tooltip_text(Some(&pbi.title));
            button.set_widget_name(&format!("pbi-{}", pbi.id));
            let workspace = self.clone();
            let url = pbi.url.clone();
            button.connect_clicked(move |_| workspace.open_pr_link(&url));
            self.context_pr_pbis.append(&button);
        }
        self.context_pr_pbis
            .set_visible(!pull_request.linked_pbis.is_empty());
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
        self.open_pr_link(&context.pull_request.url);
    }

    fn open_pr_link(&self, url: &str) {
        let launcher = gtk::UriLauncher::new(url);
        let workspace = self.clone();
        launcher.launch(
            Some(&self.window),
            None::<&gio::Cancellable>,
            move |result| {
                if let Err(error) = result {
                    workspace.show_error(&format!("Could not open Azure DevOps link: {error}"));
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
            ControlCommand::PrList(params) => self.list_prs(params, Some(pending), false),
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
            false,
        );
    }

    /// A poll runs in the background: it stays silent and leaves the dialog's spinner alone.
    fn list_prs(&self, params: PrListParams, pending: Option<PendingRequest>, poll: bool) {
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
        let requested_key = pr_cache_key(&params.project_root);
        let had_cached = self.pr_context_cache.borrow().contains_key(&requested_key);
        if !poll {
            self.prs.loading.set_visible(!had_cached);
        }
        let finish = move |workspace: &Self, result: PersistResult<LoadedPrContext>| match result {
            Ok(loaded) => {
                if !poll {
                    workspace.prs.loading.set_visible(false);
                }
                workspace.update_pr_indicator();
                workspace.cache_pr_context(&loaded.context);
                workspace.match_sessions_to_prs(&loaded.context);
                if pr_cache_key(Path::new(workspace.prs.root.text().trim()))
                    == pr_cache_key(&loaded.context.project_root)
                {
                    workspace.prs.updating_toggle.set(true);
                    workspace
                        .prs
                        .auto_review
                        .set_active(loaded.context.auto_review);
                    workspace.prs.updating_toggle.set(false);
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
                            && item
                                .attention
                                .is_some_and(PrAttention::launches_auto_review)
                            && !item.pull_request.is_draft
                            && !item.pull_request.is_own_author
                    }) {
                        workspace.launch_pr_review(&loaded.context.project_root, item, None);
                    }
                }
            }
            Err(_) if poll => {}
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
        let finish = move |workspace: &Self, result: PersistResult<LoadedPrContext>| {
            finish(workspace, result);
            if poll {
                workspace
                    .pr_polls
                    .set(workspace.pr_polls.get().saturating_sub(1));
            }
        };
        let worker = if poll {
            &self.pr_poll_io
        } else {
            &self.slow_io
        };
        self.run_on(
            worker,
            move || {
                let project_root = params.project_root.canonicalize()?;
                let root_key = project_root.to_string_lossy().to_string();
                let Some(list) = AzureClient::from_environment().list_active(&project_root)? else {
                    return Err("project origin is not an Azure DevOps repository".into());
                };
                let previous = preferences.projects.get(&root_key);
                let reconciliation = reconcile_attention(previous, &list.pull_requests);
                let changed = reconciliation.changed.iter().copied().collect::<Vec<_>>();

                let manager = WorktreeManager::new(attic);
                let worktrees = manager.linked_worktrees(&project_root).unwrap_or_default();
                let layout = manager.layout(&project_root).ok();
                let pull_requests = list
                    .pull_requests
                    .into_iter()
                    .map(|pull_request| {
                        let worktree_path =
                            worktree_for_branch(&worktrees, &pull_request.source_branch);
                        let attention = reconciliation
                            .state
                            .attention
                            .get(&pull_request.id)
                            .copied();
                        let review_path = review_path(layout.as_ref(), pull_request.id);
                        PrItem {
                            pull_request,
                            attention,
                            worktree_path,
                            review_path,
                        }
                    })
                    .collect();
                Ok(LoadedPrContext {
                    root_key,
                    context: PrContext {
                        project_root,
                        repository: list.repository,
                        current_user: list.current_user,
                        auto_review: reconciliation.state.auto_review,
                        pull_requests,
                    },
                    changed,
                    state: reconciliation.state,
                })
            },
            move |workspace, result| {
                let mut loaded = match result {
                    Ok(loaded) => loaded,
                    Err(error) => return finish(workspace, Err(error)),
                };
                // Merge into the current state: other projects, or the toggle, may have
                // changed while Azure was answering.
                let mut preferences = workspace.pr_preferences.borrow().clone();
                loaded.state.auto_review = preferences.project(&loaded.root_key).auto_review;
                loaded.context.auto_review = loaded.state.auto_review;
                preferences.set_project(loaded.root_key.clone(), loaded.state.clone());
                let value = match serde_json::to_value(&preferences) {
                    Ok(value) => value,
                    Err(error) => return finish(workspace, Err(error.into())),
                };
                *workspace.pr_preferences.borrow_mut() = preferences;
                // The preference write stays on the durable queue with the other writes.
                workspace.run_io(
                    move || {
                        store
                            .set_preference("pullRequests", &value)
                            .map(|()| loaded)
                    },
                    finish,
                );
            },
        );
    }

    fn render_pr_message(&self, text: &str) {
        while let Some(child) = self.prs.list.first_child() {
            self.prs.list.remove(&child);
        }
        let row = adw::ActionRow::builder()
            .title(text)
            .use_markup(false)
            .build();
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
        let columns = PrColumns::new();
        self.prs.list.append(&columns.header());
        for item in &context.pull_requests {
            self.prs.list.append(&self.pr_row(context, item, &columns));
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

    fn pr_row(&self, context: &PrContext, item: &PrItem, columns: &PrColumns) -> adw::ActionRow {
        let pull_request = &item.pull_request;
        let row = adw::ActionRow::builder()
            .title(glib::markup_escape_text(&pull_request.title))
            .subtitle(glib::markup_escape_text(&format!(
                "{} → {}",
                pull_request.source_branch, pull_request.target_branch
            )))
            .subtitle_lines(1)
            .build();
        row.set_tooltip_text(Some(&match &item.review_path {
            Some(location) => format!(
                "{}\nReview runs in {}",
                pull_request.title,
                location.display()
            ),
            None => pull_request.title.clone(),
        }));

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
        columns.number.add_widget(&number);
        row.add_prefix(&number);

        let pbis = gtk::Box::new(gtk::Orientation::Vertical, 0);
        pbis.set_valign(gtk::Align::Center);
        pbis.set_margin_start(12);
        pbis.set_margin_end(12);
        if pull_request.linked_pbis.is_empty() {
            let empty = gtk::Label::new(Some("—"));
            empty.set_tooltip_text(Some("No linked PBIs"));
            pbis.append(&empty);
        }
        for pbi in &pull_request.linked_pbis {
            let button = gtk::Button::with_label(&format!("#{}", pbi.id));
            button.add_css_class("flat");
            button.add_css_class("accent");
            let description = format!("Open PBI #{}: {}", pbi.id, pbi.title);
            button.set_tooltip_text(Some(&description));
            button.update_property(&[gtk::accessible::Property::Label(&description)]);
            let workspace = self.clone();
            let url = pbi.url.clone();
            button.connect_clicked(move |_| workspace.open_pr_link(&url));
            pbis.append(&button);
        }
        columns.pbis.add_widget(&pbis);
        row.add_suffix(&pbis);

        let author = PrColumns::label(&pull_request.author, &columns.author, 0.0);
        author.set_width_chars(18);
        author.set_max_width_chars(18);
        author.set_ellipsize(gtk::pango::EllipsizeMode::End);
        author.set_tooltip_text(Some(&pull_request.author));
        row.add_suffix(&author);
        let (count, description) = match pull_request.total_threads {
            Some(total) => (
                format!("{}/{total}", pull_request.resolved_threads),
                format!(
                    "{} of {total} comment threads resolved",
                    pull_request.resolved_threads
                ),
            ),
            None => ("—".to_owned(), "Comment counts unavailable".to_owned()),
        };
        let comments = PrColumns::label(&count, &columns.comments, 0.5);
        comments.set_tooltip_text(Some(&description));
        comments.update_property(&[gtk::accessible::Property::Label(&description)]);
        row.add_suffix(&comments);

        let actions = gtk::Box::new(gtk::Orientation::Horizontal, 6);
        actions.set_hexpand(false);
        let spacer = gtk::Box::new(gtk::Orientation::Horizontal, 0);
        spacer.set_hexpand(true);
        actions.append(&spacer);
        if let Some(attention) = item.attention {
            actions.append(&badge(attention_name(attention), "accent"));
        }
        if pull_request.is_draft {
            actions.append(&badge("draft", "dim-label"));
        }
        let view = gtk::Button::builder()
            .icon_name("adw-external-link-symbolic")
            .valign(gtk::Align::Center)
            .tooltip_text("Open in the browser and mark current attention viewed")
            .build();
        view.add_css_class("flat");
        view.connect_clicked(move |_| open());
        actions.append(&view);
        let launch = gtk::Button::builder()
            .label("Launch")
            .valign(gtk::Align::Center)
            .tooltip_text("Launch an agent or review for this pull request")
            .build();
        let launch_workspace = self.clone();
        let launch_root = context.project_root.clone();
        let launch_item = item.clone();
        launch.connect_clicked(move |_| {
            launch_workspace.open_launch_for_pr(&launch_root, &launch_item)
        });
        actions.append(&launch);
        columns.actions.add_widget(&actions);
        row.add_suffix(&actions);
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
                let worktrees = WorktreeManager::new(attic)
                    .linked_worktrees(&load_root)
                    .unwrap_or_default();
                let worktree_path = worktree_for_branch(&worktrees, &pull_request.source_branch);
                Ok(PrItem {
                    pull_request,
                    attention: None,
                    worktree_path,
                    review_path: None,
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
        let settings = self.pr_preferences.borrow().review.clone();
        self.launch_pr_review_with_params(
            project_root,
            item,
            CreateSessionParams {
                kind: SessionKind::Codex,
                command: None,
                args: settings.args(),
                cwd: None,
                name: None,
                project_root: None,
                worktree_path: None,
                initial_input: Some(settings.prompt(item.pull_request.id)),
            },
            pending,
        );
    }

    pub(super) fn launch_pr_review_with_params(
        &self,
        project_root: &Path,
        item: &PrItem,
        mut params: CreateSessionParams,
        pending: Option<PendingRequest>,
    ) {
        let project_root = project_root.to_path_buf();
        let pull_request = item.pull_request.clone();
        let attic = self.paths.attic_dir();
        let checkout_root = project_root.clone();
        params.project_root = Some(project_root);
        params
            .name
            .get_or_insert_with(|| format!("review: PR #{}", pull_request.id));
        // Each review gets its own detached checkout, so it never runs in a
        // worktree the user is editing and a repeat review reuses it.
        self.run_slow(
            move || {
                WorktreeManager::new(attic).review_checkout(
                    &checkout_root,
                    pull_request.id,
                    &pull_request.source_branch,
                )
            },
            move |workspace, result| match result {
                // A review can start while the user is typing elsewhere, so it never takes focus.
                Ok(worktree_path) => {
                    params.cwd = Some(worktree_path.clone());
                    params.worktree_path = Some(worktree_path);
                    workspace.launch_session(
                        params,
                        None,
                        Some(super::sessions::Placement::in_background()),
                        pending,
                    );
                }
                Err(error) => workspace.report_failure(
                    pending,
                    ErrorCode::OperationRefused,
                    "Could not prepare the PR review checkout",
                    error.to_string(),
                ),
            },
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
        let session_ids = self.sessions.borrow().keys().cloned().collect::<Vec<_>>();
        for id in session_ids {
            self.refresh_session_pr_button(&id);
        }
        // The project headers also carry the attention dot.
        self.rebuild_sidebar();
    }

    fn pr_attention(&self, root: &Path, id: u64) -> Option<PrAttention> {
        self.pr_preferences
            .borrow()
            .projects
            .get(&pr_cache_key(root))?
            .attention
            .get(&id)
            .copied()
    }

    /// `None` for a project without Azure DevOps, else how many PRs have unseen activity.
    pub(super) fn project_pr_attention(&self, root: &str) -> Option<usize> {
        let projects = self.azure_projects.borrow();
        let key = projects.get(root)?.as_ref()?;
        Some(
            self.pr_preferences
                .borrow()
                .projects
                .get(key)
                .map_or(0, |state| state.attention.len()),
        )
    }

    pub(super) fn has_unchecked_projects(&self, roots: &[String]) -> bool {
        let projects = self.azure_projects.borrow();
        roots.iter().any(|root| !projects.contains_key(root))
    }

    /// Polls pause while no agtk window has focus, and never run more often than the interval.
    fn poll_pull_requests_if_focused(&self) {
        let focused = self
            .application
            .windows()
            .iter()
            .any(|window| window.is_active());
        let due = self
            .pr_polled_at
            .get()
            .is_none_or(|polled_at| polled_at.elapsed() >= PR_POLL_INTERVAL);
        if focused && due {
            self.poll_pull_requests();
        }
    }

    /// Finds the Azure DevOps projects in the sidebar and refreshes their PR attention.
    /// Inactive projects show no PR button, so they are not polled.
    pub(super) fn poll_pull_requests(&self) {
        if self.store.borrow().is_none() || self.pr_polls.get() > 0 {
            return;
        }
        self.pr_polled_at.set(Some(std::time::Instant::now()));
        let roots = self
            .project_summaries()
            .into_iter()
            .filter(|project| project.is_active)
            .map(|project| project.root)
            .collect::<Vec<_>>();
        if roots.is_empty() {
            return;
        }
        {
            let mut projects = self.azure_projects.borrow_mut();
            for root in &roots {
                projects.entry(root.clone()).or_insert(None);
            }
        }
        self.pr_polls.set(1);
        self.run_on(
            &self.pr_poll_io,
            move || {
                let client = AzureClient::from_environment();
                Ok(roots
                    .into_iter()
                    .map(|root| {
                        let path = Path::new(&root);
                        let key = client
                            .repository(path)
                            .ok()
                            .flatten()
                            .and_then(|_| path.canonicalize().ok())
                            .map(|path| path.to_string_lossy().to_string());
                        (root, key)
                    })
                    .collect::<Vec<_>>())
            },
            |workspace, result| {
                let checked = result.unwrap_or_default();
                let azure = checked
                    .iter()
                    .filter(|(_, key)| key.is_some())
                    .map(|(root, _)| PathBuf::from(root))
                    .collect::<Vec<_>>();
                workspace.azure_projects.borrow_mut().extend(checked);
                workspace.pr_polls.set(azure.len());
                workspace.rebuild_sidebar();
                for project_root in azure {
                    workspace.list_prs(PrListParams { project_root }, None, true);
                }
            },
        );
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
    worktrees: &[PorcelainWorktree],
    layout: Option<&WorktreeLayout>,
) -> PrContext {
    let pull_requests = list
        .pull_requests
        .into_iter()
        .map(|pull_request| {
            let worktree_path = worktree_for_branch(worktrees, &pull_request.source_branch);
            PrItem {
                attention: state.attention.get(&pull_request.id).copied(),
                review_path: review_path(layout, pull_request.id),
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

/// Where `WorktreeManager::review_checkout` puts the review of one PR.
fn review_path(layout: Option<&WorktreeLayout>, pull_request_id: u64) -> Option<PathBuf> {
    layout?.review_path(pull_request_id).ok()
}

fn worktree_for_branch(worktrees: &[PorcelainWorktree], branch: &str) -> Option<PathBuf> {
    worktrees
        .iter()
        .find(|worktree| worktree.branch.as_deref() == Some(branch))
        .map(|worktree| worktree.path.clone())
}

fn pr_cache_key(root: &Path) -> String {
    root.canonicalize()
        .unwrap_or_else(|_| root.to_path_buf())
        .to_string_lossy()
        .into_owned()
}

/// The PR number in the name of a session that `launch_pr_review` started.
fn review_pr_id(name: &str) -> Option<u64> {
    name.strip_prefix("review: PR #")?.trim().parse().ok()
}

fn in_project(record: &SessionRecord, context: &PrContext) -> bool {
    record
        .project_root
        .as_deref()
        .is_some_and(|root| pr_cache_key(root) == pr_cache_key(&context.project_root))
}

/// The PR a session works on: the one it reviews, else the one whose branch
/// its worktree has checked out.
fn session_pr_from_context(record: &SessionRecord, context: &PrContext) -> Option<AzurePr> {
    if !in_project(record, context) {
        return None;
    }
    if let Some(id) = review_pr_id(&record.name)
        && let Some(item) = context
            .pull_requests
            .iter()
            .find(|item| item.pull_request.id == id)
    {
        return Some(item.pull_request.clone());
    }
    let worktree = pr_cache_key(record.worktree_path.as_deref().or(record.cwd.as_deref())?);
    context
        .pull_requests
        .iter()
        .find(|item| {
            item.worktree_path
                .iter()
                .chain(item.review_path.iter())
                .any(|path| pr_cache_key(path) == worktree)
        })
        .map(|item| item.pull_request.clone())
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

#[cfg(test)]
mod tests {
    use super::{review_pr_id, session_pr_from_context};
    use crate::azure::{AzurePr, AzureRepoRef, PrContext, PrItem};
    use crate::persist::SessionRecord;
    use std::path::PathBuf;

    fn pull_request(id: u64, branch: &str) -> AzurePr {
        serde_json::from_value(serde_json::json!({
            "id": id, "title": format!("Review {branch}"), "author": "Colleague",
            "isOwnAuthor": false, "isDraft": false, "sourceBranch": branch,
            "targetBranch": "main", "createdAt": 0, "updatedAt": 0, "latestReviewAt": 0,
            "mergeStatus": "succeeded", "reviewerVotes": [], "unresolvedThreads": 0,
            "url": format!("https://dev.azure.com/org/project/_git/repo/pullrequest/{id}")
        }))
        .unwrap()
    }

    fn context(project: &std::path::Path) -> PrContext {
        PrContext {
            project_root: project.to_owned(),
            repository: AzureRepoRef {
                org_url: "https://dev.azure.com/org".to_owned(),
                project: "project".to_owned(),
                repository: "repo".to_owned(),
            },
            current_user: None,
            auto_review: false,
            pull_requests: vec![
                PrItem {
                    pull_request: pull_request(42, "feature"),
                    attention: None,
                    worktree_path: Some(project.join("wt/feature")),
                    review_path: None,
                },
                PrItem {
                    pull_request: pull_request(44, "later"),
                    attention: None,
                    worktree_path: None,
                    review_path: Some(project.join("wt/pr-44")),
                },
            ],
        }
    }

    fn record(name: &str, project: &std::path::Path, cwd: &std::path::Path) -> SessionRecord {
        let mut record = SessionRecord::discovered("codex-1", PathBuf::from("/tmp/a.sock"));
        record.name = name.to_owned();
        record.project_root = Some(project.to_owned());
        record.cwd = Some(cwd.to_owned());
        record
    }

    #[test]
    fn only_a_review_session_name_carries_a_pr_number() {
        assert_eq!(review_pr_id("review: PR #42"), Some(42));
        assert_eq!(review_pr_id("review: PR #x"), None);
        assert_eq!(review_pr_id("PR #42"), None);
    }

    #[test]
    fn a_review_links_by_name_even_from_the_main_checkout() {
        let fixture = tempfile::tempdir().unwrap();
        let project = fixture.path();
        let context = context(project);
        let review = record("review: PR #44", project, project);
        assert_eq!(
            session_pr_from_context(&review, &context).map(|pr| pr.id),
            Some(44)
        );
    }

    #[test]
    fn a_review_with_a_custom_session_name_links_through_its_checkout() {
        let fixture = tempfile::tempdir().unwrap();
        let project = fixture.path();
        let context = context(project);
        let review = record("My review", project, &project.join("wt/pr-44"));
        assert_eq!(
            session_pr_from_context(&review, &context).map(|pr| pr.id),
            Some(44)
        );
    }

    #[test]
    fn other_sessions_link_through_their_worktree() {
        let fixture = tempfile::tempdir().unwrap();
        let project = fixture.path();
        std::fs::create_dir_all(project.join("wt/feature")).unwrap();
        let context = context(project);
        let on_branch = record("Codex 3", project, &project.join("wt/feature"));
        assert_eq!(
            session_pr_from_context(&on_branch, &context).map(|pr| pr.id),
            Some(42)
        );
        let elsewhere = record("Codex 4", project, project);
        assert_eq!(session_pr_from_context(&elsewhere, &context), None);
        let other_project = record(
            "Codex 5",
            &project.join("other"),
            &project.join("wt/feature"),
        );
        assert_eq!(session_pr_from_context(&other_project, &context), None);
    }
}
