use std::collections::{BTreeMap, BTreeSet};
use std::path::{Path, PathBuf};

use super::*;
use crate::control::{AgentListParams, AgentPreviewParams, AgentRestoreParams};
use crate::persist::now_millis;
use crate::providers::{
    AgentProvider, ConversationRole, DiscoveryRoots, ProviderDiscovery, ProviderPreview,
    ProviderSession, RestoreTarget,
};

const LIST_WIDTH: i32 = 440;

#[derive(Debug, Clone, PartialEq, Eq)]
pub(super) struct AgentSessionItem {
    session: ProviderSession,
    project_root: Option<PathBuf>,
    worktree_path: Option<PathBuf>,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub(super) enum AgentRestoreDestination {
    LastKnown,
    ExistingWorktree(PathBuf),
    NewWorktree,
    CustomDirectory,
}

/// Every word must appear in one of the session's texts.
fn filter_agent_sessions(
    sessions: &[AgentSessionItem],
    project_root: Option<&Path>,
    filter: &str,
    names: &HashMap<String, String>,
) -> Vec<AgentSessionItem> {
    let words = filter
        .split_whitespace()
        .map(str::to_lowercase)
        .collect::<Vec<_>>();
    sessions
        .iter()
        .filter(|item| {
            if project_root.is_some_and(|root| item.project_root.as_deref() != Some(root)) {
                return false;
            }
            if words.is_empty() {
                return true;
            }
            let session = &item.session;
            let texts = [
                names.get(&agent_session_key(session)).map(String::as_str),
                Some(session.name.as_str()),
                session.custom_title.as_deref(),
                session.ai_title.as_deref(),
                session.first_prompt.as_deref(),
                session.last_prompt.as_deref(),
                session.branch.as_deref(),
                Some(session.provider.command()),
                Some(session.provider_session_id.as_str()),
                session.cwd.as_deref().and_then(Path::to_str),
                item.project_root.as_deref().and_then(Path::to_str),
                agent_worktree_name(item).as_deref(),
            ]
            .into_iter()
            .flatten()
            .map(str::to_lowercase)
            .collect::<Vec<_>>();
            words
                .iter()
                .all(|word| texts.iter().any(|text| text.contains(word.as_str())))
        })
        .cloned()
        .collect()
}

fn classify_agent_sessions(
    sessions: Vec<ProviderSession>,
    known_project_roots: &[PathBuf],
    manager: &crate::worktrees::WorktreeManager,
) -> Vec<AgentSessionItem> {
    let mut resolved = BTreeMap::<PathBuf, crate::worktrees::SessionLocation>::new();
    sessions
        .into_iter()
        .map(|session| {
            let location = session.cwd.as_ref().map(|cwd| {
                resolved
                    .entry(cwd.clone())
                    .or_insert_with(|| manager.resolve_session_location(cwd, known_project_roots))
                    .clone()
            });
            AgentSessionItem {
                session,
                project_root: location
                    .as_ref()
                    .map(|location| location.project_root.clone()),
                worktree_path: location.and_then(|location| location.worktree_path),
            }
        })
        .collect()
}

/// A running agent row whose conversation agtk does not know yet.
#[derive(Debug, Clone)]
struct UnclaimedAgent {
    session_id: String,
    provider: AgentProvider,
    cwd: PathBuf,
    launched_at: u64,
}

/// Pairs each running agent with the earliest log in its directory that
/// began after it launched. Codex has no hook to say which log is its own.
fn match_unclaimed_agents(
    agents: &[UnclaimedAgent],
    items: &[AgentSessionItem],
) -> Vec<(String, usize)> {
    // Logs start at the first prompt, a little after launch at the earliest.
    const CLOCK_SLACK_MILLIS: u64 = 5_000;
    let mut agents = agents.to_vec();
    agents.sort_by_key(|agent| agent.launched_at);
    let mut claimed = BTreeSet::new();
    let mut matches = Vec::new();
    for agent in agents {
        let best = items
            .iter()
            .enumerate()
            .filter(|(index, item)| {
                !claimed.contains(index)
                    && item.session.provider == agent.provider
                    && item.session.cwd.as_deref() == Some(agent.cwd.as_path())
                    && item.session.created_at + CLOCK_SLACK_MILLIS >= agent.launched_at
            })
            .min_by_key(|(_, item)| item.session.created_at)
            .map(|(index, _)| index);
        if let Some(index) = best {
            claimed.insert(index);
            matches.push((agent.session_id, index));
        }
    }
    matches
}

fn restore_target_for_location(
    cwd: Option<PathBuf>,
    project_root: Option<PathBuf>,
    known_project_roots: &[PathBuf],
    manager: &crate::worktrees::WorktreeManager,
) -> RestoreTarget {
    let Some(cwd) = cwd else {
        return RestoreTarget {
            project_root,
            ..RestoreTarget::default()
        };
    };
    let location = manager.resolve_session_location(&cwd, known_project_roots);
    RestoreTarget {
        cwd: Some(location.cwd),
        project_root: project_root.or(Some(location.project_root)),
        worktree_path: location.worktree_path,
        name: None,
    }
}

fn agent_project_name(root: &Path) -> String {
    let leaf = root
        .file_name()
        .and_then(|name| name.to_str())
        .filter(|name| !name.is_empty())
        .unwrap_or_else(|| root.to_str().unwrap_or("Other locations"));
    // A checkout called main says little without the folder it sits in.
    let parent = root
        .parent()
        .and_then(Path::file_name)
        .and_then(|name| name.to_str());
    match parent {
        Some(parent) if is_default_branch(leaf) => format!("{parent}/{leaf}"),
        _ => leaf.to_owned(),
    }
}

fn agent_worktree_name(item: &AgentSessionItem) -> Option<String> {
    item.worktree_path
        .as_deref()?
        .file_name()
        .and_then(|name| name.to_str())
        .filter(|name| !name.is_empty())
        .map(str::to_owned)
}

fn is_default_branch(branch: &str) -> bool {
    matches!(branch, "main" | "master")
}

/// Where the session ran, leaving out what every main-checkout session shares.
fn agent_location_label(item: &AgentSessionItem) -> String {
    let project = item
        .project_root
        .as_deref()
        .or(item.session.cwd.as_deref())
        .map(agent_project_name)
        .unwrap_or_else(|| "Unknown location".to_owned());
    let in_worktree = item.worktree_path.is_some() && item.worktree_path != item.project_root;
    let place = match item.session.branch.as_deref() {
        Some(branch) if !is_default_branch(branch) => Some(branch.to_owned()),
        _ if in_worktree => agent_worktree_name(item),
        _ => None,
    };
    match place {
        Some(place) => format!("{project} · {place}"),
        None => project,
    }
}

fn prompt_count_label(count: u32) -> String {
    match count {
        0 => "no prompts".to_owned(),
        1 => "1 prompt".to_owned(),
        count => format!("{count} prompts"),
    }
}

/// Calendar days between two local times, so 23:50 yesterday is one day ago.
fn days_between(then: &glib::DateTime, now: &glib::DateTime) -> i64 {
    let midnight = |time: &glib::DateTime| {
        let (year, month, day) = time.ymd();
        glib::DateTime::from_local(year, month, day, 0, 0, 0.0)
            .map(|midnight| midnight.to_unix())
            .unwrap_or_else(|_| time.to_unix())
    };
    // Rounding absorbs daylight saving hours.
    ((midnight(now) - midnight(then)) as f64 / 86_400.0).round() as i64
}

fn local_time(millis: u64) -> Option<glib::DateTime> {
    glib::DateTime::from_unix_local((millis / 1_000) as i64).ok()
}

fn format_time(time: &glib::DateTime, format: &str) -> String {
    time.format(format).map(String::from).unwrap_or_default()
}

/// The list heading for a session's day.
fn agent_day_heading(days: i64, then: &glib::DateTime) -> String {
    match days {
        ..=0 => "Today".to_owned(),
        1 => "Yesterday".to_owned(),
        2..=6 => format_time(then, "%A"),
        _ => "Older".to_owned(),
    }
}

/// The time on a row; the heading already names the day of recent ones.
fn agent_row_time(days: i64, then: &glib::DateTime) -> String {
    if days <= 6 {
        format_time(then, "%H:%M")
    } else {
        format_time(then, "%b %-d")
    }
}

/// When the session ran, as one time or a span when it lasted a while.
fn agent_span_label(now: &glib::DateTime, start: u64, end: u64) -> String {
    let (Some(first), Some(last)) = (local_time(start), local_time(end)) else {
        return agent_when_label(now, end);
    };
    if end.saturating_sub(start) < 60_000 {
        agent_when_label(now, end)
    } else if days_between(&first, &last) == 0 {
        format!(
            "{} {} to {}",
            agent_day_label(now, &last),
            format_time(&first, "%H:%M"),
            format_time(&last, "%H:%M")
        )
    } else {
        format!(
            "{} to {}",
            agent_when_label(now, start),
            agent_when_label(now, end)
        )
    }
}

fn agent_day_label(now: &glib::DateTime, then: &glib::DateTime) -> String {
    match days_between(then, now) {
        days @ ..=6 => agent_day_heading(days, then),
        _ => format_time(then, "%b %-d, %Y"),
    }
}

fn agent_when_label(now: &glib::DateTime, millis: u64) -> String {
    let Some(then) = local_time(millis) else {
        return String::new();
    };
    format!(
        "{} {}",
        agent_day_label(now, &then),
        format_time(&then, "%H:%M")
    )
}

/// The resume dialog and what it remembers between openings.
#[derive(Clone)]
pub(super) struct AgentDialog {
    pub(super) modal: modal::Modal,
    pub(super) list: gtk::ListBox,
    list_scroller: gtk::ScrolledWindow,
    pub(super) project_filter: gtk::DropDown,
    pub(super) project_choices: gtk::StringList,
    pub(super) project_values: Rc<RefCell<Vec<Option<PathBuf>>>>,
    pub(super) filter: gtk::SearchEntry,
    pub(super) preview: gtk::Box,
    title: gtk::EditableLabel,
    pub(super) restore_destination: adw::ComboRow,
    pub(super) restore_destination_choices: gtk::StringList,
    pub(super) restore_destination_values: Rc<RefCell<Vec<AgentRestoreDestination>>>,
    pub(super) restore_branch: adw::EntryRow,
    pub(super) restore_custom_cwd: adw::EntryRow,
    pub(super) restore_button: gtk::Button,
    pub(super) sessions: Rc<RefCell<Vec<AgentSessionItem>>>,
    /// The rows on screen, in order, with the session each one shows.
    rows: Rc<RefCell<Vec<(gtk::ListBoxRow, ProviderSession)>>>,
    /// Set while the list changes its selection itself.
    selecting: Rc<Cell<bool>>,
    pub(super) hidden: Rc<RefCell<HashSet<String>>>,
    /// Names given in agtk, by session key. They win over the log's title.
    pub(super) names: Rc<RefCell<HashMap<String, String>>>,
    pub(super) hidden_loaded: Rc<Cell<bool>>,
    pub(super) selected: Rc<RefCell<Option<ProviderSession>>>,
    /// True while the logs are being read.
    loading: Rc<Cell<bool>>,
}

impl AgentDialog {
    pub(super) fn build(parent: &adw::ApplicationWindow) -> Self {
        let project_choices = gtk::StringList::new(&["All projects"]);
        let project_filter =
            gtk::DropDown::new(Some(project_choices.clone()), None::<&gtk::Expression>);
        project_filter.set_tooltip_text(Some("Show sessions from one project"));
        let filter = gtk::SearchEntry::builder()
            .placeholder_text("Search titles, prompts, and branches")
            .margin_start(6)
            .margin_end(6)
            .margin_top(6)
            .margin_bottom(6)
            .build();
        let list = gtk::ListBox::new();
        list.add_css_class("navigation-sidebar");
        list.set_activate_on_single_click(false);
        let list_scroller = gtk::ScrolledWindow::builder()
            .hscrollbar_policy(gtk::PolicyType::Never)
            .vexpand(true)
            .child(&list)
            .build();
        let sidebar = gtk::Box::new(gtk::Orientation::Vertical, 0);
        sidebar.add_css_class("sidebar-pane");
        sidebar.set_size_request(LIST_WIDTH, -1);
        sidebar.append(&filter);
        sidebar.append(&list_scroller);

        let title = gtk::EditableLabel::new("");
        title.add_css_class("title-2");
        title.set_tooltip_text(Some("Rename (F2)"));
        let preview = gtk::Box::new(gtk::Orientation::Vertical, 18);
        preview.set_margin_top(18);
        preview.set_margin_bottom(18);
        preview.set_margin_start(24);
        preview.set_margin_end(24);
        let preview_scroller = gtk::ScrolledWindow::builder()
            .hscrollbar_policy(gtk::PolicyType::Never)
            .vexpand(true)
            .child(&preview)
            .build();
        let restore_destination_choices = gtk::StringList::new(&["Last location"]);
        let restore_destination = adw::ComboRow::builder()
            .title("Resumes in")
            .model(&restore_destination_choices)
            .build();
        let restore_branch = adw::EntryRow::builder().title("Branch Name").build();
        restore_branch.set_visible(false);
        let restore_custom_cwd = adw::EntryRow::builder().title("Directory").build();
        restore_custom_cwd.set_visible(false);
        let destination = gtk::ListBox::new();
        destination.add_css_class("boxed-list");
        destination.set_selection_mode(gtk::SelectionMode::None);
        destination.append(&restore_destination);
        destination.append(&restore_branch);
        destination.append(&restore_custom_cwd);
        destination.set_margin_start(24);
        destination.set_margin_end(24);
        destination.set_margin_bottom(18);
        let detail = gtk::Box::new(gtk::Orientation::Vertical, 0);
        detail.set_hexpand(true);
        detail.append(&preview_scroller);
        detail.append(&destination);

        let content = gtk::Box::new(gtk::Orientation::Horizontal, 0);
        content.append(&sidebar);
        content.append(&gtk::Separator::new(gtk::Orientation::Vertical));
        content.append(&detail);

        let restore_button = gtk::Button::with_label("Resume");
        restore_button.add_css_class("suggested-action");
        restore_button.set_sensitive(false);
        let modal = modal::Modal::new(parent, "Resume Session", 1100, 720, &content);
        let header = modal.header();
        header.pack_start(&project_filter);
        header.pack_end(&restore_button);

        Self {
            modal,
            list,
            list_scroller,
            project_filter,
            project_choices,
            project_values: Rc::new(RefCell::new(vec![None])),
            filter,
            preview,
            title,
            restore_destination,
            restore_destination_choices,
            restore_destination_values: Rc::new(RefCell::new(vec![
                AgentRestoreDestination::LastKnown,
            ])),
            restore_branch,
            restore_custom_cwd,
            restore_button,
            sessions: Rc::new(RefCell::new(Vec::new())),
            rows: Rc::default(),
            selecting: Rc::default(),
            hidden: Rc::new(RefCell::new(HashSet::new())),
            names: Rc::default(),
            hidden_loaded: Rc::new(Cell::new(false)),
            selected: Rc::new(RefCell::new(None)),
            loading: Rc::default(),
        }
    }
}

impl Workspace {
    pub(super) fn connect_agents(&self) {
        let agent_workspace = self.clone();
        self.agents
            .filter
            .connect_search_changed(move |_| agent_workspace.filter_agent_panel());
        let agent_workspace = self.clone();
        self.agents
            .filter
            .connect_activate(move |_| agent_workspace.restore_agent_from_panel());
        // Escape clears the search first and closes the dialog second.
        let agent_workspace = self.clone();
        self.agents.filter.connect_stop_search(move |filter| {
            if filter.text().is_empty() {
                agent_workspace.agents.modal.hide();
            } else {
                filter.set_text("");
            }
        });
        let agent_workspace = self.clone();
        self.agents
            .project_filter
            .connect_selected_notify(move |_| agent_workspace.filter_agent_panel());
        let agent_workspace = self.clone();
        self.agents.list.connect_row_selected(move |_, row| {
            if agent_workspace.agents.selecting.get() {
                return;
            }
            if let Some(session) = row.and_then(|row| agent_workspace.agent_row_session(row)) {
                agent_workspace.preview_agent_for_panel(session);
            }
        });
        let agent_workspace = self.clone();
        self.agents.list.connect_row_activated(move |_, row| {
            if let Some(session) = agent_workspace.agent_row_session(row) {
                let is_selected = agent_workspace
                    .agents
                    .selected
                    .borrow()
                    .as_ref()
                    .is_some_and(|selected| same_agent_session(selected, &session));
                if is_selected {
                    agent_workspace.restore_agent_from_panel();
                }
            }
        });
        let agent_workspace = self.clone();
        self.agents.title.connect_editing_notify(move |title| {
            if !title.is_editing() {
                agent_workspace.rename_selected_agent(title.text().trim());
            }
        });
        let agent_workspace = self.clone();
        self.agents
            .restore_destination
            .connect_selected_notify(move |_| agent_workspace.update_agent_destination_inputs());
        let agent_workspace = self.clone();
        self.agents
            .restore_button
            .connect_clicked(move |_| agent_workspace.restore_agent_from_panel());
        let agent_workspace = self.clone();
        self.agents.modal.connect_show(move || {
            agent_workspace.agents.filter.set_text("");
            agent_workspace.agents.filter.grab_focus();
            agent_workspace.refresh_agent_panel();
        });
        let agent_navigation = gtk::EventControllerKey::new();
        agent_navigation.set_propagation_phase(gtk::PropagationPhase::Capture);
        let agent_workspace = self.clone();
        agent_navigation.connect_key_pressed(move |_, key, _, _| {
            let agents = &agent_workspace.agents;
            let editing = agents.restore_destination.has_focus()
                || agents.project_filter.has_focus()
                || agents.restore_branch.has_focus()
                || agents.restore_custom_cwd.has_focus()
                || agents.title.is_editing();
            let handled = match key {
                gtk::gdk::Key::Down if !editing => {
                    agent_workspace.navigate_agent_selection(1);
                    true
                }
                gtk::gdk::Key::Up if !editing => {
                    agent_workspace.navigate_agent_selection(-1);
                    true
                }
                gtk::gdk::Key::Page_Down if !editing => {
                    agent_workspace.navigate_agent_selection(8);
                    true
                }
                gtk::gdk::Key::Page_Up if !editing => {
                    agent_workspace.navigate_agent_selection(-8);
                    true
                }
                gtk::gdk::Key::F2 if !editing && agents.selected.borrow().is_some() => {
                    agents.title.start_editing();
                    true
                }
                // Delete in an empty search box has nothing else to do.
                gtk::gdk::Key::Delete if !editing && agents.filter.text().is_empty() => {
                    agent_workspace.hide_selected_agent();
                    true
                }
                _ => false,
            };
            if handled {
                glib::Propagation::Stop
            } else {
                glib::Propagation::Proceed
            }
        });
        self.agents.modal.add_controller(agent_navigation);
    }
}

impl Workspace {
    fn agent_row_session(&self, row: &gtk::ListBoxRow) -> Option<ProviderSession> {
        self.agents
            .rows
            .borrow()
            .iter()
            .find(|(candidate, _)| candidate == row)
            .map(|(_, session)| session.clone())
    }

