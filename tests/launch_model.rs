use std::path::PathBuf;

use agmux_native::control::SessionKind;
use agmux_native::launch_model::{
    NEW_WORKTREE, directory_choices, path_completions, provider_args, worktree_choices,
};
use serde_json::json;

#[test]
fn worktree_choices_keep_new_and_current_pinned_then_sort_real_worktrees() {
    let choices = worktree_choices(
        "/work/agmux",
        [
            ("/work/agmux-zeta".to_owned(), "zeta".to_owned()),
            ("/work/agmux-alpha".to_owned(), "alpha".to_owned()),
            ("/work/agmux".to_owned(), "agmux".to_owned()),
        ],
        true,
    );

    assert_eq!(choices[0].value, NEW_WORKTREE);
    assert_eq!(choices[0].label, "+ New worktree");
    assert_eq!(choices[1].value, "/work/agmux");
    assert_eq!(choices[1].label, "Current (agmux)");
    assert_eq!(choices[2].label, "alpha");
    assert_eq!(choices[3].label, "zeta");
}

#[test]
fn directory_choices_are_sorted_by_project_name() {
    let choices = directory_choices([PathBuf::from("/work/zeta"), PathBuf::from("/work/alpha")]);
    assert_eq!(choices[0].label, "alpha");
    assert_eq!(choices[1].value, "/work/zeta");
}

#[test]
fn path_completions_only_return_matching_directories() {
    let temp = tempfile::tempdir().expect("temp directory");
    std::fs::create_dir(temp.path().join("alpha")).expect("alpha directory");
    std::fs::create_dir(temp.path().join("beta")).expect("beta directory");
    std::fs::write(temp.path().join("alpha.txt"), "not a directory").expect("file");

    let prefix = format!("{}/a", temp.path().display());
    assert_eq!(
        path_completions(&prefix),
        vec![temp.path().join("alpha").to_string_lossy()]
    );
}

#[test]
fn provider_args_translate_saved_options_without_affecting_shell() {
    let flags = [
        ("--ask-for-approval".to_owned(), json!("never")),
        ("--sandbox".to_owned(), json!("workspace-write")),
        ("--full-auto".to_owned(), json!(true)),
    ]
    .into_iter()
    .collect();
    assert_eq!(
        provider_args(SessionKind::Codex, &flags),
        [
            "--ask-for-approval",
            "never",
            "--sandbox",
            "workspace-write",
            "--full-auto"
        ]
    );
    assert!(provider_args(SessionKind::Shell, &flags).is_empty());
}
