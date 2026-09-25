use super::*;
use crate::changes::ChangeStatus;

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

        let launch_children = if self.launch.modal.is_visible() {
            vec![
                UiNode {
                    id: "launch-existing-worktree".to_owned(),
                    role: "radio".to_owned(),
                    label: Some("Existing worktree".to_owned()),
                    is_visible: self.launch.existing_worktree_mode.is_visible(),
                    is_enabled: self.launch.existing_worktree_radio.is_sensitive(),
                    is_selected: self.launch.existing_worktree_radio.is_active(),
                    bounds: widget_bounds(&self.launch.existing_worktree_radio, &self.window),
                    children: Vec::new(),
                },
                UiNode {
                    id: "launch-project-input".to_owned(),
                    role: "textbox".to_owned(),
                    label: Some("Project directory".to_owned()),
                    is_visible: gtk::prelude::WidgetExt::is_visible(&self.launch.project),
                    is_enabled: self.launch.project.is_sensitive(),
                    is_selected: self.launch.project.has_focus(),
                    bounds: widget_bounds(&self.launch.project, &self.window),
                    children: completion_nodes(
                        "launch-project",
                        &self.launch.project_completion_items.borrow(),
                    ),
                },
                UiNode {
                    id: "launch-worktree-input".to_owned(),
                    role: "textbox".to_owned(),
                    label: Some("Worktree".to_owned()),
                    is_visible: self.launch.existing_worktree_mode.is_visible(),
                    is_enabled: self.launch.worktree.is_sensitive(),
                    is_selected: self.launch.worktree.has_focus(),
                    bounds: widget_bounds(&self.launch.worktree, &self.window),
                    children: completion_nodes(
                        "launch-worktree",
                        &self.launch.worktree_completion_items.borrow(),
                    ),
                },
                UiNode {
                    id: "launch-new-worktree".to_owned(),
                    role: "radio".to_owned(),
                    label: Some("New worktree".to_owned()),
                    is_visible: self.launch.new_worktree_mode.is_visible(),
                    is_enabled: self.launch.new_worktree_radio.is_sensitive(),
                    is_selected: self.launch.new_worktree_radio.is_active(),
                    bounds: widget_bounds(&self.launch.new_worktree_radio, &self.window),
                    children: Vec::new(),
                },
                UiNode {
                    id: "launch-agent".to_owned(),
                    role: "group".to_owned(),
                    label: Some("Agent provider".to_owned()),
                    is_visible: self
                        .launch
                        .agent_buttons
                        .first()
                        .is_some_and(|(_, button)| button.is_visible()),
                    is_enabled: self
                        .launch
                        .agent_buttons
                        .first()
                        .is_some_and(|(_, button)| button.is_sensitive()),
                    is_selected: false,
                    bounds: self
                        .launch
                        .agent_buttons
                        .first()
                        .map(|(_, button)| widget_bounds(button, &self.window))
                        .unwrap_or_default(),
                    children: self
                        .launch
                        .agent_buttons
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
                    is_visible: self.launch.new_worktree_mode.is_visible(),
                    is_enabled: self.launch.branch.is_sensitive(),
                    is_selected: self.launch.branch.has_focus(),
                    bounds: widget_bounds(&self.launch.branch, &self.window),
                    children: Vec::new(),
                },
                UiNode {
                    id: "launch-base-branch".to_owned(),
                    role: "textbox".to_owned(),
                    label: Some("Base branch".to_owned()),
                    is_visible: self.launch.new_worktree_mode.is_visible(),
                    is_enabled: self.launch.base_branch.is_sensitive(),
                    is_selected: self.launch.base_branch.has_focus(),
                    bounds: widget_bounds(&self.launch.base_branch, &self.window),
                    children: Vec::new(),
                },
                UiNode {
                    id: "launch-cancel".to_owned(),
                    role: "button".to_owned(),
                    label: Some("Cancel".to_owned()),
                    is_visible: self.launch.cancel.is_visible(),
                    is_enabled: self.launch.cancel.is_sensitive(),
                    is_selected: false,
                    bounds: widget_bounds(&self.launch.cancel, &self.window),
                    children: Vec::new(),
                },
                UiNode {
                    id: "launch-submit".to_owned(),
                    role: "button".to_owned(),
                    label: Some("Launch".to_owned()),
                    is_visible: self.launch.submit.is_visible(),
                    is_enabled: self.launch.submit.is_sensitive(),
                    is_selected: false,
                    bounds: widget_bounds(&self.launch.submit, &self.window),
                    children: Vec::new(),
                },
            ]
        } else {
            Vec::new()
        };

        let visible_page = self.stack.visible_child_name().unwrap_or_default();
        let diff_visible = visible_page == "changes-diff";
        let active_tab = self.workspace_tabs.borrow().visible_tab().cloned();
        let tabs_context = self
            .workspace_tabs
            .borrow()
            .active_context()
            .map(str::to_owned);
        let tab_nodes = tabs_context
            .as_deref()
            .map(|context| self.workspace_tabs.borrow().context_tabs(context))
            .unwrap_or_default()
            .into_iter()
            .map(|tab| match &tab {
                crate::workspace_tabs::WorkspaceTabId::Session(id) => UiNode {
                    id: format!("session-tab-{id}"),
                    role: "tab".to_owned(),
                    label: self
                        .sessions
                        .borrow()
                        .get(id)
                        .map(|session| session.record.name.clone()),
                    is_visible: self.center_tabs.is_visible(),
                    is_enabled: self.sessions.borrow().contains_key(id),
                    is_selected: active_tab.as_ref() == Some(&tab),
                    bounds: widget_bounds(&self.center_tabs, &self.window),
                    children: Vec::new(),
                },
                crate::workspace_tabs::WorkspaceTabId::Diff(key) => {
                    let path = crate::changes::display_changed_path(&ChangedFile {
                        old_path: key.old_path.clone(),
                        new_path: key.new_path.clone(),
                        status: ChangeStatus::Unknown,
                        scope: key.scope.clone(),
                        old_mode: None,
                        new_mode: None,
                        old_blob: None,
                        new_blob: None,
                    });
                    UiNode {
                        id: crate::workspace_tabs::WorkspaceTabs::diff_tab_id(key),
                        role: "tab".to_owned(),
                        label: Some(format!("{path} ({:?})", key.scope)),
                        is_visible: self.center_tabs.is_visible(),
                        is_enabled: self.diff_tab_data.borrow().contains_key(key),
                        is_selected: active_tab.as_ref() == Some(&tab),
                        bounds: widget_bounds(&self.center_tabs, &self.window),
                        children: Vec::new(),
                    }
                }
            })
            .collect::<Vec<_>>();
        let workspace_tabs_node = UiNode {
            id: "workspace-tabs".to_owned(),
            role: "tablist".to_owned(),
            label: Some("Workspace tabs".to_owned()),
            is_visible: self.center_tabs.is_visible(),
            is_enabled: self.center_tabs.is_sensitive(),
            is_selected: !tab_nodes.is_empty(),
            bounds: widget_bounds(&self.center_tabs, &self.window),
            children: tab_nodes,
        };
        let mut changes_children = vec![
            UiNode {
                id: "changes-header".to_owned(),
                role: "heading".to_owned(),
                label: Some(self.changes.header.text().to_string()),
                is_visible: self.changes.header.is_visible(),
                is_enabled: self.changes.header.is_sensitive(),
                is_selected: false,
                bounds: widget_bounds(&self.changes.header, &self.window),
                children: Vec::new(),
            },
            UiNode {
                id: "changes-base".to_owned(),
                role: "combobox".to_owned(),
                label: self
                    .changes
                    .base
                    .selected_item()
                    .and_downcast::<gtk::StringObject>()
                    .map(|item| item.string().to_string()),
                is_visible: self.changes.base.is_visible(),
                is_enabled: self.changes.base.is_sensitive(),
                is_selected: false,
                bounds: widget_bounds(&self.changes.base, &self.window),
                children: Vec::new(),
            },
            UiNode {
                id: "changes-commits-ago".to_owned(),
                role: "spinbutton".to_owned(),
                label: Some(self.changes.comparison_hint.text().to_string()),
                is_visible: self.changes.commits_ago_row.is_visible(),
                is_enabled: self.changes.commits_ago.is_sensitive(),
                is_selected: false,
                bounds: widget_bounds(&self.changes.commits_ago, &self.window),
                children: Vec::new(),
            },
            UiNode {
                id: "changes-refresh".to_owned(),
                role: "button".to_owned(),
                label: Some("Refresh changes".to_owned()),
                is_visible: self.changes.refresh.is_visible(),
                is_enabled: self.changes.refresh.is_sensitive(),
                is_selected: false,
                bounds: widget_bounds(&self.changes.refresh, &self.window),
                children: Vec::new(),
            },
        ];
        changes_children.extend(changes_content_nodes(self));
        let changes_sidebar_node = UiNode {
            id: "changes-sidebar".to_owned(),
            role: "complementary".to_owned(),
            label: Some(self.changes.branch.text().to_string()),
            is_visible: self.changes.split.shows_sidebar(),
            is_enabled: self.changes.root.is_sensitive(),
            is_selected: self.changes.split.shows_sidebar(),
            bounds: widget_bounds(&self.changes.root, &self.window),
            children: changes_children,
        };
        let changes_toggle_node = UiNode {
            id: "changes-toggle".to_owned(),
            role: "toggle-button".to_owned(),
            label: Some("Changes".to_owned()),
            is_visible: self.changes.toggle.is_visible(),
            is_enabled: self.changes.toggle.is_sensitive(),
            is_selected: self.changes.toggle.is_active(),
            bounds: widget_bounds(&self.changes.toggle, &self.window),
            children: Vec::new(),
        };
        let current_diff_request = self.diff_current_request.borrow().clone();
        let rendered_diff = self.diff_rendered.borrow().clone();
        let diff_error = self.diff_error.borrow().clone();
        let diff_rendered = current_diff_request
            .as_ref()
            .and_then(|request| rendered_diff.filter(|(rendered, _, _)| rendered == request));
        let diff_label = if let Some((_, line_changes, character_changes)) = &diff_rendered {
            format!("rendered: {line_changes} line changes, {character_changes} character changes")
        } else if let Some((_, message)) = diff_error
            .filter(|(error_request, _)| current_diff_request.as_ref() == Some(error_request))
        {
            format!("error: {message}")
        } else if current_diff_request.is_some() {
            "rendering".to_owned()
        } else {
            self.diff_placeholder.text().to_string()
        };
        let diff_pane_node = UiNode {
            id: "diff-pane".to_owned(),
            role: "main".to_owned(),
            label: Some(format!(
                "{} · {}",
                self.diff_heading.text(),
                current_diff_scope_label(self)
            )),
            is_visible: diff_visible,
            is_enabled: self.diff_page.is_sensitive(),
            is_selected: diff_visible,
            bounds: widget_bounds(&self.diff_page, &self.window),
            children: vec![
                UiNode {
                    id: "diff-viewer".to_owned(),
                    role: "document".to_owned(),
                    label: Some(diff_label),
                    is_visible: diff_visible,
                    is_enabled: diff_rendered.is_some(),
                    is_selected: diff_visible,
                    bounds: widget_bounds(&self.diff_viewer_host, &self.window),
                    children: Vec::new(),
                },
                UiNode {
                    id: "diff-stale".to_owned(),
                    role: "status".to_owned(),
                    label: Some(self.diff_stale_label.text().to_string()),
                    is_visible: self.diff_stale_banner.is_visible(),
                    is_enabled: self.diff_reload_button.is_sensitive(),
                    is_selected: false,
                    bounds: widget_bounds(&self.diff_stale_banner, &self.window),
                    children: Vec::new(),
                },
            ],
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
                                        is_visible: self.menus.main_button.is_visible(),
                                        is_enabled: self.menus.shortcuts.is_enabled(),
                                        is_selected: self
                                            .preferences
                                            .is_showing(preferences_ui::SHORTCUTS_PAGE),
                                        bounds: widget_bounds(
                                            &self.menus.main_button,
                                            &self.window,
                                        ),
                                        children: Vec::new(),
                                    },
                                    UiNode {
                                        id: "appearance".to_owned(),
                                        role: "button".to_owned(),
                                        label: Some("Terminal appearance".to_owned()),
                                        is_visible: self.menus.main_button.is_visible(),
                                        is_enabled: self.menus.preferences.is_enabled(),
                                        is_selected: self
                                            .preferences
                                            .is_showing(preferences_ui::APPEARANCE_PAGE),
                                        bounds: widget_bounds(
                                            &self.menus.main_button,
                                            &self.window,
                                        ),
                                        children: Vec::new(),
                                    },
                                    UiNode {
                                        id: "launch".to_owned(),
                                        role: "button".to_owned(),
                                        label: Some("New".to_owned()),
                                        is_visible: self.launch_button.is_visible(),
                                        is_enabled: self.launch_button.is_sensitive(),
                                        is_selected: self.launch.modal.is_visible(),
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
                                        is_visible: self.menus.main_button.is_visible(),
                                        is_enabled: self.menus.worktrees.is_enabled(),
                                        is_selected: self.worktrees.modal.is_visible(),
                                        bounds: widget_bounds(
                                            &self.menus.main_button,
                                            &self.window,
                                        ),
                                        children: Vec::new(),
                                    },
                                    UiNode {
                                        id: "agents".to_owned(),
                                        role: "button".to_owned(),
                                        label: Some("Recent agent sessions".to_owned()),
                                        is_visible: self.menus.main_button.is_visible(),
                                        is_enabled: self.menus.agents.is_enabled(),
                                        is_selected: self.agents.modal.is_visible(),
                                        bounds: widget_bounds(
                                            &self.menus.main_button,
                                            &self.window,
                                        ),
                                        children: Vec::new(),
                                    },
                                    UiNode {
                                        id: "pull-requests".to_owned(),
                                        role: "button".to_owned(),
                                        label: Some("Azure pull requests".to_owned()),
                                        is_visible: self.menus.main_button.is_visible(),
                                        is_enabled: self.menus.pull_requests.is_enabled(),
                                        is_selected: self.prs.modal.is_visible(),
                                        bounds: widget_bounds(
                                            &self.menus.main_button,
                                            &self.window,
                                        ),
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
                                        is_selected: self.search_bar.is_search_mode(),
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
                        is_visible: self.selected_session_id().as_deref()
                            == Some(visible_page.as_str()),
                        is_enabled: self.stack.is_sensitive(),
                        is_selected: self.selected_session_id().as_deref()
                            == Some(visible_page.as_str()),
                        bounds: widget_bounds(&self.stack, &self.window),
                        children: {
                            let mut children = vec![UiNode {
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
                                        is_visible: self.menus.git_button.is_visible(),
                                        is_enabled: self.menus.branch_review.is_enabled(),
                                        is_selected: false,
                                        bounds: widget_bounds(&self.menus.git_button, &self.window),
                                        children: Vec::new(),
                                    },
                                    UiNode {
                                        id: "magit".to_owned(),
                                        role: "button".to_owned(),
                                        label: Some("Magit".to_owned()),
                                        is_visible: self.menus.git_button.is_visible(),
                                        is_enabled: self.menus.magit.is_enabled(),
                                        is_selected: false,
                                        bounds: widget_bounds(&self.menus.git_button, &self.window),
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
                                                label: Some(
                                                    self.context_last_input.text().to_string(),
                                                ),
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
                            }];
                            children.extend([
                                workspace_tabs_node,
                                changes_toggle_node,
                                diff_pane_node,
                                changes_sidebar_node,
                            ]);
                            children
                        },
                    },
                ],
            },
            is_truncated,
        }
    }
}