    pub(super) fn filter_agent_panel(&self) {
        self.render_agent_sessions();
        let rows = self.agents.rows.borrow().clone();
        let selected = self.agents.selected.borrow().clone();
        let still_shown = selected.as_ref().is_some_and(|current| {
            rows.iter()
                .any(|(_, session)| same_agent_session(session, current))
        });
        if still_shown {
            return;
        }
        match rows.first() {
            Some((_, session)) => self.preview_agent_for_panel(session.clone()),
            None => {
                self.agents.selected.borrow_mut().take();
                self.agents.restore_button.set_sensitive(false);
                self.render_agent_preview_message("No session selected");
            }
        }
    }

    pub(super) fn navigate_agent_selection(&self, delta: i32) {
        let rows = self.agents.rows.borrow().clone();
        if rows.is_empty() {
            return;
        }
        let current = self.agents.selected.borrow().clone();
        let index = current
            .as_ref()
            .and_then(|selected| {
                rows.iter()
                    .position(|(_, session)| same_agent_session(session, selected))
            })
            .map_or(0, |index| index as i32 + delta);
        let next = index.clamp(0, rows.len() as i32 - 1) as usize;
        self.preview_agent_for_panel(rows[next].1.clone());
    }

    fn visible_agent_items(&self) -> Vec<AgentSessionItem> {
        let hidden = self.agents.hidden.borrow();
        let sessions = self
            .agents
            .sessions
            .borrow()
            .iter()
            .filter(|item| !hidden.contains(&agent_session_key(&item.session)))
            .cloned()
            .collect::<Vec<_>>();
        filter_agent_sessions(
            &sessions,
            self.selected_agent_project().as_deref(),
            self.agents.filter.text().as_str(),
            &self.agents.names.borrow(),
        )
    }

