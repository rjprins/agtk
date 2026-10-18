use std::fs;
use std::path::Path;
use std::time::Duration;

use agtk::agent_hooks::{
    HOOK_TIMEOUT_SECONDS, claude_hook_settings, context_report_path, ensure_claude_hook_settings,
    has_own_status_line,
};
use agtk::control::{AgentSignalState, ControlCommand, SessionSetStateParams, control_timeout};

fn command(settings: &serde_json::Value, event: &str) -> String {
    settings["hooks"][event][0]["hooks"][0]["command"]
        .as_str()
        .expect("hook command")
        .to_owned()
}

#[test]
fn claude_hooks_report_each_turn_boundary() {
    let settings = claude_hook_settings(Path::new("/opt/agtk/agtkctl"), None);
    for (event, state) in [
        ("UserPromptSubmit", "busy"),
        ("PreToolUse", "busy"),
        ("PostToolUse", "busy"),
        ("Notification", "waiting"),
        ("Stop", "ready"),
        ("StopFailure", "waiting"),
        ("SessionStart", "idle"),
    ] {
        let command = command(&settings, event);
        assert!(
            command.starts_with(r#"[ -z "$AGTK_SESSION_ID" ] || '/opt/agtk/agtkctl' session state "$AGTK_SESSION_ID" "#),
            "{event}: {command}"
        );
        assert!(
            command.ends_with(&format!(" {state} --hook-input >/dev/null 2>&1 || true")),
            "{event}: {command}"
        );
    }
    // The idle reminder does not block the turn, so only real prompts wait.
    assert_eq!(
        settings["hooks"]["Notification"][0]["matcher"],
        "permission_prompt|elicitation_dialog"
    );
}

#[test]
fn hooks_outlast_the_agtkctl_reply_deadline() {
    let settings = claude_hook_settings(Path::new("/opt/agtk/agtkctl"), None);
    let deadline = control_timeout(&ControlCommand::SessionSetState(SessionSetStateParams {
        session_id: "claude-1".to_owned(),
        state: AgentSignalState::Busy,
        conversation_id: None,
        prompt: None,
    }));
    // Otherwise Claude Code kills a hook that agtkctl would have given up on quietly.
    assert!(Duration::from_secs(HOOK_TIMEOUT_SECONDS) > deadline);
    for event in ["UserPromptSubmit", "PreToolUse", "Stop"] {
        assert_eq!(
            settings["hooks"][event][0]["hooks"][0]["timeout"],
            HOOK_TIMEOUT_SECONDS
        );
    }
}

#[test]
fn hook_commands_quote_the_agtkctl_path() {
    let settings = claude_hook_settings(Path::new("/tmp/it's here/agtkctl"), None);
    assert!(command(&settings, "Stop").contains(r"'/tmp/it'\''s here/agtkctl'"));
}

#[test]
fn settings_file_is_written_once_and_kept() {
    let directory = tempfile::tempdir().expect("temporary directory");
    let agtkctl = Path::new("/opt/agtk/agtkctl");
    let path =
        ensure_claude_hook_settings(directory.path(), agtkctl, None).expect("write settings");
    let written: serde_json::Value =
        serde_json::from_slice(&fs::read(&path).expect("read settings")).expect("valid json");
    assert_eq!(written, claude_hook_settings(agtkctl, None));

    let modified = fs::metadata(&path)
        .and_then(|m| m.modified())
        .expect("mtime");
    let again = ensure_claude_hook_settings(directory.path(), agtkctl, None).expect("rewrite");
    assert_eq!(again, path);
    let unchanged = fs::metadata(&path)
        .and_then(|m| m.modified())
        .expect("mtime");
    assert_eq!(modified, unchanged);
}

fn run_status_line(settings: &serde_json::Value, session_id: Option<&str>) -> Vec<u8> {
    use std::io::Write;
    use std::process::{Command, Stdio};
    let mut command = Command::new("sh");
    command
        .arg("-c")
        .arg(
            settings["statusLine"]["command"]
                .as_str()
                .expect("status line"),
        )
        .env_remove("AGTK_SESSION_ID")
        .stdin(Stdio::piped())
        .stdout(Stdio::piped());
    if let Some(id) = session_id {
        command.env("AGTK_SESSION_ID", id);
    }
    let mut child = command.spawn().expect("run status line");
    child
        .stdin
        .take()
        .unwrap()
        .write_all(br#"{"context_window":{"context_window_size":1000000}}"#)
        .unwrap();
    let output = child.wait_with_output().expect("status line exits");
    assert!(output.status.success());
    output.stdout
}

#[test]
fn status_line_saves_the_context_report_beside_the_socket_and_prints_nothing() {
    let directory = tempfile::tempdir().expect("temporary directory");
    let sessions = directory.path().join("it's sessions");
    fs::create_dir(&sessions).unwrap();
    let settings = claude_hook_settings(Path::new("/opt/agtk/agtkctl"), Some(&sessions));
    assert_eq!(settings["statusLine"]["type"], "command");

    assert!(run_status_line(&settings, Some("claude-1")).is_empty());
    let report = fs::read(context_report_path(&sessions.join("claude-1.sock"))).expect("report");
    assert_eq!(
        report,
        br#"{"context_window":{"context_window_size":1000000}}"#
    );
    assert_eq!(fs::read_dir(&sessions).unwrap().count(), 1);

    // Outside agtk the status line stays silent and writes nothing.
    fs::remove_file(context_report_path(&sessions.join("claude-1.sock"))).unwrap();
    assert!(run_status_line(&settings, None).is_empty());
    assert_eq!(fs::read_dir(&sessions).unwrap().count(), 0);
}

#[test]
fn context_reporting_uses_its_own_settings_file() {
    let directory = tempfile::tempdir().expect("temporary directory");
    let agtkctl = Path::new("/opt/agtk/agtkctl");
    let plain = ensure_claude_hook_settings(directory.path(), agtkctl, None).unwrap();
    let reporting =
        ensure_claude_hook_settings(directory.path(), agtkctl, Some(directory.path())).unwrap();
    assert_ne!(plain, reporting);
    let read =
        |path| serde_json::from_slice::<serde_json::Value>(&fs::read(path).unwrap()).unwrap();
    assert!(read(&plain).get("statusLine").is_none());
    assert!(read(&reporting).get("statusLine").is_some());
}

#[test]
fn a_status_line_of_the_users_own_is_detected_in_user_and_project_settings() {
    let directory = tempfile::tempdir().expect("temporary directory");
    let config = directory.path().join("config");
    let project = directory.path().join("project");
    fs::create_dir_all(project.join(".claude")).unwrap();
    fs::create_dir_all(&config).unwrap();
    fs::write(config.join("settings.json"), r#"{"theme":"dark"}"#).unwrap();
    assert!(!has_own_status_line(&config, Some(&project)));
    let own = r#"{"statusLine":{"type":"command","command":"echo mine"}}"#;
    for file in ["settings.json", "settings.local.json"] {
        let path = project.join(".claude").join(file);
        fs::write(&path, own).unwrap();
        assert!(has_own_status_line(&config, Some(&project)), "{file}");
        assert!(!has_own_status_line(&config, None), "{file}");
        fs::remove_file(path).unwrap();
    }
    fs::write(config.join("settings.json"), own).unwrap();
    assert!(has_own_status_line(&config, None));
}
