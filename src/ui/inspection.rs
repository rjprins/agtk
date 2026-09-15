use super::*;

impl Workspace {
    pub(super) fn app_state(&self) -> AppState {
        let selected_session_id = self.selected_session_id();
        let mut sessions = self
            .sessions
            .borrow()
            .iter()
            .map(|(id, session)| SessionSummary {
                id: id.clone(),
                name: session.record.name.clone(),
                kind: session.record.kind,
                state: session.record.state,
                is_selected: selected_session_id.as_deref() == Some(id.as_str()),
                history_count: session.history.len(),
                cwd: session.record.cwd.clone(),
                project_root: session.record.project_root.clone(),
                worktree_path: session.record.worktree_path.clone(),
                created_at: session.record.created_at,
                position: session.record.position,
            })
            .collect::<Vec<_>>();
        sessions.sort_by(|left, right| (left.position, &left.id).cmp(&(right.position, &right.id)));

        AppState {
            instance: self.paths.name().as_str().to_owned(),
            protocol_version: PROTOCOL_VERSION,
            selected_session_id,
            appearance: self.appearance_summary(),
            shortcuts: self.shortcut_summaries(),
            window: WindowState {
                width: self.window.width(),
                height: self.window.height(),
            },
            projects: self.project_summaries(),
            worktree_groups: self.worktree_group_summaries(),
            sessions,
            attention: AttentionSummary {
                count: self
                    .sessions
                    .borrow()
                    .values()
                    .filter(|session| {
                        matches!(
                            session.record.state,
                            SessionState::Ready | SessionState::Waiting
                        )
                    })
                    .count()
                    + self.pr_attention_count(),
            },
        }
    }

    pub(super) fn session_summary(&self, id: &str) -> Result<SessionSummary, String> {
        let selected_session_id = self.selected_session_id();
        let sessions = self.sessions.borrow();
        let session = sessions
            .get(id)
            .ok_or_else(|| format!("session disappeared before it could be described: {id}"))?;
        Ok(SessionSummary {
            id: id.to_owned(),
            name: session.record.name.clone(),
            kind: session.record.kind,
            state: session.record.state,
            is_selected: selected_session_id.as_deref() == Some(id),
            history_count: session.history.len(),
            cwd: session.record.cwd.clone(),
            project_root: session.record.project_root.clone(),
            worktree_path: session.record.worktree_path.clone(),
            created_at: session.record.created_at,
            position: session.record.position,
        })
    }

