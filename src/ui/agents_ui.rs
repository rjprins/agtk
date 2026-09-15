use std::collections::BTreeSet;

use super::*;
use crate::control::{
    AgentListParams, AgentPreviewParams, AgentRestoreParams, AgentSignalState,
    SessionSetStateParams,
};
use crate::persist::now_millis;
use crate::providers::{
    AgentProvider, ConversationRole, DiscoveryRoots, ProviderDiscovery, ProviderPreview,
    ProviderSession, RestoreTarget,
};

impl Workspace {
    pub(super) fn open_agent_for_project(&self, root: &str) {
        self.agent_restore_project.set_text(root);
        if !self.agent_popover.is_mapped() {
            self.agent_popover.popup();
        } else {
            self.refresh_agent_panel();
        }
    }

    pub(super) fn refresh_agent_panel(&self) {
        self.render_agent_list_message("SCANNING LOCAL PROVIDER LOGS...");
        self.render_agent_preview_message("Select a Codex or Claude session");
        self.agent_restore_button.set_sensitive(false);
        self.selected_agent.borrow_mut().take();
        let discovery = ProviderDiscovery::from_environment();
        let live = self.live_provider_sessions();
        self.run_io(
            move || discovery.discover(now_millis(), &live),
            |workspace, result| match result {
                Ok(sessions) => workspace.render_agent_sessions(sessions),
                Err(error) => {
                    workspace.render_agent_list_message(&format!("ERROR  {error}"));
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
        let target = RestoreTarget {
            cwd: entry_path(&self.agent_restore_cwd),
            project_root: entry_path(&self.agent_restore_project),
            worktree_path: entry_path(&self.agent_restore_worktree),
            name: None,
        };
        let plan = session.restore_plan(target);
        self.agent_popover.popdown();
        self.launch_controlled_with_conversation(plan.params, Some(plan.conversation_id), None);
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

    fn set_session_state(&self, params: SessionSetStateParams, pending: PendingRequest) {
        let state = match params.state {
            AgentSignalState::Busy => SessionState::Busy,
            AgentSignalState::Ready => SessionState::Ready,
            AgentSignalState::Waiting => SessionState::Waiting,
        };
        let record = self
            .sessions
            .borrow()
            .get(&params.session_id)
            .map(|session| session.record.clone());
        let Some(mut record) = record else {
            let id = pending.request.id.clone();
            let _ = pending.respond(session_not_found(id, &params.session_id));
            return;
        };
        if record.state == SessionState::Exited {
            self.report_failure(
                Some(pending),
                ErrorCode::OperationRefused,
                "Could not update agent state",
                "session has exited".to_owned(),
            );
            return;
        }
        let Some(store) = self.store.borrow().clone() else {
            self.report_launch_failure(
                Some(pending),
                "Could not update agent state",
                "workspace is loading".to_owned(),
            );
            return;
        };
        record.state = state;
        let session_id = params.session_id;
        self.run_io(
            move || store.save_session(&record),
            move |workspace, result| match result {
                Ok(()) => {
                    workspace.apply_session_state(&session_id, state);
                    let response = workspace.session_summary(&session_id).and_then(|summary| {
                        serde_json::to_value(summary).map_err(|error| error.to_string())
                    });
                    match response {
                        Ok(response) => {
                            let id = pending.request.id.clone();
                            let _ = pending.respond(ControlResponse::success(id, response));
                        }
                        Err(error) => workspace.report_launch_failure(
                            Some(pending),
                            "Could not describe agent state",
                            error,
                        ),
                    }
                }
                Err(error) => workspace.report_launch_failure(
                    Some(pending),
                    "Could not save agent state",
                    error.to_string(),
                ),
            },
        );
    }

    pub(super) fn mark_agent_busy(&self, session_id: &str) {
        let record = {
            let mut sessions = self.sessions.borrow_mut();
            let Some(session) = sessions.get_mut(session_id) else {
                return;
            };
            if !matches!(
                session.record.kind,
                SessionKind::Codex | SessionKind::Claude | SessionKind::Gemini
            ) || session.record.state == SessionState::Exited
            {
                return;
            }
            session.record.state = SessionState::Busy;
            session.record.clone()
        };
        self.apply_session_state(session_id, SessionState::Busy);
        self.persist_record(record);
    }

    pub(super) fn apply_session_state(&self, session_id: &str, state: SessionState) {
        if let Some(session) = self.sessions.borrow_mut().get_mut(session_id) {
            session.record.state = state;
            session.state_label.set_text(session_state_indicator(state));
            session
                .state_label
                .set_tooltip_text(Some(session_state_name(state)));
            session.state_label.remove_css_class("tui-state-exited");
            if state == SessionState::Exited {
                session.state_label.add_css_class("tui-state-exited");
            }
        }
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

    fn render_agent_sessions(&self, sessions: Vec<ProviderSession>) {
        while let Some(child) = self.agent_list.first_child() {
            self.agent_list.remove(&child);
        }
        if sessions.is_empty() {
            self.render_agent_list_message("NO RECENT CODEX OR CLAUDE SESSIONS");
            return;
        }
        let first = sessions.first().cloned();
        for session in sessions {
            let button = gtk::Button::new();
            button.add_css_class("tui-agent-row");
            let content = gtk::Box::new(gtk::Orientation::Vertical, 1);
            let title_row = gtk::Box::new(gtk::Orientation::Horizontal, 5);
            title_row.append(&provider_icons::agent_icon(session.provider));
            let title = gtk::Label::new(Some(&session.name));
            title.set_xalign(0.0);
            title_row.append(&title);
            content.append(&title_row);
            let details = gtk::Label::new(Some(&format!(
                "{}  {}",
                session.provider_session_id,
                session
                    .cwd
                    .as_ref()
                    .map(|path| path.to_string_lossy().to_string())
                    .unwrap_or_else(|| "unknown cwd".to_owned())
            )));
            details.set_xalign(0.0);
            details.set_ellipsize(gtk::pango::EllipsizeMode::Middle);
            details.add_css_class("tui-muted");
            content.append(&details);
            button.set_child(Some(&content));
            let preview_workspace = self.clone();
            button.connect_clicked(move |_| {
                preview_workspace.preview_agent_for_panel(session.clone())
            });
            self.agent_list.append(&button);
        }
        if let Some(first) = first {
            self.preview_agent_for_panel(first);
        }
    }

    fn preview_agent_for_panel(&self, session: ProviderSession) {
        self.agent_restore_cwd.set_text(
            session
                .cwd
                .as_ref()
                .map(|path| path.to_string_lossy())
                .as_deref()
                .unwrap_or(""),
        );
        self.agent_restore_worktree.set_text("");
        self.agent_restore_button.set_sensitive(false);
        self.selected_agent.borrow_mut().replace(session.clone());
        self.render_agent_preview_message("LOADING TRUSTED CONVERSATION PREVIEW...");
        let discovery = ProviderDiscovery::from_environment();
        let provider = session.provider;
        let provider_session_id = session.provider_session_id.clone();
        self.run_io(
            move || discovery.preview(provider, &provider_session_id, 40),
            |workspace, result| match result {
                Ok(preview) => workspace.render_agent_preview(preview),
                Err(error) => {
                    workspace.render_agent_preview_message(&format!("ERROR  {error}"));
                    workspace.show_error(&format!("Could not preview agent session: {error}"));
                }
            },
        );
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
        let heading = gtk::Label::new(Some(&format!(
            "{}  {}{}",
            preview.session.provider.command(),
            preview.session.provider_session_id,
            if preview.is_truncated {
                "  [bounded]"
            } else {
                ""
            }
        )));
        heading.set_xalign(0.0);
        heading.add_css_class("tui-muted");
        self.agent_preview.append(&heading);
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
            let label = gtk::Label::new(Some(&format!("{prefix}> {}", message.text)));
            label.set_xalign(0.0);
            label.set_wrap(true);
            label.set_wrap_mode(gtk::pango::WrapMode::WordChar);
            label.set_selectable(true);
            label.add_css_class("tui-agent-message");
            self.agent_preview.append(&label);
        }
        self.agent_restore_button.set_sensitive(true);
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

pub(super) const fn session_state_indicator(state: SessionState) -> &'static str {
    match state {
        SessionState::Running => "*",
        SessionState::Busy => "~",
        SessionState::Ready => "!",
        SessionState::Waiting => "?",
        SessionState::Exited => "x",
        SessionState::Reconnecting => ".",
    }
}

pub(super) const fn session_state_name(state: SessionState) -> &'static str {
    match state {
        SessionState::Running => "Running",
        SessionState::Busy => "Busy",
        SessionState::Ready => "Ready",
        SessionState::Waiting => "Waiting for input",
        SessionState::Exited => "Exited",
        SessionState::Reconnecting => "Reconnecting",
    }
}
