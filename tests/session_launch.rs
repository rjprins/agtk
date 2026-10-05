use std::path::{Path, PathBuf};
use std::process::Command;

use agtk::control::{CreateSessionParams, SessionKind};
use agtk::session::{SessionHostLaunchPlan, SessionLaunchPlan};

#[test]
fn session_plan_infers_project_and_worktree_from_cwd() {
    let directory = tempfile::tempdir().unwrap();
    let root = directory.path().join("project");
    let nested = root.join("src/deep");
    std::fs::create_dir_all(&nested).unwrap();
    git(&root, &["init", "--quiet", "--initial-branch=main"]);

    for cwd in [&root, &nested] {
        for kind in [SessionKind::Shell, SessionKind::Codex, SessionKind::Claude] {
            let mut params = launch_params(cwd);
            params.kind = kind;
            let plan = SessionLaunchPlan::new(params).unwrap();

            assert_eq!(plan.cwd.as_ref(), Some(cwd));
            assert_eq!(plan.project_root.as_ref(), Some(&root));
            assert_eq!(plan.worktree_path.as_ref(), Some(&root));
        }
    }
}

#[test]
fn session_plan_groups_linked_worktrees_under_the_primary_repository() {
    let directory = tempfile::tempdir().unwrap();
    let root = directory.path().join("project");
    let linked = directory.path().join("project-feature");
    std::fs::create_dir(&root).unwrap();
    git(&root, &["init", "--quiet", "--initial-branch=main"]);
    git(
        &root,
        &["commit", "--quiet", "--allow-empty", "-m", "Initial"],
    );
    git(
        &root,
        &[
            "worktree",
            "add",
            "--quiet",
            "-b",
            "feature",
            linked.to_str().unwrap(),
        ],
    );
    let nested = linked.join("src/deep");
    std::fs::create_dir_all(&nested).unwrap();

    let plan = SessionLaunchPlan::new(launch_params(&nested)).unwrap();

    assert_eq!(plan.cwd, Some(nested));
    assert_eq!(plan.project_root, Some(root));
    assert_eq!(plan.worktree_path, Some(linked));
}

#[test]
fn session_plan_infers_the_checkout_when_git_metadata_lives_elsewhere() {
    let directory = tempfile::tempdir().unwrap();
    let root = directory.path().join("project");
    let metadata = directory.path().join("metadata");
    std::fs::create_dir(&root).unwrap();
    git(
        &root,
        &[
            "init",
            "--quiet",
            "--separate-git-dir",
            metadata.to_str().unwrap(),
        ],
    );

    let plan = SessionLaunchPlan::new(launch_params(&root)).unwrap();

    assert_eq!(plan.project_root.as_ref(), Some(&root));
    assert_eq!(plan.worktree_path, Some(root));
}

#[test]
fn session_plan_preserves_explicit_associations() {
    let directory = tempfile::tempdir().unwrap();
    let cwd = directory.path().join("cwd");
    let project = directory.path().join("project");
    let worktree = directory.path().join("worktree");
    for path in [&cwd, &project, &worktree] {
        std::fs::create_dir(path).unwrap();
    }
    git(&cwd, &["init", "--quiet", "--initial-branch=main"]);
    let mut params = launch_params(&cwd);
    params.project_root = Some(project.clone());
    params.worktree_path = Some(worktree.clone());

    let plan = SessionLaunchPlan::new(params).unwrap();

    assert_eq!(plan.cwd, Some(cwd));
    assert_eq!(plan.project_root, Some(project));
    assert_eq!(plan.worktree_path, Some(worktree));
}

#[test]
fn session_plan_fills_missing_associations_without_replacing_explicit_fields() {
    let directory = tempfile::tempdir().unwrap();
    let root = directory.path().join("project");
    let nested = root.join("src");
    let explicit_project = directory.path().join("explicit-project");
    std::fs::create_dir_all(&nested).unwrap();
    std::fs::create_dir(&explicit_project).unwrap();
    git(&root, &["init", "--quiet", "--initial-branch=main"]);
    let mut params = launch_params(&nested);
    params.project_root = Some(explicit_project.clone());

    let plan = SessionLaunchPlan::new(params).unwrap();

    assert_eq!(plan.project_root, Some(explicit_project));
    assert_eq!(plan.worktree_path.as_ref(), Some(&root));

    // The explicit association can differ from the process's working directory.
    let mut params = launch_params(directory.path());
    params.worktree_path = Some(root.clone());
    let plan = SessionLaunchPlan::new(params).unwrap();

    assert_eq!(plan.cwd.as_deref(), Some(directory.path()));
    assert_eq!(plan.project_root.as_ref(), Some(&root));
    assert_eq!(plan.worktree_path, Some(root));
}