fn current_diff_scope_label(workspace: &Workspace) -> &'static str {
    match workspace.workspace_tabs.borrow().visible_tab() {
        Some(crate::workspace_tabs::WorkspaceTabId::Diff(key)) => match &key.scope {
            crate::changes::DiffScope::All => "All changes",
            crate::changes::DiffScope::Staged => "Staged",
            crate::changes::DiffScope::Unstaged => "Unstaged",
            crate::changes::DiffScope::Untracked => "Untracked",
            crate::changes::DiffScope::Committed => "Committed on branch",
            crate::changes::DiffScope::Commit { .. } => "Commit",
        },
        _ => "No diff",
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

fn changes_content_nodes(workspace: &Workspace) -> Vec<UiNode> {
    let visible = workspace.changes.split.shows_sidebar();
    let Some(snapshot) = workspace.changes.state.borrow().snapshot.clone() else {
        return Vec::new();
    };
    let groups = [
        ("all", "All changes", &snapshot.all_changes),
        ("staged", "Staged", &snapshot.staged),
        ("unstaged", "Unstaged", &snapshot.unstaged),
        ("untracked", "Untracked", &snapshot.untracked),
        ("conflicts", "Conflicts", &snapshot.conflicts),
        ("committed", "Committed on branch", &snapshot.branch_changes),
    ];
    let mut nodes = groups
        .into_iter()
        .map(|(key, label, files)| UiNode {
            id: format!("changes-{key}"),
            role: "group".to_owned(),
            label: Some(format!("{label} ({})", files.len())),
            is_visible: visible,
            is_enabled: true,
            is_selected: false,
            bounds: widget_bounds(&workspace.changes.rows, &workspace.window),
            children: files
                .iter()
                .enumerate()
                .map(|(index, file)| {
                    changed_file_node(
                        &format!("changes-{key}-file"),
                        index,
                        file,
                        visible,
                        widget_bounds(&workspace.changes.rows, &workspace.window),
                    )
                })
                .collect(),
        })
        .collect::<Vec<_>>();

    let loaded_commits = workspace.changes.state.borrow().loaded_commits.clone();
    nodes.push(UiNode {
        id: "changes-commits".to_owned(),
        role: "list".to_owned(),
        label: Some(format!("Commits ({})", snapshot.commits.len())),
        is_visible: visible,
        is_enabled: true,
        is_selected: false,
        bounds: widget_bounds(&workspace.changes.rows, &workspace.window),
        children: snapshot
            .commits
            .iter()
            .map(|commit| {
                let files = loaded_commits.get(&commit.oid);
                UiNode {
                    id: format!("changes-commit-{}", commit.oid),
                    role: "treeitem".to_owned(),
                    label: Some(commit.subject.clone()),
                    is_visible: visible,
                    is_enabled: true,
                    is_selected: false,
                    bounds: widget_bounds(&workspace.changes.rows, &workspace.window),
                    children: files
                        .into_iter()
                        .flatten()
                        .enumerate()
                        .map(|(index, file)| {
                            changed_file_node(
                                &format!("commit-{}-file", commit.oid),
                                index,
                                file,
                                visible,
                                widget_bounds(&workspace.changes.rows, &workspace.window),
                            )
                        })
                        .collect(),
                }
            })
            .collect(),
    });
    nodes
}

fn changed_file_node(
    prefix: &str,
    index: usize,
    file: &crate::changes::ChangedFile,
    visible: bool,
    bounds: Bounds,
) -> UiNode {
    let mut hasher = std::collections::hash_map::DefaultHasher::new();
    std::hash::Hash::hash(file, &mut hasher);
    UiNode {
        id: format!(
            "{prefix}-{index}-{:016x}",
            std::hash::Hasher::finish(&hasher)
        ),
        role: "button".to_owned(),
        label: Some(crate::changes::display_changed_path(file)),
        is_visible: visible,
        is_enabled: true,
        is_selected: false,
        bounds,
        children: Vec::new(),
    }
}
