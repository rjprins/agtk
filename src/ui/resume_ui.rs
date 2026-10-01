//! After a reboot the session hosts are gone while the store still lists the
//! sessions as open. One prompt at startup offers to start them again.

use super::*;

/// The startup prompt, kept so inspection can report what it offers.
#[derive(Clone)]
pub(super) struct ResumePrompt {
    dialog: adw::AlertDialog,
    choices: Rc<Vec<(String, gtk::CheckButton)>>,
}

impl ResumePrompt {
    pub(super) fn is_visible(&self) -> bool {
        self.dialog.is_mapped()
    }

    pub(super) fn choices(&self) -> impl Iterator<Item = (&str, bool)> {
        self.choices
            .iter()
            .map(|(id, check)| (id.as_str(), check.is_active()))
    }

    pub(super) fn inspection_node(&self, window: &adw::ApplicationWindow) -> UiNode {
        UiNode {
            id: "resume-prompt".to_owned(),
            role: "dialog".to_owned(),
            label: Some("Resume sessions from before the restart".to_owned()),
            is_visible: self.is_visible(),
            is_enabled: self.dialog.is_sensitive(),
            is_selected: false,
            bounds: widget_bounds(&self.dialog, window),
            children: self
                .choices
                .iter()
                .map(|(id, check)| UiNode {
                    id: format!("resume-session-{id}"),
                    role: "checkbox".to_owned(),
                    label: Some(id.clone()),
                    is_visible: check.is_visible(),
                    is_enabled: check.is_sensitive(),
                    is_selected: check.is_active(),
                    bounds: widget_bounds(check, window),
                    children: Vec::new(),
                })
                .collect(),
        }
    }
}

/// A session the store lists as open although nothing answers on its socket.
pub(super) fn lost_its_host(stored_state: SessionState, connected: bool) -> bool {
    !connected && stored_state != SessionState::Exited
}

/// Where the session ran: its project, and its worktree when that is a separate checkout.
pub(super) fn session_location(record: &SessionRecord) -> Option<String> {
    let root = record.project_root.as_deref().or(record.cwd.as_deref())?;
    let project = agents_ui::agent_project_name(root);
    let worktree = record
        .worktree_path
        .as_deref()
        .filter(|path| Some(*path) != record.project_root.as_deref())
        .and_then(|path| path.file_name())
        .and_then(|name| name.to_str())
        .filter(|name| !name.is_empty());
    Some(match worktree {
        Some(worktree) => format!("{project} · {worktree}"),
        None => project,
    })
}

/// How a non-agent session starts again where it was. Agents go through restart instead.
pub(super) fn relaunch_params(record: &SessionRecord) -> CreateSessionParams {
    let existing = |path: Option<&Path>| path.filter(|path| path.exists()).map(Path::to_owned);
    let command = record
        .program
        .to_str()
        .filter(|program| !program.is_empty())
        .map(str::to_owned);
    CreateSessionParams {
        kind: record.kind,
        command,
        args: record.args.clone(),
        cwd: existing(record.cwd.as_deref()).or(existing(record.project_root.as_deref())),
        name: Some(record.name.clone()),
        project_root: record.project_root.clone(),
        worktree_path: existing(record.worktree_path.as_deref()),
        initial_input: None,
    }
}

fn prompt_body(count: usize) -> String {
    let sessions = if count == 1 {
        "This session was".to_owned()
    } else {
        format!("These {count} sessions were")
    };
    format!(
        "{sessions} open when agtk last ran, but the processes are gone. \
         Agents continue their conversation when agtk knows it. \
         Everything else starts again in the same directory."
    )
}

impl Workspace {
    /// Asks which of the sessions that lost their host should start again.
    pub(super) fn offer_to_resume_lost_sessions(&self, ids: &[String]) {
        let records = {
            let sessions = self.sessions.borrow();
            ids.iter()
                .filter_map(|id| sessions.get(id).map(|session| session.record.clone()))
                .collect::<Vec<_>>()
        };
        if records.is_empty() {
            return;
        }
        let list = gtk::ListBox::new();
        list.add_css_class("boxed-list");
        list.set_selection_mode(gtk::SelectionMode::None);
        let mut choices = Vec::new();
        for record in &records {
            let check = gtk::CheckButton::builder()
                .active(true)
                .valign(gtk::Align::Center)
                .build();
            let row = adw::ActionRow::builder()
                .title(&record.name)
                .activatable_widget(&check)
                .build();
            if let Some(location) = session_location(record) {
                row.set_subtitle(&location);
            }
            // Prefixes pack from the right, so the check box lands leftmost.
            let icon = provider_icons::session_icon(record.kind, &record.program);
            icon.set_valign(gtk::Align::Center);
            row.add_prefix(&icon);
            row.add_prefix(&check);
            list.append(&row);
            choices.push((record.id.clone(), check));
        }
        let scroller = gtk::ScrolledWindow::builder()
            .child(&list)
            .hscrollbar_policy(gtk::PolicyType::Never)
            .propagate_natural_height(true)
            .max_content_height(360)
            .build();
        let dialog = adw::AlertDialog::new(
            Some("Resume Sessions From Before the Restart?"),
            Some(&prompt_body(records.len())),
        );
        dialog.set_extra_child(Some(&scroller));
        dialog.add_responses(&[("skip", "_Not Now"), ("resume", "_Resume")]);
        dialog.set_response_appearance("resume", adw::ResponseAppearance::Suggested);
        dialog.set_default_response(Some("resume"));
        dialog.set_close_response("skip");
        let prompt = ResumePrompt {
            dialog: dialog.clone(),
            choices: Rc::new(choices),
        };
        let workspace = self.clone();
        let chosen = prompt.clone();
        dialog.connect_response(Some("resume"), move |_, _| {
            let ids = chosen
                .choices()
                .filter(|(_, checked)| *checked)
                .map(|(id, _)| id.to_owned())
                .collect::<Vec<_>>();
            workspace.resume_lost_sessions(ids);
        });
        let forgotten = self.clone();
        dialog.connect_closed(move |_| {
            forgotten.resume_prompt.borrow_mut().take();
        });
        *self.resume_prompt.borrow_mut() = Some(prompt);
        dialog.present(Some(&self.window));
    }

