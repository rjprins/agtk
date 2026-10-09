use std::collections::{BTreeMap, BTreeSet};
use std::path::Path;

use super::*;
use crate::projects::ProjectSettings;

pub(super) const INACTIVE_PROJECTS_PREFERENCE: &str = "inactiveProjectsExpanded";

impl Workspace {
    pub(super) fn load_projects(&self, preferences: ProjectPreferences) {
        *self.projects.borrow_mut() = preferences;
        self.rebuild_sidebar();
    }

    pub(super) fn set_project_preferences(
        &self,
        root: String,
        is_pinned: Option<bool>,
        is_collapsed: Option<bool>,
        pending: Option<PendingRequest>,
    ) {
        // A change before the workspace loads would be replaced by the saved settings.
        if self.store.borrow().is_none() {
            self.report_projects_loading(pending);
            return;
        }
        self.projects
            .borrow_mut()
            .set(&root, is_pinned, is_collapsed);
        self.rebuild_sidebar();
        self.save_projects(pending);
    }

    /// Lists the project a session runs in, so it stays listed once its sessions close.
    pub(super) fn remember_project(&self, root: Option<&Path>) {
        let Some(root) = root else {
            return;
        };
        if self
            .projects
            .borrow_mut()
            .remember(root.to_string_lossy().as_ref())
        {
            self.save_projects(None);
        }
    }

    /// Forgets a project without sessions, so Inactive Projects no longer lists it.
    pub(super) fn remove_project(&self, root: &str, pending: Option<PendingRequest>) {
        if self.store.borrow().is_none() {
            self.report_projects_loading(pending);
            return;
        }
        let has_sessions = self.sessions.borrow().values().any(|session| {
            session
                .record
                .project_root
                .as_ref()
                .is_some_and(|project| project.to_string_lossy() == root)
        });
        if has_sessions {
            self.report_failure(
                pending,
                ErrorCode::OperationRefused,
                "Could not remove project",
                format!("{root} still has sessions; close them first"),
            );
            return;
        }
        if !self.projects.borrow_mut().remove(root) {
            self.report_failure(
                pending,
                ErrorCode::InvalidParams,
                "No project is listed with that root",
                root.to_owned(),
            );
            return;
        }
        self.rebuild_sidebar();
        self.save_projects(pending);
    }

    fn set_inactive_projects_expanded(&self, expanded: bool) {
        self.inactive_projects_expanded.set(expanded);
        self.save_preference(INACTIVE_PROJECTS_PREFERENCE, serde_json::json!(expanded));
        self.rebuild_sidebar();
    }

    fn report_projects_loading(&self, pending: Option<PendingRequest>) {
        self.report_failure(
            pending,
            ErrorCode::InternalError,
            "Workspace is loading",
            "project settings are not available yet".to_owned(),
        );
    }

    /// Saves the projects as they are now, then answers `pending` with all of them.
    fn save_projects(&self, pending: Option<PendingRequest>) {
        let Some(store) = self.store.borrow().clone() else {
            self.report_projects_loading(pending);
            return;
        };
        let value = match serde_json::to_value(&*self.projects.borrow()) {
            Ok(value) => value,
            Err(error) => {
                self.report_launch_failure(
                    pending,
                    "Could not encode project settings",
                    error.to_string(),
                );
                return;
            }
        };
        self.run_io(
            move || store.set_preference("projects", &value),
            move |workspace, result| match result {
                Ok(()) => {
                    if let Some(pending) = pending {
                        let id = pending.request.id.clone();
                        match serde_json::to_value(workspace.project_summaries()) {
                            Ok(projects) => {
                                let _ = pending.respond(ControlResponse::success(
                                    id,
                                    serde_json::json!({ "projects": projects }),
                                ));
                            }
                            Err(error) => respond_failure(
                                pending,
                                ErrorCode::InternalError,
                                "Could not describe projects",
                                Some(serde_json::json!({ "reason": error.to_string() })),
                            ),
                        }
                    }
                }
                Err(error) => workspace.report_launch_failure(
                    pending,
                    "Could not save project settings",
                    error.to_string(),
                ),
            },
        );
    }

