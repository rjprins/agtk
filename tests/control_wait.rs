use agtk::control::{ControlCommand, GetTextParams, SessionState, WaitCondition};
use serde_json::json;

#[test]
fn wait_conditions_choose_the_smallest_observation() {
    assert_eq!(
        WaitCondition::SessionExists("shell-1".to_owned()).observation(),
        ControlCommand::AppGetState
    );
    assert_eq!(
        WaitCondition::SelectedSession("shell-1".to_owned()).observation(),
        ControlCommand::AppGetState
    );
    assert_eq!(
        WaitCondition::SessionState {
            session_id: "shell-1".to_owned(),
            state: SessionState::Ready,
        }
        .observation(),
        ControlCommand::AppGetState
    );
    assert_eq!(
        WaitCondition::TerminalText {
            session_id: "shell-1".to_owned(),
            literal: "finished".to_owned(),
            lines: 400,
        }
        .observation(),
        ControlCommand::SessionGetText(GetTextParams {
            session_id: "shell-1".to_owned(),
            lines: 400,
        })
    );
}

#[test]
fn wait_conditions_match_documented_state_shapes() {
    let state = json!({
        "selectedSessionId": "shell-1",
        "sessions": [
            { "id": "shell-1", "state": "ready" },
            { "id": "shell-2", "state": "running" }
        ]
    });
    assert!(WaitCondition::SessionExists("shell-2".to_owned()).is_satisfied(&state));
    assert!(WaitCondition::SelectedSession("shell-1".to_owned()).is_satisfied(&state));
    assert!(
        WaitCondition::SessionState {
            session_id: "shell-1".to_owned(),
            state: SessionState::Ready,
        }
        .is_satisfied(&state)
    );
    assert!(
        WaitCondition::TerminalText {
            session_id: "shell-1".to_owned(),
            literal: "finished".to_owned(),
            lines: 200,
        }
        .is_satisfied(&json!({ "text": "build finished successfully" }))
    );
}

#[test]
fn wait_conditions_do_not_match_missing_or_partial_state() {
    let state = json!({
        "selectedSessionId": null,
        "sessions": [{ "id": "shell-1", "state": "running" }]
    });

    assert!(!WaitCondition::SessionExists("missing".to_owned()).is_satisfied(&state));
    assert!(!WaitCondition::SelectedSession("shell-1".to_owned()).is_satisfied(&state));
    assert!(
        !WaitCondition::SessionState {
            session_id: "shell-1".to_owned(),
            state: SessionState::Ready,
        }
        .is_satisfied(&state)
    );
    assert!(
        !WaitCondition::TerminalText {
            session_id: "shell-1".to_owned(),
            literal: "complete".to_owned(),
            lines: 200,
        }
        .is_satisfied(&json!({ "text": "comp" }))
    );
}
