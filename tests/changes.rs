use std::fs;
use std::path::Path;
use std::process::Command;

use agtk::changes::{DiffDocumentResult, read_changes_snapshot, read_diff_document};

fn git(root: &Path, args: &[&str]) -> String {
    let output = Command::new("git")
        .current_dir(root)
        .args(args)
        .env("GIT_CONFIG_GLOBAL", "/dev/null")
        .env("GIT_CONFIG_NOSYSTEM", "1")
        .output()
        .unwrap();
    assert!(
        output.status.success(),
        "{}",
        String::from_utf8_lossy(&output.stderr)
    );
    String::from_utf8_lossy(&output.stdout).trim().to_owned()
}

fn repository() -> tempfile::TempDir {
    let dir = tempfile::tempdir().unwrap();
    git(dir.path(), &["init", "-b", "main"]);
    git(dir.path(), &["config", "user.name", "Test"]);
    git(dir.path(), &["config", "user.email", "test@example.com"]);
    for content in ["original\n", "committed\n"] {
        fs::write(dir.path().join("file.txt"), content).unwrap();
        git(dir.path(), &["add", "."]);
        git(
            dir.path(),
            &["-c", "commit.gpgsign=false", "commit", "-m", "Change"],
        );
    }
    dir
}

#[test]
fn commits_ago_compares_ancestor_to_worktree_including_uncommitted_files() {
    let dir = repository();
    let root = dir.path();
    fs::write(root.join("file.txt"), "staged\n").unwrap();
    git(root, &["add", "."]);
    fs::write(root.join("file.txt"), "working\n").unwrap();
    fs::write(root.join("new.txt"), "untracked\n").unwrap();
    let snapshot = read_changes_snapshot(root, Some("HEAD~1"), 0, 0)
        .unwrap()
        .unwrap();
    assert_eq!(
        snapshot.context.merge_base_oid.as_deref(),
        Some(git(root, &["rev-parse", "HEAD~1"]).as_str())
    );
    assert_eq!(snapshot.commits.len(), 1);
    assert_eq!(snapshot.staged.len(), 1);
    assert_eq!(snapshot.unstaged.len(), 1);
    assert_eq!(snapshot.untracked.len(), 1);
    assert_eq!(snapshot.all_changes.len(), 2);
    let file = snapshot
        .all_changes
        .iter()
        .find(|file| file.new_path.as_deref() == Some(b"file.txt"))
        .unwrap();
    let DiffDocumentResult::Text(document) = read_diff_document(&snapshot.context, file).unwrap()
    else {
        panic!("expected a text comparison");
    };
    assert_eq!(document.original, "original\n");
    assert_eq!(document.modified, "working\n");

    git(root, &["add", "."]);
    git(
        root,
        &["-c", "commit.gpgsign=false", "commit", "-m", "Next change"],
    );
    let snapshot = read_changes_snapshot(root, Some("HEAD~2"), 1, 0)
        .unwrap()
        .unwrap();
    assert_eq!(snapshot.commits.len(), 2);
    assert_eq!(
        snapshot.context.merge_base_oid.as_deref(),
        Some(git(root, &["rev-parse", "HEAD~2"]).as_str())
    );
    assert_eq!(snapshot.branch_changes.len(), 2);
}

#[test]
fn unavailable_ancestor_keeps_selection_instead_of_falling_back_to_main() {
    let dir = repository();
    let snapshot = read_changes_snapshot(dir.path(), Some("HEAD~10"), 0, 0)
        .unwrap()
        .unwrap();
    assert_eq!(snapshot.context.base_ref.as_deref(), Some("HEAD~10"));
    assert!(snapshot.context.base_oid.is_none());
    assert!(snapshot.context.merge_base_oid.is_none());
    assert!(
        snapshot
            .warnings
            .iter()
            .any(|warning| warning.contains("10 commits ago"))
    );
    assert!(snapshot.commits.is_empty());
}

