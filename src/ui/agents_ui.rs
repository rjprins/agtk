use std::collections::{BTreeMap, BTreeSet};
use std::path::{Path, PathBuf};

use super::*;
use crate::control::{AgentListParams, AgentPreviewParams, AgentRestoreParams};
use crate::persist::now_millis;
use crate::providers::{
    AgentProvider, ConversationRole, DiscoveryRoots, ProviderDiscovery, ProviderPreview,
    ProviderSession, RestoreTarget,
};

const AGENT_FILTER_MIN_SESSIONS: usize = 6;

#[derive(Debug, Clone, PartialEq, Eq)]
pub(super) struct AgentSessionItem {
    session: ProviderSession,
    project_root: Option<PathBuf>,
}

#[derive(Debug, Clone, PartialEq, Eq)]
struct AgentSessionGroup {
    project_root: Option<PathBuf>,
    sessions: Vec<ProviderSession>,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub(super) enum AgentRestoreDestination {
    LastKnown,
    ExistingWorktree(PathBuf),
    NewWorktree,
    CustomDirectory,
}

fn filter_agent_sessions(
    sessions: &[AgentSessionItem],
    project_root: Option<&Path>,
    filter: &str,
) -> Vec<AgentSessionItem> {
    let filter = filter.trim().to_lowercase();
    sessions
        .iter()
        .filter(|item| {
            project_root.is_none_or(|root| item.project_root.as_deref() == Some(root))
                && (filter.is_empty()
                    || [
                        item.session.name.as_str(),
                        item.session.provider.command(),
                        item.session.provider_session_id.as_str(),
                        item.session
                            .cwd
                            .as_deref()
                            .and_then(Path::to_str)
                            .unwrap_or(""),
                        item.project_root
                            .as_deref()
                            .and_then(Path::to_str)
                            .unwrap_or(""),
                    ]
                    .into_iter()
                    .any(|field| field.to_lowercase().contains(&filter)))
        })
        .cloned()
        .collect()
}

fn group_agent_sessions(sessions: &[AgentSessionItem]) -> Vec<AgentSessionGroup> {
    let mut groups = Vec::<AgentSessionGroup>::new();
    for item in sessions {
        if let Some(group) = groups
            .iter_mut()
            .find(|group| group.project_root == item.project_root)
        {
            group.sessions.push(item.session.clone());
        } else {
            groups.push(AgentSessionGroup {
                project_root: item.project_root.clone(),
                sessions: vec![item.session.clone()],
            });
        }
    }
    groups.sort_by(
        |left, right| match (&left.project_root, &right.project_root) {
            (Some(left), Some(right)) => agent_project_name(left)
                .to_lowercase()
                .cmp(&agent_project_name(right).to_lowercase())
                .then_with(|| left.cmp(right)),
            (Some(_), None) => std::cmp::Ordering::Less,
            (None, Some(_)) => std::cmp::Ordering::Greater,
            (None, None) => std::cmp::Ordering::Equal,
        },
    );
    groups
}

fn classify_agent_sessions(
    sessions: Vec<ProviderSession>,
    known_project_roots: &[PathBuf],
    manager: &crate::worktrees::WorktreeManager,
) -> Vec<AgentSessionItem> {
    let mut resolved = BTreeMap::<PathBuf, PathBuf>::new();
    sessions
        .into_iter()
        .map(|session| {
            let project_root = session.cwd.as_ref().map(|cwd| {
                resolved
                    .entry(cwd.clone())
                    .or_insert_with(|| {
                        manager
                            .resolve_session_location(cwd, known_project_roots)
                            .project_root
                    })
                    .clone()
            });
            AgentSessionItem {
                session,
                project_root,
            }
        })
        .collect()
}

fn restore_target_for_location(
    cwd: Option<PathBuf>,
    known_project_roots: &[PathBuf],
    manager: &crate::worktrees::WorktreeManager,
) -> RestoreTarget {
    let Some(cwd) = cwd else {
        return RestoreTarget::default();
    };
    let location = manager.resolve_session_location(&cwd, known_project_roots);
    RestoreTarget {
        cwd: Some(location.cwd),
        project_root: Some(location.project_root),
        worktree_path: location.worktree_path,
        name: None,
    }
}

fn agent_project_name(root: &Path) -> String {
    root.file_name()
        .and_then(|name| name.to_str())
        .filter(|name| !name.is_empty())
        .unwrap_or_else(|| root.to_str().unwrap_or("Other locations"))
        .to_owned()
}

fn format_agent_elapsed(now: u64, last_seen_at: u64) -> String {
    if now < last_seen_at {
        return String::new();
    }
    let seconds = (now - last_seen_at) / 1_000;
    if seconds < 5 {
        "now".to_owned()
    } else if seconds < 60 {
        format!("{}s", seconds)
    } else if seconds < 3_600 {
        format!("{}m", seconds / 60)
    } else if seconds < 86_400 {
        format!("{}h", seconds / 3_600)
    } else {
        format!("{}d", seconds / 86_400)
    }
}

impl Workspace {
    pub(super) fn filter_agent_panel(&self) {
        let sessions = self.visible_agent_sessions();
        let selected = self.selected_agent.borrow().clone();
        if selected.as_ref().is_none_or(|current| {
            !sessions
                .iter()
                .any(|item| same_agent_session(item, current))
        }) {
            if let Some(session) = sessions.first() {
                self.preview_agent_for_panel(session.clone());
            } else {
                self.selected_agent.borrow_mut().take();
                self.agent_restore_button.set_sensitive(false);
                self.render_agent_preview_message("Select a Codex or Claude session");
                self.render_agent_sessions();
            }
            return;
        }
        self.render_agent_sessions();
    }

