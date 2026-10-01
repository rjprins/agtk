use super::sidebar::{
    CHANGES_WIDTH_PREFERENCE, SIDEBAR_WIDTH_PREFERENCE, changes_width, sidebar_width,
};
use super::*;
use crate::persist::now_millis;
use crate::session::Attachment;
use std::ffi::OsString;
use std::process::Command;
use std::time::Instant;

use crate::agent_hooks::ensure_claude_hook_settings;
use crate::agent_status::ScreenTracker;
use crate::emacs::EmacsIntegration;
use crate::session_names::next_worktree_session_name;

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
        record.renamed = true;
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
                        session.record.renamed = true;
                        session.label.set_text(&updated.name);
                        if matches!(
                            session.record.kind,
                            SessionKind::Claude | SessionKind::Codex
                        ) {
                            session.pending_agent_name = Some(updated.name.clone());
                        }
                    }
                    workspace.send_pending_agent_name(&updated.id);
                    workspace.refresh_content_title();
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
    /// Swaps the row title for an inline editor; finishing the edit renames the session.
    pub(super) fn start_session_rename(&self, id: &str) {
        let Some(label) = self
            .sessions
            .borrow()
            .get(id)
            .map(|session| session.label.clone())
        else {
            return;
        };
        let Some(details) = label.parent().and_downcast::<gtk::Box>() else {
            return;
        };
        let editor = gtk::EditableLabel::builder()
            .editable(true)
            .focus_on_click(true)
            .build();
        editor.set_text(&label.text());
        details.remove(&label);
        details.prepend(&editor);
        let workspace = self.clone();
        let id = id.to_owned();
        editor.connect_editing_notify(move |editor| {
            if editor.is_editing() {
                return;
            }
            let name = editor.text().trim().to_owned();
            details.remove(editor);
            details.prepend(&label);
            workspace.rename_session(&id, name, None);
        });
        editor.start_editing();
    }

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
                let loaded_sidebar_width =
                    sidebar_width(store.preference(SIDEBAR_WIDTH_PREFERENCE)?.as_ref());
                let loaded_changes_width =
                    changes_width(store.preference(CHANGES_WIDTH_PREFERENCE)?.as_ref());
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
                let mut session_pr_cache = store
                    .preference("sessionPullRequests")?
                    .and_then(|value| {
                        serde_json::from_value::<HashMap<String, crate::azure::AzurePr>>(value).ok()
                    })
                    .unwrap_or_default();
                let claude_presets = store
                    .preference("claudeModelPresets")?
                    .map(crate::claude_presets::ClaudePresetPreferences::from_value_lossy)
                    .unwrap_or_default();
                let changes_sidebar_open = store
                    .preference("changesSidebarOpen")?
                    .and_then(|value| value.as_bool());
                let changes_sidebar_page = store
                    .preference(explorer_ui::SIDEBAR_PAGE_PREFERENCE)?
                    .and_then(|value| value.as_str().map(str::to_owned));
                let changes_base_refs = store
                    .preference("changesBaseRefs")?
                    .map(serde_json::from_value::<std::collections::HashMap<String, String>>)
                    .transpose()?
                    .unwrap_or_default();
                let changes_commits_ago = store
                    .preference("changesCommitsAgo")?
                    .and_then(|value| value.as_i64())
                    .filter(|count| (1..=i64::from(i32::MAX)).contains(count))
                    .unwrap_or(1);
                let mut records = store.sessions()?;
                session_pr_cache.retain(|id, _| records.iter().any(|record| &record.id == id));
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
                let mut lost = Vec::new();
                let known_project_roots = projects
                    .projects
                    .keys()
                    .map(PathBuf::from)
                    .collect::<Vec<_>>();
                let manager = crate::worktrees::WorktreeManager::new(paths.attic_dir());
                for mut record in records {
                    backfill_restored_session_context(&mut record, &known_project_roots, &manager);
                    let connected = connect_session(&record.socket_path).ok();
                    if resume_ui::lost_its_host(record.state, connected.is_some()) {
                        lost.push(record.id.clone());
                    }
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
                    loaded_sidebar_width,
                    loaded_changes_width,
                    shortcuts,
                    projects,
                    quick_launch,
                    pr_preferences,
                    session_pr_cache,
                    claude_presets,
                    changes_sidebar_open,
                    changes_sidebar_page,
                    changes_base_refs,
                    changes_commits_ago,
                    recovered,
                    lost,
                ))
            },
            |workspace, result| match result {
                Ok((
                    store,
                    selected,
                    appearance,
                    sidebar_width,
                    changes_width,
                    shortcuts,
                    projects,
                    quick_launch,
                    pr_preferences,
                    session_pr_cache,
                    claude_presets,
                    changes_sidebar_open,
                    changes_sidebar_page,
                    changes_base_refs,
                    changes_commits_ago,
                    recovered,
                    lost,
                )) => {
                    *workspace.store.borrow_mut() = Some(store);
                    workspace.load_dialog_sizes();
                    workspace.sidebar_split.set_position(sidebar_width);
                    workspace
                        .changes
                        .split
                        .set_max_sidebar_width(f64::from(changes_width));
                    workspace.load_appearance(appearance);
                    workspace.load_shortcuts(shortcuts);
                    workspace.load_projects(projects);
                    workspace.load_quick_launch(quick_launch);
                    *workspace.pr_preferences.borrow_mut() = pr_preferences;
                    *workspace.session_pr_cache.borrow_mut() = session_pr_cache;
                    workspace.update_pr_indicator();
                    workspace.load_claude_presets(claude_presets);
                    workspace.changes.state.borrow_mut().base_ref_by_context = changes_base_refs;
                    workspace.changes.updating_base.set(true);
                    workspace
                        .changes
                        .commits_ago
                        .set_value(changes_commits_ago as f64);
                    workspace.changes.updating_base.set(false);
                    if changes_sidebar_page.as_deref() == Some(explorer_ui::FILES_PAGE) {
                        workspace
                            .changes
                            .pages
                            .set_visible_child_name(explorer_ui::FILES_PAGE);
                    }
                    if let Some(open) = changes_sidebar_open {
                        workspace.changes.split.set_show_sidebar(open);
                        workspace.changes.toggle.set_active(open);
                    }
                    let mut last = None;
                    for (record, connected) in recovered {
                        let (control, attachment) =
                            connected.map_or((None, None), |(c, a)| (Some(c), Some(a)));
                        let id = record.id.clone();
                        // Selecting marks a finished turn as seen, so select only once below.
                        match workspace.attach(record, control, attachment, false) {
                            Ok(()) => last = Some(id),
                            Err(error) => workspace
                                .show_error(&format!("Could not restore terminal: {error}")),
                        }
                    }
                    let row = {
                        let sessions = workspace.sessions.borrow();
                        selected
                            .and_then(|id| sessions.get(&id))
                            .or_else(|| last.and_then(|id| sessions.get(&id)))
                            .map(|s| s.row.clone())
                    };
                    if let Some(row) = row {
                        workspace.list.select_row(Some(&row));
                    }
                    workspace.new_shell_button.set_sensitive(true);
                    workspace.menus.enable_surfaces();
                    workspace.launch_button.set_sensitive(true);
                    workspace.start_control_server();
                    if workspace.sessions.borrow().is_empty()
                        && workspace.paths.name().as_str() == "default"
                    {
                        workspace.launch_shell();
                    }
                    workspace.offer_to_resume_lost_sessions(&lost);
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

    /// A session created over the control socket. A script or agent asks for it
    /// while the user may be typing elsewhere, so it never takes the selection.
    pub(super) fn launch_controlled_session(
        &self,
        params: CreateSessionParams,
        pending: PendingRequest,
    ) {
        self.launch_session(
            params,
            None,
            Some(Placement::in_background()),
            Some(pending),
        );
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
        self.launch_session(params, conversation_id, None, pending);
    }

    /// Launches a session, in the place of a row it replaces when `placement` is given.
    pub(super) fn launch_session(
        &self,
        params: CreateSessionParams,
        conversation_id: Option<String>,
        placement: Option<Placement>,
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
        let position = placement.and_then(|p| p.position).unwrap_or_else(|| {
            self.sessions
                .borrow()
                .values()
                .map(|s| s.record.position)
                .max()
                .unwrap_or(-1)
                + 1
        });
        // A background launch still fills an empty view, so the first row shows.
        let select = placement.is_none_or(|p| p.select) || self.selected_session_id().is_none();
        let socket = self.paths.sessions_dir().join(format!("{id}.sock"));
        let paths = self.paths.clone();
        let host_binary = self.host_binary.clone();
        let agtkctl = sibling_binary("agtkctl");
        self.run_io(
            move || {
                let plan = SessionLaunchPlan::new(params)?;
                fs::create_dir_all(socket.parent().ok_or("invalid socket path")?)?;
                let mut record = SessionRecord::discovered(&id, socket.clone());
                record.name = match plan.name {
                    Some(name) => name,
                    None => next_worktree_session_name(
                        &display_name(&id),
                        plan.worktree_path.as_deref().or(plan.cwd.as_deref()),
                        &store.sessions()?,
                    ),
                };
                record.kind = plan.kind;
                record.program = plan.program;
                record.args = plan.args;
                let session_path = plan.path;
                record.cwd = plan.cwd;
                record.project_root = plan.project_root;
                record.worktree_path = plan.worktree_path;
                record.conversation_id = conversation_id;
                record.position = position;
                record.state = SessionState::Reconnecting;
                store.save_session(&record)?;
                let mut hook_args: Vec<OsString> = if record.kind == SessionKind::Claude {
                    let settings = ensure_claude_hook_settings(paths.runtime_dir(), &agtkctl)?;
                    vec!["--settings".into(), settings.into_os_string()]
                } else {
                    Vec::new()
                };
                // The prompt goes last, and not in the record, so a relaunch does not repeat it.
                hook_args.extend(plan.prompt_args.iter().map(OsString::from));
                let host_plan = SessionHostLaunchPlan::detected(
                    host_binary,
                    paths.name().as_str(),
                    &id,
                    socket.clone(),
                    record.program.clone(),
                    record.args.clone(),
                );
                let spawn_host = |mut command: Command| {
                    command
                        .args(&hook_args)
                        .stdin(Stdio::null())
                        .stdout(Stdio::null())
                        .stderr(Stdio::null())
                        .env("AGTK_INSTANCE", paths.name().as_str())
                        .env("AGTK_SESSION_ID", &id)
                        .env("AGTK_CONTROL_SOCKET", paths.control_socket());
                    command.env("PATH", &session_path);
                    if let Some(cwd) = &record.cwd {
                        command.current_dir(cwd);
                    }
                    command.spawn()
                };
                let mut fallback = host_plan.fallback_command();
                let mut child = match spawn_host(host_plan.command()) {
                    Ok(child) => child,
                    Err(scoped_error) => match fallback.take() {
                        Some(command) => {
                            warn_unscoped_session(&id, &scoped_error);
                            match spawn_host(command) {
                                Ok(child) => child,
                                Err(direct_error) => {
                                    store.remove_session(&id)?;
                                    return Err(io::Error::other(format!(
                                        "isolated session launch failed: {scoped_error}. Direct launch failed: {direct_error}"
                                    ))
                                    .into());
                                }
                            }
                        }
                        None => {
                            store.remove_session(&id)?;
                            return Err(scoped_error.into());
                        }
                    }
                };
                let mut deadline = Instant::now() + Duration::from_secs(2);
                let connected = loop {
                    match connect_session(&socket) {
                        Ok(connected) => break Ok(connected),
                        Err(error) => {
                            if let Some(status) = child.try_wait()? {
                                if !status.success()
                                    && let Some(command) = fallback.take()
                                {
                                    warn_unscoped_session(&id, &status);
                                    child = spawn_host(command)?;
                                    deadline = Instant::now() + Duration::from_secs(2);
                                    continue;
                                }
                                break Err(io::Error::other("session host exited during launch"));
                            }
                            if Instant::now() >= deadline {
                                break Err(error);
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
                    match workspace.attach(record, Some(control), Some(attachment), select) {
                        Ok(()) => {
                            let terminal = workspace
                                .sessions
                                .borrow()
                                .get(&id)
                                .map(|s| s.terminal.clone());
                            if let (Some(input), Some(terminal)) = (initial_input, terminal) {
                                terminal.feed_child(input.as_bytes());
                                terminal.feed_child(b"\r");
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
        let closed_id = id.to_owned();
        self.stop_session_then(id, pending, move |_, pending| {
            if let Some(pending) = pending {
                let request_id = pending.request.id.clone();
                let _ = pending.respond(ControlResponse::success(
                    request_id,
                    serde_json::json!({"closedSessionId":closed_id}),
                ));
            }
        });
    }

    /// Stops a session, then hands the request on. A failed stop answers it and keeps the row.
    pub(super) fn stop_session_then(
        &self,
        id: &str,
        pending: Option<PendingRequest>,
        stopped: impl FnOnce(&Self, Option<PendingRequest>) + 'static,
    ) {
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
        let closed = record.clone();
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
                    workspace.remember_agent_name(&closed);
                    workspace.remove_session_view(&id);
                    workspace.closing_sessions.borrow_mut().remove(&id);
                    stopped(workspace, pending);
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
        let add_pattern =
            |pattern: &str| match vte::Regex::for_match(pattern, PCRE2_UTF | PCRE2_MULTILINE) {
                Ok(regex) => {
                    let tag = terminal.match_add_regex(&regex, 0);
                    terminal.match_set_cursor_name(tag, "pointer");
                    tag
                }
                Err(error) => {
                    eprintln!("agtk: invalid link pattern: {error}");
                    -1
                }
            };
        let url_tag = add_pattern(URL_PATTERN);
        let file_tag = add_pattern(crate::file_links::TERMINAL_PATTERN);

        // Ctrl+click opens right away and keeps the click from the terminal.
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
            if let Some((text, tag)) = link_at(&terminal, x, y, url_tag, file_tag) {
                gesture.set_state(gtk::EventSequenceState::Claimed);
                workspace.open_link(&terminal, &text, tag == file_tag);
            }
        });
        terminal.add_controller(click);
        self.enable_plain_link_clicks(terminal, url_tag, file_tag);
    }

    /// A plain click on a link opens it once the double-click window has passed, so
    /// drags still select and double-clicks still select a word. Raw events are used
    /// because a click gesture would lose the sequence to VTE's own gestures.
    fn enable_plain_link_clicks(&self, terminal: &vte::Terminal, url_tag: i32, file_tag: i32) {
        let events = gtk::EventControllerLegacy::new();
        events.set_propagation_phase(gtk::PropagationPhase::Capture);
        let pressed_at = Rc::new(Cell::new(None::<(f64, f64)>));
        let pending = Rc::new(RefCell::new(None::<glib::SourceId>));
        let workspace = self.clone();
        events.connect_event(move |controller, event| {
            let is_press = event.event_type() == gtk::gdk::EventType::ButtonPress;
            let is_release = event.event_type() == gtk::gdk::EventType::ButtonRelease;
            let is_primary = event
                .downcast_ref::<gtk::gdk::ButtonEvent>()
                .is_some_and(|button| button.button() == gtk::gdk::BUTTON_PRIMARY);
            if !(is_press || is_release) || !is_primary {
                return glib::Propagation::Proceed;
            }
            let Some(terminal) = controller.widget().and_downcast::<vte::Terminal>() else {
                return glib::Propagation::Proceed;
            };
            let Some(position) = widget_position(&terminal, event) else {
                return glib::Propagation::Proceed;
            };
            if is_press {
                // A second press within the double-click window means the user is selecting.
                if let Some(source) = pending.borrow_mut().take() {
                    source.remove();
                }
                pressed_at.set(Some(position));
                return glib::Propagation::Proceed;
            }
            let modified = event.modifier_state().intersects(
                gtk::gdk::ModifierType::CONTROL_MASK
                    | gtk::gdk::ModifierType::SHIFT_MASK
                    | gtk::gdk::ModifierType::ALT_MASK,
            );
            let Some(start) = pressed_at.take() else {
                return glib::Propagation::Proceed;
            };
            if modified || !is_click(start, position) {
                return glib::Propagation::Proceed;
            }
            let Some((text, tag)) = link_at(&terminal, position.0, position.1, url_tag, file_tag)
            else {
                return glib::Propagation::Proceed;
            };
            let delay = terminal
                .settings()
                .gtk_double_click_time()
                .max(0)
                .unsigned_abs();
            let open_workspace = workspace.clone();
            let open_pending = pending.clone();
            let source =
                glib::timeout_add_local_once(Duration::from_millis(u64::from(delay)), move || {
                    open_pending.borrow_mut().take();
                    open_workspace.open_link(&terminal, &text, tag == file_tag);
                });
            pending.borrow_mut().replace(source);
            glib::Propagation::Proceed
        });
        terminal.add_controller(events);
    }

    fn open_link(&self, terminal: &vte::Terminal, text: &str, is_file_path: bool) {
        if is_file_path {
            self.open_terminal_file(terminal, text);
        } else if let Some(path) = text
            .starts_with("file://")
            .then(|| gio::File::for_uri(text).path())
            .flatten()
        {
            let session_id = self.session_id_for_terminal(terminal);
            if path.is_file() {
                self.open_file_from_session(session_id, path, None, None);
            } else {
                self.show_error(&format!("{} is not a file", path.display()));
            }
        } else {
            let workspace = self.clone();
            gtk::UriLauncher::new(text).launch(
                Some(&self.window),
                None::<&gio::Cancellable>,
                move |result| {
                    if let Err(error) = result {
                        workspace.show_error(&format!("Could not open link: {error}"));
                    }
                },
            );
        }
    }

    fn session_id_for_terminal(&self, terminal: &vte::Terminal) -> Option<String> {
        self.sessions
            .borrow()
            .values()
            .find(|session| &session.terminal == terminal)
            .map(|session| session.record.id.clone())
    }

    /// Opens a path printed in the terminal in the file viewer, resolved against
    /// where that terminal runs.
    fn open_terminal_file(&self, terminal: &vte::Terminal, text: &str) {
        let Some(link) = crate::file_links::parse_link(text) else {
            return;
        };
        let session_id = self.session_id_for_terminal(terminal);
        // The shell's live directory (OSC 7) first, then where the session started.
        let mut bases = terminal
            .current_directory_uri()
            .and_then(|uri| gio::File::for_uri(&uri).path())
            .into_iter()
            .collect::<Vec<_>>();
        bases.extend(
            self.sessions
                .borrow()
                .values()
                .find(|session| &session.terminal == terminal)
                .and_then(|session| {
                    session
                        .record
                        .cwd
                        .clone()
                        .or_else(|| session.record.worktree_path.clone())
                }),
        );
        let home = glib::home_dir();
        self.run_slow(
            move || {
                // Agents usually print paths from the repository root.
                for base in bases.clone() {
                    if let Ok(root) = crate::emacs::resolve_worktree_root(&base)
                        && !bases.contains(&root)
                    {
                        bases.push(root);
                    }
                }
                let path = crate::file_links::resolve(&link.path, &bases, &home).ok_or_else(
                    || match bases.first() {
                        Some(base) => format!("{} is not in {}", link.path, base.display()),
                        None => format!("{} does not exist", link.path),
                    },
                )?;
                Ok((path, link.line, link.column))
            },
            move |workspace, result| match result {
                Ok((path, line, column)) => {
                    workspace.open_file_from_session(session_id, path, line, column);
                }
                Err(error) => workspace.show_error(&format!("Could not open file: {error}")),
            },
        );
    }

    pub(super) fn open_file_in_emacs(&self, path: PathBuf, line: Option<u32>, column: Option<u32>) {
        self.run_slow(
            move || EmacsIntegration::from_environment().open_file(&path, line, column),
            |workspace, result| {
                if let Err(error) = result {
                    workspace.show_error(&format!("Could not open file: {error}"));
                }
            },
        );
    }

    /// Rows drag their session id; dropping on another row of the same group reorders them.
    fn enable_session_drag(&self, row: &gtk::ListBoxRow, id: &str) {
        let source = gtk::DragSource::new();
        source.set_actions(gtk::gdk::DragAction::MOVE);
        let drag_id = id.to_owned();
        source.connect_prepare(move |_, _, _| {
            Some(gtk::gdk::ContentProvider::for_value(&drag_id.to_value()))
        });
        let icon_row = row.clone();
        source.connect_drag_begin(move |source, _| {
            let icon = gtk::WidgetPaintable::new(Some(&icon_row));
            source.set_icon(Some(&icon), 0, 0);
        });
        row.add_controller(source);

        let target = gtk::DropTarget::new(String::static_type(), gtk::gdk::DragAction::MOVE);
        // Read the id on enter so motion can already refuse rows from other groups.
        target.set_preload(true);
        let motion_workspace = self.clone();
        let motion_id = id.to_owned();
        target.connect_motion(move |target, _, y| {
            let Some(row) = target.widget().and_downcast::<gtk::ListBoxRow>() else {
                return gtk::gdk::DragAction::empty();
            };
            let dragged = target.value().and_then(|value| value.get::<String>().ok());
            let allowed = dragged
                .is_some_and(|dragged| motion_workspace.can_reorder_session(&dragged, &motion_id));
            if !allowed {
                clear_drop_marks(&row);
                return gtk::gdk::DragAction::empty();
            }
            let before = drop_before(&row, y);
            row.remove_css_class(if before { "drop-below" } else { "drop-above" });
            row.add_css_class(if before { "drop-above" } else { "drop-below" });
            gtk::gdk::DragAction::MOVE
        });
        target.connect_leave(|target| {
            if let Some(row) = target.widget().and_downcast::<gtk::ListBoxRow>() {
                clear_drop_marks(&row);
            }
        });
        let drop_workspace = self.clone();
        let drop_id = id.to_owned();
        target.connect_drop(move |target, value, _, y| {
            let Some(row) = target.widget().and_downcast::<gtk::ListBoxRow>() else {
                return false;
            };
            clear_drop_marks(&row);
            let Ok(dragged) = value.get::<String>() else {
                return false;
            };
            drop_workspace.reorder_session(&dragged, &drop_id, drop_before(&row, y))
        });
        row.add_controller(target);
    }

    pub(super) fn attach(
        &self,
        record: SessionRecord,
        control: Option<UnixStream>,
        attachment: Option<Attachment>,
        select: bool,
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
        // Capture, because VTE takes keys itself even without a child.
        let resume_keys = gtk::EventControllerKey::new();
        resume_keys.set_propagation_phase(gtk::PropagationPhase::Capture);
        let resume_workspace = self.clone();
        let resume_id = id.clone();
        resume_keys.connect_key_pressed(move |_, key, _, modifiers| {
            let resumable = resume_workspace
                .sessions
                .borrow()
                .get(&resume_id)
                .is_some_and(|session| is_resumable(&session.record));
            if resumable
                && matches!(key, gtk::gdk::Key::Return | gtk::gdk::Key::KP_Enter)
                && !modifiers.intersects(gtk::accelerator_get_default_mod_mask())
            {
                resume_workspace.resume_exited_session(&resume_id);
                glib::Propagation::Stop
            } else {
                glib::Propagation::Proceed
            }
        });
        terminal.add_controller(resume_keys);
        let pty = if let Some(attachment) = attachment {
            terminal.feed(&attachment.replay);
            let pty = vte::Pty::foreign_sync(attachment.pty, None::<&gio::Cancellable>)?;
            terminal.set_pty(Some(&pty));
            Some(pty)
        } else {
            terminal.feed(exited_message(&record).as_bytes());
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
                if is_resumable(&record)
                    && let Some(session) = eof_workspace.sessions.borrow().get(&eof_id)
                {
                    session
                        .terminal
                        .feed(format!("\r\n{}", exited_message(&record)).as_bytes());
                }
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
                history_workspace.record_typed_input(&history_id, input);
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
        row.set_focus_on_click(false);
        focus_terminal_on_row_activation(&row, &terminal);
        let repeat_click = gtk::GestureClick::new();
        repeat_click.set_button(1);
        let repeat_workspace = self.clone();
        let repeat_id = id.clone();
        repeat_click.connect_pressed(move |_, _, _, _| {
            if repeat_workspace.selected_session_id().as_deref() == Some(repeat_id.as_str())
                && repeat_workspace.stack.visible_child_name().as_deref() == Some("changes-diff")
            {
                repeat_workspace.activate_session(&repeat_id);
            }
        });
        row.add_controller(repeat_click);
        let content = gtk::Box::new(gtk::Orientation::Horizontal, 10);
        let kind = provider_icons::session_icon(record.kind, &record.program);
        kind.set_valign(gtk::Align::Center);
        content.append(&kind);
        let mainline = gtk::Box::new(gtk::Orientation::Vertical, 2);
        mainline.set_hexpand(true);
        mainline.set_valign(gtk::Align::Center);
        let primary = gtk::Box::new(gtk::Orientation::Horizontal, 0);
        let label = gtk::Label::new(Some(&name));
        label.set_xalign(0.0);
        label.set_ellipsize(gtk::pango::EllipsizeMode::End);
        label.add_css_class("session-title");
        primary.append(&label);
        mainline.append(&primary);
        let worktree_label = gtk::Label::new(None);
        worktree_label.set_xalign(0.0);
        worktree_label.set_ellipsize(gtk::pango::EllipsizeMode::Middle);
        worktree_label.add_css_class("caption");
        worktree_label.add_css_class("dim-label");
        show_worktree_caption(&worktree_label, &record);
        mainline.append(&worktree_label);
        content.append(&mainline);
        // Just the number: the sidebar is narrow and the tooltip carries the title.
        let pr_button = gtk::Button::with_label("#");
        pr_button.add_css_class("flat");
        pr_button.add_css_class("accent");
        pr_button.add_css_class("caption");
        pr_button.set_valign(gtk::Align::Center);
        pr_button.set_visible(false);
        let pr_workspace = self.clone();
        let pr_id = id.clone();
        pr_button.connect_clicked(move |_| pr_workspace.open_session_pr(&pr_id));
        content.append(&pr_button);
        let elapsed_label = gtk::Label::new(None);
        elapsed_label.add_css_class("caption");
        elapsed_label.add_css_class("dim-label");
        elapsed_label.add_css_class("numeric");
        elapsed_label.set_valign(gtk::Align::Center);
        content.append(&elapsed_label);
        let state_label = gtk::Label::new(None);
        state_label.add_css_class("session-state");
        state_label.set_valign(gtk::Align::Center);
        content.append(&state_label);

        // Rename, launch-here and close live in one menu, from the overflow button or a right-click.
        let menu = gio::Menu::new();
        let edit_section = gio::Menu::new();
        edit_section.append_item(&menus::targeted_item("Rename…", "win.session-rename", &id));
        if record.worktree_path.is_some() || record.cwd.is_some() {
            edit_section.append_item(&menus::targeted_item(
                "Change Worktree…",
                "win.session-worktree",
                &id,
            ));
        }
        if matches!(record.kind, SessionKind::Claude | SessionKind::Codex) {
            edit_section.append_item(&menus::targeted_item(
                "Resume Conversation",
                "win.session-resume",
                &id,
            ));
            edit_section.append_item(&menus::targeted_item(
                "Restart Agent",
                "win.session-restart",
                &id,
            ));
        }
        if record.project_root.is_some() {
            edit_section.append_item(&menus::targeted_item(
                "Launch in This Worktree…",
                "win.session-launch-here",
                &id,
            ));
            edit_section.append_item(&menus::targeted_item(
                "Open Pull Request",
                "win.session-open-pr",
                &id,
            ));
        }
        menu.append_section(None, &edit_section);
        let close_section = gio::Menu::new();
        close_section.append_item(&menus::targeted_item(
            "Close Session",
            "win.session-close",
            &id,
        ));
        menu.append_section(None, &close_section);
        let actions = gtk::MenuButton::builder()
            .icon_name("view-more-symbolic")
            .menu_model(&menu)
            .valign(gtk::Align::Center)
            .tooltip_text("Session actions")
            .build();
        actions.add_css_class("flat");
        content.append(&actions);
        row.set_child(Some(&content));
        menus::open_menu_on_right_click(&row, &actions);
        self.enable_session_drag(&row, &id);

        self.sessions.borrow_mut().insert(
            id.clone(),
            SessionView {
                record,
                terminal,
                page: scroll,
                row: row.clone(),
                label,
                worktree_label,
                state_label,
                elapsed_label,
                pr_button,
                history: Vec::new(),
                typed_history_pending: false,
                hook_signal: None,
                tracker: ScreenTracker::new(Instant::now()),
                pending_agent_name: None,
                _pty: pty,
                control,
            },
        );
        self.attach_workspace_session(&id);
        self.apply_session_state(&id);
        self.refresh_session_pr_button(&id);
        self.rebuild_sidebar();
        if select {
            self.list.select_row(Some(&row));
        }
        Ok(())
    }
}

/// Shows the worktree name, or the cwd name for sessions outside a checkout.
pub(super) fn show_worktree_caption(label: &gtk::Label, record: &SessionRecord) {
    let Some(worktree) = record.worktree_path.as_ref().or(record.cwd.as_ref()) else {
        label.set_visible(false);
        return;
    };
    let name = worktree
        .file_name()
        .and_then(|name| name.to_str())
        .filter(|name| !name.is_empty())
        .map(str::to_owned)
        .unwrap_or_else(|| worktree.to_string_lossy().into_owned());
    label.set_text(&name);
    label.set_tooltip_text(Some(&worktree.to_string_lossy()));
    label.set_visible(true);
}

/// The hyperlink or matched link text under a point, with the tag of the pattern that matched.
fn link_at(
    terminal: &vte::Terminal,
    x: f64,
    y: f64,
    url_tag: i32,
    file_tag: i32,
) -> Option<(String, i32)> {
    if let Some(uri) = terminal.check_hyperlink_at(x, y) {
        return Some((uri.to_string(), url_tag));
    }
    match terminal.check_match_at(x, y) {
        (Some(text), tag) if tag == url_tag || tag == file_tag => Some((text.to_string(), tag)),
        _ => None,
    }
}

/// Raw events carry surface coordinates; this gives them in the widget's own.
pub(super) fn widget_position(
    widget: &impl IsA<gtk::Widget>,
    event: &gtk::gdk::Event,
) -> Option<(f64, f64)> {
    let (x, y) = event.position()?;
    let native = widget.native()?;
    let (offset_x, offset_y) = native.surface_transform();
    let point = native.upcast_ref::<gtk::Widget>().compute_point(
        widget,
        &gtk::graphene::Point::new((x - offset_x) as f32, (y - offset_y) as f32),
    )?;
    Some((f64::from(point.x()), f64::from(point.y())))
}

/// Pointer travel beyond this turns a click into a selection drag.
const CLICK_SLOP: f64 = 4.0;

fn is_click(press: (f64, f64), release: (f64, f64)) -> bool {
    (press.0 - release.0).hypot(press.1 - release.1) <= CLICK_SLOP
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

/// Where a new session goes and whether it takes the selection. A relaunch
/// keeps the row it replaces; a review stays out of the way.
#[derive(Debug, Clone, Copy)]
pub(super) struct Placement {
    /// `None` appends the row after the last one.
    pub(super) position: Option<i64>,
    pub(super) select: bool,
}

impl Placement {
    /// A new last row that leaves the selected session alone, unless nothing is selected.
    pub(super) const fn in_background() -> Self {
        Self {
            position: None,
            select: false,
        }
    }
}

/// An exited agent whose conversation agtk knows can start again in its row.
pub(super) fn is_resumable(record: &SessionRecord) -> bool {
    record.state == SessionState::Exited
        && matches!(record.kind, SessionKind::Claude | SessionKind::Codex)
        && record.conversation_id.is_some()
}

fn exited_message(record: &SessionRecord) -> String {
    let resumable = SessionRecord {
        state: SessionState::Exited,
        ..record.clone()
    };
    if is_resumable(&resumable) {
        "This session has exited. Press Enter to resume the conversation, or close the row to dismiss it.\r\n".to_owned()
    } else {
        "This session has exited. Close its row to dismiss it.\r\n".to_owned()
    }
}

// Unscoped hosts share the app's cgroup, so its memory figures include the agents.
fn warn_unscoped_session(id: &str, reason: &dyn std::fmt::Display) {
    eprintln!(
        "agtk: systemd-run failed for session {id} ({reason}); launching it inside the app's cgroup"
    );
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

fn backfill_restored_session_context(
    record: &mut SessionRecord,
    known_project_roots: &[PathBuf],
    manager: &crate::worktrees::WorktreeManager,
) {
    if record.conversation_id.is_none()
        || !matches!(record.kind, SessionKind::Codex | SessionKind::Claude)
        || (record.project_root.is_some() && record.worktree_path.is_some())
    {
        return;
    }
    let Some(cwd) = record.cwd.as_deref() else {
        return;
    };
    let location = manager.resolve_session_location(cwd, known_project_roots);
    record.cwd = Some(location.cwd);
    if record.project_root.is_none() {
        record.project_root = Some(location.project_root);
    }
    if record.worktree_path.is_none() {
        record.worktree_path = location.worktree_path;
    }
}

/// The top half of a row drops before it, the bottom half after it.
fn drop_before(row: &gtk::ListBoxRow, y: f64) -> bool {
    y < f64::from(row.height()) / 2.0
}

fn clear_drop_marks(row: &gtk::ListBoxRow) {
    row.remove_css_class("drop-above");
    row.remove_css_class("drop-below");
}

fn focus_terminal_on_row_activation(row: &gtk::ListBoxRow, terminal: &vte::Terminal) {
    let terminal = terminal.clone();
    row.connect_activate(move |_| {
        terminal.grab_focus();
    });
}

#[cfg(test)]
mod tests {
    #[test]
    fn only_a_nearly_still_press_counts_as_a_link_click() {
        assert!(super::is_click((10.0, 10.0), (10.0, 10.0)));
        assert!(super::is_click((10.0, 10.0), (12.0, 13.0)));
        assert!(!super::is_click((10.0, 10.0), (30.0, 10.0)));
    }

    /// Wraps every match of the terminal file pattern in brackets, using VTE's own PCRE2.
    fn bracket_file_links(text: &str) -> String {
        const PCRE2_SUBSTITUTE_GLOBAL: u32 = 0x0000_0100;
        vte::Regex::for_match(
            crate::file_links::TERMINAL_PATTERN,
            super::PCRE2_UTF | super::PCRE2_MULTILINE,
        )
        .expect("file link pattern compiles in PCRE2")
        .substitute(text, "[$0]", PCRE2_SUBSTITUTE_GLOBAL)
        .unwrap()
        .to_string()
    }

    #[test]
    fn terminal_file_pattern_finds_paths_but_not_words_or_urls() {
        assert_eq!(
            bracket_file_links("edited src/ui/mod.rs:42:7 and README.md."),
            "edited [src/ui/mod.rs:42:7] and [README.md]."
        );
        assert_eq!(
            bracket_file_links("  File \"/app/x.py\", line 9, in main"),
            "  File \"[/app/x.py\", line 9], in main"
        );
        assert_eq!(
            bracket_file_links("see ./run.sh and ~/notes"),
            "see [./run.sh] and [~/notes]"
        );
        assert_eq!(
            bracket_file_links("plain words, version 1.2.3"),
            "plain words, version 1.2.3"
        );
        assert_eq!(
            bracket_file_links("https://example.org/a/b"),
            "https://example.org/a/b"
        );
    }

    use super::{backfill_restored_session_context, focus_terminal_on_row_activation};
    use crate::control::SessionKind;
    use crate::persist::SessionRecord;
    use crate::worktrees::WorktreeManager;
    use gtk::prelude::*;
    use std::fs;
    use std::process::Command;

    #[test]
    #[ignore = "requires a private display"]
    fn activating_a_session_row_focuses_its_terminal() {
        gtk::init().unwrap();
        let window = gtk::Window::new();
        let content = gtk::Box::new(gtk::Orientation::Horizontal, 0);
        let list = gtk::ListBox::new();
        let row = gtk::ListBoxRow::new();
        let terminal = vte::Terminal::new();
        list.append(&row);
        content.append(&list);
        content.append(&terminal);
        window.set_child(Some(&content));
        focus_terminal_on_row_activation(&row, &terminal);
        window.present();
        let context = glib::MainContext::default();
        while context.pending() {
            context.iteration(false);
        }
        gtk::prelude::GtkWindowExt::set_focus(&window, Some(&row));
        assert!(row.is_focus());
        assert!(!terminal.is_focus());

        row.emit_activate();

        assert!(terminal.is_focus());
        window.close();
    }

    #[test]
    fn recovery_backfills_only_restored_agent_session_context() {
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

        let mut restored = SessionRecord::discovered("restored", fixture.path().join("a.sock"));
        restored.kind = SessionKind::Codex;
        restored.cwd = Some(nested.clone());
        restored.conversation_id = Some("conversation".to_owned());
        backfill_restored_session_context(&mut restored, &[], &manager);

        assert_eq!(restored.cwd, Some(nested.clone()));
        assert_eq!(restored.project_root, Some(root.clone()));
        assert_eq!(restored.worktree_path, Some(root));

        let mut ordinary = SessionRecord::discovered("ordinary", fixture.path().join("b.sock"));
        ordinary.kind = SessionKind::Codex;
        ordinary.cwd = Some(nested);
        backfill_restored_session_context(&mut ordinary, &[], &manager);

        assert_eq!(ordinary.project_root, None);
        assert_eq!(ordinary.worktree_path, None);
    }
}
