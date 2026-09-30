use std::process::Command;
use std::thread;
use std::time::Duration;

use agtk::control::{ControlCommand, ControlResponse, ControlServer, GetTextParams};
use serde_json::json;

#[test]
fn wait_prints_the_observation_that_satisfies_terminal_text() {
    let directory = tempfile::tempdir().expect("create temporary directory");
    let socket = directory.path().join("agtk/wait-success/control.sock");
    let (_server, requests) = ControlServer::bind(&socket).expect("bind control server");
    let worker = thread::spawn(move || {
        for text in ["starting", "build complete"] {
            let pending = requests.recv().expect("receive wait observation");
            assert_eq!(
                pending.request.command,
                ControlCommand::SessionGetText(GetTextParams {
                    session_id: "shell-7".to_owned(),
                    lines: 80,
                })
            );
            let request_id = pending.request.id.clone();
            pending
                .respond(ControlResponse::success(
                    request_id,
                    json!({ "sessionId": "shell-7", "text": text }),
                ))
                .expect("respond to wait observation");
        }
    });

    let output = Command::new(env!("CARGO_BIN_EXE_agtkctl"))
        .args([
            "--instance",
            "wait-success",
            "wait",
            "--session",
            "shell-7",
            "--text",
            "complete",
            "--lines",
            "80",
            "--timeout-ms",
            "1000",
            "--poll-ms",
            "1",
        ])
        .env("AGTK_RUNTIME_ROOT", directory.path())
        .output()
        .expect("run agtkctl wait");

    assert!(
        output.status.success(),
        "agtkctl wait failed: {}",
        String::from_utf8_lossy(&output.stderr)
    );
    assert_eq!(
        serde_json::from_slice::<serde_json::Value>(&output.stdout).expect("JSON stdout"),
        json!({ "sessionId": "shell-7", "text": "build complete" })
    );
    worker.join().expect("request worker");
}

#[test]
fn wait_uses_exit_code_four_when_the_deadline_expires() {
    let directory = tempfile::tempdir().expect("create temporary directory");
    let socket = directory.path().join("agtk/wait-timeout/control.sock");
    let (_server, requests) = ControlServer::bind(&socket).expect("bind control server");
    let worker = thread::spawn(move || {
        while let Ok(pending) = requests.recv_timeout(Duration::from_millis(50)) {
            assert_eq!(pending.request.command, ControlCommand::AppGetState);
            let request_id = pending.request.id.clone();
            pending
                .respond(ControlResponse::success(
                    request_id,
                    json!({ "selectedSessionId": null, "sessions": [] }),
                ))
                .expect("respond to wait observation");
        }
    });

    let output = Command::new(env!("CARGO_BIN_EXE_agtkctl"))
        .args([
            "--instance",
            "wait-timeout",
            "wait",
            "--session",
            "missing",
            "--exists",
            "--timeout-ms",
            "20",
            "--poll-ms",
            "1",
        ])
        .env("AGTK_RUNTIME_ROOT", directory.path())
        .output()
        .expect("run agtkctl wait");

    assert_eq!(output.status.code(), Some(4));
    assert!(output.stdout.is_empty());
    assert!(String::from_utf8_lossy(&output.stderr).contains("timed out"));
    worker.join().expect("request worker");
}

#[test]
fn wait_supports_selected_session_and_session_state_conditions() {
    let directory = tempfile::tempdir().expect("create temporary directory");
    let socket = directory.path().join("agtk/wait-state/control.sock");
    let (_server, requests) = ControlServer::bind(&socket).expect("bind control server");
    let worker = thread::spawn(move || {
        for _ in 0..2 {
            let pending = requests.recv().expect("receive state observation");
            assert_eq!(pending.request.command, ControlCommand::AppGetState);
            let request_id = pending.request.id.clone();
            pending
                .respond(ControlResponse::success(
                    request_id,
                    json!({
                        "selectedSessionId": "shell-7",
                        "sessions": [{ "id": "shell-7", "state": "ready" }]
                    }),
                ))
                .expect("respond to state observation");
        }
    });

    let selected = Command::new(env!("CARGO_BIN_EXE_agtkctl"))
        .args([
            "--instance",
            "wait-state",
            "wait",
            "--selected",
            "shell-7",
            "--timeout-ms",
            "1000",
        ])
        .env("AGTK_RUNTIME_ROOT", directory.path())
        .output()
        .expect("wait for selected session");
    assert!(selected.status.success());

    let ready = Command::new(env!("CARGO_BIN_EXE_agtkctl"))
        .args([
            "--instance",
            "wait-state",
            "wait",
            "--session",
            "shell-7",
            "--state",
            "ready",
            "--timeout-ms",
            "1000",
        ])
        .env("AGTK_RUNTIME_ROOT", directory.path())
        .output()
        .expect("wait for session state");
    assert!(ready.status.success());

    worker.join().expect("request worker");
}
