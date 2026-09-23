use super::*;

const FIXED_SHORTCUTS: [(&str, &str); 3] = [
    ("copy", "<Control><Shift>c"),
    ("paste", "<Control><Shift>v"),
    ("search", "<Control><Shift>f"),
];
const SHORTCUT_MODIFIERS: gtk::gdk::ModifierType = gtk::gdk::ModifierType::SHIFT_MASK
    .union(gtk::gdk::ModifierType::CONTROL_MASK)
    .union(gtk::gdk::ModifierType::ALT_MASK)
    .union(gtk::gdk::ModifierType::META_MASK)
    .union(gtk::gdk::ModifierType::SUPER_MASK);
const REQUIRED_MODIFIERS: gtk::gdk::ModifierType = gtk::gdk::ModifierType::CONTROL_MASK
    .union(gtk::gdk::ModifierType::ALT_MASK)
    .union(gtk::gdk::ModifierType::META_MASK)
    .union(gtk::gdk::ModifierType::SUPER_MASK);

fn is_image_mime_type(mime_type: &str) -> bool {
    mime_type
        .split_once(';')
        .map_or(mime_type, |(mime_type, _)| mime_type)
        .trim()
        .starts_with("image/")
}

/// Shows an accelerator the way people write it, such as Shift+Ctrl+Q.
pub(super) fn accelerator_label(accelerator: &str) -> String {
    gtk::accelerator_parse(accelerator).map_or_else(
        || accelerator.to_owned(),
        |(key, modifiers)| gtk::accelerator_get_label(key, modifiers).to_string(),
    )
}

impl Workspace {
    pub(super) fn install_shortcut_actions(&self) {
        self.add_action("new-shell", true, |workspace| workspace.launch_shell());
        self.add_action("close-session", true, |workspace| {
            if let Some(id) = workspace.selected_session_id() {
                workspace.stop_session(&id, None);
            }
        });
        self.add_action("toggle-sidebar", true, |workspace| {
            workspace.sidebar_toggle.emit_clicked();
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
                workspace.prs.root.set_text(&root);
                workspace.prs.modal.present();
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
                if clipboard_has_image(&terminal) {
                    // VTE's clipboard action only requests text. Let agent TUIs
                    // handle image paste through their native clipboard reader.
                    terminal.feed_child(b"\x16");
                } else {
                    terminal.paste_clipboard();
                }
            }
        });
        self.add_action("search", true, |workspace| {
            if workspace.selected_session_id().is_some() {
                workspace.open_search();
            }
        });

