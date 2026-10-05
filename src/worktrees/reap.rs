//! Removes a worktree after salvaging its uncommitted files and tagging its head.

use std::collections::BTreeSet;
use std::ffi::OsStr;
use std::fs;
use std::os::unix::ffi::OsStrExt;
use std::os::unix::fs::{DirBuilderExt, symlink};
use std::path::{Path, PathBuf};
use std::process::Command;

use super::layout::sanitize;
use super::lifecycle::merged_into_default;
use super::snapshot::{has_populated_submodules, status};
use super::{
    DeleteBranch, ReapRequest, ReapResult, WorktreeManager, WorktreeResult, checked_output,
    common_repo_root, git_success, git_text, now_millis, os_args, run_git,
};

impl WorktreeManager {
    pub fn reap(&self, request: ReapRequest, live_paths: &[PathBuf]) -> WorktreeResult<ReapResult> {
        let path = request.path.canonicalize()?;
        let repo_root = common_repo_root(&path)?;
        let inventory = self.list(&repo_root, live_paths)?;
        let Some(preview) = inventory
            .worktrees
            .iter()
            .find(|worktree| worktree.path == path)
        else {
            return Err("path is not a linked worktree of its repository".into());
        };
        if preview.is_primary {
            return Err("refusing to reap the primary worktree".into());
        }
        if preview.live_session_count > 0 {
            return Err("refusing to reap a worktree with live sessions".into());
        }
        if preview.reap_class.is_none() && !request.confirmed {
            return Err(format!(
                "refusing to reap a worktree classified as {:?}: {}",
                preview.state, preview.evidence
            )
            .into());
        }
        let head = preview
            .head
            .clone()
            .ok_or("worktree has no readable HEAD")?;
        if request.expected_head != head {
            return Ok(aborted(&repo_root, "HEAD moved since preview"));
        }
        let initial_status = status(&path)?;
        if request.expected_status_hash != initial_status.hash {
            return Ok(aborted(
                &repo_root,
                "worktree contents changed since preview",
            ));
        }
        if has_populated_submodules(&path) {
            return Err("worktree contains populated submodules".into());
        }
        let branch = git_text(&path, ["symbolic-ref", "--quiet", "--short", "HEAD"])
            .ok()
            .map(|branch| branch.trim().to_owned());
        let salvage_path = if initial_status.dirty || initial_status.ignored_only {
            self.salvage(
                &repo_root,
                &path,
                branch.as_deref(),
                &head,
                &initial_status.raw,
            )?
        } else {
            None
        };
        let final_status = status(&path)?;
        if final_status.hash != initial_status.hash {
            if let Some(salvage) = &salvage_path {
                discard_salvage(salvage);
            }
            return Ok(aborted(&repo_root, "worktree changed during reap"));
        }
        let mut remove = os_args(&["worktree", "remove"]);
        if final_status.dirty || final_status.ignored_only {
            remove.push("--force".into());
        }
        remove.push(path.as_os_str().to_owned());
        run_git(&repo_root, remove)?;

        let mut attic_tag = None;
        let mut branch_deleted = false;
        let mut reason = None;
        if let Some(branch) = branch
            && request.delete_branch != DeleteBranch::Never
        {
            let merge_proven = merged_into_default(&repo_root, &head, &inventory.default_branch);
            if merge_proven || request.delete_branch == DeleteBranch::Force {
                match self.write_attic_tag(&repo_root, &branch, &head, salvage_path.as_deref()) {
                    Ok(tag) => {
                        let reference = format!("refs/heads/{branch}");
                        if git_success(&repo_root, ["update-ref", "-d", &reference, &head]) {
                            branch_deleted = true;
                        } else {
                            reason = Some("branch kept because its ref moved".to_owned());
                        }
                        attic_tag = Some(tag);
                    }
                    Err(error) => {
                        reason = Some(format!(
                            "branch kept because its attic tag could not be created: {error}"
                        ));
                    }
                }
            } else {
                reason = Some("branch kept because merge is not proven".to_owned());
            }
        }
        Ok(ReapResult {
            ok: true,
            aborted: false,
            reason,
            repo_root,
            salvage_path,
            attic_tag,
            branch_deleted,
        })
    }

