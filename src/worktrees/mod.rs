//! Git worktrees: listing, creating, reviewing and reaping them.

use std::collections::BTreeSet;
use std::error::Error;
use std::ffi::OsString;
use std::fs;
use std::path::{Path, PathBuf};
use std::process::{Command, Output};
use std::time::{SystemTime, UNIX_EPOCH};

use serde::{Deserialize, Serialize};

use crate::command_runner::BoundedByteOutput;
use crate::git;
use layout::worktree_template;
use lifecycle::{ClassificationContext, classify};
use snapshot::status;

mod layout;
mod lifecycle;
mod reap;
mod snapshot;

pub use layout::WorktreeLayout;

pub type WorktreeResult<T> = Result<T, Box<dyn Error + Send + Sync>>;

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct PorcelainWorktree {
    pub path: PathBuf,
    pub head: Option<String>,
    pub branch: Option<String>,
    pub detached: bool,
    pub locked: bool,
    pub prunable: bool,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "kebab-case")]
pub enum WorktreeState {
    Active,
    Open,
    Merged,
    LocalOnly,
    Review,
    Stale,
    Ephemeral,
    Unknown,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "kebab-case")]
pub enum ReapClass {
    Safe,
    Check,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct WorktreeInfo {
    pub path: PathBuf,
    pub head: Option<String>,
    pub branch: Option<String>,
    pub is_primary: bool,
    pub state: WorktreeState,
    pub reap_class: Option<ReapClass>,
    pub evidence: String,
    pub dirty: bool,
    pub ignored_only: bool,
    pub status_hash: String,
    pub locked: bool,
    pub prunable: bool,
    pub live_session_count: usize,
    pub last_commit_at: Option<u64>,
    pub unpushed_count: Option<u64>,
    pub drifted: bool,
    pub off_convention: bool,
    pub upstream_gone: bool,
    pub never_pushed: bool,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct WorktreeInventory {
    pub repo_root: PathBuf,
    pub default_branch: String,
    pub worktrees: Vec<WorktreeInfo>,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct CreatedWorktree {
    pub repo_root: PathBuf,
    pub path: PathBuf,
    pub branch: Option<String>,
    pub purpose: String,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct SessionLocation {
    pub cwd: PathBuf,
    pub project_root: PathBuf,
    pub worktree_path: Option<PathBuf>,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "lowercase")]
pub enum DeleteBranch {
    Auto,
    Never,
    Force,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ReapRequest {
    pub path: PathBuf,
    pub expected_head: String,
    pub expected_status_hash: String,
    pub delete_branch: DeleteBranch,
    /// A person confirmed this removal, so the lifecycle class no longer gates it.
    /// The HEAD and content guards, salvage, and attic tags still apply.
    pub confirmed: bool,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct ReapResult {
    pub ok: bool,
    pub aborted: bool,
    pub reason: Option<String>,
    pub repo_root: PathBuf,
    pub salvage_path: Option<PathBuf>,
    pub attic_tag: Option<String>,
    pub branch_deleted: bool,
}

#[derive(Debug, Clone)]
pub struct WorktreeManager {
    attic_root: PathBuf,
    home: Option<PathBuf>,
}

impl WorktreeManager {
    pub fn new(attic_root: PathBuf) -> Self {
        let home = std::env::var_os("HOME")
            .filter(|home| !home.is_empty())
            .map(PathBuf::from);
        Self { attic_root, home }
    }

    /// Resolve `~` in worktree templates against `home` instead of `$HOME`.
    pub fn with_home(mut self, home: PathBuf) -> Self {
        self.home = Some(home);
        self
    }

    pub fn layout(&self, repo_root: &Path) -> WorktreeResult<WorktreeLayout> {
        let repo_root = canonical_repo(repo_root)?;
        let template = worktree_template(&repo_root);
        Ok(WorktreeLayout {
            repo_root,
            template,
            home: self.home.clone(),
        })
    }

    pub fn repository_root(&self, path: &Path) -> WorktreeResult<PathBuf> {
        common_repo_root(path)
    }

    pub fn worktree_root(&self, path: &Path) -> WorktreeResult<PathBuf> {
        let path = path.canonicalize()?;
        let root = git_text(&path, ["rev-parse", "--show-toplevel"])?;
        Ok(PathBuf::from(root.trim()).canonicalize()?)
    }

    /// Return the checked-out branch for a worktree. Detached HEADs have no branch.
    pub fn branch_for_worktree(&self, path: &Path) -> WorktreeResult<Option<String>> {
        let root = self.worktree_root(path)?;
        match git_text(&root, ["symbolic-ref", "--quiet", "--short", "HEAD"]) {
            Ok(branch) => Ok(Some(branch.trim().to_owned())),
            Err(_) => Ok(None),
        }
    }

    pub fn branch_for_path_in_repo(
        &self,
        repo_root: &Path,
        file_path: &Path,
    ) -> WorktreeResult<Option<String>> {
        let repo_root = canonical_repo(repo_root)?;
        let output = git_text(&repo_root, ["worktree", "list", "--porcelain"])?;
        let file_path = file_path
            .canonicalize()
            .unwrap_or_else(|_| file_path.to_path_buf());
        Ok(parse_worktree_porcelain(&output)
            .into_iter()
            .filter(|worktree| file_path.starts_with(&worktree.path))
            .max_by_key(|worktree| worktree.path.components().count())
            .and_then(|worktree| worktree.branch))
    }

    pub fn resolve_session_location(
        &self,
        cwd: &Path,
        known_project_roots: &[PathBuf],
    ) -> SessionLocation {
        let cwd = cwd.canonicalize().unwrap_or_else(|_| cwd.to_path_buf());
        let project_root = known_project_roots
            .iter()
            .filter(|root| cwd.starts_with(root))
            .max_by_key(|root| root.components().count())
            .cloned()
            .or_else(|| self.repository_root(&cwd).ok())
            .unwrap_or_else(|| cwd.clone());
        let worktree_path = self.worktree_root(&cwd).ok();
        SessionLocation {
            cwd,
            project_root,
            worktree_path,
        }
    }

    /// Return the linked checkout paths without running per-worktree status
    /// and classification commands. Launch controls only need these paths,
    /// and repositories with many worktrees must not wait for full inventory
    /// hashing before showing their choices.
    pub fn linked_paths(&self, repo_root: &Path) -> WorktreeResult<Vec<PathBuf>> {
        Ok(self
            .linked_worktrees(repo_root)?
            .into_iter()
            .map(|worktree| worktree.path)
            .collect())
    }

    /// Like `linked_paths`, with branches. PR matching only needs these, and a
    /// full `list` hashes files in every worktree.
    pub fn linked_worktrees(&self, repo_root: &Path) -> WorktreeResult<Vec<PorcelainWorktree>> {
        let repo_root = canonical_repo(repo_root)?;
        let output = git_text(&repo_root, ["worktree", "list", "--porcelain"])?;
        Ok(parse_worktree_porcelain(&output))
    }

    pub fn list(
        &self,
        repo_root: &Path,
        live_paths: &[PathBuf],
    ) -> WorktreeResult<WorktreeInventory> {
        self.inventory(repo_root, live_paths, None)
    }

    /// Lists the repository's worktrees, or only the one at `only`. Each
    /// costs several git calls, so reaping one worktree inspects just it.
    fn inventory(
        &self,
        repo_root: &Path,
        live_paths: &[PathBuf],
        only: Option<&Path>,
    ) -> WorktreeResult<WorktreeInventory> {
        let repo_root = canonical_repo(repo_root)?;
        let output = git_text(&repo_root, ["worktree", "list", "--porcelain"])?;
        let parsed = parse_worktree_porcelain(&output);
        let default_branch = default_branch(&repo_root)?;
        let layout = self.layout(&repo_root)?;
        let now = now_millis() as u64;
        let classification = ClassificationContext {
            repo_root: &repo_root,
            default_branch: &default_branch,
            layout: &layout,
            now,
        };
        let mut worktrees = Vec::with_capacity(parsed.len());
        for (index, worktree) in parsed.into_iter().enumerate() {
            if only.is_some_and(|only| worktree.path != only) {
                continue;
            }
            let status = status(&worktree.path)?;
            let live_session_count = live_paths
                .iter()
                .filter(|path| paths_equal(path, &worktree.path))
                .count();
            worktrees.push(classify(
                worktree,
                index == 0,
                status,
                live_session_count,
                &classification,
            ));
        }
        Ok(WorktreeInventory {
            repo_root,
            default_branch,
            worktrees,
        })
    }

    pub fn branch_names(&self, repo_root: &Path) -> WorktreeResult<Vec<String>> {
        let repo_root = canonical_repo(repo_root)?;
        let default = default_branch(&repo_root).ok();
        let output = git_text(
            &repo_root,
            [
                "for-each-ref",
                "--format=%(refname:short)",
                "refs/heads",
                "refs/remotes",
            ],
        )?;
        let mut branches = BTreeSet::new();
        for branch in output
            .lines()
            .map(str::trim)
            .filter(|branch| !branch.is_empty())
        {
            let branch = branch.strip_prefix("origin/").unwrap_or(branch);
            if branch != "HEAD" {
                branches.insert(branch.to_owned());
            }
        }
        let mut branches = branches.into_iter().collect::<Vec<_>>();
        branches.sort();
        if let Some(default) = default {
            branches.retain(|branch| branch != &default);
            branches.insert(0, default);
        }
        Ok(branches)
    }

    pub fn create(
        &self,
        repo_root: &Path,
        branch: &str,
        base: Option<&str>,
        purpose: &str,
    ) -> WorktreeResult<CreatedWorktree> {
        let repo_root = canonical_repo(repo_root)?;
        validate_branch(branch)?;
        let purpose = purpose.trim();
        if purpose.is_empty() || purpose.chars().count() > 240 {
            return Err("purpose must contain between 1 and 240 characters".into());
        }
        git_text(&repo_root, ["check-ref-format", "--branch", branch])?;
        let base = base.map(str::trim).filter(|base| !base.is_empty());
        let base = match base {
            Some(base) => base.to_owned(),
            None => default_branch(&repo_root)?,
        };
        git_text(
            &repo_root,
            ["rev-parse", "--verify", &format!("{base}^{{commit}}")],
        )?;
        let path = self.layout(&repo_root)?.path_for(branch)?;
        if path.exists() {
            return Err(format!("worktree target already exists: {}", path.display()).into());
        }
        let path = with_canonical_parent(&path)?;
        let branch_ref = format!("refs/heads/{branch}");
        let branch_exists = git_success(
            &repo_root,
            ["rev-parse", "--verify", "--quiet", &branch_ref],
        );
        let mut arguments = os_args(&["worktree", "add"]);
        if branch_exists {
            arguments.push(path.as_os_str().to_owned());
            arguments.push(branch.into());
        } else {
            arguments.extend([OsString::from("-b"), branch.into()]);
            arguments.push(path.as_os_str().to_owned());
            arguments.push(base.clone().into());
        }
        run_slow_git(&repo_root, arguments)?;
        if !branch_exists {
            // The Changes panel and Emacs branch review compare against it.
            git_text(
                &repo_root,
                [
                    "config",
                    &format!("branch.{branch}.agtk-base-branch"),
                    &base,
                ],
            )?;
        }
        git_text(
            &repo_root,
            ["config", &format!("branch.{branch}.description"), purpose],
        )?;
        Ok(CreatedWorktree {
            repo_root,
            path,
            branch: Some(branch.to_owned()),
            purpose: purpose.to_owned(),
        })
    }

    /// The detached `pr-<id>` checkout at the repository's worktree location,
    /// created at the PR's current source tip. An existing checkout moves to
    /// that tip when it has no edits, so an earlier review's untracked notes survive.
    pub fn review_checkout(
        &self,
        repo_root: &Path,
        pull_request_id: u64,
        source_branch: &str,
    ) -> WorktreeResult<PathBuf> {
        let repo_root = canonical_repo(repo_root)?;
        let source_branch = source_branch.trim();
        if source_branch.is_empty()
            || source_branch.starts_with('-')
            || source_branch.contains('\0')
        {
            return Err("source branch is invalid".into());
        }
        let path = with_canonical_parent(&self.layout(&repo_root)?.review_path(pull_request_id)?)?;
        if path == repo_root {
            return Err("review target is the primary checkout".into());
        }
        // The PR author controls the checked-out tree, and a repo-relative
        // core.hooksPath (husky) would run a post-checkout hook from it.
        let no_hooks = || os_args(&["-c", "core.hooksPath=/dev/null"]);
        let mut fetch = no_hooks();
        fetch.extend(os_args(&["fetch", "origin", source_branch]));
        run_slow_git(&repo_root, fetch)?;
        let tip = format!("origin/{source_branch}");
        if path.exists() {
            if !self.linked_paths(&repo_root)?.contains(&path) {
                return Err(format!("review target is not a worktree: {}", path.display()).into());
            }
            let edited = !git_text(&path, ["status", "--porcelain", "--untracked-files=no"])?
                .trim()
                .is_empty();
            if !edited {
                let mut checkout = no_hooks();
                checkout.extend(os_args(&["checkout", "--quiet", "--detach", &tip]));
                run_slow_git(&path, checkout)?;
            }
            return Ok(path);
        }
        let mut arguments = no_hooks();
        arguments.extend(os_args(&["worktree", "add", "--detach"]));
        arguments.push(path.as_os_str().to_owned());
        arguments.push(tip.into());
        run_slow_git(&repo_root, arguments)?;
        Ok(path)
    }
}

pub fn parse_worktree_porcelain(output: &str) -> Vec<PorcelainWorktree> {
    output
        .split("\n\n")
        .filter_map(|block| {
            let mut path = None;
            let mut head = None;
            let mut branch = None;
            let mut detached = false;
            let mut locked = false;
            let mut prunable = false;
            for line in block.lines() {
                if let Some(value) = line.strip_prefix("worktree ") {
                    path = Some(PathBuf::from(value));
                } else if let Some(value) = line.strip_prefix("HEAD ") {
                    head = Some(value.to_owned());
                } else if let Some(value) = line.strip_prefix("branch refs/heads/") {
                    branch = Some(value.to_owned());
                } else if line == "detached" {
                    detached = true;
                } else if line == "locked" || line.starts_with("locked ") {
                    locked = true;
                } else if line == "prunable" || line.starts_with("prunable ") {
                    prunable = true;
                }
            }
            Some(PorcelainWorktree {
                path: path?,
                head,
                branch,
                detached,
                locked,
                prunable,
            })
        })
        .collect()
}

fn canonical_repo(path: &Path) -> WorktreeResult<PathBuf> {
    let path = path.canonicalize()?;
    let root = git_text(&path, ["rev-parse", "--show-toplevel"])?;
    let root = PathBuf::from(root.trim()).canonicalize()?;
    if root != path {
        return Err(format!(
            "path is not the primary repository root: {}",
            path.display()
        )
        .into());
    }
    Ok(root)
}

fn common_repo_root(worktree: &Path) -> WorktreeResult<PathBuf> {
    let common = git_text(
        worktree,
        ["rev-parse", "--path-format=absolute", "--git-common-dir"],
    )?;
    let common = PathBuf::from(common.trim());
    common
        .parent()
        .ok_or_else(|| "git common directory has no parent".into())
        .and_then(|path| path.canonicalize().map_err(Into::into))
}

fn default_branch(repo_root: &Path) -> WorktreeResult<String> {
    match git::default_branch(repo_root) {
        Some(branch) => Ok(branch),
        None => Ok(git_text(repo_root, ["symbolic-ref", "--short", "HEAD"])?
            .trim()
            .to_owned()),
    }
}

/// Creates the parent of a worktree target and resolves it, so the path
/// matches what `git worktree list` reports.
fn with_canonical_parent(path: &Path) -> WorktreeResult<PathBuf> {
    let parent = path.parent().ok_or("worktree target has no parent")?;
    fs::create_dir_all(parent)?;
    Ok(parent
        .canonicalize()?
        .join(path.file_name().ok_or("invalid worktree target")?))
}

fn validate_branch(branch: &str) -> WorktreeResult<()> {
    let valid = (1..=120).contains(&branch.len())
        && branch.bytes().enumerate().all(|(index, byte)| {
            byte.is_ascii_lowercase() || byte.is_ascii_digit() || (byte == b'-' && index > 0)
        })
        && !branch.ends_with('-')
        && !branch.contains("--");
    if !valid {
        return Err("branch must be a concise lowercase kebab-case slug".into());
    }
    Ok(())
}

fn paths_equal(left: &Path, right: &Path) -> bool {
    left.canonicalize().ok() == right.canonicalize().ok()
}

fn now_millis() -> u128 {
    SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .unwrap_or_default()
        .as_millis()
}

fn git_text<'a>(cwd: &Path, args: impl IntoIterator<Item = &'a str>) -> WorktreeResult<String> {
    let output = run_git(cwd, args.into_iter().map(OsString::from).collect())?;
    String::from_utf8(output.stdout).map_err(Into::into)
}

fn git_success<'a>(cwd: &Path, args: impl IntoIterator<Item = &'a str>) -> bool {
    git::succeeds(cwd, args)
}

fn run_git(cwd: &Path, args: Vec<OsString>) -> WorktreeResult<BoundedByteOutput> {
    Ok(git::run_checked(cwd, args)?)
}

/// For git calls that wait on the network or run checkout hooks.
fn run_slow_git(cwd: &Path, args: Vec<OsString>) -> WorktreeResult<BoundedByteOutput> {
    Ok(git::checked(git::run_with_timeout(
        cwd,
        args,
        git::LONG_TIMEOUT,
    )?)?)
}

fn checked_output(mut command: Command) -> WorktreeResult<Output> {
    let output = command.output()?;
    if output.status.success() {
        Ok(output)
    } else {
        Err(String::from_utf8_lossy(&output.stderr)
            .trim()
            .to_owned()
            .into())
    }
}

fn os_args(values: &[&str]) -> Vec<OsString> {
    values.iter().map(OsString::from).collect()
}
