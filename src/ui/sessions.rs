use super::*;

impl Workspace {
    pub(super) fn discover_sessions(&self) {
        let mut sockets = socket_files(&self.paths.sessions_dir());
        if self.paths.name().as_str() == "default"
            && let Some(legacy_dir) = self.paths.runtime_dir().parent()
        {
            sockets.extend(socket_files(legacy_dir));
        }
        sockets.sort();
        for socket in sockets {
            let _ = self.attach(&socket);
        }
    }

    pub(super) fn launch_shell(&self) {
        let params = CreateSessionParams {
            kind: SessionKind::Shell,
            command: None,
            args: Vec::new(),
            cwd: None,
            name: None,
            project_root: None,
            worktree_path: None,
            initial_input: None,
        };
        match SessionLaunchPlan::new(params) {
            Ok(plan) => self.launch_session(plan, None),
            Err(error) => self.show_error(&format!("Could not prepare shell: {error}")),
        }
    }

    pub(super) fn launch_controlled_session(
        &self,
        params: CreateSessionParams,
        pending: PendingRequest,
    ) {
        match SessionLaunchPlan::new(params) {
            Ok(plan) => self.launch_session(plan, Some(pending)),
            Err(error) => respond_failure(
                pending,
                ErrorCode::InvalidParams,
                "Session launch parameters are invalid",
                Some(serde_json::json!({ "reason": error.to_string() })),
            ),
        }
    }

    pub(super) fn launch_session(&self, plan: SessionLaunchPlan, pending: Option<PendingRequest>) {
        let sessions_dir = self.paths.sessions_dir();
        if let Err(error) = fs::create_dir_all(&sessions_dir) {
            self.report_launch_failure(
                pending,
                "Could not create runtime directory",
                error.to_string(),
            );
            return;
        }

        let id = self.next_session_id(plan.kind);
        let socket_path = sessions_dir.join(format!("{id}.sock"));
        let mut host = Command::new(&self.host_binary);
        host.arg("--socket")
            .arg(&socket_path)
            .arg("--")
            .arg(&plan.program)
            .args(&plan.args)
            .stdin(Stdio::null())
            .stdout(Stdio::null())
            .stderr(Stdio::null());
        if let Some(cwd) = &plan.cwd {
            host.current_dir(cwd);
        }
        match host.spawn() {
            Ok(mut child) => {
                thread::spawn(move || {
                    let _ = child.wait();
                });
            }
            Err(error) => {
                self.report_launch_failure(
                    pending,
                    "Could not start session host",
                    error.to_string(),
                );
                return;
            }
        }

        let workspace = self.clone();
        let attempts = Rc::new(Cell::new(0_u8));
        let pending = Rc::new(RefCell::new(pending));
        let metadata = SessionMetadata {
            kind: plan.kind,
            name: plan.name,
        };
        let initial_input = plan.initial_input;
        glib::timeout_add_local(Duration::from_millis(20), move || {
            match workspace.attach_with_metadata(&socket_path, Some(metadata.clone())) {
                Ok(()) => {
                    if let Some(input) = &initial_input {
                        let terminal = workspace
                            .sessions
                            .borrow()
                            .get(&id)
                            .map(|session| session.terminal.clone());
                        if let Some(terminal) = terminal {
                            terminal.feed_child(input.as_bytes());
                            terminal.feed_child(b"\n");
                        }
                    }
                    if let Some(pending) = pending.borrow_mut().take() {
                        match workspace.session_summary(&id).and_then(|summary| {
                            serde_json::to_value(summary).map_err(|error| error.to_string())
                        }) {
                            Ok(summary) => {
                                let request_id = pending.request.id.clone();
                                let _ =
                                    pending.respond(ControlResponse::success(request_id, summary));
                            }
                            Err(error) => respond_failure(
                                pending,
                                ErrorCode::InternalError,
                                "Could not describe created session",
                                Some(serde_json::json!({ "reason": error })),
                            ),
                        }
                    }
                    glib::ControlFlow::Break
                }
                Err(_) if attempts.get() < 50 => {
                    attempts.set(attempts.get() + 1);
                    glib::ControlFlow::Continue
                }
                Err(error) => {
                    if let Some(pending) = pending.borrow_mut().take() {
                        respond_failure(
                            pending,
                            ErrorCode::InternalError,
                            "Could not attach session",
                            Some(serde_json::json!({ "reason": error.to_string() })),
                        );
                    } else {
                        workspace.show_error(&format!("Could not attach session: {error}"));
                    }
                    glib::ControlFlow::Break
                }
            }
        });
    }

