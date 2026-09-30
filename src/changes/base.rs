use std::os::unix::ffi::OsStringExt;
use std::path::{Path, PathBuf};

use super::WorktreeContext;
use super::git::{git_argument, oid_from_output, output_text, run_git};

// The agmux key is still set on branches the old agmux created.
const BASE_CONFIG_KEYS: [&str; 5] = [
    "agtk-base-branch",
    "agmux-base-branch",
    "vscode-merge-base",
    "gh-merge-base",
    "merge-base",
];

pub fn resolve_worktree_context(
    path: &Path,
    preferred_base: Option<&str>,
    generation: u64,
) -> Result<Option<WorktreeContext>, String> {
    let path = path
        .canonicalize()
        .map_err(|error| format!("worktree path is unavailable: {error}"))?;
    if !path.is_dir() {
        return Err("worktree path is not a directory".to_owned());
    }

    let top = run_git(&path, ["rev-parse", "--show-toplevel"])?;
    if !top.status.success() {
        return Ok(None);
    }
    let root = PathBuf::from(std::ffi::OsString::from_vec(trim_line_ending(&top.stdout)));
    let root = root
        .canonicalize()
        .map_err(|error| format!("could not canonicalize Git worktree root: {error}"))?;

    let head_output = run_git(
        &root,
        ["rev-parse", "--verify", "--end-of-options", "HEAD^{commit}"],
    )?;
    let head_oid = head_output
        .status
        .success()
        .then(|| oid_from_output(&head_output))
        .flatten();

    let branch_output = run_git(&root, ["symbolic-ref", "--quiet", "--short", "HEAD"])?;
    let branch = branch_output
        .status
        .success()
        .then(|| output_text(&branch_output))
        .filter(|branch| !branch.is_empty());
    let detached = head_oid.is_some() && branch.is_none();

    let selected = select_base(&root, preferred_base)?;
    let merge_base_oid = match (&selected, &head_oid) {
        (Some((_, base_oid)), Some(head_oid)) => {
            let output = run_git(&root, ["merge-base", base_oid.as_str(), head_oid.as_str()])?;
            output
                .status
                .success()
                .then(|| oid_from_output(&output))
                .flatten()
        }
        _ => None,
    };
    let comparison_warning = match (
        head_oid.is_none(),
        selected.is_some(),
        merge_base_oid.is_none(),
    ) {
        (true, _, _) => None,
        (false, false, _) => Some(match preferred_base.and_then(commits_ago_count) {
            Some(count) => format!(
                "Cannot compare with {count} {} ago. That history is not available in this repository. Choose a smaller number.",
                if count == 1 { "commit" } else { "commits" }
            ),
            None => "Choose a comparison base".to_owned(),
        }),
        (false, true, true) => {
            Some("The selected base has no common ancestor with this worktree".to_owned())
        }
        _ => None,
    };

    Ok(Some(WorktreeContext {
        root,
        branch,
        head_oid: head_oid.clone(),
        base_ref: selected
            .as_ref()
            .map(|(reference, _)| reference.clone())
            .or_else(|| {
                preferred_base
                    .filter(|reference| commits_ago_count(reference).is_some())
                    .map(str::to_owned)
            }),
        base_oid: selected.map(|(_, oid)| oid),
        merge_base_oid,
        comparison_warning,
        detached,
        unborn: head_oid.is_none(),
        generation,
    }))
}

/// Recognize the bounded positive ancestor counts offered by the Changes sidebar.
pub fn commits_ago_count(reference: &str) -> Option<i32> {
    let count = reference.strip_prefix("HEAD~")?;
    if count.is_empty() || !count.bytes().all(|byte| byte.is_ascii_digit()) {
        return None;
    }
    count.parse::<i32>().ok().filter(|count| *count > 0)
}

pub fn validate_base_ref(root: &Path, reference: &str) -> Result<Option<String>, String> {
    if reference.is_empty() || reference.len() > 1024 || reference.contains('\0') {
        return Ok(None);
    }
    let revision = format!("{reference}^{{commit}}");
    let output = run_git(
        root,
        [
            git_argument("rev-parse"),
            git_argument("--verify"),
            git_argument("--end-of-options"),
            git_argument(revision),
        ],
    )?;
    if !output.status.success() {
        return Ok(None);
    }
    Ok(oid_from_output(&output))
}

fn select_base(
    root: &Path,
    preferred_base: Option<&str>,
) -> Result<Option<(String, String)>, String> {
    if let Some(reference) = preferred_base {
        if let Some(oid) = validate_base_ref(root, reference)? {
            return Ok(Some((reference.to_owned(), oid)));
        }
        // An unavailable ancestor must not silently select an unrelated branch.
        if commits_ago_count(reference).is_some() {
            return Ok(None);
        }
    }

    for key in BASE_CONFIG_KEYS {
        let output = run_git(root, ["config", "--get", key])?;
        if !output.status.success() {
            continue;
        }
        let reference = output_text(&output);
        if let Some(oid) = validate_base_ref(root, &reference)? {
            return Ok(Some((reference, oid)));
        }
    }

    let origin_head = run_git(
        root,
        [
            "symbolic-ref",
            "--quiet",
            "--short",
            "refs/remotes/origin/HEAD",
        ],
    )?;
    if origin_head.status.success() {
        let reference = output_text(&origin_head);
        if let Some(oid) = validate_base_ref(root, &reference)? {
            return Ok(Some((reference, oid)));
        }
    }

    for branch in ["main", "master"] {
        let reference = format!("refs/heads/{branch}");
        if let Some(oid) = validate_base_ref(root, &reference)? {
            return Ok(Some((branch.to_owned(), oid)));
        }
    }
    Ok(None)
}

fn trim_line_ending(bytes: &[u8]) -> Vec<u8> {
    let mut end = bytes.len();
    while end > 0 && matches!(bytes[end - 1], b'\n' | b'\r') {
        end -= 1;
    }
    bytes[..end].to_vec()
}
