use std::io::Write;
use std::process::{Command, Stdio};
use std::thread;

use agmux_native::control::{ControlCommand, ControlResponse, ControlServer};
use serde_json::{Value, json};

#[test]
fn stdio_server_bridges_a_real_mcp_tool_call_to_the_native_socket() {
    let directory = tempfile::tempdir().unwrap();
    let socket = directory
        .path()
        .join("agmux-native/mcp-integration/control.sock");
    let (_server, requests) = ControlServer::bind(&socket).unwrap();
    let responder = thread::spawn(move || {
        let pending = requests.recv().unwrap();
        assert_eq!(pending.request.command, ControlCommand::AppGetState);
        let id = pending.request.id.clone();
        pending
            .respond(ControlResponse::success(
                id,
                json!({
                    "selectedSessionId":"session-1",
                    "sessions":[{"id":"session-1","name":"agent"}],
                    "attention":{"count":1}
                }),
            ))
            .unwrap();
    });

    let mut child = Command::new(env!("CARGO_BIN_EXE_agmux-mcp"))
        .args(["--instance", "mcp-integration"])
        .env("AGMUX_RUNTIME_ROOT", directory.path())
        .stdin(Stdio::piped())
        .stdout(Stdio::piped())
        .stderr(Stdio::piped())
        .spawn()
        .unwrap();
    let mut stdin = child.stdin.take().unwrap();
    for message in [
        json!({
            "jsonrpc":"2.0","id":1,"method":"initialize","params":{
                "protocolVersion":"2025-11-25","capabilities":{},
                "clientInfo":{"name":"test","version":"1"}
            }
        }),
        json!({"jsonrpc":"2.0","method":"notifications/initialized"}),
        json!({"jsonrpc":"2.0","id":2,"method":"tools/list","params":{}}),
        json!({"jsonrpc":"2.0","id":3,"method":"tools/call","params":{"name":"list_sessions","arguments":{}}}),
    ] {
        writeln!(stdin, "{}", serde_json::to_string(&message).unwrap()).unwrap();
    }
    drop(stdin);
    let output = child.wait_with_output().unwrap();
    assert!(
        output.status.success(),
        "agmux-mcp failed: {}",
        String::from_utf8_lossy(&output.stderr)
    );
    let responses = String::from_utf8(output.stdout)
        .unwrap()
        .lines()
        .map(|line| serde_json::from_str::<Value>(line).unwrap())
        .collect::<Vec<_>>();
    assert_eq!(responses.len(), 3);
    assert_eq!(responses[0]["id"], 1);
    assert_eq!(responses[1]["id"], 2);
    assert_eq!(responses[2]["id"], 3);
    assert_eq!(
        responses[2]["result"]["structuredContent"]["sessions"][0]["id"],
        "session-1"
    );
    assert!(output.stderr.is_empty());
    responder.join().unwrap();
}
