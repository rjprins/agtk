use agmux_native::appearance::ThemeKey;
use agmux_native::control::{
    AgentListParams, AgentPreviewParams, AgentRestoreParams, AgentSignalState, AppearanceSetParams,
    ControlCommand, ControlResponse, ErrorCode, PROTOCOL_VERSION, ProjectSetParams, ResponseBody,
    SessionIdParams, SessionKind, SessionSetStateParams, ShortcutSetParams, WorktreeCreateParams,
    WorktreeListParams, WorktreeReapParams, decode_request, decode_response, encode_request,
    encode_response,
};
use agmux_native::providers::AgentProvider;
use agmux_native::shortcuts::ShortcutAction;
use agmux_native::worktrees::DeleteBranch;
use serde_json::json;

#[test]
fn request_round_trips_with_the_documented_envelope() {
    let request = decode_request(
        br#"{"version":1,"id":"req-42","method":"session.select","params":{"sessionId":"shell-1"}}"#,
    )
    .expect("decode request");

    assert_eq!(request.version, PROTOCOL_VERSION);
    assert_eq!(request.id, "req-42");
    assert_eq!(
        request.command,
        ControlCommand::SessionSelect(SessionIdParams {
            session_id: "shell-1".to_owned(),
        })
    );
    assert_eq!(
        encode_request(&request).expect("encode request"),
        r#"{"version":1,"id":"req-42","method":"session.select","params":{"sessionId":"shell-1"}}
"#
    );
}

#[test]
fn request_defaults_are_applied_at_the_protocol_boundary() {
    let request = decode_request(
        br#"{"version":1,"id":"input-1","method":"session.send_input","params":{"sessionId":"shell-1","text":"status"}}"#,
    )
    .expect("decode request");

    let ControlCommand::SessionSendInput(params) = request.command else {
        panic!("expected session.send_input");
    };
    assert!(params.append_enter);
}