        self.apply_shortcuts();
        let workspace = self.clone();
        self.preferences
            .modal
            .connect_hide(move || workspace.apply_shortcuts());
    }

    pub(super) fn load_shortcuts(&self, preferences: ShortcutPreferences) {
        *self.shortcuts.borrow_mut() = preferences;
        self.apply_shortcuts();
        let preferences = self.shortcuts.borrow();
        for (action, entry) in self.preferences.shortcut_entries.iter() {
            entry.set_text(&accelerator_label(preferences.binding(*action)));
        }
    }

    pub(super) fn install_terminal_shortcuts(&self, terminal: &vte::Terminal) {
        let controller = gtk::EventControllerKey::new();
        controller.set_propagation_phase(gtk::PropagationPhase::Capture);
        let workspace = self.clone();
        controller.connect_key_pressed(move |controller, _, _, _| {
            let Some(event) = controller
                .current_event()
                .and_then(|event| event.downcast::<gtk::gdk::KeyEvent>().ok())
            else {
                return glib::Propagation::Proceed;
            };
            let Some(action) = workspace.shortcut_action_for_event(&event) else {
                return glib::Propagation::Proceed;
            };
            let action = format!("win.{action}");
            if gtk::prelude::WidgetExt::activate_action(&workspace.window, &action, None).is_ok() {
                glib::Propagation::Stop
            } else {
                glib::Propagation::Proceed
            }
        });
        terminal.add_controller(controller);
    }

    /// Records a chord pressed in a shortcut field and saves it right away.
    pub(super) fn install_shortcut_capture(&self, action: ShortcutAction, entry: &gtk::Entry) {
        let keys = gtk::EventControllerKey::new();
        keys.set_propagation_phase(gtk::PropagationPhase::Capture);
        let workspace = self.clone();
        keys.connect_key_pressed(move |_, key, _, state| {
            let modifiers = state & SHORTCUT_MODIFIERS;
            if !modifiers.intersects(REQUIRED_MODIFIERS) || !gtk::accelerator_valid(key, modifiers)
            {
                return glib::Propagation::Proceed;
            }
            workspace.set_shortcut(
                ShortcutSetParams {
                    action,
                    accelerator: Some(gtk::accelerator_name(key, modifiers).to_string()),
                    reset: false,
                },
                None,
            );
            glib::Propagation::Stop
        });
        entry.add_controller(keys);
        // While recording, a chord that is already bound must not run its action.
        let focus = gtk::EventControllerFocus::new();
        let workspace = self.clone();
        focus.connect_enter(move |_| workspace.suspend_shortcuts());
        let workspace = self.clone();
        focus.connect_leave(move |_| workspace.apply_shortcuts());
        entry.add_controller(focus);
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
        for (action, accelerator) in FIXED_SHORTCUTS {
            self.application
                .set_accels_for_action(&format!("win.{action}"), &[accelerator]);
        }
    }

    fn suspend_shortcuts(&self) {
        let actions = ShortcutAction::ALL
            .into_iter()
            .map(ShortcutAction::as_str)
            .chain(FIXED_SHORTCUTS.map(|(action, _)| action));
        for action in actions {
            self.application
                .set_accels_for_action(&format!("win.{action}"), &[]);
        }
    }

    /// GDK's matching treats Shift+] and } alike, which a raw key compare would not.
    fn shortcut_action_for_event(&self, event: &gtk::gdk::KeyEvent) -> Option<&'static str> {
        let matches = |accelerator: &str| {
            gtk::accelerator_parse(accelerator).is_some_and(|(key, modifiers)| {
                event.matches(key, modifiers) == gtk::gdk::KeyMatch::Exact
            })
        };
        let preferences = self.shortcuts.borrow();
        ShortcutAction::ALL
            .into_iter()
            .find(|action| matches(preferences.binding(*action)))
            .map(ShortcutAction::as_str)
            .or_else(|| {
                FIXED_SHORTCUTS
                    .into_iter()
                    .find(|(_, accelerator)| matches(accelerator))
                    .map(|(action, _)| action)
            })
    }

    fn selected_terminal(&self) -> Option<vte::Terminal> {
        let id = self.selected_session_id()?;
        self.sessions
            .borrow()
            .get(&id)
            .map(|session| session.terminal.clone())
    }

    fn select_relative_session(&self, offset: i32) {
        // Sidebar order across projects; headers and collapsed projects are skipped.
        let rows = (0..)
            .map_while(|index| self.list.row_at_index(index))
            .filter(|row| row.is_selectable() && row.is_visible())
            .collect::<Vec<_>>();
        if rows.is_empty() {
            return;
        }
        let selected = self
            .list
            .selected_row()
            .and_then(|selected| rows.iter().position(|row| *row == selected));
        let index = match selected {
            Some(index) => (index as i32 + offset).rem_euclid(rows.len() as i32) as usize,
            // The selection is hidden in a collapsed project: start from an end.
            None if offset > 0 => 0,
            None => rows.len() - 1,
        };
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

fn clipboard_has_image(terminal: &vte::Terminal) -> bool {
    terminal
        .clipboard()
        .formats()
        .mime_types()
        .iter()
        .any(|mime_type| is_image_mime_type(mime_type.as_str()))
}

#[cfg(test)]
mod tests {
    use super::is_image_mime_type;

    #[test]
    fn image_mime_types_are_detected_for_clipboard_paste() {
        assert!(is_image_mime_type("image/png"));
        assert!(is_image_mime_type("image/jpeg; charset=binary"));
        assert!(!is_image_mime_type("text/plain"));
        assert!(!is_image_mime_type("application/octet-stream"));
    }
}