    /// Opens the dialog on every project.
    pub(super) fn open_agents(&self) {
        self.rebuild_agent_project_choices(None);
        self.present_agents();
    }

    pub(super) fn open_agent_for_project(&self, root: &str) {
        self.rebuild_agent_project_choices(Some(PathBuf::from(root)));
        self.present_agents();
    }

    fn present_agents(&self) {
        if self.agents.modal.is_visible() {
            self.refresh_agent_panel();
        } else {
            self.agents.modal.present();
        }
    }

    fn selected_agent_project(&self) -> Option<PathBuf> {
        self.agents
            .project_values
            .borrow()
            .get(self.agents.project_filter.selected() as usize)
            .cloned()
            .flatten()
    }

    fn refresh_agent_project_choices(&self) {
        let selected = self.selected_agent_project();
        self.rebuild_agent_project_choices(selected);
    }

    fn rebuild_agent_project_choices(&self, selected: Option<PathBuf>) {
        let mut seen = BTreeSet::<PathBuf>::new();
        let mut roots = self
            .agents
            .sessions
            .borrow()
            .iter()
            .filter_map(|item| item.project_root.clone())
            .filter(|root| seen.insert(root.clone()))
            .collect::<Vec<_>>();
        if let Some(root) = selected.as_ref()
            && seen.insert(root.clone())
        {
            roots.push(root.clone());
        }
        roots.sort_by(|left, right| {
            agent_project_name(left)
                .to_lowercase()
                .cmp(&agent_project_name(right).to_lowercase())
                .then_with(|| left.cmp(right))
        });

        let mut labels = vec!["All projects".to_owned()];
        labels.extend(roots.iter().map(|root| agent_project_name(root)));
        let labels = labels.iter().map(String::as_str).collect::<Vec<_>>();
        let mut values = vec![None];
        values.extend(roots.into_iter().map(Some));
        let selected_index = selected
            .as_ref()
            .and_then(|selected| {
                values
                    .iter()
                    .position(|value| value.as_ref() == Some(selected))
            })
            .unwrap_or(0);
        // Rebuilding fires the filter handler, which must see the new values.
        *self.agents.project_values.borrow_mut() = values;
        self.agents
            .project_choices
            .splice(0, self.agents.project_choices.n_items(), &labels);
        self.agents
            .project_filter
            .set_selected(selected_index as u32);
    }

    pub(super) fn refresh_agent_panel(&self) {
        self.load_agent_preferences();
        self.agents.loading.set(true);
        if self.agents.sessions.borrow().is_empty() {
            self.render_agent_list_message("Reading agent logs…");
            self.render_agent_preview_message("");
        }
        let discovery = self.provider_discovery();
        let live = self.live_provider_sessions();
        let known_project_roots = self
            .project_summaries()
            .into_iter()
            .map(|project| PathBuf::from(project.root))
            .collect::<Vec<_>>();
        let manager = crate::worktrees::WorktreeManager::new(self.paths.attic_dir());
        self.run_slow(
            move || {
                discovery.discover(now_millis(), &live).map(|sessions| {
                    classify_agent_sessions(sessions, &known_project_roots, &manager)
                })
            },
            |workspace, result| match result {
                Ok(mut sessions) => {
                    workspace.agents.loading.set(false);
                    workspace.claim_live_conversations(&mut sessions);
                    workspace.agents.sessions.replace(sessions);
                    workspace.agents.selected.borrow_mut().take();
                    workspace.refresh_agent_project_choices();
                    workspace.filter_agent_panel();
                }
                Err(error) => {
                    workspace.agents.loading.set(false);
                    workspace.render_agent_list_message(&format!(
                        "Could not read the agent logs: {error}"
                    ));
                    workspace.show_error(&format!("Could not discover agent sessions: {error}"));
                }
            },
        );
    }

    pub(super) fn provider_discovery(&self) -> ProviderDiscovery {
        ProviderDiscovery::from_environment().with_cache(self.paths.provider_log_cache())
    }

    pub(super) fn restore_agent_from_panel(&self) {
        if !self.agents.restore_button.is_sensitive() {
            return;
        }
        let Some(mut session) = self.agents.selected.borrow().clone() else {
            self.show_error("Select an agent session first");
            return;
        };
        if let Some(name) = self.agents.names.borrow().get(&agent_session_key(&session)) {
            session.name = name.clone();
        }
        let project_root = self
            .agents
            .sessions
            .borrow()
            .iter()
            .find(|item| same_agent_session(&item.session, &session))
            .and_then(|item| item.project_root.clone());
        let index = self.agents.restore_destination.selected() as usize;
        let destination = self
            .agents
            .restore_destination_values
            .borrow()
            .get(index)
            .cloned()
            .unwrap_or(AgentRestoreDestination::LastKnown);
        let cwd = match destination {
            AgentRestoreDestination::LastKnown => session.cwd.clone(),
            AgentRestoreDestination::ExistingWorktree(path) => Some(path),
            AgentRestoreDestination::CustomDirectory => {
                let Some(cwd) = entry_path(&self.agents.restore_custom_cwd) else {
                    self.show_error("Enter a custom directory first");
                    return;
                };
                Some(cwd)
            }
            AgentRestoreDestination::NewWorktree => {
                let Some(cwd) = session
                    .cwd
                    .clone()
                    .filter(|cwd| cwd.exists())
                    .or(project_root.clone())
                else {
                    self.show_error("This session has no known location for a new worktree");
                    return;
                };
                let branch = self.agents.restore_branch.text().trim().to_owned();
                let branch = if branch.is_empty() {
                    format!("restore-{}", now_millis())
                } else {
                    branch
                };
                let purpose = format!("Restore agent session {}", session.name);
                let manager = crate::worktrees::WorktreeManager::new(self.paths.attic_dir());
                let project_root = project_root.clone();
                self.agents.restore_button.set_sensitive(false);
                self.agents.modal.hide();
                self.run_slow(
                    move || {
                        let root = manager.repository_root(&cwd)?;
                        let created = manager.create(&root, &branch, None, &purpose)?;
                        let path = created.path;
                        Ok(session.restore_plan(RestoreTarget {
                            cwd: Some(path.clone()),
                            project_root: project_root.or(Some(root)),
                            worktree_path: Some(path),
                            ..RestoreTarget::default()
                        }))
                    },
                    |workspace, result| match result {
                        Ok(plan) => workspace.launch_controlled_with_conversation(
                            plan.params,
                            Some(plan.conversation_id),
                            None,
                        ),
                        Err(error) => workspace
                            .show_error(&format!("Could not create a restore worktree: {error}")),
                    },
                );
                return;
            }
        };
        let known_project_roots = self
            .project_summaries()
            .into_iter()
            .map(|project| PathBuf::from(project.root))
            .collect::<Vec<_>>();
        let manager = crate::worktrees::WorktreeManager::new(self.paths.attic_dir());
        self.agents.restore_button.set_sensitive(false);
        self.agents.modal.hide();
        self.run_slow(
            move || {
                let target =
                    restore_target_for_location(cwd, project_root, &known_project_roots, &manager);
                Ok(session.restore_plan(target))
            },
            |workspace, result| match result {
                Ok(plan) => workspace.launch_controlled_with_conversation(
                    plan.params,
                    Some(plan.conversation_id),
                    None,
                ),
                Err(error) => {
                    workspace.show_error(&format!("Could not restore agent session: {error}"))
                }
            },
        );
    }