    pub(super) fn attach(&self, socket_path: &Path) -> Result<(), Box<dyn Error>> {
        self.attach_with_metadata(socket_path, None)
    }

    pub(super) fn attach_with_metadata(
        &self,
        socket_path: &Path,
        metadata: Option<SessionMetadata>,
    ) -> Result<(), Box<dyn Error>> {
        let id = socket_path
            .file_stem()
            .and_then(|name| name.to_str())
            .ok_or("invalid session socket name")?
            .to_owned();
        if self.sessions.borrow().contains_key(&id) {
            return Ok(());
        }

        let control = UnixStream::connect(socket_path)?;
        let attachment = receive_attachment(&control)?;
        let terminal = vte::Terminal::new();
        terminal.set_hexpand(true);
        terminal.set_vexpand(true);
        terminal.set_scrollback_lines(50_000);
        terminal.set_scroll_on_keystroke(true);
        terminal.set_font(Some(&FontDescription::from_string("Monospace 11")));
        terminal.feed(&attachment.replay);

        let pty = vte::Pty::foreign_sync(attachment.pty, None::<&gio::Cancellable>)?;
        terminal.set_pty(Some(&pty));
        terminal.connect_selection_changed(|terminal| {
            let Some(selection) = terminal.text_selected(vte::Format::Text) else {
                return;
            };
            let cleaned = cleanup_copied_text(selection.as_str());
            if !cleaned.is_empty() {
                terminal.clipboard().set_text(&cleaned);
            }
        });

        let input_tracker = Rc::new(RefCell::new(InputTracker::default()));
        let tracker = input_tracker.clone();
        let history_workspace = self.clone();
        let history_id = id.clone();
        terminal.connect_commit(move |_, text, _| {
            if let Some(input) = tracker.borrow_mut().push(text) {
                history_workspace.record_input(&history_id, input);
            }
        });

        let scroll = gtk::ScrolledWindow::builder().child(&terminal).build();
        self.stack.add_named(&scroll, Some(&id));

        let kind = metadata
            .as_ref()
            .map_or(SessionKind::Shell, |metadata| metadata.kind);
        let name = metadata
            .and_then(|metadata| metadata.name)
            .unwrap_or_else(|| display_name(&id));
        let row = gtk::ListBoxRow::new();
        row.set_widget_name(&id);
        let content = gtk::Box::new(gtk::Orientation::Horizontal, 10);
        content.set_margin_top(10);
        content.set_margin_bottom(10);
        content.set_margin_start(12);
        content.set_margin_end(12);
        content.append(&gtk::Image::from_icon_name("utilities-terminal-symbolic"));
        let label = gtk::Label::new(Some(&name));
        label.set_xalign(0.0);
        label.set_hexpand(true);
        content.append(&label);
        let close = gtk::Button::from_icon_name("window-close-symbolic");
        close.add_css_class("flat");
        close.set_tooltip_text(Some("Close this shell session"));
        content.append(&close);
        row.set_child(Some(&content));
        self.list.append(&row);

        let close_workspace = self.clone();
        let close_id = id.clone();
        close.connect_clicked(move |_| {
            if let Err(error) = close_workspace.stop_session(&close_id) {
                close_workspace.show_error(&format!("Could not stop session: {error}"));
            }
        });

        self.sessions.borrow_mut().insert(
            id.clone(),
            SessionView {
                name,
                kind,
                terminal,
                page: scroll,
                row: row.clone(),
                label,
                history: Vec::new(),
                _pty: pty,
                control,
            },
        );
        self.list.select_row(Some(&row));
        Ok(())
    }
}
