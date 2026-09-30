use std::cell::RefCell;
use std::collections::VecDeque;
use std::fs;

use agtk::control::{ClaudePresetApplyParams, ControlCommand, PrListParams, SendInputParams};
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
            "select_session",
            "open_diff",
            "open_file",
            "kill_session",
            "rename_session",
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