    pub(super) fn handle_agent_control(&self, pending: PendingRequest) {
        match pending.request.command.clone() {
            ControlCommand::AgentList(params) => self.list_agents(params, pending),
            ControlCommand::AgentPreview(params) => self.preview_agent(params, pending),
            ControlCommand::AgentRestore(params) => self.restore_agent(params, pending),
            ControlCommand::SessionSetState(params) => self.set_session_state(params, pending),
            _ => unreachable!("caller filters agent commands"),
        }
    }

    fn list_agents(&self, params: AgentListParams, pending: PendingRequest) {
        let discovery = ProviderDiscovery::new(
            DiscoveryRoots::from_environment(),
            params.limit as usize,
            u64::from(params.max_age_days) * 24 * 60 * 60 * 1_000,
        )
        .with_cache(self.paths.provider_log_cache());
        let live = self.live_provider_sessions();
        self.run_slow(
            move || discovery.discover(now_millis(), &live),
            move |workspace, result| {
                workspace.respond_agent_result(pending, "Could not discover agent sessions", result)
            },
        );
    }

    fn preview_agent(&self, params: AgentPreviewParams, pending: PendingRequest) {
        let discovery = self.provider_discovery();
        self.run_slow(
            move || {
                discovery.preview(
                    params.provider,
                    &params.provider_session_id,
                    params.max_messages as usize,
                )
            },
            move |workspace, result| {
                workspace.respond_agent_result(pending, "Could not preview agent session", result)
            },
        );
    }

    fn restore_agent(&self, params: AgentRestoreParams, pending: PendingRequest) {
        let discovery = self.provider_discovery();
        let target = RestoreTarget {
            cwd: params.cwd,
            project_root: params.project_root,
            worktree_path: params.worktree_path,
            name: params.name,
        };
        self.run_slow(
            move || {
                let preview = discovery.preview(params.provider, &params.provider_session_id, 1)?;
                Ok(preview.session.restore_plan(target))
            },
            move |workspace, result| match result {
                Ok(plan) => workspace.launch_controlled_with_conversation(
                    plan.params,
                    Some(plan.conversation_id),
                    Some(pending),
                ),
                Err(error) => workspace.report_failure(
                    Some(pending),
                    ErrorCode::OperationRefused,
                    "Could not restore agent session",
                    error.to_string(),
                ),
            },
        );
    }

    fn live_provider_sessions(&self) -> BTreeSet<(AgentProvider, String)> {
        self.sessions
            .borrow()
            .values()
            .filter(|session| session.record.state != SessionState::Exited)
            .filter_map(|session| {
                let provider = match session.record.kind {
                    SessionKind::Codex => AgentProvider::Codex,
                    SessionKind::Claude => AgentProvider::Claude,
                    _ => return None,
                };
                Some((provider, session.record.conversation_id.clone()?))
            })
            .collect()
    }

    /// Starts an exited agent's conversation again in place of its row.
    pub(super) fn resume_exited_session(&self, id: &str) {
        let Some(record) = self.sessions.borrow().get(id).map(|s| s.record.clone()) else {
            return;
        };
        if record.state != SessionState::Exited {
            self.show_error("This session is still running");
            return;
        }
        if agent_provider(record.kind).is_none() {
            return;
        }
        let Some(conversation_id) = record.conversation_id.clone() else {
            self.show_error(
                "agtk does not know this session's conversation. Find it with Resume Session.",
            );
            return;
        };
        self.relaunch_agent(id, Some(conversation_id), None);
    }

    /// Restarts an agent in its row, so an updated CLI takes over. A working
    /// agent loses its current turn, so the menu asks first.
    pub(super) fn restart_agent(&self, id: &str, pending: Option<PendingRequest>) {
        let Some(record) = self.sessions.borrow().get(id).map(|s| s.record.clone()) else {
            return;
        };
        if agent_provider(record.kind).is_none() {
            self.report_failure(
                pending,
                ErrorCode::OperationRefused,
                "Could not restart session",
                "only Claude and Codex agents can restart".into(),
            );
            return;
        }
        if pending.is_none()
            && !is_between_turns(record.state)
            && record.state != SessionState::Exited
        {
            let dialog = adw::AlertDialog::new(
                Some("Restart Working Agent?"),
                Some(
                    "It is in the middle of a turn. Restarting stops that turn but keeps the conversation so far.",
                ),
            );
            dialog.add_responses(&[("cancel", "_Cancel"), ("restart", "_Restart")]);
            dialog.set_response_appearance("restart", adw::ResponseAppearance::Destructive);
            dialog.set_default_response(Some("cancel"));
            dialog.set_close_response("cancel");
            let workspace = self.clone();
            let id = id.to_owned();
            dialog.connect_response(Some("restart"), move |_, _| {
                workspace.restart_agents(vec![id.clone()], false, None);
            });
            dialog.present(Some(&self.window));
            return;
        }
        self.restart_agents(vec![id.to_owned()], false, pending);
    }

    /// Restarts every agent between turns, the usual step after a Claude or Codex update.
    pub(super) fn restart_idle_agents(&self) {
        let (idle, working): (Vec<_>, Vec<_>) = self
            .sessions
            .borrow()
            .values()
            .filter(|s| {
                agent_provider(s.record.kind).is_some() && s.record.state != SessionState::Exited
            })
            .map(|s| (s.record.id.clone(), s.record.state))
            .partition(|(_, state)| is_between_turns(*state));
        let idle = idle.into_iter().map(|(id, _)| id).collect::<Vec<_>>();
        let agents = |count: usize| match count {
            1 => "1 agent".to_owned(),
            count => format!("{count} agents"),
        };
        let message = match (idle.len(), working.len()) {
            (0, _) => "No idle agents to restart".to_owned(),
            (restarting, 0) => format!("Restarting {}", agents(restarting)),
            (restarting, working) => format!(
                "Restarting {}. {} still working keep running",
                agents(restarting),
                working
            ),
        };
        self.overlay.add_toast(adw::Toast::new(&message));
        if !idle.is_empty() {
            self.restart_agents(idle, true, None);
        }
    }

    /// Restarts agents in their rows. Codex rows first claim their log, since
    /// Codex never reports which conversation it holds.
    fn restart_agents(&self, ids: Vec<String>, only_idle: bool, pending: Option<PendingRequest>) {
        let unclaimed = {
            let sessions = self.sessions.borrow();
            ids.iter().any(|id| {
                sessions
                    .get(id)
                    .is_some_and(|s| s.record.conversation_id.is_none())
            })
        };
        let discovery = self.provider_discovery();
        let live = self.live_provider_sessions();
        let known_project_roots = self
            .project_summaries()
            .into_iter()
            .map(|project| PathBuf::from(project.root))
            .collect::<Vec<_>>();
        let manager = crate::worktrees::WorktreeManager::new(self.paths.attic_dir());
        self.run_slow(
            move || {
                if !unclaimed {
                    return Ok(None);
                }
                discovery.discover(now_millis(), &live).map(|sessions| {
                    Some(classify_agent_sessions(
                        sessions,
                        &known_project_roots,
                        &manager,
                    ))
                })
            },
            move |workspace, result| match result {
                Ok(items) => {
                    if let Some(mut items) = items {
                        workspace.claim_live_conversations(&mut items);
                    }
                    workspace.restart_claimed_agents(ids, only_idle, pending);
                }
                Err(error) => workspace.report_failure(
                    pending,
                    ErrorCode::InternalError,
                    "Could not restart agent",
                    format!("could not read the agent logs: {error}"),
                ),
            },
        );
    }

    /// Each agent resumes its conversation when a log holds it. One without a
    /// log has not taken a turn yet, so it starts fresh.
    fn restart_claimed_agents(
        &self,
        ids: Vec<String>,
        only_idle: bool,
        pending: Option<PendingRequest>,
    ) {
        let targets = {
            let sessions = self.sessions.borrow();
            ids.iter()
                .filter_map(|id| {
                    let record = &sessions.get(id)?.record;
                    Some((
                        id.clone(),
                        agent_provider(record.kind)?,
                        record.conversation_id.clone(),
                    ))
                })
                .collect::<Vec<_>>()
        };
        let discovery = self.provider_discovery();
        self.run_slow(
            move || {
                targets
                    .into_iter()
                    .map(|(id, provider, conversation_id)| {
                        let resumable = match conversation_id {
                            Some(conversation_id)
                                if discovery.has_log(provider, &conversation_id)? =>
                            {
                                Some(conversation_id)
                            }
                            _ => None,
                        };
                        Ok((id, resumable))
                    })
                    .collect::<PersistResult<Vec<_>>>()
            },
            move |workspace, result| match result {
                Ok(plans) => {
                    let mut pending = pending;
                    for (id, conversation_id) in plans {
                        // A turn may have started while the logs were read.
                        let idle = workspace
                            .sessions
                            .borrow()
                            .get(&id)
                            .is_some_and(|s| is_between_turns(s.record.state));
                        if only_idle && !idle {
                            continue;
                        }
                        workspace.relaunch_agent(&id, conversation_id, pending.take());
                    }
                    if let Some(pending) = pending {
                        respond_failure(
                            pending,
                            ErrorCode::SessionNotFound,
                            "Session closed before it could restart",
                            None,
                        );
                    }
                }
                Err(error) => workspace.report_failure(
                    pending,
                    ErrorCode::InternalError,
                    "Could not restart agent",
                    error.to_string(),
                ),
            },
        );
    }

