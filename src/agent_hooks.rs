//! Claude Code hook settings that report session state back to agtk.
//!
//! Passed per launch with `claude --settings`, so it works without touching the
//! user's global settings. Every hook no-ops outside an agtk session. Unless the
//! user has a status line of their own, a silent one reports the context window.

use std::fs;
use std::io;
use std::path::{Path, PathBuf};

use serde_json::{Value, json};

use crate::instance::ensure_private_dir;

const SETTINGS_NAME: &str = "claude-hooks.json";
const CONTEXT_SETTINGS_NAME: &str = "claude-context-hooks.json";
/// A session's context report sits beside its socket, as `{id}.context.json`.
const CONTEXT_EXTENSION: &str = "context.json";

/// Notification types that leave the turn blocked on the user. The idle reminder
/// also fires Notification but does not block, so it is not listed.
const BLOCKING_NOTIFICATIONS: &str = "permission_prompt|elicitation_dialog";

/// Longer than agtkctl's own reply deadline, so a slow app fails quietly
/// through `|| true` instead of Claude Code killing the hook with a warning.
pub const HOOK_TIMEOUT_SECONDS: u64 = 10;

fn shell_quote(value: &str) -> String {
    format!("'{}'", value.replace('\'', r"'\''"))
}

fn state_command(agtkctl: &Path, state: &str) -> String {
    format!(
        r#"[ -z "$AGTK_SESSION_ID" ] || {} session state "$AGTK_SESSION_ID" {state} --hook-input >/dev/null 2>&1 || true"#,
        shell_quote(&agtkctl.to_string_lossy())
    )
}

/// Claude Code reports its context window only to a status line. This one saves
/// that input next to the session socket and prints nothing.
fn context_command(sessions_dir: &Path) -> String {
    format!(
        r#"[ -z "$AGTK_SESSION_ID" ] || {{ f={}/"$AGTK_SESSION_ID".{CONTEXT_EXTENSION}; cat > "$f.tmp" && mv -f "$f.tmp" "$f"; }} >/dev/null 2>&1 || true"#,
        shell_quote(&sessions_dir.to_string_lossy())
    )
}

/// Where a session's status line leaves Claude Code's last context report.
pub fn context_report_path(socket: &Path) -> PathBuf {
    socket.with_extension(CONTEXT_EXTENSION)
}

/// The user's Claude Code settings that apply in `cwd`, least specific first.
fn claude_settings(claude_config_dir: &Path, cwd: Option<&Path>) -> Vec<Value> {
    let mut candidates = vec![claude_config_dir.join("settings.json")];
    if let Some(cwd) = cwd {
        candidates.push(cwd.join(".claude/settings.json"));
        candidates.push(cwd.join(".claude/settings.local.json"));
    }
    candidates
        .into_iter()
        .filter_map(|path| {
            fs::read(path)
                .ok()
                .and_then(|bytes| serde_json::from_slice::<Value>(&bytes).ok())
        })
        .collect()
}

/// True when Claude Code would show the user's own status line in `cwd`, which
/// agtk's would replace.
pub fn has_own_status_line(claude_config_dir: &Path, cwd: Option<&Path>) -> bool {
    claude_settings(claude_config_dir, cwd)
        .iter()
        .any(|settings| !settings["statusLine"].is_null())
}

/// The token count at which Claude Code auto-compacts `model` in `cwd`, when
/// the user's settings set one.
pub fn auto_compact_window(
    claude_config_dir: &Path,
    cwd: Option<&Path>,
    model: &str,
) -> Option<u64> {
    claude_settings(claude_config_dir, cwd)
        .iter()
        .rev()
        .find_map(|settings| settings["modelSettings"][model]["autoCompactWindow"].as_u64())
        .filter(|window| *window > 0)
}

/// With `sessions_dir`, also adds the status line that reports the context window.
pub fn claude_hook_settings(agtkctl: &Path, sessions_dir: Option<&Path>) -> Value {
    let report = |matcher: &str, state: &str| {
        json!([{
            "matcher": matcher,
            "hooks": [{
                "type": "command",
                "command": state_command(agtkctl, state),
                "timeout": HOOK_TIMEOUT_SECONDS,
            }],
        }])
    };
    let mut settings = json!({
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
    });
    if let Some(sessions_dir) = sessions_dir {
        settings["statusLine"] = json!({
            "type": "command",
            "command": context_command(sessions_dir),
        });
    }
    settings
}

/// Writes the settings file when its content changed and returns its path.
pub fn ensure_claude_hook_settings(
    dir: &Path,
    agtkctl: &Path,
    sessions_dir: Option<&Path>,
) -> io::Result<PathBuf> {
    ensure_private_dir(dir)?;
    let name = if sessions_dir.is_some() {
        CONTEXT_SETTINGS_NAME
    } else {
        SETTINGS_NAME
    };
    let path = dir.join(name);
    let wanted = serde_json::to_vec_pretty(&claude_hook_settings(agtkctl, sessions_dir))?;
    if fs::read(&path).ok().as_deref() != Some(wanted.as_slice()) {
        let temporary = dir.join(format!("{name}.tmp"));
        fs::write(&temporary, &wanted)?;
        fs::rename(&temporary, &path)?;
    }
    Ok(path)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn the_most_specific_settings_set_the_auto_compact_window() {
        let dir = tempfile::tempdir().unwrap();
        let (user, project) = (dir.path().join("user"), dir.path().join("project"));
        fs::create_dir_all(project.join(".claude")).unwrap();
        fs::create_dir_all(&user).unwrap();
        let write = |path: PathBuf, window: u64| {
            let settings = json!({"modelSettings":{"opus":{"autoCompactWindow":window}}});
            fs::write(path, settings.to_string()).unwrap();
        };
        let window = |model| auto_compact_window(&user, Some(&project), model);
        assert_eq!(window("opus"), None);
        write(user.join("settings.json"), 800_000);
        assert_eq!(window("opus"), Some(800_000));
        write(project.join(".claude/settings.json"), 300_000);
        assert_eq!(window("opus"), Some(300_000));
        write(project.join(".claude/settings.local.json"), 200_000);
        assert_eq!(window("opus"), Some(200_000));
        assert_eq!(window("sonnet"), None);
    }
}
