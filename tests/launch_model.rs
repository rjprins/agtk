use std::path::PathBuf;

use agmux_native::control::SessionKind;
use agmux_native::launch_model::{
    DEFAULT_BASE_BRANCH, carried_agent_args, choice_matches, directory_choices, path_completions,
    provider_args, worktree_choices,
};
use serde_json::json;

#[test]
fn worktree_choices_keep_current_pinned_then_sort_real_worktrees() {
    let choices = worktree_choices(
        "/work/agmux",
        [
            ("/work/agmux-zeta".to_owned(), "zeta".to_owned()),
            ("/work/agmux-alpha".to_owned(), "alpha".to_owned()),
            ("/work/agmux".to_owned(), "agmux".to_owned()),
        ],
    );

    assert_eq!(choices[0].value, "/work/agmux");
    assert_eq!(choices[0].label, "Current (agmux)");
    assert_eq!(choices[1].label, "alpha");
    assert_eq!(choices[2].label, "zeta");
}

#[test]
fn worktree_choices_treat_trailing_project_separators_as_the_current_tree() {
    let choices = worktree_choices(
        "/work/agmux/",
        [("/work/agmux".to_owned(), "agmux".to_owned())],
    );

    assert_eq!(
        choices
            .iter()
            .map(|choice| choice.value.as_str())
            .collect::<Vec<_>>(),
        vec!["/work/agmux/"]
    );
}

#[test]
fn new_worktree_uses_origin_main_as_the_default_base_branch() {
    assert_eq!(DEFAULT_BASE_BRANCH, "origin/main");
}

#[test]
fn choice_matching_searches_names_and_absolute_paths() {
    assert!(choice_matches(
        "orders",
        "main-orders-overview",
        "/home/rutger/shop/main-orders-overview"
    ));
    assert!(choice_matches(
        "~/shop/main-orders",
        "main-orders-overview",
        "/home/rutger/shop/main-orders-overview"
    ));
    assert!(!choice_matches(
        "unrelated",
        "main-orders-overview",
        "/home/rutger/shop/main-orders-overview"
    ));
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
        ["--approve-for-me"]
    );
    assert!(provider_args(SessionKind::Shell, &flags).is_empty());
}

#[test]
fn provider_args_codex_bypass_suppresses_manual_policies() {
    let flags = [
        ("--ask-for-approval".to_owned(), json!("never")),
        ("--sandbox".to_owned(), json!("danger-full-access")),
        (
            "--dangerously-bypass-approvals-and-sandbox".to_owned(),
            json!(true),
        ),
    ]
    .into_iter()
    .collect();

    assert_eq!(
        provider_args(SessionKind::Codex, &flags),
        ["--dangerously-bypass-approvals-and-sandbox"]
    );
}

#[test]
fn provider_args_codex_prefers_bypass_when_both_automatic_modes_are_saved() {
    let flags = [
        ("--full-auto".to_owned(), json!(true)),
        (
            "--dangerously-bypass-approvals-and-sandbox".to_owned(),
            json!(true),
        ),
    ]
    .into_iter()
    .collect();

    assert_eq!(
        provider_args(SessionKind::Codex, &flags),
        ["--dangerously-bypass-approvals-and-sandbox"]
    );
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

#[test]
fn a_restarted_agent_keeps_its_launch_flags_but_not_its_resume_or_prompt() {
    let args = |args: &[&str]| args.iter().map(|arg| (*arg).to_owned()).collect::<Vec<_>>();
    assert_eq!(
        carried_agent_args(
            SessionKind::Claude,
            &args(&[
                "--resume",
                "old-id",
                "--permission-mode",
                "plan",
                "--dangerously-skip-permissions",
                "--model=opus",
                "fix the bug",
            ])
        ),
        args(&[
            "--permission-mode",
            "plan",
            "--dangerously-skip-permissions",
            "--model=opus"
        ])
    );
    assert_eq!(
        carried_agent_args(
            SessionKind::Codex,
            &args(&[
                "resume",
                "old-id",
                "-s",
                "workspace-write",
                "--approve-for-me"
            ])
        ),
        args(&["-s", "workspace-write", "--approve-for-me"])
    );
    assert!(carried_agent_args(SessionKind::Shell, &args(&["--model", "x"])).is_empty());
}
