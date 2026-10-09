use std::fs;
use std::path::Path;
use std::process::Command;

use agtk::changes::{
    ChangedFile, DiffDocumentResult, LineCounts, parse_numstat_z, read_changes_snapshot,
    read_commit_files, read_diff_document,
};

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
fn the_base_recorded_on_the_branch_is_compared_against() {
    let dir = repository();
    let root = dir.path();
    git(root, &["branch", "develop", "HEAD~1"]);
    git(root, &["switch", "-c", "feature"]);
    git(
        root,
        &["config", "branch.feature.agtk-base-branch", "develop"],
    );

    let snapshot = read_changes_snapshot(root, None, 0, 0).unwrap().unwrap();

    assert_eq!(snapshot.context.base_ref.as_deref(), Some("develop"));
    assert_eq!(snapshot.commits.len(), 1);
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
fn file_documents_embed_images_without_interpreting_them_as_text() {
    use agtk::changes::read_file_document;
    use base64::Engine;

    let dir = tempfile::tempdir().unwrap();
    let png = include_bytes!("../galaxy.png");
    let svg = br#"<svg xmlns="http://www.w3.org/2000/svg" width="10" height="20"/>"#;
    for (name, bytes, mime) in [
        ("screenshot.PNG", png.as_slice(), "image/png"),
        ("drawing.svg", svg.as_slice(), "image/svg+xml"),
    ] {
        let path = dir.path().join(name);
        fs::write(&path, bytes).unwrap();
        let document = serde_json::to_value(read_file_document(&path, name).unwrap()).unwrap();
        assert_eq!(document["kind"], "image");
        assert_eq!(document["value"]["path"], name);
        assert_eq!(document["value"]["byteSize"], bytes.len());
        assert_eq!(
            document["value"]["dataUrl"],
            format!(
                "data:{mime};base64,{}",
                base64::engine::general_purpose::STANDARD.encode(bytes)
            )
        );
        // Replacing an image with different content of the same length must offer a reload.
        fs::write(&path, vec![b'x'; bytes.len()]).unwrap();
        let changed = serde_json::to_value(read_file_document(&path, name).unwrap()).unwrap();
        assert_ne!(document["value"]["identity"], changed["value"]["identity"]);
    }
}

#[test]
fn image_files_have_a_separate_bounded_size_limit() {
    use agtk::changes::{FileDocumentResult, read_file_document};

    let dir = tempfile::tempdir().unwrap();
    let path = dir.path().join("large.png");
    let file = fs::File::create(&path).unwrap();
    file.set_len(3 * 1024 * 1024).unwrap();
    let document = serde_json::to_value(read_file_document(&path, "large.png").unwrap()).unwrap();
    assert_eq!(document["kind"], "image");

    file.set_len(20 * 1024 * 1024 + 1).unwrap();
    let FileDocumentResult::Placeholder(placeholder) =
        read_file_document(&path, "large.png").unwrap()
    else {
        panic!("expected an image size limit placeholder");
    };
    assert_eq!(
        placeholder.reason,
        "File is larger than the 20 MiB image limit"
    );
    assert_eq!(placeholder.byte_size, Some(20 * 1024 * 1024 + 1));
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

#[test]
fn changed_files_carry_their_line_counts() {
    let dir = repository();
    let root = dir.path();
    fs::write(root.join("file.txt"), "committed\nstaged\n").unwrap();
    git(root, &["add", "."]);
    fs::write(root.join("file.txt"), "working\nstaged\nmore\n").unwrap();
    fs::write(root.join("new.txt"), "one\ntwo\nno newline").unwrap();
    fs::write(root.join("blob.bin"), b"\0binary").unwrap();
    let snapshot = read_changes_snapshot(root, Some("HEAD~1"), 0, 0)
        .unwrap()
        .unwrap();
    let lines = |files: &[ChangedFile], path: &str| {
        files
            .iter()
            .find(|file| file.new_path.as_deref() == Some(path.as_bytes()))
            .unwrap()
            .lines
    };
    let counts = |added, deleted| Some(LineCounts { added, deleted });

    assert_eq!(lines(&snapshot.staged, "file.txt"), counts(1, 0));
    assert_eq!(lines(&snapshot.unstaged, "file.txt"), counts(2, 1));
    assert_eq!(lines(&snapshot.branch_changes, "file.txt"), counts(1, 1));
    assert_eq!(lines(&snapshot.all_changes, "file.txt"), counts(3, 1));
    assert_eq!(lines(&snapshot.untracked, "new.txt"), counts(3, 0));
    assert_eq!(lines(&snapshot.all_changes, "new.txt"), counts(3, 0));
    assert_eq!(lines(&snapshot.untracked, "blob.bin"), None);
    let commit = read_commit_files(&snapshot.context, &snapshot.commits[0].oid).unwrap();
    assert_eq!(lines(&commit, "file.txt"), counts(1, 1));
}

#[test]
fn numstat_records_key_counts_by_new_path_and_leave_binaries_uncounted() {
    let input = [
        b"3\t1\tsrc/a.rs\0".as_slice(),
        b"-\t-\timage.png\0",
        b"1\t0\t\0old.rs\0new.rs\0",
    ]
    .concat();
    let counts = parse_numstat_z(&input).unwrap();
    assert_eq!(counts.len(), 3);
    assert_eq!(
        counts[b"src/a.rs".as_slice()],
        Some(LineCounts {
            added: 3,
            deleted: 1
        })
    );
    assert_eq!(counts[b"image.png".as_slice()], None);
    assert_eq!(
        counts[b"new.rs".as_slice()],
        Some(LineCounts {
            added: 1,
            deleted: 0
        })
    );
    assert!(parse_numstat_z(b"3\tsrc/a.rs\0").is_err());
}
