use std::path::{Path, PathBuf};

use serde_json::Value;

use crate::session::SessionKind;

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Choice {
    pub value: String,
    pub label: String,
}

pub const DEFAULT_BASE_BRANCH: &str = "origin/main";

/// Resolve the directory used when a launch has no explicit worktree.
///
/// The project directory is itself a valid launch context, so an empty
/// worktree selection must not be sent to the session host as a missing path.
pub fn effective_worktree_path(
    project_root: Option<&Path>,
    worktree: Option<&Path>,
) -> Option<PathBuf> {
    worktree
        .map(Path::to_path_buf)
        .or_else(|| project_root.map(Path::to_path_buf))
}

/// Expand the shell-style home shorthand accepted by the launch fields.
pub fn expand_user_path(value: &str) -> PathBuf {
    if (value == "~" || value.starts_with("~/"))
        && let Some(home) = std::env::var_os("HOME")
    {
        return PathBuf::from(home).join(value.strip_prefix("~/").unwrap_or_default());
    }
    PathBuf::from(value)
}

pub fn worktree_choices(
    project_root: &str,
    worktrees: impl IntoIterator<Item = (String, String)>,
) -> Vec<Choice> {
    let mut choices = Vec::new();
    if !project_root.is_empty() {
        choices.push(Choice {
            value: project_root.to_owned(),
            label: format!("Current ({})", project_name(project_root)),
        });
    }
    let mut rest = worktrees
        .into_iter()
        .filter(|(path, _)| Path::new(path) != Path::new(project_root))
        .map(|(value, label)| Choice { value, label })
        .collect::<Vec<_>>();
    rest.sort_by(|left, right| left.label.cmp(&right.label));
    choices.extend(rest);
    choices
}

pub fn directory_choices(paths: impl IntoIterator<Item = PathBuf>) -> Vec<Choice> {
    let mut paths = paths.into_iter().collect::<Vec<_>>();
    paths.sort_by(|left, right| {
        project_name(left.to_string_lossy().as_ref())
            .cmp(&project_name(right.to_string_lossy().as_ref()))
            .then_with(|| left.cmp(right))
    });
    paths
        .into_iter()
        .map(|path| Choice {
            label: project_name(path.to_string_lossy().as_ref()),
            value: path.to_string_lossy().into_owned(),
        })
        .collect()
}

pub fn path_completions(prefix: &str) -> Vec<String> {
    if !(prefix.starts_with('/')
        || prefix.starts_with('~')
        || prefix.starts_with('.')
        || prefix.contains('/'))
    {
        return Vec::new();
    }
    let expanded = expand_home(prefix);
    let preserve_tilde = prefix.starts_with('~');
    let (parent, fragment) = match expanded.rsplit_once('/') {
        Some((parent, fragment)) if !parent.is_empty() => (PathBuf::from(parent), fragment),
        _ => (PathBuf::from("."), expanded.as_str()),
    };
    let Ok(entries) = std::fs::read_dir(parent) else {
        return Vec::new();
    };
    let mut completions = entries
        .filter_map(Result::ok)
        .filter_map(|entry| {
            let name = entry.file_name().to_string_lossy().into_owned();
            (!name.starts_with('.') || fragment.starts_with('.')).then_some((name, entry.path()))
        })
        .filter(|(name, _)| name.starts_with(fragment))
        .filter_map(|(_, path)| path.is_dir().then(|| path.to_string_lossy().into_owned()))
        .collect::<Vec<_>>();
    completions.sort();
    completions
        .into_iter()
        .map(|path| {
            if preserve_tilde && let Some(home) = std::env::var_os("HOME") {
                let home = PathBuf::from(home).to_string_lossy().into_owned();
                if path == home {
                    return "~".to_owned();
                }
                if let Some(suffix) = path.strip_prefix(&(home + "/")) {
                    return format!("~/{suffix}");
                }
            }
            path
        })
        .collect()
}

/// Match a launch choice by either its compact name or its full path.
/// Accepting both forms keeps the editable entry useful when a user types a
/// worktree name, an absolute path, or the shell-style `~/...` shorthand.
pub fn choice_matches(query: &str, label: &str, value: &str) -> bool {
    let query = query.trim();
    if query.is_empty() {
        return true;
    }
    let query_lower = query.to_ascii_lowercase();
    let expanded_lower = expand_user_path(query)
        .to_string_lossy()
        .to_ascii_lowercase();
    let label_lower = label.to_ascii_lowercase();
    let value_lower = value.to_ascii_lowercase();
    label_lower.contains(&query_lower)
        || value_lower.contains(&query_lower)
        || value_lower.contains(&expanded_lower)
}

