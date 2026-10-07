use super::*;
use crate::appearance::{
    MAX_UI_FONT_SIZE, MAX_VIEWER_FONT_SIZE, MIN_UI_FONT_SIZE, MIN_VIEWER_FONT_SIZE,
    clamp_ui_font_size, resolve_system_theme, theme,
};

#[derive(Clone, Copy, PartialEq, Eq)]
enum FontSurface {
    Ui,
    Terminal,
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
        if let Some(viewer) = self.code_viewer.borrow().as_ref() {
            let (theme, font_family, font_size) = self.viewer_appearance();
            viewer.set_appearance(theme, &font_family, font_size);
        }
    }

    pub(super) fn apply_current_terminal_appearance(&self, terminal: &vte::Terminal) {
        let preferences = self.appearance.borrow();
        apply_terminal_appearance(terminal, self.effective_terminal_theme(), &preferences.font);
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
        if font.is_none() && ui_font_size.is_none() && viewer_font_size.is_none() {
            return;
        }
        self.set_appearance(
            AppearanceSetParams {
                font,
                ui_font_size,
                viewer_font_size,
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
                FontSurface::Terminal
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
