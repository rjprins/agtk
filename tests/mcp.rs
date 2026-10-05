use std::cell::RefCell;
use std::collections::VecDeque;
use std::fs;

use agtk::control::{
    ClaudePresetApplyParams, ControlCommand, CreateSessionParams, PrListParams, SendInputParams,
    SessionIdParams, SessionKind, SetSessionWorktreeParams, WorktreeCreateParams,
};
use agtk::mcp::{ControlBackend, McpServer};
use serde_json::{Value, json};

#[derive(Default)]
struct MockBackend {
    calls: RefCell<Vec<ControlCommand>>,
    results: RefCell<VecDeque<Result<Value, String>>>,
}

impl ControlBackend for MockBackend {
    fn call(&self, command: ControlCommand) -> Result<Value, String> {
        self.calls.borrow_mut().push(command);
        self.results
            .borrow_mut()
            .pop_front()
            .unwrap_or_else(|| Ok(json!({"ok":true})))
    }
}

fn request(id: i64, method: &str, params: Value) -> Value {
    json!({"jsonrpc":"2.0","id":id,"method":method,"params":params})
}

#[test]
fn legacy_initialize_and_tool_listing_are_compatible_and_deterministic() {
    let mut server = McpServer::new(MockBackend::default());
    let initialized = server
        .handle(request(
            1,
            "initialize",
            json!({
                "protocolVersion":"2025-11-25",
                "capabilities":{},
                "clientInfo":{"name":"test","version":"1"}
            }),
        ))
        .unwrap();
    assert_eq!(initialized["result"]["protocolVersion"], "2025-11-25");
    assert_eq!(initialized["result"]["capabilities"]["tools"], json!({}));

    let listed = server.handle(request(2, "tools/list", json!({}))).unwrap();
    let names = listed["result"]["tools"]
        .as_array()
        .unwrap()
        .iter()
        .map(|tool| tool["name"].as_str().unwrap())
        .collect::<Vec<_>>();
    assert_eq!(
        names,
        [
            "list_sessions",
            "send_input",
            "snapshot",
            "spawn_shell",
            "launch_agent",
            "fork_session",
            "select_session",
            "open_diff",
            "open_file",
            "kill_session",
            "rename_session",
            "set_session_worktree",
            "open_magit",
            "open_branch_review",
            "list_agent_sessions",
            "preview_agent_session",
            "restore_agent_session",
            "worktree_list",
            "worktree_create",
            "worktree_reap",
            "worktree_context",
            "list_pull_requests",
            "acknowledge_pull_request",
            "set_auto_review",
            "launch_pr_review",
            "list_claude_presets",
            "set_claude_presets",
            "apply_claude_preset",
            "inspect_ui",
            "capture_ui",
            "set_session_state",
        ]
    );
}

#[test]
fn modern_discovery_and_tool_results_include_required_result_shape() {
    let mut server = McpServer::new(MockBackend::default());
    let meta = json!({
        "_meta": {
            "io.modelcontextprotocol/protocolVersion":"2026-07-28",
            "io.modelcontextprotocol/clientCapabilities":{}
        }
    });
    let discovered = server
        .handle(request(1, "server/discover", meta.clone()))
        .unwrap();
    assert_eq!(discovered["result"]["resultType"], "complete");
    assert!(
        discovered["result"]["supportedVersions"]
            .as_array()
            .unwrap()
            .iter()
            .any(|version| version == "2026-07-28")
    );

    let listed = server.handle(request(2, "tools/list", meta)).unwrap();
    assert_eq!(listed["result"]["resultType"], "complete");
    assert_eq!(listed["result"]["ttlMs"], 300_000);
    assert_eq!(listed["result"]["cacheScope"], "public");
}

#[test]
fn send_input_maps_to_the_typed_local_control_protocol() {
    let backend = MockBackend::default();
    backend
        .results
        .borrow_mut()
        .push_back(Ok(json!({"bytesWritten":6})));
    let mut server = McpServer::new(backend);
    let response = server
        .handle(request(
            7,
            "tools/call",
            json!({
                "name":"send_input",
                "arguments":{"sessionId":"session-1","text":"hello","appendEnter":true}
            }),
        ))
        .unwrap();
    assert_eq!(response["result"]["isError"], false);
    assert_eq!(response["result"]["structuredContent"]["bytesWritten"], 6);
    assert_eq!(
        server.backend().calls.borrow().as_slice(),
        [ControlCommand::SessionSendInput(SendInputParams {
            session_id: "session-1".to_owned(),
            text: "hello".to_owned(),
            append_enter: true,
        })]
    );
}