pub fn provider_args(
    kind: SessionKind,
    flags: &std::collections::BTreeMap<String, Value>,
) -> Vec<String> {
    let mut args = Vec::new();
    let value = |key: &str| flags.get(key).and_then(Value::as_str);
    let enabled = |key: &str| flags.get(key).and_then(Value::as_bool).unwrap_or(false);
    match kind {
        SessionKind::Claude => {
            if let Some(value) = value("--permission-mode") {
                args.extend(["--permission-mode".to_owned(), value.to_owned()]);
            }
            if enabled("--dangerously-skip-permissions") {
                args.push("--dangerously-skip-permissions".to_owned());
            }
        }
        SessionKind::Codex => {
            // Codex's automatic modes replace both manual approval and
            // sandbox policy flags. Passing them together makes current
            // Codex reject the launch before the TUI starts.
            if enabled("--dangerously-bypass-approvals-and-sandbox") {
                args.push("--dangerously-bypass-approvals-and-sandbox".to_owned());
            } else if enabled("--full-auto") {
                // `--full-auto` was removed from Codex CLI. This is its
                // supported replacement in current releases.
                args.push("--approve-for-me".to_owned());
            } else {
                if let Some(value) = value("--ask-for-approval") {
                    // Codex versions before 0.154 accepted `untrusted` and
                    // `on-failure`. Keep old saved preferences launchable by
                    // mapping both values to the current interactive policy.
                    let value = match value {
                        "untrusted" | "on-failure" => "on-request",
                        value => value,
                    };
                    args.extend(["--ask-for-approval".to_owned(), value.to_owned()]);
                }
                if let Some(value) = value("--sandbox") {
                    args.extend(["--sandbox".to_owned(), value.to_owned()]);
                }
            }
        }
        SessionKind::Gemini => {
            if let Some(value) = value("--approval-mode") {
                args.extend(["--approval-mode".to_owned(), value.to_owned()]);
            }
            if enabled("--yolo") {
                args.push("--yolo".to_owned());
            }
        }
        SessionKind::Shell | SessionKind::Custom => {}
    }
    args
}

/// Codex arguments that turn off the agtk MCP server for one launch. A PR
/// review reads a tree its author controls, so it gets no tools to start or
/// drive other sessions. Codex's sandbox already keeps its shell commands off
/// agtk's socket. The command keeps the override valid without an agtk entry.
pub fn codex_args_without_agtk_mcp() -> Vec<String> {
    [
        "-c",
        "mcp_servers.agtk.command=\"true\"",
        "-c",
        "mcp_servers.agtk.enabled=false",
    ]
    .map(str::to_owned)
    .to_vec()
}

/// The launch flags an agent keeps when it starts again. Anything else in its
/// arguments, such as an old resume or a first prompt, belongs to that launch only.
pub fn carried_agent_args(kind: SessionKind, args: &[String]) -> Vec<String> {
    // (flag, takes a value)
    let known: &[(&str, bool)] = match kind {
        SessionKind::Claude => &[
            ("--permission-mode", true),
            ("--dangerously-skip-permissions", false),
            ("--model", true),
            ("--add-dir", true),
        ],
        SessionKind::Codex => &[
            ("--dangerously-bypass-approvals-and-sandbox", false),
            ("--approve-for-me", false),
            ("--ask-for-approval", true),
            ("-a", true),
            ("--sandbox", true),
            ("-s", true),
            ("--model", true),
            ("-m", true),
            ("--profile", true),
            ("-p", true),
            ("--add-dir", true),
            // Config overrides, such as the one that keeps a PR review away from agtk's tools.
            ("-c", true),
            ("--config", true),
        ],
        _ => &[],
    };
    let mut carried = Vec::new();
    let mut args = args.iter();
    while let Some(arg) = args.next() {
        let (flag, inline_value) = match arg.split_once('=') {
            Some((flag, _)) if flag.starts_with("--") => (flag, true),
            _ => (arg.as_str(), false),
        };
        let Some((_, takes_value)) = known.iter().find(|(known, _)| *known == flag) else {
            continue;
        };
        carried.push(arg.clone());
        if *takes_value
            && !inline_value
            && let Some(value) = args.next()
        {
            carried.push(value.clone());
        }
    }
    carried
}

fn project_name(path: &str) -> String {
    Path::new(path)
        .file_name()
        .and_then(|name| name.to_str())
        .filter(|name| !name.is_empty())
        .unwrap_or(path)
        .to_owned()
}

fn expand_home(value: &str) -> String {
    expand_user_path(value).to_string_lossy().into_owned()
}
