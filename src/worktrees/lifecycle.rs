//! Decides each worktree's lifecycle state and whether it is safe to reap.

use std::ffi::OsStr;
use std::path::Path;

use super::layout::{WorktreeLayout, normalize_path, sanitize};
use super::snapshot::Status;
use super::{PorcelainWorktree, ReapClass, WorktreeInfo, WorktreeState, git_success, git_text};

pub(super) struct ClassificationContext<'a> {
    pub(super) repo_root: &'a Path,
    pub(super) default_branch: &'a str,
    pub(super) layout: &'a WorktreeLayout,
    pub(super) now: u64,
}

pub(super) fn classify(
    worktree: PorcelainWorktree,
    is_primary: bool,
    status: Status,
    live_session_count: usize,
    context: &ClassificationContext<'_>,
) -> WorktreeInfo {
    let ClassificationContext {
        repo_root,
        default_branch,
        layout,
        now,
    } = *context;
    let clean = !status.dirty && !status.ignored_only;
    let path_text = worktree.path.to_string_lossy();
    let last_commit_at = worktree
        .head
        .as_deref()
        .and_then(|head| last_commit_at(repo_root, head));
    let idle_millis = last_commit_at.map(|last| now.saturating_sub(last));
    let recently_active =
        live_session_count > 0 || idle_millis.is_some_and(|idle| idle < 7 * 24 * 60 * 60 * 1_000);
    let upstream = worktree
        .branch
        .as_deref()
        .map(|branch| branch_upstream(repo_root, branch))
        .unwrap_or(BranchUpstream::None);
    let unpushed_count = worktree
        .branch
        .as_deref()
        .map(|branch| branch_ahead_of_default(repo_root, branch, default_branch));
    let ancestry_merged = worktree
        .head
        .as_deref()
        .is_some_and(|head| merged_into_default(repo_root, head, default_branch));
    let (state, reap_class, evidence) = if path_text.contains("/.claude/worktrees/") {
        (WorktreeState::Ephemeral, None, "Claude-managed worktree")
    } else if is_primary {
        (
            if recently_active {
                WorktreeState::Active
            } else {
                WorktreeState::Open
            },
            None,
            "primary worktree",
        )
    } else if worktree.detached {
        let review = worktree
            .path
            .file_name()
            .and_then(OsStr::to_str)
            .is_some_and(|name| {
                name.starts_with("pr-")
                    && name[3..].chars().next().is_some_and(|c| c.is_ascii_digit())
            });
        (
            if review {
                WorktreeState::Review
            } else {
                WorktreeState::Unknown
            },
            None,
            if review {
                "detached PR review"
            } else {
                "detached HEAD"
            },
        )
    } else if upstream == BranchUpstream::Gone {
        (
            WorktreeState::Merged,
            Some(if clean && !worktree.locked && ancestry_merged {
                ReapClass::Safe
            } else {
                ReapClass::Check
            }),
            if ancestry_merged {
                "upstream gone, merged into default branch"
            } else {
                "upstream gone, merge unproven"
            },
        )
    } else if recently_active {
        (
            WorktreeState::Active,
            None,
            if live_session_count > 0 {
                "live session"
            } else {
                "commit activity within 7 days"
            },
        )
    } else if ancestry_merged {
        (
            WorktreeState::Merged,
            Some(if clean && !worktree.locked {
                ReapClass::Safe
            } else {
                ReapClass::Check
            }),
            "merged into default branch",
        )
    } else if unpushed_count.is_some_and(|count| count > 0) && upstream == BranchUpstream::None {
        (
            WorktreeState::LocalOnly,
            None,
            "never pushed, local commits",
        )
    } else if unpushed_count.is_some_and(|count| count > 0) && upstream == BranchUpstream::Live {
        (WorktreeState::Open, None, "upstream configured")
    } else if idle_millis.is_some_and(|idle| idle >= 14 * 24 * 60 * 60 * 1_000) {
        (WorktreeState::Stale, None, "idle for at least 14 days")
    } else {
        (
            WorktreeState::Unknown,
            None,
            "lifecycle evidence incomplete",
        )
    };
    let drifted = !is_primary
        && worktree.branch.as_deref().is_some_and(|branch| {
            let base = worktree.path.file_name().and_then(OsStr::to_str);
            let branch = sanitize(branch);
            base.is_none_or(|base| base != branch && !base.ends_with(&format!("-{branch}")))
        });
    let off_convention = !is_primary
        && worktree.branch.as_deref().is_some_and(|branch| {
            layout
                .path_for(branch)
                .is_ok_and(|expected| normalize_path(&worktree.path) != expected)
        });
    WorktreeInfo {
        path: worktree.path,
        head: worktree.head,
        branch: worktree.branch,
        is_primary,
        state,
        reap_class,
        evidence: evidence.to_owned(),
        dirty: status.dirty,
        ignored_only: status.ignored_only,
        status_hash: status.hash,
        locked: worktree.locked,
        prunable: worktree.prunable,
        live_session_count,
        last_commit_at,
        unpushed_count,
        drifted,
        off_convention,
        upstream_gone: upstream == BranchUpstream::Gone,
        never_pushed: upstream == BranchUpstream::None,
    }
}

fn last_commit_at(repo_root: &Path, head: &str) -> Option<u64> {
    git_text(repo_root, ["show", "-s", "--format=%ct", head])
        .ok()?
        .trim()
        .parse::<u64>()
        .ok()?
        .checked_mul(1_000)
}

pub(super) fn merged_into_default(repo_root: &Path, head: &str, default_branch: &str) -> bool {
    git_success(
        repo_root,
        ["merge-base", "--is-ancestor", head, default_branch],
    )
}

fn branch_ahead_of_default(repo_root: &Path, branch: &str, default_branch: &str) -> u64 {
    git_text(
        repo_root,
        [
            "rev-list",
            "--count",
            &format!("{default_branch}..{branch}"),
        ],
    )
    .ok()
    .and_then(|count| count.trim().parse().ok())
    .unwrap_or(0)
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum BranchUpstream {
    None,
    RemoteKnown,
    Live,
    Gone,
}

fn branch_upstream(repo_root: &Path, branch: &str) -> BranchUpstream {
    if git_success(
        repo_root,
        [
            "rev-parse",
            "--verify",
            "--quiet",
            &format!("{branch}@{{upstream}}"),
        ],
    ) {
        return BranchUpstream::Live;
    }
    let has_remote = git_success(
        repo_root,
        ["config", "--get", &format!("branch.{branch}.remote")],
    );
    let has_merge = git_success(
        repo_root,
        ["config", "--get", &format!("branch.{branch}.merge")],
    );
    if has_remote && has_merge {
        BranchUpstream::Gone
    } else if git_success(
        repo_root,
        [
            "rev-parse",
            "--verify",
            "--quiet",
            &format!("refs/remotes/origin/{branch}"),
        ],
    ) {
        BranchUpstream::RemoteKnown
    } else {
        BranchUpstream::None
    }
}
