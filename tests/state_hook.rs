use std::fs;
use std::path::PathBuf;

use agtk::control::{AgentSignalState, SessionKind, SessionState};
use agtk::state_hook::{HookContext, StateChange, StateHookPreferences, run_hook};

fn change() -> StateChange {
    StateChange {
        session_id: "session-1".to_owned(),
        name: "fix the sidebar".to_owned(),
        kind: SessionKind::Claude,
        cwd: Some(PathBuf::from("/work/agtk")),
        conversation_id: None,
        old_state: SessionState::Busy,
        new_state: SessionState::Ready,
    }
}

fn context() -> HookContext {
    HookContext {
        instance: "test".to_owned(),
        control_socket: PathBuf::from("/run/agtk/test/control.sock"),
        path: None,
    }
}

#[test]
fn hooks_fire_only_for_the_chosen_agent_states() {
    let mut preferences = StateHookPreferences::default();
    assert_eq!(preferences.command_for(SessionState::Ready), None);

    preferences.command = Some("notify".to_owned());
    assert_eq!(preferences.command_for(SessionState::Ready), Some("notify"));
    assert_eq!(
        preferences.command_for(SessionState::Waiting),
        Some("notify")
    );
    for state in [
        SessionState::Busy,
        SessionState::Idle,
        SessionState::Running,
        SessionState::Exited,
        SessionState::Reconnecting,
    ] {
        assert_eq!(preferences.command_for(state), None, "{state:?}");
    }

    preferences.states = vec![AgentSignalState::Idle];
    assert_eq!(preferences.command_for(SessionState::Idle), Some("notify"));
    assert_eq!(preferences.command_for(SessionState::Ready), None);
}

#[test]
fn saved_hooks_without_states_default_to_ready_and_waiting() {
    let preferences: StateHookPreferences =
        serde_json::from_value(serde_json::json!({ "command": "notify" })).unwrap();
    assert_eq!(
        preferences.states,
        [AgentSignalState::Ready, AgentSignalState::Waiting]
    );
}

#[test]
fn the_hook_gets_the_change_in_its_environment() {
    let directory = tempfile::tempdir().unwrap();
    let output = directory.path().join("env");
    let command = format!(
        r#"printf '%s|' "$AGTK_SESSION_ID" "$AGTK_SESSION_NAME" "$AGTK_SESSION_KIND" "$AGTK_SESSION_CWD" "$AGTK_CONVERSATION_ID" "$AGTK_OLD_STATE" "$AGTK_NEW_STATE" "$AGTK_INSTANCE" "$AGTK_CONTROL_SOCKET" > '{}'"#,
        output.display()
    );
    run_hook(&command, &change(), &context()).unwrap();
    assert_eq!(
        fs::read_to_string(&output).unwrap(),
        "session-1|fix the sidebar|claude|/work/agtk||busy|ready|test|/run/agtk/test/control.sock|"
    );

    let mut known = change();
    known.conversation_id = Some("6b1c".to_owned());
    known.cwd = None;
    run_hook(&command, &known, &context()).unwrap();
    assert_eq!(
        fs::read_to_string(&output).unwrap(),
        "session-1|fix the sidebar|claude||6b1c|busy|ready|test|/run/agtk/test/control.sock|"
    );
}

#[test]
fn a_failing_hook_reports_its_status_and_stderr() {
    let error = run_hook("echo 'no assistant' >&2; exit 3", &change(), &context()).unwrap_err();
    assert!(error.contains("3"), "{error}");
    assert!(error.contains("no assistant"), "{error}");
}
