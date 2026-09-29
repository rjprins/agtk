use std::fs;
use std::path::Path;
use std::process::Command;

use agmux_native::worktrees::{
    DeleteBranch, ReapClass, ReapRequest, WorktreeManager, WorktreeState, parse_worktree_porcelain,
};

#[test]
fn porcelain_parser_preserves_detached_locked_and_prunable_flags() {
    let parsed = parse_worktree_porcelain(
        "worktree /repo\nHEAD abc\nbranch refs/heads/main\n\nworktree /repo-pr-1\nHEAD def\ndetached\nlocked reason\nprunable stale\n\n",
    );

    assert_eq!(parsed.len(), 2);
    assert_eq!(parsed[0].branch.as_deref(), Some("main"));
    assert!(parsed[1].detached);
    assert!(parsed[1].locked);
    assert!(parsed[1].prunable);
}

#[test]
fn linked_worktree_paths_include_every_checkout_without_status_scanning() {
    let fixture = Repository::new();
    let manager = WorktreeManager::new(fixture.attic());
    let extra = fixture.root().parent().unwrap().join("demo-extra");
    git(
        fixture.root(),
        &[
            "worktree",
            "add",
            "-b",
            "extra-work",
            extra.to_str().unwrap(),
            "main",
        ],
    );

    let paths = manager.linked_paths(fixture.root()).unwrap();

    assert_eq!(paths, vec![fixture.root().to_path_buf(), extra]);
}

#[test]
fn linked_worktrees_report_each_checkout_branch() {
    let fixture = Repository::new();
    let manager = WorktreeManager::new(fixture.attic());
    let extra = fixture.root().parent().unwrap().join("demo-linked-branch");
    git(
        fixture.root(),
        &[
            "worktree",
            "add",
            "-b",
            "linked-work",
            extra.to_str().unwrap(),
            "main",
        ],
    );

    let worktrees = manager.linked_worktrees(fixture.root()).unwrap();

    let branches = worktrees
        .iter()
        .map(|worktree| (worktree.path.clone(), worktree.branch.clone()))
        .collect::<Vec<_>>();
    assert_eq!(
        branches,
        vec![
            (fixture.root().to_path_buf(), Some("main".to_owned())),
            (extra, Some("linked-work".to_owned())),
        ]
    );
}

#[test]
fn worktree_root_resolves_nested_directories_in_linked_checkouts() {
    let fixture = Repository::new();
    let manager = WorktreeManager::new(fixture.attic());
    let extra = fixture.root().parent().unwrap().join("demo-nested-extra");
    git(
        fixture.root(),
        &[
            "worktree",
            "add",
            "-b",
            "nested-work",
            extra.to_str().unwrap(),
            "main",
        ],
    );
    let nested = extra.join("src/deep");
    fs::create_dir_all(&nested).unwrap();

    assert_eq!(manager.worktree_root(&nested).unwrap(), extra);
    assert_eq!(manager.repository_root(&nested).unwrap(), fixture.root());

    let location = manager.resolve_session_location(&nested, &[]);
    assert_eq!(location.cwd, nested);
    assert_eq!(location.project_root, fixture.root());
    assert_eq!(location.worktree_path, Some(extra));
}

#[test]
fn branch_for_worktree_resolves_the_checked_out_branch() {
    let fixture = Repository::new();
    let manager = WorktreeManager::new(fixture.attic());
    let extra = fixture.root().parent().unwrap().join("demo-branch-extra");
    git(
        fixture.root(),
        &[
            "worktree",
            "add",
            "-b",
            "pr-feature",
            extra.to_str().unwrap(),
            "main",
        ],
    );

    assert_eq!(
        manager.branch_for_worktree(&extra).unwrap().as_deref(),
        Some("pr-feature")
    );
}

#[test]
fn branch_for_path_in_repo_resolves_a_file_in_another_worktree() {
    let fixture = Repository::new();
    let manager = WorktreeManager::new(fixture.attic());
    let extra = fixture.root().parent().unwrap().join("demo-edited-extra");
    git(
        fixture.root(),
        &[
            "worktree",
            "add",
            "-b",
            "edited-feature",
            extra.to_str().unwrap(),
            "main",
        ],
    );
    let edited = extra.join("src/edited.rs");
    fs::create_dir_all(edited.parent().unwrap()).unwrap();
    fs::write(&edited, "changed").unwrap();

    assert_eq!(
        manager
            .branch_for_path_in_repo(fixture.root(), &edited)
            .unwrap()
            .as_deref(),
        Some("edited-feature")
    );
}

#[test]
fn create_records_purpose_and_uses_the_sibling_template() {
    let fixture = Repository::new();
    let manager = WorktreeManager::new(fixture.attic());

    let created = manager
        .create(
            fixture.root(),
            "native-feature",
            Some("main"),
            "test native worktree creation",
        )
        .expect("create worktree");

    assert_eq!(
        created.path,
        fixture.root().parent().unwrap().join(format!(
            "{}-native-feature",
            fixture.root().file_name().unwrap().to_string_lossy()
        ))
    );
    assert_eq!(
        git(
            fixture.root(),
            &["config", "--get", "branch.native-feature.description"]
        )
        .trim(),
        "test native worktree creation"
    );
    assert_eq!(created.branch.as_deref(), Some("native-feature"));
}