    pub(super) fn ui_inspection(&self) -> UiInspection {
        let selected_session_id = self.selected_session_id();
        let sessions = self.sessions.borrow();
        let mut session_ids = sessions.keys().cloned().collect::<Vec<_>>();
        session_ids.sort();
        let is_truncated = session_ids.len() > MAX_INSPECTED_SESSIONS;

        let session_nodes = session_ids
            .into_iter()
            .take(MAX_INSPECTED_SESSIONS)
            .filter_map(|id| {
                let session = sessions.get(&id)?;
                Some(UiNode {
                    id: format!("session-{id}"),
                    role: "terminal-session".to_owned(),
                    label: Some(session.record.name.clone()),
                    is_visible: session.row.is_visible(),
                    is_enabled: session.row.is_sensitive(),
                    is_selected: selected_session_id.as_deref() == Some(id.as_str()),
                    bounds: widget_bounds(&session.row, &self.window),
                    children: vec![UiNode {
                        id: format!("session-state-{id}"),
                        role: "status".to_owned(),
                        label: Some(format!("{:?}", session.record.state).to_lowercase()),
                        is_visible: session.state_label.is_visible(),
                        is_enabled: true,
                        is_selected: false,
                        bounds: widget_bounds(&session.state_label, &self.window),
                        children: Vec::new(),
                    }],
                })
            })
            .collect();

        UiInspection {
            root: UiNode {
                id: "main-window".to_owned(),
                role: "window".to_owned(),
                label: Some("agmux native".to_owned()),
                is_visible: self.window.is_visible(),
                is_enabled: self.window.is_sensitive(),
                is_selected: false,
                bounds: Bounds {
                    x: 0.0,
                    y: 0.0,
                    width: self.window.width() as f32,
                    height: self.window.height() as f32,
                },
                children: vec![
                    UiNode {
                        id: "top-bar".to_owned(),
                        role: "toolbar".to_owned(),
                        label: Some("agmux-native".to_owned()),
                        is_visible: self.top_bar.is_visible(),
                        is_enabled: self.top_bar.is_sensitive(),
                        is_selected: false,
                        bounds: widget_bounds(&self.top_bar, &self.window),
                        children: vec![
                            UiNode {
                                id: "launch".to_owned(),
                                role: "button".to_owned(),
                                label: Some("Quick launch".to_owned()),
                                is_visible: self.launch_button.is_visible(),
                                is_enabled: self.launch_button.is_sensitive(),
                                is_selected: self.launch_popover.is_mapped(),
                                bounds: widget_bounds(&self.launch_button, &self.window),
                                children: Vec::new(),
                            },
                            UiNode {
                                id: "worktrees".to_owned(),
                                role: "button".to_owned(),
                                label: Some("Worktrees".to_owned()),
                                is_visible: self.worktree_button.is_visible(),
                                is_enabled: self.worktree_button.is_sensitive(),
                                is_selected: self.worktree_popover.is_mapped(),
                                bounds: widget_bounds(&self.worktree_button, &self.window),
                                children: Vec::new(),
                            },
                            UiNode {
                                id: "agents".to_owned(),
                                role: "button".to_owned(),
                                label: Some("Recent agent sessions".to_owned()),
                                is_visible: self.agent_button.is_visible(),
                                is_enabled: self.agent_button.is_sensitive(),
                                is_selected: self.agent_popover.is_mapped(),
                                bounds: widget_bounds(&self.agent_button, &self.window),
                                children: Vec::new(),
                            },
                            UiNode {
                                id: "pull-requests".to_owned(),
                                role: "button".to_owned(),
                                label: Some("Azure pull requests".to_owned()),
                                is_visible: self.pr_button.is_visible(),
                                is_enabled: self.pr_button.is_sensitive(),
                                is_selected: self.pr_popover.is_mapped(),
                                bounds: widget_bounds(&self.pr_button, &self.window),
                                children: Vec::new(),
                            },
                            UiNode {
                                id: "shortcuts".to_owned(),
                                role: "button".to_owned(),
                                label: Some("Application shortcuts".to_owned()),
                                is_visible: self.shortcut_button.is_visible(),
                                is_enabled: self.shortcut_button.is_sensitive(),
                                is_selected: self.shortcut_popover.is_mapped(),
                                bounds: widget_bounds(&self.shortcut_button, &self.window),
                                children: Vec::new(),
                            },
                            UiNode {
                                id: "appearance".to_owned(),
                                role: "button".to_owned(),
                                label: Some("Terminal appearance".to_owned()),
                                is_visible: self.theme_button.is_visible(),
                                is_enabled: self.theme_button.is_sensitive(),
                                is_selected: self.theme_popover.is_mapped(),
                                bounds: widget_bounds(&self.theme_button, &self.window),
                                children: Vec::new(),
                            },
                            UiNode {
                                id: "search".to_owned(),
                                role: "button".to_owned(),
                                label: Some("Search terminal".to_owned()),
                                is_visible: self.search_button.is_visible(),
                                is_enabled: self.search_button.is_sensitive(),
                                is_selected: self.search_popover.is_mapped(),
                                bounds: widget_bounds(&self.search_button, &self.window),
                                children: Vec::new(),
                            },
                            UiNode {
                                id: "history".to_owned(),
                                role: "button".to_owned(),
                                label: Some("History".to_owned()),
                                is_visible: self.history_button.is_visible(),
                                is_enabled: self.history_button.is_sensitive(),
                                is_selected: self.history_popover.is_mapped(),
                                bounds: widget_bounds(&self.history_button, &self.window),
                                children: Vec::new(),
                            },
                        ],
                    },
                    UiNode {
                        id: "sidebar".to_owned(),
                        role: "complementary".to_owned(),
                        label: Some("Sessions".to_owned()),
                        is_visible: self.sidebar_panel.is_visible(),
                        is_enabled: self.sidebar_panel.is_sensitive(),
                        is_selected: false,
                        bounds: widget_bounds(&self.sidebar_panel, &self.window),
                        children: vec![
                            UiNode {
                                id: "new-shell".to_owned(),
                                role: "button".to_owned(),
                                label: Some("New shell".to_owned()),
                                is_visible: self.new_shell_button.is_visible(),
                                is_enabled: self.new_shell_button.is_sensitive(),
                                is_selected: false,
                                bounds: widget_bounds(&self.new_shell_button, &self.window),
                                children: Vec::new(),
                            },
                            UiNode {
                                id: "git-actions".to_owned(),
                                role: "button".to_owned(),
                                label: Some("Emacs Git actions".to_owned()),
                                is_visible: self.git_button.is_visible(),
                                is_enabled: self.git_button.is_sensitive(),
                                is_selected: false,
                                bounds: widget_bounds(&self.git_button, &self.window),
                                children: Vec::new(),
                            },
                            UiNode {
                                id: "sessions".to_owned(),
                                role: "list".to_owned(),
                                label: Some("Sessions".to_owned()),
                                is_visible: self.list.is_visible(),
                                is_enabled: self.list.is_sensitive(),
                                is_selected: false,
                                bounds: widget_bounds(&self.list, &self.window),
                                children: session_nodes,
                            },
                        ],
                    },
                    UiNode {
                        id: "terminal-pane".to_owned(),
                        role: "main".to_owned(),
                        label: Some("Terminal".to_owned()),
                        is_visible: self.stack.is_visible(),
                        is_enabled: self.stack.is_sensitive(),
                        is_selected: true,
                        bounds: widget_bounds(&self.stack, &self.window),
                        children: Vec::new(),
                    },
                    UiNode {
                        id: "status-bar".to_owned(),
                        role: "status".to_owned(),
                        label: Some("new close sidebar previous next sessions".to_owned()),
                        is_visible: self.status_bar.is_visible(),
                        is_enabled: self.status_bar.is_sensitive(),
                        is_selected: false,
                        bounds: widget_bounds(&self.status_bar, &self.window),
                        children: Vec::new(),
                    },
                ],
            },
            is_truncated,
        }
    }
}
