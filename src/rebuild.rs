use std::path::{Path, PathBuf};
use std::process::Command;
use std::time::Duration;

use crate::command_runner::run_bounded;

/// Rebuild the checkout into the target directory the running window came from.
/// This keeps custom CARGO_TARGET_DIR installations pointed at the updated binaries.
#[derive(Debug, Clone)]
pub struct RebuildPlan {
    root: PathBuf,
    target: PathBuf,
    executable: PathBuf,
}

impl RebuildPlan {
    pub fn new(root: &Path, running_executable: &Path) -> Result<Self, String> {
        if !root.join("Cargo.toml").is_file() || !root.join("scripts/build-viewer.sh").is_file() {
            return Err(format!(
                "The agtk checkout is unavailable at {}",
                root.display()
            ));
        }
        let debug = running_executable.parent().filter(|path| path.file_name().is_some_and(|name| name == "debug"))
            .ok_or("agtk must run from its checkout's debug build. Run scripts/install-local.sh first.")?;
        let target = debug
            .parent()
            .ok_or("The build target directory is unavailable")?;
        Ok(Self {
            root: root.to_path_buf(),
            target: target.to_path_buf(),
            // Linux adds " (deleted)" to current_exe after a previous rebuild.
            executable: debug.join("agtk"),
        })
    }

    pub fn run(&self, cargo: &Path) -> Result<PathBuf, String> {
        let mut viewer = Command::new(self.root.join("scripts/build-viewer.sh"));
        viewer.current_dir(&self.root);
        run_step(viewer, "Build Changes viewer")?;
        let mut build = Command::new(cargo);
        build
            .current_dir(&self.root)
            .args(["build", "--locked", "--bins", "--target-dir"])
            .arg(&self.target);
        run_step(build, "Build agtk")?;
        use std::os::unix::fs::PermissionsExt;
        if !std::fs::metadata(&self.executable)
            .is_ok_and(|metadata| metadata.is_file() && metadata.permissions().mode() & 0o111 != 0)
        {
            return Err(format!(
                "The rebuilt agtk binary is unavailable at {}",
                self.executable.display()
            ));
        }
        Ok(self.executable.clone())
    }
}

fn run_step(command: Command, label: &str) -> Result<(), String> {
    let output = run_bounded(command, Duration::from_secs(15 * 60), 4 * 1024 * 1024)
        .map_err(|error| format!("{label}: {error}"))?;
    if !output.status.success() {
        return Err(format!(
            "{label} failed ({}).\n{}\n{}",
            output.status, output.stdout, output.stderr
        ));
    }
    Ok(())
}