    pub(super) fn navigate_agent_selection(&self, delta: i32) {
        let sessions = self.visible_agent_sessions();
        if sessions.is_empty() {
            return;
        }
        let current = self.selected_agent.borrow().clone();
        let index = current
            .as_ref()
            .and_then(|selected| {
                sessions
                    .iter()
                    .position(|item| same_agent_session(item, selected))
            })
            .unwrap_or(0);
        let next = (index as i32 + delta).clamp(0, sessions.len() as i32 - 1) as usize;
        if let Some(session) = sessions.get(next) {
            self.preview_agent_for_panel(session.clone());
        }
    }

    fn visible_agent_sessions(&self) -> Vec<ProviderSession> {
        let sessions = self
            .agent_sessions
            .borrow()
            .iter()
            .filter(|item| {
                !self
                    .hidden_agent_sessions
                    .borrow()
                    .contains(&agent_session_key(&item.session))
            })
            .cloned()
            .collect::<Vec<_>>();
        filter_agent_sessions(
            &sessions,
            self.selected_agent_project().as_deref(),
            self.agent_filter.text().as_str(),
        )
        .into_iter()
        .map(|item| item.session)
        .collect()
    }

    pub(super) fn open_agent_for_project(&self, root: &str) {
        self.agent_filter.set_text("");
        self.set_agent_project_filter(Some(PathBuf::from(root)));
        if !self.agent_window.is_visible() {
            self.agent_window.present();
        } else {
            self.refresh_agent_panel();
        }
    }

    fn selected_agent_project(&self) -> Option<PathBuf> {
        self.agent_project_values
            .borrow()
            .get(self.agent_project_filter.selected() as usize)
            .cloned()
            .flatten()
    }

    fn set_agent_project_filter(&self, project_root: Option<PathBuf>) {
        self.rebuild_agent_project_choices(project_root);
        self.filter_agent_panel();
    }

    fn refresh_agent_project_choices(&self) {
        let selected = self.selected_agent_project();
        self.rebuild_agent_project_choices(selected);
    }

    fn rebuild_agent_project_choices(&self, selected: Option<PathBuf>) {
        let mut seen = BTreeSet::<PathBuf>::new();
        let mut roots = self
            .agent_sessions
            .borrow()
            .iter()
            .filter_map(|item| item.project_root.clone())
            .filter(|root| seen.insert(root.clone()))
            .collect::<Vec<_>>();
        if let Some(root) = selected.as_ref()
            && seen.insert(root.clone())
        {
            roots.push(root.clone());
        }
        roots.sort_by(|left, right| {
            agent_project_name(left)
                .to_lowercase()
                .cmp(&agent_project_name(right).to_lowercase())
                .then_with(|| left.cmp(right))
        });

        let mut labels = vec!["All projects".to_owned()];
        labels.extend(roots.iter().map(|root| agent_project_name(root)));
        let labels = labels.iter().map(String::as_str).collect::<Vec<_>>();
        self.agent_project_choices
            .splice(0, self.agent_project_choices.n_items(), &labels);

        let mut values = vec![None];
        values.extend(roots.into_iter().map(Some));
        let selected_index = selected
            .as_ref()
            .and_then(|selected| {
                values
                    .iter()
                    .position(|value| value.as_ref() == Some(selected))
            })
            .unwrap_or(0);
        *self.agent_project_values.borrow_mut() = values;
        self.agent_project_filter
            .set_selected(selected_index as u32);
    }

