use agtk::appearance::ThemeKey;
use agtk::control::{
    AgentListParams, AgentPreviewParams, AgentRestoreParams, AgentSignalState, AppearanceSetParams,
    ControlCommand, ControlResponse, ErrorCode, PROTOCOL_VERSION, PrListParams,
    ProjectRemoveParams, ProjectSetParams, ResponseBody, SessionIdParams, SessionKind,
    SessionSetStateParams, ShortcutSetParams, WorktreeCreateParams, WorktreeListParams,
    WorktreeReapParams, control_timeout, decode_request, decode_response, encode_request,
    encode_response,
};
use agtk::providers::AgentProvider;
use agtk::shortcuts::ShortcutAction;
use agtk::worktrees::DeleteBranch;
use serde_json::json;
use std::time::Duration;

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
fn pull_request_commands_get_a_longer_control_deadline() {
    let pull_requests = ControlCommand::PrList(PrListParams {
        project_root: "/work/agtk".into(),
    });

    assert_eq!(control_timeout(&pull_requests), Duration::from_secs(30));
    assert_eq!(
        control_timeout(&ControlCommand::AppGetState),
        Duration::from_secs(5)
    );
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
            r#"{"action":"launch-in-project","accelerator":"<Control><Shift>n"}"#,
            "ShortcutSet",
        ),
        (
            "project.set",
            r#"{"root":"/work/agtk","isPinned":true}"#,
            "ProjectSet",
        ),
        (
            "project.remove",
            r#"{"root":"/work/agtk"}"#,
            "ProjectRemove",
        ),
        (
            "worktree.list",
            r#"{"projectRoot":"/work/agtk"}"#,
            "WorktreeList",
        ),
        (
            "worktree.create",
            r#"{"projectRoot":"/work/agtk","branch":"native-ui","purpose":"Build native UI"}"#,
            "WorktreeCreate",
        ),
        (
            "worktree.reap",
            r#"{"path":"/work/agtk-ui","expectedHead":"0123456789abcdef","expectedStatusHash":"abcdef","deleteBranch":"auto"}"#,
            "WorktreeReap",
        ),
        ("pr.list", r#"{"projectRoot":"/work/agtk"}"#, "PrList"),
        (
            "pr.acknowledge",
            r#"{"projectRoot":"/work/agtk","pullRequestId":42,"marker":"review"}"#,
            "PrAcknowledge",
        ),
        (
            "pr.set_auto_review",
            r#"{"projectRoot":"/work/agtk","enabled":true}"#,
            "PrSetAutoReview",
        ),
        (
            "pr.launch_review",
            r#"{"projectRoot":"/work/agtk","pullRequestId":42}"#,
            "PrLaunchReview",
        ),
        (
            "claude.presets_set",
            r#"{"presets":[{"id":"opus-high","name":"Opus / high","model":"opus","effort":"high"}]}"#,
            "ClaudePresetsSet",
        ),
        ("claude.presets_get", r#"{}"#, "ClaudePresetsGet"),
        (
            "claude.preset_apply",
            r#"{"sessionId":"claude-1","presetId":"opus-high"}"#,
            "ClaudePresetApply",
        ),
        ("agent.list", "{}", "AgentList"),
        (
            "agent.preview",
            r#"{"provider":"codex","providerSessionId":"codex-1","maxMessages":20}"#,
            "AgentPreview",
        ),
        (
            "agent.restore",
            r#"{"provider":"claude","providerSessionId":"claude-1","cwd":"/work/agtk"}"#,
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
            "session.set_worktree",
            r#"{"sessionId":"shell-1","worktreePath":"/work/agtk-fix"}"#,
            "SessionSetWorktree",
        ),
        (
            "session.close",
            r#"{"sessionId":"shell-1","allowMissing":true}"#,
            "SessionClose",
        ),
        (
            "session.restart",
            r#"{"sessionId":"claude-1"}"#,
            "SessionRestart",
        ),
        ("session.fork", r#"{"sessionId":"claude-1"}"#, "SessionFork"),
        (
            "session.open_magit",
            r#"{"sessionId":"shell-1"}"#,
            "SessionOpenMagit",
        ),
        (
            "session.open_branch_review",
            r#"{"sessionId":"shell-1"}"#,
            "SessionOpenBranchReview",
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
        decode_request(br#"{"version":1,"id":"agent","method":"agent.restore","params":{"provider":"claude","providerSessionId":"claude-1","cwd":"/work/agtk","name":"Recovered"}}"#)
            .unwrap()
            .command,
        ControlCommand::AgentRestore(AgentRestoreParams {
            provider: AgentProvider::Claude,
            provider_session_id: "claude-1".to_owned(),
            cwd: Some("/work/agtk".into()),
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
            conversation_id: None,
            prompt: None,
        })
    );
    // A hook-reported prompt is kept as sent, minus surrounding whitespace.
    assert_eq!(
        decode_request(br#"{"version":1,"id":"agent","method":"session.set_state","params":{"sessionId":"claude-1","state":"busy","conversationId":"a3030a89","prompt":" fix the build\nthen test "}}"#)
            .unwrap()
            .command,
        ControlCommand::SessionSetState(SessionSetStateParams {
            session_id: "claude-1".to_owned(),
            state: AgentSignalState::Busy,
            conversation_id: Some("a3030a89".to_owned()),
            prompt: Some("fix the build\nthen test".to_owned()),
        })
    );
    assert_eq!(
        decode_request(br#"{"version":1,"id":"agent","method":"session.set_state","params":{"sessionId":"claude-1","state":"busy","prompt":"  "}}"#)
            .unwrap()
            .command,
        ControlCommand::SessionSetState(SessionSetStateParams {
            session_id: "claude-1".to_owned(),
            state: AgentSignalState::Busy,
            conversation_id: None,
            prompt: None,
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
    let list = decode_request(br#"{"version":1,"id":"wt","method":"worktree.list","params":{"projectRoot":"/work/agtk"}}"#)
        .expect("decode list");
    assert_eq!(
        list.command,
        ControlCommand::WorktreeList(WorktreeListParams {
            project_root: "/work/agtk".into(),
        })
    );

    let create = decode_request(br#"{"version":1,"id":"wt","method":"worktree.create","params":{"projectRoot":"/work/agtk","branch":"native-ui","baseBranch":"main","purpose":"Build native UI"}}"#)
        .expect("decode create");
    assert_eq!(
        create.command,
        ControlCommand::WorktreeCreate(WorktreeCreateParams {
            project_root: "/work/agtk".into(),
            branch: "native-ui".to_owned(),
            base_branch: Some("main".to_owned()),
            purpose: "Build native UI".to_owned(),
        })
    );

    let reap = decode_request(br#"{"version":1,"id":"wt","method":"worktree.reap","params":{"path":"/work/agtk-ui","expectedHead":"0123456789abcdef","expectedStatusHash":"abcdef","deleteBranch":"force"}}"#)
        .expect("decode reap");
    assert_eq!(
        reap.command,
        ControlCommand::WorktreeReap(WorktreeReapParams {
            path: "/work/agtk-ui".into(),
            expected_head: "0123456789abcdef".to_owned(),
            expected_status_hash: "abcdef".to_owned(),
            delete_branch: DeleteBranch::Force,
        })
    );

    for invalid in [
        br#"{"version":1,"id":"wt","method":"worktree.list","params":{"projectRoot":"relative"}}"#.as_slice(),
        br#"{"version":1,"id":"wt","method":"worktree.create","params":{"projectRoot":"/work/agtk","branch":"Bad_Branch","purpose":"work"}}"#.as_slice(),
        br#"{"version":1,"id":"wt","method":"worktree.create","params":{"projectRoot":"/work/agtk","branch":"good-branch","purpose":""}}"#.as_slice(),
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
    let request = decode_request(br#"{"version":1,"id":"project","method":"project.set","params":{"root":"/work/agtk","isPinned":true,"isCollapsed":false}}"#)
        .expect("decode project update");
    assert_eq!(
        request.command,
        ControlCommand::ProjectSet(ProjectSetParams {
            root: "/work/agtk".into(),
            is_pinned: Some(true),
            is_collapsed: Some(false),
        })
    );

    for invalid in [
        br#"{"version":1,"id":"project","method":"project.set","params":{"root":"relative","isPinned":true}}"#.as_slice(),
        br#"{"version":1,"id":"project","method":"project.set","params":{"root":"/work/agtk"}}"#.as_slice(),
    ] {
        assert_eq!(
            decode_request(invalid).unwrap_err().code,
            ErrorCode::InvalidParams
        );
    }
}

#[test]
fn project_removal_requires_an_absolute_root() {
    let request = decode_request(
        br#"{"version":1,"id":"project","method":"project.remove","params":{"root":"/work/agtk"}}"#,
    )
    .expect("decode project removal");
    assert_eq!(
        request.command,
        ControlCommand::ProjectRemove(ProjectRemoveParams {
            root: "/work/agtk".into(),
        })
    );

    for invalid in [
        br#"{"version":1,"id":"project","method":"project.remove","params":{"root":"relative"}}"#
            .as_slice(),
        br#"{"version":1,"id":"project","method":"project.remove","params":{}}"#.as_slice(),
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
    let request = decode_request(br#"{"version":1,"id":"theme","method":"appearance.set","params":{"theme":"tokyo-night","followSystem":true,"font":"Iosevka 12","uiFontSize":15}}"#)
        .expect("decode appearance update");
    assert_eq!(
        request.command,
        ControlCommand::AppearanceSet(AppearanceSetParams {
            theme: Some(ThemeKey::TokyoNight),
            follow_system: Some(true),
            font: Some("Iosevka 12".to_owned()),
            ui_font_size: Some(15),
            viewer_font_size: None,
        })
    );

    for invalid in [
        br#"{"version":1,"id":"theme","method":"appearance.set","params":{}}"#.as_slice(),
        br#"{"version":1,"id":"theme","method":"appearance.set","params":{"theme":"missing"}}"#
            .as_slice(),
        br#"{"version":1,"id":"theme","method":"appearance.set","params":{"font":""}}"#.as_slice(),
        br#"{"version":1,"id":"theme","method":"appearance.set","params":{"uiFontSize":25}}"#
            .as_slice(),
        br#"{"version":1,"id":"theme","method":"appearance.set","params":{"viewerFontSize":7}}"#
            .as_slice(),
        br#"{"version":1,"id":"theme","method":"appearance.set","params":{"viewerFontSize":49}}"#
            .as_slice(),
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

#[test]
fn provider_session_ids_cannot_pass_as_agent_flags() {
    for id in [
        "--dangerously-skip-permissions",
        "-c",
        "../log",
        "a b",
        "a;b",
    ] {
        let request = json!({
            "version": 1,
            "id": "agent",
            "method": "agent.restore",
            "params": {"provider": "claude", "providerSessionId": id},
        });
        let error = decode_request(request.to_string().as_bytes()).unwrap_err();
        assert_eq!(error.code, ErrorCode::InvalidParams, "{id}");
    }
    let uuid = "0199a2b3-c4d5-7e6f-8091-a2b3c4d5e6f7";
    let request = json!({
        "version": 1,
        "id": "agent",
        "method": "agent.restore",
        "params": {"provider": "codex", "providerSessionId": uuid},
    });
    assert!(decode_request(request.to_string().as_bytes()).is_ok());
}
