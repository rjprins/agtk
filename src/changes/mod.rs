mod base;
mod content;
mod git;

pub use base::{resolve_worktree_context, validate_base_ref};
pub use content::{DiffDocumentResult, DiffPlaceholder, display_changed_path, read_diff_document};
pub use git::{
    ChangeStatus, DiffScope, GitStatusFile, parse_name_status_z, parse_status_porcelain_v2_z,
};

pub fn list_base_refs(root: &Path) -> Result<Vec<String>, String> {
    git::base_refs(root)
}

pub fn commit_is_in_branch(context: &WorktreeContext, commit_oid: &str) -> Result<bool, String> {
    let (Some(base), Some(head)) = (context.base_oid.as_deref(), context.head_oid.as_deref())
    else {
        return Ok(false);
    };
    if context.comparison_warning.is_some()
        || (commit_oid.len() != 40 && commit_oid.len() != 64)
        || !commit_oid
            .bytes()
            .all(|byte| byte.is_ascii_digit() || (b'a'..=b'f').contains(&byte))
    {
        return Ok(false);
    }
    git::commit_in_first_parent_range(&context.root, base, head, commit_oid)
}

pub fn read_more_commits(
    context: &WorktreeContext,
    offset: usize,
) -> Result<(Vec<CommitSummary>, bool), String> {
    let (Some(base), Some(head)) = (context.base_oid.as_deref(), context.head_oid.as_deref())
    else {
        return Ok((Vec::new(), false));
    };
    if context.comparison_warning.is_some() {
        return Ok((Vec::new(), false));
    }
    git::commit_page(&context.root, base, head, offset)
}

use std::collections::HashSet;
use std::ffi::OsStr;
use std::fs;
use std::os::unix::ffi::OsStrExt;
use std::path::PathBuf;
use std::path::{Component, Path};

use serde::{Deserialize, Serialize};

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct WorktreeContext {
    pub root: PathBuf,
    pub branch: Option<String>,
    pub head_oid: Option<String>,
    pub base_ref: Option<String>,
    pub base_oid: Option<String>,
    pub merge_base_oid: Option<String>,
    pub comparison_warning: Option<String>,
    pub detached: bool,
    pub unborn: bool,
    pub generation: u64,
}

