//! A worktree's uncommitted state, with content hashes to tell whether it changed.

use std::io::Write;
use std::os::unix::ffi::OsStrExt;
use std::path::{Path, PathBuf};
use std::process::{Command, Stdio};
use std::thread;

use super::reap::salvageable_paths;
use super::{WorktreeResult, git_text, os_args, run_git};

#[derive(Debug)]
pub(super) struct Status {
    pub(super) raw: String,
    pub(super) dirty: bool,
    pub(super) ignored_only: bool,
    pub(super) hash: String,
}

pub(super) fn status(worktree: &Path) -> WorktreeResult<Status> {
    let raw = git_text(
        worktree,
        [
            "status",
            "--porcelain=v2",
            "--ignored",
            "--untracked-files=all",
        ],
    )?;
    let mut changed = false;
    let mut ignored = false;
    for line in raw.lines() {
        changed |= line.starts_with("1 ")
            || line.starts_with("2 ")
            || line.starts_with("u ")
            || line.starts_with("? ");
        ignored |= line.starts_with("! ");
    }
    let mut fingerprint = raw.as_bytes().to_vec();
    let paths = salvageable_paths(worktree, &raw)?;
    for (path, object) in paths.iter().zip(object_ids(worktree, &paths)?) {
        fingerprint.push(0);
        fingerprint.extend_from_slice(path.as_os_str().as_bytes());
        fingerprint.push(0);
        fingerprint.extend_from_slice(&object);
    }
    let hash = glib::compute_checksum_for_data(glib::ChecksumType::Sha256, &fingerprint)
        .ok_or("could not hash worktree status")?
        .to_string();
    Ok(Status {
        raw,
        dirty: changed,
        ignored_only: !changed && ignored,
        hash,
    })
}

/// Object ids as `git hash-object` prints them, one per path. Readable files
/// share one process, since a process per file took seconds in large checkouts.
fn object_ids(worktree: &Path, paths: &[PathBuf]) -> WorktreeResult<Vec<Vec<u8>>> {
    let mut objects = Vec::with_capacity(paths.len());
    while objects.len() < paths.len() {
        let batch = paths[objects.len()..]
            .iter()
            .take_while(|path| worktree.join(path).is_file())
            .collect::<Vec<_>>();
        if !batch.is_empty() {
            objects.extend(hash_files(worktree, &batch)?);
        }
        // Deleted, unreadable or special paths keep the index fallback.
        if let Some(path) = paths.get(objects.len()) {
            objects.push(object_id(worktree, path)?);
        }
    }
    Ok(objects)
}

/// Returns the ids printed before the first path git could not read.
fn hash_files(worktree: &Path, paths: &[&PathBuf]) -> WorktreeResult<Vec<Vec<u8>>> {
    let mut input = Vec::new();
    for path in paths {
        // Quoted, so names with newlines or a leading quote reach git intact.
        quote_c_style(path.as_os_str().as_bytes(), &mut input);
        input.push(b'\n');
    }
    let mut child = Command::new("git")
        .args(["hash-object", "--no-filters", "--stdin-paths"])
        .current_dir(worktree)
        .stdin(Stdio::piped())
        .stdout(Stdio::piped())
        .stderr(Stdio::piped())
        .spawn()?;
    let mut stdin = child.stdin.take().ok_or("git hash-object has no stdin")?;
    // Write from another thread so a full stdout pipe cannot stall both sides.
    let writer = thread::spawn(move || stdin.write_all(&input));
    let output = child.wait_with_output()?;
    let _ = writer.join();
    Ok(output
        .stdout
        .split_inclusive(|byte| *byte == b'\n')
        .filter(|line| line.ends_with(b"\n"))
        .take(paths.len())
        .map(<[u8]>::to_vec)
        .collect())
}

fn object_id(worktree: &Path, path: &Path) -> WorktreeResult<Vec<u8>> {
    let mut arguments = os_args(&["hash-object", "--no-filters", "--"]);
    arguments.push(path.as_os_str().to_owned());
    run_git(worktree, arguments)
        .map(|output| output.stdout)
        .or_else(|_| {
            git_text(
                worktree,
                ["rev-parse", &format!(":{}", path.to_string_lossy())],
            )
            .map(String::into_bytes)
        })
}

fn quote_c_style(path: &[u8], output: &mut Vec<u8>) {
    output.push(b'"');
    for &byte in path {
        match byte {
            b'"' | b'\\' => output.extend_from_slice(&[b'\\', byte]),
            0x20..=0x7e => output.push(byte),
            _ => output.extend_from_slice(format!("\\{byte:03o}").as_bytes()),
        }
    }
    output.push(b'"');
}

pub(super) fn has_populated_submodules(worktree: &Path) -> bool {
    git_text(worktree, ["submodule", "status"]).is_ok_and(|output| {
        output
            .lines()
            .any(|line| !line.is_empty() && !line.starts_with('-'))
    })
}