    pub(super) fn refresh_agent_panel(&self) {
        self.load_hidden_agent_sessions();
        self.render_agent_list_message("SCANNING LOCAL PROVIDER LOGS...");
        self.render_agent_preview_message("Select a Codex or Claude session");
        self.agent_count.set_text("Scanning...");
        self.agent_restore_button.set_sensitive(false);
        self.agent_restore_button.set_label("Restore session");
        self.selected_agent.borrow_mut().take();
        self.agent_sessions.borrow_mut().clear();
        let discovery = ProviderDiscovery::from_environment();
        let live = self.live_provider_sessions();
        let known_project_roots = self
            .project_summaries()
            .into_iter()
            .map(|project| PathBuf::from(project.root))
            .collect::<Vec<_>>();
        let manager = crate::worktrees::WorktreeManager::new(self.paths.attic_dir());
        self.run_io(
            move || {
                discovery.discover(now_millis(), &live).map(|sessions| {
                    classify_agent_sessions(sessions, &known_project_roots, &manager)
                })
            },
            |workspace, result| match result {
                Ok(sessions) => {
                    workspace.agent_sessions.replace(sessions);
                    workspace.refresh_agent_project_choices();
                    workspace.render_agent_sessions();
                    if let Some(first) = workspace.visible_agent_sessions().first().cloned() {
                        workspace.preview_agent_for_panel(first);
                    }
                }
                Err(error) => {
                    workspace.render_agent_list_message(&format!("ERROR  {error}"));
                    workspace.agent_count.set_text("Error");
                    workspace.show_error(&format!("Could not discover agent sessions: {error}"));
                }
            },
        );
    }

    pub(super) fn restore_agent_from_panel(&self) {
        let Some(session) = self.selected_agent.borrow().clone() else {
            self.show_error("Select an agent session first");
            return;
        };
        let index = self.agent_restore_destination.selected() as usize;
        let destination = self
            .agent_restore_destination_values
            .borrow()
            .get(index)
            .cloned()
            .unwrap_or(AgentRestoreDestination::LastKnown);
        let cwd = match destination {
            AgentRestoreDestination::LastKnown => session.cwd.clone(),
            AgentRestoreDestination::ExistingWorktree(path) => Some(path),
            AgentRestoreDestination::CustomDirectory => {
                let Some(cwd) = entry_path(&self.agent_restore_custom_cwd) else {
                    self.show_error("Enter a custom directory first");
                    return;
                };
                Some(cwd)
            }
            AgentRestoreDestination::NewWorktree => {
                let Some(cwd) = session.cwd.clone() else {
                    self.show_error("This session has no known location for a new worktree");
                    return;
                };
                let branch = self.agent_restore_branch.text().trim().to_owned();
                let branch = if branch.is_empty() {
                    format!("restore-{}", now_millis())
                } else {
                    branch
                };
                let purpose = format!("Restore agent session {}", session.name);
                let manager = crate::worktrees::WorktreeManager::new(self.paths.attic_dir());
                self.agent_restore_button.set_sensitive(false);
                self.agent_restore_button.set_label("Restoring...");
                self.agent_window.hide();
                self.run_io(
                    move || {
                        let root = manager.repository_root(&cwd)?;
                        let created = manager.create(&root, &branch, None, &purpose)?;
                        let path = created.path;
                        Ok(session.restore_plan(RestoreTarget {
                            cwd: Some(path.clone()),
                            project_root: Some(root),
                            worktree_path: Some(path),
                            ..RestoreTarget::default()
                        }))
                    },
                    |workspace, result| match result {
                        Ok(plan) => workspace.launch_controlled_with_conversation(
                            plan.params,
                            Some(plan.conversation_id),
                            None,
                        ),
                        Err(error) => workspace
                            .show_error(&format!("Could not create a restore worktree: {error}")),
                    },
                );
                return;
            }
        };
        let known_project_roots = self
            .project_summaries()
            .into_iter()
            .map(|project| PathBuf::from(project.root))
            .collect::<Vec<_>>();
        let manager = crate::worktrees::WorktreeManager::new(self.paths.attic_dir());
        self.agent_restore_button.set_sensitive(false);
        self.agent_restore_button.set_label("Restoring...");
        self.agent_window.hide();
        self.run_io(
            move || {
                let target = restore_target_for_location(cwd, &known_project_roots, &manager);
                Ok(session.restore_plan(target))
            },
            |workspace, result| match result {
                Ok(plan) => workspace.launch_controlled_with_conversation(
                    plan.params,
                    Some(plan.conversation_id),
                    None,
                ),
                Err(error) => {
                    workspace.show_error(&format!("Could not restore agent session: {error}"))
                }
            },
        );
    }

    pub(super) fn handle_agent_control(&self, pending: PendingRequest) {
        match pending.request.command.clone() {
            ControlCommand::AgentList(params) => self.list_agents(params, pending),
            ControlCommand::AgentPreview(params) => self.preview_agent(params, pending),
            ControlCommand::AgentRestore(params) => self.restore_agent(params, pending),
            ControlCommand::SessionSetState(params) => self.set_session_state(params, pending),
            _ => unreachable!("caller filters agent commands"),
        }
    }