    /// Every listed project and every project with sessions, the active ones first.
    pub(super) fn project_summaries(&self) -> Vec<crate::control::ProjectSummary> {
        let preferences = self.projects.borrow();
        let with_sessions = self
            .sessions
            .borrow()
            .values()
            .filter_map(|session| session.record.project_root.as_ref())
            .map(|root| root.to_string_lossy().to_string())
            .collect::<BTreeSet<_>>();
        let roots = preferences
            .projects
            .keys()
            .chain(&with_sessions)
            .collect::<BTreeSet<_>>();
        let mut projects = roots
            .into_iter()
            .map(|root| {
                let settings = preferences.get(root);
                crate::control::ProjectSummary {
                    name: project_name(root),
                    root: root.clone(),
                    is_pinned: settings.is_pinned,
                    is_collapsed: settings.is_collapsed,
                    is_active: settings.is_pinned || with_sessions.contains(root),
                }
            })
            .collect::<Vec<_>>();
        projects.sort_by(|left, right| {
            right
                .is_active
                .cmp(&left.is_active)
                .then_with(|| right.is_pinned.cmp(&left.is_pinned))
                .then_with(|| left.name.cmp(&right.name))
                .then_with(|| left.root.cmp(&right.root))
        });
        projects
    }

    pub(super) fn worktree_group_summaries(&self) -> Vec<crate::control::WorktreeGroupSummary> {
        let mut groups = BTreeMap::<(String, String), Vec<String>>::new();
        for (id, session) in self.sessions.borrow().iter() {
            let (Some(project), Some(worktree)) = (
                session.record.project_root.as_ref(),
                session.record.worktree_path.as_ref(),
            ) else {
                continue;
            };
            groups
                .entry((
                    project.to_string_lossy().to_string(),
                    worktree.to_string_lossy().to_string(),
                ))
                .or_default()
                .push(id.clone());
        }
        groups
            .into_iter()
            .map(|((project_root, path), mut session_ids)| {
                session_ids.sort();
                crate::control::WorktreeGroupSummary {
                    project_root,
                    branch: project_name(&path),
                    path,
                    session_ids,
                }
            })
            .collect()
    }

    pub(super) fn bind_sidebar_model(&self) {
        let workspace = self.clone();
        self.list
            .bind_model(Some(&self.sidebar_model), move |item| {
                let key = item
                    .downcast_ref::<gtk::StringObject>()
                    .map(|key| key.string().to_string())
                    .unwrap_or_default();
                workspace.sidebar_row(&key).upcast()
            });
        let workspace = self.clone();
        self.inactive_list
            .bind_model(Some(&self.inactive_sidebar_model), move |item| {
                let key = item
                    .downcast_ref::<gtk::StringObject>()
                    .map(|key| key.string().to_string())
                    .unwrap_or_default();
                workspace.sidebar_row(&key).upcast()
            });
    }

    /// Builds the row for a key; session rows are the long-lived rows the sessions own.
    fn sidebar_row(&self, key: &str) -> gtk::ListBoxRow {
        match SidebarKey::parse(key) {
            Some(SidebarKey::Session(id)) => self
                .sessions
                .borrow()
                .get(id)
                .map(|session| session.row.clone())
                .unwrap_or_default(),
            Some(SidebarKey::Project {
                root,
                name,
                settings,
                pr_attention,
            }) => self.project_header(root, name, settings, pr_attention),
            Some(SidebarKey::Inactive { count, is_expanded }) => {
                self.inactive_projects_header(count, is_expanded)
            }
            Some(SidebarKey::InactiveProject(root)) => self.inactive_project_row(root),
            Some(SidebarKey::Other) | None => group_label("Other"),
        }
    }

