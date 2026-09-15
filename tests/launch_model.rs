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
    assert!(path_completions("alpha").is_empty());
    let temp = tempfile::tempdir().expect("temp directory");
    std::fs::create_dir(temp.path().join("alpha")).expect("alpha directory");
    std::fs::create_dir(temp.path().join("beta")).expect("beta directory");
    std::fs::write(temp.path().join("alpha.txt"), "not a directory").expect("file");

    let prefix = format!("{}/a", temp.path().display());
    assert_eq!(
        path_completions(&prefix),
        vec![temp.path().join("alpha").to_string_lossy()]
    );

    let home = PathBuf::from(std::env::var_os("HOME").expect("HOME is set for tilde completion"));
    let home_temp = tempfile::tempdir_in(&home).expect("create home-relative completion root");
    std::fs::create_dir(home_temp.path().join("alpha")).expect("alpha directory");
    let relative = home_temp
        .path()
        .strip_prefix(&home)
        .expect("temporary path is below HOME")
        .to_string_lossy();
    let tilde_prefix = format!("~/{relative}/a");
    assert_eq!(
        path_completions(&tilde_prefix),
        vec![format!("~/{relative}/alpha")]
    );
}

#[test]
fn empty_worktree_selection_uses_the_project_directory() {
    let project = PathBuf::from("/tmp/project");
    assert_eq!(
        agmux_native::launch_model::effective_worktree_path(Some(&project), None),
        Some(project.clone())
    );
    let selected = PathBuf::from("/tmp/project-worktree");
    assert_eq!(
        agmux_native::launch_model::effective_worktree_path(Some(&project), Some(&selected)),
        Some(selected)
    );
    assert_eq!(
        agmux_native::launch_model::effective_worktree_path(None, None),
        None
    );
}

#[test]
fn launch_paths_expand_home_directory_shorthand() {
    let home = std::env::var_os("HOME").expect("HOME is set for launch path expansion");
    assert_eq!(
        agmux_native::launch_model::expand_user_path("~/fastapi-restly"),
        PathBuf::from(home).join("fastapi-restly")
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
            "--approve-for-me"
        ]
    );
    assert!(provider_args(SessionKind::Shell, &flags).is_empty());
}

#[test]
fn provider_args_migrate_legacy_codex_approval_values() {
    let flags = [
        ("--ask-for-approval".to_owned(), json!("untrusted")),
        ("--sandbox".to_owned(), json!("read-only")),
        ("--full-auto".to_owned(), json!(false)),
    ]
    .into_iter()
    .collect();

    assert_eq!(
        provider_args(SessionKind::Codex, &flags),
        ["--ask-for-approval", "on-request", "--sandbox", "read-only"]
    );
}
