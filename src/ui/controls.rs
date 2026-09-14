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
        if let ControlCommand::SessionClose(params) = &pending.request.command
            && self.sessions.borrow().contains_key(&params.session_id)
        {
            self.stop_session(&params.session_id.clone(), Some(pending));
            return;
        }
        if let ControlCommand::SessionRename(params) = &pending.request.command {
            let record = self
                .sessions
                .borrow()
                .get(&params.session_id)
                .map(|s| s.record.clone());
            if let Some(mut record) = record {
                record.name.clone_from(&params.name);
                let updated = record.clone();
                let store = self.store.borrow().clone().expect("workspace loaded");
                self.run_io(
                    move || store.save_session(&record),
                    move |workspace, result| match result {
                        Ok(()) => {
                            if let Some(session) =
                                workspace.sessions.borrow_mut().get_mut(&updated.id)
                            {
                                session.record.name = updated.name.clone();
                                session.label.set_text(&updated.name);
                            }
                            let summary = workspace
                                .session_summary(&updated.id)
                                .and_then(|s| serde_json::to_value(s).map_err(|e| e.to_string()));
                            match summary {
                                Ok(summary) => {
                                    let id = pending.request.id.clone();
                                    let _ = pending.respond(ControlResponse::success(id, summary));
                                }
                                Err(error) => workspace.report_launch_failure(
                                    Some(pending),
                                    "Could not describe session",
                                    error,
                                ),
                            }
                        }
                        Err(error) => workspace.report_launch_failure(
                            Some(pending),
                            "Could not rename session",
                            error.to_string(),
                        ),
                    },
                );
                return;
            }
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
            ControlCommand::UiCapture => match capture::capture_workspace(self)
                .and_then(|capture| serde_json::to_value(capture).map_err(Into::into))
            {
                Ok(capture) => ControlResponse::success(id, capture),
                Err(error) => ControlResponse::failure(
                    id,
                    ErrorCode::InternalError,
                    "Could not capture application content",
                    Some(serde_json::json!({ "reason": error.to_string() })),
                ),
            },
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
            ControlCommand::SessionCreate(_) => unreachable!("session creation handled above"),
        };
        let _ = pending.respond(response);
    }
}
