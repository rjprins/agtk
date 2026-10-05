use std::path::PathBuf;

use agtk::control::SessionKind;
use agtk::launch_model::{
    DEFAULT_BASE_BRANCH, carried_agent_args, choice_matches, codex_args_without_agtk_mcp,
    directory_choices, path_completions, provider_args, worktree_choices,
};
use serde_json::json;

#[test]
fn worktree_choices_keep_current_pinned_then_sort_real_worktrees() {
    let choices = worktree_choices(
        "/work/agtk",
        [
            ("/work/agtk-zeta".to_owned(), "zeta".to_owned()),
            ("/work/agtk-alpha".to_owned(), "alpha".to_owned()),
            ("/work/agtk".to_owned(), "agtk".to_owned()),
        ],
    );

    assert_eq!(choices[0].value, "/work/agtk");
    assert_eq!(choices[0].label, "Current (agtk)");
    assert_eq!(choices[1].label, "alpha");
    assert_eq!(choices[2].label, "zeta");
}

#[test]
fn worktree_choices_treat_trailing_project_separators_as_the_current_tree() {
    let choices = worktree_choices(
        "/work/agtk/",
        [("/work/agtk".to_owned(), "agtk".to_owned())],
    );

    assert_eq!(
        choices
            .iter()
            .map(|choice| choice.value.as_str())
            .collect::<Vec<_>>(),
        vec!["/work/agtk/"]
    );
}

#[test]
fn new_worktree_uses_origin_main_as_the_default_base_branch() {
    assert_eq!(DEFAULT_BASE_BRANCH, "origin/main");
}

#[test]
fn choice_matching_searches_names_and_absolute_paths() {
    // The `~/` query expands to the real home directory.
    let home = std::env::var("HOME").unwrap();
    let path = format!("{home}/shop/main-orders-overview");
    assert!(choice_matches("orders", "main-orders-overview", &path));
    assert!(choice_matches(
        "~/shop/main-orders",
        "main-orders-overview",
        &path
    ));
    assert!(!choice_matches("unrelated", "main-orders-overview", &path));
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
        agtk::launch_model::effective_worktree_path(Some(&project), None),
        Some(project.clone())
    );
    let selected = PathBuf::from("/tmp/project-worktree");
    assert_eq!(
        agtk::launch_model::effective_worktree_path(Some(&project), Some(&selected)),
        Some(selected)
    );
    assert_eq!(
        agtk::launch_model::effective_worktree_path(None, None),
        None
    );
}

#[test]
fn launch_paths_expand_home_directory_shorthand() {
    let home = std::env::var_os("HOME").expect("HOME is set for launch path expansion");
    assert_eq!(
        agtk::launch_model::expand_user_path("~/fastapi-restly"),
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

#[test]
fn a_restarted_fork_resumes_its_own_conversation_instead_of_forking_again() {
    let args = |args: &[&str]| args.iter().map(|arg| (*arg).to_owned()).collect::<Vec<_>>();
    assert_eq!(
        carried_agent_args(
            SessionKind::Claude,
            &args(&[
                "--resume",
                "parent-id",
                "--fork-session",
                "--session-id",
                "fork-id",
                "--model",
                "opus",
            ])
        ),
        args(&["--model", "opus"])
    );
    assert_eq!(
        carried_agent_args(
            SessionKind::Codex,
            &args(&["fork", "parent-id", "-m", "o3"])
        ),
        args(&["-m", "o3"])
    );
}

#[test]
fn a_restarted_pr_review_stays_without_the_agtk_tools() {
    let mut args = vec!["resume".to_owned(), "old-id".to_owned()];
    args.extend(codex_args_without_agtk_mcp());
    assert_eq!(
        carried_agent_args(SessionKind::Codex, &args),
        codex_args_without_agtk_mcp()
    );
}