    fn resume_lost_sessions(&self, ids: Vec<String>) {
        if ids.is_empty() {
            return;
        }
        let (agents, others): (Vec<_>, Vec<_>) = {
            let sessions = self.sessions.borrow();
            ids.into_iter()
                .filter(|id| sessions.contains_key(id))
                .partition(|id| {
                    sessions
                        .get(id)
                        .is_some_and(|s| agents_ui::agent_provider(s.record.kind).is_some())
                })
        };
        let count = agents.len() + others.len();
        let message = if count == 1 {
            "Resuming 1 session".to_owned()
        } else {
            format!("Resuming {count} sessions")
        };
        self.overlay.add_toast(adw::Toast::new(&message));
        if !agents.is_empty() {
            self.restart_agents(agents, false, None);
        }
        for id in others {
            self.relaunch_in_place(&id);
        }
    }

    /// Starts a shell or custom session again with its row's name and place.
    fn relaunch_in_place(&self, id: &str) {
        let Some(record) = self.sessions.borrow().get(id).map(|s| s.record.clone()) else {
            return;
        };
        let params = relaunch_params(&record);
        let placement = sessions::Placement {
            position: Some(record.position),
            select: self.selected_session_id().as_deref() == Some(id),
        };
        self.stop_session_then(id, None, move |workspace, pending| {
            workspace.launch_session(params, None, Some(placement), pending);
        });
    }
}

#[cfg(test)]
mod tests {
    use super::{lost_its_host, prompt_body, relaunch_params, session_location};
    use crate::control::{SessionKind, SessionState};
    use crate::persist::SessionRecord;
    use std::path::PathBuf;

    fn record(kind: SessionKind) -> SessionRecord {
        let mut record = SessionRecord::discovered("shell-1", PathBuf::from("/tmp/a.sock"));
        record.kind = kind;
        record.name = "Shell 1".to_owned();
        record
    }

    #[test]
    fn only_a_session_recorded_as_open_counts_as_lost() {
        assert!(lost_its_host(SessionState::Busy, false));
        assert!(lost_its_host(SessionState::Running, false));
        assert!(!lost_its_host(SessionState::Exited, false));
        assert!(!lost_its_host(SessionState::Busy, true));
    }

    #[test]
    fn location_names_the_project_and_a_separate_worktree() {
        let mut record = record(SessionKind::Claude);
        assert_eq!(session_location(&record), None);
        record.project_root = Some(PathBuf::from("/home/me/code/agtk"));
        assert_eq!(session_location(&record).as_deref(), Some("agtk"));
        record.worktree_path = Some(PathBuf::from("/home/me/code/agtk"));
        assert_eq!(session_location(&record).as_deref(), Some("agtk"));
        record.worktree_path = Some(PathBuf::from("/home/me/code/agtk-wt/fix-login"));
        assert_eq!(
            session_location(&record).as_deref(),
            Some("agtk · fix-login")
        );
    }

    #[test]
    fn relaunch_keeps_the_command_name_and_place_but_drops_missing_paths() {
        let fixture = tempfile::tempdir().unwrap();
        let mut record = record(SessionKind::Custom);
        record.program = PathBuf::from("/usr/bin/htop");
        record.args = vec!["--delay".to_owned(), "5".to_owned()];
        record.cwd = Some(fixture.path().join("gone"));
        record.project_root = Some(fixture.path().to_owned());
        record.worktree_path = Some(fixture.path().join("gone"));
        let params = relaunch_params(&record);
        assert_eq!(params.kind, SessionKind::Custom);
        assert_eq!(params.command.as_deref(), Some("/usr/bin/htop"));
        assert_eq!(params.args, ["--delay", "5"]);
        assert_eq!(params.name.as_deref(), Some("Shell 1"));
        assert_eq!(params.cwd.as_deref(), Some(fixture.path()));
        assert_eq!(params.worktree_path, None);
        assert_eq!(params.initial_input, None);
    }

    #[test]
    fn a_discovered_session_without_a_program_uses_the_default_shell() {
        let params = relaunch_params(&record(SessionKind::Shell));
        assert_eq!(params.command, None);
    }

    #[test]
    fn body_counts_the_sessions() {
        assert!(prompt_body(1).starts_with("This session was open"));
        assert!(prompt_body(3).starts_with("These 3 sessions were open"));
    }
}