    /// Stops an agent and starts it again in its row with the same name, place
    /// and launch flags, resuming `conversation_id` when there is one.
    fn relaunch_agent(
        &self,
        id: &str,
        conversation_id: Option<String>,
        pending: Option<PendingRequest>,
    ) {
        let Some(record) = self.sessions.borrow().get(id).map(|s| s.record.clone()) else {
            return;
        };
        let Some(provider) = agent_provider(record.kind) else {
            return;
        };
        let mut args = conversation_id
            .as_deref()
            .map(|id| provider.resume_args(id))
            .unwrap_or_default();
        args.extend(crate::launch_model::carried_agent_args(
            record.kind,
            &record.args,
        ));
        let existing = |path: Option<PathBuf>| path.filter(|path| path.exists());
        let params = crate::control::CreateSessionParams {
            kind: provider.kind(),
            command: None,
            args,
            cwd: existing(record.cwd.clone()).or(existing(record.project_root.clone())),
            name: Some(record.name.clone()),
            project_root: record.project_root.clone(),
            worktree_path: existing(record.worktree_path.clone()),
            initial_input: None,
        };
        let placement = super::sessions::Placement {
            position: record.position,
            select: self.selected_session_id().as_deref() == Some(id),
        };
        self.stop_session_then(id, pending, move |workspace, pending| {
            workspace.launch_session(params, conversation_id, Some(placement), pending);
        });
    }

    /// Records the conversation of running agents that never reported one, and
    /// drops those conversations from the list since they are open already.
    fn claim_live_conversations(&self, items: &mut Vec<AgentSessionItem>) {
        let agents = self
            .sessions
            .borrow()
            .values()
            .filter(|session| {
                session.record.state != SessionState::Exited
                    && session.record.conversation_id.is_none()
            })
            .filter_map(|session| {
                Some(UnclaimedAgent {
                    session_id: session.record.id.clone(),
                    provider: match session.record.kind {
                        SessionKind::Codex => AgentProvider::Codex,
                        SessionKind::Claude => AgentProvider::Claude,
                        _ => return None,
                    },
                    cwd: session.record.cwd.clone()?,
                    launched_at: session.record.created_at,
                })
            })
            .collect::<Vec<_>>();
        let mut matches = match_unclaimed_agents(&agents, items);
        for (session_id, index) in &matches {
            let record = {
                let mut sessions = self.sessions.borrow_mut();
                let Some(session) = sessions.get_mut(session_id) else {
                    continue;
                };
                session.record.conversation_id =
                    Some(items[*index].session.provider_session_id.clone());
                session.record.clone()
            };
            self.persist_record(record);
        }
        matches.sort_by_key(|(_, index)| std::cmp::Reverse(*index));
        for (_, index) in matches {
            items.remove(index);
        }
    }

    fn render_agent_sessions(&self) {
        while let Some(child) = self.agents.list.first_child() {
            self.agents.list.remove(&child);
        }
        self.agents.rows.borrow_mut().clear();
        let items = self.visible_agent_items();
        if items.is_empty() {
            let has_any = !self.agents.sessions.borrow().is_empty();
            self.render_agent_list_message(if self.agents.loading.get() && !has_any {
                "Reading agent logs…"
            } else if has_any {
                "No sessions match"
            } else {
                "No recent Claude or Codex sessions"
            });
            return;
        }
        let Ok(now) = glib::DateTime::now_local() else {
            return;
        };
        let mut heading = None::<String>;
        for item in items {
            let then = local_time(item.session.last_seen_at).unwrap_or_else(|| now.clone());
            let days = days_between(&then, &now);
            let day = agent_day_heading(days, &then);
            if heading.as_deref() != Some(day.as_str()) {
                let label = gtk::Label::new(Some(&day));
                label.set_xalign(0.0);
                label.add_css_class("heading");
                label.add_css_class("dim-label");
                label.set_margin_top(if heading.is_some() { 12 } else { 0 });
                let row = gtk::ListBoxRow::builder()
                    .child(&label)
                    .selectable(false)
                    .activatable(false)
                    .focusable(false)
                    .build();
                self.agents.list.append(&row);
                heading = Some(day);
            }
            self.append_agent_session_row(item, &agent_row_time(days, &then));
        }
        self.select_agent_row();
    }

    fn agent_title(&self, session: &ProviderSession) -> String {
        self.agents
            .names
            .borrow()
            .get(&agent_session_key(session))
            .cloned()
            .unwrap_or_else(|| session.name.clone())
    }

    fn append_agent_session_row(&self, item: AgentSessionItem, time: &str) {
        let location = agent_location_label(&item);
        let session = item.session;
        let content = gtk::Box::new(gtk::Orientation::Horizontal, 10);
        let icon = provider_icons::agent_icon(session.provider);
        icon.set_valign(gtk::Align::Center);
        content.append(&icon);
        let text = gtk::Box::new(gtk::Orientation::Vertical, 2);
        text.set_hexpand(true);
        let top = gtk::Box::new(gtk::Orientation::Horizontal, 8);
        let title = gtk::Label::new(Some(&self.agent_title(&session)));
        title.set_xalign(0.0);
        title.set_hexpand(true);
        title.set_ellipsize(gtk::pango::EllipsizeMode::End);
        top.append(&title);
        let time = gtk::Label::new(Some(time));
        time.add_css_class("caption");
        time.add_css_class("dim-label");
        time.add_css_class("numeric");
        top.append(&time);
        text.append(&top);
        let bottom = gtk::Box::new(gtk::Orientation::Horizontal, 0);
        let place = gtk::Label::new(Some(&location));
        place.set_xalign(0.0);
        place.set_ellipsize(gtk::pango::EllipsizeMode::End);
        let count = gtk::Label::new(Some(&format!(
            " · {}",
            prompt_count_label(session.prompt_count)
        )));
        for label in [&place, &count] {
            label.add_css_class("caption");
            label.add_css_class("dim-label");
            bottom.append(label);
        }
        text.append(&bottom);
        content.append(&text);

        let row = gtk::ListBoxRow::builder().child(&content).build();
        row.set_tooltip_text(session.first_prompt.as_deref());
        let menu_workspace = self.clone();
        let menu_session = session.clone();
        let click = gtk::GestureClick::new();
        click.set_button(gtk::gdk::BUTTON_SECONDARY);
        click.connect_pressed(move |gesture, _, x, y| {
            let Some(row) = gesture.widget() else {
                return;
            };
            menu_workspace.show_agent_row_menu(&row, &menu_session, x, y);
        });
        row.add_controller(click);
        self.agents.list.append(&row);
        self.agents.rows.borrow_mut().push((row, session));
    }

    fn show_agent_row_menu(&self, row: &gtk::Widget, session: &ProviderSession, x: f64, y: f64) {
        let hide = gtk::Button::with_label("Hide from List");
        hide.add_css_class("flat");
        let popover = gtk::Popover::builder()
            .child(&hide)
            .has_arrow(false)
            .build();
        popover.set_parent(row);
        popover.set_pointing_to(Some(&gtk::gdk::Rectangle::new(x as i32, y as i32, 1, 1)));
        popover.connect_closed(|popover| {
            let popover = popover.clone();
            glib::idle_add_local_once(move || popover.unparent());
        });
        let workspace = self.clone();
        let session = session.clone();
        let hide_popover = popover.clone();
        hide.connect_clicked(move |_| {
            hide_popover.popdown();
            workspace.hide_agent(&session);
        });
        popover.popup();
    }

    /// Shows the selected session's row as selected and scrolls it into view.
    fn select_agent_row(&self) {
        let selected = self.agents.selected.borrow().clone();
        let row = selected.and_then(|selected| {
            self.agents
                .rows
                .borrow()
                .iter()
                .find(|(_, session)| same_agent_session(session, &selected))
                .map(|(row, _)| row.clone())
        });
        self.agents.selecting.set(true);
        self.agents.list.select_row(row.as_ref());
        self.agents.selecting.set(false);
        let Some(row) = row else {
            return;
        };
        let scroller = self.agents.list_scroller.clone();
        let list = self.agents.list.clone();
        // Row bounds are only known after the next layout.
        glib::idle_add_local_once(move || {
            let Some(bounds) = row.compute_bounds(&list) else {
                return;
            };
            let adjustment = scroller.vadjustment();
            let top = f64::from(bounds.y());
            let bottom = top + f64::from(bounds.height());
            if top < adjustment.value() {
                adjustment.set_value(top);
            } else if bottom > adjustment.value() + adjustment.page_size() {
                adjustment.set_value(bottom - adjustment.page_size());
            }
        });
    }

    fn hide_selected_agent(&self) {
        let selected = self.agents.selected.borrow().clone();
        if let Some(session) = selected {
            self.hide_agent(&session);
        }
    }

