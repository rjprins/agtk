use super::*;
use crate::appearance::{resolve_system_theme, theme};

impl Workspace {
    pub(super) fn load_appearance(&self, preferences: AppearancePreferences) {
        *self.appearance.borrow_mut() = preferences;
        self.apply_appearance();
    }

    pub(super) fn set_appearance(
        &self,
        update: AppearanceSetParams,
        pending: Option<PendingRequest>,
    ) {
        let Some(store) = self.store.borrow().clone() else {
            self.report_failure(
                pending,
                ErrorCode::InternalError,
                "Workspace is loading",
                "appearance settings are not available yet".to_owned(),
            );
            return;
        };
        if update.font.as_ref().is_some_and(|font| {
            font.trim() != font || !(1..=120).contains(&font.chars().count()) || font.contains('\0')
        }) {
            self.report_failure(
                pending,
                ErrorCode::InvalidParams,
                "Terminal font is invalid",
                "use a trimmed Pango font description between 1 and 120 characters".to_owned(),
            );
            return;
        }

        let mut preferences = self.appearance.borrow().clone();
        if let Some(theme) = update.theme {
            preferences.theme = theme;
        }
        if let Some(follow_system) = update.follow_system {
            preferences.follow_system = follow_system;
        }
        if let Some(font) = update.font {
            preferences.font = font;
        }
        let value = match serde_json::to_value(&preferences) {
            Ok(value) => value,
            Err(error) => {
                self.report_launch_failure(
                    pending,
                    "Could not encode appearance settings",
                    error.to_string(),
                );
                return;
            }
        };
        self.run_io(
            move || store.set_preference("appearance", &value),
            move |workspace, result| match result {
                Ok(()) => {
                    workspace.load_appearance(preferences);
                    workspace.theme_popover.popdown();
                    if let Some(pending) = pending {
                        let id = pending.request.id.clone();
                        match serde_json::to_value(workspace.appearance_summary()) {
                            Ok(summary) => {
                                let _ = pending.respond(ControlResponse::success(id, summary));
                            }
                            Err(error) => respond_failure(
                                pending,
                                ErrorCode::InternalError,
                                "Could not describe appearance settings",
                                Some(serde_json::json!({ "reason": error.to_string() })),
                            ),
                        }
                    }
                }
                Err(error) => workspace.report_launch_failure(
                    pending,
                    "Could not save appearance settings",
                    error.to_string(),
                ),
            },
        );
    }

    pub(super) fn appearance_summary(&self) -> AppearanceSummary {
        let preferences = self.appearance.borrow();
        AppearanceSummary {
            theme: preferences.theme,
            effective_theme: self.effective_terminal_theme(),
            follow_system: preferences.follow_system,
            font: preferences.font.clone(),
            available_themes: ThemeKey::ALL.to_vec(),
        }
    }

    pub(super) fn apply_appearance(&self) {
        let chrome_key = if self.style_manager.is_dark() {
            ThemeKey::Neutral
        } else {
            ThemeKey::NeutralLight
        };
        self.chrome_style.apply(theme(chrome_key).chrome);

        let preferences = self.appearance.borrow().clone();
        let effective = self.effective_terminal_theme();
        self.theme_button
            .set_label(&format!("[theme: {}]", effective.as_str()));
        if self.follow_system_toggle.is_active() != preferences.follow_system {
            self.follow_system_toggle
                .set_active(preferences.follow_system);
        }
        if self.font_entry.text().as_str() != preferences.font {
            self.font_entry.set_text(&preferences.font);
        }
        let terminals = self
            .sessions
            .borrow()
            .values()
            .map(|session| session.terminal.clone())
            .collect::<Vec<_>>();
        for terminal in terminals {
            apply_terminal_appearance(&terminal, effective, &preferences.font);
        }
    }

    pub(super) fn apply_current_terminal_appearance(&self, terminal: &vte::Terminal) {
        let preferences = self.appearance.borrow();
        apply_terminal_appearance(terminal, self.effective_terminal_theme(), &preferences.font);
    }

    fn effective_terminal_theme(&self) -> ThemeKey {
        let preferences = self.appearance.borrow();
        if preferences.follow_system {
            resolve_system_theme(preferences.theme, self.style_manager.is_dark())
        } else {
            preferences.theme
        }
    }
}

fn apply_terminal_appearance(terminal: &vte::Terminal, key: ThemeKey, font: &str) {
    let palette = theme(key).terminal;
    let foreground = parse_color(palette.foreground);
    let background = parse_color(palette.background);
    let cursor = parse_color(palette.cursor);
    let selection = parse_color(palette.selection);
    let ansi = palette.ansi.map(parse_color);
    let ansi_refs = ansi.iter().collect::<Vec<_>>();

    // VTE applies the foreground, background, and all sixteen ANSI colors as
    // one palette update. Cursor and selection colors are explicit properties.
    // https://gnome.pages.gitlab.gnome.org/vte/gtk4/method.Terminal.set_colors.html
    terminal.set_colors(Some(&foreground), Some(&background), &ansi_refs);
    terminal.set_color_cursor(Some(&cursor));
    terminal.set_color_highlight(Some(&selection));
    terminal.set_color_highlight_foreground(Some(&foreground));
    terminal.set_font(Some(&FontDescription::from_string(font)));
}

fn parse_color(value: &str) -> gtk::gdk::RGBA {
    gtk::gdk::RGBA::parse(value).expect("built-in theme contains a valid GDK color")
}
