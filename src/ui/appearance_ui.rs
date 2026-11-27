use super::*;
use crate::appearance::{
    MAX_TERMINAL_FONT_SIZE, MAX_UI_FONT_SIZE, MAX_VIEWER_FONT_SIZE, MIN_TERMINAL_FONT_SIZE,
    MIN_UI_FONT_SIZE, MIN_VIEWER_FONT_SIZE, clamp_ui_font_size, resolve_system_theme, theme,
};

#[derive(Clone, Copy, PartialEq, Eq)]
enum FontSurface {
    Ui,
    Terminal,
    /// The terminal in the pinned session's panel, which has a size of its own.
    Pinned,
    Viewer,
    All,
}

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

        if update
            .viewer_font_size
            .is_some_and(|size| !(MIN_VIEWER_FONT_SIZE..=MAX_VIEWER_FONT_SIZE).contains(&size))
        {
            self.report_failure(
                pending,
                ErrorCode::InvalidParams,
                "Viewer font size is invalid",
                "use a viewer font size between 8 and 48".to_owned(),
            );
            return;
        }
        if update
            .pinned_font_size
            .is_some_and(|size| !(MIN_TERMINAL_FONT_SIZE..=MAX_TERMINAL_FONT_SIZE).contains(&size))
        {
            self.report_failure(
                pending,
                ErrorCode::InvalidParams,
                "Pinned font size is invalid",
                "use a pinned font size between 6 and 32".to_owned(),
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
        if let Some(size) = update.viewer_font_size {
            preferences.viewer_font_size = size;
        }
        if let Some(size) = update.pinned_font_size {
            preferences.pinned_font_size = Some(size);
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
        // Scroll gestures can deliver several steps before the first write finishes.
        // Apply now so every step builds on the latest sizes, and never replay an
        // older snapshot from a save callback. The IO worker keeps writes ordered.
        self.load_appearance(preferences);
        self.run_io(
            move || store.set_preference("appearance", &value),
            move |workspace, result| match result {
                Ok(()) => {
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
            viewer_font_size: preferences.viewer_font_size,
            pinned_font_size: preferences.pinned_font_size,
            available_themes: ThemeKey::ALL.to_vec(),
        }
    }

    pub(super) fn apply_appearance(&self) {
        let preferences = self.appearance.borrow().clone();
        self.chrome_style.apply(preferences.ui_font_size);
        let effective = self.effective_terminal_theme();
        self.preferences.show_appearance(&preferences);
        let pinned = self.pinned_session_id();
        let pinned_font = pinned_terminal_font(&preferences);
        let terminals = self
            .sessions
            .borrow()
            .iter()
            .map(|(id, session)| (pinned.as_ref() == Some(id), session.terminal.clone()))
            .collect::<Vec<_>>();
        for (is_pinned, terminal) in terminals {
            let font = if is_pinned {
                &pinned_font
            } else {
                &preferences.font
            };
            apply_terminal_appearance(&terminal, effective, font);
        }
        if let Some(viewer) = self.code_viewer.borrow().as_ref() {
            let (theme, font_family, font_size) = self.viewer_appearance();
            viewer.set_appearance(theme, &font_family, font_size);
        }
    }

    pub(super) fn apply_current_terminal_appearance(&self, terminal: &vte::Terminal) {
        let preferences = self.appearance.borrow();
        apply_terminal_appearance(terminal, self.effective_terminal_theme(), &preferences.font);
    }

    /// Gives a session's terminal the font of the place it is in now: the
    /// pinned session's panel or the main area.
    pub(super) fn apply_session_font(&self, id: &str) {
        let Some(terminal) = self
            .sessions
            .borrow()
            .get(id)
            .map(|session| session.terminal.clone())
        else {
            return;
        };
        let font = {
            let preferences = self.appearance.borrow();
            if self.is_pinned(id) {
                pinned_terminal_font(&preferences)
            } else {
                preferences.font.clone()
            }
        };
        terminal.set_font(Some(&FontDescription::from_string(&font)));
    }

    pub(super) fn adjust_font_size(&self, delta: i8) {
        self.adjust_surface_font_size(FontSurface::All, delta);
    }

    fn adjust_surface_font_size(&self, surface: FontSurface, delta: i8) {
        let preferences = self.appearance.borrow().clone();
        let current = i16::from(preferences.ui_font_size);
        let next = (current + i16::from(delta))
            .clamp(i16::from(MIN_UI_FONT_SIZE), i16::from(MAX_UI_FONT_SIZE))
            as u8;
        let font = matches!(surface, FontSurface::Terminal | FontSurface::All)
            .then(|| adjust_terminal_font_size(&preferences.font, delta))
            .filter(|font| *font != preferences.font);
        let ui_font_size = (matches!(surface, FontSurface::Ui | FontSurface::All)
            && next != preferences.ui_font_size)
            .then_some(next);
        let next_viewer = (i16::from(preferences.viewer_font_size) + i16::from(delta)).clamp(
            i16::from(MIN_VIEWER_FONT_SIZE),
            i16::from(MAX_VIEWER_FONT_SIZE),
        ) as u8;
        let viewer_font_size = (matches!(surface, FontSurface::Viewer | FontSurface::All)
            && next_viewer != preferences.viewer_font_size)
            .then_some(next_viewer);
        // Changing every size also moves the panel's own size, once it has one.
        let pinned_base = match surface {
            FontSurface::Pinned => Some(
                preferences
                    .pinned_font_size
                    .unwrap_or_else(|| terminal_font_points(&preferences.font)),
            ),
            FontSurface::All => preferences.pinned_font_size,
            _ => None,
        };
        let pinned_font_size = pinned_base
            .map(|size| {
                (i16::from(size) + i16::from(delta)).clamp(
                    i16::from(MIN_TERMINAL_FONT_SIZE),
                    i16::from(MAX_TERMINAL_FONT_SIZE),
                ) as u8
            })
            .filter(|size| Some(*size) != preferences.pinned_font_size);
        if font.is_none()
            && ui_font_size.is_none()
            && viewer_font_size.is_none()
            && pinned_font_size.is_none()
        {
            return;
        }
        self.set_appearance(
            AppearanceSetParams {
                font,
                ui_font_size,
                viewer_font_size,
                pinned_font_size,
                ..Default::default()
            },
            None,
        );
    }

    pub(super) fn install_font_scroll(&self) {
        for window in self.application.windows() {
            self.install_window_font_scroll(&window);
        }
        let workspace = self.clone();
        self.application.connect_window_added(move |_, window| {
            workspace.install_window_font_scroll(window);
        });
    }

    fn install_window_font_scroll(&self, window: &gtk::Window) {
        // Wayland scroll events carry no position. Track motion in window
        // coordinates, then pick the current widget (independent of focus).
        let position = Rc::new(Cell::new(None));
        let motion = gtk::EventControllerMotion::new();
        motion.set_propagation_phase(gtk::PropagationPhase::Capture);
        let pointer = position.clone();
        motion.connect_enter(move |_, x, y| pointer.set(Some((x, y))));
        let pointer = position.clone();
        motion.connect_motion(move |_, x, y| pointer.set(Some((x, y))));
        let pointer = position.clone();
        motion.connect_leave(move |_| pointer.set(None));
        window.add_controller(motion);

        // GtkEventControllerScroll propagates stop frames even when its signal
        // handler returns Stop. Those frames can contain the final touchpad
        // deltas, so handle raw events to keep them away from VTE and WebKit.
        let scroll = gtk::EventControllerLegacy::new();
        scroll.set_propagation_phase(gtk::PropagationPhase::Capture);
        let pending = Cell::new(0.0_f64);
        let previous_surface = Cell::new(FontSurface::Ui);
        let workspace = self.clone();
        scroll.connect_event(move |controller, event| {
            if !controller
                .current_event_state()
                .contains(gtk::gdk::ModifierType::CONTROL_MASK)
            {
                pending.set(0.0);
                return glib::Propagation::Proceed;
            }
            let Some(event) = event.downcast_ref::<gtk::gdk::ScrollEvent>() else {
                return glib::Propagation::Proceed;
            };
            let (_, dy) = event.deltas();
            let Some(widget) = controller.widget().and_then(|window| {
                let (x, y) = position.get()?;
                window.pick(x, y, gtk::PickFlags::DEFAULT)
            }) else {
                return glib::Propagation::Proceed;
            };
            let surface = if widget.ancestor(vte::Terminal::static_type()).is_some() {
                if widget.is_ancestor(&workspace.changes.pinned.host) {
                    FontSurface::Pinned
                } else {
                    FontSurface::Terminal
                }
            } else if widget.ancestor(webkit6::WebView::static_type()).is_some() {
                FontSurface::Viewer
            } else {
                FontSurface::Ui
            };
            if previous_surface.replace(surface) != surface || pending.get() * dy < 0.0 {
                pending.set(0.0);
            }
            // Accumulate touchpad pixels and high-resolution wheel fractions;
            // consume even partial steps so Ctrl+Scroll never reaches the PTY.
            let step = if event.unit() == gtk::gdk::ScrollUnit::Surface {
                50.0
            } else {
                1.0
            };
            let accumulated = pending.get() + dy / step;
            let steps = accumulated.trunc() as i8;
            pending.set(accumulated.fract());
            if steps != 0 {
                workspace.adjust_surface_font_size(surface, steps.saturating_neg());
            }
            if event.is_stop() {
                pending.set(0.0);
            }
            glib::Propagation::Stop
        });
        window.add_controller(scroll);
    }

    pub(super) fn effective_terminal_theme(&self) -> ThemeKey {
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
    let next_points = (i16::from(terminal_font_points(font)) + i16::from(delta)).clamp(
        i16::from(MIN_TERMINAL_FONT_SIZE),
        i16::from(MAX_TERMINAL_FONT_SIZE),
    );
    terminal_font_at(font, next_points as u8)
}

/// The point size a terminal font names, or 11 when it names none.
fn terminal_font_points(font: &str) -> u8 {
    let size = FontDescription::from_string(font).size();
    if size > 0 {
        (f64::from(size) / f64::from(gtk::pango::SCALE))
            .round()
            .clamp(1.0, 255.0) as u8
    } else {
        11
    }
}

fn terminal_font_at(font: &str, points: u8) -> String {
    let mut description = FontDescription::from_string(font);
    description.set_size(i32::from(points) * gtk::pango::SCALE);
    description.to_string()
}

/// The font of the pinned session's panel: the terminal font at the panel's own size.
fn pinned_terminal_font(preferences: &AppearancePreferences) -> String {
    match preferences.pinned_font_size {
        Some(points) => terminal_font_at(&preferences.font, points),
        None => preferences.font.clone(),
    }
}

fn parse_color(value: &str) -> gtk::gdk::RGBA {
    gtk::gdk::RGBA::parse(value).expect("built-in theme contains a valid GDK color")
}