#[test]
fn inventory_distinguishes_live_upstream_and_gone_upstream() {
    let fixture = Repository::new();
    let manager = WorktreeManager::new(fixture.attic());

    let open = manager
        .create(fixture.root(), "open-work", Some("main"), "open upstream")
        .unwrap();
    fs::write(open.path.join("open.txt"), "open\n").unwrap();
    git(&open.path, &["add", "open.txt"]);
    git_at(&open.path, &["commit", "-m", "open work"]);
    git(fixture.root(), &["remote", "add", "origin", "."]);
    let open_head = git(&open.path, &["rev-parse", "HEAD"]);
    git(
        fixture.root(),
        &[
            "update-ref",
            "refs/remotes/origin/open-work",
            open_head.trim(),
        ],
    );
    git(
        fixture.root(),
        &["config", "branch.open-work.remote", "origin"],
    );
    git(
        fixture.root(),
        &["config", "branch.open-work.merge", "refs/heads/open-work"],
    );

    let gone = manager
        .create(fixture.root(), "gone-work", Some("main"), "gone upstream")
        .unwrap();
    fs::write(gone.path.join("gone.txt"), "gone\n").unwrap();
    git(&gone.path, &["add", "gone.txt"]);
    git_at(&gone.path, &["commit", "-m", "gone work"]);
    git(
        fixture.root(),
        &["config", "branch.gone-work.remote", "origin"],
    );
    git(
        fixture.root(),
        &["config", "branch.gone-work.merge", "refs/heads/gone-work"],
    );

    let stale = manager
        .create(
            fixture.root(),
            "stale-work",
            Some("main"),
            "old remote work",
        )
        .unwrap();
    fs::write(stale.path.join("stale.txt"), "stale\n").unwrap();
    git(&stale.path, &["add", "stale.txt"]);
    git_at(&stale.path, &["commit", "-m", "stale work"]);
    let stale_head = git(&stale.path, &["rev-parse", "HEAD"]);
    git(
        fixture.root(),
        &[
            "update-ref",
            "refs/remotes/origin/stale-work",
            stale_head.trim(),
        ],
    );

    let inventory = manager.list(fixture.root(), &[]).unwrap();
    let find = |path: &Path| {
        inventory
            .worktrees
            .iter()
            .find(|worktree| worktree.path == path)
            .unwrap()
    };
    assert_eq!(find(&open.path).state, WorktreeState::Open);
    assert_eq!(find(&open.path).evidence, "upstream configured");
    assert_eq!(find(&gone.path).state, WorktreeState::Merged);
    assert_eq!(find(&gone.path).reap_class, Some(ReapClass::Check));
    assert_eq!(find(&gone.path).evidence, "upstream gone, merge unproven");
    assert_eq!(find(&stale.path).state, WorktreeState::Stale);
    assert_eq!(find(&stale.path).evidence, "idle for at least 14 days");
}

#[test]
fn inventory_covers_active_review_ephemeral_unknown_and_path_overlays() {
    let fixture = Repository::new();
    let manager = WorktreeManager::new(fixture.attic());
    let parent = fixture.root().parent().unwrap();
    let active = parent.join("odd-active-location");
    let review = parent.join("pr-42-checkout");
    let unknown = parent.join("bisect-checkout");
    let ephemeral = parent.join(".claude/worktrees/quick-fix");
    git(
        fixture.root(),
        &[
            "worktree",
            "add",
            "-b",
            "active-work",
            active.to_str().unwrap(),
            "main",
        ],
    );
    git(
        fixture.root(),
        &[
            "worktree",
            "add",
            "--detach",
            review.to_str().unwrap(),
            "main",
        ],
    );
    git(
        fixture.root(),
        &[
            "worktree",
            "add",
            "--detach",
            unknown.to_str().unwrap(),
            "main",
        ],
    );
    git(
        fixture.root(),
        &[
            "worktree",
            "add",
            "-b",
            "quick-fix",
            ephemeral.to_str().unwrap(),
            "main",
        ],
    );

    let inventory = manager
        .list(fixture.root(), std::slice::from_ref(&active))
        .unwrap();
    let find = |path: &Path| {
        inventory
            .worktrees
            .iter()
            .find(|worktree| worktree.path == path)
            .unwrap()
    };
    assert_eq!(find(&active).state, WorktreeState::Active);
    assert_eq!(find(&active).evidence, "live session");
    assert!(find(&active).drifted);
    assert!(find(&active).off_convention);
    assert_eq!(find(&review).state, WorktreeState::Review);
    assert_eq!(find(&unknown).state, WorktreeState::Unknown);
    assert_eq!(find(&ephemeral).state, WorktreeState::Ephemeral);
}

