use super::*;
use crate::claude_presets::{ClaudeModelPreset, ClaudePresetPreferences};
use crate::control::{ClaudePresetApplyParams, ClaudePresetsSetParams};

impl Workspace {
    pub(super) fn handle_claude_control(&self, pending: PendingRequest) {
        match pending.request.command.clone() {
            ControlCommand::ClaudePresetsSet(params) => {
                self.set_claude_presets(params, Some(pending));
            }
            ControlCommand::ClaudePresetApply(params) => {
                self.apply_claude_preset(params, Some(pending));
            }
            _ => unreachable!("caller filters Claude commands"),
        }
    }

    pub(super) fn load_claude_presets(&self, preferences: ClaudePresetPreferences) {
        *self.claude_presets.borrow_mut() = preferences;
        self.selected_claude_preset.set(0);
        let presets = &self.claude_presets.borrow().presets;
        self.preferences
            .claude_presets
            .set_text(&serde_json::to_string(presets).unwrap_or_else(|_| "[]".to_owned()));
        self.render_claude_preset_menu();
        self.update_claude_actions();
    }

    pub(super) fn set_claude_presets(
        &self,
        params: ClaudePresetsSetParams,
        pending: Option<PendingRequest>,
    ) {
        let preferences = match ClaudePresetPreferences::new(params.presets) {
            Ok(preferences) => preferences,
            Err(error) => {
                self.report_failure(
                    pending,
                    ErrorCode::InvalidParams,
                    "Claude presets are invalid",
                    error.to_string(),
                );
                return;
            }
        };
        let Some(store) = self.store.borrow().clone() else {
            self.report_failure(
                pending,
                ErrorCode::InternalError,
                "Workspace is loading",
                "Claude preset settings are not available yet".to_owned(),
            );
            return;
        };
        let value = match serde_json::to_value(&preferences.presets) {
            Ok(value) => value,
            Err(error) => {
                self.report_launch_failure(
                    pending,
                    "Could not encode Claude presets",
                    error.to_string(),
                );
                return;
            }
        };
        self.run_io(
            move || store.set_preference("claudeModelPresets", &value),
            move |workspace, result| match result {
                Ok(()) => {
                    workspace.load_claude_presets(preferences);
                    if let Some(pending) = pending {
                        let id = pending.request.id.clone();
                        match serde_json::to_value(&workspace.claude_presets.borrow().presets) {
                            Ok(presets) => {
                                let _ = pending.respond(ControlResponse::success(
                                    id,
                                    serde_json::json!({ "presets": presets }),
                                ));
                            }
                            Err(error) => workspace.report_launch_failure(
                                Some(pending),
                                "Could not describe Claude presets",
                                error.to_string(),
                            ),
                        }
                    }
                }
                Err(error) => workspace.report_launch_failure(
                    pending,
                    "Could not save Claude presets",
                    error.to_string(),
                ),
            },
        );
    }

    pub(super) fn apply_claude_preset(
        &self,
        params: ClaudePresetApplyParams,
        pending: Option<PendingRequest>,
    ) {
        let preset = self
            .claude_presets
            .borrow()
            .preset(&params.preset_id)
            .cloned();
        let Some(preset) = preset else {
            self.report_failure(
                pending,
                ErrorCode::OperationRefused,
                "Claude preset was not found",
                params.preset_id,
            );
            return;
        };
        let terminal = self
            .sessions
            .borrow()
            .get(&params.session_id)
            .filter(|session| {
                session.record.kind == SessionKind::Claude
                    && session.record.state != SessionState::Exited
            })
            .map(|session| session.terminal.clone());
        let Some(terminal) = terminal else {
            self.report_failure(
                pending,
                ErrorCode::OperationRefused,
                "Preset requires a live Claude session",
                params.session_id,
            );
            return;
        };
        for command in preset.commands() {
            terminal.feed_child(command.as_bytes());
        }
        self.mark_agent_busy(&params.session_id);
        if let Some(pending) = pending {
            let id = pending.request.id.clone();
            let _ = pending.respond(ControlResponse::success(
                id,
                serde_json::json!({
                    "sessionId": params.session_id,
                    "preset": preset,
                    "commandsSent": 2
                }),
            ));
        }
    }

