//! Claude Code hook settings that report session state back to agmux.
//!
//! Passed per launch with `claude --settings`, so it works without touching the
//! user's global settings. Every hook no-ops outside an agmux session.

use std::fs;
use std::io;
use std::path::{Path, PathBuf};

use serde_json::{Value, json};

const SETTINGS_NAME: &str = "claude-hooks.json";

/// Notification types that leave the turn blocked on the user. The idle reminder
/// also fires Notification but does not block, so it is not listed.
const BLOCKING_NOTIFICATIONS: &str = "permission_prompt|elicitation_dialog";

/// Longer than agmuxctl's own reply deadline, so a slow app fails quietly
/// through `|| true` instead of Claude Code killing the hook with a warning.
pub const HOOK_TIMEOUT_SECONDS: u64 = 10;

fn shell_quote(value: &str) -> String {
    format!("'{}'", value.replace('\'', r"'\''"))
}

fn state_command(agmuxctl: &Path, state: &str) -> String {
    format!(
        r#"[ -z "$AGMUX_SESSION_ID" ] || {} session state "$AGMUX_SESSION_ID" {state} --hook-input >/dev/null 2>&1 || true"#,
        shell_quote(&agmuxctl.to_string_lossy())
    )
}

pub fn claude_hook_settings(agmuxctl: &Path) -> Value {
    let report = |matcher: &str, state: &str| {
        json!([{
            "matcher": matcher,
            "hooks": [{
                "type": "command",
                "command": state_command(agmuxctl, state),
                "timeout": HOOK_TIMEOUT_SECONDS,
            }],
        }])
    };
    json!({
        "hooks": {
            "UserPromptSubmit": report("", "busy"),
            // Tool hooks bring a session back to busy after a permission prompt.
            "PreToolUse": report("*", "busy"),
            "PostToolUse": report("*", "busy"),
            "Notification": report(BLOCKING_NOTIFICATIONS, "waiting"),
            "Stop": report("", "ready"),
            "StopFailure": report("rate_limit", "waiting"),
            // compact also fires SessionStart, in the middle of a turn
            "SessionStart": report("startup|resume|clear", "idle"),
        }
    })
}

/// Writes the settings file when its content changed and returns its path.
pub fn ensure_claude_hook_settings(dir: &Path, agmuxctl: &Path) -> io::Result<PathBuf> {
    fs::create_dir_all(dir)?;
    let path = dir.join(SETTINGS_NAME);
    let wanted = serde_json::to_vec_pretty(&claude_hook_settings(agmuxctl))?;
    if fs::read(&path).ok().as_deref() != Some(wanted.as_slice()) {
        let temporary = dir.join(format!("{SETTINGS_NAME}.tmp"));
        fs::write(&temporary, &wanted)?;
        fs::rename(&temporary, &path)?;
    }
    Ok(path)
}