#[test]
fn reap_aborts_on_drift_then_salvages_and_attic_tags_dirty_work() {
    let fixture = Repository::new();
    let manager = WorktreeManager::new(fixture.attic());
    let created = manager
        .create(
            fixture.root(),
            "discard-me",
            Some("main"),
            "exercise guarded reap",
        )
        .unwrap();
    fs::write(created.path.join("feature.txt"), "local commit\n").unwrap();
    git(&created.path, &["add", "feature.txt"]);
    git_at(&created.path, &["commit", "-m", "local work"]);
    fs::write(created.path.join("untracked.txt"), "recover me\n").unwrap();
    let preview = manager.list(fixture.root(), &[]).unwrap();
    let worktree = preview
        .worktrees
        .iter()
        .find(|worktree| worktree.path == created.path)
        .unwrap();
    assert_eq!(worktree.state, WorktreeState::LocalOnly);
    assert!(worktree.dirty);

    let refused = manager.reap(
        ReapRequest {
            path: created.path.clone(),
            expected_head: worktree.head.clone().unwrap(),
            expected_status_hash: worktree.status_hash.clone(),
            delete_branch: DeleteBranch::Force,
        },
        &[],
    );
    assert!(refused.unwrap_err().to_string().contains("LocalOnly"));
    assert!(created.path.exists());

    git(
        fixture.root(),
        &["config", "branch.discard-me.remote", "origin"],
    );
    git(
        fixture.root(),
        &["config", "branch.discard-me.merge", "refs/heads/discard-me"],
    );
    let preview = manager.list(fixture.root(), &[]).unwrap();
    let worktree = preview
        .worktrees
        .iter()
        .find(|worktree| worktree.path == created.path)
        .unwrap();
    assert_eq!(worktree.state, WorktreeState::Merged);
    assert_eq!(worktree.reap_class, Some(ReapClass::Check));

    let moved = manager
        .reap(
            ReapRequest {
                path: created.path.clone(),
                expected_head: "wrong".to_owned(),
                expected_status_hash: worktree.status_hash.clone(),
                delete_branch: DeleteBranch::Force,
            },
            &[],
        )
        .expect("guarded result");
    assert!(moved.aborted);
    assert!(created.path.exists());

    fs::write(
        created.path.join("untracked.txt"),
        "newer recoverable content\n",
    )
    .unwrap();
    let drifted = manager
        .reap(
            ReapRequest {
                path: created.path.clone(),
                expected_head: worktree.head.clone().unwrap(),
                expected_status_hash: worktree.status_hash.clone(),
                delete_branch: DeleteBranch::Force,
            },
            &[],
        )
        .expect("content drift result");
    assert!(drifted.aborted);
    assert!(created.path.exists());

    let preview = manager.list(fixture.root(), &[]).unwrap();
    let worktree = preview
        .worktrees
        .iter()
        .find(|worktree| worktree.path == created.path)
        .unwrap();

    let result = manager
        .reap(
            ReapRequest {
                path: created.path.clone(),
                expected_head: worktree.head.clone().unwrap(),
                expected_status_hash: worktree.status_hash.clone(),
                delete_branch: DeleteBranch::Force,
            },
            &[],
        )
        .expect("reap worktree");
    assert!(result.ok);
    assert!(!created.path.exists());
    assert!(result.salvage_path.as_ref().unwrap().exists());
    let attic_tag = result.attic_tag.as_ref().unwrap();
    assert_eq!(
        git(fixture.root(), &["tag", "-l", attic_tag]).trim(),
        attic_tag
    );
    assert!(result.branch_deleted);
}

struct Repository {
    directory: tempfile::TempDir,
    root: std::path::PathBuf,
}

impl Repository {
    fn new() -> Self {
        let directory = tempfile::tempdir().unwrap();
        let root = directory.path().join("demo");
        fs::create_dir(&root).unwrap();
        git(&root, &["init", "-b", "main"]);
        git(&root, &["config", "user.name", "Agmux Test"]);
        git(&root, &["config", "user.email", "agmux@example.invalid"]);
        fs::write(root.join("README.md"), "fixture\n").unwrap();
        git(&root, &["add", "README.md"]);
        git(&root, &["commit", "-m", "fixture"]);
        Self { directory, root }
    }

    fn root(&self) -> &Path {
        &self.root
    }

    fn attic(&self) -> std::path::PathBuf {
        self.directory.path().join("attic")
    }
}

fn git(cwd: &Path, args: &[&str]) -> String {
    let output = Command::new("git")
        .args(args)
        .current_dir(cwd)
        .output()
        .unwrap();
    assert!(
        output.status.success(),
        "git {args:?}: {}",
        String::from_utf8_lossy(&output.stderr)
    );
    String::from_utf8(output.stdout).unwrap()
}

fn git_at(cwd: &Path, args: &[&str]) -> String {
    let output = Command::new("git")
        .args(args)
        .env("GIT_AUTHOR_DATE", "2020-01-01T00:00:00Z")
        .env("GIT_COMMITTER_DATE", "2020-01-01T00:00:00Z")
        .current_dir(cwd)
        .output()
        .unwrap();
    assert!(
        output.status.success(),
        "git {args:?}: {}",
        String::from_utf8_lossy(&output.stderr)
    );
    String::from_utf8(output.stdout).unwrap()
}