    pub(super) fn apply_claude_presets_from_entry(&self) {
        let presets = match serde_json::from_str::<Vec<ClaudeModelPreset>>(
            self.preferences.claude_presets.text().as_str(),
        ) {
            Ok(presets) => presets,
            Err(error) => {
                self.show_error(&format!("Invalid Claude preset JSON: {error}"));
                return;
            }
        };
        self.set_claude_presets(ClaudePresetsSetParams { presets }, None);
    }

    pub(super) fn open_or_cycle_claude_presets(&self) {
        let Some(session_id) = self.selected_session_id() else {
            self.show_error("Select a Claude session first");
            return;
        };
        let is_claude = self
            .sessions
            .borrow()
            .get(&session_id)
            .is_some_and(|session| {
                session.record.kind == SessionKind::Claude
                    && session.record.state != SessionState::Exited
            });
        if !is_claude {
            self.show_error("Model presets apply only to Claude sessions");
            return;
        }
        let count = self.claude_presets.borrow().presets.len();
        if count == 0 {
            self.show_error("No Claude model presets are configured");
            return;
        }
        let was_visible = self.claude_model_window.is_visible();
        let index = if was_visible {
            (self.selected_claude_preset.get() + 1).rem_euclid(count as i32)
        } else {
            self.claude_model_window.present();
            0
        };
        self.selected_claude_preset.set(index);
        if was_visible {
            self.focus_selected_claude_preset();
        } else {
            let workspace = self.clone();
            glib::idle_add_local_once(move || workspace.focus_selected_claude_preset());
        }
    }

    pub(super) fn update_claude_actions(&self) {
        let selected_is_claude = self.selected_session_id().is_some_and(|id| {
            self.sessions.borrow().get(&id).is_some_and(|session| {
                session.record.kind == SessionKind::Claude
                    && session.record.state != SessionState::Exited
            })
        });
        self.claude_model_button.set_visible(selected_is_claude);
        self.claude_model_button
            .set_sensitive(selected_is_claude && !self.claude_presets.borrow().presets.is_empty());
        if !selected_is_claude {
            self.claude_model_window.hide();
        }
    }

    fn render_claude_preset_menu(&self) {
        while let Some(child) = self.claude_model_list.first_child() {
            self.claude_model_list.remove(&child);
        }
        for preset in self.claude_presets.borrow().presets.clone() {
            let button = gtk::Button::with_label(&format!(
                "[{}]  {} / {}",
                preset.name,
                preset.model,
                preset.effort.as_str()
            ));
            button.set_halign(gtk::Align::Fill);
            let workspace = self.clone();
            let preset_id = preset.id;
            button.connect_clicked(move |_| {
                let Some(session_id) = workspace.selected_session_id() else {
                    return;
                };
                workspace.claude_model_window.hide();
                workspace.apply_claude_preset(
                    ClaudePresetApplyParams {
                        session_id,
                        preset_id: preset_id.clone(),
                    },
                    None,
                );
            });
            self.claude_model_list.append(&button);
        }
    }

    fn focus_selected_claude_preset(&self) {
        if !self.claude_model_window.is_visible() {
            return;
        }
        let index = self.selected_claude_preset.get().max(0) as usize;
        if let Some(button) = nth_child(&self.claude_model_list, index) {
            button.grab_focus();
        }
    }
}

fn nth_child(container: &gtk::Box, index: usize) -> Option<gtk::Widget> {
    let mut child = container.first_child()?;
    for _ in 0..=index {
        child = child.next_sibling()?;
    }
    Some(child)
}