#[test]
fn session_plan_leaves_non_git_directories_ungrouped() {
    let directory = tempfile::tempdir().unwrap();

    let plan = SessionLaunchPlan::new(launch_params(directory.path())).unwrap();

    assert_eq!(plan.project_root, None);
    assert_eq!(plan.worktree_path, None);
}

fn launch_params(cwd: &Path) -> CreateSessionParams {
    CreateSessionParams {
        kind: SessionKind::Claude,
        command: Some("/bin/sh".to_owned()),
        args: Vec::new(),
        cwd: Some(cwd.to_owned()),
        name: None,
        project_root: None,
        worktree_path: None,
        initial_input: None,
    }
}

fn git(root: &Path, args: &[&str]) {
    let output = Command::new("git")
        .current_dir(root)
        .args([
            "-c",
            "user.name=Test",
            "-c",
            "user.email=test@example.invalid",
        ])
        .args(args)
        .output()
        .unwrap();
    assert!(
        output.status.success(),
        "{}",
        String::from_utf8_lossy(&output.stderr)
    );
}

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
        cwd: Some(PathBuf::from("/definitely/missing/agtk")),
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
        command: Some("/definitely/missing/agtk-command".to_owned()),
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
        project_root: Some(PathBuf::from("/definitely/missing/agtk-project")),
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

#[test]
fn session_host_plan_uses_an_independent_systemd_user_scope() {
    let plan = SessionHostLaunchPlan::new(
        Some(PathBuf::from("/usr/bin/systemd-run")),
        PathBuf::from("/opt/agtk/agtk-session"),
        "default",
        "codex-123-0",
        PathBuf::from("/run/user/1000/agtk/default/sessions/codex-123-0.sock"),
        PathBuf::from("/usr/bin/codex"),
        vec!["resume".to_owned(), "conversation-id".to_owned()],
    );

    let command = plan.command();
    let args = command
        .get_args()
        .map(|argument| argument.to_string_lossy().into_owned())
        .collect::<Vec<_>>();

    assert_eq!(command.get_program(), "/usr/bin/systemd-run");
    assert_eq!(
        args,
        [
            "--user",
            "--scope",
            "--quiet",
            "--collect",
            "--expand-environment=no",
            "--unit=agtk-session-default-codex-123-0",
            "--",
            "/opt/agtk/agtk-session",
            "--socket",
            "/run/user/1000/agtk/default/sessions/codex-123-0.sock",
            "--",
            "/usr/bin/codex",
            "resume",
            "conversation-id",
        ]
    );
    assert!(plan.fallback_command().is_some());
}

#[test]
fn session_host_plan_falls_back_to_direct_launch_without_systemd_run() {
    let plan = SessionHostLaunchPlan::new(
        None,
        PathBuf::from("/opt/agtk/agtk-session"),
        "test-instance",
        "claude-456-1",
        PathBuf::from("/tmp/claude-456-1.sock"),
        PathBuf::from("/usr/bin/claude"),
        vec!["--resume".to_owned(), "conversation-id".to_owned()],
    );

    let command = plan.command();
    let args = command
        .get_args()
        .map(|argument| argument.to_string_lossy().into_owned())
        .collect::<Vec<_>>();

    assert_eq!(command.get_program(), "/opt/agtk/agtk-session");
    assert_eq!(
        args,
        [
            "--socket",
            "/tmp/claude-456-1.sock",
            "--",
            "/usr/bin/claude",
            "--resume",
            "conversation-id",
        ]
    );
    assert!(plan.fallback_command().is_none());
}

#[test]
fn agent_plan_passes_the_first_prompt_as_an_argument_instead_of_typing_it() {
    let params = CreateSessionParams {
        kind: SessionKind::Claude,
        command: Some("/bin/sh".to_owned()),
        args: vec!["--model".to_owned(), "haiku".to_owned()],
        cwd: None,
        name: None,
        project_root: None,
        worktree_path: None,
        initial_input: Some("-fix the\nbuild".to_owned()),
    };

    let plan = SessionLaunchPlan::new(params).expect("valid launch plan");

    assert_eq!(plan.args, ["--model", "haiku"]);
    assert_eq!(plan.prompt_args, ["--", "-fix the\nbuild"]);
    assert_eq!(plan.initial_input, None);
}
