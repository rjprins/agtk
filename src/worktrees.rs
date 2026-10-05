use std::collections::BTreeSet;
use std::error::Error;
use std::ffi::{OsStr, OsString};
use std::fs;
use std::io::Write;
use std::os::unix::fs::{DirBuilderExt, symlink};
use std::path::{Component, Path, PathBuf};
use std::process::{Command, Output, Stdio};
use std::thread;
use std::time::{SystemTime, UNIX_EPOCH};

use serde::{Deserialize, Serialize};

use crate::command_runner::BoundedByteOutput;
use crate::git;

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

/// Where one repository's worktrees go, from `agtk.worktreeTemplate` or the
/// default `~/worktrees/{repo-name}/{branch}`.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct WorktreeLayout {
    repo_root: PathBuf,
    template: String,
    home: Option<PathBuf>,
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

#[derive(Debug)]
struct Status {
    raw: String,
    dirty: bool,
    ignored_only: bool,
    hash: String,
}

fn status(worktree: &Path) -> WorktreeResult<Status> {
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

struct ClassificationContext<'a> {
    repo_root: &'a Path,
    default_branch: &'a str,
    layout: &'a WorktreeLayout,
    now: u64,
}

fn classify(
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

fn merged_into_default(repo_root: &Path, head: &str, default_branch: &str) -> bool {
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

fn worktree_template(repo_root: &Path) -> String {
    // Repos configured for the old agmux keep working.
    ["agtk.worktreeTemplate", "agmux.worktreeTemplate"]
        .into_iter()
        .filter_map(|key| git_text(repo_root, ["config", "--get", key]).ok())
        .map(|value| value.trim().to_owned())
        .find(|value| !value.is_empty())
        .unwrap_or_else(|| "~/worktrees/{repo-name}/{branch}".to_owned())
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

fn has_populated_submodules(worktree: &Path) -> bool {
    git_text(worktree, ["submodule", "status"]).is_ok_and(|output| {
        output
            .lines()
            .any(|line| !line.is_empty() && !line.starts_with('-'))
    })
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

fn normalize_path(path: &Path) -> PathBuf {
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

fn sanitize(value: &str) -> String {
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

fn salvageable_paths(worktree: &Path, raw_status: &str) -> WorktreeResult<Vec<PathBuf>> {
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

use std::os::unix::ffi::OsStrExt;
