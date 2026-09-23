use std::time::Instant;

use super::*;
use crate::agent_status::{self, Signal};
use crate::control::{AgentSignalState, SessionSetStateParams};
use crate::persist::now_millis;

const SPINNER_FRAMES: [&str; 4] = ["◐", "◓", "◑", "◒"];
const STATE_CLASSES: [&str; 7] = [
    "state-running",
    "state-busy",
    "state-ready",
    "state-waiting",
    "state-idle",
    "state-exited",
    "state-reconnecting",
];

impl Workspace {
    /// Polls agent screens and keeps the sidebar timers current.
    pub(super) fn install_status_timers(&self) {
        let workspace = self.clone();
        glib::timeout_add_local(Duration::from_secs(1), move || {
            workspace.refresh_agent_states();
            workspace.refresh_state_labels();
            glib::ControlFlow::Continue
        });
        let workspace = self.clone();
        let frame = Cell::new(0_usize);
        glib::timeout_add_local(Duration::from_millis(200), move || {
            frame.set((frame.get() + 1) % SPINNER_FRAMES.len());
            for session in workspace.sessions.borrow().values() {
                if session.record.state == SessionState::Busy {
                    session.state_label.set_text(SPINNER_FRAMES[frame.get()]);
                }
            }
            glib::ControlFlow::Continue
        });
        let workspace = self.clone();
        self.window.connect_is_active_notify(move |window| {
            if window.is_active()
                && let Some(id) = workspace.selected_session_id()
            {
                workspace.acknowledge_session(&id);
            }
        });
    }

    pub(super) fn set_session_state(&self, params: SessionSetStateParams, pending: PendingRequest) {
        let signal = match params.state {
            AgentSignalState::Busy => Signal::Working,
            AgentSignalState::Ready => Signal::Finished,
            AgentSignalState::Waiting => Signal::Waiting,
            AgentSignalState::Idle => Signal::Idle,
        };
        let state = {
            let mut sessions = self.sessions.borrow_mut();
            let Some(session) = sessions.get_mut(&params.session_id) else {
                drop(sessions);
                let id = pending.request.id.clone();
                let _ = pending.respond(session_not_found(id, &params.session_id));
                return;
            };
            if session.record.state != SessionState::Exited {
                session.hook_signal = Some(signal);
            }
            session.record.state
        };
        if state == SessionState::Exited {
            self.report_failure(
                Some(pending),
                ErrorCode::OperationRefused,
                "Could not update agent state",
                "session has exited".to_owned(),
            );
            return;
        }
        self.refresh_agent_state(&params.session_id);
        let session_id = params.session_id;
        // Queue behind the state save so the caller observes a durable state.
        self.run_io(
            || Ok(()),
            move |workspace, _| {
                let response = workspace
                    .session_summary(&session_id)
                    .and_then(|summary| serde_json::to_value(summary).map_err(|e| e.to_string()));
                match response {
                    Ok(response) => {
                        let id = pending.request.id.clone();
                        let _ = pending.respond(ControlResponse::success(id, response));
                    }
                    Err(error) => workspace.report_launch_failure(
                        Some(pending),
                        "Could not describe agent state",
                        error,
                    ),
                }
            },
        );
    }

    /// Submitted input starts a turn.
    pub(super) fn mark_agent_busy(&self, session_id: &str) {
        {
            let mut sessions = self.sessions.borrow_mut();
            let Some(session) = sessions.get_mut(session_id) else {
                return;
            };
            if !is_agent(session.record.kind) {
                return;
            }
            // Keep a hook-driven session from falling back to the stale hook state.
            if session.hook_signal.is_some() {
                session.hook_signal = Some(Signal::Working);
            }
        }
        self.transition_session(session_id, SessionState::Busy);
    }

    fn refresh_agent_states(&self) {
        let ids = self.sessions.borrow().keys().cloned().collect::<Vec<_>>();
        for id in ids {
            self.refresh_agent_state(&id);
        }
    }

    fn refresh_agent_state(&self, id: &str) {
        let next = {
            let mut sessions = self.sessions.borrow_mut();
            let Some(session) = sessions.get_mut(id) else {
                return;
            };
            if matches!(
                session.record.state,
                SessionState::Exited | SessionState::Reconnecting
            ) || !(is_agent(session.record.kind) || session.hook_signal.is_some())
            {
                return;
            }
            let screen = screen_text(&session.terminal);
            session.tracker.derive(
                agent_status::rules_for(session.record.kind),
                &screen,
                session.record.state,
                session.hook_signal,
                Instant::now(),
            )
        };
        if let Some(next) = next {
            self.transition_session(id, next);
        }
    }

    /// The user has seen the session, so a finished turn no longer needs attention.
    pub(super) fn acknowledge_session(&self, id: &str) {
        let is_ready = self
            .sessions
            .borrow()
            .get(id)
            .is_some_and(|session| session.record.state == SessionState::Ready);
        if is_ready {
            self.transition_session(id, SessionState::Idle);
        }
    }