    pub(super) fn rebuild_sidebar(&self) {
        let projects = self.project_summaries();
        self.menus.worktrees.set_enabled(!projects.is_empty());
        let (projects, inactive): (Vec<_>, Vec<_>) =
            projects.into_iter().partition(|project| project.is_active);
        let roots = projects
            .iter()
            .map(|project| project.root.clone())
            .collect::<Vec<_>>();
        let mut keys = Vec::new();
        {
            let sessions = self.sessions.borrow();
            for project in projects {
                let settings = ProjectSettings {
                    is_pinned: project.is_pinned,
                    is_collapsed: project.is_collapsed,
                };
                keys.push(SidebarKey::project_key(
                    &project.root,
                    &project.name,
                    settings,
                    self.project_pr_attention(&project.root),
                ));
                let mut project_sessions = sessions
                    .values()
                    .filter(|session| {
                        session.record.project_root.as_ref().is_some_and(|root| {
                            root.to_string_lossy().as_ref() == project.root.as_str()
                        })
                    })
                    .collect::<Vec<_>>();
                project_sessions.sort_by_key(|session| session.record.position);
                for session in project_sessions {
                    session.row.set_visible(!project.is_collapsed);
                    keys.push(SidebarKey::session_key(&session.record.id));
                }
            }
            let mut ungrouped = sessions
                .values()
                .filter(|session| session.record.project_root.is_none())
                .collect::<Vec<_>>();
            if !ungrouped.is_empty() {
                keys.push(SidebarKey::OTHER.to_owned());
                ungrouped.sort_by_key(|session| session.record.position);
                for session in ungrouped {
                    session.row.set_visible(true);
                    keys.push(SidebarKey::session_key(&session.record.id));
                }
            }
        }
        let mut inactive_keys = Vec::new();
        if !inactive.is_empty() {
            let is_expanded = self.inactive_projects_expanded.get();
            inactive_keys.push(SidebarKey::inactive_key(inactive.len(), is_expanded));
            if is_expanded {
                inactive_keys.extend(
                    inactive
                        .iter()
                        .map(|project| SidebarKey::inactive_project_key(&project.root)),
                );
            }
        }
        self.splice_sidebar(&keys);
        self.splice_inactive_sidebar(&inactive_keys);

        let selected = self.selected_session_id().and_then(|id| {
            self.sessions
                .borrow()
                .get(&id)
                .map(|session| session.row.clone())
        });
        if let Some(row) = selected
            && self.list.selected_row().as_ref() != Some(&row)
        {
            self.list.select_row(Some(&row));
        }
        self.refresh_launch_project_choices();
        if self.has_unchecked_projects(&roots) {
            self.poll_pull_requests();
        }
    }

    /// Whether a dragged session may land on `target`: only within its own group.
    pub(super) fn can_reorder_session(&self, dragged: &str, target: &str) -> bool {
        let sessions = self.sessions.borrow();
        match (sessions.get(dragged), sessions.get(target)) {
            (Some(dragged), Some(target)) => {
                dragged.record.project_root == target.record.project_root
            }
            _ => false,
        }
    }

    /// Moves `dragged` before or after `target` in the sidebar and saves the new order.
    pub(super) fn reorder_session(&self, dragged: &str, target: &str, before: bool) -> bool {
        if !self.can_reorder_session(dragged, target) {
            return false;
        }
        let changed = {
            let sessions = self.sessions.borrow();
            let root = sessions[dragged].record.project_root.clone();
            let mut group = sessions
                .values()
                .filter(|session| session.record.project_root == root)
                .map(|session| (session.record.id.clone(), session.record.position))
                .collect::<Vec<_>>();
            group.sort_by(|left, right| (left.1, &left.0).cmp(&(right.1, &right.0)));
            super::sidebar::reordered_positions(&group, dragged, target, before)
        };
        if changed.is_empty() {
            return true;
        }
        let mut records = Vec::new();
        {
            let mut sessions = self.sessions.borrow_mut();
            for (id, position) in changed {
                if let Some(session) = sessions.get_mut(&id) {
                    session.record.position = position;
                    records.push(session.record.clone());
                }
            }
        }
        for record in records {
            self.persist_record(record);
        }
        self.rebuild_sidebar();
        true
    }

    /// Moves every row after `id` down one and returns the position straight
    /// below it, or `None` once `id` is gone.
    pub(super) fn make_room_below(&self, id: &str) -> Option<i64> {
        let (position, moved) = {
            let mut sessions = self.sessions.borrow_mut();
            let position = sessions.get(id)?.record.position;
            let moved = sessions
                .values_mut()
                .filter(|session| session.record.position > position)
                .map(|session| {
                    session.record.position += 1;
                    session.record.clone()
                })
                .collect::<Vec<_>>();
            (position, moved)
        };
        for record in moved {
            self.persist_record(record);
        }
        Some(position + 1)
    }

    /// Replaces only the changed middle of the key list, so untouched rows keep focus.
    fn splice_sidebar(&self, keys: &[String]) {
        Self::splice_model(&self.sidebar_model, keys);
    }