    fn list_agents(&self, params: AgentListParams, pending: PendingRequest) {
        let discovery = ProviderDiscovery::new(
            DiscoveryRoots::from_environment(),
            params.limit as usize,
            u64::from(params.max_age_days) * 24 * 60 * 60 * 1_000,
        );
        let live = self.live_provider_sessions();
        self.run_io(
            move || discovery.discover(now_millis(), &live),
            move |workspace, result| {
                workspace.respond_agent_result(pending, "Could not discover agent sessions", result)
            },
        );
    }

    fn preview_agent(&self, params: AgentPreviewParams, pending: PendingRequest) {
        let discovery = ProviderDiscovery::from_environment();
        self.run_io(
            move || {
                discovery.preview(
                    params.provider,
                    &params.provider_session_id,
                    params.max_messages as usize,
                )
            },
            move |workspace, result| {
                workspace.respond_agent_result(pending, "Could not preview agent session", result)
            },
        );
    }

    fn restore_agent(&self, params: AgentRestoreParams, pending: PendingRequest) {
        let discovery = ProviderDiscovery::from_environment();
        let target = RestoreTarget {
            cwd: params.cwd,
            project_root: params.project_root,
            worktree_path: params.worktree_path,
            name: params.name,
        };
        self.run_io(
            move || {
                let preview = discovery.preview(params.provider, &params.provider_session_id, 1)?;
                Ok(preview.session.restore_plan(target))
            },
            move |workspace, result| match result {
                Ok(plan) => workspace.launch_controlled_with_conversation(
                    plan.params,
                    Some(plan.conversation_id),
                    Some(pending),
                ),
                Err(error) => workspace.report_failure(
                    Some(pending),
                    ErrorCode::OperationRefused,
                    "Could not restore agent session",
                    error.to_string(),
                ),
            },
        );
    }

    fn live_provider_sessions(&self) -> BTreeSet<(AgentProvider, String)> {
        self.sessions
            .borrow()
            .values()
            .filter(|session| session.record.state != SessionState::Exited)
            .filter_map(|session| {
                let provider = match session.record.kind {
                    SessionKind::Codex => AgentProvider::Codex,
                    SessionKind::Claude => AgentProvider::Claude,
                    _ => return None,
                };
                Some((provider, session.record.conversation_id.clone()?))
            })
            .collect()
    }

    fn render_agent_sessions(&self) {
        while let Some(child) = self.agent_list.first_child() {
            self.agent_list.remove(&child);
        }
        let all_sessions = self
            .agent_sessions
            .borrow()
            .iter()
            .filter(|item| {
                !self
                    .hidden_agent_sessions
                    .borrow()
                    .contains(&agent_session_key(&item.session))
            })
            .cloned()
            .collect::<Vec<_>>();
        let sessions = filter_agent_sessions(
            &all_sessions,
            self.selected_agent_project().as_deref(),
            self.agent_filter.text().as_str(),
        );
        self.agent_count
            .set_text(&format!("{} available", sessions.len()));
        self.agent_filter.set_visible(
            self.agent_sessions.borrow().len() >= AGENT_FILTER_MIN_SESSIONS
                || !self.agent_filter.text().is_empty(),
        );
        if sessions.is_empty() {
            self.render_agent_list_message(if all_sessions.is_empty() {
                "NO RECENT CODEX OR CLAUDE SESSIONS"
            } else {
                "NO SESSIONS MATCH THE FILTER"
            });
            return;
        }
        for group in group_agent_sessions(&sessions) {
            let heading = gtk::Box::new(gtk::Orientation::Horizontal, 6);
            heading.add_css_class("agent-project-heading");
            let name = group
                .project_root
                .as_deref()
                .map(agent_project_name)
                .unwrap_or_else(|| "Other locations".to_owned());
            let label = gtk::Label::new(Some(&name));
            label.set_xalign(0.0);
            label.set_hexpand(true);
            label.set_ellipsize(gtk::pango::EllipsizeMode::End);
            if let Some(root) = group.project_root.as_ref() {
                label.set_tooltip_text(Some(&root.to_string_lossy()));
            }
            heading.append(&label);
            let count = gtk::Label::new(Some(&group.sessions.len().to_string()));
            count.add_css_class("tui-muted");
            heading.append(&count);
            self.agent_list.append(&heading);
            for session in group.sessions {
                self.append_agent_session_row(session);
            }
        }
    }

