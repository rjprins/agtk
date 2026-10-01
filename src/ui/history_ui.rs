use super::*;

impl Workspace {
    /// A line the user typed into the terminal, reconstructed from keystrokes.
    pub(super) fn record_typed_input(&self, id: &str, input: String) {
        let hooked = self
            .sessions
            .borrow()
            .get(id)
            .is_some_and(|session| session.hook_signal.is_some());
        // Keystrokes are a guess: cursor motion and completions are invisible
        // here. Once an agent hook has reported, the hook delivers the prompt.
        if !hooked {
            self.push_history(id, input, true);
        }
        self.mark_agent_busy(id);
    }

    /// A prompt as the agent's hook reported it.
    pub(super) fn record_submitted_prompt(&self, id: &str, prompt: String) {
        self.push_history(id, prompt, false);
    }

    fn push_history(&self, id: &str, input: String, typed: bool) {
        let mut sessions = self.sessions.borrow_mut();
        let Some(session) = sessions.get_mut(id) else {
            return;
        };
        let history = &mut session.history;
        if !typed && session.typed_history_pending > 0 {
            // The agent's version supersedes the oldest keystroke guess still waiting.
            let index = history.len() - session.typed_history_pending;
            history[index] = input;
            session.typed_history_pending -= 1;
        } else if history.last() != Some(&input) {
            history.push(input);
            if typed {
                session.typed_history_pending += 1;
            }
            if history.len() > 200 {
                history.remove(0);
                session.typed_history_pending = session.typed_history_pending.min(history.len());
            }
        }
        drop(sessions);

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
            self.content_title.set_tooltip_text(None);
            return;
        };
        let sessions = self.sessions.borrow();
        let Some(session) = sessions.get(id) else {
            self.history_button.set_sensitive(false);
            self.history_button.set_label("History (0)");
            self.context_last_input.set_text("(none yet)");
            self.content_title.set_tooltip_text(None);
            return;
        };
        self.history_button
            .set_sensitive(!session.history.is_empty());
        self.history_button
            .set_label(&format!("History ({})", session.history.len()));
        // The subtitle is one line; hovering it shows the full prompt.
        let last_prompt = session.history.last();
        let last_line =
            last_prompt.map(|input| input.split_whitespace().collect::<Vec<_>>().join(" "));
        self.context_last_input
            .set_text(last_line.as_deref().unwrap_or("(none yet)"));
        self.content_title
            .set_tooltip_text(last_prompt.map(|input| input.trim()));
        for input in session.history.iter().rev() {
            let row = adw::ActionRow::builder()
                .title(glib::markup_escape_text(input))
                .title_lines(8)
                .activatable(true)
                .build();
            row.set_tooltip_text(Some(input.as_str()));
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
