use std::path::{Path, PathBuf};

use serde_json::Value;

use crate::control::SessionKind;

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Choice {
    pub value: String,
    pub label: String,
}

pub const NEW_WORKTREE: &str = "__new__";

pub fn worktree_choices(
    project_root: &str,
    worktrees: impl IntoIterator<Item = (String, String)>,
    allow_new: bool,
) -> Vec<Choice> {
    let mut choices = Vec::new();
    if allow_new {
        choices.push(Choice {
            value: NEW_WORKTREE.to_owned(),
            label: "+ New worktree".to_owned(),
        });
    }
    if !project_root.is_empty() {
        choices.push(Choice {
            value: project_root.to_owned(),
            label: format!("Current ({})", project_name(project_root)),
        });
    }
    let mut rest = worktrees
        .into_iter()
        .filter(|(path, _)| path != project_root)
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
    let expanded = expand_home(prefix);
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
            if let Some(value) = value("--ask-for-approval") {
                args.extend(["--ask-for-approval".to_owned(), value.to_owned()]);
            }
            if let Some(value) = value("--sandbox") {
                args.extend(["--sandbox".to_owned(), value.to_owned()]);
            }
            if enabled("--full-auto") {
                args.push("--full-auto".to_owned());
            }
            if enabled("--dangerously-bypass-approvals-and-sandbox") {
                args.push("--dangerously-bypass-approvals-and-sandbox".to_owned());
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

fn project_name(path: &str) -> String {
    Path::new(path)
        .file_name()
        .and_then(|name| name.to_str())
        .filter(|name| !name.is_empty())
        .unwrap_or(path)
        .to_owned()
}

fn expand_home(value: &str) -> String {
    if (value == "~" || value.starts_with("~/"))
        && let Some(home) = std::env::var_os("HOME")
    {
        return PathBuf::from(home)
            .join(value.strip_prefix("~/").unwrap_or_default())
            .to_string_lossy()
            .into_owned();
    }
    value.to_owned()
}
