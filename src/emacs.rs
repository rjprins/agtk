use std::error::Error;
use std::ffi::OsString;
use std::path::{Path, PathBuf};
use std::process::Command;
use std::time::Duration;

use serde::{Deserialize, Serialize};

use crate::command_runner::run_bounded;
use crate::git;

pub type EmacsResult<T> = Result<T, Box<dyn Error + Send + Sync>>;
const EMACS_TIMEOUT: Duration = Duration::from_secs(30);
const OUTPUT_LIMIT: usize = 256 * 1024;

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "kebab-case")]
pub enum EmacsAction {
    Magit,
    BranchReview,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct EmacsOpenResult {
    pub action: EmacsAction,
    pub path: PathBuf,
    pub base_branch: Option<String>,
}

#[derive(Debug, Clone)]
pub struct EmacsIntegration {
    command: OsString,
}

impl EmacsIntegration {
    pub fn from_environment() -> Self {
        Self::new(
            std::env::var_os("AGTK_EMACSCLIENT")
                .filter(|value| !value.is_empty())
                .unwrap_or_else(|| OsString::from("emacsclient")),
        )
    }

    pub fn new(command: impl Into<OsString>) -> Self {
        Self {
            command: command.into(),
        }
    }

    pub fn open(&self, cwd: &Path, action: EmacsAction) -> EmacsResult<EmacsOpenResult> {
        let path = resolve_worktree_root(cwd)?;
        let base_branch = if action == EmacsAction::BranchReview {
            resolve_branch_review_base(&path)?
        } else {
            None
        };
        let form = match action {
            EmacsAction::Magit => build_magit_eval(&path),
            EmacsAction::BranchReview => build_branch_review_eval(&path, base_branch.as_deref()),
        };
        self.eval(&form)?;
        Ok(EmacsOpenResult {
            action,
            path,
            base_branch,
        })
    }

    /// Visits `path` in a raised Emacs frame, at `line` and `column` when given.
    pub fn open_file(
        &self,
        path: &Path,
        line: Option<u32>,
        column: Option<u32>,
    ) -> EmacsResult<()> {
        self.eval(&build_open_file_eval(path, line, column))
    }

