use agmux_native::control::{
    AppState, AttentionSummary, Bounds, SessionKind, SessionState, SessionSummary, TextSnapshot,
    UiInspection, UiNode, WindowState,
};

#[test]
fn application_state_has_a_stable_machine_readable_shape() {
    let state = AppState {
        instance: "test-1".to_owned(),
        protocol_version: 1,
        selected_session_id: Some("shell-1".to_owned()),
        window: WindowState {
            width: 1200,
            height: 800,
        },
        projects: Vec::new(),
        worktree_groups: Vec::new(),
        sessions: vec![SessionSummary {
            id: "shell-1".to_owned(),
            name: "Shell 1".to_owned(),
            kind: SessionKind::Shell,
            state: SessionState::Running,
            is_selected: true,
            history_count: 2,
        }],
        attention: AttentionSummary { count: 0 },
    };

    assert_eq!(
        serde_json::to_value(state).expect("serialize state"),
        serde_json::json!({
            "instance": "test-1",
            "protocolVersion": 1,
            "selectedSessionId": "shell-1",
            "window": { "width": 1200, "height": 800 },
            "projects": [],
            "worktreeGroups": [],
            "sessions": [{
                "id": "shell-1",
                "name": "Shell 1",
                "kind": "shell",
                "state": "running",
                "isSelected": true,
                "historyCount": 2
            }],
            "attention": { "count": 0 }
        })
    );
}

#[test]
fn terminal_text_snapshot_has_truncation_metadata() {
    let snapshot = TextSnapshot {
        session_id: "shell-1".to_owned(),
        text: "prompt output".to_owned(),
        lines: 1,
        is_truncated: true,
    };

    assert_eq!(
        serde_json::to_value(snapshot).expect("serialize terminal text snapshot"),
        serde_json::json!({
            "sessionId": "shell-1",
            "text": "prompt output",
            "lines": 1,
            "isTruncated": true
        })
    );
}

#[test]
fn ui_inspection_describes_controls_without_terminal_or_clipboard_content() {
    let inspection = UiInspection {
        root: UiNode {
            id: "main-window".to_owned(),
            role: "window".to_owned(),
            label: Some("agmux native".to_owned()),
            is_visible: true,
            is_enabled: true,
            is_selected: false,
            bounds: Bounds {
                x: 0.0,
                y: 0.0,
                width: 1200.0,
                height: 800.0,
            },
            children: vec![UiNode {
                id: "new-shell".to_owned(),
                role: "button".to_owned(),
                label: Some("New shell".to_owned()),
                is_visible: true,
                is_enabled: true,
                is_selected: false,
                bounds: Bounds::default(),
                children: Vec::new(),
            }],
        },
        is_truncated: false,
    };

    let json = serde_json::to_value(inspection).expect("serialize UI inspection");

    assert_eq!(json["root"]["role"], "window");
    assert_eq!(json["root"]["children"][0]["id"], "new-shell");
    assert_eq!(json["isTruncated"], false);
    assert!(json.to_string().find("terminal contents").is_none());
}