    fn hide_agent(&self, session: &ProviderSession) {
        let rows = self.agents.rows.borrow().clone();
        let index = rows
            .iter()
            .position(|(_, candidate)| same_agent_session(candidate, session));
        self.agents
            .hidden
            .borrow_mut()
            .insert(agent_session_key(session));
        self.save_preference(
            "hiddenAgentSessions",
            serde_json::json!(sorted(self.agents.hidden.borrow().iter())),
        );
        let is_selected = self
            .agents
            .selected
            .borrow()
            .as_ref()
            .is_some_and(|selected| same_agent_session(selected, session));
        if is_selected {
            // Select the next row, so Delete can clear several in a row.
            let next = index.and_then(|index| {
                rows.iter()
                    .skip(index + 1)
                    .chain(rows.iter().take(index).rev())
                    .next()
                    .map(|(_, session)| session.clone())
            });
            self.agents.selected.borrow_mut().take();
            if let Some(next) = next {
                self.agents.selected.borrow_mut().replace(next);
            }
        }
        self.filter_agent_panel();
        if is_selected && let Some(next) = self.agents.selected.borrow().clone() {
            self.preview_agent_for_panel(next);
        }
    }

    fn rename_selected_agent(&self, name: &str) {
        let Some(session) = self.agents.selected.borrow().clone() else {
            return;
        };
        let key = agent_session_key(&session);
        let changed = {
            let mut names = self.agents.names.borrow_mut();
            if name.is_empty() || name == session.name {
                names.remove(&key).is_some()
            } else if names.get(&key).map(String::as_str) != Some(name) {
                names.insert(key, name.chars().take(80).collect());
                true
            } else {
                false
            }
        };
        self.agents.title.set_text(&self.agent_title(&session));
        if changed {
            self.save_agent_names();
            self.render_agent_sessions();
        }
    }

    fn save_agent_names(&self) {
        let names = self
            .agents
            .names
            .borrow()
            .iter()
            .map(|(key, name)| (key.clone(), serde_json::json!(name)))
            .collect::<serde_json::Map<_, _>>();
        self.save_preference("agentSessionNames", serde_json::Value::Object(names));
    }

    /// Remembers the name of an agent row that is closing, so the dialog shows it later.
    pub(super) fn remember_agent_name(&self, record: &SessionRecord) {
        let provider = match record.kind {
            SessionKind::Claude => AgentProvider::Claude,
            SessionKind::Codex => AgentProvider::Codex,
            _ => return,
        };
        let Some(conversation_id) = record.conversation_id.as_deref() else {
            return;
        };
        if !record.renamed {
            return;
        }
        self.load_agent_preferences();
        self.agents.names.borrow_mut().insert(
            format!("{}:{conversation_id}", provider.command()),
            record.name.clone(),
        );
        self.save_agent_names();
    }

    fn load_agent_preferences(&self) {
        if self.agents.hidden_loaded.replace(true) {
            return;
        }
        let Some(store) = self.store.borrow().clone() else {
            self.agents.hidden_loaded.set(false);
            return;
        };
        self.run_io(
            move || {
                Ok((
                    store.preference("hiddenAgentSessions")?,
                    store.preference("agentSessionNames")?,
                ))
            },
            |workspace, result| {
                let Ok((hidden, names)) = result else {
                    return;
                };
                if let Some(values) = hidden.as_ref().and_then(|value| value.as_array()) {
                    workspace.agents.hidden.borrow_mut().extend(
                        values
                            .iter()
                            .filter_map(|value| value.as_str().map(str::to_owned)),
                    );
                }
                if let Some(names) = names.as_ref().and_then(|value| value.as_object()) {
                    let mut known = workspace.agents.names.borrow_mut();
                    for (key, name) in names {
                        if let Some(name) = name.as_str() {
                            // A name saved during this run is newer than the stored one.
                            known.entry(key.clone()).or_insert_with(|| name.to_owned());
                        }
                    }
                }
                if workspace.agents.modal.is_visible() {
                    workspace.filter_agent_panel();
                }
            },
        );
    }

    fn preview_agent_for_panel(&self, session: ProviderSession) {
        let is_same = self
            .agents
            .selected
            .borrow()
            .as_ref()
            .is_some_and(|selected| same_agent_session(selected, &session));
        self.agents.selected.borrow_mut().replace(session.clone());
        self.select_agent_row();
        if is_same && self.agents.restore_button.is_sensitive() {
            return;
        }
        self.agents
            .restore_branch
            .set_text(&format!("restore-{}", now_millis()));
        self.agents.restore_custom_cwd.set_text(
            session
                .cwd
                .as_ref()
                .map(|path| path.to_string_lossy())
                .as_deref()
                .unwrap_or(""),
        );
        self.reset_agent_destinations(&session);
        self.agents.restore_button.set_sensitive(false);
        let discovery = self.provider_discovery();
        let provider = session.provider;
        let provider_session_id = session.provider_session_id.clone();
        self.run_slow(
            move || discovery.preview(provider, &provider_session_id, 40),
            |workspace, result| match result {
                Ok(preview) => workspace.render_agent_preview(preview),
                Err(error) => {
                    workspace.render_agent_preview_message(&format!(
                        "Could not load the preview: {error}"
                    ));
                }
            },
        );
        self.load_agent_worktrees(session);
    }

    fn render_agent_preview(&self, preview: ProviderPreview) {
        let Some(selected) = self.agents.selected.borrow().clone() else {
            return;
        };
        if !same_agent_session(&selected, &preview.session) {
            return;
        }
        let item = self
            .agents
            .sessions
            .borrow()
            .iter()
            .find(|item| same_agent_session(&item.session, &selected))
            .cloned();
        self.clear_agent_preview();
        let session = &preview.session;
        let preview_box = &self.agents.preview;

        let title_row = gtk::Box::new(gtk::Orientation::Horizontal, 6);
        let title = &self.agents.title;
        title.set_text(&self.agent_title(session));
        title.set_hexpand(true);
        title_row.append(title);
        let rename = gtk::Button::builder()
            .icon_name("document-edit-symbolic")
            .tooltip_text("Rename (F2)")
            .valign(gtk::Align::Center)
            .build();
        rename.add_css_class("flat");
        let rename_title = title.clone();
        rename.connect_clicked(move |_| rename_title.start_editing());
        title_row.append(&rename);
        preview_box.append(&title_row);

        let mut meta = vec![provider_label(session.provider).to_owned()];
        if let Some(item) = &item {
            meta.push(agent_location_label(item));
        }
        if let Some(branch) = session
            .branch
            .as_deref()
            .filter(|branch| is_default_branch(branch))
        {
            meta.push(branch.to_owned());
        }
        if let Ok(now) = glib::DateTime::now_local() {
            meta.push(agent_span_label(
                &now,
                session.created_at,
                session.last_seen_at,
            ));
        }
        meta.push(prompt_count_label(session.prompt_count));
        let meta = gtk::Label::new(Some(&meta.join(" · ")));
        meta.set_xalign(0.0);
        meta.set_wrap(true);
        meta.add_css_class("dim-label");
        // Keep the title's own margin from pushing the line away.
        meta.set_margin_top(-12);
        preview_box.append(&meta);

        let aliases = [session.custom_title.as_deref(), session.ai_title.as_deref()]
            .into_iter()
            .flatten()
            .filter(|alias| *alias != self.agent_title(session))
            .collect::<Vec<_>>();
        if !aliases.is_empty() {
            let label = gtk::Label::new(Some(&format!("Also called {}", aliases.join(", "))));
            label.set_xalign(0.0);
            label.set_wrap(true);
            label.add_css_class("caption");
            label.add_css_class("dim-label");
            label.set_margin_top(-12);
            preview_box.append(&label);
        }

        if let Some(first) = &session.first_prompt {
            self.append_agent_section("You asked", &[(None, first.as_str(), 6)]);
        }
        let last_reply = preview
            .messages
            .iter()
            .rev()
            .find(|message| message.role == ConversationRole::Assistant)
            .map(|message| message.text.as_str());
        let last_prompt = session
            .last_prompt
            .as_deref()
            .filter(|last| Some(*last) != session.first_prompt.as_deref());
        let mut left_off = Vec::new();
        if let Some(prompt) = last_prompt {
            left_off.push((Some("You"), prompt, 3));
        }
        if let Some(reply) = last_reply {
            left_off.push((Some("Agent"), reply, 10));
        }
        if !left_off.is_empty() {
            self.append_agent_section("Where it left off", &left_off);
        }
        if !preview.files_changed.is_empty() {
            let base = item
                .as_ref()
                .and_then(|item| item.worktree_path.clone())
                .or(session.cwd.clone());
            let files = preview
                .files_changed
                .iter()
                .map(|path| {
                    base.as_deref()
                        .and_then(|base| path.strip_prefix(base).ok())
                        .unwrap_or(path)
                        .to_string_lossy()
                        .into_owned()
                })
                .collect::<Vec<_>>()
                .join("\n");
            self.append_agent_section("Files changed", &[(None, &files, 8)]);
        }

        let conversation = gtk::Box::new(gtk::Orientation::Vertical, 12);
        conversation.set_margin_top(12);
        if preview.is_truncated {
            let note = gtk::Label::new(Some("Earlier messages are left out."));
            note.set_xalign(0.0);
            note.add_css_class("dim-label");
            conversation.append(&note);
        }
        for message in &preview.messages {
            conversation.append(&agent_message_widget(
                match message.role {
                    ConversationRole::User => "You",
                    ConversationRole::Assistant => "Agent",
                },
                &message.text,
                0,
            ));
        }
        let expander = gtk::Expander::builder()
            .label("Show conversation")
            .child(&conversation)
            .build();
        preview_box.append(&expander);
        self.agents.restore_button.set_sensitive(true);
    }

