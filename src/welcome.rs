//! When agtk starts with nothing open, it opens an agent in its own source.
//! The agent explains agtk and changes it, so agtk adapts to its user.

use std::path::{Path, PathBuf};

use crate::control::{CreateSessionParams, SessionKind};

/// The welcome agent's first prompt. The user reads it as their own first message.
pub const PROMPT: &str = "\
agtk just started with nothing open, so it opened you in its own source code. \
Read README.md, then welcome me to agtk in a few short lines. Tell me you can:
- explain any agtk feature, or help me set up the optional integrations
- change agtk or add features to it: agtk is meant to keep changing to fit my workflow, \
and this checkout is where that happens
Also mention that Ctrl+Shift+` launches more agents and shells, in any project.
Don't change any files until I ask. After a change, run the checks from the README, \
rebuild with ./scripts/install-local.sh, and ask me to restart agtk. \
Sessions keep running while it restarts.";

/// The checkout this agtk was built from, unless it moved or was removed since.
pub fn source_dir() -> Option<PathBuf> {
    let source = Path::new(env!("CARGO_MANIFEST_DIR"));
    source
        .join("Cargo.toml")
        .is_file()
        .then(|| source.to_owned())
}

/// Claude in `source`, or Codex when only Codex is installed.
pub fn session_params(
    source: &Path,
    is_installed: impl Fn(SessionKind) -> bool,
) -> Option<CreateSessionParams> {
    let kind = [SessionKind::Claude, SessionKind::Codex]
        .into_iter()
        .find(|kind| is_installed(*kind))?;
    Some(CreateSessionParams {
        kind,
        command: None,
        args: Vec::new(),
        cwd: Some(source.to_owned()),
        name: None,
        project_root: Some(source.to_owned()),
        worktree_path: Some(source.to_owned()),
        initial_input: Some(PROMPT.to_owned()),
    })
}