    fn splice_inactive_sidebar(&self, keys: &[String]) {
        Self::splice_model(&self.inactive_sidebar_model, keys);
    }

    fn splice_model(model: &gio::ListStore, keys: &[String]) {
        let current = (0..model.n_items())
            .filter_map(|index| model.item(index).and_downcast::<gtk::StringObject>())
            .map(|key| key.string().to_string())
            .collect::<Vec<_>>();
        let (position, removed, added) = changed_range(&current, keys);
        if removed == 0 && added.is_empty() {
            return;
        }
        let added = added
            .iter()
            .map(|key| gtk::StringObject::new(key))
            .collect::<Vec<_>>();
        model.splice(position as u32, removed as u32, &added);
    }

    fn project_header(
        &self,
        root: &str,
        name: &str,
        settings: ProjectSettings,
        pr_attention: Option<usize>,
    ) -> gtk::ListBoxRow {
        let row = gtk::ListBoxRow::new();
        row.set_selectable(false);
        row.set_activatable(false);
        let content = gtk::Box::new(gtk::Orientation::Horizontal, 3);
        // GtkButton's `icon_name` child replaces its label child. Build the
        // compact project control explicitly so the chevron and project name
        // remain visible together.
        let collapse = gtk::Button::new();
        let collapse_contents = gtk::Box::new(gtk::Orientation::Horizontal, 4);
        collapse_contents.set_hexpand(true);
        let collapse_icon = gtk::Image::from_icon_name(if settings.is_collapsed {
            "pan-end-symbolic"
        } else {
            "pan-down-symbolic"
        });
        collapse_icon.set_pixel_size(14);
        let collapse_label = gtk::Label::new(Some(name));
        collapse_label.set_xalign(0.0);
        collapse_label.set_hexpand(true);
        collapse_label.set_ellipsize(gtk::pango::EllipsizeMode::End);
        collapse_label.add_css_class("heading");
        collapse_contents.append(&collapse_icon);
        collapse_contents.append(&collapse_label);
        if settings.is_pinned {
            let pinned = gtk::Image::from_icon_name("starred-symbolic");
            pinned.add_css_class("dim-label");
            pinned.set_tooltip_text(Some("Pinned project"));
            collapse_contents.append(&pinned);
        }
        collapse.set_child(Some(&collapse_contents));
        collapse.add_css_class("flat");
        collapse.set_hexpand(true);
        collapse.set_halign(gtk::Align::Fill);
        collapse.set_tooltip_text(Some(root));
        let collapse_workspace = self.clone();
        let collapse_root = root.to_owned();
        collapse.connect_clicked(move |_| {
            collapse_workspace.set_project_preferences(
                collapse_root.clone(),
                None,
                Some(!settings.is_collapsed),
                None,
            );
        });
        content.append(&collapse);
        let launch = gtk::Button::builder()
            .icon_name("list-add-symbolic")
            .build();
        launch.add_css_class("flat");
        launch.set_tooltip_text(Some("Launch in this project"));
        let launch_workspace = self.clone();
        let launch_root = root.to_owned();
        launch.connect_clicked(move |_| launch_workspace.open_launch_for_project(&launch_root));
        let menu = gio::Menu::new();
        let surfaces = gio::Menu::new();
        surfaces.append_item(&menus::targeted_item(
            "Resume Recent Session…",
            "win.project-resume",
            root,
        ));
        surfaces.append_item(&menus::targeted_item(
            "Worktrees",
            "win.project-worktrees",
            root,
        ));
        surfaces.append_item(&menus::targeted_item(
            "Pull Requests",
            "win.project-pull-requests",
            root,
        ));
        menu.append_section(None, &surfaces);
        let pin = gio::Menu::new();
        pin.append_item(&menus::targeted_item(
            if settings.is_pinned {
                "Unpin Project"
            } else {
                "Pin Project"
            },
            "win.project-pin",
            root,
        ));
        menu.append_section(None, &pin);
        let more = gtk::MenuButton::builder()
            .icon_name("view-more-symbolic")
            .menu_model(&menu)
            .valign(gtk::Align::Center)
            .tooltip_text("Project actions")
            .build();
        more.add_css_class("flat");
        launch.set_valign(gtk::Align::Center);
        if let Some(attention) = pr_attention {
            content.append(&self.project_pr_button(root, attention));
        }
        content.append(&self.project_repository_link(root));
        content.append(&launch);
        content.append(&more);
        menus::open_menu_on_right_click(&collapse, &more);
        row.set_child(Some(&content));
        row
    }

