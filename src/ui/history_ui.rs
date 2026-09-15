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
            return;
        };
        let sessions = self.sessions.borrow();
        let Some(session) = sessions.get(id) else {
            self.history_button.set_sensitive(false);
            return;
        };
        self.history_button
            .set_sensitive(!session.history.is_empty());
        for input in session.history.iter().rev() {
            let button = gtk::Button::with_label(input);
            button.add_css_class("tui-button");
            button.set_halign(gtk::Align::Fill);
            button.set_tooltip_text(Some("Scroll the terminal to this prompt"));

            let terminal = session.terminal.clone();
            let popover = self.history_popover.clone();
            let overlay = self.overlay.clone();
            let input = input.clone();
            button.connect_clicked(move |_| {
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
                popover.popdown();
                terminal.grab_focus();
            });
            self.history_list.append(&button);
        }
    }
}
