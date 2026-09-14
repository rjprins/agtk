use agmux_native::control::{
    AppState, AttentionSummary, SessionKind, SessionState, SessionSummary, WindowState,
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