    fn salvage(
        &self,
        repo_root: &Path,
        worktree: &Path,
        branch: Option<&str>,
        head: &str,
        raw_status: &str,
    ) -> WorktreeResult<Option<PathBuf>> {
        let paths = salvageable_paths(worktree, raw_status)?;
        if paths.is_empty() {
            return Ok(None);
        }
        let nonce = now_millis();
        let stage =
            std::env::temp_dir().join(format!("agtk-salvage-{}-{nonce}", std::process::id()));
        // Copies keep their mode, so a 0644 .env must not sit in a readable /tmp dir.
        fs::DirBuilder::new().mode(0o700).create(&stage)?;
        let result = (|| {
            for path in &paths {
                stage_path(worktree, &stage, path)?;
            }
            let repo_name = repo_root
                .file_name()
                .and_then(OsStr::to_str)
                .unwrap_or("repository");
            let slug = sanitize(branch.unwrap_or("detached"));
            let directory = self.attic_root.join(repo_name);
            fs::create_dir_all(&directory)?;
            let archive = directory.join(format!(
                "{slug}-{nonce}-{}.tar.gz",
                &head[..head.len().min(7)]
            ));
            let mut tar = Command::new("tar");
            tar.args(["-czf"])
                .arg(&archive)
                .arg("-C")
                .arg(&stage)
                .arg(".");
            checked_output(tar)?;
            let manifest = archive.with_extension("manifest.json");
            let data = serde_json::to_vec_pretty(&serde_json::json!({
                "branch": branch,
                "head": head,
                "sourcePath": worktree,
                "files": paths,
            }))?;
            fs::write(manifest, data)?;
            Ok(Some(archive))
        })();
        let _ = fs::remove_dir_all(&stage);
        result
    }

    fn write_attic_tag(
        &self,
        repo_root: &Path,
        branch: &str,
        head: &str,
        salvage: Option<&Path>,
    ) -> WorktreeResult<String> {
        let tag = format!(
            "attic/{}-{}-{}",
            sanitize(branch),
            now_millis(),
            &head[..head.len().min(7)]
        );
        let message = format!(
            "reaped by agtk{}",
            salvage
                .map(|path| format!(", salvage: {}", path.display()))
                .unwrap_or_default()
        );
        git_text(repo_root, ["tag", "-a", &tag, head, "-m", &message])?;
        Ok(tag)
    }
}

pub(super) fn salvageable_paths(worktree: &Path, raw_status: &str) -> WorktreeResult<Vec<PathBuf>> {
    let mut paths = BTreeSet::new();
    for args in [
        vec![
            "ls-files",
            "--others",
            "--modified",
            "--exclude-standard",
            "-z",
        ],
        // A staged deletion has no content to salvage; status already records it.
        vec!["diff", "--name-only", "--cached", "--diff-filter=d", "-z"],
    ] {
        // Without this list the archive would miss files that removal then deletes.
        let output = run_git(worktree, os_args(&args))?;
        for path in output
            .stdout
            .split(|byte| *byte == 0)
            .filter(|path| !path.is_empty())
        {
            paths.insert(PathBuf::from(OsStr::from_bytes(path)));
        }
    }
    for line in raw_status
        .lines()
        .filter_map(|line| line.strip_prefix("! "))
    {
        let path = PathBuf::from(line);
        if let Ok(metadata) = fs::symlink_metadata(worktree.join(&path))
            && metadata.is_file()
            && metadata.len() <= 1024 * 1024
            && paths.len() < 500
        {
            paths.insert(path);
        }
    }
    Ok(paths.into_iter().collect())
}

fn stage_path(worktree: &Path, stage: &Path, relative: &Path) -> WorktreeResult<()> {
    let source = worktree.join(relative);
    let destination = stage.join(relative);
    if let Ok(metadata) = fs::symlink_metadata(&source) {
        if metadata.file_type().is_symlink() {
            if let Some(parent) = destination.parent() {
                fs::create_dir_all(parent)?;
            }
            symlink(fs::read_link(source)?, destination)?;
        } else if metadata.is_dir() {
            copy_directory(&source, &destination)?;
        } else {
            if let Some(parent) = destination.parent() {
                fs::create_dir_all(parent)?;
            }
            fs::copy(source, destination)?;
        }
        return Ok(());
    }
    let specification = format!(":{}", relative.to_string_lossy());
    let output = run_git(worktree, os_args(&["cat-file", "blob", &specification]))?;
    if let Some(parent) = destination.parent() {
        fs::create_dir_all(parent)?;
    }
    fs::write(destination, output.stdout)?;
    Ok(())
}

fn copy_directory(source: &Path, destination: &Path) -> WorktreeResult<()> {
    fs::create_dir_all(destination)?;
    for entry in fs::read_dir(source)? {
        let entry = entry?;
        stage_path(source, destination, Path::new(&entry.file_name()))?;
    }
    Ok(())
}

fn discard_salvage(archive: &Path) {
    let _ = fs::remove_file(archive);
    let _ = fs::remove_file(archive.with_extension("manifest.json"));
}

fn aborted(repo_root: &Path, reason: &str) -> ReapResult {
    ReapResult {
        ok: false,
        aborted: true,
        reason: Some(reason.to_owned()),
        repo_root: repo_root.to_owned(),
        salvage_path: None,
        attic_tag: None,
        branch_deleted: false,
    }
}
