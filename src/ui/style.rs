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
        style.apply(palette, 13);
        style
    }

    pub(super) fn apply(&self, palette: ChromePalette, font_size: u8) {
        self.provider.load_from_string(&tui_css(palette, font_size));
    }
}

fn tui_css(palette: ChromePalette, font_size: u8) -> String {
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
        .replace("@font-size", &format!("{font_size}px"))
        .replace(
            "@small-font-size",
            &format!("{}px", font_size.saturating_sub(2).max(9)),
        )
}

const TUI_CSS: &str = r#"
.tui-root {
  background: @background;
  color: @text;
  font-family: monospace;
  font-size: @font-size;
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

.tui-brand { color: @text; font-weight: bold; letter-spacing: 0.3px; }
.tui-muted { color: @muted; }

.tui-brand-row {
  min-height: 34px;
  padding: 4px 8px;
  color: @text;
  background: @panel;
  border-bottom: 1px solid @line;
}

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
.tui-button image { -gtk-icon-size: 16px; }
.tui-danger-button {
  min-height: 24px;
  min-width: 0;
  padding: 2px 7px;
  color: @danger;
  background: transparent;
  border: 0;
  border-radius: 0;
  box-shadow: none;
}
.tui-danger-button:hover { color: @background; background: @danger; }

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
.tui-sidebar-actions {
  min-height: 26px;
  padding: 1px 4px;
  border-bottom: 1px solid @line;
}
.tui-sidebar-controls {
  min-height: 28px;
  padding: 2px 4px;
  border-top: 1px solid @line;
  background: @panel;
}
.tui-sidebar-controls > button {
  flex: 0 0 auto;
}
.tui-surface-anchors {
  min-width: 1px;
  min-height: 1px;
  max-height: 1px;
  opacity: 0;
}
.tui-surface-anchors > button {
  min-width: 1px;
  min-height: 1px;
  max-width: 1px;
  max-height: 1px;
  padding: 0;
}
.tui-session-list, .tui-session-list > row { background: transparent; }
.tui-session-row {
  margin: 2px 0;
  padding: 0;
  color: @text;
  border: 1px solid @line;
  border-left: 2px solid @line;
  border-radius: 6px;
  background: @background;
}
.tui-session-row:hover { background: @hover; }
.tui-session-row:selected {
  color: @text;
  background: @hover;
  border-color: @accent;
  border-left-color: @accent;
}
.tui-session-content { padding: 5px 7px; min-height: 42px; }
.tui-session-primary, .tui-session-secondary {
  min-width: 0;
}
.tui-session-primary { min-height: 18px; }
.tui-session-secondary { color: @muted; }
.tui-session-actions { margin-left: 4px; }
.tui-state { color: @ready; font-weight: bold; }
.tui-state-running { color: @accent; }
.tui-state-busy { color: #e5a50a; }
.tui-state-ready { color: @ready; }
.tui-state-waiting { color: #e5a50a; }
.tui-state-reconnecting { color: @muted; }
.tui-state-exited { color: @danger; }
.tui-elapsed {
  min-width: 28px;
  color: @ready;
  font-size: @small-font-size;
}
.tui-kind { color: @muted; font-size: @small-font-size; }
.tui-provider-icon { min-width: 16px; min-height: 16px; }

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
.tui-group-button image { -gtk-icon-size: 14px; margin-right: 4px; }
.tui-subgroup-row {
  padding: 3px 8px;
  color: @muted;
  background: @background;
  font-size: @small-font-size;
  border-bottom: 1px solid @line;
}
.tui-worktree-name { color: @text; font-weight: bold; }
.tui-worktree-path, .tui-session-worktree {
  color: @muted;
  font-size: @small-font-size;
}
.tui-worktree-row {
  padding: 5px 7px;
  color: @text;
  background: @background;
  border-top: 1px solid @line;
}
.tui-agent-row {
  min-height: 38px;
  padding: 4px 7px;
  color: @text;
  background: @background;
  border: 0;
  border-bottom: 1px solid @line;
  border-radius: 0;
  box-shadow: none;
}
.tui-agent-row:hover { color: @background; background: @accent; }
.tui-agent-buttons { border: 1px solid @line; }
.tui-agent-button {
  min-height: 25px;
  min-width: 0;
  padding: 2px 10px;
  color: @text;
  background: @background;
  border: 0;
  border-radius: 0;
  box-shadow: none;
}
.tui-agent-button:hover { color: @background; background: @accent; }
.tui-agent-button:checked { color: @background; background: @accent; }
.tui-agent-button:focus-visible { outline: 1px solid @accent; outline-offset: -1px; }
.tui-agent-message {
  padding: 5px 7px;
  border-bottom: 1px solid @line;
}

.tui-main-pane {
  padding: 8px;
  background: @background;
}
.tui-main { background: @background; }
.tui-empty { color: @muted; }
.tui-empty-title { color: @text; font-weight: bold; }

.tui-session-context {
  min-height: 36px;
  padding: 5px 8px;
  margin-bottom: 6px;
  color: @muted;
  background: @background;
  border: 1px solid @line;
  border-radius: 6px;
}
.tui-context-input {
  min-width: 0;
  padding: 2px 4px 2px 8px;
  background: @panel;
  border: 1px solid @line;
  border-radius: 6px;
}
.tui-context-caption { color: @muted; font-weight: bold; }
.tui-context-last-input {
  min-width: 0;
  padding: 3px 0;
  color: @text;
}
.tui-context-action {
  min-height: 25px;
  min-width: 0;
  padding: 2px 8px;
  color: @text;
  background: transparent;
  border: 1px solid @line;
  border-radius: 0;
  box-shadow: none;
}
.tui-context-action:hover { color: @background; background: @accent; }
.tui-context-action:disabled { color: @muted; }

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
  font-size: @font-size;
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
.tui-path-dropdown {
  min-height: 25px;
  min-width: 220px;
  color: @text;
  background: @background;
  border: 1px solid @line;
  border-radius: 0;
}
.tui-path-dropdown button {
  min-height: 25px;
  min-width: 0;
  padding: 2px 7px;
  color: @text;
  background: @background;
  border: 0;
  border-radius: 0;
  box-shadow: none;
}
.tui-path-dropdown button:hover {
  color: @background;
  background: @accent;
}

.tui-root scrollbar { background: @panel; }
.tui-root scrollbar slider {
  min-width: 5px;
  min-height: 5px;
  background: @muted;
  border-radius: 0;
}
"#;