    /// Heads the projects without sessions; the list under it starts collapsed.
    fn inactive_projects_header(&self, count: usize, is_expanded: bool) -> gtk::ListBoxRow {
        let row = gtk::ListBoxRow::new();
        row.set_selectable(false);
        row.set_activatable(false);
        let contents = gtk::Box::new(gtk::Orientation::Horizontal, 4);
        contents.add_css_class("dim-label");
        let icon = gtk::Image::from_icon_name(if is_expanded {
            "pan-down-symbolic"
        } else {
            "pan-end-symbolic"
        });
        icon.set_pixel_size(14);
        let label = gtk::Label::new(Some("Inactive Projects"));
        label.set_xalign(0.0);
        label.set_hexpand(true);
        label.set_ellipsize(gtk::pango::EllipsizeMode::End);
        label.add_css_class("heading");
        let count_label = gtk::Label::new(Some(&count.to_string()));
        count_label.add_css_class("numeric");
        contents.append(&icon);
        contents.append(&label);
        contents.append(&count_label);
        let toggle = gtk::Button::builder()
            .child(&contents)
            .hexpand(true)
            .tooltip_text(if is_expanded {
                "Hide projects without sessions"
            } else {
                "Show projects without sessions"
            })
            .build();
        toggle.add_css_class("flat");
        toggle.update_state(&[gtk::accessible::State::Expanded(Some(is_expanded))]);
        let workspace = self.clone();
        toggle.connect_clicked(move |_| workspace.set_inactive_projects_expanded(!is_expanded));
        row.set_child(Some(&toggle));
        row
    }

    /// Opening an inactive project launches in it; pinning keeps it among the active ones.
    fn inactive_project_row(&self, root: &str) -> gtk::ListBoxRow {
        let row = gtk::ListBoxRow::new();
        row.set_selectable(false);
        row.set_activatable(false);
        let content = gtk::Box::new(gtk::Orientation::Horizontal, 3);
        let open_contents = gtk::Box::new(gtk::Orientation::Horizontal, 10);
        let icon = gtk::Image::from_icon_name("folder-symbolic");
        icon.add_css_class("dim-label");
        let label = gtk::Label::new(Some(&project_name(root)));
        label.set_xalign(0.0);
        label.set_hexpand(true);
        label.set_ellipsize(gtk::pango::EllipsizeMode::End);
        // Button labels are bold, which would make the project read as a header.
        label.add_css_class("body");
        open_contents.append(&icon);
        open_contents.append(&label);
        let open = gtk::Button::builder()
            .child(&open_contents)
            .hexpand(true)
            .tooltip_text(format!("Launch in {root}"))
            .build();
        open.add_css_class("flat");
        let workspace = self.clone();
        let open_root = root.to_owned();
        open.connect_clicked(move |_| workspace.open_launch_for_project(&open_root));
        let pin = gtk::Button::builder()
            .icon_name("non-starred-symbolic")
            .valign(gtk::Align::Center)
            .tooltip_text("Pin project")
            .build();
        pin.add_css_class("flat");
        let workspace = self.clone();
        let pin_root = root.to_owned();
        pin.connect_clicked(move |_| {
            workspace.set_project_preferences(pin_root.clone(), Some(true), None, None);
        });
        let remove = gtk::Button::builder()
            .icon_name("user-trash-symbolic")
            .valign(gtk::Align::Center)
            .tooltip_text("Remove from list")
            .build();
        remove.add_css_class("flat");
        let workspace = self.clone();
        let remove_root = root.to_owned();
        remove.connect_clicked(move |_| workspace.remove_project(&remove_root, None));
        content.append(&open);
        content.append(&self.project_repository_link(root));
        content.append(&pin);
        content.append(&remove);
        row.set_child(Some(&content));
        row
    }