    fn append_agent_section(&self, heading: &str, entries: &[(Option<&str>, &str, i32)]) {
        let section = gtk::Box::new(gtk::Orientation::Vertical, 6);
        let label = gtk::Label::new(Some(heading));
        label.set_xalign(0.0);
        label.add_css_class("heading");
        section.append(&label);
        for (role, text, lines) in entries {
            match role {
                Some(role) => section.append(&agent_message_widget(role, text, *lines)),
                None => section.append(&agent_text(text, *lines)),
            }
        }
        self.agents.preview.append(&section);
    }

    fn clear_agent_preview(&self) {
        while let Some(child) = self.agents.preview.first_child() {
            if let Some(title_row) = child.downcast_ref::<gtk::Box>() {
                // The title label lives on; only its row is rebuilt.
                if self.agents.title.parent().as_ref() == Some(title_row.upcast_ref()) {
                    title_row.remove(&self.agents.title);
                }
            }
            self.agents.preview.remove(&child);
        }
    }

    fn reset_agent_destinations(&self, session: &ProviderSession) {
        let item_count = self.agents.restore_destination_choices.n_items();
        self.agents
            .restore_destination_choices
            .splice(0, item_count, &[]);
        self.agents.restore_destination_values.borrow_mut().clear();
        self.agents.restore_destination.set_subtitle("");
        let project_root = self
            .agents
            .sessions
            .borrow()
            .iter()
            .find(|item| same_agent_session(&item.session, session))
            .and_then(|item| item.project_root.clone());
        let add = |label: String, value: AgentRestoreDestination| {
            self.agents.restore_destination_choices.append(&label);
            self.agents
                .restore_destination_values
                .borrow_mut()
                .push(value);
        };
        match session.cwd.as_ref() {
            Some(cwd) if cwd.exists() => {
                let branch = session
                    .branch
                    .as_deref()
                    .map(|branch| format!(" ({branch})"))
                    .unwrap_or_default();
                add(
                    format!("{}{branch}", compact_agent_path(cwd)),
                    AgentRestoreDestination::LastKnown,
                );
            }
            Some(cwd) => {
                self.agents
                    .restore_destination
                    .set_subtitle(&format!("{} no longer exists", compact_agent_path(cwd)));
                if let Some(root) = project_root.filter(|root| root.exists()) {
                    add(
                        format!("Project root {}", compact_agent_path(&root)),
                        AgentRestoreDestination::ExistingWorktree(root),
                    );
                }
            }
            None => add(
                "Last known location".to_owned(),
                AgentRestoreDestination::LastKnown,
            ),
        }
        add(
            "New worktree…".to_owned(),
            AgentRestoreDestination::NewWorktree,
        );
        add(
            "Other directory…".to_owned(),
            AgentRestoreDestination::CustomDirectory,
        );
        self.agents.restore_destination.set_selected(0);
        self.update_agent_destination_inputs();
    }

    fn load_agent_worktrees(&self, session: ProviderSession) {
        let Some(cwd) = session.cwd.clone().filter(|cwd| cwd.exists()) else {
            return;
        };
        let manager = crate::worktrees::WorktreeManager::new(self.paths.attic_dir());
        self.run_slow(
            move || {
                let root = manager.repository_root(&cwd)?;
                manager.linked_paths(&root)
            },
            move |workspace, result| {
                let Some(selected) = workspace.agents.selected.borrow().clone() else {
                    return;
                };
                if !same_agent_session(&selected, &session) {
                    return;
                }
                let Ok(paths) = result else {
                    return;
                };
                let paths = paths
                    .into_iter()
                    .filter(|path| Some(path) != selected.cwd.as_ref())
                    .collect::<Vec<_>>();
                if paths.is_empty() {
                    return;
                }
                let labels = paths
                    .iter()
                    .map(|path| format!("Worktree {}", compact_agent_path(path)))
                    .collect::<Vec<_>>();
                let labels = labels.iter().map(String::as_str).collect::<Vec<_>>();
                workspace
                    .agents
                    .restore_destination_choices
                    .splice(1, 0, &labels);
                workspace
                    .agents
                    .restore_destination_values
                    .borrow_mut()
                    .splice(
                        1..1,
                        paths
                            .into_iter()
                            .map(AgentRestoreDestination::ExistingWorktree),
                    );
            },
        );
    }

    pub(super) fn update_agent_destination_inputs(&self) {
        let destination = self
            .agents
            .restore_destination_values
            .borrow()
            .get(self.agents.restore_destination.selected() as usize)
            .cloned()
            .unwrap_or(AgentRestoreDestination::LastKnown);
        let show_branch = matches!(destination, AgentRestoreDestination::NewWorktree);
        let show_custom = matches!(destination, AgentRestoreDestination::CustomDirectory);
        self.agents.restore_branch.set_visible(show_branch);
        self.agents.restore_custom_cwd.set_visible(show_custom);
        self.agents.restore_button.set_label(match destination {
            AgentRestoreDestination::NewWorktree => "Resume in New Worktree",
            _ => "Resume",
        });
    }

    fn render_agent_list_message(&self, text: &str) {
        while let Some(child) = self.agents.list.first_child() {
            self.agents.list.remove(&child);
        }
        self.agents.rows.borrow_mut().clear();
        let label = gtk::Label::new(Some(text));
        label.set_xalign(0.0);
        label.set_margin_start(12);
        label.set_margin_top(12);
        label.add_css_class("dim-label");
        self.agents.list.append(&label);
    }

    fn render_agent_preview_message(&self, text: &str) {
        self.clear_agent_preview();
        let label = gtk::Label::new(Some(text));
        label.set_xalign(0.0);
        label.add_css_class("dim-label");
        self.agents.preview.append(&label);
    }

    fn respond_agent_result<T: serde::Serialize>(
        &self,
        pending: PendingRequest,
        message: &'static str,
        result: PersistResult<T>,
    ) {
        match result.and_then(|value| serde_json::to_value(value).map_err(Into::into)) {
            Ok(value) => {
                let id = pending.request.id.clone();
                let _ = pending.respond(ControlResponse::success(id, value));
            }
            Err(error) => self.report_failure(
                Some(pending),
                ErrorCode::OperationRefused,
                message,
                error.to_string(),
            ),
        }
    }
}

fn agent_text(text: &str, lines: i32) -> gtk::Label {
    let label = gtk::Label::new(Some(text));
    label.set_xalign(0.0);
    label.set_wrap(true);
    label.set_wrap_mode(gtk::pango::WrapMode::WordChar);
    label.set_selectable(true);
    if lines > 0 {
        label.set_lines(lines);
        label.set_ellipsize(gtk::pango::EllipsizeMode::End);
    }
    label
}

fn agent_message_widget(role: &str, text: &str, lines: i32) -> gtk::Box {
    let heading = gtk::Label::new(Some(role));
    heading.set_xalign(0.0);
    heading.add_css_class("caption-heading");
    heading.add_css_class(if role == "You" { "accent" } else { "dim-label" });
    let entry = gtk::Box::new(gtk::Orientation::Vertical, 2);
    entry.append(&heading);
    entry.append(&agent_text(text, lines));
    entry
}

fn provider_label(provider: AgentProvider) -> &'static str {
    match provider {
        AgentProvider::Claude => "Claude",
        AgentProvider::Codex => "Codex",
    }
}

fn sorted<'a>(values: impl Iterator<Item = &'a String>) -> Vec<&'a String> {
    let mut values = values.collect::<Vec<_>>();
    values.sort();
    values
}

fn entry_path(entry: &adw::EntryRow) -> Option<PathBuf> {
    let text = entry.text();
    let text = text.trim();
    (!text.is_empty()).then(|| PathBuf::from(text))
}

fn same_agent_session(left: &ProviderSession, right: &ProviderSession) -> bool {
    left.provider == right.provider && left.provider_session_id == right.provider_session_id
}

pub(super) fn agent_session_key(session: &ProviderSession) -> String {
    format!(
        "{}:{}",
        session.provider.command(),
        session.provider_session_id
    )
}

fn compact_agent_path(path: &Path) -> String {
    let value = path.to_string_lossy();
    let value = match std::env::var("HOME") {
        Ok(home) if !home.is_empty() && value.starts_with(&home) => {
            format!("~{}", &value[home.len()..])
        }
        _ => value.into_owned(),
    };
    if value.chars().count() <= 56 {
        return value;
    }
    let suffix = value
        .chars()
        .rev()
        .take(53)
        .collect::<String>()
        .chars()
        .rev()
        .collect::<String>();
    format!("…{suffix}")
}

fn agent_provider(kind: SessionKind) -> Option<AgentProvider> {
    match kind {
        SessionKind::Claude => Some(AgentProvider::Claude),
        SessionKind::Codex => Some(AgentProvider::Codex),
        _ => None,
    }
}

/// An agent in these states has no turn in flight, so a restart loses nothing.
fn is_between_turns(state: SessionState) -> bool {
    matches!(state, SessionState::Idle | SessionState::Ready)
}

#[cfg(test)]
mod tests {
    use super::{
        AgentSessionItem, UnclaimedAgent, agent_day_heading, agent_location_label, agent_row_time,
        agent_worktree_name, classify_agent_sessions, days_between, filter_agent_sessions,
        match_unclaimed_agents, restore_target_for_location,
    };
    use crate::providers::{AgentProvider, ProviderSession};
    use crate::worktrees::WorktreeManager;
    use std::collections::HashMap;
    use std::fs;
    use std::path::PathBuf;
    use std::process::Command;

