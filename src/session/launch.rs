use std::env;
use std::ffi::OsString;
use std::fmt;
use std::fs;
use std::os::unix::fs::PermissionsExt;
use std::path::{Path, PathBuf};
use std::process::Command;

use crate::control::{CreateSessionParams, SessionKind};

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct SessionLaunchPlan {
    pub kind: SessionKind,
    pub program: PathBuf,
    pub args: Vec<String>,
    pub cwd: Option<PathBuf>,
    pub name: Option<String>,
    pub initial_input: Option<String>,
    pub project_root: Option<PathBuf>,
    pub worktree_path: Option<PathBuf>,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct SessionHostLaunchPlan {
    program: PathBuf,
    args: Vec<OsString>,
    fallback: Option<(PathBuf, Vec<OsString>)>,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct LaunchPlanError(String);

impl SessionLaunchPlan {
    pub fn new(params: CreateSessionParams) -> Result<Self, LaunchPlanError> {
        let cwd = canonical_directory(params.cwd, "working directory")?;
        let project_root = canonical_directory(params.project_root, "project root")?;
        let worktree_path = canonical_directory(params.worktree_path, "worktree path")?;

        let command = params.command.unwrap_or_else(|| match params.kind {
            SessionKind::Shell => env::var("SHELL").unwrap_or_else(|_| "/bin/sh".to_owned()),
            SessionKind::Codex => {
                env::var("AGMUX_CODEX_BIN").unwrap_or_else(|_| "codex".to_owned())
            }
            SessionKind::Claude => {
                env::var("AGMUX_CLAUDE_BIN").unwrap_or_else(|_| "claude".to_owned())
            }
            SessionKind::Gemini => {
                env::var("AGMUX_GEMINI_BIN").unwrap_or_else(|_| "gemini".to_owned())
            }
            SessionKind::Custom => String::new(),
        });
        if command.trim().is_empty() {
            return Err(LaunchPlanError(
                "custom sessions require an executable".to_owned(),
            ));
        }
        let program = resolve_executable(&command, cwd.as_deref()).ok_or_else(|| {
            LaunchPlanError(format!(
                "executable was not found or is not executable: {command}"
            ))
        })?;

        if params.args.iter().any(|argument| argument.contains('\0')) {
            return Err(LaunchPlanError(
                "arguments cannot contain NUL bytes".to_owned(),
            ));
        }
        let cwd = cwd
            .or_else(|| env::current_dir().ok())
            .map(|path| fs::canonicalize(path).map_err(|e| LaunchPlanError(e.to_string())))
            .transpose()?;
        Ok(Self {
            kind: params.kind,
            program,
            args: params.args,
            cwd,
            name: params.name,
            initial_input: params.initial_input,
            project_root,
            worktree_path,
        })
    }
}

impl SessionHostLaunchPlan {
    pub fn detected(
        host_binary: PathBuf,
        instance: &str,
        session_id: &str,
        socket_path: PathBuf,
        program: PathBuf,
        args: Vec<String>,
    ) -> Self {
        Self::new(
            resolve_executable("systemd-run", None),
            host_binary,
            instance,
            session_id,
            socket_path,
            program,
            args,
        )
    }

    pub fn new(
        systemd_run: Option<PathBuf>,
        host_binary: PathBuf,
        instance: &str,
        session_id: &str,
        socket_path: PathBuf,
        program: PathBuf,
        args: Vec<String>,
    ) -> Self {
        let mut host_args = vec![
            OsString::from("--socket"),
            socket_path.into_os_string(),
            OsString::from("--"),
            program.into_os_string(),
        ];
        host_args.extend(args.into_iter().map(OsString::from));

        let Some(systemd_run) = systemd_run else {
            return Self {
                program: host_binary,
                args: host_args,
                fallback: None,
            };
        };

        // A transient scope inherits the caller's environment but is managed outside the
        // desktop application's cgroup. See systemd-run(1):
        // https://www.freedesktop.org/software/systemd/man/latest/systemd-run.html
        let mut scoped_args = vec![
            OsString::from("--user"),
            OsString::from("--scope"),
            OsString::from("--quiet"),
            OsString::from("--collect"),
            OsString::from("--expand-environment=no"),
            OsString::from(format!("--unit=agmux-session-{instance}-{session_id}")),
            OsString::from("--"),
            host_binary.clone().into_os_string(),
        ];
        scoped_args.extend(host_args.iter().cloned());
        Self {
            program: systemd_run,
            args: scoped_args,
            fallback: Some((host_binary, host_args)),
        }
    }

    pub fn command(&self) -> Command {
        let mut command = Command::new(&self.program);
        command.args(&self.args);
        command
    }

    pub fn fallback_command(&self) -> Option<Command> {
        let (program, args) = self.fallback.as_ref()?;
        let mut command = Command::new(program);
        command.args(args);
        Some(command)
    }
}

fn canonical_directory(
    path: Option<PathBuf>,
    label: &str,
) -> Result<Option<PathBuf>, LaunchPlanError> {
    path.map(|path| {
        let path = crate::launch_model::expand_user_path(path.to_string_lossy().as_ref());
        if !path.is_dir() {
            return Err(LaunchPlanError(format!(
                "{label} does not exist or is not a directory: {}",
                path.display()
            )));
        }
        fs::canonicalize(&path).map_err(|error| {
            LaunchPlanError(format!(
                "could not resolve {label} {}: {error}",
                path.display()
            ))
        })
    })
    .transpose()
}

impl fmt::Display for LaunchPlanError {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        formatter.write_str(&self.0)
    }
}

impl std::error::Error for LaunchPlanError {}

fn resolve_executable(command: &str, cwd: Option<&Path>) -> Option<PathBuf> {
    resolve_executable_from(
        command,
        cwd,
        env::var_os("PATH").as_deref(),
        env::var_os("HOME").as_deref(),
    )
}

fn resolve_executable_from(
    command: &str,
    cwd: Option<&Path>,
    path: Option<&std::ffi::OsStr>,
    home: Option<&std::ffi::OsStr>,
) -> Option<PathBuf> {
    let command_path = Path::new(command);
    if command_path.components().count() > 1 {
        let candidate = if command_path.is_absolute() {
            command_path.to_owned()
        } else {
            cwd.map_or_else(
                || env::current_dir().unwrap_or_else(|_| PathBuf::from(".")),
                Path::to_owned,
            )
            .join(command_path)
        };
        return is_executable(&candidate).then_some(candidate);
    }

    let mut directories = path
        .into_iter()
        .flat_map(env::split_paths)
        .collect::<Vec<_>>();
    if let Some(home) = home {
        directories.push(PathBuf::from(home).join(".npm-global/bin"));
    }
    directories
        .into_iter()
        .map(|directory| directory.join(command_path))
        .find(|candidate| is_executable(candidate))
}

fn is_executable(path: &Path) -> bool {
    fs::metadata(path)
        .is_ok_and(|metadata| metadata.is_file() && metadata.permissions().mode() & 0o111 != 0)
}

#[cfg(test)]
mod tests {
    use std::ffi::OsStr;
    use std::os::unix::fs::PermissionsExt;

    use super::resolve_executable_from;

    #[test]
    fn resolves_user_npm_global_executable_when_path_omits_it() {
        let home = tempfile::tempdir().expect("create fake home directory");
        let bin = home.path().join(".npm-global/bin");
        std::fs::create_dir_all(&bin).expect("create npm global bin directory");
        let executable = bin.join("codex");
        std::fs::write(&executable, "#!/bin/sh\n").expect("create fake executable");
        let mut permissions = std::fs::metadata(&executable)
            .expect("read fake executable metadata")
            .permissions();
        permissions.set_mode(0o755);
        std::fs::set_permissions(&executable, permissions).expect("make fake executable runnable");

        let resolved = resolve_executable_from(
            "codex",
            None,
            Some(OsStr::new("/usr/bin")),
            Some(home.path().as_os_str()),
        );

        assert_eq!(resolved.as_deref(), Some(executable.as_path()));
    }
}
