use super::*;

impl Workspace {
    pub(super) fn search_selected(&self, forward: bool) {
        let query = self.search_entry.text();
        if query.is_empty() {
            return;
        }
        let Some(id) = self.selected_session_id() else {
            return;
        };
        let Some(terminal) = self
            .sessions
            .borrow()
            .get(&id)
            .map(|session| session.terminal.clone())
        else {
            return;
        };
        let regex = match vte::Regex::for_search(&query, PCRE2_UTF | PCRE2_LITERAL) {
            Ok(regex) => regex,
            Err(error) => {
                self.show_error(&format!("Could not search terminal: {error}"));
                return;
            }
        };
        terminal.search_set_regex(Some(&regex), 0);
        terminal.search_set_wrap_around(true);
        let found = if forward {
            terminal.search_find_next()
        } else {
            terminal.search_find_previous()
        };
        if !found {
            self.show_error("No matching terminal text");
        }
    }
}
