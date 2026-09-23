use super::*;

impl Workspace {
    pub(super) fn record_input(&self, id: &str, input: String) {
        let mut sessions = self.sessions.borrow_mut();
        let Some(session) = sessions.get_mut(id) else {
            return;
        };
        if session.history.last() != Some(&input) {
            session.history.push(input);
            if session.history.len() > 200 {
                session.history.remove(0);
            }
        }
        drop(sessions);

        self.mark_agent_busy(id);

        if self.selected_session_id().as_deref() == Some(id) {
            self.render_history(Some(id));
        }
    }

    pub(super) fn render_history(&self, id: Option<&str>) {
        while let Some(child) = self.history_list.first_child() {
            self.history_list.remove(&child);
        }

        let Some(id) = id else {
            self.history_button.set_sensitive(false);
            self.history_button.set_label("History (0)");
            self.context_last_input.set_text("(none yet)");
            return;
        };
        let sessions = self.sessions.borrow();
        let Some(session) = sessions.get(id) else {
            self.history_button.set_sensitive(false);
            self.history_button.set_label("History (0)");
            self.context_last_input.set_text("(none yet)");
            return;
        };
        self.history_button
            .set_sensitive(!session.history.is_empty());
        self.history_button
            .set_label(&format!("History ({})", session.history.len()));
        self.context_last_input.set_text(
            session
                .history
                .last()
                .map(String::as_str)
                .unwrap_or("(none yet)"),
        );
        for input in session.history.iter().rev() {
            let row = adw::ActionRow::builder()
                .title(glib::markup_escape_text(input))
                .title_lines(2)
                .activatable(true)
                .build();
            row.set_tooltip_text(Some("Scroll the terminal to this prompt"));
            row.add_suffix(&gtk::Image::from_icon_name("go-next-symbolic"));

            let terminal = session.terminal.clone();
            let window = self.history_window.clone();
            let overlay = self.overlay.clone();
            let input = input.clone();
            row.connect_activated(move |_| {
                let needle = history_needle(&input, 60);
                let Ok(regex) = vte::Regex::for_search(&needle, PCRE2_UTF | PCRE2_LITERAL) else {
                    overlay.add_toast(adw::Toast::new("Could not search terminal scrollback"));
                    return;
                };
                terminal.search_set_regex(Some(&regex), 0);
                terminal.search_set_wrap_around(false);
                if !terminal.search_find_previous() {
                    overlay.add_toast(adw::Toast::new("Prompt is no longer in scrollback"));
                }
                window.hide();
                terminal.grab_focus();
            });
            self.history_list.append(&row);
        }
    }
}
