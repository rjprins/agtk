use std::process::Command;
use std::thread;

use agmux_native::control::{ControlCommand, ControlResponse, ControlServer, GetTextParams};
use serde_json::json;

#[test]
fn state_command_prints_the_application_result_as_json() {
    let directory = tempfile::tempdir().expect("create temporary directory");
    let socket = directory.path().join("agmux-native/test-cli/control.sock");
    let (_server, requests) = ControlServer::bind(&socket).expect("bind control server");
    let worker = thread::spawn(move || {
        let pending = requests.recv().expect("receive state request");
        assert_eq!(pending.request.command, ControlCommand::AppGetState);
        let request_id = pending.request.id.clone();
        pending
            .respond(ControlResponse::success(
                request_id,
                json!({ "instance": "test-cli", "sessions": [] }),
            ))
            .expect("respond to state request");
    });

    let output = Command::new(env!("CARGO_BIN_EXE_agmuxctl"))
        .args(["--instance", "test-cli", "state"])
        .env("AGMUX_RUNTIME_ROOT", directory.path())
        .output()
        .expect("run agmuxctl");

    assert!(
        output.status.success(),
        "agmuxctl failed: {}",
        String::from_utf8_lossy(&output.stderr)
    );
    assert_eq!(
        serde_json::from_slice::<serde_json::Value>(&output.stdout).expect("JSON stdout"),
        json!({ "instance": "test-cli", "sessions": [] })
    );
    assert!(output.stderr.is_empty());
    worker.join().expect("request worker");
}

#[test]
fn ui_inspect_command_requests_the_logical_widget_tree() {
    let directory = tempfile::tempdir().expect("create temporary directory");
    let socket = directory.path().join("agmux-native/test-cli/control.sock");
    let (_server, requests) = ControlServer::bind(&socket).expect("bind control server");
    let worker = thread::spawn(move || {
        let pending = requests.recv().expect("receive UI inspection request");
        assert_eq!(pending.request.command, ControlCommand::UiInspect);
        let request_id = pending.request.id.clone();
        pending
            .respond(ControlResponse::success(
                request_id,
                json!({ "root": { "id": "main-window" }, "isTruncated": false }),
            ))
            .expect("respond to UI inspection request");
    });

    let output = Command::new(env!("CARGO_BIN_EXE_agmuxctl"))
        .args(["--instance", "test-cli", "ui", "inspect"])
        .env("AGMUX_RUNTIME_ROOT", directory.path())
        .output()
        .expect("run agmuxctl");

    assert!(
        output.status.success(),
        "agmuxctl failed: {}",
        String::from_utf8_lossy(&output.stderr)
    );
    assert_eq!(
        serde_json::from_slice::<serde_json::Value>(&output.stdout).expect("JSON stdout"),
        json!({ "root": { "id": "main-window" }, "isTruncated": false })
    );
    assert!(output.stderr.is_empty());
    worker.join().expect("request worker");
}

#[test]
fn session_text_command_requests_a_bounded_snapshot() {
    let directory = tempfile::tempdir().expect("create temporary directory");
    let socket = directory.path().join("agmux-native/test-cli/control.sock");
    let (_server, requests) = ControlServer::bind(&socket).expect("bind control server");
    let worker = thread::spawn(move || {
        let pending = requests.recv().expect("receive terminal text request");
        assert_eq!(
            pending.request.command,
            ControlCommand::SessionGetText(GetTextParams {
                session_id: "shell-42".to_owned(),
                lines: 75,
            })
        );
        let request_id = pending.request.id.clone();
        pending
            .respond(ControlResponse::success(
                request_id,
                json!({
                    "sessionId": "shell-42",
                    "text": "ready",
                    "lines": 1,
                    "isTruncated": false
                }),
            ))
            .expect("respond to terminal text request");
    });

    let output = Command::new(env!("CARGO_BIN_EXE_agmuxctl"))
        .args([
            "--instance",
            "test-cli",
            "session",
            "text",
            "shell-42",
            "--lines",
            "75",
        ])
        .env("AGMUX_RUNTIME_ROOT", directory.path())
        .output()
        .expect("run agmuxctl");

    assert!(
        output.status.success(),
        "agmuxctl failed: {}",
        String::from_utf8_lossy(&output.stderr)
    );
    assert_eq!(
        serde_json::from_slice::<serde_json::Value>(&output.stdout).expect("JSON stdout"),
        json!({
            "sessionId": "shell-42",
            "text": "ready",
            "lines": 1,
            "isTruncated": false
        })
    );
    assert!(output.stderr.is_empty());
    worker.join().expect("request worker");
}

#[test]
fn connection_failure_uses_the_documented_exit_code() {
    let directory = tempfile::tempdir().expect("create temporary directory");

    let output = Command::new(env!("CARGO_BIN_EXE_agmuxctl"))
        .args(["--instance", "missing", "state"])
        .env("AGMUX_RUNTIME_ROOT", directory.path())
        .output()
        .expect("run agmuxctl");

    assert_eq!(output.status.code(), Some(3));
    assert!(output.stdout.is_empty());
    assert!(!output.stderr.is_empty());
}
