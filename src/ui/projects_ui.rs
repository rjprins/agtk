use std::collections::BTreeMap;
use std::path::Path;

use super::*;
use crate::projects::ProjectSettings;

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
        let Some(store) = self.store.borrow().clone() else {
            self.report_failure(
                pending,
                ErrorCode::InternalError,
                "Workspace is loading",
                "project settings are not available yet".to_owned(),
            );
            return;
        };
        let mut preferences = self.projects.borrow().clone();
        preferences.set(&root, is_pinned, is_collapsed);
        let value = match serde_json::to_value(&preferences) {
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
                    workspace.load_projects(preferences);
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

    pub(super) fn project_summaries(&self) -> Vec<crate::control::ProjectSummary> {
        let preferences = self.projects.borrow();
        let mut roots = preferences.projects.keys().cloned().collect::<Vec<_>>();
        for session in self.sessions.borrow().values() {
            if let Some(root) = session.record.project_root.as_ref() {
                let root = root.to_string_lossy().to_string();
                if !roots.contains(&root) {
                    roots.push(root);
                }
            }
        }
        roots.sort_by(|left, right| {
            let left_settings = preferences.get(left);
            let right_settings = preferences.get(right);
            right_settings
                .is_pinned
                .cmp(&left_settings.is_pinned)
                .then_with(|| project_name(left).cmp(&project_name(right)))
                .then_with(|| left.cmp(right))
        });
        roots
            .into_iter()
            .map(|root| {
                let settings = preferences.get(&root);
                crate::control::ProjectSummary {
                    name: project_name(&root),
                    root,
                    is_pinned: settings.is_pinned,
                    is_collapsed: settings.is_collapsed,
                }
            })
            .collect()
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
            Some(SidebarKey::Other) | None => group_label("Other"),
        }
    }

    pub(super) fn rebuild_sidebar(&self) {
        let projects = self.project_summaries();
        self.menus.worktrees.set_enabled(!projects.is_empty());
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
        self.splice_sidebar(&keys);

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

    /// Replaces only the changed middle of the key list, so untouched rows keep focus.
    fn splice_sidebar(&self, keys: &[String]) {
        let model = &self.sidebar_model;
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
        content.append(&launch);
        content.append(&more);
        menus::open_menu_on_right_click(&collapse, &more);
        row.set_child(Some(&content));
        row
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

    fn parse(key: &'a str) -> Option<Self> {
        if key == Self::OTHER {
            return Some(Self::Other);
        }
        let mut parts = key.splitn(6, KEY_SEPARATOR);
        match parts.next()? {
            "session" => Some(Self::Session(parts.next()?)),
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
}