    fn transition_session(&self, id: &str, next: SessionState) {
        // A turn that finishes in front of the user is already viewed.
        let viewing = self.selected_session_id().as_deref() == Some(id) && self.window.is_active();
        let next = if next == SessionState::Ready && viewing {
            SessionState::Idle
        } else {
            next
        };
        let record = {
            let mut sessions = self.sessions.borrow_mut();
            let Some(session) = sessions.get_mut(id) else {
                return;
            };
            if session.record.state == next || session.record.state == SessionState::Exited {
                return;
            }
            session.record.state = next;
            session.record.state_changed_at = now_millis();
            session.record.clone()
        };
        self.apply_session_state(id);
        self.persist_record(record);
        self.send_pending_agent_name(id);
    }

    pub(super) fn apply_session_state(&self, id: &str) {
        let sessions = self.sessions.borrow();
        let Some(session) = sessions.get(id) else {
            return;
        };
        let state = session.record.state;
        session.state_label.set_text(session_state_indicator(state));
        for class in STATE_CLASSES {
            session.state_label.remove_css_class(class);
        }
        session
            .state_label
            .add_css_class(session_state_css_class(state));
        for (class, active) in [
            ("session-ready", state == SessionState::Ready),
            ("session-waiting", state == SessionState::Waiting),
        ] {
            if active {
                session.row.add_css_class(class);
            } else {
                session.row.remove_css_class(class);
            }
        }
        refresh_state_label(session);
    }

    fn refresh_state_labels(&self) {
        for session in self.sessions.borrow().values() {
            refresh_state_label(session);
        }
    }
}

fn refresh_state_label(session: &SessionView) {
    let since = state_since(&session.record);
    let elapsed = elapsed_since(since);
    if session.elapsed_label.text() != elapsed {
        session.elapsed_label.set_text(&elapsed);
    }
    let tooltip = match session.record.state {
        SessionState::Running | SessionState::Exited | SessionState::Reconnecting => {
            session_state_name(session.record.state).to_owned()
        }
        state => format!("{} for {}", session_state_name(state), elapsed_long(since)),
    };
    session.state_label.set_tooltip_text(Some(&tooltip));
}

fn state_since(record: &SessionRecord) -> u64 {
    if record.state_changed_at == 0 {
        record.created_at
    } else {
        record.state_changed_at
    }
}

pub(super) const fn is_agent(kind: SessionKind) -> bool {
    matches!(
        kind,
        SessionKind::Codex | SessionKind::Claude | SessionKind::Gemini
    )
}

/// The live screen, not the scrolled view, so scrolling back does not change the reading.
fn screen_text(terminal: &vte::Terminal) -> String {
    let rows = terminal.row_count();
    let end = terminal
        .vadjustment()
        .map(|adjustment| adjustment.upper() as libc::c_long)
        .unwrap_or(rows);
    let start = (end - rows).max(0);
    terminal
        .text_range_format(
            vte::Format::Text,
            start,
            0,
            end - 1,
            terminal.column_count(),
        )
        .0
        .map(|text| text.to_string())
        .unwrap_or_default()
}

pub(super) const fn session_state_indicator(state: SessionState) -> &'static str {
    match state {
        SessionState::Running => "○",
        SessionState::Busy => SPINNER_FRAMES[0],
        SessionState::Ready => "●",
        SessionState::Waiting => "◆",
        SessionState::Idle => "○",
        SessionState::Exited => "✕",
        SessionState::Reconnecting => "◌",
    }
}

pub(super) const fn session_state_css_class(state: SessionState) -> &'static str {
    match state {
        SessionState::Running => "state-running",
        SessionState::Busy => "state-busy",
        SessionState::Ready => "state-ready",
        SessionState::Waiting => "state-waiting",
        SessionState::Idle => "state-idle",
        SessionState::Exited => "state-exited",
        SessionState::Reconnecting => "state-reconnecting",
    }
}

pub(super) const fn session_state_name(state: SessionState) -> &'static str {
    match state {
        SessionState::Running => "Running",
        SessionState::Busy => "Busy",
        SessionState::Ready => "Ready",
        SessionState::Waiting => "Waiting for input",
        SessionState::Idle => "Idle",
        SessionState::Exited => "Exited",
        SessionState::Reconnecting => "Reconnecting",
    }
}

pub(super) fn elapsed_since(since: u64) -> String {
    let elapsed = now_millis().saturating_sub(since) / 1_000;
    if elapsed < 5 {
        "now".to_owned()
    } else if elapsed < 60 {
        format!("{elapsed}s")
    } else if elapsed < 3_600 {
        format!("{}m", elapsed / 60)
    } else if elapsed < 86_400 {
        format!("{}h", elapsed / 3_600)
    } else {
        format!("{}d", elapsed / 86_400)
    }
}

fn elapsed_long(since: u64) -> String {
    let elapsed = now_millis().saturating_sub(since) / 1_000;
    if elapsed < 60 {
        format!("{elapsed}s")
    } else if elapsed < 3_600 {
        format!("{}m {}s", elapsed / 60, elapsed % 60)
    } else {
        format!("{}h {}m", elapsed / 3_600, elapsed % 3_600 / 60)
    }
}
