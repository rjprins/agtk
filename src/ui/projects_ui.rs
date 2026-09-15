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

    pub(super) fn rebuild_sidebar(&self) {
        while let Some(child) = self.list.first_child() {
            let row = child
                .downcast::<gtk::ListBoxRow>()
                .expect("list contains rows");
            self.list.remove(&row);
        }

        let projects = self.project_summaries();
        self.worktree_button.set_sensitive(!projects.is_empty());
        let sessions = self.sessions.borrow();
        for project in projects {
            self.list.append(&self.project_header(
                &project.root,
                &project.name,
                ProjectSettings {
                    is_pinned: project.is_pinned,
                    is_collapsed: project.is_collapsed,
                },
            ));
            let mut grouped = BTreeMap::<Option<String>, Vec<&SessionView>>::new();
            for session in
                sessions.values().filter(|session| {
                    session.record.project_root.as_ref().is_some_and(|root| {
                        root.to_string_lossy().as_ref() == project.root.as_str()
                    })
                })
            {
                grouped
                    .entry(
                        session
                            .record
                            .worktree_path
                            .as_ref()
                            .map(|path| path.to_string_lossy().to_string()),
                    )
                    .or_default()
                    .push(session);
            }
            for (worktree, mut project_sessions) in grouped {
                project_sessions.sort_by_key(|session| session.record.position);
                if let Some(worktree) = worktree {
                    let row = group_label(&format!("  ├ {}", project_name(&worktree)));
                    row.set_visible(!project.is_collapsed);
                    self.list.append(&row);
                }
                for session in project_sessions {
                    session.row.set_visible(!project.is_collapsed);
                    self.list.append(&session.row);
                }
            }
        }

        let mut ungrouped = sessions
            .values()
            .filter(|session| session.record.project_root.is_none())
            .collect::<Vec<_>>();
        if !ungrouped.is_empty() {
            self.list.append(&group_label("OTHER"));
            ungrouped.sort_by_key(|session| session.record.position);
            for session in ungrouped {
                session.row.set_visible(true);
                self.list.append(&session.row);
            }
        }

        if let Some(selected) = self.selected_session_id()
            && let Some(row) = sessions.get(&selected).map(|session| session.row.clone())
        {
            self.list.select_row(Some(&row));
        }
        self.refresh_launch_project_choices();
    }

    fn project_header(&self, root: &str, name: &str, settings: ProjectSettings) -> gtk::ListBoxRow {
        let row = gtk::ListBoxRow::new();
        row.set_selectable(false);
        row.set_activatable(false);
        row.add_css_class("tui-group-row");
        let content = gtk::Box::new(gtk::Orientation::Horizontal, 3);
        let collapse = gtk::Button::with_label(&format!(
            "[{}] {}",
            if settings.is_collapsed { ">" } else { "v" },
            name
        ));
        collapse.add_css_class("tui-group-button");
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
        let pin = gtk::Button::with_label(if settings.is_pinned { "[*]" } else { "[ ]" });
        pin.add_css_class("tui-button");
        pin.set_tooltip_text(Some(if settings.is_pinned {
            "Unpin project"
        } else {
            "Pin project"
        }));
        let pin_workspace = self.clone();
        let pin_root = root.to_owned();
        pin.connect_clicked(move |_| {
            pin_workspace.set_project_preferences(
                pin_root.clone(),
                Some(!settings.is_pinned),
                None,
                None,
            );
        });
        content.append(&pin);
        let worktrees = gtk::Button::with_label("[wt]");
        worktrees.add_css_class("tui-button");
        worktrees.set_tooltip_text(Some("Manage project worktrees"));
        let worktree_workspace = self.clone();
        let worktree_root = root.to_owned();
        worktrees.connect_clicked(move |_| {
            worktree_workspace.open_worktrees_for_project(&worktree_root)
        });
        content.append(&worktrees);
        let launch = gtk::Button::with_label("[+]");
        launch.add_css_class("tui-button");
        launch.set_tooltip_text(Some("Launch in this project"));
        let launch_workspace = self.clone();
        let launch_root = root.to_owned();
        launch.connect_clicked(move |_| launch_workspace.open_launch_for_project(&launch_root));
        content.append(&launch);
        row.set_child(Some(&content));
        row
    }
}

fn group_label(text: &str) -> gtk::ListBoxRow {
    let row = gtk::ListBoxRow::new();
    row.set_selectable(false);
    row.set_activatable(false);
    row.add_css_class("tui-subgroup-row");
    let label = gtk::Label::new(Some(text));
    label.set_xalign(0.0);
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