    fn project_repository_link(&self, root: &str) -> gtk::LinkButton {
        let link = gtk::LinkButton::builder()
            .icon_name("web-browser-symbolic")
            .valign(gtk::Align::Center)
            .visible(false)
            .build();
        link.add_css_class("flat");
        link.update_property(&[gtk::accessible::Property::Label(
            "Open repository in browser",
        )]);
        let workspace = self.clone();
        link.connect_activate_link(move |link| {
            let workspace = workspace.clone();
            gtk::UriLauncher::new(&link.uri()).launch(
                Some(&workspace.window.clone()),
                None::<&gio::Cancellable>,
                move |result| {
                    if let Err(error) = result {
                        workspace.show_error(&format!("Could not open repository: {error}"));
                    }
                },
            );
            glib::Propagation::Stop
        });
        let root = root.to_owned();
        let weak_link = link.downgrade();
        self.run_io(
            move || Ok(crate::projects::repository_url(Path::new(&root))),
            move |_, result| {
                if let Some(link) = weak_link.upgrade()
                    && let Ok(Some(url)) = result
                {
                    link.set_uri(&url);
                    link.set_tooltip_text(Some(&format!("Open repository in browser\n{url}")));
                    link.set_visible(true);
                }
            },
        );
        link
    }
}

impl Workspace {
    /// Only Azure DevOps projects get this; the dot marks activity not yet viewed.
    fn project_pr_button(&self, root: &str, attention: usize) -> gtk::Button {
        let label = gtk::Label::new(Some("PR"));
        label.add_css_class("caption-heading");
        let contents = gtk::Overlay::new();
        contents.set_child(Some(&label));
        if attention > 0 {
            let dot = gtk::Box::new(gtk::Orientation::Horizontal, 0);
            dot.add_css_class("attention-dot");
            dot.set_halign(gtk::Align::End);
            dot.set_valign(gtk::Align::Start);
            contents.add_overlay(&dot);
        }
        let button = gtk::Button::builder()
            .child(&contents)
            .valign(gtk::Align::Center)
            .tooltip_text(match attention {
                0 => "Pull requests".to_owned(),
                count => format!("Pull requests, {count} with new activity"),
            })
            .build();
        button.add_css_class("flat");
        let workspace = self.clone();
        let root = root.to_owned();
        button.connect_clicked(move |_| workspace.open_pr_for_project(&root));
        button
    }
}

fn group_label(text: &str) -> gtk::ListBoxRow {
    let row = gtk::ListBoxRow::new();
    row.set_selectable(false);
    row.set_activatable(false);
    let label = gtk::Label::new(Some(text));
    label.set_xalign(0.0);
    label.add_css_class("heading");
    label.add_css_class("dim-label");
    row.set_child(Some(&label));
    row
}

fn project_name(path: &str) -> String {
    Path::new(path)
        .file_name()
        .and_then(|name| name.to_str())
        .filter(|name| !name.is_empty())
        .unwrap_or(path)
        .to_owned()
}

