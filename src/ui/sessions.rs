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
use crate::instance::ensure_private_dir;
use crate::session_names::next_worktree_session_name;

// Temper agent TUIs' own wheel acceleration when translating touchpad movement.
const TOUCHPAD_LINE_HEIGHTS_PER_EVENT: f64 = 2.0;

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
                let appearance =
                    store.preference_or_default::<AppearancePreferences>("appearance")?;
                let loaded_sidebar_width =
                    sidebar_width(store.preference(SIDEBAR_WIDTH_PREFERENCE)?.as_ref());
                let loaded_changes_width =
                    changes_width(store.preference(CHANGES_WIDTH_PREFERENCE)?.as_ref());
                let shortcuts = store.preference_or_default::<ShortcutPreferences>("shortcuts")?;
                let projects = store.preference_or_default::<ProjectPreferences>("projects")?;
                let inactive_projects_expanded = store
                    .preference(projects_ui::INACTIVE_PROJECTS_PREFERENCE)?
                    .and_then(|value| value.as_bool())
                    .unwrap_or(false);
                let quick_launch =
                    store.preference_or_default::<QuickLaunchPreferences>("quickLaunch")?;
                let pr_preferences =
                    store.preference_or_default::<crate::azure::PrPreferences>("pullRequests")?;
                let mut session_pr_cache = store
                    .preference("sessionPullRequests")?
                    .and_then(|value| {
                        serde_json::from_value::<HashMap<String, crate::azure::AzurePr>>(value).ok()
                    })
                    .unwrap_or_default();
                let mut pr_activity = store
                    .preference_or_default::<crate::azure::PrActivityTracker>("prAgentActivity")?;
                let mut session_input_pending =
                    store.preference_or_default::<HashSet<String>>("sessionInputPending")?;
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
                    .preference_or_default::<std::collections::HashMap<String, String>>(
                        "changesBaseRefs",
                    )?;
                let markdown_preview = store
                    .preference(file_tabs_ui::MARKDOWN_PREVIEW_PREFERENCE)?
                    .and_then(|value| serde_json::from_value(value).ok())
                    .unwrap_or_default();
                let changes_commits_ago = store
                    .preference("changesCommitsAgo")?
                    .and_then(|value| value.as_i64())
                    .filter(|count| (1..=i64::from(i32::MAX)).contains(count))
                    .unwrap_or(1);
                let mut records = store.sessions()?;
                session_pr_cache.retain(|id, _| records.iter().any(|record| &record.id == id));
                pr_activity
                    .retain_sessions(&records.iter().map(|record| record.id.clone()).collect());
                session_input_pending.retain(|id| records.iter().any(|record| &record.id == id));
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
                    inactive_projects_expanded,
                    quick_launch,
                    pr_preferences,
                    session_pr_cache,
                    pr_activity,
                    session_input_pending,
                    claude_presets,
                    changes_sidebar_open,
                    changes_sidebar_page,
                    changes_base_refs,
                    changes_commits_ago,
                    markdown_preview,
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
                    inactive_projects_expanded,
                    quick_launch,
                    pr_preferences,
                    session_pr_cache,
                    pr_activity,
                    session_input_pending,
                    claude_presets,
                    changes_sidebar_open,
                    changes_sidebar_page,
                    changes_base_refs,
                    changes_commits_ago,
                    markdown_preview,
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
                    workspace
                        .inactive_projects_expanded
                        .set(inactive_projects_expanded);
                    workspace.load_projects(projects);
                    workspace.load_quick_launch(quick_launch);
                    *workspace.pr_preferences.borrow_mut() = pr_preferences;
                    *workspace.session_pr_cache.borrow_mut() = session_pr_cache;
                    *workspace.pr_activity.borrow_mut() = pr_activity;
                    *workspace.session_input_pending.borrow_mut() = session_input_pending;
                    workspace.update_pr_indicator();
                    workspace.load_claude_presets(claude_presets);
                    workspace.changes.state.borrow_mut().base_ref_by_context = changes_base_refs;
                    workspace.changes.updating_base.set(true);
                    workspace
                        .changes
                        .commits_ago
                        .set_value(changes_commits_ago as f64);
                    workspace.changes.updating_base.set(false);
                    workspace.markdown_preview.set(markdown_preview);
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
                        workspace.launch_welcome_session();
                    }
                    workspace.offer_to_resume_lost_sessions(&lost);
                }
                Err(error) => workspace.show_error(&format!("Could not load workspace: {error}")),
            },
        );
    }

    /// Opens an agent in agtk's own source, or a shell without an agent or source.
    fn launch_welcome_session(&self) {
        self.run_slow(
            || {
                Ok(crate::welcome::source_dir().and_then(|source| {
                    crate::welcome::session_params(&source, crate::session::is_installed)
                }))
            },
            |workspace, params| {
                // The user may have opened a session while the agents were looked up.
                if !workspace.sessions.borrow().is_empty() {
                    return;
                }
                match params {
                    Ok(Some(params)) => workspace.launch_controlled(params, None),
                    _ => workspace.launch_shell(),
                }
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
                ensure_private_dir(socket.parent().ok_or("invalid socket path")?)?;
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
        // A hyperlink's text can say anything, so its real target shows on hover.
        terminal.connect_hyperlink_hover_uri_notify(|terminal| {
            terminal.set_tooltip_text(terminal.hyperlink_hover_uri().as_deref());
        });
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
        } else if !is_web_link(text) {
            // Hyperlinks from terminal output can name any scheme, and a custom
            // scheme handler would start another program.
            self.show_error("Only http, https, and file links open from the terminal");
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
    /// the session's associated worktree before its terminal directories.
    fn open_terminal_file(&self, terminal: &vte::Terminal, text: &str) {
        let Some(link) = crate::file_links::parse_link(text) else {
            return;
        };
        let session_id = self.session_id_for_terminal(terminal);
        let current_directory = terminal
            .current_directory_uri()
            .and_then(|uri| gio::File::for_uri(&uri).path());
        let mut bases = terminal_file_bases(
            current_directory,
            self.sessions
                .borrow()
                .values()
                .find(|session| &session.terminal == terminal)
                .map(|session| &session.record),
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
        let project_root = record.project_root.clone();
        let terminal = vte::Terminal::new();
        terminal.set_hexpand(true);
        terminal.set_vexpand(true);
        terminal.set_scrollback_lines(50_000);
        terminal.set_scroll_on_keystroke(true);
        self.apply_current_terminal_appearance(&terminal);
        self.enable_terminal_links(&terminal);
        configure_terminal_scroll(&terminal, &record);
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

        let input_tracker = Rc::new(RefCell::new(InputTracker::with_pending_input(
            self.session_input_pending.borrow().contains(&id),
        )));
        let tracker = input_tracker.clone();
        let history_workspace = self.clone();
        let history_id = id.clone();
        terminal.connect_commit(move |_, text, _| {
            let submitted = tracker.borrow_mut().push(text);
            history_workspace
                .remember_pending_input(&history_id, tracker.borrow().has_pending_input());
            if let Some(input) = submitted {
                history_workspace.record_typed_input(&history_id, input);
            }
        });

        let page = terminal_page(&terminal);
        self.stack.add_named(&page, Some(&id));

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
        let pr_label = gtk::Label::new(Some("#"));
        let pr_contents = gtk::Overlay::new();
        pr_contents.set_child(Some(&pr_label));
        let pr_attention_dot = gtk::Box::new(gtk::Orientation::Horizontal, 0);
        pr_attention_dot.add_css_class("attention-dot");
        pr_attention_dot.set_halign(gtk::Align::End);
        pr_attention_dot.set_valign(gtk::Align::Start);
        pr_attention_dot.set_visible(false);
        pr_contents.add_overlay(&pr_attention_dot);
        let pr_button = gtk::Button::builder().child(&pr_contents).build();
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
                "Fork Conversation",
                "win.session-fork",
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
                page,
                row: row.clone(),
                label,
                worktree_label,
                state_label,
                elapsed_label,
                pr_button,
                pr_label,
                pr_attention_dot,
                history: Vec::new(),
                typed_history_pending: 0,
                input_tracker,
                prompt_log: prompt_log_ui::PromptLogFollower::default(),
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
        self.remember_project(project_root.as_deref());
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

fn is_web_link(uri: &str) -> bool {
    glib::Uri::peek_scheme(uri).is_some_and(|scheme| scheme == "http" || scheme == "https")
}

fn terminal_file_bases(
    current_directory: Option<PathBuf>,
    record: Option<&SessionRecord>,
) -> Vec<PathBuf> {
    // Re-associating a session changes its worktree without changing the running
    // program's directory, so that worktree must take precedence over OSC 7/cwd.
    let mut bases = Vec::new();
    for base in [
        record.and_then(|record| record.worktree_path.clone()),
        current_directory,
        record.and_then(|record| record.cwd.clone()),
    ]
    .into_iter()
    .flatten()
    {
        if !bases.contains(&base) {
            bases.push(base);
        }
    }
    bases
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

fn terminal_page(terminal: &vte::Terminal) -> gtk::Widget {
    // VTE and GtkScrolledWindow have incompatible scrolling. Let VTE own its
    // buffer and share its adjustment with a sibling scrollbar instead.
    let page = gtk::Box::new(gtk::Orientation::Horizontal, 0);
    let adjustment = terminal.vadjustment().expect("terminal adjustment");
    let scrollbar = gtk::Scrollbar::new(gtk::Orientation::Vertical, Some(&adjustment));
    scrollbar.set_visible(adjustment.upper() - adjustment.lower() > adjustment.page_size());
    let weak_scrollbar = scrollbar.downgrade();
    adjustment.connect_changed(move |adjustment| {
        if let Some(scrollbar) = weak_scrollbar.upgrade() {
            scrollbar.set_visible(adjustment.upper() - adjustment.lower() > adjustment.page_size());
        }
    });
    page.append(terminal);
    page.append(&scrollbar);
    page.upcast()
}

fn configure_terminal_scroll(terminal: &vte::Terminal, record: &SessionRecord) {
    // VTE's GTK 4 controller treats surface pixels as wheel clicks. Normalize
    // its fallback path too, for shells and TUIs other than Claude and Codex.
    let controllers = terminal.observe_controllers();
    for index in 0..controllers.n_items() {
        if let Some(scroll) = controllers
            .item(index)
            .and_downcast::<gtk::EventControllerScroll>()
        {
            scroll.set_flags(scroll.flags() | gtk::EventControllerScrollFlags::DISCRETE);
        }
    }
    route_wheel_to_fullscreen_app(terminal, runs_sgr_tui(record));
}

fn runs_sgr_tui(record: &SessionRecord) -> bool {
    matches!(record.kind, SessionKind::Claude | SessionKind::Codex)
        || record
            .program
            .file_name()
            .is_some_and(|name| name == "claude" || name == "codex")
}

/// Send normalized SGR wheel events to agent TUIs even after reattachment loses
/// their mouse-reporting setup. Ordinary scrollback follows touchpad pixels.
fn route_wheel_to_fullscreen_app(terminal: &vte::Terminal, sgr_tui: bool) {
    // Wayland scroll events have no position; retain terminal-relative motion
    // coordinates so Codex can target the transcript beneath the pointer.
    let position = Rc::new(Cell::new((0.0_f64, 0.0_f64)));
    let motion = gtk::EventControllerMotion::new();
    let enter_position = position.clone();
    motion.connect_enter(move |_, x, y| enter_position.set((x, y)));
    let motion_position = position.clone();
    motion.connect_motion(move |_, x, y| motion_position.set((x, y)));
    terminal.add_controller(motion);
    let scroll = gtk::EventControllerScroll::new(gtk::EventControllerScrollFlags::VERTICAL);
    scroll.set_propagation_phase(gtk::PropagationPhase::Capture);
    let pending = Rc::new(Cell::new(0.0_f64));
    let begin_pending = pending.clone();
    scroll.connect_scroll_begin(move |_| begin_pending.set(0.0));
    let end_pending = pending.clone();
    scroll.connect_scroll_end(move |_| end_pending.set(0.0));
    scroll.connect_scroll(move |controller, _, dy| {
        let Some(terminal) = controller.widget().and_downcast::<vte::Terminal>() else {
            return glib::Propagation::Proceed;
        };
        let zooming = controller
            .current_event_state()
            .contains(gtk::gdk::ModifierType::CONTROL_MASK);
        if zooming {
            pending.set(0.0);
            return glib::Propagation::Proceed;
        }
        if has_scrollback(&terminal) {
            pending.set(0.0);
            if controller.unit() == gtk::gdk::ScrollUnit::Surface {
                let adjustment = terminal.vadjustment().expect("terminal adjustment");
                let next = adjustment.value() + dy / terminal.char_height().max(1) as f64;
                adjustment.set_value(next.clamp(
                    adjustment.lower(),
                    (adjustment.upper() - adjustment.page_size()).max(adjustment.lower()),
                ));
                return glib::Propagation::Stop;
            }
            return glib::Propagation::Proceed;
        }
        if !sgr_tui {
            pending.set(0.0);
            return glib::Propagation::Proceed;
        }
        let events = wheel_events(&pending, dy, controller.unit(), terminal.char_height());
        let count = events.abs() as usize;
        if count > 0 {
            let (x, y) = position.get();
            let column = ((x / terminal.char_width().max(1) as f64) as libc::c_long + 1)
                .clamp(1, terminal.column_count().max(1));
            let row = ((y / terminal.char_height().max(1) as f64) as libc::c_long + 1)
                .clamp(1, terminal.row_count().max(1));
            let modifiers = controller.current_event_state();
            let button = if events < 0.0 { 64 } else { 65 }
                + if modifiers.contains(gtk::gdk::ModifierType::SHIFT_MASK) {
                    4
                } else {
                    0
                }
                + if modifiers.contains(gtk::gdk::ModifierType::ALT_MASK) {
                    8
                } else {
                    0
                };
            let sequence = format!("\x1b[<{button};{column};{row}M");
            terminal.feed_child(sequence.repeat(count).as_bytes());
        }
        glib::Propagation::Stop
    });
    terminal.add_controller(scroll);
}

fn wheel_events(
    pending: &Cell<f64>,
    dy: f64,
    unit: gtk::gdk::ScrollUnit,
    line_height: libc::c_long,
) -> f64 {
    // Wheel deltas count clicks; touchpad deltas count logical pixels.
    // Agent TUIs apply their own scroll step to each event, so send each only once.
    let delta = match unit {
        gtk::gdk::ScrollUnit::Surface => {
            dy / (line_height.max(1) as f64 * TOUCHPAD_LINE_HEIGHTS_PER_EVENT)
        }
        _ => dy,
    };
    // Preserve sub-line movement instead of dropping small touchpad deltas.
    let total = pending.get() + delta;
    let events = total.trunc();
    pending.set(total - events);
    events
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

/// A fullscreen redraw has no scrollback, including after reattachment loses
/// the original alternate-screen setup.
fn has_scrollback(terminal: &vte::Terminal) -> bool {
    terminal.vadjustment().is_some_and(|adjustment| {
        (adjustment.upper() - adjustment.lower()) as libc::c_long > terminal.row_count()
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
    #[ignore = "requires a private Mutter display and AGTK_TEST_BUS session bus"]
    fn real_touchpad_input_scrolls_history_and_reattached_agent_tuis() {
        use glib::variant::ToVariant;
        use nix::fcntl::{FcntlArg, OFlag, fcntl};
        use nix::sys::termios::{SetArg, cfmakeraw, tcgetattr, tcsetattr};
        use std::io::{ErrorKind, Read};
        use vte::prelude::*;

        let display = std::env::var("AGTK_TEST_DISPLAY").expect("private display required");
        let test_bus = std::env::var("AGTK_TEST_BUS").expect("private session bus required");
        assert!(display.contains("agtk-scroll-"));
        assert_eq!(std::env::var("DBUS_SESSION_BUS_ADDRESS").unwrap(), test_bus);
        // SAFETY: run this GTK test alone with --test-threads=1.
        unsafe {
            std::env::set_var("WAYLAND_DISPLAY", display);
            std::env::set_var("GDK_BACKEND", "wayland");
        }
        gtk::init().unwrap();
        let bus = gio::bus_get_sync(gio::BusType::Session, None::<&gio::Cancellable>).unwrap();
        let destination = "org.gnome.Mutter.RemoteDesktop";
        let session = bus
            .call_sync(
                Some(destination),
                "/org/gnome/Mutter/RemoteDesktop",
                destination,
                "CreateSession",
                None,
                None,
                gio::DBusCallFlags::NONE,
                5000,
                None::<&gio::Cancellable>,
            )
            .unwrap();
        let session_path = session.child_value(0).str().unwrap().to_owned();
        let call = |method: &str, parameters: Option<glib::Variant>| {
            bus.call_sync(
                Some(destination),
                &session_path,
                "org.gnome.Mutter.RemoteDesktop.Session",
                method,
                parameters.as_ref(),
                None,
                gio::DBusCallFlags::NONE,
                5000,
                None::<&gio::Cancellable>,
            )
            .unwrap();
        };
        call("Start", None);
        let pump = |milliseconds| {
            let deadline =
                std::time::Instant::now() + std::time::Duration::from_millis(milliseconds);
            while std::time::Instant::now() < deadline {
                glib::MainContext::default().iteration(false);
                std::thread::sleep(std::time::Duration::from_millis(1));
            }
        };
        let scroll = |dy| {
            call("NotifyPointerAxis", Some((0.0_f64, dy, 4_u32).to_variant()));
            pump(100);
            call(
                "NotifyPointerAxis",
                Some((0.0_f64, 0.0_f64, 5_u32).to_variant()),
            );
            pump(100);
        };
        let window = gtk::Window::new();
        window.set_decorated(false);
        window.set_default_size(1000, 700);
        window.present();
        pump(300);
        call(
            "NotifyPointerMotionRelative",
            Some((-10000.0_f64, -10000.0_f64).to_variant()),
        );
        pump(100);
        call(
            "NotifyPointerMotionRelative",
            Some((300.0_f64, 200.0_f64).to_variant()),
        );
        pump(100);

        for kind in [SessionKind::Codex, SessionKind::Claude] {
            let terminal = vte::Terminal::new();
            terminal.set_hexpand(true);
            terminal.set_vexpand(true);
            terminal.set_scrollback_lines(50_000);
            let record = SessionRecord {
                kind,
                ..SessionRecord::discovered("scroll-test", "/unused".into())
            };
            super::configure_terminal_scroll(&terminal, &record);
            let descriptors = nix::pty::openpty(None, None).unwrap();
            let mut termios = tcgetattr(&descriptors.slave).unwrap();
            cfmakeraw(&mut termios);
            tcsetattr(&descriptors.slave, SetArg::TCSANOW, &termios).unwrap();
            fcntl(&descriptors.slave, FcntlArg::F_SETFL(OFlag::O_NONBLOCK)).unwrap();
            let mut slave = std::fs::File::from(descriptors.slave);
            let pty =
                vte::Pty::foreign_sync(descriptors.master, None::<&gio::Cancellable>).unwrap();
            terminal.set_pty(Some(&pty));
            let read_input = |slave: &mut std::fs::File| {
                let mut input = Vec::new();
                let mut buffer = [0_u8; 4096];
                loop {
                    match slave.read(&mut buffer) {
                        Ok(0) => panic!("PTY closed before the test finished"),
                        Ok(count) => input.extend_from_slice(&buffer[..count]),
                        Err(error) if error.kind() == ErrorKind::WouldBlock => break,
                        result => panic!("unexpected PTY result: {result:?}"),
                    }
                }
                String::from_utf8(input).unwrap()
            };
            window.set_child(Some(&super::terminal_page(&terminal)));
            terminal.feed("line\r\n".repeat(150).as_bytes());
            pump(200);
            let adjustment = terminal.vadjustment().unwrap();
            let before = adjustment.value();
            let height = terminal.char_height() as f64;
            scroll(-height);
            assert!(read_input(&mut slave).is_empty());
            assert!(
                (before - adjustment.value() - 1.0).abs() < 0.01,
                "{kind:?}: one line of pixels must scroll one line, got {}",
                before - adjustment.value()
            );

            // A reattached TUI redraws without re-sending mouse or alternate-screen setup.
            terminal.reset(true, true);
            terminal.feed(b"\x1b[2J\x1b[Hreattached agent");
            pump(100);
            assert!(!super::has_scrollback(&terminal));
            read_input(&mut slave);
            scroll(-height * 2.0);
            let input = read_input(&mut slave);
            assert_eq!(
                input.matches("\x1b[<64;").count(),
                1,
                "{kind:?}: one normalized wheel event must reach a reattached TUI: {input:?}"
            );
            assert!(
                !input.contains("\x1b[<64;1;1M"),
                "wheel events must retain pointer coordinates"
            );

            // A fresh TUI enables these modes, but must receive the same normalized input.
            terminal.feed(b"\x1b[?1049h\x1b[?1000h\x1b[?1003h\x1b[?1006h\x1b[?1007l");
            pump(100);
            read_input(&mut slave);
            scroll(-height * 2.0);
            let input = read_input(&mut slave);
            assert_eq!(
                input.matches("\x1b[<64;").count(),
                1,
                "{kind:?}: fresh TUI must not amplify touchpad pixels: {input:?}"
            );
            window.set_child(None::<&gtk::Widget>);
        }
        call("Stop", None);
        window.close();
    }

    #[test]
    #[ignore = "requires AGTK_TEST_DISPLAY private Wayland compositor"]
    fn fullscreen_wheel_input_is_forwarded_once() {
        use std::cell::RefCell;
        use std::rc::Rc;
        use std::time::{Duration, Instant};
        use vte::prelude::*;

        let display = std::env::var("AGTK_TEST_DISPLAY").expect("private display required");
        assert!(std::path::Path::new(&display).is_absolute());
        // SAFETY: run this GTK test by itself with --test-threads=1.
        unsafe {
            std::env::set_var("WAYLAND_DISPLAY", display);
            std::env::set_var("GDK_BACKEND", "wayland");
        }
        gtk::init().unwrap();
        let terminal = vte::Terminal::new();
        let window = gtk::Window::new();
        window.set_default_size(800, 480);
        window.set_child(Some(&terminal));
        window.present();
        let pump_until = |condition: &dyn Fn() -> bool| {
            let deadline = Instant::now() + Duration::from_secs(5);
            while !condition() {
                assert!(Instant::now() < deadline, "timed out waiting for VTE");
                glib::MainContext::default().iteration(false);
            }
        };
        terminal.feed("line\r\n".repeat(100).as_bytes());
        pump_until(&|| super::has_scrollback(&terminal));
        super::route_wheel_to_fullscreen_app(&terminal, true);
        let controllers = terminal.observe_controllers();
        let controller = (0..controllers.n_items())
            .filter_map(|index| controllers.item(index))
            .filter_map(|object| object.downcast::<gtk::EventControllerScroll>().ok())
            .find(|controller| controller.propagation_phase() == gtk::PropagationPhase::Capture)
            .expect("Claude scroll controller");
        let input = Rc::new(RefCell::new(String::new()));
        let sent = input.clone();
        terminal.connect_commit(move |_, text, _| sent.borrow_mut().push_str(text));
        let emit = |dy: f64| controller.emit_by_name::<bool>("scroll", &[&0.0_f64, &dy]);

        assert!(!emit(1.0), "normal scrollback belongs to VTE");
        assert!(input.borrow().is_empty());
        terminal.feed(b"\x1b[?1049h");
        pump_until(&|| !super::has_scrollback(&terminal));

        assert!(emit(1.0));
        assert_eq!(&*input.borrow(), "\x1b[<65;1;1M");
        input.borrow_mut().clear();
        assert!(emit(-2.0));
        assert_eq!(&*input.borrow(), &"\x1b[<64;1;1M".repeat(2));
        input.borrow_mut().clear();

        assert!(emit(0.5));
        assert!(input.borrow().is_empty());
        controller.emit_by_name::<()>("scroll-end", &[]);
        assert!(emit(0.5));
        assert!(
            input.borrow().is_empty(),
            "a finished gesture leaves no remainder"
        );
        controller.emit_by_name::<()>("scroll-begin", &[]);
        assert!(emit(0.5));
        assert!(input.borrow().is_empty(), "a new gesture starts from zero");
        assert!(emit(0.5));
        assert_eq!(&*input.borrow(), "\x1b[<65;1;1M");
        window.close();
    }

    #[test]
    fn a_wheel_click_sends_one_event_to_claude() {
        let pending = std::cell::Cell::new(0.0);
        for dy in [1.0, -1.0, 2.0, -2.0] {
            assert_eq!(
                super::wheel_events(&pending, dy, gtk::gdk::ScrollUnit::Wheel, 24),
                dy
            );
        }
    }

    #[test]
    fn two_line_heights_of_touchpad_movement_send_one_event() {
        let pending = std::cell::Cell::new(0.0);
        for expected in [0.0, 1.0] {
            assert_eq!(
                super::wheel_events(&pending, 24.0, gtk::gdk::ScrollUnit::Surface, 24),
                expected
            );
        }
        assert_eq!(
            super::wheel_events(&pending, -192.0, gtk::gdk::ScrollUnit::Surface, 24),
            -4.0
        );
        // Doubling the font height doubles the movement needed for an event.
        for expected in [0.0, 1.0] {
            assert_eq!(
                super::wheel_events(&pending, 48.0, gtk::gdk::ScrollUnit::Surface, 48),
                expected
            );
        }
    }

    #[test]
    fn fractional_wheel_clicks_are_preserved() {
        let pending = std::cell::Cell::new(0.0);
        for expected in [0.0, 0.0, 0.0, -1.0] {
            assert_eq!(
                super::wheel_events(&pending, -0.25, gtk::gdk::ScrollUnit::Wheel, 24),
                expected
            );
        }
    }

    #[test]
    fn only_a_nearly_still_press_counts_as_a_link_click() {
        assert!(super::is_click((10.0, 10.0), (10.0, 10.0)));
        assert!(super::is_click((10.0, 10.0), (12.0, 13.0)));
        assert!(!super::is_click((10.0, 10.0), (30.0, 10.0)));
    }

    #[test]
    fn only_web_links_go_to_the_uri_launcher() {
        assert!(super::is_web_link("https://github.com/rjprins/agtk"));
        assert!(super::is_web_link("HTTP://example.org"));
        assert!(!super::is_web_link("smb://attacker/share"));
        assert!(!super::is_web_link("x-custom-handler:run"));
        assert!(!super::is_web_link("not a uri"));
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
    use crate::persist::SessionRecord;
    use crate::session::SessionKind;
    use crate::worktrees::WorktreeManager;
    use gtk::prelude::*;
    use std::fs;
    use std::process::Command;

    #[test]
    fn terminal_file_links_follow_the_associated_worktree() {
        let fixture = tempfile::tempdir().unwrap();
        let project = fixture.path().join("project");
        let worktree = fixture.path().join("worktree");
        for directory in [&project, &worktree] {
            fs::create_dir_all(directory.join("src")).unwrap();
            fs::write(directory.join("src/main.rs"), "").unwrap();
        }
        fs::write(worktree.join("worktree-only.rs"), "").unwrap();
        let mut record = SessionRecord::discovered("session", fixture.path().join("a.sock"));
        record.project_root = Some(project.clone());
        record.cwd = Some(project.clone());
        record.worktree_path = Some(worktree.clone());

        // Changing the associated worktree leaves both the launch directory and
        // the terminal's OSC 7 directory pointing at the original checkout.
        for current_directory in [Some(project.clone()), None] {
            let bases = super::terminal_file_bases(current_directory, Some(&record));
            for path in ["src/main.rs", "worktree-only.rs"] {
                assert_eq!(
                    crate::file_links::resolve(path, &bases, fixture.path()),
                    Some(worktree.join(path)),
                );
            }
            let absolute = project.join("src/main.rs");
            assert_eq!(
                crate::file_links::resolve(absolute.to_str().unwrap(), &bases, fixture.path()),
                Some(absolute),
            );
        }
    }

    #[test]
    fn terminal_file_links_fall_back_to_current_and_launch_directories() {
        let fixture = tempfile::tempdir().unwrap();
        let launch = fixture.path().join("launch");
        let current = fixture.path().join("current");
        let worktree = fixture.path().join("worktree");
        for directory in [&launch, &current, &worktree] {
            fs::create_dir(directory).unwrap();
        }
        fs::write(launch.join("local.rs"), "").unwrap();
        fs::write(current.join("local.rs"), "").unwrap();
        fs::write(launch.join("launch.rs"), "").unwrap();
        let mut record = SessionRecord::discovered("session", fixture.path().join("a.sock"));
        record.cwd = Some(launch.clone());
        for associated_worktree in [None, Some(worktree)] {
            record.worktree_path = associated_worktree;
            let bases = super::terminal_file_bases(Some(current.clone()), Some(&record));
            assert_eq!(
                crate::file_links::resolve("local.rs", &bases, fixture.path()),
                Some(current.join("local.rs")),
            );
            assert_eq!(
                crate::file_links::resolve("launch.rs", &bases, fixture.path()),
                Some(launch.join("launch.rs")),
            );
        }
    }

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
