use super::*;

impl Workspace {
    pub(super) fn start_control_server(&self) {
        let socket_path = self.paths.control_socket();
        let (server, requests) = match ControlServer::bind(&socket_path) {
            Ok(bound) => bound,
            Err(error) => {
                self.show_error(&format!("Could not start control socket: {error}"));
                return;
            }
        };
        self.control_server.borrow_mut().replace(server);

        let workspace = self.clone();
        // GLib documents timeout_add_local as scheduling on the default main loop.
        // Source: https://gtk-rs.org/gtk-rs-core/stable/latest/docs/glib/source/fn.timeout_add_local.html
        glib::timeout_add_local(Duration::from_millis(10), move || {
            for pending in requests.try_iter() {
                workspace.handle_control_request(pending);
            }
            glib::ControlFlow::Continue
        });
    }

    pub(super) fn handle_control_request(&self, pending: PendingRequest) {
        if let ControlCommand::SessionCreate(params) = &pending.request.command {
            self.launch_controlled_session(params.clone(), pending);
            return;
        }
        if matches!(&pending.request.command, ControlCommand::UiCapture) {
            capture::capture_controlled(self.clone(), pending);
            return;
        }
        if let ControlCommand::AppearanceSet(params) = &pending.request.command {
            self.set_appearance(params.clone(), Some(pending));
            return;
        }
        if let ControlCommand::ShortcutSet(params) = &pending.request.command {
            self.set_shortcut(params.clone(), Some(pending));
            return;
        }
        if let ControlCommand::ProjectSet(params) = &pending.request.command {
            self.set_project_preferences(
                params.root.to_string_lossy().to_string(),
                params.is_pinned,
                params.is_collapsed,
                Some(pending),
            );
            return;
        }
        if matches!(
            &pending.request.command,
            ControlCommand::WorktreeList(_)
                | ControlCommand::WorktreeCreate(_)
                | ControlCommand::WorktreeReap(_)
        ) {
            self.handle_worktree_control(pending);
            return;
        }
        if matches!(
            &pending.request.command,
            ControlCommand::ClaudePresetsSet(_) | ControlCommand::ClaudePresetApply(_)
        ) {
            self.handle_claude_control(pending);
            return;
        }
        if matches!(
            &pending.request.command,
            ControlCommand::PrList(_)
                | ControlCommand::PrAcknowledge(_)
                | ControlCommand::PrSetAutoReview(_)
                | ControlCommand::PrLaunchReview(_)
        ) {
            self.handle_pr_control(pending);
            return;
        }
        let emacs_action = match &pending.request.command {
            ControlCommand::SessionOpenMagit(params) => {
                Some((params.session_id.clone(), crate::emacs::EmacsAction::Magit))
            }
            ControlCommand::SessionOpenBranchReview(params) => Some((
                params.session_id.clone(),
                crate::emacs::EmacsAction::BranchReview,
            )),
            _ => None,
        };
        if let Some((session_id, action)) = emacs_action {
            if self.sessions.borrow().contains_key(&session_id) {
                self.open_session_in_emacs(&session_id, action, Some(pending));
            } else {
                let id = pending.request.id.clone();
                let _ = pending.respond(session_not_found(id, &session_id));
            }
            return;
        }
        if matches!(
            &pending.request.command,
            ControlCommand::AgentList(_)
                | ControlCommand::AgentPreview(_)
                | ControlCommand::AgentRestore(_)
                | ControlCommand::SessionSetState(_)
        ) {
            self.handle_agent_control(pending);
            return;
        }
        if let ControlCommand::SessionClose(params) = &pending.request.command
            && self.sessions.borrow().contains_key(&params.session_id)
        {
            self.stop_session(&params.session_id.clone(), Some(pending));
            return;
        }
        let rename = match &pending.request.command {
            ControlCommand::SessionRename(params) => {
                Some((params.session_id.clone(), params.name.clone()))
            }
            _ => None,
        };
        if let Some((session_id, name)) = rename
            && self.sessions.borrow().contains_key(&session_id)
        {
            self.rename_session(&session_id, name, Some(pending));
            return;
        }

        let id = pending.request.id.clone();
        let response = match &pending.request.command {
            ControlCommand::AppGetState => match serde_json::to_value(self.app_state()) {
                Ok(state) => ControlResponse::success(id, state),
                Err(error) => ControlResponse::failure(
                    id,
                    ErrorCode::InternalError,
                    "Could not serialize application state",
                    Some(serde_json::json!({ "reason": error.to_string() })),
                ),
            },
            ControlCommand::UiInspect => match serde_json::to_value(self.ui_inspection()) {
                Ok(inspection) => ControlResponse::success(id, inspection),
                Err(error) => ControlResponse::failure(
                    id,
                    ErrorCode::InternalError,
                    "Could not serialize UI inspection",
                    Some(serde_json::json!({ "reason": error.to_string() })),
                ),
            },
            ControlCommand::UiCapture => unreachable!("capture is frame-delayed above"),
            ControlCommand::UiShow(params) => {
                let shown = match params.surface {
                    UiSurface::Launch => {
                        self.prepare_launch_panel();
                        self.launch.modal.present();
                        true
                    }
                    UiSurface::Appearance => {
                        self.preferences.open(preferences_ui::APPEARANCE_PAGE);
                        true
                    }
                    UiSurface::Shortcuts => {
                        self.preferences.open(preferences_ui::SHORTCUTS_PAGE);
                        true
                    }
                    UiSurface::History if self.history_button.is_sensitive() => {
                        self.history_window.present();
                        true
                    }
                    UiSurface::History => false,
                    UiSurface::Search if self.search_button.is_sensitive() => {
                        self.search_window.present();
                        self.search_entry.grab_focus();
                        true
                    }
                    UiSurface::Search => false,
                    UiSurface::Worktrees => {
                        if let Some(root) = self.preferred_project_root() {
                            self.open_worktrees_for_project(&root);
                            true
                        } else {
                            false
                        }
                    }
                    UiSurface::Agents => {
                        self.agents.modal.present();
                        true
                    }
                    UiSurface::PullRequests => {
                        if let Some(root) = self.preferred_project_root() {
                            self.prs.root.set_text(&root);
                            self.prs.modal.present();
                            true
                        } else {
                            false
                        }
                    }
                    UiSurface::ClaudeModels if self.claude_model_button.is_sensitive() => {
                        self.claude_model_window.present();
                        true
                    }
                    UiSurface::ClaudeModels => false,
                };
                ControlResponse::success(
                    id,
                    serde_json::json!({ "surface": params.surface, "shown": shown }),
                )
            }
            ControlCommand::SessionGetText(params) => {
                let sessions = self.sessions.borrow();
                match sessions.get(&params.session_id) {
                    Some(session) => {
                        // VTE exposes the visible screen and in-memory scrollback as plain text.
                        // Source: https://gnome.pages.gitlab.gnome.org/vte/gtk4/method.Terminal.get_text_format.html
                        let text = session
                            .terminal
                            .text_format(vte::Format::Text)
                            .unwrap_or_default();
                        let bounded = bounded_terminal_text(text.as_str(), params.lines as usize);
                        let snapshot = TextSnapshot {
                            session_id: params.session_id.clone(),
                            text: bounded.text,
                            lines: bounded.lines,
                            is_truncated: bounded.is_truncated,
                        };
                        match serde_json::to_value(snapshot) {
                            Ok(snapshot) => ControlResponse::success(id, snapshot),
                            Err(error) => ControlResponse::failure(
                                id,
                                ErrorCode::InternalError,
                                "Could not serialize terminal text",
                                Some(serde_json::json!({ "reason": error.to_string() })),
                            ),
                        }
                    }
                    None => ControlResponse::failure(
                        id,
                        ErrorCode::SessionNotFound,
                        "No session exists with that ID",
                        Some(serde_json::json!({ "sessionId": params.session_id })),
                    ),
                }
            }
            ControlCommand::SessionSelect(params) => {
                let row = self
                    .sessions
                    .borrow()
                    .get(&params.session_id)
                    .map(|session| session.row.clone());
                if let Some(row) = row {
                    self.list.select_row(Some(&row));
                    ControlResponse::success(
                        id,
                        serde_json::json!({ "selectedSessionId": params.session_id }),
                    )
                } else {
                    session_not_found(id, &params.session_id)
                }
            }
            ControlCommand::SessionSendInput(params) => {
                let terminal = self
                    .sessions
                    .borrow()
                    .get(&params.session_id)
                    .filter(|session| session.record.state != SessionState::Exited)
                    .map(|session| session.terminal.clone());
                if let Some(terminal) = terminal {
                    // VTE sends these bytes directly to the attached child PTY.
                    // Source: https://gnome.pages.gitlab.gnome.org/vte/gtk4/method.Terminal.feed_child.html
                    terminal.feed_child(params.text.as_bytes());
                    let mut bytes_written = params.text.len();
                    if params.append_enter {
                        terminal.feed_child(b"\n");
                        bytes_written += 1;
                    }
                    if params.append_enter {
                        self.mark_agent_busy(&params.session_id);
                    }
                    ControlResponse::success(
                        id,
                        serde_json::json!({ "bytesWritten": bytes_written }),
                    )
                } else {
                    session_not_found(id, &params.session_id)
                }
            }
            ControlCommand::SessionRename(params) => session_not_found(id, &params.session_id),
            ControlCommand::SessionClose(params) => {
                if params.allow_missing {
                    ControlResponse::success(
                        id,
                        serde_json::json!({"closedSessionId":params.session_id}),
                    )
                } else {
                    session_not_found(id, &params.session_id)
                }
            }
            ControlCommand::HistoryList(params) => {
                match self.sessions.borrow().get(&params.session_id) {
                    Some(session) => ControlResponse::success(
                        id,
                        serde_json::json!({"sessionId":params.session_id,"entries":session.history,"isTruncated":false}),
                    ),
                    None => session_not_found(id, &params.session_id),
                }
            }
            ControlCommand::ClaudePresetsGet => ControlResponse::success(
                id,
                serde_json::json!({ "presets": self.claude_presets.borrow().presets }),
            ),
            ControlCommand::SessionCreate(_)
            | ControlCommand::AppearanceSet(_)
            | ControlCommand::ShortcutSet(_)
            | ControlCommand::ProjectSet(_)
            | ControlCommand::WorktreeList(_)
            | ControlCommand::WorktreeCreate(_)
            | ControlCommand::WorktreeReap(_)
            | ControlCommand::PrList(_)
            | ControlCommand::PrAcknowledge(_)
            | ControlCommand::PrSetAutoReview(_)
            | ControlCommand::PrLaunchReview(_)
            | ControlCommand::ClaudePresetsSet(_)
            | ControlCommand::ClaudePresetApply(_)
            | ControlCommand::AgentList(_)
            | ControlCommand::AgentPreview(_)
            | ControlCommand::AgentRestore(_)
            | ControlCommand::SessionSetState(_)
            | ControlCommand::SessionOpenMagit(_)
            | ControlCommand::SessionOpenBranchReview(_) => {
                unreachable!("asynchronous command handled above")
            }
        };
        let _ = pending.respond(response);
    }
}