#[test]
fn unsupported_versions_have_a_stable_error_code() {
    let error =
        decode_request(br#"{"version":2,"id":"req-1","method":"app.get_state","params":{}}"#)
            .expect_err("reject unsupported version");

    assert_eq!(error.code, ErrorCode::UnsupportedVersion);
}

#[test]
fn unknown_methods_have_a_stable_error_code() {
    let error =
        decode_request(br#"{"version":1,"id":"req-1","method":"terminal.do_magic","params":{}}"#)
            .expect_err("reject unknown method");

    assert_eq!(error.code, ErrorCode::MethodNotFound);
}

#[test]
fn invalid_parameters_have_a_stable_error_code() {
    let error = decode_request(
        br#"{"version":1,"id":"req-1","method":"session.get_text","params":{"sessionId":"shell-1","lines":20001}}"#,
    )
    .expect_err("reject line count over limit");

    assert_eq!(error.code, ErrorCode::InvalidParams);
}

#[test]
fn input_is_bounded_at_sixty_four_kibibytes() {
    let text = "x".repeat(64 * 1024 + 1);
    let wire = format!(
        r#"{{"version":1,"id":"req-1","method":"session.send_input","params":{{"sessionId":"shell-1","text":{text:?}}}}}"#
    );

    let error = decode_request(wire.as_bytes()).expect_err("reject oversized terminal input");

    assert_eq!(error.code, ErrorCode::InvalidParams);
}

#[test]
fn every_core_method_decodes_to_a_typed_command() {
    let cases = [
        ("app.get_state", "{}", "AppGetState"),
        ("ui.inspect", "{}", "UiInspect"),
        ("ui.capture", "{}", "UiCapture"),
        ("ui.show", r#"{"surface":"launch"}"#, "UiShow"),
        (
            "appearance.set",
            r#"{"theme":"neutral-light"}"#,
            "AppearanceSet",
        ),
        (
            "shortcut.set",
            r#"{"action":"new-shell","accelerator":"<Control><Shift>n"}"#,
            "ShortcutSet",
        ),
        (
            "project.set",
            r#"{"root":"/work/agmux","isPinned":true}"#,
            "ProjectSet",
        ),
        (
            "worktree.list",
            r#"{"projectRoot":"/work/agmux"}"#,
            "WorktreeList",
        ),
        (
            "worktree.create",
            r#"{"projectRoot":"/work/agmux","branch":"native-ui","purpose":"Build native UI"}"#,
            "WorktreeCreate",
        ),
        (
            "worktree.reap",
            r#"{"path":"/work/agmux-native-ui","expectedHead":"0123456789abcdef","expectedStatusHash":"abcdef","deleteBranch":"auto"}"#,
            "WorktreeReap",
        ),
        ("agent.list", "{}", "AgentList"),
        (
            "agent.preview",
            r#"{"provider":"codex","providerSessionId":"codex-1","maxMessages":20}"#,
            "AgentPreview",
        ),
        (
            "agent.restore",
            r#"{"provider":"claude","providerSessionId":"claude-1","cwd":"/work/agmux"}"#,
            "AgentRestore",
        ),
        (
            "session.set_state",
            r#"{"sessionId":"claude-1","state":"ready"}"#,
            "SessionSetState",
        ),
        (
            "session.create",
            r#"{"kind":"custom","command":"printf","args":["hello"],"cwd":"/tmp","name":"Probe"}"#,
            "SessionCreate",
        ),
        (
            "session.select",
            r#"{"sessionId":"shell-1"}"#,
            "SessionSelect",
        ),
        (
            "session.send_input",
            r#"{"sessionId":"shell-1","text":"status","appendEnter":false}"#,
            "SessionSendInput",
        ),
        (
            "session.get_text",
            r#"{"sessionId":"shell-1","lines":100}"#,
            "SessionGetText",
        ),
        (
            "session.rename",
            r#"{"sessionId":"shell-1","name":"Build"}"#,
            "SessionRename",
        ),
        (
            "session.close",
            r#"{"sessionId":"shell-1","allowMissing":true}"#,
            "SessionClose",
        ),
        ("history.list", r#"{"sessionId":"shell-1"}"#, "HistoryList"),
    ];

    for (method, params, expected_variant) in cases {
        let wire = format!(r#"{{"version":1,"id":"req","method":"{method}","params":{params}}}"#);
        let request = decode_request(wire.as_bytes()).expect(method);
        assert_eq!(request.command.method(), method, "{expected_variant}");
    }
}

#[test]
fn provider_commands_are_bounded_and_typed() {
    assert_eq!(
        decode_request(br#"{"version":1,"id":"agent","method":"agent.list","params":{}}"#)
            .unwrap()
            .command,
        ControlCommand::AgentList(AgentListParams {
            limit: 100,
            max_age_days: 90,
        })
    );
    assert_eq!(
        decode_request(br#"{"version":1,"id":"agent","method":"agent.preview","params":{"provider":"codex","providerSessionId":"codex-1"}}"#)
            .unwrap()
            .command,
        ControlCommand::AgentPreview(AgentPreviewParams {
            provider: AgentProvider::Codex,
            provider_session_id: "codex-1".to_owned(),
            max_messages: 40,
        })
    );
    assert_eq!(
        decode_request(br#"{"version":1,"id":"agent","method":"agent.restore","params":{"provider":"claude","providerSessionId":"claude-1","cwd":"/work/agmux","name":"Recovered"}}"#)
            .unwrap()
            .command,
        ControlCommand::AgentRestore(AgentRestoreParams {
            provider: AgentProvider::Claude,
            provider_session_id: "claude-1".to_owned(),
            cwd: Some("/work/agmux".into()),
            project_root: None,
            worktree_path: None,
            name: Some("Recovered".to_owned()),
        })
    );
    assert_eq!(
        decode_request(br#"{"version":1,"id":"agent","method":"session.set_state","params":{"sessionId":"claude-1","state":"waiting"}}"#)
            .unwrap()
            .command,
        ControlCommand::SessionSetState(SessionSetStateParams {
            session_id: "claude-1".to_owned(),
            state: AgentSignalState::Waiting,
        })
    );

    for invalid in [
        br#"{"version":1,"id":"agent","method":"agent.list","params":{"limit":0}}"#.as_slice(),
        br#"{"version":1,"id":"agent","method":"agent.preview","params":{"provider":"codex","providerSessionId":"bad id"}}"#.as_slice(),
        br#"{"version":1,"id":"agent","method":"agent.preview","params":{"provider":"codex","providerSessionId":"ok","maxMessages":101}}"#.as_slice(),
        br#"{"version":1,"id":"agent","method":"agent.restore","params":{"provider":"claude","providerSessionId":"ok","cwd":"relative"}}"#.as_slice(),
    ] {
        assert_eq!(
            decode_request(invalid).unwrap_err().code,
            ErrorCode::InvalidParams
        );
    }
}

#[test]
fn worktree_operations_have_typed_guarded_parameters() {
    let list = decode_request(br#"{"version":1,"id":"wt","method":"worktree.list","params":{"projectRoot":"/work/agmux"}}"#)
        .expect("decode list");
    assert_eq!(
        list.command,
        ControlCommand::WorktreeList(WorktreeListParams {
            project_root: "/work/agmux".into(),
        })
    );

    let create = decode_request(br#"{"version":1,"id":"wt","method":"worktree.create","params":{"projectRoot":"/work/agmux","branch":"native-ui","baseBranch":"main","purpose":"Build native UI"}}"#)
        .expect("decode create");
    assert_eq!(
        create.command,
        ControlCommand::WorktreeCreate(WorktreeCreateParams {
            project_root: "/work/agmux".into(),
            branch: "native-ui".to_owned(),
            base_branch: Some("main".to_owned()),
            purpose: "Build native UI".to_owned(),
        })
    );

    let reap = decode_request(br#"{"version":1,"id":"wt","method":"worktree.reap","params":{"path":"/work/agmux-native-ui","expectedHead":"0123456789abcdef","expectedStatusHash":"abcdef","deleteBranch":"force"}}"#)
        .expect("decode reap");
    assert_eq!(
        reap.command,
        ControlCommand::WorktreeReap(WorktreeReapParams {
            path: "/work/agmux-native-ui".into(),
            expected_head: "0123456789abcdef".to_owned(),
            expected_status_hash: "abcdef".to_owned(),
            delete_branch: DeleteBranch::Force,
        })
    );

    for invalid in [
        br#"{"version":1,"id":"wt","method":"worktree.list","params":{"projectRoot":"relative"}}"#.as_slice(),
        br#"{"version":1,"id":"wt","method":"worktree.create","params":{"projectRoot":"/work/agmux","branch":"Bad_Branch","purpose":"work"}}"#.as_slice(),
        br#"{"version":1,"id":"wt","method":"worktree.create","params":{"projectRoot":"/work/agmux","branch":"good-branch","purpose":""}}"#.as_slice(),
        br#"{"version":1,"id":"wt","method":"worktree.reap","params":{"path":"relative","expectedHead":"abc","expectedStatusHash":"def","deleteBranch":"auto"}}"#.as_slice(),
    ] {
        assert_eq!(
            decode_request(invalid).unwrap_err().code,
            ErrorCode::InvalidParams
        );
    }
}

#[test]
fn project_updates_require_an_absolute_root_and_one_change() {
    let request = decode_request(br#"{"version":1,"id":"project","method":"project.set","params":{"root":"/work/agmux","isPinned":true,"isCollapsed":false}}"#)
        .expect("decode project update");
    assert_eq!(
        request.command,
        ControlCommand::ProjectSet(ProjectSetParams {
            root: "/work/agmux".into(),
            is_pinned: Some(true),
            is_collapsed: Some(false),
        })
    );

    for invalid in [
        br#"{"version":1,"id":"project","method":"project.set","params":{"root":"relative","isPinned":true}}"#.as_slice(),
        br#"{"version":1,"id":"project","method":"project.set","params":{"root":"/work/agmux"}}"#.as_slice(),
    ] {
        assert_eq!(
            decode_request(invalid).unwrap_err().code,
            ErrorCode::InvalidParams
        );
    }
}

#[test]
fn shortcut_updates_decode_to_typed_commands() {
    let request = decode_request(br#"{"version":1,"id":"key","method":"shortcut.set","params":{"action":"toggle-sidebar","accelerator":"<Alt>b"}}"#)
        .expect("decode shortcut update");
    assert_eq!(
        request.command,
        ControlCommand::ShortcutSet(ShortcutSetParams {
            action: ShortcutAction::ToggleSidebar,
            accelerator: Some("<Alt>b".to_owned()),
            reset: false,
        })
    );
    let reset = decode_request(br#"{"version":1,"id":"key","method":"shortcut.set","params":{"action":"toggle-sidebar","reset":true}}"#)
        .expect("decode shortcut reset");
    assert_eq!(
        reset.command,
        ControlCommand::ShortcutSet(ShortcutSetParams {
            action: ShortcutAction::ToggleSidebar,
            accelerator: None,
            reset: true,
        })
    );

    for invalid in [
        br#"{"version":1,"id":"key","method":"shortcut.set","params":{"action":"toggle-sidebar"}}"#.as_slice(),
        br#"{"version":1,"id":"key","method":"shortcut.set","params":{"action":"toggle-sidebar","accelerator":"x","reset":true}}"#.as_slice(),
        br#"{"version":1,"id":"key","method":"shortcut.set","params":{"action":"missing","reset":true}}"#.as_slice(),
    ] {
        assert_eq!(
            decode_request(invalid).unwrap_err().code,
            ErrorCode::InvalidParams
        );
    }
}

#[test]
fn appearance_updates_decode_to_a_typed_additive_command() {
    let request = decode_request(br#"{"version":1,"id":"theme","method":"appearance.set","params":{"theme":"tokyo-night","followSystem":true,"font":"Iosevka 12"}}"#)
        .expect("decode appearance update");
    assert_eq!(
        request.command,
        ControlCommand::AppearanceSet(AppearanceSetParams {
            theme: Some(ThemeKey::TokyoNight),
            follow_system: Some(true),
            font: Some("Iosevka 12".to_owned()),
        })
    );

    for invalid in [
        br#"{"version":1,"id":"theme","method":"appearance.set","params":{}}"#.as_slice(),
        br#"{"version":1,"id":"theme","method":"appearance.set","params":{"theme":"missing"}}"#
            .as_slice(),
        br#"{"version":1,"id":"theme","method":"appearance.set","params":{"font":""}}"#.as_slice(),
    ] {
        assert_eq!(
            decode_request(invalid).unwrap_err().code,
            ErrorCode::InvalidParams
        );
    }
}

#[test]
fn requests_over_one_mibibyte_are_rejected_before_json_parsing() {
    let bytes = vec![b'x'; 1024 * 1024 + 1];

    let error = decode_request(&bytes).expect_err("reject oversized request");

    assert_eq!(error.code, ErrorCode::RequestTooLarge);
}

#[test]
fn custom_sessions_require_an_explicit_command() {
    let error = decode_request(
        br#"{"version":1,"id":"req","method":"session.create","params":{"kind":"custom"}}"#,
    )
    .expect_err("reject custom session without a command");

    assert_eq!(error.code, ErrorCode::InvalidParams);
}

#[test]
fn create_session_preserves_typed_provider_kind() {
    let request = decode_request(
        br#"{"version":1,"id":"req","method":"session.create","params":{"kind":"codex","cwd":"/tmp"}}"#,
    )
    .expect("decode Codex session");

    let ControlCommand::SessionCreate(params) = request.command else {
        panic!("expected session.create");
    };
    assert_eq!(params.kind, SessionKind::Codex);
}

#[test]
fn response_envelopes_are_stable_and_exclusive() {
    let success = ControlResponse::success("req-1", json!({ "selectedSessionId": "shell-1" }));
    assert_eq!(
        encode_response(&success).expect("encode success"),
        r#"{"version":1,"id":"req-1","result":{"selectedSessionId":"shell-1"}}
"#
    );

    let failure = ControlResponse::failure(
        "req-2",
        ErrorCode::SessionNotFound,
        "No session exists with that ID",
        Some(json!({ "sessionId": "missing" })),
    );
    assert_eq!(
        encode_response(&failure).expect("encode failure"),
        r#"{"version":1,"id":"req-2","error":{"code":"SESSION_NOT_FOUND","message":"No session exists with that ID","details":{"sessionId":"missing"}}}
"#
    );

    assert_eq!(
        decode_response(
            br#"{"version":1,"id":"req-2","error":{"code":"SESSION_NOT_FOUND","message":"No session exists with that ID","details":{"sessionId":"missing"}}}"#
        )
        .expect("decode failure response"),
        failure
    );
}

#[test]
fn response_rejects_result_and_error_in_the_same_envelope() {
    let error = decode_response(
        br#"{"version":1,"id":"req","result":{},"error":{"code":"INTERNAL_ERROR","message":"broken"}}"#,
    )
    .expect_err("reject ambiguous response");

    assert_eq!(error.code, ErrorCode::InvalidRequest);
}

#[test]
fn response_body_is_available_without_reparsing_json() {
    let response = decode_response(br#"{"version":1,"id":"req","result":{"ok":true}}"#)
        .expect("decode success response");

    assert_eq!(response.body, ResponseBody::Success(json!({ "ok": true })));
}
