use std::path::PathBuf;
use std::process::{Command, Output};
use std::thread;

use agtk::appearance::ThemeKey;
use agtk::azure::PrAttention;
use agtk::claude_presets::{ClaudeEffort, ClaudeModelPreset};
use agtk::control::{
    AgentListParams, AgentPreviewParams, AgentRestoreParams, AgentSignalState, AppearanceSetParams,
    ClaudePresetApplyParams, ClaudePresetsSetParams, CloseSessionParams, ControlCommand,
    ControlResponse, ControlServer, CreateSessionParams, GetTextParams, PrAcknowledgeParams,
    ProjectRemoveParams, ProjectSetParams, RenameSessionParams, SendInputParams, SessionIdParams,
    SessionKind, SessionSetStateParams, SetSessionWorktreeParams, ShortcutSetParams, UiShowParams,
    UiSurface, WorktreeCreateParams, WorktreeListParams, WorktreeReapParams,
};
use agtk::providers::AgentProvider;
use agtk::shortcuts::ShortcutAction;
use agtk::worktrees::DeleteBranch;
use serde_json::json;

#[test]
fn state_command_prints_the_application_result_as_json() {
    let directory = tempfile::tempdir().expect("create temporary directory");
    let socket = directory.path().join("agtk/test-cli/control.sock");
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

    let output = Command::new(env!("CARGO_BIN_EXE_agtkctl"))
        .args(["--instance", "test-cli", "state"])
        .env("AGTK_RUNTIME_ROOT", directory.path())
        .output()
        .expect("run agtkctl");

    assert!(
        output.status.success(),
        "agtkctl failed: {}",
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
    let socket = directory.path().join("agtk/test-cli/control.sock");
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

    let output = Command::new(env!("CARGO_BIN_EXE_agtkctl"))
        .args(["--instance", "test-cli", "ui", "inspect"])
        .env("AGTK_RUNTIME_ROOT", directory.path())
        .output()
        .expect("run agtkctl");

    assert!(
        output.status.success(),
        "agtkctl failed: {}",
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
    let socket = directory.path().join("agtk/test-cli/control.sock");
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

    let output = Command::new(env!("CARGO_BIN_EXE_agtkctl"))
        .args(["--instance", "test-cli", "ui", "capture"])
        .env("AGTK_RUNTIME_ROOT", directory.path())
        .output()
        .expect("run agtkctl");

    assert!(
        output.status.success(),
        "agtkctl failed: {}",
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
fn pr_acknowledge_command_preserves_the_exact_attention_marker() {
    assert_fixture_command(
        &["pr", "acknowledge", "/work/agtk", "42", "review"],
        ControlCommand::PrAcknowledge(PrAcknowledgeParams {
            project_root: "/work/agtk".into(),
            pull_request_id: 42,
            marker: PrAttention::Review,
        }),
    );
}

#[test]
fn claude_presets_command_has_a_typed_protocol_mapping() {
    assert_fixture_command(&["claude", "presets"], ControlCommand::ClaudePresetsGet);
    assert_fixture_command(
        &[
            "claude",
            "presets",
            "--json",
            r#"[{"id":"focused","name":"Focused","model":"opus","effort":"high"}]"#,
        ],
        ControlCommand::ClaudePresetsSet(ClaudePresetsSetParams {
            presets: vec![ClaudeModelPreset {
                id: "focused".to_owned(),
                name: "Focused".to_owned(),
                model: "opus".to_owned(),
                effort: ClaudeEffort::High,
            }],
        }),
    );
    assert_fixture_command(
        &["claude", "apply", "claude-1", "focused"],
        ControlCommand::ClaudePresetApply(ClaudePresetApplyParams {
            session_id: "claude-1".to_owned(),
            preset_id: "focused".to_owned(),
        }),
    );
}

#[test]
fn session_text_command_requests_a_bounded_snapshot() {
    let directory = tempfile::tempdir().expect("create temporary directory");
    let socket = directory.path().join("agtk/test-cli/control.sock");
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

    let output = Command::new(env!("CARGO_BIN_EXE_agtkctl"))
        .args([
            "--instance",
            "test-cli",
            "session",
            "text",
            "shell-42",
            "--lines",
            "75",
        ])
        .env("AGTK_RUNTIME_ROOT", directory.path())
        .output()
        .expect("run agtkctl");

    assert!(
        output.status.success(),
        "agtkctl failed: {}",
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

    let output = Command::new(env!("CARGO_BIN_EXE_agtkctl"))
        .args(["--instance", "missing", "state"])
        .env("AGTK_RUNTIME_ROOT", directory.path())
        .output()
        .expect("run agtkctl");

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
        &[
            "session",
            "worktree",
            "shell-42",
            "--path",
            "/work/agtk-fix",
        ],
        ControlCommand::SessionSetWorktree(SetSessionWorktreeParams {
            session_id: "shell-42".to_owned(),
            worktree_path: "/work/agtk-fix".into(),
        }),
    );
    assert_fixture_command(
        &["session", "close", "shell-42", "--allow-missing"],
        ControlCommand::SessionClose(CloseSessionParams {
            session_id: "shell-42".to_owned(),
            allow_missing: true,
        }),
    );
    assert_fixture_command(
        &["session", "fork", "claude-42"],
        ControlCommand::SessionFork(SessionIdParams {
            session_id: "claude-42".to_owned(),
        }),
    );
    assert_fixture_command(
        &["session", "magit", "shell-42"],
        ControlCommand::SessionOpenMagit(SessionIdParams {
            session_id: "shell-42".to_owned(),
        }),
    );
    assert_fixture_command(
        &["session", "review", "shell-42"],
        ControlCommand::SessionOpenBranchReview(SessionIdParams {
            session_id: "shell-42".to_owned(),
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
            "--ui-font-size",
            "15",
            "--viewer-font-size",
            "16",
        ],
        ControlCommand::AppearanceSet(AppearanceSetParams {
            theme: Some(ThemeKey::SolarizedDark),
            follow_system: Some(true),
            font: Some("Iosevka 12".to_owned()),
            ui_font_size: Some(15),
            viewer_font_size: Some(16),
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
            "/work/agtk",
            "--pinned",
            "true",
            "--collapsed",
            "false",
        ],
        ControlCommand::ProjectSet(ProjectSetParams {
            root: "/work/agtk".into(),
            is_pinned: Some(true),
            is_collapsed: Some(false),
        }),
    );
    assert_fixture_command(
        &["project", "remove", "/work/agtk"],
        ControlCommand::ProjectRemove(ProjectRemoveParams {
            root: "/work/agtk".into(),
        }),
    );
}

#[test]
fn worktree_commands_have_typed_protocol_mappings() {
    assert_fixture_command(
        &["worktree", "list", "/work/agtk"],
        ControlCommand::WorktreeList(WorktreeListParams {
            project_root: "/work/agtk".into(),
        }),
    );
    assert_fixture_command(
        &[
            "worktree",
            "create",
            "/work/agtk",
            "--branch",
            "native-ui",
            "--base",
            "main",
            "--purpose",
            "Build native UI",
        ],
        ControlCommand::WorktreeCreate(WorktreeCreateParams {
            project_root: "/work/agtk".into(),
            branch: "native-ui".to_owned(),
            base_branch: Some("main".to_owned()),
            purpose: "Build native UI".to_owned(),
        }),
    );
    assert_fixture_command(
        &[
            "worktree",
            "reap",
            "/work/agtk-ui",
            "--expected-head",
            "0123456789abcdef",
            "--expected-status-hash",
            "abcdef",
            "--delete-branch",
            "force",
        ],
        ControlCommand::WorktreeReap(WorktreeReapParams {
            path: "/work/agtk-ui".into(),
            expected_head: "0123456789abcdef".to_owned(),
            expected_status_hash: "abcdef".to_owned(),
            delete_branch: DeleteBranch::Force,
        }),
    );
}

#[test]
fn agent_and_readiness_commands_have_typed_protocol_mappings() {
    assert_fixture_command(
        &["agent", "list", "--limit", "25", "--max-age-days", "30"],
        ControlCommand::AgentList(AgentListParams {
            limit: 25,
            max_age_days: 30,
        }),
    );
    assert_fixture_command(
        &[
            "agent",
            "preview",
            "codex",
            "codex-1",
            "--max-messages",
            "20",
        ],
        ControlCommand::AgentPreview(AgentPreviewParams {
            provider: AgentProvider::Codex,
            provider_session_id: "codex-1".to_owned(),
            max_messages: 20,
        }),
    );
    assert_fixture_command(
        &[
            "agent",
            "restore",
            "claude",
            "claude-1",
            "--cwd",
            "/work/agtk",
            "--project-root",
            "/work/agtk",
            "--name",
            "Recovered",
        ],
        ControlCommand::AgentRestore(AgentRestoreParams {
            provider: AgentProvider::Claude,
            provider_session_id: "claude-1".to_owned(),
            cwd: Some("/work/agtk".into()),
            project_root: Some("/work/agtk".into()),
            worktree_path: None,
            name: Some("Recovered".to_owned()),
        }),
    );
    assert_fixture_command(
        &["session", "state", "claude-1", "ready"],
        ControlCommand::SessionSetState(SessionSetStateParams {
            session_id: "claude-1".to_owned(),
            state: AgentSignalState::Ready,
            conversation_id: None,
            prompt: None,
        }),
    );
}

fn assert_fixture_command(arguments: &[&str], expected: ControlCommand) -> Output {
    let directory = tempfile::tempdir().expect("create temporary directory");
    let socket = directory.path().join("agtk/test-cli/control.sock");
    let (_server, requests) = ControlServer::bind(&socket).expect("bind control server");
    let worker = thread::spawn(move || {
        let pending = requests.recv().expect("receive CLI request");
        assert_eq!(pending.request.command, expected);
        let request_id = pending.request.id.clone();
        pending
            .respond(ControlResponse::success(request_id, json!({ "ok": true })))
            .expect("respond to CLI request");
    });

    let output = Command::new(env!("CARGO_BIN_EXE_agtkctl"))
        .args(["--instance", "test-cli"])
        .args(arguments)
        .env("AGTK_RUNTIME_ROOT", directory.path())
        .output()
        .expect("run agtkctl");

    assert!(
        output.status.success(),
        "agtkctl failed: {}",
        String::from_utf8_lossy(&output.stderr)
    );
    assert_eq!(
        serde_json::from_slice::<serde_json::Value>(&output.stdout).expect("JSON stdout"),
        json!({ "ok": true })
    );
    worker.join().expect("request worker");
    output
}
