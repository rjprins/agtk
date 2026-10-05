use std::fs;
use std::path::Path;
use std::process::Command;

use agtk::worktrees::{
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
    let manager = WorktreeManager::new(fixture.attic()).with_home(fixture.home());
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
    let manager = WorktreeManager::new(fixture.attic()).with_home(fixture.home());
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
fn status_hash_follows_contents_around_deleted_and_oddly_named_files() {
    let fixture = Repository::new();
    let manager = WorktreeManager::new(fixture.attic()).with_home(fixture.home());
    let root = fixture.root();
    // A deleted tracked file sits between the others and takes the index fallback.
    fs::remove_file(root.join("README.md")).unwrap();
    for (name, content) in [
        ("\"quoted", "q"),
        ("a.txt", "a"),
        ("b.txt", "b"),
        ("new\nline", "n"),
    ] {
        fs::write(root.join(name), content).unwrap();
    }
    let hash = || {
        manager.list(root, &[]).unwrap().worktrees[0]
            .status_hash
            .clone()
    };
    let first = hash();
    assert_eq!(hash(), first);

    // Status lines for untracked files carry no content, so only the file hashes see these.
    fs::write(root.join("b.txt"), "B").unwrap();
    let second = hash();
    assert_ne!(second, first);
    fs::write(root.join("new\nline"), "N").unwrap();
    assert_ne!(hash(), second);
}

#[test]
fn worktree_root_resolves_nested_directories_in_linked_checkouts() {
    let fixture = Repository::new();
    let manager = WorktreeManager::new(fixture.attic()).with_home(fixture.home());
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
    let manager = WorktreeManager::new(fixture.attic()).with_home(fixture.home());
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
    let manager = WorktreeManager::new(fixture.attic()).with_home(fixture.home());
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
fn create_records_purpose_and_groups_worktrees_by_repository_under_home() {
    let fixture = Repository::new();
    let manager = WorktreeManager::new(fixture.attic()).with_home(fixture.home());

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
        fixture.home().join("worktrees/demo/native-feature")
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
fn configured_templates_place_worktrees_and_reviews_alike() {
    let fixture = Repository::new();
    let manager = WorktreeManager::new(fixture.attic()).with_home(fixture.home());

    git(
        fixture.root(),
        &["config", "agtk.worktreeTemplate", "../{repo-name}-{branch}"],
    );
    let layout = manager.layout(fixture.root()).unwrap();
    let parent = fixture.root().parent().unwrap();
    assert_eq!(layout.path_for("topic").unwrap(), parent.join("demo-topic"));
    assert_eq!(layout.review_path(9).unwrap(), parent.join("demo-pr-9"));

    git(
        fixture.root(),
        &["config", "agtk.worktreeTemplate", "~/src/{branch}"],
    );
    let created = manager
        .create(fixture.root(), "tilde-work", Some("main"), "tilde template")
        .unwrap();
    assert_eq!(created.path, fixture.home().join("src/tilde-work"));
    let inventory = manager.list(fixture.root(), &[]).unwrap();
    let listed = inventory
        .worktrees
        .iter()
        .find(|worktree| worktree.path == created.path)
        .unwrap();
    assert!(!listed.off_convention);
}

#[test]
fn review_checkout_is_a_detached_pr_worktree_that_follows_the_source_tip() {
    let fixture = Repository::new();
    let manager = WorktreeManager::new(fixture.attic()).with_home(fixture.home());
    git(fixture.root(), &["checkout", "-q", "-b", "feature/topic"]);
    fs::write(fixture.root().join("topic.txt"), "one\n").unwrap();
    git(fixture.root(), &["add", "topic.txt"]);
    git(fixture.root(), &["commit", "-q", "-m", "topic one"]);
    let first = git(fixture.root(), &["rev-parse", "HEAD"])
        .trim()
        .to_owned();
    let clone = fixture.root().parent().unwrap().join("clone");
    git(
        fixture.root(),
        &[
            "clone",
            "-q",
            fixture.root().to_str().unwrap(),
            clone.to_str().unwrap(),
        ],
    );
    let clone = clone.canonicalize().unwrap();

    let path = manager
        .review_checkout(&clone, 42, "feature/topic")
        .expect("create review checkout");

    assert_eq!(path, fixture.home().join("worktrees/clone/pr-42"));
    assert_eq!(git(&path, &["rev-parse", "HEAD"]).trim(), first);
    assert!(manager.branch_for_worktree(&path).unwrap().is_none());
    let state = manager.list(&clone, &[]).unwrap();
    let review = state.worktrees.iter().find(|w| w.path == path).unwrap();
    assert_eq!(review.state, WorktreeState::Review);

    // Notes from an earlier review survive, while the checkout follows new pushes.
    fs::write(path.join("REVIEW.md"), "notes\n").unwrap();
    fs::write(fixture.root().join("topic.txt"), "two\n").unwrap();
    git(fixture.root(), &["commit", "-q", "-am", "topic two"]);
    let second = git(fixture.root(), &["rev-parse", "HEAD"])
        .trim()
        .to_owned();

    let again = manager
        .review_checkout(&clone, 42, "feature/topic")
        .unwrap();

    assert_eq!(again, path);
    assert_eq!(git(&path, &["rev-parse", "HEAD"]).trim(), second);
    assert!(path.join("REVIEW.md").exists());

    // Edits to tracked files pin the checkout where it is.
    fs::write(path.join("topic.txt"), "edited\n").unwrap();
    fs::write(fixture.root().join("topic.txt"), "three\n").unwrap();
    git(fixture.root(), &["commit", "-q", "-am", "topic three"]);

    manager
        .review_checkout(&clone, 42, "feature/topic")
        .unwrap();

    assert_eq!(git(&path, &["rev-parse", "HEAD"]).trim(), second);
}

#[test]
fn review_checkout_runs_no_hooks_from_the_pull_request() {
    let fixture = Repository::new();
    let manager = WorktreeManager::new(fixture.attic()).with_home(fixture.home());
    let marker = fixture.root().parent().unwrap().join("hook-ran");
    git(fixture.root(), &["checkout", "-q", "-b", "feature/hook"]);
    fs::create_dir(fixture.root().join(".hooks")).unwrap();
    let hook = fixture.root().join(".hooks/post-checkout");
    fs::write(&hook, format!("#!/bin/sh\ntouch '{}'\n", marker.display())).unwrap();
    fs::set_permissions(&hook, std::os::unix::fs::PermissionsExt::from_mode(0o755)).unwrap();
    git(fixture.root(), &["add", ".hooks"]);
    git(fixture.root(), &["commit", "-q", "-m", "add hook"]);
    let clone = fixture.root().parent().unwrap().join("clone");
    git(
        fixture.root(),
        &[
            "clone",
            "-q",
            "--no-checkout",
            fixture.root().to_str().unwrap(),
            clone.to_str().unwrap(),
        ],
    );
    // Like husky: hooks live in the tree, and git finds them relative to the checkout.
    git(&clone, &["config", "core.hooksPath", ".hooks"]);
    let clone = clone.canonicalize().unwrap();

    manager
        .review_checkout(&clone, 7, "feature/hook")
        .expect("create review checkout");
    assert!(!marker.exists(), "post-checkout hook ran on worktree add");

    manager
        .review_checkout(&clone, 7, "feature/hook")
        .expect("move review checkout");
    assert!(!marker.exists(), "post-checkout hook ran on checkout");
}

#[test]
fn inventory_distinguishes_live_upstream_and_gone_upstream() {
    let fixture = Repository::new();
    let manager = WorktreeManager::new(fixture.attic()).with_home(fixture.home());

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
    let manager = WorktreeManager::new(fixture.attic()).with_home(fixture.home());
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
    let manager = WorktreeManager::new(fixture.attic()).with_home(fixture.home());
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
            confirmed: false,
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
                confirmed: false,
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
                confirmed: false,
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
                confirmed: false,
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

#[test]
fn a_confirmed_reap_removes_an_active_worktree_but_keeps_its_branch_unless_forced() {
    let fixture = Repository::new();
    let manager = WorktreeManager::new(fixture.attic()).with_home(fixture.home());
    let created = manager
        .create(
            fixture.root(),
            "closing",
            Some("main"),
            "exercise confirmed reap",
        )
        .unwrap();
    fs::write(created.path.join("feature.txt"), "local commit\n").unwrap();
    git(&created.path, &["add", "feature.txt"]);
    git(&created.path, &["commit", "-m", "fresh work"]);
    fs::write(created.path.join("untracked.txt"), "recover me\n").unwrap();
    let preview = manager.list(fixture.root(), &[]).unwrap();
    let worktree = preview
        .worktrees
        .iter()
        .find(|worktree| worktree.path == created.path)
        .unwrap();
    assert_eq!(worktree.state, WorktreeState::Active);
    assert_eq!(worktree.reap_class, None);
    let request = ReapRequest {
        path: created.path.clone(),
        expected_head: worktree.head.clone().unwrap(),
        expected_status_hash: worktree.status_hash.clone(),
        delete_branch: DeleteBranch::Never,
        confirmed: false,
    };

    let refused = manager.reap(request.clone(), &[]);
    assert!(refused.unwrap_err().to_string().contains("Active"));
    assert!(created.path.exists());

    let live = manager.reap(
        ReapRequest {
            confirmed: true,
            ..request.clone()
        },
        std::slice::from_ref(&created.path),
    );
    assert!(live.unwrap_err().to_string().contains("live sessions"));

    let kept = manager
        .reap(
            ReapRequest {
                confirmed: true,
                ..request.clone()
            },
            &[],
        )
        .expect("confirmed reap");
    assert!(kept.ok, "{:?}", kept.reason);
    assert!(!created.path.exists());
    assert!(!kept.branch_deleted);
    assert!(kept.salvage_path.as_ref().unwrap().exists());
    assert!(git(fixture.root(), &["branch", "--list", "closing"]).contains("closing"));

    let created = manager
        .create(
            fixture.root(),
            "closing-forced",
            Some("main"),
            "exercise forced delete",
        )
        .unwrap();
    fs::write(created.path.join("feature.txt"), "local commit\n").unwrap();
    git(&created.path, &["add", "feature.txt"]);
    git(&created.path, &["commit", "-m", "fresh work"]);
    let preview = manager.list(fixture.root(), &[]).unwrap();
    let worktree = preview
        .worktrees
        .iter()
        .find(|worktree| worktree.path == created.path)
        .unwrap();
    let deleted = manager
        .reap(
            ReapRequest {
                path: created.path.clone(),
                expected_head: worktree.head.clone().unwrap(),
                expected_status_hash: worktree.status_hash.clone(),
                delete_branch: DeleteBranch::Force,
                confirmed: true,
            },
            &[],
        )
        .expect("forced reap");
    assert!(deleted.ok, "{:?}", deleted.reason);
    assert!(deleted.branch_deleted);
    let attic_tag = deleted.attic_tag.as_ref().unwrap();
    assert_eq!(
        git(fixture.root(), &["tag", "-l", attic_tag]).trim(),
        attic_tag
    );
    assert!(
        git(fixture.root(), &["branch", "--list", "closing-forced"])
            .trim()
            .is_empty()
    );
}

#[test]
fn staged_deletions_do_not_block_listing_or_reaping() {
    let fixture = Repository::new();
    let manager = WorktreeManager::new(fixture.attic()).with_home(fixture.home());
    let created = manager
        .create(
            fixture.root(),
            "drop-readme",
            Some("main"),
            "stage a deletion",
        )
        .unwrap();
    git(&created.path, &["rm", "-q", "README.md"]);
    git(
        fixture.root(),
        &["config", "branch.drop-readme.remote", "origin"],
    );
    git(
        fixture.root(),
        &[
            "config",
            "branch.drop-readme.merge",
            "refs/heads/drop-readme",
        ],
    );

    let preview = manager.list(fixture.root(), &[]).unwrap();
    let worktree = preview
        .worktrees
        .iter()
        .find(|worktree| worktree.path == created.path)
        .unwrap();
    assert!(worktree.dirty);

    let result = manager
        .reap(
            ReapRequest {
                path: created.path.clone(),
                expected_head: worktree.head.clone().unwrap(),
                expected_status_hash: worktree.status_hash.clone(),
                delete_branch: DeleteBranch::Force,
                confirmed: false,
            },
            &[],
        )
        .expect("reap worktree");
    assert!(result.ok, "{:?}", result.reason);
    assert!(!created.path.exists());
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
        git(&root, &["config", "user.name", "Agtk Test"]);
        git(&root, &["config", "user.email", "agtk@example.invalid"]);
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

    /// Stands in for `$HOME`, so default worktrees stay inside the fixture.
    fn home(&self) -> std::path::PathBuf {
        self.directory.path().canonicalize().unwrap().join("home")
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
