use super::*;
use crate::persist::now_millis;
use crate::session::Attachment;
use std::ffi::OsString;
use std::time::Instant;

use crate::agent_hooks::ensure_claude_hook_settings;
use crate::agent_status::ScreenTracker;

// Matches the original agmux, which sent 3 events for a browser wheel notch.
const WHEEL_EVENTS_PER_NOTCH: f64 = 3.0;
const SGR_WHEEL_UP: &str = "\x1b[<64;1;1M";
const SGR_WHEEL_DOWN: &str = "\x1b[<65;1;1M";

// Stops before trailing punctuation so "see https://x.y." opens https://x.y.
const URL_PATTERN: &str = r#"\b(?:https?|file)://[^\s<>"'`]*[^\s<>"'`.,;:!?)\]}]"#;

impl Workspace {
    pub(super) fn rename_session(
        &self,
        id: &str,
        name: String,
        pending: Option<PendingRequest>,
    ) -> bool {
        let record = self.sessions.borrow().get(id).map(|s| s.record.clone());
        let Some(mut record) = record else {
            return false;
        };
        if name.trim() != name || !(1..=80).contains(&name.chars().count()) {
            if let Some(session) = self.sessions.borrow().get(id) {
                session.label.set_text(&session.record.name);
            }
            self.report_failure(
                pending,
                ErrorCode::InvalidParams,
                "Session name is invalid",
                "use a trimmed name between 1 and 80 characters".to_owned(),
            );
            return true;
        }
        record.name = name;
        let updated = record.clone();
        let Some(store) = self.store.borrow().clone() else {
            self.report_launch_failure(
                pending,
                "Could not rename session",
                "workspace is loading".to_owned(),
            );
            return true;
        };
        self.run_io(
            move || store.save_session(&record),
            move |workspace, result| match result {
                Ok(()) => {
                    if let Some(session) = workspace.sessions.borrow_mut().get_mut(&updated.id) {
                        session.record.name = updated.name.clone();
                        session.label.set_text(&updated.name);
                        if matches!(
                            session.record.kind,
                            SessionKind::Claude | SessionKind::Codex
                        ) {
                            session.pending_agent_name = Some(updated.name.clone());
                        }
                    }
                    workspace.send_pending_agent_name(&updated.id);
                    if let Some(pending) = pending {
                        match workspace.session_summary(&updated.id).and_then(|summary| {
                            serde_json::to_value(summary).map_err(|error| error.to_string())
                        }) {
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
                }
                Err(error) => workspace.report_launch_failure(
                    pending,
                    "Could not rename session",
                    error.to_string(),
                ),
            },
        );
        true
    }

    /// Waits for a finished turn so `/rename` never lands in a running turn or a prompt.
    pub(super) fn send_pending_agent_name(&self, id: &str) {
        let pending = {
            let mut sessions = self.sessions.borrow_mut();
            let Some(session) = sessions.get_mut(id) else {
                return;
            };
            if !matches!(
                session.record.state,
                SessionState::Ready | SessionState::Idle
            ) {
                return;
            }
            session
                .pending_agent_name
                .take()
                .map(|name| (name, session.terminal.clone()))
        };
        // Released first: feeding the terminal re-enters handlers that borrow the sessions.
        if let Some((name, terminal)) = pending {
            let name = name.replace(char::is_control, " ");
            terminal.feed_child(format!("/rename {name}\r").as_bytes());
        }
    }

    pub(super) fn discover_sessions(&self) {
        self.new_shell_button.set_sensitive(false);
        let paths = self.paths.clone();
        self.run_io(
            move || {
                let store = Store::open(&paths.database())?;
                let selected = store
                    .preference("selectedSessionId")?
                    .and_then(|v| v.as_str().map(str::to_owned));
                let appearance = store
                    .preference("appearance")?
                    .map(serde_json::from_value::<AppearancePreferences>)
                    .transpose()?
                    .unwrap_or_default();
                let shortcuts = store
                    .preference("shortcuts")?
                    .map(serde_json::from_value::<ShortcutPreferences>)
                    .transpose()?
                    .unwrap_or_default();
                let projects = store
                    .preference("projects")?
                    .map(serde_json::from_value::<ProjectPreferences>)
                    .transpose()?
                    .unwrap_or_default();
                let quick_launch = store
                    .preference("quickLaunch")?
                    .map(serde_json::from_value::<QuickLaunchPreferences>)
                    .transpose()?
                    .unwrap_or_default();
                let pr_preferences = store
                    .preference("pullRequests")?
                    .map(serde_json::from_value::<crate::azure::PrPreferences>)
                    .transpose()?
                    .unwrap_or_default();
                let claude_presets = store
                    .preference("claudeModelPresets")?
                    .map(crate::claude_presets::ClaudePresetPreferences::from_value_lossy)
                    .unwrap_or_default();
                let mut records = store.sessions()?;
                let mut sockets = socket_files(&paths.sessions_dir());
                if paths.name().as_str() == "default"
                    && let Some(legacy) = paths.runtime_dir().parent()
                {
                    sockets.extend(socket_files(legacy));
                }
                sockets.sort();
                for socket in sockets {
                    let Some(id) = socket.file_stem().and_then(|s| s.to_str()) else {
                        continue;
                    };
                    if id == "control" || records.iter().any(|r| r.id == id) {
                        continue;
                    }
                    let mut record = SessionRecord::discovered(id, socket.clone());
                    record.name = display_name(id);
                    record.position = records.len() as i64;
                    records.push(record);
                }
                let mut recovered = Vec::new();
                for mut record in records {
                    let connected = connect_session(&record.socket_path).ok();
                    record.state = if connected.is_some() {
                        if matches!(
                            record.state,
                            SessionState::Busy
                                | SessionState::Ready
                                | SessionState::Waiting
                                | SessionState::Idle
                        ) {
                            record.state
                        } else {
                            SessionState::Running
                        }
                    } else {
                        SessionState::Exited
                    };
                    store.save_session(&record)?;
                    recovered.push((record, connected));
                }
                Ok((
                    store,
                    selected,
                    appearance,
                    shortcuts,
                    projects,
                    quick_launch,
                    pr_preferences,
                    claude_presets,
                    recovered,
                ))
            },
            |workspace, result| match result {
                Ok((
                    store,
                    selected,
                    appearance,
                    shortcuts,
                    projects,
                    quick_launch,
                    pr_preferences,
                    claude_presets,
                    recovered,
                )) => {
                    *workspace.store.borrow_mut() = Some(store);
                    workspace.load_appearance(appearance);
                    workspace.load_shortcuts(shortcuts);
                    workspace.load_projects(projects);
                    workspace.load_quick_launch(quick_launch);
                    *workspace.pr_preferences.borrow_mut() = pr_preferences;
                    workspace.update_pr_indicator();
                    workspace.load_claude_presets(claude_presets);
                    for (record, connected) in recovered {
                        let (control, attachment) =
                            connected.map_or((None, None), |(c, a)| (Some(c), Some(a)));
                        if let Err(error) = workspace.attach(record, control, attachment) {
                            workspace.show_error(&format!("Could not restore terminal: {error}"));
                        }
                    }
                    if let Some(selected) = selected {
                        let row = workspace
                            .sessions
                            .borrow()
                            .get(&selected)
                            .map(|s| s.row.clone());
                        if let Some(row) = row {
                            workspace.list.select_row(Some(&row));
                        }
                    }
                    workspace.new_shell_button.set_sensitive(true);
                    workspace.theme_button.set_sensitive(true);
                    workspace.shortcut_button.set_sensitive(true);
                    workspace.launch_button.set_sensitive(true);
                    workspace.agent_button.set_sensitive(true);
                    workspace.pr_button.set_sensitive(true);
                    workspace.start_control_server();
                    if workspace.sessions.borrow().is_empty()
                        && workspace.paths.name().as_str() == "default"
                    {
                        workspace.launch_shell();
                    }
                }
                Err(error) => workspace.show_error(&format!("Could not load workspace: {error}")),
            },
        );
    }

    pub(super) fn launch_shell(&self) {
        self.launch_controlled(
            CreateSessionParams {
                kind: SessionKind::Shell,
                command: None,
                args: Vec::new(),
                cwd: None,
                name: None,
                project_root: None,
                worktree_path: None,
                initial_input: None,
            },
            None,
        );
    }

    pub(super) fn launch_controlled_session(
        &self,
        params: CreateSessionParams,
        pending: PendingRequest,
    ) {
        self.launch_controlled(params, Some(pending));
    }

    pub(super) fn launch_controlled(
        &self,
        params: CreateSessionParams,
        pending: Option<PendingRequest>,
    ) {
        self.launch_controlled_with_conversation(params, None, pending);
    }

    pub(super) fn launch_controlled_with_conversation(
        &self,
        params: CreateSessionParams,
        conversation_id: Option<String>,
        pending: Option<PendingRequest>,
    ) {
        let Some(store) = self.store.borrow().clone() else {
            self.report_launch_failure(
                pending,
                "Workspace is loading",
                "Try again once recovery finishes".into(),
            );
            return;
        };
        let id = self.next_session_id(params.kind);
        let position = self
            .sessions
            .borrow()
            .values()
            .map(|s| s.record.position)
            .max()
            .unwrap_or(-1)
            + 1;
        let socket = self.paths.sessions_dir().join(format!("{id}.sock"));
        let paths = self.paths.clone();
        let host_binary = self.host_binary.clone();
        let agmuxctl = sibling_binary("agmuxctl");
        self.run_io(
            move || {
                let plan = SessionLaunchPlan::new(params)?;
                fs::create_dir_all(socket.parent().ok_or("invalid socket path")?)?;
                let mut record = SessionRecord::discovered(&id, socket.clone());
                record.name = plan.name.unwrap_or_else(|| display_name(&id));
                record.kind = plan.kind;
                record.program = plan.program;
                record.args = plan.args;
                record.cwd = plan.cwd;
                record.project_root = plan.project_root;
                record.worktree_path = plan.worktree_path;
                record.conversation_id = conversation_id;
                record.position = position;
                record.state = SessionState::Reconnecting;
                store.save_session(&record)?;
                let hook_args: Vec<OsString> = if record.kind == SessionKind::Claude {
                    let settings = ensure_claude_hook_settings(paths.runtime_dir(), &agmuxctl)?;
                    vec!["--settings".into(), settings.into_os_string()]
                } else {
                    Vec::new()
                };
                let mut command = Command::new(host_binary);
                command
                    .arg("--socket")
                    .arg(&socket)
                    .arg("--")
                    .arg(&record.program)
                    .args(&record.args)
                    .args(&hook_args)
                    .stdin(Stdio::null())
                    .stdout(Stdio::null())
                    .stderr(Stdio::null());
                command
                    .env("AGMUX_INSTANCE", paths.name().as_str())
                    .env("AGMUX_SESSION_ID", &id)
                    .env("AGMUX_CONTROL_SOCKET", paths.control_socket());
                if let Some(cwd) = &record.cwd {
                    command.current_dir(cwd);
                }
                let mut child = match command.spawn() {
                    Ok(child) => child,
                    Err(error) => {
                        store.remove_session(&id)?;
                        return Err(error.into());
                    }
                };
                let deadline = Instant::now() + Duration::from_secs(2);
                let connected = loop {
                    match connect_session(&socket) {
                        Ok(connected) => break Ok(connected),
                        Err(error) if Instant::now() >= deadline => break Err(error),
                        Err(_) => {
                            if child.try_wait()?.is_some() {
                                break Err(io::Error::other("session host exited during launch"));
                            }
                            thread::sleep(Duration::from_millis(20));
                        }
                    }
                };
                // On UI loss the host must remain independent. Reap only, never tie its lifetime to GTK.
                thread::spawn(move || {
                    let _ = child.wait();
                });
                let (control, attachment) = connected?;
                // Agents start idle; hooks or the screen report the first turn.
                record.state = if status_ui::is_agent(record.kind) {
                    SessionState::Idle
                } else {
                    SessionState::Running
                };
                record.state_changed_at = now_millis();
                store.save_session(&record)?;
                Ok((record, control, attachment, plan.initial_input))
            },
            move |workspace, result| match result {
                Ok((record, control, attachment, initial_input)) => {
                    let id = record.id.clone();
                    match workspace.attach(record, Some(control), Some(attachment)) {
                        Ok(()) => {
                            let terminal = workspace
                                .sessions
                                .borrow()
                                .get(&id)
                                .map(|s| s.terminal.clone());
                            if let (Some(input), Some(terminal)) = (initial_input, terminal) {
                                terminal.feed_child(input.as_bytes());
                                terminal.feed_child(b"\n");
                            }
                            // Queue behind selection persistence before acknowledging durable creation.
                            let summary = workspace
                                .session_summary(&id)
                                .and_then(|s| serde_json::to_value(s).map_err(|e| e.to_string()));
                            workspace.run_io(
                                || Ok(()),
                                move |_, _| {
                                    if let Some(pending) = pending {
                                        match summary {
                                            Ok(summary) => {
                                                let request_id = pending.request.id.clone();
                                                let _ = pending.respond(ControlResponse::success(
                                                    request_id, summary,
                                                ));
                                            }
                                            Err(error) => respond_failure(
                                                pending,
                                                ErrorCode::InternalError,
                                                "Could not describe session",
                                                Some(serde_json::json!({"reason":error})),
                                            ),
                                        }
                                    }
                                },
                            );
                        }
                        Err(error) => workspace.report_launch_failure(
                            pending,
                            "Could not attach session",
                            error.to_string(),
                        ),
                    }
                }
                Err(error) => {
                    let code = if error
                        .downcast_ref::<crate::session::LaunchPlanError>()
                        .is_some()
                    {
                        ErrorCode::InvalidParams
                    } else {
                        ErrorCode::InternalError
                    };
                    workspace.report_failure(
                        pending,
                        code,
                        "Could not launch session",
                        error.to_string(),
                    );
                }
            },
        );
    }

    pub(super) fn stop_session(&self, id: &str, pending: Option<PendingRequest>) {
        let record_and_control = self.sessions.borrow().get(id).map(|s| {
            (
                s.record.clone(),
                s.control.as_ref().map(UnixStream::try_clone).transpose(),
            )
        });
        let Some((record, control)) = record_and_control else {
            return;
        };
        let Some(store) = self.store.borrow().clone() else {
            return;
        };
        let id = id.to_owned();
        self.closing_sessions.borrow_mut().insert(id.clone());
        self.run_io(
            move || {
                if let Some(mut control) = control? {
                    control.set_write_timeout(Some(Duration::from_secs(1)))?;
                    match control.write_all(b"K") {
                        Ok(()) => {}
                        Err(error)
                            if record.state == SessionState::Exited
                                || !record.socket_path.exists() =>
                        {
                            let _ = error;
                        }
                        Err(error) => return Err(error.into()),
                    }
                    let deadline = Instant::now() + Duration::from_secs(2);
                    while record.socket_path.exists() {
                        if Instant::now() >= deadline {
                            return Err("session host did not stop, row retained".into());
                        }
                        thread::sleep(Duration::from_millis(20));
                    }
                }
                store.remove_session(&record.id)
            },
            move |workspace, result| match result {
                Ok(()) => {
                    workspace.remove_session_view(&id);
                    workspace.closing_sessions.borrow_mut().remove(&id);
                    if let Some(pending) = pending {
                        let request_id = pending.request.id.clone();
                        let _ = pending.respond(ControlResponse::success(
                            request_id,
                            serde_json::json!({"closedSessionId":id}),
                        ));
                    }
                }
                Err(error) => {
                    workspace.closing_sessions.borrow_mut().remove(&id);
                    workspace.report_launch_failure(
                        pending,
                        "Could not stop session",
                        error.to_string(),
                    );
                }
            },
        );
    }

    fn enable_terminal_links(&self, terminal: &vte::Terminal) {
        terminal.set_allow_hyperlink(true);
        match vte::Regex::for_match(URL_PATTERN, PCRE2_UTF | PCRE2_MULTILINE) {
            Ok(regex) => {
                let tag = terminal.match_add_regex(&regex, 0);
                terminal.match_set_cursor_name(tag, "pointer");
            }
            Err(error) => eprintln!("agmux-native: invalid URL pattern: {error}"),
        }

        // Ctrl+click, because agent TUIs usually grab plain mouse clicks.
        let click = gtk::GestureClick::new();
        click.set_button(gtk::gdk::BUTTON_PRIMARY);
        click.set_propagation_phase(gtk::PropagationPhase::Capture);
        let workspace = self.clone();
        click.connect_pressed(move |gesture, _, x, y| {
            if !gesture
                .current_event_state()
                .contains(gtk::gdk::ModifierType::CONTROL_MASK)
            {
                return;
            }
            let Some(terminal) = gesture.widget().and_downcast::<vte::Terminal>() else {
                return;
            };
            let Some(url) = terminal
                .check_hyperlink_at(x, y)
                .or_else(|| terminal.check_match_at(x, y).0)
            else {
                return;
            };
            gesture.set_state(gtk::EventSequenceState::Claimed);
            let launch_workspace = workspace.clone();
            gtk::UriLauncher::new(&url).launch(
                Some(&workspace.window),
                None::<&gio::Cancellable>,
                move |result| {
                    if let Err(error) = result {
                        launch_workspace.show_error(&format!("Could not open link: {error}"));
                    }
                },
            );
        });
        terminal.add_controller(click);
    }

    pub(super) fn attach(
        &self,
        record: SessionRecord,
        control: Option<UnixStream>,
        attachment: Option<Attachment>,
    ) -> Result<(), Box<dyn Error>> {
        let id = record.id.clone();
        if self.sessions.borrow().contains_key(&id) {
            return Ok(());
        }
        let terminal = vte::Terminal::new();
        terminal.set_hexpand(true);
        terminal.set_vexpand(true);
        terminal.set_scrollback_lines(50_000);
        terminal.set_scroll_on_keystroke(true);
        self.apply_current_terminal_appearance(&terminal);
        self.enable_terminal_links(&terminal);
        if runs_claude(&record) {
            route_wheel_to_fullscreen_app(&terminal);
        }
        self.install_terminal_shortcuts(&terminal);
        let pty = if let Some(attachment) = attachment {
            terminal.feed(&attachment.replay);
            let pty = vte::Pty::foreign_sync(attachment.pty, None::<&gio::Cancellable>)?;
            terminal.set_pty(Some(&pty));
            Some(pty)
        } else {
            terminal.feed(b"This session has exited. Close its row to dismiss it.\r\n");
            None
        };
        let eof_workspace = self.clone();
        let eof_id = id.clone();
        terminal.connect_eof(move |_| {
            let record = {
                let mut sessions = eof_workspace.sessions.borrow_mut();
                sessions.get_mut(&eof_id).map(|session| {
                    session.record.state = SessionState::Exited;
                    session.record.state_changed_at = now_millis();
                    session
                        .label
                        .set_text(&format!("{} (exited)", session.record.name));
                    session.control.take();
                    session.record.clone()
                })
            };
            eof_workspace.apply_session_state(&eof_id);
            if let Some(record) = record
                && !eof_workspace.closing_sessions.borrow().contains(&eof_id)
            {
                eof_workspace.persist_record(record);
            }
        });
        terminal.connect_selection_changed(|terminal| {
            let Some(selection) = terminal.text_selected(vte::Format::Text) else {
                return;
            };
            if let Some(cleaned) = copyable_selection(selection.as_str()) {
                // Keep the normal clipboard for Ctrl+V and the primary
                // selection for Linux middle-click paste in sync.
                terminal.clipboard().set_text(&cleaned);
                terminal.primary_clipboard().set_text(&cleaned);
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

        let name = if record.state == SessionState::Exited {
            format!("{} (exited)", record.name)
        } else {
            record.name.clone()
        };
        let row = gtk::ListBoxRow::new();
        row.set_widget_name(&id);
        row.add_css_class("tui-session-row");
        let content = gtk::Box::new(gtk::Orientation::Horizontal, 3);
        content.add_css_class("tui-session-content");
        let mainline = gtk::Box::new(gtk::Orientation::Vertical, 1);
        mainline.set_hexpand(true);
        let primary = gtk::Box::new(gtk::Orientation::Horizontal, 5);
        primary.add_css_class("tui-session-primary");
        let state_label = gtk::Label::new(None);
        state_label.add_css_class("tui-state");
        primary.append(&state_label);
        let elapsed_label = gtk::Label::new(None);
        elapsed_label.add_css_class("tui-elapsed");
        primary.append(&elapsed_label);
        let label = gtk::Label::new(Some(&name));
        label.set_xalign(0.0);
        label.set_ellipsize(gtk::pango::EllipsizeMode::End);
        label.set_tooltip_text(Some("Select session"));
        primary.append(&label);
        let details = primary.clone();
        let kind = provider_icons::session_icon(record.kind, &record.program);
        let secondary = gtk::Box::new(gtk::Orientation::Horizontal, 4);
        secondary.add_css_class("tui-session-secondary");
        secondary.append(&kind);
        if let Some(worktree) = record.worktree_path.as_ref().or(record.cwd.as_ref()) {
            let worktree_name = worktree
                .file_name()
                .and_then(|name| name.to_str())
                .filter(|name| !name.is_empty())
                .map(str::to_owned)
                .unwrap_or_else(|| worktree.to_string_lossy().into_owned());
            let worktree_label = gtk::Label::new(Some(&worktree_name));
            worktree_label.set_xalign(0.0);
            worktree_label.set_ellipsize(gtk::pango::EllipsizeMode::Middle);
            worktree_label.set_tooltip_text(Some(&worktree.to_string_lossy()));
            worktree_label.add_css_class("tui-session-worktree");
            secondary.append(&worktree_label);
        }
        let edit = gtk::Button::builder()
            .icon_name("document-edit-symbolic")
            .build();
        edit.add_css_class("tui-button");
        edit.set_tooltip_text(Some("Rename session"));
        let edit_details = details.clone();
        let edit_label = label.clone();
        let edit_workspace = self.clone();
        let edit_id = id.clone();
        edit.connect_clicked(move |_| {
            let editor = gtk::EditableLabel::builder()
                .editable(true)
                .focus_on_click(true)
                .build();
            editor.set_text(&edit_label.text());
            edit_details.remove(&edit_label);
            edit_details.prepend(&editor);
            let finish_details = edit_details.clone();
            let finish_label = edit_label.clone();
            let finish_workspace = edit_workspace.clone();
            let finish_id = edit_id.clone();
            editor.connect_editing_notify(move |editor| {
                if editor.is_editing() {
                    return;
                }
                let name = editor.text().trim().to_owned();
                finish_details.remove(editor);
                finish_details.prepend(&finish_label);
                finish_workspace.rename_session(&finish_id, name, None);
            });
            editor.start_editing();
        });
        primary.append(&edit);
        mainline.append(&primary);
        mainline.append(&secondary);
        content.append(&mainline);
        let actions = gtk::Box::new(gtk::Orientation::Horizontal, 1);
        actions.add_css_class("tui-session-actions");
        actions.set_valign(gtk::Align::Center);
        if let (Some(project_root), Some(worktree)) = (
            record.project_root.clone(),
            record
                .worktree_path
                .clone()
                .or_else(|| record.cwd.clone())
                .or_else(|| record.project_root.clone()),
        ) {
            let launch = gtk::Button::builder()
                .icon_name("list-add-symbolic")
                .build();
            launch.add_css_class("tui-button");
            launch.set_tooltip_text(Some("Launch in this worktree"));
            let launch_workspace = self.clone();
            launch.connect_clicked(move |_| {
                launch_workspace.open_launch_for_worktree(&project_root, &worktree)
            });
            actions.append(&launch);
        }
        let close = gtk::Button::builder()
            .icon_name("window-close-symbolic")
            .build();
        close.add_css_class("tui-button");
        close.set_tooltip_text(Some("Close this shell session"));
        actions.append(&close);
        content.append(&actions);
        row.set_child(Some(&content));
        let close_workspace = self.clone();
        let close_id = id.clone();
        close.connect_clicked(move |_| {
            close_workspace.stop_session(&close_id, None);
        });

        self.sessions.borrow_mut().insert(
            id.clone(),
            SessionView {
                record,
                terminal,
                page: scroll,
                row: row.clone(),
                label,
                state_label,
                elapsed_label,
                history: Vec::new(),
                hook_signal: None,
                tracker: ScreenTracker::new(Instant::now()),
                pending_agent_name: None,
                _pty: pty,
                control,
            },
        );
        self.apply_session_state(&id);
        self.rebuild_sidebar();
        self.list.select_row(Some(&row));
        Ok(())
    }
}

fn runs_claude(record: &SessionRecord) -> bool {
    record.kind == SessionKind::Claude
        || record
            .program
            .file_name()
            .is_some_and(|name| name == "claude")
}

/// Fullscreen Claude never enables mouse tracking, so VTE would turn the wheel into
/// arrow keys, which recall prompt history. Claude reads SGR wheel events instead.
fn route_wheel_to_fullscreen_app(terminal: &vte::Terminal) {
    let scroll = gtk::EventControllerScroll::new(gtk::EventControllerScrollFlags::VERTICAL);
    scroll.set_propagation_phase(gtk::PropagationPhase::Capture);
    let pending = Cell::new(0.0_f64);
    scroll.connect_scroll(move |controller, _, dy| {
        let Some(terminal) = controller.widget().and_downcast::<vte::Terminal>() else {
            return glib::Propagation::Proceed;
        };
        let zooming = controller
            .current_event_state()
            .contains(gtk::gdk::ModifierType::CONTROL_MASK);
        if zooming || !on_alternate_screen(&terminal) {
            pending.set(0.0);
            return glib::Propagation::Proceed;
        }
        // Touchpads send fractions of a notch, so carry the remainder over.
        let total = pending.get() + dy * WHEEL_EVENTS_PER_NOTCH;
        let events = total.trunc();
        pending.set(total - events);
        let sequence = if events < 0.0 {
            SGR_WHEEL_UP
        } else {
            SGR_WHEEL_DOWN
        };
        let count = events.abs() as usize;
        if count > 0 {
            terminal.feed_child(sequence.repeat(count).as_bytes());
        }
        glib::Propagation::Stop
    });
    terminal.add_controller(scroll);
}

/// The alternate screen keeps no scrollback, so its scroll range is one screen tall.
fn on_alternate_screen(terminal: &vte::Terminal) -> bool {
    terminal.vadjustment().is_some_and(|adjustment| {
        (adjustment.upper() - adjustment.lower()) as libc::c_long <= terminal.row_count()
    })
}

fn connect_session(socket: &Path) -> io::Result<(UnixStream, Attachment)> {
    let control = UnixStream::connect(socket)?;
    control.set_read_timeout(Some(Duration::from_millis(300)))?;
    control.set_write_timeout(Some(Duration::from_millis(300)))?;
    let attachment = receive_attachment(&control)?;
    Ok((control, attachment))
}
