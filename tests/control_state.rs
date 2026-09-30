use agtk::appearance::ThemeKey;
use agtk::control::{
    AppState, AppearanceSummary, AttentionSummary, Bounds, CaptureResult, SessionKind,
    SessionState, SessionSummary, ShortcutSummary, TextSnapshot, UiInspection, UiNode, WindowState,
};
use agtk::shortcuts::ShortcutAction;

#[test]
fn application_state_has_a_stable_machine_readable_shape() {
    let state = AppState {
        instance: "test-1".to_owned(),
        protocol_version: 1,
        selected_session_id: Some("shell-1".to_owned()),
        appearance: AppearanceSummary {
            theme: ThemeKey::Neutral,
            effective_theme: ThemeKey::Neutral,
            follow_system: false,
            font: "Monospace 11".to_owned(),
            ui_font_size: 13,
            available_themes: ThemeKey::ALL.to_vec(),
        },
        shortcuts: vec![ShortcutSummary {
            action: ShortcutAction::LaunchInProject,
            label: "Launch in current project".to_owned(),
            accelerator: "<Control><Shift>grave".to_owned(),
            default_accelerator: "<Control><Shift>grave".to_owned(),
            is_active: true,
        }],
        window: WindowState {
            width: 1200,
            height: 800,
        },
        projects: vec![agtk::control::ProjectSummary {
            root: "/work/agtk".to_owned(),
            name: "agtk".to_owned(),
            is_pinned: true,
            is_collapsed: false,
        }],
        worktree_groups: vec![agtk::control::WorktreeGroupSummary {
            project_root: "/work/agtk".to_owned(),
            path: "/work/agtk-feature".to_owned(),
            branch: "feature".to_owned(),
            session_ids: vec!["shell-1".to_owned()],
        }],
        sessions: vec![SessionSummary {
            cwd: None,
            project_root: None,
            worktree_path: None,
            created_at: 0,
            state_changed_at: 0,
            position: 0,
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
            "appearance": {
                "theme": "neutral",
                "effectiveTheme": "neutral",
                "followSystem": false,
                "font": "Monospace 11",
                "uiFontSize": 13,
                "availableThemes": [
                    "neutral", "neutral-light", "dracula", "tokyo-night",
                    "solarized-dark", "solarized-light", "light"
                ]
            },
            "shortcuts": [{
                "action": "launch-in-project",
                "label": "Launch in current project",
                "accelerator": "<Control><Shift>grave",
                "defaultAccelerator": "<Control><Shift>grave",
                "isActive": true
            }],
            "window": { "width": 1200, "height": 800 },
            "projects": [{
                "root": "/work/agtk",
                "name": "agtk",
                "isPinned": true,
                "isCollapsed": false
            }],
            "worktreeGroups": [{
                "projectRoot": "/work/agtk",
                "path": "/work/agtk-feature",
                "branch": "feature",
                "sessionIds": ["shell-1"]
            }],
            "sessions": [{
                "id": "shell-1",
                "name": "Shell 1",
                "kind": "shell",
                "state": "running",
                "isSelected": true,
                "historyCount": 2,
                "cwd": null,
                "projectRoot": null,
                "worktreePath": null,
                "createdAt": 0,
                "stateChangedAt": 0,
                "position": 0
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
fn capture_result_identifies_the_private_png_and_its_digest() {
    let result = CaptureResult {
        path: "/run/user/1000/agtk/test/captures/capture-1.png".into(),
        width: 1200,
        height: 800,
        sha256: "abc123".to_owned(),
    };

    assert_eq!(
        serde_json::to_value(result).expect("serialize capture result"),
        serde_json::json!({
            "path": "/run/user/1000/agtk/test/captures/capture-1.png",
            "width": 1200,
            "height": 800,
            "sha256": "abc123"
        })
    );
}

#[test]
fn ui_inspection_describes_controls_without_terminal_or_clipboard_content() {
    let inspection = UiInspection {
        root: UiNode {
            id: "main-window".to_owned(),
            role: "window".to_owned(),
            label: Some("agtk".to_owned()),
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