    fn append_agent_session_row(&self, session: ProviderSession) {
        let row = gtk::Box::new(gtk::Orientation::Horizontal, 2);
        row.add_css_class("agent-session-row");
        let button = gtk::Button::new();
        button.add_css_class("tui-agent-row");
        if self
            .selected_agent
            .borrow()
            .as_ref()
            .is_some_and(|selected| same_agent_session(selected, &session))
        {
            button.add_css_class("agent-session-selected");
        }
        button.set_hexpand(true);
        let content = gtk::Box::new(gtk::Orientation::Vertical, 4);
        let title_row = gtk::Box::new(gtk::Orientation::Horizontal, 5);
        title_row.append(&provider_icons::agent_icon(session.provider));
        let title = gtk::Label::new(Some(&session.name));
        title.set_xalign(0.0);
        title.set_hexpand(true);
        title.set_ellipsize(gtk::pango::EllipsizeMode::End);
        title.add_css_class("agent-card-title");
        title_row.append(&title);
        let elapsed = gtk::Label::new(Some(&format_agent_elapsed(
            now_millis(),
            session.last_seen_at,
        )));
        elapsed.add_css_class("tui-muted");
        title_row.append(&elapsed);
        content.append(&title_row);
        let chips = gtk::Box::new(gtk::Orientation::Horizontal, 4);
        chips.add_css_class("agent-card-chips");
        append_agent_chip(&chips, session.provider.command());
        append_agent_chip(
            &chips,
            &short_agent_session_id(&session.provider_session_id),
        );
        if let Some(cwd) = &session.cwd {
            append_agent_chip(&chips, &cwd.to_string_lossy());
        }
        content.append(&chips);
        button.set_child(Some(&content));
        let preview_workspace = self.clone();
        let selected_session = session.clone();
        button.connect_clicked(move |_| {
            preview_workspace.preview_agent_for_panel(selected_session.clone())
        });
        row.append(&button);

        let hide = gtk::Button::with_label("×");
        hide.add_css_class("agent-session-hide");
        hide.set_tooltip_text(Some("Hide this session from the list"));
        let hide_workspace = self.clone();
        let hidden_session = session;
        hide.connect_clicked(move |_| {
            hide_workspace
                .hidden_agent_sessions
                .borrow_mut()
                .insert(agent_session_key(&hidden_session));
            hide_workspace.save_hidden_agent_sessions();
            if hide_workspace
                .selected_agent
                .borrow()
                .as_ref()
                .is_some_and(|selected| same_agent_session(selected, &hidden_session))
            {
                hide_workspace.selected_agent.borrow_mut().take();
                hide_workspace.render_agent_preview_message("Select a Codex or Claude session");
                hide_workspace.agent_restore_button.set_sensitive(false);
            }
            hide_workspace.render_agent_sessions();
        });
        row.append(&hide);
        self.agent_list.append(&row);
    }

    fn load_hidden_agent_sessions(&self) {
        if self.hidden_agent_sessions_loaded.replace(true) {
            return;
        }
        let Some(store) = self.store.borrow().clone() else {
            return;
        };
        self.run_io(
            move || store.preference("hiddenAgentSessions"),
            |workspace, result| {
                let Ok(Some(value)) = result else {
                    return;
                };
                let Some(values) = value.as_array() else {
                    return;
                };
                workspace.hidden_agent_sessions.borrow_mut().extend(
                    values
                        .iter()
                        .filter_map(|value| value.as_str().map(str::to_owned)),
                );
                workspace.render_agent_sessions();
            },
        );
    }

    fn save_hidden_agent_sessions(&self) {
        let mut values = self
            .hidden_agent_sessions
            .borrow()
            .iter()
            .cloned()
            .collect::<Vec<_>>();
        values.sort();
        self.save_preference("hiddenAgentSessions", serde_json::json!(values));
    }

    fn preview_agent_for_panel(&self, session: ProviderSession) {
        self.selected_agent.borrow_mut().replace(session.clone());
        self.agent_restore_branch
            .set_text(&format!("restore-{}", now_millis()));
        self.agent_restore_custom_cwd.set_text(
            session
                .cwd
                .as_ref()
                .map(|path| path.to_string_lossy())
                .as_deref()
                .unwrap_or(""),
        );
        self.reset_agent_destinations(&session);
        self.render_agent_sessions();
        self.agent_restore_button.set_sensitive(false);
        self.render_agent_preview_message("LOADING TRUSTED CONVERSATION PREVIEW...");
        let discovery = ProviderDiscovery::from_environment();
        let provider = session.provider;
        let provider_session_id = session.provider_session_id.clone();
        self.run_io(
            move || discovery.preview(provider, &provider_session_id, 12),
            |workspace, result| match result {
                Ok(preview) => workspace.render_agent_preview(preview),
                Err(error) => {
                    workspace.render_agent_preview_message(&format!("ERROR  {error}"));
                    workspace.show_error(&format!("Could not preview agent session: {error}"));
                }
            },
        );
        self.load_agent_worktrees(session);
    }

