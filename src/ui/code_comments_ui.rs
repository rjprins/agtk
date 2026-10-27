//! Comments on text selected in the code viewer, sent with that text to an
//! agent session in the same worktree.

use super::*;
use crate::changes::DiffScope;
use crate::workspace_tabs::WorkspaceTabId;

/// A comment on the selection, as the viewer sends it.
pub(super) struct SelectionComment {
    pub(super) comment_id: u32,
    pub(super) document_id: String,
    pub(super) session_id: String,
    pub(super) comment: String,
    /// The first and last line, absent for words selected in rendered Markdown.
    pub(super) lines: Option<(u32, u32)>,
    /// Absent when the selection was too long to send.
    pub(super) text: Option<String>,
}

impl Workspace {
    /// Offers the viewer the agent sessions of the shown file's worktree.
    pub(super) fn refresh_comment_sessions(&self) {
        let Some(viewer) = self.code_viewer.borrow().clone() else {
            return;
        };
        let (sessions, preferred) = self.comment_sessions();
        viewer.set_comment_sessions(&sessions, preferred.as_deref());
    }

    pub(super) fn send_code_comment(&self, comment: SelectionComment) {
        let error = self
            .deliver_code_comment(&comment)
            .err()
            .unwrap_or_default();
        if let Some(viewer) = self.code_viewer.borrow().clone() {
            viewer.answer_comment(comment.comment_id, &error);
        }
    }

    /// The running agent sessions, as `(id, name)`, in the worktree of the shown
    /// diff or file, and the session that opened it when it is one of them.
    fn comment_sessions(&self) -> (Vec<(String, String)>, Option<String>) {
        let tabs = self.workspace_tabs.borrow();
        let root = match tabs.visible_tab() {
            Some(WorkspaceTabId::Diff(key)) => key.worktree_root.clone(),
            Some(WorkspaceTabId::File(key)) => key.worktree_root.clone(),
            _ => return (Vec::new(), None),
        };
        let sessions = self.sessions.borrow();
        let offered = tabs
            .context_tabs(&root)
            .into_iter()
            .filter_map(|tab| match tab {
                WorkspaceTabId::Session(id) => Some(id),
                _ => None,
            })
            .filter_map(|id| {
                let record = &sessions.get(&id)?.record;
                (status_ui::is_agent(record.kind) && record.state != SessionState::Exited)
                    .then(|| (id, record.name.clone()))
            })
            .collect::<Vec<_>>();
        let preferred = tabs
            .selected_session()
            .filter(|selected| offered.iter().any(|(id, _)| id == selected))
            .map(str::to_owned);
        (offered, preferred)
    }

    fn deliver_code_comment(&self, comment: &SelectionComment) -> Result<(), String> {
        const NOT_SHOWN: &str = "This file is no longer shown";
        if self.diff_current_request.borrow().as_deref() != Some(comment.document_id.as_str()) {
            return Err(NOT_SHOWN.to_owned());
        }
        let (path, version) = match self.workspace_tabs.borrow().visible_tab() {
            Some(WorkspaceTabId::File(key)) => (
                key.path
                    .strip_prefix(&key.worktree_root)
                    .unwrap_or(&key.path)
                    .display()
                    .to_string(),
                None,
            ),
            Some(WorkspaceTabId::Diff(key)) => {
                let path = key.new_path.as_deref().or(key.old_path.as_deref());
                let Some(path) = path else {
                    return Err(NOT_SHOWN.to_owned());
                };
                let version = match &key.scope {
                    DiffScope::Staged => Some("staged version".to_owned()),
                    DiffScope::Commit { oid } => {
                        Some(format!("as of commit {}", oid.get(..10).unwrap_or(oid)))
                    }
                    _ => None,
                };
                (String::from_utf8_lossy(path).into_owned(), version)
            }
            _ => return Err(NOT_SHOWN.to_owned()),
        };
        let (sessions, _) = self.comment_sessions();
        if !sessions.iter().any(|(id, _)| id == &comment.session_id) {
            return Err("That session no longer runs in this worktree".to_owned());
        }
        let message = comment_message(
            &path,
            version.as_deref(),
            comment.lines,
            comment.text.as_deref(),
            &comment.comment,
        );
        self.send_session_input(&comment.session_id, &message, true)
            .map(|_| ())
            .ok_or_else(|| "That session no longer runs".to_owned())
    }
}

/// The message an agent gets: where the selection is, the selected text and the comment.
fn comment_message(
    path: &str,
    version: Option<&str>,
    lines: Option<(u32, u32)>,
    text: Option<&str>,
    comment: &str,
) -> String {
    let lines = match lines {
        Some((start, end)) if start == end => format!(", line {start}"),
        Some((start, end)) => format!(", lines {start}-{end}"),
        None => String::new(),
    };
    let version = version
        .map(|version| format!(" ({version})"))
        .unwrap_or_default();
    let mut message = format!("Comment on {}{version}{lines}:\n", printable(path));
    match text {
        Some(text) => {
            let text = printable(text);
            let text = text.trim_end_matches('\n');
            let fence = "`".repeat(longest_backtick_run(text).max(2) + 1);
            message.push_str(&format!("{fence}\n{text}\n{fence}\n"));
        }
        None => message.push_str("(Too long to quote here: read these lines in the file.)\n"),
    }
    message.push_str(printable(comment).trim());
    message
}

/// Drops control characters, so file contents cannot act as keys in the
/// agent's terminal, such as an early end of the paste.
fn printable(text: &str) -> String {
    text.chars()
        .filter(|character| !character.is_control() || matches!(character, '\n' | '\t'))
        .collect()
}

fn longest_backtick_run(text: &str) -> usize {
    text.split(|character| character != '`')
        .map(str::len)
        .max()
        .unwrap_or(0)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn quotes_the_selection_with_its_place() {
        assert_eq!(
            comment_message(
                "src/main.rs",
                None,
                Some((3, 4)),
                Some("fn main() {\n}\n"),
                "  Why?\n"
            ),
            "Comment on src/main.rs, lines 3-4:\n```\nfn main() {\n}\n```\nWhy?"
        );
    }

    #[test]
    fn names_one_line_and_the_version() {
        assert_eq!(
            comment_message(
                "a.py",
                Some("staged version"),
                Some((7, 7)),
                Some("x"),
                "Rename"
            ),
            "Comment on a.py (staged version), line 7:\n```\nx\n```\nRename"
        );
    }

    #[test]
    fn fences_longer_than_backticks_inside() {
        let message = comment_message(
            "README.md",
            None,
            Some((1, 3)),
            Some("```\ncode\n```"),
            "Ok",
        );
        assert!(message.contains("````\n```\ncode\n```\n````\n"));
    }

    #[test]
    fn points_at_the_file_when_the_selection_is_too_long() {
        assert_eq!(
            comment_message("big.txt", None, Some((1, 900)), None, "Summarize"),
            "Comment on big.txt, lines 1-900:\n\
             (Too long to quote here: read these lines in the file.)\nSummarize"
        );
    }

    #[test]
    fn drops_control_characters() {
        let message = comment_message(
            "a.txt",
            None,
            Some((1, 1)),
            Some("a\x1b[201~b\r\tc"),
            "d\x03e",
        );
        assert_eq!(
            message,
            "Comment on a.txt, line 1:\n```\na[201~b\tc\n```\nde"
        );
    }

    #[test]
    fn quotes_words_from_rendered_markdown_without_lines() {
        assert_eq!(
            comment_message("README.md", None, None, Some("some words"), "Clearer?"),
            "Comment on README.md:\n```\nsome words\n```\nClearer?"
        );
    }
}
