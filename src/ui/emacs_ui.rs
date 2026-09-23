use super::*;
use crate::emacs::{EmacsAction, EmacsIntegration};

impl Workspace {
    pub(super) fn open_session_in_emacs(
        &self,
        session_id: &str,
        action: EmacsAction,
        pending: Option<PendingRequest>,
    ) {
        let cwd = self.sessions.borrow().get(session_id).and_then(|session| {
            session
                .record
                .worktree_path
                .clone()
                .or_else(|| session.record.cwd.clone())
                .or_else(|| session.record.project_root.clone())
        });
        let Some(cwd) = cwd else {
            self.report_failure(
                pending,
                ErrorCode::OperationRefused,
                "Could not open Emacs",
                "session has no working directory".to_owned(),
            );
            return;
        };
        self.run_io(
            move || EmacsIntegration::from_environment().open(&cwd, action),
            move |workspace, result| match result {
                Ok(opened) => match serde_json::to_value(opened) {
                    Ok(value) => {
                        if let Some(pending) = pending {
                            let id = pending.request.id.clone();
                            let _ = pending.respond(ControlResponse::success(id, value));
                        }
                    }
                    Err(error) => workspace.report_launch_failure(
                        pending,
                        "Could not describe Emacs action",
                        error.to_string(),
                    ),
                },
                Err(error) => workspace.report_failure(
                    pending,
                    ErrorCode::OperationRefused,
                    "Could not open Emacs",
                    error.to_string(),
                ),
            },
        );
    }

    pub(super) fn update_emacs_actions(&self) {
        let enabled = self.selected_session_id().is_some_and(|session_id| {
            self.sessions
                .borrow()
                .get(&session_id)
                .is_some_and(|session| {
                    session.record.worktree_path.is_some()
                        || session.record.cwd.is_some()
                        || session.record.project_root.is_some()
                })
        });
        self.menus.git_button.set_sensitive(enabled);
        self.menus.magit.set_enabled(enabled);
        self.menus.branch_review.set_enabled(enabled);
    }
}