    fn render_agent_preview(&self, preview: ProviderPreview) {
        let is_current = self
            .selected_agent
            .borrow()
            .as_ref()
            .is_some_and(|session| {
                session.provider == preview.session.provider
                    && session.provider_session_id == preview.session.provider_session_id
            });
        if !is_current {
            return;
        }
        while let Some(child) = self.agent_preview.first_child() {
            self.agent_preview.remove(&child);
        }
        let elapsed = format_agent_elapsed(now_millis(), preview.session.last_seen_at);
        let title = gtk::Label::new(Some(&preview.session.name));
        title.set_xalign(0.0);
        title.set_wrap(true);
        title.set_wrap_mode(gtk::pango::WrapMode::WordChar);
        title.add_css_class("agent-detail-title");
        self.agent_preview.append(&title);
        let heading = gtk::Label::new(Some(&format!(
            "{}  {}{}{}",
            preview.session.provider.command(),
            short_agent_session_id(&preview.session.provider_session_id),
            if elapsed.is_empty() {
                "".to_owned()
            } else {
                format!("  {elapsed} ago")
            },
            if preview.is_truncated {
                "  [bounded]"
            } else {
                ""
            }
        )));
        heading.set_xalign(0.0);
        heading.add_css_class("tui-muted");
        self.agent_preview.append(&heading);
        if preview.is_truncated {
            let first_prompt =
                gtk::Label::new(Some(&format!("First prompt\n{}", preview.session.name)));
            first_prompt.set_xalign(0.0);
            first_prompt.set_wrap(true);
            first_prompt.set_wrap_mode(gtk::pango::WrapMode::WordChar);
            first_prompt.add_css_class("agent-first-prompt");
            self.agent_preview.append(&first_prompt);
        }
        let preview_heading = gtk::Label::new(Some("RECENT CONVERSATION"));
        preview_heading.set_xalign(0.0);
        preview_heading.add_css_class("agent-preview-label");
        self.agent_preview.append(&preview_heading);
        if preview.messages.is_empty() {
            let empty = gtk::Label::new(Some("No user or assistant messages found"));
            empty.set_xalign(0.0);
            self.agent_preview.append(&empty);
        }
        for message in preview.messages {
            let prefix = match message.role {
                ConversationRole::User => "YOU",
                ConversationRole::Assistant => "AGENT",
            };
            let label = gtk::Label::new(Some(&format!("{prefix}\n{}", message.text)));
            label.set_xalign(0.0);
            label.set_wrap(true);
            label.set_wrap_mode(gtk::pango::WrapMode::WordChar);
            label.set_selectable(true);
            label.add_css_class("agent-preview-message");
            label.add_css_class(match message.role {
                ConversationRole::User => "agent-preview-user",
                ConversationRole::Assistant => "agent-preview-assistant",
            });
            self.agent_preview.append(&label);
        }
        self.agent_restore_button.set_sensitive(true);
    }

    fn reset_agent_destinations(&self, session: &ProviderSession) {
        let item_count = self.agent_restore_destination_choices.n_items();
        self.agent_restore_destination_choices
            .splice(0, item_count, &[]);
        self.agent_restore_destination_values.borrow_mut().clear();
        let last_known = session
            .cwd
            .as_ref()
            .map(|path| format!("Last location — {}", compact_agent_path(path)));
        self.agent_restore_destination_choices
            .append(last_known.as_deref().unwrap_or("Last known location"));
        self.agent_restore_destination_values
            .borrow_mut()
            .push(AgentRestoreDestination::LastKnown);
        self.agent_restore_destination_choices
            .append("Create a new worktree...");
        self.agent_restore_destination_values
            .borrow_mut()
            .push(AgentRestoreDestination::NewWorktree);
        self.agent_restore_destination_choices
            .append("Choose a custom directory...");
        self.agent_restore_destination_values
            .borrow_mut()
            .push(AgentRestoreDestination::CustomDirectory);
        self.agent_restore_destination.set_selected(0);
        self.update_agent_destination_inputs();
    }

    fn load_agent_worktrees(&self, session: ProviderSession) {
        let Some(cwd) = session.cwd.clone() else {
            return;
        };
        let manager = crate::worktrees::WorktreeManager::new(self.paths.attic_dir());
        self.run_io(
            move || {
                let root = manager.repository_root(&cwd)?;
                manager.linked_paths(&root)
            },
            move |workspace, result| {
                let Some(selected) = workspace.selected_agent.borrow().clone() else {
                    return;
                };
                if !same_agent_session(&selected, &session) {
                    return;
                }
                let Ok(paths) = result else {
                    return;
                };
                let paths = paths
                    .into_iter()
                    .filter(|path| Some(path) != selected.cwd.as_ref())
                    .collect::<Vec<_>>();
                if paths.is_empty() {
                    return;
                }
                let labels = paths
                    .iter()
                    .map(|path| format!("Worktree — {}", compact_agent_path(path)))
                    .collect::<Vec<_>>();
                let labels = labels.iter().map(String::as_str).collect::<Vec<_>>();
                workspace
                    .agent_restore_destination_choices
                    .splice(1, 0, &labels);
                workspace
                    .agent_restore_destination_values
                    .borrow_mut()
                    .splice(
                        1..1,
                        paths
                            .into_iter()
                            .map(AgentRestoreDestination::ExistingWorktree),
                    );
            },
        );
    }

