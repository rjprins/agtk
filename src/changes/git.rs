use std::ffi::{OsStr, OsString};
use std::os::unix::ffi::OsStrExt;
use std::path::Path;
use std::process::Command;
use std::time::Duration;

use serde::{Deserialize, Serialize};

use crate::command_runner::{BoundedByteOutput, CommandError, run_bounded_bytes};

use super::{ChangedFile, CommitSummary};

pub(super) const GIT_TIMEOUT: Duration = Duration::from_secs(5);
pub(super) const GIT_OUTPUT_LIMIT: usize = 16 * 1024 * 1024;

#[derive(Debug, Clone, PartialEq, Eq, Hash, Serialize, Deserialize)]
#[serde(tag = "kind", content = "oid", rename_all = "camelCase")]
pub enum DiffScope {
    All,
    Staged,
    Unstaged,
    Untracked,
    Committed,
    Commit { oid: String },
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Serialize, Deserialize)]
#[serde(rename_all = "lowercase")]
pub enum ChangeStatus {
    Added,
    Modified,
    Deleted,
    Renamed,
    Copied,
    TypeChanged,
    Unmerged,
    Untracked,
    Unknown,
}

impl ChangeStatus {
    fn from_code(code: u8) -> Self {
        match code {
            b'A' => Self::Added,
            b'M' => Self::Modified,
            b'D' => Self::Deleted,
            b'R' => Self::Renamed,
            b'C' => Self::Copied,
            b'T' => Self::TypeChanged,
            b'U' => Self::Unmerged,
            _ => Self::Unknown,
        }
    }
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct GitStatusFile {
    pub path: Vec<u8>,
    pub old_path: Option<Vec<u8>>,
    pub index_status: u8,
    pub worktree_status: u8,
    pub submodule: Vec<u8>,
    pub head_mode: Option<Vec<u8>>,
    pub index_mode: Option<Vec<u8>>,
    pub worktree_mode: Option<Vec<u8>>,
    pub head_oid: Option<Vec<u8>>,
    pub index_oid: Option<Vec<u8>>,
    pub unmerged: bool,
    pub untracked: bool,
}

pub(super) fn run_git<I, S>(root: &Path, arguments: I) -> Result<BoundedByteOutput, String>
where
    I: IntoIterator<Item = S>,
    S: AsRef<OsStr>,
{
    let mut command = Command::new("git");
    command
        .current_dir(root)
        // An fsmonitor hook in .git/config would otherwise run on every refresh.
        .args(["-c", "core.fsmonitor=false"])
        .arg("--literal-pathspecs")
        .args(arguments)
        .env_remove("GIT_DIR")
        .env_remove("GIT_WORK_TREE")
        .env_remove("GIT_INDEX_FILE")
        .env_remove("GIT_EXTERNAL_DIFF")
        .env("GIT_OPTIONAL_LOCKS", "0")
        .env("GIT_TERMINAL_PROMPT", "0")
        .env("LC_ALL", "C");
    run_bounded_bytes(command, GIT_TIMEOUT, GIT_OUTPUT_LIMIT).map_err(|error| match error {
        CommandError::TimedOut(timeout) => {
            format!("Git command timed out after {} ms", timeout.as_millis())
        }
        CommandError::OutputExceeded(limit) => {
            format!("Git command output exceeded {limit} bytes")
        }
        other => other.to_string(),
    })
}

pub(super) fn git_argument(value: impl AsRef<OsStr>) -> OsString {
    value.as_ref().to_os_string()
}

pub(super) fn git_path_argument(path: &[u8]) -> OsString {
    OsStr::from_bytes(path).to_os_string()
}

pub(super) fn status_files(root: &Path) -> Result<Vec<GitStatusFile>, String> {
    let output = run_checked(
        root,
        [
            "status",
            "--porcelain=v2",
            "-z",
            "--untracked-files=all",
            "--ignore-submodules=none",
        ],
    )?;
    parse_status_porcelain_v2_z(&output.stdout)
}

pub(super) fn diff_name_status<I, S>(
    root: &Path,
    arguments: I,
    scope: DiffScope,
) -> Result<Vec<ChangedFile>, String>
where
    I: IntoIterator<Item = S>,
    S: AsRef<OsStr>,
{
    let output = run_checked(root, arguments)?;
    parse_name_status_z(&output.stdout, scope)
}

pub(super) fn indexed_paths(root: &Path) -> Result<Vec<Vec<u8>>, String> {
    let output = run_checked(root, ["ls-files", "--cached", "-z"])?;
    Ok(output
        .stdout
        .split(|byte| *byte == 0)
        .filter(|path| !path.is_empty())
        .map(<[u8]>::to_vec)
        .collect())
}

/// Tracked and untracked files that are not ignored, as worktree-relative paths.
pub(super) fn worktree_files(root: &Path) -> Result<Vec<Vec<u8>>, String> {
    let output = run_checked(
        root,
        [
            "ls-files",
            "-z",
            "--cached",
            "--others",
            "--exclude-standard",
        ],
    )?;
    Ok(output
        .stdout
        .split(|byte| *byte == 0)
        .filter(|path| !path.is_empty())
        .map(<[u8]>::to_vec)
        .collect())
}

pub(super) fn base_refs(root: &Path) -> Result<Vec<String>, String> {
    let output = run_checked(
        root,
        [
            "for-each-ref",
            "--format=%(refname:short)",
            "refs/heads",
            "refs/remotes",
        ],
    )?;
    let mut refs = output
        .stdout
        .split(|byte| *byte == b'\n')
        .filter(|reference| !reference.is_empty())
        .filter_map(|reference| std::str::from_utf8(reference).ok())
        .filter(|reference| !reference.ends_with("/HEAD"))
        .map(str::to_owned)
        .collect::<Vec<_>>();
    refs.sort();
    refs.dedup();
    Ok(refs)
}

pub(super) fn index_identity(root: &Path) -> Result<Vec<u8>, String> {
    let output = run_checked(root, ["ls-files", "--stage", "-z"])?;
    Ok(output.stdout)
}

pub(super) fn commit_page(
    root: &Path,
    base_oid: &str,
    head_oid: &str,
    offset: usize,
) -> Result<(Vec<CommitSummary>, bool), String> {
    const PAGE_SIZE: usize = 50;
    let range = format!("{base_oid}..{head_oid}");
    let skip = format!("--skip={}", offset.min(100_000));
    let count = format!("--max-count={}", PAGE_SIZE + 1);
    let output = run_checked(
        root,
        [
            git_argument("log"),
            git_argument("--first-parent"),
            git_argument("--topo-order"),
            git_argument("-z"),
            git_argument("--format=%H%x00%an%x00%at%x00%s"),
            git_argument(skip),
            git_argument(count),
            git_argument(range),
        ],
    )?;
    // Each commit ends in NUL. Fields can be empty, such as a commit without a
    // message, so only the empty piece after the last NUL is dropped.
    let mut fields = output.stdout.split(|byte| *byte == 0).collect::<Vec<_>>();
    if fields.last().is_some_and(|field| field.is_empty()) {
        fields.pop();
    }
    if fields.len() % 4 != 0 {
        return Err("Git returned a malformed commit page".to_owned());
    }
    let mut commits = Vec::with_capacity(fields.len() / 4);
    for record in fields.as_chunks::<4>().0.iter().take(PAGE_SIZE + 1) {
        let oid = String::from_utf8_lossy(record[0]).into_owned();
        if oid.len() != 40 && oid.len() != 64 {
            return Err("Git returned an invalid commit object ID".to_owned());
        }
        let author = String::from_utf8_lossy(record[1]).into_owned();
        let authored_at = std::str::from_utf8(record[2])
            .ok()
            .and_then(|value| value.parse::<i64>().ok());
        let subject = String::from_utf8_lossy(record[3]).into_owned();
        commits.push(CommitSummary {
            oid,
            subject,
            author,
            authored_at,
        });
    }
    let has_more = commits.len() > PAGE_SIZE;
    commits.truncate(PAGE_SIZE);
    Ok((commits, has_more))
}

pub(super) fn commit_in_first_parent_range(
    root: &Path,
    base_oid: &str,
    head_oid: &str,
    commit_oid: &str,
) -> Result<bool, String> {
    let range = format!("{base_oid}..{head_oid}");
    let output = run_checked(
        root,
        [
            git_argument("rev-list"),
            git_argument("--first-parent"),
            git_argument(range),
        ],
    )?;
    Ok(output
        .stdout
        .split(|byte| byte.is_ascii_whitespace())
        .any(|candidate| candidate == commit_oid.as_bytes()))
}

pub(super) fn run_checked<I, S>(root: &Path, arguments: I) -> Result<BoundedByteOutput, String>
where
    I: IntoIterator<Item = S>,
    S: AsRef<OsStr>,
{
    let output = run_git(root, arguments)?;
    if output.status.success() {
        Ok(output)
    } else {
        let message = String::from_utf8_lossy(&output.stderr).trim().to_owned();
        Err(if message.is_empty() {
            "Git read command failed".to_owned()
        } else {
            message
        })
    }
}

pub fn parse_status_porcelain_v2_z(input: &[u8]) -> Result<Vec<GitStatusFile>, String> {
    let records = input.split(|byte| *byte == 0).collect::<Vec<_>>();
    let mut files = Vec::new();
    let mut index = 0;

    while index < records.len() {
        let record = records[index];
        index += 1;
        if record.is_empty() || record[0] == b'#' || record.starts_with(b"! ") {
            continue;
        }
        match record[0] {
            b'1' => {
                let (fields, path) = split_fields(record, 8)?;
                files.push(GitStatusFile {
                    path: path.to_vec(),
                    old_path: None,
                    index_status: fields[1].first().copied().unwrap_or(b'.'),
                    worktree_status: fields[1].get(1).copied().unwrap_or(b'.'),
                    submodule: fields[2].to_vec(),
                    head_mode: Some(fields[3].to_vec()),
                    index_mode: Some(fields[4].to_vec()),
                    worktree_mode: Some(fields[5].to_vec()),
                    head_oid: Some(fields[6].to_vec()),
                    index_oid: Some(fields[7].to_vec()),
                    unmerged: false,
                    untracked: false,
                });
            }
            b'2' => {
                let (fields, path) = split_fields(record, 9)?;
                let old_path = records
                    .get(index)
                    .copied()
                    .filter(|path| !path.is_empty())
                    .ok_or_else(|| "porcelain-v2 rename entry has no original path".to_owned())?;
                index += 1;
                files.push(GitStatusFile {
                    path: path.to_vec(),
                    old_path: Some(old_path.to_vec()),
                    index_status: fields[1].first().copied().unwrap_or(b'.'),
                    worktree_status: fields[1].get(1).copied().unwrap_or(b'.'),
                    submodule: fields[2].to_vec(),
                    head_mode: Some(fields[3].to_vec()),
                    index_mode: Some(fields[4].to_vec()),
                    worktree_mode: Some(fields[5].to_vec()),
                    head_oid: Some(fields[6].to_vec()),
                    index_oid: Some(fields[7].to_vec()),
                    unmerged: false,
                    untracked: false,
                });
            }
            b'u' => {
                let (fields, path) = split_fields(record, 10)?;
                files.push(GitStatusFile {
                    path: path.to_vec(),
                    old_path: None,
                    index_status: fields[1].first().copied().unwrap_or(b'.'),
                    worktree_status: fields[1].get(1).copied().unwrap_or(b'.'),
                    submodule: fields[2].to_vec(),
                    head_mode: None,
                    index_mode: None,
                    worktree_mode: Some(fields[6].to_vec()),
                    head_oid: None,
                    index_oid: None,
                    unmerged: true,
                    untracked: false,
                });
            }
            b'?' if record.starts_with(b"? ") => files.push(GitStatusFile {
                path: record[2..].to_vec(),
                old_path: None,
                index_status: b'?',
                worktree_status: b'?',
                submodule: b"N...".to_vec(),
                head_mode: None,
                index_mode: None,
                worktree_mode: None,
                head_oid: None,
                index_oid: None,
                unmerged: false,
                untracked: true,
            }),
            kind => {
                return Err(format!(
                    "unknown porcelain-v2 status record type {:?}",
                    char::from(kind)
                ));
            }
        }
    }

    Ok(files)
}

pub fn parse_name_status_z(
    input: &[u8],
    scope: DiffScope,
) -> Result<Vec<super::ChangedFile>, String> {
    let records = input.split(|byte| *byte == 0).collect::<Vec<_>>();
    let mut files = Vec::new();
    let mut index = 0;

    while index < records.len() {
        let status = records[index];
        index += 1;
        if status.is_empty() {
            continue;
        }
        let code = status[0];
        let kind = ChangeStatus::from_code(code);
        if kind == ChangeStatus::Unknown {
            return Err(format!(
                "unknown git name-status code {:?}",
                char::from(code)
            ));
        }
        let path = records
            .get(index)
            .copied()
            .filter(|path| !path.is_empty())
            .ok_or_else(|| "git name-status record has no path".to_owned())?;
        index += 1;
        let (old_path, new_path) = match kind {
            ChangeStatus::Added => (None, Some(path.to_vec())),
            ChangeStatus::Deleted => (Some(path.to_vec()), None),
            ChangeStatus::Renamed | ChangeStatus::Copied => {
                let target = records
                    .get(index)
                    .copied()
                    .filter(|path| !path.is_empty())
                    .ok_or_else(|| "git rename/copy record has no destination path".to_owned())?;
                index += 1;
                (Some(path.to_vec()), Some(target.to_vec()))
            }
            _ => (Some(path.to_vec()), Some(path.to_vec())),
        };
        files.push(super::ChangedFile {
            old_path,
            new_path,
            status: kind,
            scope: scope.clone(),
            old_mode: None,
            new_mode: None,
            old_blob: None,
            new_blob: None,
        });
    }
    Ok(files)
}

pub(super) fn output_text(output: &BoundedByteOutput) -> String {
    String::from_utf8_lossy(&output.stdout).trim().to_owned()
}

pub(super) fn oid_from_output(output: &BoundedByteOutput) -> Option<String> {
    let value = trim_ascii(&output.stdout);
    if (value.len() == 40 || value.len() == 64) && value.iter().all(u8::is_ascii_hexdigit) {
        Some(String::from_utf8_lossy(value).into_owned())
    } else {
        None
    }
}

fn split_fields(record: &[u8], count: usize) -> Result<(Vec<&[u8]>, &[u8]), String> {
    let mut fields = Vec::with_capacity(count);
    let mut rest = record;
    for _ in 0..count {
        let Some(separator) = rest.iter().position(|byte| *byte == b' ') else {
            return Err("malformed porcelain-v2 status record".to_owned());
        };
        fields.push(&rest[..separator]);
        rest = &rest[separator + 1..];
    }
    if rest.is_empty() {
        return Err("porcelain-v2 status record has an empty path".to_owned());
    }
    Ok((fields, rest))
}

fn trim_ascii(mut value: &[u8]) -> &[u8] {
    while value.first().is_some_and(u8::is_ascii_whitespace) {
        value = &value[1..];
    }
    while value.last().is_some_and(u8::is_ascii_whitespace) {
        value = &value[..value.len() - 1];
    }
    value
}
