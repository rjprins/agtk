use super::*;

impl Workspace {
    pub(super) fn install_shortcut_actions(&self) {
        self.add_action("new-shell", true, |workspace| workspace.launch_shell());
        self.add_action("close-session", true, |workspace| {
            if let Some(id) = workspace.selected_session_id() {
                workspace.stop_session(&id, None);
            }
        });
        self.add_action("toggle-sidebar", true, |workspace| {
            workspace
                .sidebar_panel
                .set_visible(!workspace.sidebar_panel.is_visible());
        });
        self.add_action("next-session", true, |workspace| {
            workspace.select_relative_session(1);
        });
        self.add_action("previous-session", true, |workspace| {
            workspace.select_relative_session(-1);
        });
        self.add_action("next-ready-session", true, |workspace| {
            workspace.select_next_ready_session();
        });
        self.add_action("reopen-pr-list", true, |workspace| {
            if let Some(root) = workspace.preferred_project_root() {
                workspace.pr_root.set_text(&root);
                workspace.pr_button.popup();
            } else {
                workspace.show_error("No project is available for pull requests");
            }
        });
        self.add_action("claude-model-preset", true, |workspace| {
            workspace.open_or_cycle_claude_presets();
        });
        self.add_action("increase-font-size", true, |workspace| {
            workspace.adjust_font_size(1);
        });
        self.add_action("decrease-font-size", true, |workspace| {
            workspace.adjust_font_size(-1);
        });

        self.add_action("copy", true, |workspace| {
            if let Some(terminal) = workspace.selected_terminal() {
                terminal.copy_clipboard_format(vte::Format::Text);
            }
        });
        self.add_action("paste", true, |workspace| {
            if let Some(terminal) = workspace.selected_terminal() {
                terminal.paste_clipboard();
            }
        });
        self.add_action("search", true, |workspace| {
            if workspace.selected_session_id().is_some() {
                workspace.search_popover.popup();
                workspace.search_entry.grab_focus();
            }
        });

        self.application
            .set_accels_for_action("win.copy", &["<Control><Shift>c"]);
        self.application
            .set_accels_for_action("win.paste", &["<Control><Shift>v"]);
        self.application
            .set_accels_for_action("win.search", &["<Control><Shift>f"]);
        self.apply_shortcuts();
    }

    pub(super) fn load_shortcuts(&self, preferences: ShortcutPreferences) {
        *self.shortcuts.borrow_mut() = preferences;
        self.apply_shortcuts();
        let preferences = self.shortcuts.borrow();
        for (action, entry) in self.shortcut_entries.iter() {
            entry.set_text(preferences.binding(*action));
        }
    }