#[test]
fn spawning_a_shell_passes_its_initial_command_as_input() {
    let mut server = McpServer::new(MockBackend::default());
    let response = server
        .handle(request(
            8,
            "tools/call",
            json!({
                "name":"spawn_shell",
                "arguments":{"cwd":"/work/agtk","initialCommand":"cargo test"}
            }),
        ))
        .unwrap();
    assert_eq!(response["result"]["isError"], false);
    assert_eq!(
        server.backend().calls.borrow().as_slice(),
        [ControlCommand::SessionCreate(CreateSessionParams {
            kind: SessionKind::Shell,
            command: None,
            args: Vec::new(),
            cwd: Some("/work/agtk".into()),
            name: None,
            project_root: None,
            worktree_path: None,
            initial_input: Some("cargo test".to_owned()),
        })]
    );
}

#[test]
fn forking_a_session_maps_to_the_typed_local_control_protocol() {
    let mut server = McpServer::new(MockBackend::default());
    let response = server
        .handle(request(
            8,
            "tools/call",
            json!({"name":"fork_session","arguments":{"sessionId":"claude-1"}}),
        ))
        .unwrap();
    assert_eq!(response["result"]["isError"], false);
    assert_eq!(
        server.backend().calls.borrow().as_slice(),
        [ControlCommand::SessionFork(SessionIdParams {
            session_id: "claude-1".to_owned(),
        })]
    );
}

#[test]
fn setting_a_session_worktree_maps_to_the_typed_local_control_protocol() {
    let mut server = McpServer::new(MockBackend::default());
    let response = server
        .handle(request(
            8,
            "tools/call",
            json!({
                "name":"set_session_worktree",
                "arguments":{"sessionId":"session-1","worktreePath":"/work/agtk-fix"}
            }),
        ))
        .unwrap();
    assert_eq!(response["result"]["isError"], false);
    assert_eq!(
        server.backend().calls.borrow().as_slice(),
        [ControlCommand::SessionSetWorktree(
            SetSessionWorktreeParams {
                session_id: "session-1".to_owned(),
                worktree_path: "/work/agtk-fix".into(),
            }
        )]
    );
}

#[test]
fn launching_an_agent_with_a_branch_creates_the_worktree_first() {
    let backend = MockBackend::default();
    backend.results.borrow_mut().push_back(Ok(json!({
        "repoRoot":"/work/agtk",
        "path":"/work/agtk-fix-login",
        "branch":"fix-login",
        "purpose":"Fix the login redirect"
    })));
    backend
        .results
        .borrow_mut()
        .push_back(Ok(json!({"id":"claude-1","isSelected":false})));
    let mut server = McpServer::new(backend);
    let response = server
        .handle(request(
            9,
            "tools/call",
            json!({
                "name":"launch_agent",
                "arguments":{
                    "provider":"claude",
                    "projectRoot":"/work/agtk",
                    "branch":"fix-login",
                    "purpose":"Fix the login redirect",
                    "initialInput":"Fix the login redirect"
                }
            }),
        ))
        .unwrap();
    assert_eq!(response["result"]["isError"], false);
    let text: Value =
        serde_json::from_str(response["result"]["content"][0]["text"].as_str().unwrap()).unwrap();
    assert_eq!(text["session"]["id"], "claude-1");
    assert_eq!(text["worktree"]["path"], "/work/agtk-fix-login");
    assert_eq!(
        server.backend().calls.borrow().as_slice(),
        [
            ControlCommand::WorktreeCreate(WorktreeCreateParams {
                project_root: "/work/agtk".into(),
                branch: "fix-login".to_owned(),
                base_branch: None,
                purpose: "Fix the login redirect".to_owned(),
            }),
            ControlCommand::SessionCreate(CreateSessionParams {
                kind: SessionKind::Claude,
                command: None,
                args: Vec::new(),
                cwd: Some("/work/agtk-fix-login".into()),
                name: None,
                project_root: Some("/work/agtk".into()),
                worktree_path: Some("/work/agtk-fix-login".into()),
                initial_input: Some("Fix the login redirect".to_owned()),
            }),
        ]
    );

    // A branch without a purpose, or next to an explicit worktree, is refused before any call.
    for arguments in [
        json!({"provider":"claude","projectRoot":"/work/agtk","branch":"fix-login"}),
        json!({"provider":"claude","projectRoot":"/work/agtk","branch":"fix-login","purpose":"x","worktreePath":"/elsewhere"}),
    ] {
        let calls_before = server.backend().calls.borrow().len();
        let response = server
            .handle(request(
                10,
                "tools/call",
                json!({"name":"launch_agent","arguments":arguments}),
            ))
            .unwrap();
        assert_eq!(response["result"]["isError"], true);
        assert_eq!(server.backend().calls.borrow().len(), calls_before);
    }
}