#[test]
fn worktree_files_list_tracked_and_untracked_but_not_ignored_paths() {
    let dir = repository();
    let root = dir.path();
    fs::create_dir_all(root.join("src/nested")).unwrap();
    fs::write(root.join("src/nested/new.rs"), "fn main() {}\n").unwrap();
    fs::write(root.join(".gitignore"), "target/\n").unwrap();
    fs::create_dir_all(root.join("target")).unwrap();
    fs::write(root.join("target/ignored.txt"), "ignored\n").unwrap();
    let mut files = agtk::changes::list_worktree_files(root).unwrap();
    files.sort();
    assert_eq!(
        files,
        [
            b".gitignore".to_vec(),
            b"file.txt".to_vec(),
            b"src/nested/new.rs".to_vec(),
        ]
    );
}

#[test]
fn file_documents_report_text_binary_and_missing_files() {
    use agtk::changes::{FileDocumentResult, read_file_document};

    let dir = tempfile::tempdir().unwrap();
    let root = dir.path();
    fs::write(root.join("main.rs"), "fn main() {}\nfn other() {}\n").unwrap();
    fs::write(root.join("image.bin"), b"\x89PNG\0\0\0").unwrap();

    let FileDocumentResult::Text(document) =
        read_file_document(&root.join("main.rs"), "main.rs").unwrap()
    else {
        panic!("expected text");
    };
    assert_eq!(document.path, "main.rs");
    assert_eq!(document.language.as_deref(), Some("rust"));
    assert_eq!(document.line_count, 2);
    assert!(document.identity.starts_with("disk:100644:"));

    let FileDocumentResult::Placeholder(binary) =
        read_file_document(&root.join("image.bin"), "image.bin").unwrap()
    else {
        panic!("expected a placeholder");
    };
    assert_eq!(binary.reason, "Binary content is not shown as text");
    assert_eq!(binary.byte_size, Some(7));

    let FileDocumentResult::Placeholder(missing) =
        read_file_document(&root.join("gone.rs"), "gone.rs").unwrap()
    else {
        panic!("expected a placeholder");
    };
    assert_eq!(missing.reason, "This file does not exist");

    let FileDocumentResult::Placeholder(directory) = read_file_document(root, ".").unwrap() else {
        panic!("expected a placeholder");
    };
    assert_eq!(directory.reason, "This path is a directory");
}

#[test]
fn file_documents_flag_links_out_of_the_worktree_and_skip_fifos() {
    use agtk::changes::{FileDocumentResult, link_target_outside, read_file_document};

    let dir = tempfile::tempdir().unwrap();
    let worktree = dir.path().join("worktree");
    fs::create_dir(&worktree).unwrap();
    let secret = dir.path().join("credentials");
    fs::write(&secret, "secret\n").unwrap();
    fs::write(worktree.join("inside.txt"), "inside\n").unwrap();
    std::os::unix::fs::symlink(&secret, worktree.join("config.yml")).unwrap();
    std::os::unix::fs::symlink("inside.txt", worktree.join("alias.txt")).unwrap();

    assert_eq!(
        link_target_outside(&worktree.join("config.yml"), &worktree),
        Some(secret.canonicalize().unwrap())
    );
    assert_eq!(
        link_target_outside(&worktree.join("alias.txt"), &worktree),
        None
    );
    assert_eq!(
        link_target_outside(&worktree.join("inside.txt"), &worktree),
        None
    );
    // A path outside the worktree was opened as such, not through a link.
    assert_eq!(link_target_outside(&secret, &worktree), None);

    let fifo = worktree.join("pipe");
    assert!(
        Command::new("mkfifo")
            .arg(&fifo)
            .status()
            .unwrap()
            .success()
    );
    let FileDocumentResult::Placeholder(placeholder) = read_file_document(&fifo, "pipe").unwrap()
    else {
        panic!("expected a placeholder");
    };
    assert_eq!(placeholder.reason, "This path is not a regular file");
}

#[test]
fn a_commit_without_a_message_still_lists_every_commit() {
    let dir = repository();
    let root = dir.path();
    git(
        root,
        &[
            "-c",
            "commit.gpgsign=false",
            "commit",
            "-q",
            "--allow-empty",
            "--allow-empty-message",
            "-m",
            "",
        ],
    );
    let snapshot = read_changes_snapshot(root, Some("HEAD~2"), 1, 0)
        .unwrap()
        .unwrap();
    assert_eq!(snapshot.commits.len(), 2);
    assert_eq!(snapshot.commits[0].subject, "");
    assert_eq!(snapshot.commits[1].subject, "Change");
}
