use std::path::PathBuf;

use agmux_native::control::{CreateSessionParams, SessionKind};
use agmux_native::session::SessionLaunchPlan;

#[test]
fn custom_session_plan_preserves_validated_launch_details() {
    let directory = tempfile::tempdir().expect("create working directory");
    let params = CreateSessionParams {
        kind: SessionKind::Custom,
        command: Some("/bin/sh".to_owned()),
        args: vec!["-l".to_owned()],
        cwd: Some(directory.path().to_owned()),
        name: Some("Login shell".to_owned()),
        project_root: None,
        worktree_path: None,
        initial_input: Some("printf ready".to_owned()),
    };

    let plan = SessionLaunchPlan::new(params).expect("valid launch plan");

    assert_eq!(plan.kind, SessionKind::Custom);
    assert_eq!(plan.program, PathBuf::from("/bin/sh"));
    assert_eq!(plan.args, ["-l"]);
    assert_eq!(plan.cwd.as_deref(), Some(directory.path()));
    assert_eq!(plan.name.as_deref(), Some("Login shell"));
    assert_eq!(plan.initial_input.as_deref(), Some("printf ready"));
}

#[test]
fn session_plan_expands_home_shorthand_for_launch_directories() {
    let home = std::env::var_os("HOME").expect("HOME is set for launch path expansion");
    let home = std::fs::canonicalize(home).expect("home directory is accessible");
    let params = CreateSessionParams {
        kind: SessionKind::Custom,
        command: Some("/bin/sh".to_owned()),
        args: Vec::new(),
        cwd: Some(PathBuf::from("~")),
        name: None,
        project_root: Some(PathBuf::from("~")),
        worktree_path: Some(PathBuf::from("~")),
        initial_input: None,
    };

    let plan = SessionLaunchPlan::new(params).expect("home shorthand is a valid directory");

    assert_eq!(plan.cwd.as_deref(), Some(home.as_path()));
    assert_eq!(plan.project_root.as_deref(), Some(home.as_path()));
    assert_eq!(plan.worktree_path.as_deref(), Some(home.as_path()));
}

#[test]
fn session_plan_rejects_a_missing_working_directory() {
    let params = CreateSessionParams {
        kind: SessionKind::Shell,
        command: Some("/bin/sh".to_owned()),
        args: Vec::new(),
        cwd: Some(PathBuf::from("/definitely/missing/agmux-native")),
        name: None,
        project_root: None,
        worktree_path: None,
        initial_input: None,
    };

    let error = SessionLaunchPlan::new(params).expect_err("missing cwd must fail");

    assert!(error.to_string().contains("working directory"));
}

#[test]
fn session_plan_rejects_a_missing_executable() {
    let params = CreateSessionParams {
        kind: SessionKind::Custom,
        command: Some("/definitely/missing/agmux-command".to_owned()),
        args: Vec::new(),
        cwd: None,
        name: None,
        project_root: None,
        worktree_path: None,
        initial_input: None,
    };

    let error = SessionLaunchPlan::new(params).expect_err("missing executable must fail");

    assert!(error.to_string().contains("executable"));
}

#[test]
fn session_plan_rejects_missing_project_and_nul_arguments() {
    let missing_project = CreateSessionParams {
        kind: SessionKind::Shell,
        command: Some("/bin/sh".to_owned()),
        args: Vec::new(),
        cwd: None,
        name: None,
        project_root: Some(PathBuf::from("/definitely/missing/agmux-project")),
        worktree_path: None,
        initial_input: None,
    };
    assert!(
        SessionLaunchPlan::new(missing_project)
            .unwrap_err()
            .to_string()
            .contains("project root")
    );

    let nul_argument = CreateSessionParams {
        kind: SessionKind::Shell,
        command: Some("/bin/sh".to_owned()),
        args: vec!["bad\0argument".to_owned()],
        cwd: None,
        name: None,
        project_root: None,
        worktree_path: None,
        initial_input: None,
    };
    assert!(
        SessionLaunchPlan::new(nul_argument)
            .unwrap_err()
            .to_string()
            .contains("NUL")
    );
}