#[test]
fn pull_request_listing_maps_to_the_typed_local_control_protocol() {
    let mut server = McpServer::new(MockBackend::default());
    let response = server
        .handle(request(
            12,
            "tools/call",
            json!({
                "name":"list_pull_requests",
                "arguments":{"projectRoot":"/work/agtk"}
            }),
        ))
        .unwrap();
    assert_eq!(response["result"]["isError"], false);
    assert_eq!(
        server.backend().calls.borrow().as_slice(),
        &[ControlCommand::PrList(PrListParams {
            project_root: "/work/agtk".into(),
        })]
    );
}

#[test]
fn claude_preset_tools_map_to_the_typed_local_control_protocol() {
    let mut server = McpServer::new(MockBackend::default());
    let listed = server
        .handle(request(
            13,
            "tools/call",
            json!({"name":"list_claude_presets","arguments":{}}),
        ))
        .unwrap();
    assert_eq!(listed["result"]["isError"], false);

    let applied = server
        .handle(request(
            14,
            "tools/call",
            json!({
                "name":"apply_claude_preset",
                "arguments":{"sessionId":"claude-1","presetId":"opus-high"}
            }),
        ))
        .unwrap();
    assert_eq!(applied["result"]["isError"], false);
    assert_eq!(
        server.backend().calls.borrow().as_slice(),
        &[
            ControlCommand::ClaudePresetsGet,
            ControlCommand::ClaudePresetApply(ClaudePresetApplyParams {
                session_id: "claude-1".to_owned(),
                preset_id: "opus-high".to_owned(),
            }),
        ]
    );
}

#[test]
fn capture_tool_returns_app_png_as_image_content() {
    let directory = tempfile::tempdir().unwrap();
    let path = directory.path().join("capture.png");
    fs::write(&path, b"\x89PNG\r\n\x1a\nprivate-app-capture").unwrap();
    let backend = MockBackend::default();
    backend.results.borrow_mut().push_back(Ok(json!({
        "path":path,
        "width":1200,
        "height":800,
        "sha256":"digest"
    })));
    let mut server = McpServer::new(backend);
    let response = server
        .handle(request(
            9,
            "tools/call",
            json!({"name":"capture_ui","arguments":{}}),
        ))
        .unwrap();
    assert_eq!(response["result"]["content"][1]["type"], "image");
    assert_eq!(response["result"]["content"][1]["mimeType"], "image/png");
    assert!(
        response["result"]["content"][1]["data"]
            .as_str()
            .unwrap()
            .starts_with("iVBOR")
    );
}

#[test]
fn backend_failures_are_reported_as_tool_errors_not_json_rpc_failures() {
    let backend = MockBackend::default();
    backend
        .results
        .borrow_mut()
        .push_back(Err("native control socket is unavailable".to_owned()));
    let mut server = McpServer::new(backend);
    let response = server
        .handle(request(
            11,
            "tools/call",
            json!({"name":"list_sessions","arguments":{}}),
        ))
        .unwrap();
    assert_eq!(response["result"]["isError"], true);
    assert!(
        response["result"]["content"][0]["text"]
            .as_str()
            .unwrap()
            .contains("unavailable")
    );
}

#[test]
fn an_oversized_line_is_rejected_and_the_next_message_still_served() {
    let mut input = vec![b'x'; 3 * 1024 * 1024];
    input.push(b'\n');
    input.extend(request(2, "tools/list", json!({})).to_string().bytes());
    input.push(b'\n');
    let mut output = Vec::new();
    McpServer::new(MockBackend::default())
        .serve(std::io::BufReader::new(input.as_slice()), &mut output)
        .unwrap();

    let replies = String::from_utf8(output).unwrap();
    let replies = replies
        .lines()
        .map(|line| serde_json::from_str::<Value>(line).unwrap())
        .collect::<Vec<_>>();
    assert_eq!(replies.len(), 2);
    assert_eq!(replies[0]["error"]["code"], -32600);
    assert_eq!(replies[1]["id"], 2);
}
