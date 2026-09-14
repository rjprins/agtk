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
                name: session.name.clone(),
                kind: session.kind,
                state: SessionState::Running,
                is_selected: selected_session_id.as_deref() == Some(id.as_str()),
                history_count: session.history.len(),
            })
            .collect::<Vec<_>>();
        sessions.sort_by(|left, right| left.id.cmp(&right.id));

        AppState {
            instance: self.paths.name().as_str().to_owned(),
            protocol_version: PROTOCOL_VERSION,
            selected_session_id,
            window: WindowState {
                width: self.window.width(),
                height: self.window.height(),
            },
            projects: Vec::new(),
            worktree_groups: Vec::new(),
            sessions,
            attention: AttentionSummary { count: 0 },
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
            name: session.name.clone(),
            kind: session.kind,
            state: SessionState::Running,
            is_selected: selected_session_id.as_deref() == Some(id),
            history_count: session.history.len(),
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
                    label: Some(session.name.clone()),
                    is_visible: session.row.is_visible(),
                    is_enabled: session.row.is_sensitive(),
                    is_selected: selected_session_id.as_deref() == Some(id.as_str()),
                    bounds: widget_bounds(&session.row, &self.window),
                    children: Vec::new(),
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
                        id: "history".to_owned(),
                        role: "button".to_owned(),
                        label: Some("History".to_owned()),
                        is_visible: self.history_button.is_visible(),
                        is_enabled: self.history_button.is_sensitive(),
                        is_selected: self.history_popover.is_visible(),
                        bounds: widget_bounds(&self.history_button, &self.window),
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
            is_truncated,
        }
    }
}
