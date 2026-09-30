use std::fs;
use std::process::Command;

use agtk::emacs::{
    EmacsAction, EmacsIntegration, build_branch_review_eval, build_magit_eval,
    resolve_branch_review_base, resolve_worktree_root,
};

#[test]
fn elisp_forms_escape_paths_and_raise_an_existing_graphical_frame() {
    let path = std::path::Path::new("/tmp/project \"quoted\"");
    let magit = build_magit_eval(path);
    assert!(magit.contains("/tmp/project \\\"quoted\\\""));
    assert!(magit.contains("(require 'magit nil t)"));
    assert!(magit.contains("(call-interactively 'magit-status)"));
    assert!(magit.contains("(raise-frame frame)"));

    let review = build_branch_review_eval(path, Some("origin/develop"));
    assert!(review.contains("(require 'branch-review nil t)"));
    assert!(review.contains("branch-review-with-base"));
    assert!(review.contains("origin/develop"));
}

#[test]
fn git_context_resolves_worktree_root_and_explicit_review_base() {
    let directory = tempfile::tempdir().unwrap();
    run_git(directory.path(), &["init", "-q"]);
    run_git(directory.path(), &["config", "user.name", "Test"]);
    run_git(
        directory.path(),
        &["config", "user.email", "test@example.com"],
    );
    fs::write(directory.path().join("tracked"), "base").unwrap();
    run_git(directory.path(), &["add", "tracked"]);
    run_git(directory.path(), &["commit", "-qm", "base"]);
    run_git(directory.path(), &["branch", "develop"]);
    run_git(directory.path(), &["switch", "-qc", "feature"]);
    run_git(
        directory.path(),
        &["config", "branch.feature.agmux-base-branch", "develop"],
    );
    let nested = directory.path().join("src/nested");
    fs::create_dir_all(&nested).unwrap();

    assert_eq!(resolve_worktree_root(&nested).unwrap(), directory.path());
    assert_eq!(
        resolve_branch_review_base(directory.path())
            .unwrap()
            .as_deref(),
        Some("develop")
    );
}

#[test]
fn emacs_integration_runs_off_the_resolved_repository_root() {
    let directory = tempfile::tempdir().unwrap();
    run_git(directory.path(), &["init", "-q"]);
    let integration = EmacsIntegration::new("/bin/true");
    let opened = integration
        .open(directory.path(), EmacsAction::Magit)
        .unwrap();
    assert_eq!(opened.path, directory.path());
    assert_eq!(opened.action, EmacsAction::Magit);
}

fn run_git(cwd: &std::path::Path, args: &[&str]) {
    let status = Command::new("git")
        .args(args)
        .current_dir(cwd)
        .status()
        .unwrap();
    assert!(status.success(), "git {args:?} failed");
}

#[test]
fn open_file_form_visits_the_file_at_its_line_and_column() {
    let form = agtk::emacs::build_open_file_eval(
        std::path::Path::new("/work/src/main.rs"),
        Some(42),
        Some(7),
    );
    assert!(form.contains("(find-file \"/work/src/main.rs\")"));
    assert!(form.contains("(forward-line 41)"));
    assert!(form.contains("(move-to-column 6)"));
    assert!(form.contains("(raise-frame frame)"));
    assert!(!form.contains("(require '"));

    let plain = agtk::emacs::build_open_file_eval(
        std::path::Path::new("/work/a.rs"),
        None,
        Some(3),
    );
    assert!(!plain.contains("forward-line"));
    assert!(!plain.contains("move-to-column"));
}
