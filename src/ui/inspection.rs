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
                state_changed_at: session.record.state_changed_at,
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
            state_changed_at: session.record.state_changed_at,
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

        let launch_children = if self.launch_window.is_visible() {
            vec![
                UiNode {
                    id: "launch-existing-worktree".to_owned(),
                    role: "radio".to_owned(),
                    label: Some("Existing worktree".to_owned()),
                    is_visible: self.launch_existing_worktree_mode.is_visible(),
                    is_enabled: self.launch_existing_worktree_radio.is_sensitive(),
                    is_selected: self.launch_existing_worktree_radio.is_active(),
                    bounds: widget_bounds(&self.launch_existing_worktree_radio, &self.window),
                    children: Vec::new(),
                },
                UiNode {
                    id: "launch-project-input".to_owned(),
                    role: "textbox".to_owned(),
                    label: Some("Project directory".to_owned()),
                    is_visible: gtk::prelude::WidgetExt::is_visible(&self.launch_project),
                    is_enabled: self.launch_project.is_sensitive(),
                    is_selected: self.launch_project.has_focus(),
                    bounds: widget_bounds(&self.launch_project, &self.window),
                    children: completion_nodes(
                        "launch-project",
                        &self.launch_project_completion_items.borrow(),
                    ),
                },
                UiNode {
                    id: "launch-worktree-input".to_owned(),
                    role: "textbox".to_owned(),
                    label: Some("Worktree".to_owned()),
                    is_visible: self.launch_existing_worktree_mode.is_visible(),
                    is_enabled: self.launch_worktree.is_sensitive(),
                    is_selected: self.launch_worktree.has_focus(),
                    bounds: widget_bounds(&self.launch_worktree, &self.window),
                    children: completion_nodes(
                        "launch-worktree",
                        &self.launch_worktree_completion_items.borrow(),
                    ),
                },
                UiNode {
                    id: "launch-new-worktree".to_owned(),
                    role: "radio".to_owned(),
                    label: Some("New worktree".to_owned()),
                    is_visible: self.launch_new_worktree_mode.is_visible(),
                    is_enabled: self.launch_new_worktree_radio.is_sensitive(),
                    is_selected: self.launch_new_worktree_radio.is_active(),
                    bounds: widget_bounds(&self.launch_new_worktree_radio, &self.window),
                    children: Vec::new(),
                },
                UiNode {
                    id: "launch-agent".to_owned(),
                    role: "group".to_owned(),
                    label: Some("Agent provider".to_owned()),
                    is_visible: self
                        .launch_agent_buttons
                        .first()
                        .is_some_and(|(_, button)| button.is_visible()),
                    is_enabled: self
                        .launch_agent_buttons
                        .first()
                        .is_some_and(|(_, button)| button.is_sensitive()),
                    is_selected: false,
                    bounds: self
                        .launch_agent_buttons
                        .first()
                        .map(|(_, button)| widget_bounds(button, &self.window))
                        .unwrap_or_default(),
                    children: self
                        .launch_agent_buttons
                        .iter()
                        .map(|(kind, button)| UiNode {
                            id: format!("launch-agent-{}", session_kind_name(*kind)),
                            role: "button".to_owned(),
                            label: button.label().map(|label| label.to_string()),
                            is_visible: button.is_visible(),
                            is_enabled: button.is_sensitive(),
                            is_selected: button.is_active(),
                            bounds: widget_bounds(button, &self.window),
                            children: Vec::new(),
                        })
                        .collect(),
                },
                UiNode {
                    id: "launch-branch".to_owned(),
                    role: "textbox".to_owned(),
                    label: Some("New worktree name".to_owned()),
                    is_visible: self.launch_new_worktree_mode.is_visible(),
                    is_enabled: self.launch_branch.is_sensitive(),
                    is_selected: self.launch_branch.has_focus(),
                    bounds: widget_bounds(&self.launch_branch, &self.window),
                    children: Vec::new(),
                },
                UiNode {
                    id: "launch-base-branch".to_owned(),
                    role: "textbox".to_owned(),
                    label: Some("Base branch".to_owned()),
                    is_visible: self.launch_new_worktree_mode.is_visible(),
                    is_enabled: self.launch_base_branch.is_sensitive(),
                    is_selected: self.launch_base_branch.has_focus(),
                    bounds: widget_bounds(&self.launch_base_branch, &self.window),
                    children: Vec::new(),
                },
                UiNode {
                    id: "launch-cancel".to_owned(),
                    role: "button".to_owned(),
                    label: Some("Cancel".to_owned()),
                    is_visible: self.launch_cancel.is_visible(),
                    is_enabled: self.launch_cancel.is_sensitive(),
                    is_selected: false,
                    bounds: widget_bounds(&self.launch_cancel, &self.window),
                    children: Vec::new(),
                },
                UiNode {
                    id: "launch-submit".to_owned(),
                    role: "button".to_owned(),
                    label: Some("Launch".to_owned()),
                    is_visible: self.launch_submit.is_visible(),
                    is_enabled: self.launch_submit.is_sensitive(),
                    is_selected: false,
                    bounds: widget_bounds(&self.launch_submit, &self.window),
                    children: Vec::new(),
                },
            ]
        } else {
            Vec::new()
        };

        UiInspection {
            root: UiNode {
                id: "main-window".to_owned(),
                role: "window".to_owned(),
                label: Some("agmux".to_owned()),
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
                        id: "sidebar".to_owned(),
                        role: "complementary".to_owned(),
                        label: Some("agmux".to_owned()),
                        is_visible: self.sidebar_panel.is_visible(),
                        is_enabled: self.sidebar_panel.is_sensitive(),
                        is_selected: false,
                        bounds: widget_bounds(&self.sidebar_panel, &self.window),
                        children: vec![
                            UiNode {
                                id: "brand".to_owned(),
                                role: "heading".to_owned(),
                                label: Some("agmux".to_owned()),
                                is_visible: self.sidebar_header.is_visible(),
                                is_enabled: self.sidebar_header.is_sensitive(),
                                is_selected: false,
                                bounds: widget_bounds(&self.sidebar_header, &self.window),
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
                            UiNode {
                                id: "sidebar-controls".to_owned(),
                                role: "toolbar".to_owned(),
                                label: Some("Sidebar controls".to_owned()),
                                is_visible: self.sidebar_controls.is_visible(),
                                is_enabled: self.sidebar_controls.is_sensitive(),
                                is_selected: false,
                                bounds: widget_bounds(&self.sidebar_controls, &self.window),
                                children: vec![
                                    UiNode {
                                        id: "shortcuts".to_owned(),
                                        role: "button".to_owned(),
                                        label: Some("Application shortcuts".to_owned()),
                                        is_visible: self.shortcut_button.is_visible(),
                                        is_enabled: self.shortcut_button.is_sensitive(),
                                        is_selected: self.shortcut_window.is_visible(),
                                        bounds: widget_bounds(&self.shortcut_button, &self.window),
                                        children: Vec::new(),
                                    },
                                    UiNode {
                                        id: "appearance".to_owned(),
                                        role: "button".to_owned(),
                                        label: Some("Terminal appearance".to_owned()),
                                        is_visible: self.theme_button.is_visible(),
                                        is_enabled: self.theme_button.is_sensitive(),
                                        is_selected: self.theme_window.is_visible(),
                                        bounds: widget_bounds(&self.theme_button, &self.window),
                                        children: Vec::new(),
                                    },
                                    UiNode {
                                        id: "launch".to_owned(),
                                        role: "button".to_owned(),
                                        label: Some("New".to_owned()),
                                        is_visible: self.launch_button.is_visible(),
                                        is_enabled: self.launch_button.is_sensitive(),
                                        is_selected: self.launch_window.is_visible(),
                                        bounds: widget_bounds(&self.launch_button, &self.window),
                                        children: launch_children,
                                    },
                                    UiNode {
                                        id: "sidebar-toggle".to_owned(),
                                        role: "button".to_owned(),
                                        label: Some(
                                            if self.sidebar_panel.is_visible() {
                                                "Collapse sidebar"
                                            } else {
                                                "Expand sidebar"
                                            }
                                            .to_owned(),
                                        ),
                                        is_visible: self.sidebar_toggle.is_visible(),
                                        is_enabled: self.sidebar_toggle.is_sensitive(),
                                        is_selected: false,
                                        bounds: widget_bounds(&self.sidebar_toggle, &self.window),
                                        children: Vec::new(),
                                    },
                                ],
                            },
                            UiNode {
                                id: "surface-anchors".to_owned(),
                                role: "group".to_owned(),
                                label: Some("Additional surfaces".to_owned()),
                                is_visible: self.surface_anchors.is_visible(),
                                is_enabled: self.surface_anchors.is_sensitive(),
                                is_selected: false,
                                bounds: widget_bounds(&self.surface_anchors, &self.window),
                                children: vec![
                                    UiNode {
                                        id: "worktrees".to_owned(),
                                        role: "button".to_owned(),
                                        label: Some("Worktrees".to_owned()),
                                        is_visible: self.worktree_button.is_visible(),
                                        is_enabled: self.worktree_button.is_sensitive(),
                                        is_selected: self.worktree_window.is_visible(),
                                        bounds: widget_bounds(&self.worktree_button, &self.window),
                                        children: Vec::new(),
                                    },
                                    UiNode {
                                        id: "agents".to_owned(),
                                        role: "button".to_owned(),
                                        label: Some("Recent agent sessions".to_owned()),
                                        is_visible: self.agent_button.is_visible(),
                                        is_enabled: self.agent_button.is_sensitive(),
                                        is_selected: self.agent_window.is_visible(),
                                        bounds: widget_bounds(&self.agent_button, &self.window),
                                        children: Vec::new(),
                                    },
                                    UiNode {
                                        id: "pull-requests".to_owned(),
                                        role: "button".to_owned(),
                                        label: Some("Azure pull requests".to_owned()),
                                        is_visible: self.pr_button.is_visible(),
                                        is_enabled: self.pr_button.is_sensitive(),
                                        is_selected: self.pr_window.is_visible(),
                                        bounds: widget_bounds(&self.pr_button, &self.window),
                                        children: Vec::new(),
                                    },
                                    UiNode {
                                        id: "claude-model".to_owned(),
                                        role: "button".to_owned(),
                                        label: Some("Claude model presets".to_owned()),
                                        is_visible: self.claude_model_button.is_visible(),
                                        is_enabled: self.claude_model_button.is_sensitive(),
                                        is_selected: self.claude_model_window.is_visible(),
                                        bounds: widget_bounds(
                                            &self.claude_model_button,
                                            &self.window,
                                        ),
                                        children: Vec::new(),
                                    },
                                    UiNode {
                                        id: "search".to_owned(),
                                        role: "button".to_owned(),
                                        label: Some("Search terminal".to_owned()),
                                        is_visible: self.search_button.is_visible(),
                                        is_enabled: self.search_button.is_sensitive(),
                                        is_selected: self.search_window.is_visible(),
                                        bounds: widget_bounds(&self.search_button, &self.window),
                                        children: Vec::new(),
                                    },
                                ],
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
                        children: vec![UiNode {
                            id: "session-context".to_owned(),
                            role: "toolbar".to_owned(),
                            label: Some("Last input and history".to_owned()),
                            is_visible: self.session_context_bar.is_visible(),
                            is_enabled: self.session_context_bar.is_sensitive(),
                            is_selected: false,
                            bounds: widget_bounds(&self.session_context_bar, &self.window),
                            children: vec![
                                UiNode {
                                    id: "branch-review".to_owned(),
                                    role: "button".to_owned(),
                                    label: Some("Review".to_owned()),
                                    is_visible: self.context_branch_review.is_visible(),
                                    is_enabled: self.context_branch_review.is_sensitive(),
                                    is_selected: false,
                                    bounds: widget_bounds(
                                        &self.context_branch_review,
                                        &self.window,
                                    ),
                                    children: Vec::new(),
                                },
                                UiNode {
                                    id: "magit".to_owned(),
                                    role: "button".to_owned(),
                                    label: Some("Magit".to_owned()),
                                    is_visible: self.context_magit.is_visible(),
                                    is_enabled: self.context_magit.is_sensitive(),
                                    is_selected: false,
                                    bounds: widget_bounds(&self.context_magit, &self.window),
                                    children: Vec::new(),
                                },
                                UiNode {
                                    id: "context-input".to_owned(),
                                    role: "group".to_owned(),
                                    label: Some("Last input".to_owned()),
                                    is_visible: self.context_input.is_visible(),
                                    is_enabled: self.context_input.is_sensitive(),
                                    is_selected: false,
                                    bounds: widget_bounds(&self.context_input, &self.window),
                                    children: vec![
                                        UiNode {
                                            id: "last-input".to_owned(),
                                            role: "label".to_owned(),
                                            label: Some(self.context_last_input.text().to_string()),
                                            is_visible: self.context_last_input.is_visible(),
                                            is_enabled: true,
                                            is_selected: false,
                                            bounds: widget_bounds(
                                                &self.context_last_input,
                                                &self.window,
                                            ),
                                            children: Vec::new(),
                                        },
                                        UiNode {
                                            id: "history".to_owned(),
                                            role: "button".to_owned(),
                                            label: self
                                                .history_button
                                                .label()
                                                .map(|label| label.to_string()),
                                            is_visible: self.history_button.is_visible(),
                                            is_enabled: self.history_button.is_sensitive(),
                                            is_selected: self.history_window.is_visible(),
                                            bounds: widget_bounds(
                                                &self.history_button,
                                                &self.window,
                                            ),
                                            children: Vec::new(),
                                        },
                                    ],
                                },
                                UiNode {
                                    id: "pr-context".to_owned(),
                                    role: "status".to_owned(),
                                    label: self.selected_pr.borrow().as_ref().map(|context| {
                                        format!(
                                            "PR #{}: {}",
                                            context.pull_request.id, context.pull_request.title
                                        )
                                    }),
                                    is_visible: self.context_pr.is_visible(),
                                    is_enabled: true,
                                    is_selected: false,
                                    bounds: widget_bounds(&self.context_pr, &self.window),
                                    children: Vec::new(),
                                },
                            ],
                        }],
                    },
                ],
            },
            is_truncated,
        }
    }
}

/// Suggestions only render while typing, so expose them for inspection.
fn completion_nodes(prefix: &str, items: &[(String, String)]) -> Vec<UiNode> {
    items
        .iter()
        .enumerate()
        .map(|(index, (label, _))| UiNode {
            id: format!("{prefix}-option-{index}"),
            role: "option".to_owned(),
            label: Some(label.clone()),
            is_visible: false,
            is_enabled: true,
            is_selected: false,
            bounds: Bounds::default(),
            children: Vec::new(),
        })
        .collect()
}