    fn eval(&self, form: &str) -> EmacsResult<()> {
        let mut command = Command::new(&self.command);
        command.args(["-n", "-a", "", "--eval", form]);
        if let Some(display) =
            std::env::var_os("AGTK_EMACS_DISPLAY").filter(|value| !value.is_empty())
        {
            command.env("DISPLAY", display);
        }
        if let Some(authority) =
            std::env::var_os("AGTK_EMACS_XAUTHORITY").filter(|value| !value.is_empty())
        {
            command.env("XAUTHORITY", authority);
        }
        let output = run_bounded(command, EMACS_TIMEOUT, OUTPUT_LIMIT)?;
        if !output.status.success() {
            return Err(format!(
                "emacsclient exited with {}: {}",
                output.status,
                output.stderr.trim()
            )
            .into());
        }
        Ok(())
    }
}

pub fn build_open_file_eval(path: &Path, line: Option<u32>, column: Option<u32>) -> String {
    let mut body = vec![format!(
        "    (find-file {})",
        elisp_string(&path.to_string_lossy())
    )];
    if let Some(line) = line.filter(|line| *line > 0) {
        body.push("    (goto-char (point-min))".to_owned());
        body.push(format!("    (forward-line {})", line - 1));
        if let Some(column) = column.filter(|column| *column > 0) {
            body.push(format!("    (move-to-column {})", column - 1));
        }
    }
    wrap_raised_frame(None, &body)
}

pub fn build_magit_eval(worktree_path: &Path) -> String {
    let path = elisp_string(&worktree_path.to_string_lossy());
    wrap_raised_frame(
        Some("magit"),
        &[
            format!("    (let ((default-directory (file-name-as-directory {path})))"),
            "      (call-interactively 'magit-status))".to_owned(),
        ],
    )
}

pub fn build_branch_review_eval(worktree_path: &Path, base_branch: Option<&str>) -> String {
    let path = elisp_string(&worktree_path.to_string_lossy());
    let mut body = vec![format!(
        "    (let ((default-directory (file-name-as-directory {path})))"
    )];
    if let Some(base) = base_branch.map(str::trim).filter(|base| !base.is_empty()) {
        let base = elisp_string(base);
        body.extend([
            format!("      (let ((agtk-branch-review-base {base}))"),
            "        (require 'cl-lib)".to_owned(),
            "        (cl-letf (((symbol-function 'magit-read-branch-or-commit)".to_owned(),
            "                   (lambda (&rest _) agtk-branch-review-base)))".to_owned(),
            "          (call-interactively 'branch-review-with-base))))".to_owned(),
        ]);
    } else {
        body.push("      (call-interactively 'branch-review))".to_owned());
    }
    wrap_raised_frame(Some("branch-review"), &body)
}

fn wrap_raised_frame(package: Option<&str>, body: &[String]) -> String {
    let mut lines = vec!["(progn".to_owned()];
    if let Some(package) = package {
        lines.push(format!("  (require '{package} nil t)"));
    }
    lines.extend([
        "  (let ((frame (or (car (filtered-frame-list #'display-graphic-p))".to_owned(),
        "                   (let ((d (or (getenv \"DISPLAY\") (getenv \"WAYLAND_DISPLAY\"))))"
            .to_owned(),
        "                     (and d (ignore-errors (make-frame-on-display d))))".to_owned(),
        "                   (selected-frame))))".to_owned(),
        "    (select-frame frame)".to_owned(),
    ]);
    lines.extend_from_slice(body);
    lines.extend([
        "    (make-frame-visible frame)".to_owned(),
        "    (raise-frame frame)".to_owned(),
        "    (select-frame-set-input-focus frame))".to_owned(),
        "  )".to_owned(),
    ]);
    lines.join("\n")
}

fn elisp_string(value: &str) -> String {
    serde_json::to_string(value).expect("strings always serialize")
}

pub fn resolve_worktree_root(cwd: &Path) -> EmacsResult<PathBuf> {
    let cwd = cwd.canonicalize()?;
    Ok(git::toplevel(&cwd).unwrap_or(cwd))
}

pub fn resolve_branch_review_base(cwd: &Path) -> EmacsResult<Option<String>> {
    let cwd = cwd.canonicalize()?;
    let Some(current) = git::optional(&cwd, ["branch", "--show-current"]) else {
        return Ok(None);
    };
    if let Some(base) = git::configured_bases(&cwd, &current)
        .into_iter()
        .find(|base| git::commit_exists(&cwd, base))
    {
        return Ok(Some(base));
    }
    if let Some(rebase_base) = resolve_rebase_base(&cwd, &current) {
        return Ok(Some(rebase_base));
    }
    if let Some(upstream) = git::optional(
        &cwd,
        [
            "rev-parse",
            "--abbrev-ref",
            "--symbolic-full-name",
            "@{upstream}",
        ],
    ) && !upstream_tracks_current(&upstream, &current)
        && git::commit_exists(&cwd, &upstream)
    {
        return Ok(Some(upstream));
    }
    Ok(None)
}

fn resolve_rebase_base(cwd: &Path, current: &str) -> Option<String> {
    let reflog = git::optional(cwd, ["reflog", "show", "--format=%gs", current])?;
    let current_ref = format!("refs/heads/{current}");
    let target = reflog.lines().find_map(|line| {
        let rest = line.strip_prefix("rebase (finish): ")?;
        let rest = rest.strip_prefix("returning to ").unwrap_or(rest);
        let (reference, target) = rest.rsplit_once(" onto ")?;
        (reference == current || reference == current_ref).then(|| target.trim().to_owned())
    })?;
    if !git::commit_exists(cwd, &target) {
        return None;
    }
    let refs = git::optional(
        cwd,
        [
            "for-each-ref",
            "--points-at",
            &target,
            "--format=%(refname:short)",
            "refs/heads",
            "refs/remotes",
        ],
    );
    refs.and_then(|refs| branch_name_for_rebase_target(&refs, current))
        .or(Some(target))
}

fn branch_name_for_rebase_target(refs: &str, current: &str) -> Option<String> {
    let current_remote = format!("origin/{current}");
    let candidates = refs
        .lines()
        .map(str::trim)
        .filter(|reference| {
            !reference.is_empty()
                && *reference != "origin/HEAD"
                && *reference != current
                && *reference != current_remote
        })
        .collect::<Vec<_>>();
    candidates
        .iter()
        .find(|reference| !reference.contains('/'))
        .or_else(|| {
            candidates
                .iter()
                .find(|reference| !reference.starts_with("origin/"))
        })
        .or_else(|| candidates.first())
        .map(|reference| (*reference).to_owned())
}

fn upstream_tracks_current(upstream: &str, current: &str) -> bool {
    let shortened = upstream.strip_prefix("refs/heads/").unwrap_or(upstream);
    let shortened = shortened.strip_prefix("refs/remotes/").unwrap_or(shortened);
    shortened == current
        || shortened
            .split_once('/')
            .is_some_and(|(_, branch)| branch == current)
}
