use std::path::PathBuf;
use std::process::{Command, Output};
use std::thread;

use agmux_native::appearance::ThemeKey;
use agmux_native::control::{
    AppearanceSetParams, CloseSessionParams, ControlCommand, ControlResponse, ControlServer,
    CreateSessionParams, GetTextParams, ProjectSetParams, RenameSessionParams, SendInputParams,
    SessionIdParams, SessionKind, ShortcutSetParams, UiShowParams, UiSurface, WorktreeCreateParams,
    WorktreeListParams, WorktreeReapParams,
};
use agmux_native::shortcuts::ShortcutAction;
use agmux_native::worktrees::DeleteBranch;
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
fn ui_capture_command_requests_an_app_only_png() {
    let directory = tempfile::tempdir().expect("create temporary directory");
    let socket = directory.path().join("agmux-native/test-cli/control.sock");
    let (_server, requests) = ControlServer::bind(&socket).expect("bind control server");
    let worker = thread::spawn(move || {
        let pending = requests.recv().expect("receive UI capture request");
        assert_eq!(pending.request.command, ControlCommand::UiCapture);
        let request_id = pending.request.id.clone();
        pending
            .respond(ControlResponse::success(
                request_id,
                json!({ "path": "/tmp/capture.png", "width": 1200, "height": 800 }),
            ))
            .expect("respond to UI capture request");
    });

    let output = Command::new(env!("CARGO_BIN_EXE_agmuxctl"))
        .args(["--instance", "test-cli", "ui", "capture"])
        .env("AGMUX_RUNTIME_ROOT", directory.path())
        .output()
        .expect("run agmuxctl");

    assert!(
        output.status.success(),
        "agmuxctl failed: {}",
        String::from_utf8_lossy(&output.stderr)
    );
    assert!(output.stderr.is_empty());
    worker.join().expect("request worker");
}

