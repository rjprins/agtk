//! Where worktrees and PR review checkouts go, from the configured path template.

use std::ffi::OsStr;
use std::path::{Component, Path, PathBuf};

use super::{WorktreeResult, git_text};

/// Where one repository's worktrees go, from `agtk.worktreeTemplate` or the
/// default `~/worktrees/{repo-name}/{branch}`.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct WorktreeLayout {
    pub(super) repo_root: PathBuf,
    pub(super) template: String,
    pub(super) home: Option<PathBuf>,
}

impl WorktreeLayout {
    pub fn path_for(&self, branch: &str) -> WorktreeResult<PathBuf> {
        resolve_template(
            &self.repo_root,
            branch,
            &self.template,
            self.home.as_deref(),
        )
    }

    /// The detached checkout that reviews one pull request.
    pub fn review_path(&self, pull_request_id: u64) -> WorktreeResult<PathBuf> {
        self.path_for(&format!("pr-{pull_request_id}"))
    }
}

pub(super) fn worktree_template(repo_root: &Path) -> String {
    // Repos configured for the old agmux keep working.
    ["agtk.worktreeTemplate", "agmux.worktreeTemplate"]
        .into_iter()
        .filter_map(|key| git_text(repo_root, ["config", "--get", key]).ok())
        .map(|value| value.trim().to_owned())
        .find(|value| !value.is_empty())
        .unwrap_or_else(|| "~/worktrees/{repo-name}/{branch}".to_owned())
}

fn resolve_template(
    repo_root: &Path,
    branch: &str,
    template: &str,
    home: Option<&Path>,
) -> WorktreeResult<PathBuf> {
    let repo_name = repo_root
        .file_name()
        .and_then(OsStr::to_str)
        .ok_or("repository name is not UTF-8")?;
    let replaced = template
        .replace("{repo-name}", repo_name)
        .replace("{repo-root}", &repo_root.to_string_lossy())
        .replace("{branch}", &sanitize(branch));
    let replaced = match replaced.strip_prefix('~') {
        Some(rest) if rest.is_empty() || rest.starts_with('/') => {
            let home = home.ok_or("worktree template uses ~, but HOME is not set")?;
            format!("{}{rest}", home.display())
        }
        _ => replaced,
    };
    let joined = if Path::new(&replaced).is_absolute() {
        PathBuf::from(replaced)
    } else {
        repo_root.join(replaced)
    };
    Ok(normalize_path(&joined))
}

pub(super) fn normalize_path(path: &Path) -> PathBuf {
    let mut normalized = PathBuf::new();
    for component in path.components() {
        match component {
            Component::ParentDir => {
                normalized.pop();
            }
            Component::CurDir => {}
            other => normalized.push(other.as_os_str()),
        }
    }
    normalized
}

pub(super) fn sanitize(value: &str) -> String {
    value
        .chars()
        .map(|character| {
            if character == '/' || character == '\\' || character == ' ' {
                '-'
            } else {
                character
            }
        })
        .collect()
}