/// What one sidebar row shows, encoded as a string for the list model.
enum SidebarKey<'a> {
    Project {
        root: &'a str,
        name: &'a str,
        settings: ProjectSettings,
        /// `None` hides the PR button.
        pr_attention: Option<usize>,
    },
    Session(&'a str),
    Other,
    /// The header above the projects without sessions.
    Inactive {
        count: usize,
        is_expanded: bool,
    },
    InactiveProject(&'a str),
}

const KEY_SEPARATOR: char = '\u{1f}';

impl<'a> SidebarKey<'a> {
    const OTHER: &'static str = "other";

    /// Pin, collapse and PR state are part of the key, so changing them rebuilds the header.
    fn project_key(
        root: &str,
        name: &str,
        settings: ProjectSettings,
        pr_attention: Option<usize>,
    ) -> String {
        format!(
            "project{KEY_SEPARATOR}{}{KEY_SEPARATOR}{}{KEY_SEPARATOR}{}{KEY_SEPARATOR}{root}{KEY_SEPARATOR}{name}",
            u8::from(settings.is_pinned),
            u8::from(settings.is_collapsed),
            pr_attention.map_or_else(|| "-".to_owned(), |count| count.to_string()),
        )
    }

    fn session_key(id: &str) -> String {
        format!("session{KEY_SEPARATOR}{id}")
    }

    /// The count and expansion are part of the key, so changing them rebuilds the header.
    fn inactive_key(count: usize, is_expanded: bool) -> String {
        format!(
            "inactive{KEY_SEPARATOR}{}{KEY_SEPARATOR}{count}",
            u8::from(is_expanded)
        )
    }

    fn inactive_project_key(root: &str) -> String {
        format!("inactive-project{KEY_SEPARATOR}{root}")
    }

    fn parse(key: &'a str) -> Option<Self> {
        if key == Self::OTHER {
            return Some(Self::Other);
        }
        let mut parts = key.splitn(6, KEY_SEPARATOR);
        match parts.next()? {
            "session" => Some(Self::Session(parts.next()?)),
            "inactive" => {
                let is_expanded = parts.next()? == "1";
                Some(Self::Inactive {
                    is_expanded,
                    count: parts.next()?.parse().ok()?,
                })
            }
            "inactive-project" => Some(Self::InactiveProject(parts.next()?)),
            "project" => {
                let is_pinned = parts.next()? == "1";
                let is_collapsed = parts.next()? == "1";
                let pr_attention = parts.next()?.parse().ok();
                Some(Self::Project {
                    root: parts.next()?,
                    name: parts.next()?,
                    settings: ProjectSettings {
                        is_pinned,
                        is_collapsed,
                    },
                    pr_attention,
                })
            }
            _ => None,
        }
    }
}

/// The start, removed count and new items that turn `current` into `wanted`.
fn changed_range<'a>(current: &[String], wanted: &'a [String]) -> (usize, usize, &'a [String]) {
    let prefix = current
        .iter()
        .zip(wanted)
        .take_while(|(left, right)| left == right)
        .count();
    let suffix = current[prefix..]
        .iter()
        .rev()
        .zip(wanted[prefix..].iter().rev())
        .take_while(|(left, right)| left == right)
        .count();
    (
        prefix,
        current.len() - prefix - suffix,
        &wanted[prefix..wanted.len() - suffix],
    )
}

#[cfg(test)]
mod sidebar_tests {
    use super::{ProjectSettings, SidebarKey, changed_range};

    fn keys(values: &[&str]) -> Vec<String> {
        values.iter().map(|value| (*value).to_owned()).collect()
    }

    #[test]
    fn unchanged_keys_need_no_splice() {
        let current = keys(&["a", "b", "c"]);
        assert_eq!(changed_range(&current, &current), (3, 0, &[][..]));
    }

    #[test]
    fn an_added_session_only_inserts_its_row() {
        let current = keys(&["project", "one", "two"]);
        let wanted = keys(&["project", "one", "new", "two"]);
        assert_eq!(changed_range(&current, &wanted), (2, 0, &wanted[2..3]));
    }

    #[test]
    fn a_removed_session_only_removes_its_row() {
        let current = keys(&["project", "one", "gone", "two"]);
        let wanted = keys(&["project", "one", "two"]);
        assert_eq!(changed_range(&current, &wanted), (2, 1, &[][..]));
    }

    #[test]
    fn project_keys_round_trip() {
        let key = SidebarKey::project_key(
            "/work/agtk",
            "agtk",
            ProjectSettings {
                is_pinned: true,
                is_collapsed: false,
            },
            Some(2),
        );
        let Some(SidebarKey::Project {
            root,
            name,
            settings,
            pr_attention,
        }) = SidebarKey::parse(&key)
        else {
            panic!("project key did not parse");
        };
        assert_eq!((root, name), ("/work/agtk", "agtk"));
        assert!(settings.is_pinned && !settings.is_collapsed);
        assert_eq!(pr_attention, Some(2));
    }

    #[test]
    fn inactive_project_keys_round_trip() {
        let header = SidebarKey::inactive_key(3, true);
        let Some(SidebarKey::Inactive { count, is_expanded }) = SidebarKey::parse(&header) else {
            panic!("inactive header key did not parse");
        };
        assert_eq!((count, is_expanded), (3, true));
        assert_ne!(header, SidebarKey::inactive_key(3, false));
        assert_ne!(header, SidebarKey::inactive_key(4, true));

        let row = SidebarKey::inactive_project_key("/work/old project");
        let Some(SidebarKey::InactiveProject(root)) = SidebarKey::parse(&row) else {
            panic!("inactive project key did not parse");
        };
        assert_eq!(root, "/work/old project");
    }
}