#[test]
fn ui_show_command_opens_a_named_transient_surface() {
    assert_fixture_command(
        &["ui", "show", "launch"],
        ControlCommand::UiShow(UiShowParams {
            surface: UiSurface::Launch,
        }),
    );
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

#[test]
fn session_mutation_commands_have_typed_protocol_mappings() {
    assert_fixture_command(
        &[
            "session",
            "create",
            "--kind",
            "custom",
            "--command",
            "/bin/printf",
            "--arg",
            "ready\\n",
            "--cwd",
            "/tmp",
            "--name",
            "Probe",
            "--initial-input",
            "hello",
        ],
        ControlCommand::SessionCreate(CreateSessionParams {
            kind: SessionKind::Custom,
            command: Some("/bin/printf".to_owned()),
            args: vec!["ready\\n".to_owned()],
            cwd: Some(PathBuf::from("/tmp")),
            name: Some("Probe".to_owned()),
            project_root: None,
            worktree_path: None,
            initial_input: Some("hello".to_owned()),
        }),
    );
    assert_fixture_command(
        &["session", "select", "shell-42"],
        ControlCommand::SessionSelect(SessionIdParams {
            session_id: "shell-42".to_owned(),
        }),
    );
    assert_fixture_command(
        &[
            "session",
            "input",
            "shell-42",
            "--text",
            "status",
            "--no-enter",
        ],
        ControlCommand::SessionSendInput(SendInputParams {
            session_id: "shell-42".to_owned(),
            text: "status".to_owned(),
            append_enter: false,
        }),
    );
    assert_fixture_command(
        &["session", "rename", "shell-42", "--name", "Build"],
        ControlCommand::SessionRename(RenameSessionParams {
            session_id: "shell-42".to_owned(),
            name: "Build".to_owned(),
        }),
    );
    assert_fixture_command(
        &["session", "close", "shell-42", "--allow-missing"],
        ControlCommand::SessionClose(CloseSessionParams {
            session_id: "shell-42".to_owned(),
            allow_missing: true,
        }),
    );
}

#[test]
fn appearance_set_command_has_a_typed_protocol_mapping() {
    assert_fixture_command(
        &[
            "appearance",
            "set",
            "--theme",
            "solarized-dark",
            "--follow-system",
            "true",
            "--font",
            "Iosevka 12",
        ],
        ControlCommand::AppearanceSet(AppearanceSetParams {
            theme: Some(ThemeKey::SolarizedDark),
            follow_system: Some(true),
            font: Some("Iosevka 12".to_owned()),
        }),
    );
}

#[test]
fn shortcut_commands_have_typed_protocol_mappings() {
    assert_fixture_command(
        &[
            "shortcut",
            "set",
            "--action",
            "toggle-sidebar",
            "--accelerator",
            "<Alt>b",
        ],
        ControlCommand::ShortcutSet(ShortcutSetParams {
            action: ShortcutAction::ToggleSidebar,
            accelerator: Some("<Alt>b".to_owned()),
            reset: false,
        }),
    );
    assert_fixture_command(
        &["shortcut", "reset", "--action", "toggle-sidebar"],
        ControlCommand::ShortcutSet(ShortcutSetParams {
            action: ShortcutAction::ToggleSidebar,
            accelerator: None,
            reset: true,
        }),
    );
}

#[test]
fn project_set_command_has_a_typed_protocol_mapping() {
    assert_fixture_command(
        &[
            "project",
            "set",
            "/work/agmux",
            "--pinned",
            "true",
            "--collapsed",
            "false",
        ],
        ControlCommand::ProjectSet(ProjectSetParams {
            root: "/work/agmux".into(),
            is_pinned: Some(true),
            is_collapsed: Some(false),
        }),
    );
}

#[test]
fn worktree_commands_have_typed_protocol_mappings() {
    assert_fixture_command(
        &["worktree", "list", "/work/agmux"],
        ControlCommand::WorktreeList(WorktreeListParams {
            project_root: "/work/agmux".into(),
        }),
    );
    assert_fixture_command(
        &[
            "worktree",
            "create",
            "/work/agmux",
            "--branch",
            "native-ui",
            "--base",
            "main",
            "--purpose",
            "Build native UI",
        ],
        ControlCommand::WorktreeCreate(WorktreeCreateParams {
            project_root: "/work/agmux".into(),
            branch: "native-ui".to_owned(),
            base_branch: Some("main".to_owned()),
            purpose: "Build native UI".to_owned(),
        }),
    );
    assert_fixture_command(
        &[
            "worktree",
            "reap",
            "/work/agmux-native-ui",
            "--expected-head",
            "0123456789abcdef",
            "--expected-status-hash",
            "abcdef",
            "--delete-branch",
            "force",
        ],
        ControlCommand::WorktreeReap(WorktreeReapParams {
            path: "/work/agmux-native-ui".into(),
            expected_head: "0123456789abcdef".to_owned(),
            expected_status_hash: "abcdef".to_owned(),
            delete_branch: DeleteBranch::Force,
        }),
    );
}

fn assert_fixture_command(arguments: &[&str], expected: ControlCommand) -> Output {
    let directory = tempfile::tempdir().expect("create temporary directory");
    let socket = directory.path().join("agmux-native/test-cli/control.sock");
    let (_server, requests) = ControlServer::bind(&socket).expect("bind control server");
    let worker = thread::spawn(move || {
        let pending = requests.recv().expect("receive CLI request");
        assert_eq!(pending.request.command, expected);
        let request_id = pending.request.id.clone();
        pending
            .respond(ControlResponse::success(request_id, json!({ "ok": true })))
            .expect("respond to CLI request");
    });

    let output = Command::new(env!("CARGO_BIN_EXE_agmuxctl"))
        .args(["--instance", "test-cli"])
        .args(arguments)
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
        json!({ "ok": true })
    );
    worker.join().expect("request worker");
    output
}
