use std::path::{Path, PathBuf};

use agtk::control::SessionKind;
use agtk::welcome::{PROMPT, session_params, source_dir};

#[test]
fn welcome_prefers_claude_in_the_source_checkout() {
    let source = Path::new("/work/agtk");

    let params = session_params(source, |_| true).expect("an agent is installed");

    assert_eq!(params.kind, SessionKind::Claude);
    assert_eq!(params.cwd.as_deref(), Some(source));
    assert_eq!(params.project_root.as_deref(), Some(source));
    assert_eq!(params.worktree_path.as_deref(), Some(source));
    assert_eq!(params.initial_input.as_deref(), Some(PROMPT));
}

#[test]
fn welcome_falls_back_to_codex() {
    let params = session_params(Path::new("/work/agtk"), |kind| kind == SessionKind::Codex)
        .expect("codex is installed");

    assert_eq!(params.kind, SessionKind::Codex);
}

#[test]
fn welcome_needs_claude_or_codex() {
    assert!(session_params(Path::new("/work/agtk"), |kind| kind == SessionKind::Gemini).is_none());
}

#[test]
fn source_dir_is_this_checkout() {
    assert_eq!(
        source_dir(),
        Some(PathBuf::from(env!("CARGO_MANIFEST_DIR")))
    );
}
