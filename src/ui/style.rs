use crate::appearance::DEFAULT_UI_FONT_SIZE;

#[derive(Clone)]
pub(super) struct ChromeStyle {
    provider: gtk::CssProvider,
}

impl ChromeStyle {
    /// libadwaita owns the chrome; this only adds UI scaling and the session-state hints.
    pub(super) fn install(display: &gtk::gdk::Display) -> Self {
        let provider = gtk::CssProvider::new();
        provider.connect_parsing_error(|_, section, error| {
            eprintln!("agmux CSS error at {section:?}: {error}");
        });
        gtk::style_context_add_provider_for_display(
            display,
            &provider,
            gtk::STYLE_PROVIDER_PRIORITY_APPLICATION,
        );
        let style = Self { provider };
        style.apply(DEFAULT_UI_FONT_SIZE);
        style
    }

    /// Ctrl+plus and Ctrl+minus scale the system font rather than replacing it.
    pub(super) fn apply(&self, font_size: u8) {
        let percent = u32::from(font_size) * 100 / u32::from(DEFAULT_UI_FONT_SIZE);
        let icon = 16 * percent / 100;
        self.provider.load_from_string(&format!(
            "window {{ font-size: {percent}%; }}\n.provider-icon {{ -gtk-icon-size: {icon}px; }}\n{CHROME_CSS}"
        ));
    }
}

const CHROME_CSS: &str = r#"
.navigation-sidebar row.session-ready:not(:selected) {
  background: color-mix(in srgb, var(--success-color) 12%, transparent);
}
.navigation-sidebar row.session-waiting:not(:selected) {
  background: color-mix(in srgb, var(--warning-color) 14%, transparent);
}
.session-state { min-width: 12px; font-weight: bold; }
vte-terminal { padding: 6px 10px; }
.session-state.state-busy { color: var(--warning-color); }
.session-state.state-ready { color: var(--success-color); }
.session-state.state-waiting { color: var(--warning-color); }
.session-state.state-exited { color: var(--error-color); }
.session-state.state-running { color: var(--accent-color); }
"#;
