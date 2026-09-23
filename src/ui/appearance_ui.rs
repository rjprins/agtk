use super::*;
use crate::appearance::{
    MAX_UI_FONT_SIZE, MIN_UI_FONT_SIZE, clamp_ui_font_size, resolve_system_theme, theme,
};

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
        if update
            .ui_font_size
            .is_some_and(|size| !(MIN_UI_FONT_SIZE..=MAX_UI_FONT_SIZE).contains(&size))
        {
            self.report_failure(
                pending,
                ErrorCode::InvalidParams,
                "UI font size is invalid",
                "use a UI font size between 9 and 24".to_owned(),
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
        if let Some(ui_font_size) = update.ui_font_size {
            preferences.ui_font_size = clamp_ui_font_size(ui_font_size);
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
            ui_font_size: preferences.ui_font_size,
            available_themes: ThemeKey::ALL.to_vec(),
        }
    }

    pub(super) fn apply_appearance(&self) {
        let preferences = self.appearance.borrow().clone();
        self.chrome_style.apply(preferences.ui_font_size);
        let effective = self.effective_terminal_theme();
        self.preferences.show_appearance(&preferences);
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

    pub(super) fn adjust_font_size(&self, delta: i8) {
        let preferences = self.appearance.borrow().clone();
        let current = i16::from(preferences.ui_font_size);
        let next = (current + i16::from(delta))
            .clamp(i16::from(MIN_UI_FONT_SIZE), i16::from(MAX_UI_FONT_SIZE))
            as u8;
        if next == preferences.ui_font_size {
            return;
        }
        self.set_appearance(
            AppearanceSetParams {
                theme: None,
                follow_system: None,
                font: Some(adjust_terminal_font_size(&preferences.font, delta)),
                ui_font_size: Some(next),
            },
            None,
        );
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

fn adjust_terminal_font_size(font: &str, delta: i8) -> String {
    let mut description = FontDescription::from_string(font);
    let current = description.size();
    let current_points = if current > 0 {
        (current as f64 / gtk::pango::SCALE as f64).round() as i16
    } else {
        11
    };
    let next_points = (current_points + i16::from(delta)).clamp(6, 32);
    description.set_size(i32::from(next_points) * gtk::pango::SCALE);
    description.to_string()
}

fn parse_color(value: &str) -> gtk::gdk::RGBA {
    gtk::gdk::RGBA::parse(value).expect("built-in theme contains a valid GDK color")
}