    pub(super) fn set_shortcut(&self, update: ShortcutSetParams, pending: Option<PendingRequest>) {
        let Some(store) = self.store.borrow().clone() else {
            self.report_failure(
                pending,
                ErrorCode::InternalError,
                "Workspace is loading",
                "shortcut settings are not available yet".to_owned(),
            );
            return;
        };
        let mut preferences = self.shortcuts.borrow().clone();
        if update.reset {
            preferences.reset(update.action);
        } else {
            let accelerator = update
                .accelerator
                .expect("protocol requires accelerator unless resetting");
            let Some((key, modifiers)) = gtk::accelerator_parse(&accelerator) else {
                self.report_failure(
                    pending,
                    ErrorCode::InvalidParams,
                    "Shortcut is invalid",
                    "GTK could not parse the accelerator".to_owned(),
                );
                return;
            };
            let required = gtk::gdk::ModifierType::CONTROL_MASK
                | gtk::gdk::ModifierType::ALT_MASK
                | gtk::gdk::ModifierType::META_MASK
                | gtk::gdk::ModifierType::SUPER_MASK;
            if !gtk::accelerator_valid(key, modifiers) || !modifiers.intersects(required) {
                self.report_failure(
                    pending,
                    ErrorCode::InvalidParams,
                    "Shortcut is invalid",
                    "use a valid key chord containing Ctrl, Alt, or Meta".to_owned(),
                );
                return;
            }
            let accelerator = gtk::accelerator_name(key, modifiers).to_string();
            if preferences.bindings.iter().any(|(action, binding)| {
                *action != update.action
                    && gtk::accelerator_parse(binding)
                        .is_some_and(|candidate| candidate == (key, modifiers))
            }) {
                self.report_failure(
                    pending,
                    ErrorCode::InvalidParams,
                    "Shortcut is already assigned",
                    accelerator,
                );
                return;
            }
            preferences.set(update.action, accelerator);
        }
        let value = match serde_json::to_value(&preferences) {
            Ok(value) => value,
            Err(error) => {
                self.report_launch_failure(
                    pending,
                    "Could not encode shortcuts",
                    error.to_string(),
                );
                return;
            }
        };
        self.run_io(
            move || store.set_preference("shortcuts", &value),
            move |workspace, result| match result {
                Ok(()) => {
                    workspace.load_shortcuts(preferences);
                    workspace.shortcut_popover.popdown();
                    if let Some(pending) = pending {
                        let id = pending.request.id.clone();
                        match serde_json::to_value(workspace.shortcut_summaries()) {
                            Ok(shortcuts) => {
                                let _ = pending.respond(ControlResponse::success(
                                    id,
                                    serde_json::json!({ "shortcuts": shortcuts }),
                                ));
                            }
                            Err(error) => respond_failure(
                                pending,
                                ErrorCode::InternalError,
                                "Could not describe shortcuts",
                                Some(serde_json::json!({ "reason": error.to_string() })),
                            ),
                        }
                    }
                }
                Err(error) => workspace.report_launch_failure(
                    pending,
                    "Could not save shortcuts",
                    error.to_string(),
                ),
            },
        );
    }

    pub(super) fn shortcut_summaries(&self) -> Vec<ShortcutSummary> {
        let preferences = self.shortcuts.borrow();
        ShortcutAction::ALL
            .into_iter()
            .map(|action| ShortcutSummary {
                action,
                label: action.label().to_owned(),
                accelerator: preferences.binding(action).to_owned(),
                default_accelerator: action.default_accelerator().to_owned(),
                is_active: action.is_active(),
            })
            .collect()
    }

    fn add_action(&self, name: &'static str, enabled: bool, activate: impl Fn(&Self) + 'static) {
        let action = gio::SimpleAction::new(name, None);
        action.set_enabled(enabled);
        let workspace = self.clone();
        action.connect_activate(move |_, _| activate(&workspace));
        self.window.add_action(&action);
    }

    fn apply_shortcuts(&self) {
        let preferences = self.shortcuts.borrow();
        for action in ShortcutAction::ALL {
            self.application.set_accels_for_action(
                &format!("win.{}", action.as_str()),
                &[preferences.binding(action)],
            );
        }
    }

    fn selected_terminal(&self) -> Option<vte::Terminal> {
        let id = self.selected_session_id()?;
        self.sessions
            .borrow()
            .get(&id)
            .map(|session| session.terminal.clone())
    }

    fn select_relative_session(&self, offset: i32) {
        let rows = (0..)
            .map_while(|index| self.list.row_at_index(index))
            .collect::<Vec<_>>();
        if rows.is_empty() {
            return;
        }
        let selected = self.list.selected_row().map_or(0, |row| row.index());
        let index = (selected + offset).rem_euclid(rows.len() as i32) as usize;
        self.list.select_row(Some(&rows[index]));
    }

    fn select_next_ready_session(&self) {
        let sessions = self.sessions.borrow();
        let mut ready = sessions
            .values()
            .filter(|session| {
                matches!(
                    session.record.state,
                    SessionState::Ready | SessionState::Waiting
                )
            })
            .collect::<Vec<_>>();
        ready.sort_by_key(|session| session.record.position);
        if ready.is_empty() {
            self.show_error("No agent session is ready");
            return;
        }
        let selected = self.selected_session_id();
        let index = ready
            .iter()
            .position(|session| selected.as_deref() == Some(session.record.id.as_str()))
            .map_or(0, |index| (index + 1) % ready.len());
        let row = ready[index].row.clone();
        drop(sessions);
        self.list.select_row(Some(&row));
    }
}