    pub(super) fn update_agent_destination_inputs(&self) {
        let destination = self
            .agent_restore_destination_values
            .borrow()
            .get(self.agent_restore_destination.selected() as usize)
            .cloned()
            .unwrap_or(AgentRestoreDestination::LastKnown);
        let show_branch = matches!(destination, AgentRestoreDestination::NewWorktree);
        let show_custom = matches!(destination, AgentRestoreDestination::CustomDirectory);
        if let Some(row) = self.agent_restore_branch.parent() {
            row.set_visible(show_branch);
        }
        if let Some(row) = self.agent_restore_custom_cwd.parent() {
            row.set_visible(show_custom);
        }
    }

    fn render_agent_list_message(&self, text: &str) {
        while let Some(child) = self.agent_list.first_child() {
            self.agent_list.remove(&child);
        }
        let label = gtk::Label::new(Some(text));
        label.set_xalign(0.0);
        label.add_css_class("tui-muted");
        self.agent_list.append(&label);
    }

    fn render_agent_preview_message(&self, text: &str) {
        while let Some(child) = self.agent_preview.first_child() {
            self.agent_preview.remove(&child);
        }
        let label = gtk::Label::new(Some(text));
        label.set_xalign(0.0);
        label.add_css_class("tui-muted");
        self.agent_preview.append(&label);
    }

