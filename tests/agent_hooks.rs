use std::fs;
use std::path::Path;
use std::time::Duration;

use agtk::agent_hooks::{HOOK_TIMEOUT_SECONDS, claude_hook_settings, ensure_claude_hook_settings};
use agtk::control::{AgentSignalState, ControlCommand, SessionSetStateParams, control_timeout};

fn command(settings: &serde_json::Value, event: &str) -> String {
    settings["hooks"][event][0]["hooks"][0]["command"]
        .as_str()
        .expect("hook command")
        .to_owned()
}

#[test]
fn claude_hooks_report_each_turn_boundary() {
    let settings = claude_hook_settings(Path::new("/opt/agtk/agtkctl"));
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
    let settings = claude_hook_settings(Path::new("/opt/agtk/agtkctl"));
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
    let settings = claude_hook_settings(Path::new("/tmp/it's here/agtkctl"));
    assert!(command(&settings, "Stop").contains(r"'/tmp/it'\''s here/agtkctl'"));
}

#[test]
fn settings_file_is_written_once_and_kept() {
    let directory = tempfile::tempdir().expect("temporary directory");
    let agtkctl = Path::new("/opt/agtk/agtkctl");
    let path = ensure_claude_hook_settings(directory.path(), agtkctl).expect("write settings");
    let written: serde_json::Value =
        serde_json::from_slice(&fs::read(&path).expect("read settings")).expect("valid json");
    assert_eq!(written, claude_hook_settings(agtkctl));

    let modified = fs::metadata(&path)
        .and_then(|m| m.modified())
        .expect("mtime");
    let again = ensure_claude_hook_settings(directory.path(), agtkctl).expect("rewrite");
    assert_eq!(again, path);
    let unchanged = fs::metadata(&path)
        .and_then(|m| m.modified())
        .expect("mtime");
    assert_eq!(modified, unchanged);
}
