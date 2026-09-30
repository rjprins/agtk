use std::path::Path;

use crate::persist::SessionRecord;

pub(crate) fn next_worktree_session_name(
    fallback: &str,
    worktree_path: Option<&Path>,
    sessions: &[SessionRecord],
) -> String {
    let Some(worktree_path) = worktree_path else {
        return fallback.to_owned();
    };
    let Some(base) = worktree_path.file_name().and_then(|name| name.to_str()) else {
        return fallback.to_owned();
    };

    let next = sessions
        .iter()
        .filter(|session| {
            session.worktree_path.as_deref().or(session.cwd.as_deref()) == Some(worktree_path)
        })
        .filter_map(|session| session_name_increment(base, &session.name))
        .max()
        .map_or(1, |increment| increment.saturating_add(1));

    if next == 1 {
        base.to_owned()
    } else {
        format!("{base} {next}")
    }
}

/// Whether `name` is one this module generated for `worktree_path`, so it may follow a move.
pub(crate) fn follows_worktree_name(name: &str, worktree_path: Option<&Path>) -> bool {
    worktree_path
        .and_then(|path| path.file_name())
        .and_then(|base| base.to_str())
        .is_some_and(|base| session_name_increment(base, name).is_some())
}

fn session_name_increment(base: &str, name: &str) -> Option<u64> {
    if name == base {
        return Some(1);
    }
    name.strip_prefix(base)?
        .strip_prefix(' ')?
        .parse::<u64>()
        .ok()
        .filter(|increment| *increment >= 2)
}

#[cfg(test)]
mod tests {
    use super::{follows_worktree_name, next_worktree_session_name};
    use crate::persist::SessionRecord;
    use std::path::{Path, PathBuf};

    fn session(name: &str, worktree: &str) -> SessionRecord {
        let mut record = SessionRecord::discovered(name, PathBuf::from(format!("/{name}.sock")));
        record.name = name.to_owned();
        record.worktree_path = Some(PathBuf::from(worktree));
        record.cwd = Some(PathBuf::from(worktree));
        record
    }

    #[test]
    fn first_unnamed_session_uses_worktree_name() {
        assert_eq!(
            next_worktree_session_name("codex-123", Some(Path::new("/work/agtk")), &[]),
            "agtk"
        );
    }

    #[test]
    fn later_unnamed_sessions_use_the_next_worktree_increment() {
        let sessions = vec![
            session("agtk", "/work/agtk"),
            session("agtk 2", "/work/agtk"),
            session("agtk 4", "/work/agtk"),
        ];

        assert_eq!(
            next_worktree_session_name("codex-123", Some(Path::new("/work/agtk")), &sessions,),
            "agtk 5"
        );
    }

    #[test]
    fn increments_are_scoped_by_full_worktree_path() {
        let sessions = vec![session("agtk", "/other/agtk")];

        assert_eq!(
            next_worktree_session_name("codex-123", Some(Path::new("/work/agtk")), &sessions,),
            "agtk"
        );
    }

    #[test]
    fn only_generated_names_follow_the_worktree() {
        let worktree = Some(Path::new("/work/agtk"));
        assert!(follows_worktree_name("agtk", worktree));
        assert!(follows_worktree_name("agtk 3", worktree));
        assert!(!follows_worktree_name("agtk 1", worktree));
        assert!(!follows_worktree_name("PR 12 review", worktree));
        assert!(!follows_worktree_name("agtk", None));
    }

    #[test]
    fn missing_worktree_path_keeps_the_existing_fallback() {
        assert_eq!(
            next_worktree_session_name("codex-123", None, &[]),
            "codex-123"
        );
    }
}