    fn session(name: &str, provider_session_id: &str, cwd: &str) -> ProviderSession {
        ProviderSession {
            provider: AgentProvider::Codex,
            provider_session_id: provider_session_id.to_owned(),
            name: name.to_owned(),
            custom_title: None,
            ai_title: None,
            first_prompt: None,
            last_prompt: None,
            branch: None,
            prompt_count: 0,
            cwd: Some(PathBuf::from(cwd)),
            created_at: 1_000,
            last_seen_at: 2_000,
            log_path: PathBuf::from("/tmp/session.jsonl"),
        }
    }

    fn item(
        name: &str,
        provider_session_id: &str,
        cwd: &str,
        project_root: Option<&str>,
    ) -> AgentSessionItem {
        AgentSessionItem {
            session: session(name, provider_session_id, cwd),
            project_root: project_root.map(PathBuf::from),
            worktree_path: Some(PathBuf::from(cwd)),
        }
    }

    #[test]
    fn filtering_matches_original_session_card_fields() {
        let sessions = vec![
            item(
                "Review the launch flow",
                "codex-review",
                "/work/agtk-feature",
                Some("/work/agtk"),
            ),
            item(
                "Fix the terminal",
                "codex-terminal",
                "/work/other",
                Some("/work/other"),
            ),
        ];

        assert_eq!(
            filter_agent_sessions(&sessions, None, "launch", &HashMap::new()).len(),
            1
        );
        assert_eq!(
            filter_agent_sessions(&sessions, None, "terminal", &HashMap::new()).len(),
            1
        );
        assert_eq!(
            filter_agent_sessions(&sessions, None, "agtk", &HashMap::new()).len(),
            1
        );
        assert_eq!(
            filter_agent_sessions(&sessions, None, "codex-review", &HashMap::new()).len(),
            1
        );
        assert_eq!(
            filter_agent_sessions(&sessions, None, "missing", &HashMap::new()).len(),
            0
        );
    }

    #[test]
    fn worktree_name_is_the_full_directory_name() {
        let session = item(
            "Review the launch flow",
            "codex-review",
            "/work/agtk-feature-with-a-long-descriptive-name",
            Some("/work/agtk"),
        );

        assert_eq!(
            agent_worktree_name(&session).as_deref(),
            Some("agtk-feature-with-a-long-descriptive-name")
        );
    }

    #[test]
    fn filtering_matches_the_resolved_worktree_name() {
        let mut session = item(
            "Review the launch flow",
            "codex-review",
            "/tmp/session-location",
            Some("/work/agtk"),
        );
        session.worktree_path = Some(PathBuf::from("/worktrees/native-session-picker"));

        let filtered = filter_agent_sessions(&[session], None, "session-picker", &HashMap::new());

        assert_eq!(filtered.len(), 1);
    }

    #[test]
    fn filtering_limits_results_to_the_selected_project() {
        let sessions = vec![
            item(
                "Review the launch flow",
                "codex-review",
                "/work/agtk-feature",
                Some("/work/agtk"),
            ),
            item(
                "Fix the terminal",
                "codex-terminal",
                "/work/other",
                Some("/work/other"),
            ),
        ];

        let filtered = filter_agent_sessions(
            &sessions,
            Some(PathBuf::from("/work/agtk").as_path()),
            "",
            &HashMap::new(),
        );

        assert_eq!(filtered.len(), 1);
        assert_eq!(filtered[0].session.provider_session_id, "codex-review");
    }

    #[test]
    fn classification_prefers_the_most_specific_known_project_root() {
        let sessions = vec![session(
            "Nested session",
            "codex-nested",
            "/work/agtk/crates/native",
        )];
        let roots = vec![PathBuf::from("/work"), PathBuf::from("/work/agtk")];
        let manager = WorktreeManager::new(PathBuf::from("/tmp/unused-attic"));

        let classified = classify_agent_sessions(sessions, &roots, &manager);

        assert_eq!(
            classified[0].project_root,
            Some(PathBuf::from("/work/agtk"))
        );
    }

    #[test]
    fn running_agents_claim_the_first_log_after_their_launch() {
        let log = |id: &str, cwd: &str, created_at: u64| {
            let mut item = item(id, id, cwd, Some("/work"));
            item.session.created_at = created_at;
            item
        };
        let items = vec![
            log("older", "/work/a", 1_000),
            log("second", "/work/a", 60_000),
            log("first", "/work/a", 30_000),
            log("elsewhere", "/work/b", 30_000),
        ];
        let agent = |id: &str, launched_at: u64| UnclaimedAgent {
            session_id: id.to_owned(),
            provider: AgentProvider::Codex,
            cwd: PathBuf::from("/work/a"),
            launched_at,
        };

        let matches =
            match_unclaimed_agents(&[agent("late", 50_000), agent("early", 20_000)], &items);

        assert_eq!(matches, [("early".to_owned(), 2), ("late".to_owned(), 1)]);
    }

    #[test]
    fn filtering_needs_every_word_and_matches_names_and_prompts() {
        let mut session = item(
            "Changes sidebar resizable",
            "a",
            "/work/agtk",
            Some("/work/agtk"),
        );
        session.session.first_prompt = Some("Make the right sidebar draggable".to_owned());
        session.session.branch = Some("main".to_owned());
        let sessions = [session];
        let names = HashMap::from([("codex:a".to_owned(), "Wide panel".to_owned())]);

        assert_eq!(
            filter_agent_sessions(&sessions, None, "sidebar draggable", &names).len(),
            1
        );
        assert_eq!(
            filter_agent_sessions(&sessions, None, "wide main", &names).len(),
            1
        );
        assert_eq!(
            filter_agent_sessions(&sessions, None, "sidebar modal", &names).len(),
            0
        );
    }

    #[test]
    fn location_leaves_out_the_main_checkout_and_default_branch() {
        let mut main = item("A", "a", "/work/agtk", Some("/work/agtk"));
        main.session.branch = Some("main".to_owned());
        assert_eq!(agent_location_label(&main), "agtk");

        let mut feature = item("B", "b", "/work/agtk-cleanup", Some("/work/agtk"));
        feature.session.branch = Some("cleanup-euw".to_owned());
        assert_eq!(agent_location_label(&feature), "agtk · cleanup-euw");

        let detached = item("C", "c", "/work/agtk-review", Some("/work/agtk"));
        assert_eq!(agent_location_label(&detached), "agtk · agtk-review");
    }

    #[test]
    fn days_count_calendar_days_and_pick_headings() {
        let local = |day, hour| glib::DateTime::from_local(2026, 9, day, hour, 30, 0.0).unwrap();
        let now = local(29, 9);
        assert_eq!(days_between(&local(28, 23), &now), 1);
        assert_eq!(days_between(&local(29, 1), &now), 0);
        assert_eq!(agent_day_heading(0, &now), "Today");
        assert_eq!(agent_day_heading(1, &local(28, 23)), "Yesterday");
        assert_eq!(agent_day_heading(4, &local(25, 10)), "Friday");
        assert_eq!(agent_day_heading(9, &local(20, 10)), "Older");
        assert_eq!(agent_row_time(4, &local(25, 10)), "10:30");
        assert_eq!(agent_row_time(9, &local(20, 10)), "Sep 20");
    }

    #[test]
    fn restored_git_locations_keep_project_and_worktree_metadata() {
        let fixture = tempfile::tempdir().unwrap();
        let root = fixture.path().join("project");
        let nested = root.join("src/deep");
        fs::create_dir_all(&nested).unwrap();
        assert!(
            Command::new("git")
                .args(["init", "-b", "main"])
                .current_dir(&root)
                .status()
                .unwrap()
                .success()
        );
        let manager = WorktreeManager::new(fixture.path().join("attic"));

        let target = restore_target_for_location(
            Some(nested.clone()),
            None,
            std::slice::from_ref(&root),
            &manager,
        );

        assert_eq!(target.cwd, Some(nested));
        assert_eq!(target.project_root, Some(root.clone()));
        assert_eq!(target.worktree_path, Some(root));
    }

    #[test]
    fn restored_sessions_keep_the_project_selected_by_the_dialog() {
        let fixture = tempfile::tempdir().unwrap();
        let project = fixture.path().join("project");
        let destination = fixture.path().join("external-worktree");
        fs::create_dir_all(&project).unwrap();
        fs::create_dir_all(&destination).unwrap();
        let manager = WorktreeManager::new(fixture.path().join("attic"));

        let target = restore_target_for_location(
            Some(destination.clone()),
            Some(project.clone()),
            &[],
            &manager,
        );

        assert_eq!(target.cwd, Some(destination));
        assert_eq!(target.project_root, Some(project));
    }

    #[test]
    fn restored_non_git_locations_still_receive_project_metadata() {
        let fixture = tempfile::tempdir().unwrap();
        let root = fixture.path().join("project");
        let nested = root.join("src/deep");
        fs::create_dir_all(&nested).unwrap();
        let manager = WorktreeManager::new(fixture.path().join("attic"));

        let known = restore_target_for_location(
            Some(nested.clone()),
            None,
            std::slice::from_ref(&root),
            &manager,
        );
        let standalone = restore_target_for_location(Some(nested.clone()), None, &[], &manager);

        assert_eq!(known.project_root, Some(root));
        assert_eq!(known.worktree_path, None);
        assert_eq!(standalone.project_root, Some(nested));
        assert_eq!(standalone.worktree_path, None);
    }
}