#[derive(Debug, Clone, PartialEq, Eq, Hash)]
pub struct ChangedFile {
    pub old_path: Option<Vec<u8>>,
    pub new_path: Option<Vec<u8>>,
    pub status: ChangeStatus,
    pub scope: DiffScope,
    pub old_mode: Option<String>,
    pub new_mode: Option<String>,
    pub old_blob: Option<String>,
    pub new_blob: Option<String>,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct CommitSummary {
    pub oid: String,
    pub subject: String,
    pub author: String,
    pub authored_at: Option<i64>,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ChangesSnapshot {
    pub context: WorktreeContext,
    pub all_changes: Vec<ChangedFile>,
    pub staged: Vec<ChangedFile>,
    pub unstaged: Vec<ChangedFile>,
    pub untracked: Vec<ChangedFile>,
    pub conflicts: Vec<ChangedFile>,
    pub branch_changes: Vec<ChangedFile>,
    pub commits: Vec<CommitSummary>,
    pub has_more_commits: bool,
    pub warnings: Vec<String>,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct DiffDocument {
    pub path: String,
    pub original_label: String,
    pub modified_label: String,
    pub original: String,
    pub modified: String,
    pub language: Option<String>,
    pub identity: String,
    pub metadata: Vec<String>,
}

pub fn read_commit_files(
    context: &WorktreeContext,
    commit_oid: &str,
) -> Result<Vec<ChangedFile>, String> {
    if commit_oid.len() != 40 && commit_oid.len() != 64
        || !commit_oid.bytes().all(|byte| byte.is_ascii_hexdigit())
    {
        return Err("Commit ID must be a full object ID".to_owned());
    }
    let parents = git::run_checked(
        &context.root,
        [
            git::git_argument("rev-list"),
            git::git_argument("--parents"),
            git::git_argument("-n"),
            git::git_argument("1"),
            git::git_argument("--end-of-options"),
            git::git_argument(commit_oid),
        ],
    )?;
    let fields = parents
        .stdout
        .split(|byte| byte.is_ascii_whitespace())
        .filter(|field| !field.is_empty())
        .collect::<Vec<_>>();
    if fields.first().is_none_or(|field| !valid_object_id(field)) {
        return Err("Git returned an invalid commit identity".to_owned());
    }
    let actual_oid = std::str::from_utf8(fields[0])
        .map_err(|_| "Git returned an invalid commit identity".to_owned())?;
    if actual_oid != commit_oid {
        return Err("Git resolved a different commit identity".to_owned());
    }
    let parent = fields
        .get(1)
        .copied()
        .filter(|field| valid_object_id(field));
    let mut args = vec![
        git::git_argument("diff-tree"),
        git::git_argument("--no-commit-id"),
        git::git_argument("-r"),
        git::git_argument("--name-status"),
        git::git_argument("-z"),
        git::git_argument("-M"),
        git::git_argument("--no-ext-diff"),
        git::git_argument("--no-textconv"),
    ];
    if let Some(parent) = parent {
        let parent = std::str::from_utf8(parent)
            .map_err(|_| "Git returned an invalid parent identity".to_owned())?;
        args.push(git::git_argument(parent));
        args.push(git::git_argument(commit_oid));
    } else {
        args.push(git::git_argument("--root"));
        args.push(git::git_argument(commit_oid));
    }
    args.push(git::git_argument("--"));
    let mut files = git::diff_name_status(
        &context.root,
        args,
        DiffScope::Commit {
            oid: commit_oid.to_owned(),
        },
    )?;
    for file in &mut files {
        content::capture_file_metadata(context, file)?;
    }
    Ok(files)
}

fn valid_object_id(value: &[u8]) -> bool {
    (value.len() == 40 || value.len() == 64) && value.iter().all(u8::is_ascii_hexdigit)
}

pub fn read_changes_snapshot(
    path: &Path,
    preferred_base: Option<&str>,
    generation: u64,
    commit_offset: usize,
) -> Result<Option<ChangesSnapshot>, String> {
    for attempt in 0..=1 {
        let Some(context) = resolve_worktree_context(path, preferred_base, generation)? else {
            return Ok(None);
        };
        let before_index = git::index_identity(&context.root)?;
        let mut snapshot = assemble_snapshot(context.clone(), commit_offset)?;
        let after_index = git::index_identity(&context.root)?;
        let after_context = resolve_worktree_context(path, preferred_base, generation)?;
        let same_context = after_context.as_ref().is_some_and(|after| {
            after.head_oid == context.head_oid
                && after.base_ref == context.base_ref
                && after.base_oid == context.base_oid
                && after.merge_base_oid == context.merge_base_oid
        });
        if before_index == after_index && same_context {
            return Ok(Some(snapshot));
        }
        if attempt == 1 {
            snapshot
                .warnings
                .push("Git state changed while refreshing. This list may be partial.".to_owned());
            return Ok(Some(snapshot));
        }
    }
    unreachable!("the bounded snapshot retry loop always returns")
}

fn assemble_snapshot(
    context: WorktreeContext,
    commit_offset: usize,
) -> Result<ChangesSnapshot, String> {
    let status = git::status_files(&context.root)?;
    let mut staged_args = vec![
        "diff",
        "--cached",
        "--name-status",
        "-z",
        "-M",
        "--no-ext-diff",
        "--no-textconv",
    ];
    if let Some(head) = context.head_oid.as_deref() {
        staged_args.push(head);
    }
    staged_args.push("--");
    let mut staged = git::diff_name_status(&context.root, staged_args, DiffScope::Staged)?;
    let mut unstaged = git::diff_name_status(
        &context.root,
        [
            "diff",
            "--name-status",
            "-z",
            "-M",
            "--no-ext-diff",
            "--no-textconv",
            "--",
        ],
        DiffScope::Unstaged,
    )?;

    let mut all_changes = if let Some(merge_base) = context.merge_base_oid.as_deref() {
        git::diff_name_status(
            &context.root,
            [
                "diff",
                "--name-status",
                "-z",
                "-M",
                "--no-ext-diff",
                "--no-textconv",
                merge_base,
                "--",
            ],
            DiffScope::All,
        )?
    } else if context.unborn {
        unborn_worktree_files(&context.root, &status)?
    } else {
        Vec::new()
    };

    let untracked = status
        .iter()
        .filter(|file| file.untracked)
        .map(|file| ChangedFile {
            old_path: None,
            new_path: Some(file.path.clone()),
            status: ChangeStatus::Untracked,
            scope: DiffScope::Untracked,
            old_mode: None,
            new_mode: None,
            old_blob: None,
            new_blob: None,
        })
        .collect::<Vec<_>>();
    let mut all_paths = all_changes
        .iter()
        .flat_map(|file| [file.old_path.as_ref(), file.new_path.as_ref()])
        .flatten()
        .cloned()
        .collect::<HashSet<_>>();
    if context.unborn {
        for file in &untracked {
            let Some(path) = file.new_path.as_ref() else {
                continue;
            };
            if all_paths.insert(path.clone()) {
                all_changes.push(ChangedFile {
                    old_path: None,
                    new_path: Some(path.clone()),
                    status: ChangeStatus::Added,
                    scope: DiffScope::All,
                    old_mode: None,
                    new_mode: None,
                    old_blob: None,
                    new_blob: None,
                });
            }
        }
    } else {
        for file in &untracked {
            let Some(path) = file.new_path.as_ref() else {
                continue;
            };
            if all_paths.insert(path.clone()) {
                all_changes.push(ChangedFile {
                    old_path: None,
                    new_path: Some(path.clone()),
                    status: ChangeStatus::Untracked,
                    scope: DiffScope::All,
                    old_mode: None,
                    new_mode: None,
                    old_blob: None,
                    new_blob: None,
                });
            }
        }
    }

    let conflicts = status
        .iter()
        .filter(|file| file.unmerged)
        .map(|file| ChangedFile {
            old_path: Some(file.path.clone()),
            new_path: Some(file.path.clone()),
            status: ChangeStatus::Unmerged,
            scope: DiffScope::All,
            old_mode: None,
            new_mode: file
                .worktree_mode
                .as_ref()
                .map(|mode| String::from_utf8_lossy(mode).into_owned()),
            old_blob: None,
            new_blob: None,
        })
        .collect::<Vec<_>>();
    let conflict_paths = conflicts
        .iter()
        .flat_map(|file| [file.old_path.as_ref(), file.new_path.as_ref()])
        .flatten()
        .cloned()
        .collect::<HashSet<_>>();
    all_changes.retain(|file| !contains_any_path(file, &conflict_paths));
    staged.retain(|file| !contains_any_path(file, &conflict_paths));
    unstaged.retain(|file| !contains_any_path(file, &conflict_paths));
    all_changes.extend(conflicts.iter().cloned());

    let mut branch_changes = match (
        context.merge_base_oid.as_deref(),
        context.head_oid.as_deref(),
        context.comparison_warning.is_none(),
    ) {
        (Some(base), Some(head), true) => git::diff_name_status(
            &context.root,
            [
                "diff-tree",
                "--no-commit-id",
                "-r",
                "--name-status",
                "-z",
                "-M",
                "--no-ext-diff",
                "--no-textconv",
                base,
                head,
                "--",
            ],
            DiffScope::Committed,
        )?,
        _ => Vec::new(),
    };
    let (commits, has_more_commits) = match (
        context.base_oid.as_deref(),
        context.head_oid.as_deref(),
        context.comparison_warning.is_none(),
    ) {
        (Some(base), Some(head), true) => {
            git::commit_page(&context.root, base, head, commit_offset)?
        }
        _ => (Vec::new(), false),
    };
    let warnings = context
        .comparison_warning
        .iter()
        .cloned()
        .collect::<Vec<_>>();

    for file in staged
        .iter_mut()
        .chain(unstaged.iter_mut())
        .chain(all_changes.iter_mut())
        .chain(branch_changes.iter_mut())
    {
        content::capture_file_metadata(&context, file)?;
    }

    Ok(ChangesSnapshot {
        context,
        all_changes,
        staged,
        unstaged,
        untracked,
        conflicts,
        branch_changes,
        commits,
        has_more_commits,
        warnings,
    })
}

fn unborn_worktree_files(
    root: &Path,
    status: &[git::GitStatusFile],
) -> Result<Vec<ChangedFile>, String> {
    let mut paths = git::indexed_paths(root)?
        .into_iter()
        .collect::<HashSet<_>>();
    paths.extend(
        status
            .iter()
            .filter(|file| file.untracked)
            .map(|file| file.path.clone()),
    );
    let mut files = Vec::new();
    for raw_path in paths {
        let Some(path) = safe_worktree_path(root, &raw_path) else {
            continue;
        };
        if fs::symlink_metadata(path).is_ok() {
            files.push(ChangedFile {
                old_path: None,
                new_path: Some(raw_path),
                status: ChangeStatus::Added,
                scope: DiffScope::All,
                old_mode: None,
                new_mode: None,
                old_blob: None,
                new_blob: None,
            });
        }
    }
    Ok(files)
}

fn safe_worktree_path(root: &Path, raw_path: &[u8]) -> Option<PathBuf> {
    let path = Path::new(OsStr::from_bytes(raw_path));
    if path.is_absolute()
        || path.components().any(|component| {
            matches!(
                component,
                Component::ParentDir | Component::RootDir | Component::Prefix(_)
            )
        })
    {
        return None;
    }
    Some(root.join(path))
}

fn contains_any_path(file: &ChangedFile, paths: &HashSet<Vec<u8>>) -> bool {
    file.old_path
        .as_ref()
        .is_some_and(|path| paths.contains(path))
        || file
            .new_path
            .as_ref()
            .is_some_and(|path| paths.contains(path))
}
