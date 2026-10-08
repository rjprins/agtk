use super::*;

impl Workspace {
    /// Save only whether a draft exists, so a restart cannot mix an automatic
    /// notification into a reattached composer. Draft text stays in the agent.
    pub(super) fn remember_pending_input(&self, id: &str, pending: bool) {
        let changed = {
            let mut drafts = self.session_input_pending.borrow_mut();
            if pending {
                drafts.insert(id.to_owned())
            } else {
                drafts.remove(id)
            }
        };
        if changed && let Ok(value) = serde_json::to_value(&*self.session_input_pending.borrow()) {
            self.save_preference("sessionInputPending", value);
        }
    }

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
        let history = Rc::new(session.history.clone());
        for (index, input) in history.iter().enumerate() {
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
            let history = history.clone();
            row.connect_activated(move |_| {
                if !scroll_to_prompt(&terminal, &history, index) {
                    overlay.add_toast(adw::Toast::new("Prompt is no longer in scrollback"));
                }
                window.hide();
                terminal.grab_focus();
            });
            self.history_list.append(&row);
        }
        self.scroll_history_to_end();
    }

    /// Keeps the newest prompt in view once the rows have been laid out.
    pub(super) fn scroll_history_to_end(&self) {
        let Some(scrolled) = self
            .history_list
            .ancestor(gtk::ScrolledWindow::static_type())
            .and_downcast::<gtk::ScrolledWindow>()
        else {
            return;
        };
        let adjustment = scrolled.vadjustment();
        glib::idle_add_local_once(move || {
            adjustment.set_value(adjustment.upper() - adjustment.page_size());
        });
    }
}

/// Selects the prompt `history[index]` in the scrollback and scrolls to it.
pub fn scroll_to_prompt(terminal: &vte::Terminal, history: &[String], index: usize) -> bool {
    // Agents wrap long prompts with hard line breaks, so the needle must fit
    // on the first rendered row.
    let limit = (terminal.column_count() as usize)
        .saturating_sub(12)
        .clamp(16, 60);
    let needle = history_needle(&history[index], limit);
    // VTE rejects PCRE2_LITERAL for search regexes, so escape instead.
    let pattern = glib::Regex::escape_string(&needle);
    let Ok(regex) = vte::Regex::for_search(&pattern, PCRE2_UTF) else {
        return false;
    };
    // VTE searches backwards from the selection, and a failed search parks an
    // empty one at the top of the buffer, so always start from the very end.
    terminal.unselect_all();
    if let Some(adjustment) = terminal.vadjustment() {
        adjustment.set_value(adjustment.upper() - adjustment.page_size());
    }
    terminal.search_set_regex(Some(&regex), 0);
    terminal.search_set_wrap_around(false);
    // Later prompts with the same opening words sit closer to the end.
    let repeats = history[index + 1..]
        .iter()
        .filter(|later| history_needle(later, limit) == needle)
        .count();
    (0..=repeats).all(|_| terminal.search_find_previous())
}
