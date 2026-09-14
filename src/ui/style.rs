const TUI_CSS: &str = r#"
.tui-root {
  background: #0b0e14;
  color: #e7ecff;
  font-family: monospace;
  font-size: 13px;
}

.tui-topbar {
  min-height: 30px;
  padding: 0 6px;
  background: #101626;
  color: #e7ecff;
  border-bottom: 1px solid #30384e;
  border-radius: 0;
  box-shadow: none;
}

.tui-brand { color: #ffcc66; font-weight: bold; }
.tui-muted { color: #99a2c2; }

.tui-button {
  min-height: 24px;
  min-width: 0;
  padding: 2px 7px;
  color: #d9f0ff;
  background: transparent;
  border: 0;
  border-radius: 0;
  box-shadow: none;
}
.tui-button:hover { color: #0b0e14; background: #ffcc66; }
.tui-button:focus-visible { outline: 1px solid #ffcc66; outline-offset: -1px; }
.tui-button:disabled { color: #5c6685; }

.tui-sidebar {
  background: #101626;
  border-right: 1px solid #30384e;
}
.tui-sidebar-heading {
  padding: 7px 8px 5px 8px;
  color: #99a2c2;
  font-weight: bold;
  letter-spacing: 1px;
  border-bottom: 1px solid #30384e;
}
.tui-session-list, .tui-session-list > row { background: transparent; }
.tui-session-row {
  padding: 0;
  color: #e7ecff;
  border-radius: 0;
  border-bottom: 1px solid #20273a;
}
.tui-session-row:hover { background: #1a2440; }
.tui-session-row:selected { color: #0b0e14; background: #e7ecff; }
.tui-session-row:selected .tui-muted,
.tui-session-row:selected .tui-state { color: #38415c; }
.tui-session-content { padding: 4px 7px; min-height: 27px; }
.tui-state { color: #5fd18c; font-weight: bold; }
.tui-state-exited { color: #ff5f87; }
.tui-kind { color: #99a2c2; font-size: 11px; }

.tui-main { background: #0b0e14; }
.tui-empty { color: #99a2c2; }
.tui-empty-title { color: #e7ecff; font-weight: bold; }

.tui-statusbar {
  min-height: 23px;
  padding: 2px 7px;
  color: #99a2c2;
  background: #101626;
  border-top: 1px solid #30384e;
}
.tui-status-accent { color: #ffcc66; }

.tui-popover contents {
  color: #e7ecff;
  background: #101626;
  border: 1px solid #59627d;
  border-radius: 0;
  box-shadow: none;
}

.tui-root scrollbar { background: #101626; }
.tui-root scrollbar slider { min-width: 5px; min-height: 5px; background: #59627d; border-radius: 0; }
"#;

pub(super) fn install(display: &gtk::gdk::Display) {
    let provider = gtk::CssProvider::new();
    provider.connect_parsing_error(|_, section, error| {
        eprintln!("agmux CSS error at {section:?}: {error}");
    });
    // GTK documents application CSS providers as the supported way to style
    // widget classes across one display.
    // https://docs.gtk.org/gtk4/class.CssProvider.html
    provider.load_from_string(TUI_CSS);
    gtk::style_context_add_provider_for_display(
        display,
        &provider,
        gtk::STYLE_PROVIDER_PRIORITY_APPLICATION,
    );
}