    fn respond_agent_result<T: serde::Serialize>(
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

fn entry_path(entry: &gtk::Entry) -> Option<PathBuf> {
    let text = entry.text();
    let text = text.trim();
    (!text.is_empty()).then(|| PathBuf::from(text))
}

fn same_agent_session(left: &ProviderSession, right: &ProviderSession) -> bool {
    left.provider == right.provider && left.provider_session_id == right.provider_session_id
}

fn agent_session_key(session: &ProviderSession) -> String {
    format!(
        "{}:{}",
        session.provider.command(),
        session.provider_session_id
    )
}

fn short_agent_session_id(value: &str) -> String {
    value.chars().take(12).collect()
}

fn compact_agent_path(path: &Path) -> String {
    let value = path.to_string_lossy();
    if value.chars().count() <= 56 {
        return value.into_owned();
    }
    let suffix = value
        .chars()
        .rev()
        .take(53)
        .collect::<String>()
        .chars()
        .rev()
        .collect::<String>();
    format!("…{suffix}")
}

fn append_agent_chip(parent: &gtk::Box, text: &str) {
    let chip = gtk::Label::new(Some(text));
    chip.set_ellipsize(gtk::pango::EllipsizeMode::End);
    chip.set_max_width_chars(24);
    chip.add_css_class("agent-chip");
    parent.append(&chip);
}

#[cfg(test)]
mod tests {
    use super::{
        AgentSessionItem, classify_agent_sessions, filter_agent_sessions, format_agent_elapsed,
        group_agent_sessions, restore_target_for_location,
    };
    use crate::providers::{AgentProvider, ProviderSession};
    use crate::worktrees::WorktreeManager;
    use std::fs;
    use std::path::PathBuf;
    use std::process::Command;

    fn session(name: &str, provider_session_id: &str, cwd: &str) -> ProviderSession {
        ProviderSession {
            provider: AgentProvider::Codex,
            provider_session_id: provider_session_id.to_owned(),
            name: name.to_owned(),
            cwd: Some(PathBuf::from(cwd)),
            created_at: 1_000,
            last_seen_at: 2_000,
            log_path: PathBuf::from("/tmp/session.jsonl"),
        }
    }

    fn item(
        name: &str,
        provider_session_id: &str,
        cwd: &str,
        project_root: Option<&str>,
    ) -> AgentSessionItem {
        AgentSessionItem {
            session: session(name, provider_session_id, cwd),
            project_root: project_root.map(PathBuf::from),
        }
    }

    #[test]
    fn filtering_matches_original_session_card_fields() {
        let sessions = vec![
            item(
                "Review the launch flow",
                "codex-review",
                "/work/agmux-feature",
                Some("/work/agmux"),
            ),
            item(
                "Fix the terminal",
                "codex-terminal",
                "/work/other",
                Some("/work/other"),
            ),
        ];

        assert_eq!(filter_agent_sessions(&sessions, None, "launch").len(), 1);
        assert_eq!(filter_agent_sessions(&sessions, None, "terminal").len(), 1);
        assert_eq!(filter_agent_sessions(&sessions, None, "agmux").len(), 1);
        assert_eq!(
            filter_agent_sessions(&sessions, None, "codex-review").len(),
            1
        );
        assert_eq!(filter_agent_sessions(&sessions, None, "missing").len(), 0);
    }

    #[test]
    fn filtering_limits_results_to_the_selected_project() {
        let sessions = vec![
            item(
                "Review the launch flow",
                "codex-review",
                "/work/agmux-feature",
                Some("/work/agmux"),
            ),
            item(
                "Fix the terminal",
                "codex-terminal",
                "/work/other",
                Some("/work/other"),
            ),
        ];

        let filtered =
            filter_agent_sessions(&sessions, Some(PathBuf::from("/work/agmux").as_path()), "");

        assert_eq!(filtered.len(), 1);
        assert_eq!(filtered[0].session.provider_session_id, "codex-review");
    }

    #[test]
    fn grouping_sorts_projects_and_keeps_session_recency() {
        let sessions = vec![
            item(
                "Newest other session",
                "other-new",
                "/work/other",
                Some("/work/other"),
            ),
            item(
                "Newest agmux session",
                "agmux-new",
                "/work/agmux-feature",
                Some("/work/agmux"),
            ),
            item(
                "Older agmux session",
                "agmux-old",
                "/work/agmux-old",
                Some("/work/agmux"),
            ),
            item("Ungrouped", "unknown", "/tmp", None),
        ];

        let groups = group_agent_sessions(&sessions);

        assert_eq!(groups.len(), 3);
        assert_eq!(groups[0].project_root, Some(PathBuf::from("/work/agmux")));
        assert_eq!(groups[0].sessions[0].provider_session_id, "agmux-new");
        assert_eq!(groups[0].sessions[1].provider_session_id, "agmux-old");
        assert_eq!(groups[1].project_root, Some(PathBuf::from("/work/other")));
        assert_eq!(groups[2].project_root, None);
    }

    #[test]
    fn classification_prefers_the_most_specific_known_project_root() {
        let sessions = vec![session(
            "Nested session",
            "codex-nested",
            "/work/agmux/crates/native",
        )];
        let roots = vec![PathBuf::from("/work"), PathBuf::from("/work/agmux")];
        let manager = WorktreeManager::new(PathBuf::from("/tmp/unused-attic"));

        let classified = classify_agent_sessions(sessions, &roots, &manager);

        assert_eq!(
            classified[0].project_root,
            Some(PathBuf::from("/work/agmux"))
        );
    }

    #[test]
    fn elapsed_labels_use_original_short_units() {
        assert_eq!(format_agent_elapsed(1_000, 1_000), "now");
        assert_eq!(format_agent_elapsed(7_000, 1_000), "6s");
        assert_eq!(format_agent_elapsed(62_000, 1_000), "1m");
        assert_eq!(format_agent_elapsed(3_661_000, 1_000), "1h");
        assert_eq!(format_agent_elapsed(86_401_000, 1_000), "1d");
    }

    #[test]
    fn restored_git_locations_keep_project_and_worktree_metadata() {
        let fixture = tempfile::tempdir().unwrap();
        let root = fixture.path().join("project");
        let nested = root.join("src/deep");
        fs::create_dir_all(&nested).unwrap();
        assert!(
            Command::new("git")
                .args(["init", "-b", "main"])
                .current_dir(&root)
                .status()
                .unwrap()
                .success()
        );
        let manager = WorktreeManager::new(fixture.path().join("attic"));

        let target = restore_target_for_location(
            Some(nested.clone()),
            std::slice::from_ref(&root),
            &manager,
        );

        assert_eq!(target.cwd, Some(nested));
        assert_eq!(target.project_root, Some(root.clone()));
        assert_eq!(target.worktree_path, Some(root));
    }

    #[test]
    fn restored_non_git_locations_still_receive_project_metadata() {
        let fixture = tempfile::tempdir().unwrap();
        let root = fixture.path().join("project");
        let nested = root.join("src/deep");
        fs::create_dir_all(&nested).unwrap();
        let manager = WorktreeManager::new(fixture.path().join("attic"));

        let known = restore_target_for_location(
            Some(nested.clone()),
            std::slice::from_ref(&root),
            &manager,
        );
        let standalone = restore_target_for_location(Some(nested.clone()), &[], &manager);

        assert_eq!(known.project_root, Some(root));
        assert_eq!(known.worktree_path, None);
        assert_eq!(standalone.project_root, Some(nested));
        assert_eq!(standalone.worktree_path, None);
    }
}
