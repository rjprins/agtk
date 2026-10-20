use super::*;
use crate::control::StateHookSetParams;
use crate::state_hook::{self, HookContext, StateChange};

impl Workspace {
    pub(super) fn load_state_hook(&self) {
        let Some(store) = self.store.borrow().clone() else {
            return;
        };
        self.run_io(
            move || store.preference_or_default::<StateHookPreferences>(state_hook::PREFERENCE_KEY),
            |workspace, result| match result {
                Ok(preferences) => *workspace.state_hook.borrow_mut() = preferences,
                Err(error) => eprintln!("agtk: could not load the state hook: {error}"),
            },
        );
    }

    pub(super) fn state_hook_summary(&self) -> serde_json::Value {
        serde_json::json!({ "stateHook": *self.state_hook.borrow() })
    }

    pub(super) fn set_state_hook(&self, params: StateHookSetParams, pending: PendingRequest) {
        let Some(store) = self.store.borrow().clone() else {
            self.report_failure(
                Some(pending),
                ErrorCode::InternalError,
                "Workspace is loading",
                "state hook settings are not available yet".to_owned(),
            );
            return;
        };
        let preferences = StateHookPreferences {
            command: params.command,
            states: params.states,
        };
        let value = match serde_json::to_value(&preferences) {
            Ok(value) => value,
            Err(error) => {
                self.report_launch_failure(
                    Some(pending),
                    "Could not encode the state hook",
                    error.to_string(),
                );
                return;
            }
        };
        self.run_io(
            move || store.set_preference(state_hook::PREFERENCE_KEY, &value),
            move |workspace, result| match result {
                Ok(()) => {
                    *workspace.state_hook.borrow_mut() = preferences;
                    let id = pending.request.id.clone();
                    let _ = pending
                        .respond(ControlResponse::success(id, workspace.state_hook_summary()));
                }
                Err(error) => workspace.report_launch_failure(
                    Some(pending),
                    "Could not save the state hook",
                    error.to_string(),
                ),
            },
        );
    }

    /// Runs the user's hook for a state change in the background. A failure
    /// only goes to the log: nobody is waiting on it.
    pub(super) fn run_state_hook(&self, change: StateChange) {
        let Some(command) = self
            .state_hook
            .borrow()
            .command_for(change.new_state)
            .map(str::to_owned)
        else {
            return;
        };
        let instance = self.paths.name().as_str().to_owned();
        let control_socket = self.paths.control_socket();
        // Nothing waits on the result, so the receiver is dropped.
        drop(self.hook_io.submit(move || {
            let context = HookContext {
                instance,
                control_socket,
                path: crate::session::login_shell_path(),
            };
            if let Err(error) = state_hook::run_hook(&command, &change, &context) {
                eprintln!(
                    "agtk: state hook for session {} ({} to {}) failed: {error}",
                    change.session_id,
                    state_hook::state_name(change.old_state),
                    state_hook::state_name(change.new_state),
                );
            }
        }));
    }
}
