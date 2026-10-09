//! When agtk starts with nothing open, it opens an agent in its own source.
//! The agent explains agtk and changes it, so agtk adapts to its user.

use std::path::{Path, PathBuf};

use crate::control::CreateSessionParams;

use crate::session::SessionKind;

/// The welcome agent's first prompt. The user reads it as their own first message.
pub const PROMPT: &str = "\
agtk just started with nothing open, so it opened you in its own source code. \
This checkout is my own copy of agtk, personal software that I change to fit how I work. \
Read README.md, then welcome me in a few short lines: you can explain any part of agtk, \
and change it or add to it when I ask. Mention that Ctrl+Shift+Backquote launches more \
agents and shells, in any project. Then ask what I want to know or change, \
and offer these as examples:
- \"How do sessions keep running when I close the window?\"
- \"Rename agtk to ...\"
- \"Add a dialog that lists my open GitHub pull requests\"
- \"Remove the 'Open this file in Emacs' button\"
- \"Add OpenCode as an agent I can launch\"

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
