use crate::appearance::ChromePalette;

#[derive(Clone)]
pub(super) struct ChromeStyle {
    provider: gtk::CssProvider,
}

impl ChromeStyle {
    pub(super) fn install(display: &gtk::gdk::Display, palette: ChromePalette) -> Self {
        let provider = gtk::CssProvider::new();
        provider.connect_parsing_error(|_, section, error| {
            eprintln!("agmux CSS error at {section:?}: {error}");
        });
        // GTK documents an application-level provider as the supported way to
        // style app-owned widget classes across one display.
        // https://docs.gtk.org/gtk4/class.CssProvider.html
        gtk::style_context_add_provider_for_display(
            display,
            &provider,
            gtk::STYLE_PROVIDER_PRIORITY_APPLICATION,
        );
        let style = Self { provider };
        style.apply(palette);
        style
    }

    pub(super) fn apply(&self, palette: ChromePalette) {
        self.provider.load_from_string(&tui_css(palette));
    }
}

fn tui_css(palette: ChromePalette) -> String {
    TUI_CSS
        .replace("@background", palette.background)
        .replace("@panel", palette.panel)
        .replace("@text", palette.text)
        .replace("@muted", palette.muted)
        .replace("@accent", palette.accent)
        .replace("@danger", palette.danger)
        .replace("@line", palette.line)
        .replace("@hover", palette.hover)
        .replace("@ready", palette.ready)
}

const TUI_CSS: &str = r#"
.tui-root {
  background: @background;
  color: @text;
  font-family: monospace;
  font-size: 13px;
}

.tui-topbar {
  min-height: 30px;
  padding: 0 6px;
  background: @panel;
  color: @text;
  border-bottom: 1px solid @line;
  border-radius: 0;
  box-shadow: none;
}

.tui-brand { color: @accent; font-weight: bold; }
.tui-muted { color: @muted; }

.tui-button {
  min-height: 24px;
  min-width: 0;
  padding: 2px 7px;
  color: @text;
  background: transparent;
  border: 0;
  border-radius: 0;
  box-shadow: none;
}
.tui-button:hover { color: @background; background: @accent; }
.tui-button:focus-visible { outline: 1px solid @accent; outline-offset: -1px; }
.tui-button:disabled { color: @muted; }

.tui-sidebar {
  background: @panel;
  border-right: 1px solid @line;
}
.tui-sidebar-heading {
  padding: 7px 8px 5px 8px;
  color: @muted;
  font-weight: bold;
  letter-spacing: 1px;
  border-bottom: 1px solid @line;
}
.tui-session-list, .tui-session-list > row { background: transparent; }
.tui-session-row {
  padding: 0;
  color: @text;
  border-radius: 0;
  border-bottom: 1px solid @line;
}
.tui-session-row:hover { background: @hover; }
.tui-session-row:selected { color: @background; background: @text; }
.tui-session-row:selected .tui-muted,
.tui-session-row:selected .tui-state,
.tui-session-row:selected .tui-kind { color: @panel; }
.tui-session-content { padding: 4px 7px; min-height: 27px; }
.tui-state { color: @ready; font-weight: bold; }
.tui-state-exited { color: @danger; }
.tui-kind { color: @muted; font-size: 11px; }

.tui-group-row { background: @panel; border-top: 1px solid @line; }
.tui-group-button {
  min-height: 27px;
  padding: 3px 7px;
  color: @text;
  background: transparent;
  border: 0;
  border-radius: 0;
  box-shadow: none;
  font-weight: bold;
}
.tui-group-button:hover { color: @background; background: @accent; }
.tui-subgroup-row {
  padding: 3px 8px;
  color: @muted;
  background: @background;
  font-size: 11px;
  border-bottom: 1px solid @line;
}

.tui-main { background: @background; }
.tui-empty { color: @muted; }
.tui-empty-title { color: @text; font-weight: bold; }

.tui-statusbar {
  min-height: 23px;
  padding: 2px 7px;
  color: @muted;
  background: @panel;
  border-top: 1px solid @line;
}
.tui-status-accent { color: @accent; }

.tui-popover contents {
  color: @text;
  background: @panel;
  border: 1px solid @line;
  border-radius: 0;
  box-shadow: none;
}

.tui-surface {
  color: @text;
  background: @panel;
  border: 1px solid @line;
  font-family: monospace;
  font-size: 13px;
}

.tui-setting-row { padding: 2px 7px; }
.tui-setting-entry {
  min-height: 25px;
  color: @text;
  background: @background;
  border: 1px solid @line;
  border-radius: 0;
  box-shadow: none;
}

.tui-root scrollbar { background: @panel; }
.tui-root scrollbar slider {
  min-width: 5px;
  min-height: 5px;
  background: @muted;
  border-radius: 0;
}
"#;
