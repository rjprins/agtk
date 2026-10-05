//! Runs git for agtk's own queries and worktree operations.
//!
//! Every call has a timeout and an output limit, and runs with an environment
//! that cannot point it at another repository or make it prompt for
//! credentials, so a hung fetch cannot stall agtk's background queues.

use std::ffi::OsStr;
use std::path::{Path, PathBuf};
use std::process::Command;
use std::time::Duration;

use crate::command_runner::{BoundedByteOutput, CommandError, run_bounded_bytes};

/// Enough for a local query or worktree change on a large repository.
pub const TIMEOUT: Duration = Duration::from_secs(30);
/// Fetches wait on the network, and checkouts run the repository's hooks.
pub const LONG_TIMEOUT: Duration = Duration::from_secs(5 * 60);
pub const OUTPUT_LIMIT: usize = 64 * 1024 * 1024;

/// Branch config keys that record what a branch was created from, in order of
/// preference. The agmux key is still set on branches the old agmux created.
pub const BASE_CONFIG_KEYS: [&str; 5] = [
    "agtk-base-branch",
    "agmux-base-branch",
    "vscode-merge-base",
    "gh-merge-base",
    "merge-base",
];

pub fn command(root: &Path) -> Command {
    let mut command = Command::new("git");
    command
        .current_dir(root)
        // An fsmonitor hook in .git/config would otherwise run on every query.
        .args(["-c", "core.fsmonitor=false"])
        .env_remove("GIT_DIR")
        .env_remove("GIT_WORK_TREE")
        .env_remove("GIT_INDEX_FILE")
        .env_remove("GIT_EXTERNAL_DIFF")
        .env("GIT_OPTIONAL_LOCKS", "0")
        .env("GIT_TERMINAL_PROMPT", "0")
        .env("LC_ALL", "C");
    command
}

/// Runs a prepared git command, whatever its exit status.
pub fn output(
    command: Command,
    timeout: Duration,
    output_limit: usize,
) -> Result<BoundedByteOutput, String> {
    run_bounded_bytes(command, timeout, output_limit).map_err(|error| match error {
        CommandError::TimedOut(timeout) => {
            format!("Git command timed out after {} ms", timeout.as_millis())
        }
        CommandError::OutputExceeded(limit) => {
            format!("Git command output exceeded {limit} bytes")
        }
        other => other.to_string(),
    })
}

/// Runs git in `root`, whatever its exit status.
pub fn run<I, S>(root: &Path, arguments: I) -> Result<BoundedByteOutput, String>
where
    I: IntoIterator<Item = S>,
    S: AsRef<OsStr>,
{
    run_with_timeout(root, arguments, TIMEOUT)
}

pub fn run_with_timeout<I, S>(
    root: &Path,
    arguments: I,
    timeout: Duration,
) -> Result<BoundedByteOutput, String>
where
    I: IntoIterator<Item = S>,
    S: AsRef<OsStr>,
{
    let mut command = command(root);
    command.args(arguments);
    output(command, timeout, OUTPUT_LIMIT)
}

/// Turns a failed exit into an error carrying git's message.
pub fn checked(output: BoundedByteOutput) -> Result<BoundedByteOutput, String> {
    if output.status.success() {
        return Ok(output);
    }
    let message = String::from_utf8_lossy(&output.stderr).trim().to_owned();
    Err(if message.is_empty() {
        format!("git exited with {}", output.status)
    } else {
        message
    })
}

/// Runs git in `root` and fails unless it succeeds.
pub fn run_checked<I, S>(root: &Path, arguments: I) -> Result<BoundedByteOutput, String>
where
    I: IntoIterator<Item = S>,
    S: AsRef<OsStr>,
{
    checked(run(root, arguments)?)
}

/// The trimmed output of a successful git call.
pub fn text<I, S>(root: &Path, arguments: I) -> Result<String, String>
where
    I: IntoIterator<Item = S>,
    S: AsRef<OsStr>,
{
    let output = run_checked(root, arguments)?;
    Ok(String::from_utf8_lossy(&output.stdout).trim().to_owned())
}

/// The trimmed output of a successful git call, when it printed anything.
pub fn optional<I, S>(root: &Path, arguments: I) -> Option<String>
where
    I: IntoIterator<Item = S>,
    S: AsRef<OsStr>,
{
    text(root, arguments).ok().filter(|value| !value.is_empty())
}

pub fn succeeds<I, S>(root: &Path, arguments: I) -> bool
where
    I: IntoIterator<Item = S>,
    S: AsRef<OsStr>,
{
    run(root, arguments).is_ok_and(|output| output.status.success())
}

pub fn commit_exists(root: &Path, reference: &str) -> bool {
    succeeds(
        root,
        [
            "rev-parse",
            "--verify",
            "--quiet",
            "--end-of-options",
            &format!("{reference}^{{commit}}"),
        ],
    )
}

/// The top of the worktree that contains `path`.
pub fn toplevel(path: &Path) -> Option<PathBuf> {
    optional(path, ["rev-parse", "--show-toplevel"]).map(PathBuf::from)
}

/// Resolve a checkout root to its primary repository. Ordinary checkouts keep
/// their own root; linked checkouts use Git's first worktree entry.
pub(crate) fn primary_worktree(checkout: &Path) -> Option<PathBuf> {
    let directories = optional(
        checkout,
        [
            "rev-parse",
            "--path-format=absolute",
            "--git-dir",
            "--git-common-dir",
        ],
    )?;
    let mut directories = directories.lines();
    if directories.next()? == directories.next()? {
        return Some(checkout.to_owned());
    }
    let output = optional(checkout, ["worktree", "list", "--porcelain", "-z"])?;
    let root = output.split('\0').next()?.strip_prefix("worktree ")?;
    Path::new(root).canonicalize().ok()
}

/// The bases recorded in `branch`'s config, most preferred first, as written
/// and without checking that they still resolve.
pub fn configured_bases(root: &Path, branch: &str) -> Vec<String> {
    BASE_CONFIG_KEYS
        .iter()
        .filter_map(|key| optional(root, ["config", "--get", &format!("branch.{branch}.{key}")]))
        .collect()
}

/// The branch new work starts from: origin's HEAD, else a local main or master.
pub fn default_branch(root: &Path) -> Option<String> {
    if let Some(reference) = optional(
        root,
        [
            "symbolic-ref",
            "--quiet",
            "--short",
            "refs/remotes/origin/HEAD",
        ],
    ) && let Some(branch) = reference.strip_prefix("origin/")
        && commit_exists(root, branch)
    {
        return Some(branch.to_owned());
    }
    ["main", "master"]
        .into_iter()
        .find(|branch| commit_exists(root, &format!("refs/heads/{branch}")))
        .map(str::to_owned)
}
