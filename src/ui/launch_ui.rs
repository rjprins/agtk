#![allow(deprecated)]

use super::*;
use crate::launch_model::{self, NEW_WORKTREE};
use crate::worktrees::WorktreeManager;
use serde_json::Value;
use std::collections::BTreeMap;

const PROJECT_PLACEHOLDER: &str = "Search projects or type a path…";
const WORKTREE_PLACEHOLDER: &str = "Search worktrees…";

impl Workspace {
    pub(super) fn prepare_launch_panel(&self) {
        if self.launch_project.text().trim().is_empty()
            && let Some(root) = self.preferred_project_root()
        {
            self.launch_project.set_text(&root);
            self.launch_cwd.set_text(&root);
        }
        self.refresh_launch_project_choices();
        self.refresh_launch_worktree_choices();
        self.refresh_launch_base_branches();
        let creating = self.launch_creating_worktree.get();
        if !creating && self.launch_worktree.text().trim().is_empty() {
            self.launch_cwd.set_text(self.launch_project.text().trim());
        }
        if creating {
            if self.launch_branch.text().trim().is_empty() {
                self.launch_branch.set_text(&generated_branch_name());
            }
            if self.launch_base_branch.text().trim().is_empty() {
                self.launch_base_branch.set_text("main");
            }
        }
        if let Some(row) = self.launch_branch.parent() {
            row.set_visible(creating);
        }
        if let Some(row) = self.launch_base_branch.parent() {
            row.set_visible(creating);
        }
        self.set_launch_agent(self.quick_launch.borrow().kind);
    }

    pub(super) fn load_quick_launch(&self, preferences: QuickLaunchPreferences) {
        self.launch_cwd.set_text(&display_path(
            preferences
                .cwd
                .as_ref()
                .or(preferences.worktree_path.as_ref()),
        ));
        self.launch_project
            .set_text(&display_path(preferences.project_root.as_ref()));
        self.launch_worktree
            .set_text(&display_path(preferences.worktree_path.as_ref()));
        self.launch_creating_worktree.set(false);
        self.launch_args.set_text(
            &serde_json::to_string(&preferences.args).unwrap_or_else(|_| "[]".to_owned()),
        );
        self.apply_launch_flags(&preferences.flags);
        self.set_launch_agent(preferences.kind);
        self.launch_project_custom.set(false);
        *self.quick_launch.borrow_mut() = preferences;
        self.refresh_launch_project_choices();
        self.refresh_launch_worktree_choices();
    }

    pub(super) fn launch_from_form(&self, kind: SessionKind) {
        let raw_args = match parse_arguments(&self.launch_args) {
            Ok(args) => args,
            Err(error) => {
                self.show_error(&error);
                return;
            }
        };
        let flags = self.current_launch_flags(kind);
        let mut args = raw_args.clone();
        args.extend(launch_model::provider_args(kind, &flags));
        let launch_args = args;
        let creating_worktree = self.launch_creating_worktree.get();
        let project_root = optional_path(&self.launch_project);
        if creating_worktree && project_root.is_none() {
            self.show_error("A project directory is required for a new worktree");
            return;
        }
        let preferences = QuickLaunchPreferences {
            kind,
            args: raw_args,
            flags: self.all_launch_flags(),
            cwd: if creating_worktree {
                project_root.clone()
            } else {
                launch_model::effective_worktree_path(
                    project_root.as_deref(),
                    optional_path(&self.launch_worktree).as_deref(),
                )
                .or_else(|| optional_path(&self.launch_cwd))
            },
            project_root: project_root.clone(),
            worktree_path: (!creating_worktree)
                .then(|| {
                    launch_model::effective_worktree_path(
                        project_root.as_deref(),
                        optional_path(&self.launch_worktree).as_deref(),
                    )
                })
                .flatten(),
        };
        let name = optional_text(&self.launch_name);
        let initial_input = optional_text(&self.launch_prompt);
        if let Ok(value) = serde_json::to_value(&preferences) {
            self.save_preference("quickLaunch", value);
        }
        *self.quick_launch.borrow_mut() = preferences.clone();
        if creating_worktree {
            let root = project_root.expect("validated project root");
            let branch = optional_text(&self.launch_branch).unwrap_or_else(generated_branch_name);
            let base = optional_text(&self.launch_base_branch);
            let purpose = name.clone().unwrap_or_else(|| branch.clone());
            let manager = WorktreeManager::new(self.paths.attic_dir());
            self.launch_popover.popdown();
            self.run_io(
                move || manager.create(&root, &branch, base.as_deref(), &purpose),
                move |workspace, result| match result {
                    Ok(created) => workspace.launch_controlled(
                        CreateSessionParams {
                            kind,
                            command: None,
                            args: launch_args,
                            cwd: Some(created.path.clone()),
                            name,
                            project_root: preferences.project_root,
                            worktree_path: Some(created.path),
                            initial_input,
                        },
                        None,
                    ),
                    Err(error) => {
                        workspace.show_error(&format!("Could not create worktree: {error}"))
                    }
                },
            );
        } else {
            self.launch_popover.popdown();
            self.launch_controlled(
                CreateSessionParams {
                    kind,
                    command: None,
                    args: launch_args,
                    cwd: preferences.cwd,
                    name,
                    project_root: preferences.project_root,
                    worktree_path: preferences.worktree_path,
                    initial_input,
                },
                None,
            );
        }
    }

    pub(super) fn open_launch_for_project(&self, root: &str) {
        self.launch_project_custom.set(false);
        self.launch_project.set_text(root);
        self.launch_cwd.set_text(root);
        self.launch_worktree.set_text("");
        self.launch_creating_worktree.set(false);
        self.launch_branch.set_text(&generated_branch_name());
        self.launch_base_branch.set_text("main");
        if let Some(row) = self.launch_branch.parent() {
            row.set_visible(false);
        }
        if let Some(row) = self.launch_base_branch.parent() {
            row.set_visible(false);
        }
        self.refresh_launch_project_choices();
        self.refresh_launch_worktree_choices();
        self.launch_popover.popup();
    }

    pub(super) fn open_launch_for_worktree(&self, project_root: &Path, worktree: &Path) {
        self.launch_project_custom.set(false);
        self.launch_project
            .set_text(project_root.to_string_lossy().as_ref());
        self.launch_worktree
            .set_text(worktree.to_string_lossy().as_ref());
        self.launch_creating_worktree.set(false);
        self.launch_cwd
            .set_text(worktree.to_string_lossy().as_ref());
        if let Some(row) = self.launch_branch.parent() {
            row.set_visible(false);
        }
        if let Some(row) = self.launch_base_branch.parent() {
            row.set_visible(false);
        }
        self.refresh_launch_project_choices();
        self.refresh_launch_worktree_choices();
        self.worktree_popover.popdown();
        self.launch_popover.popup();
    }

    pub(super) fn refresh_launch_project_choices(&self) {
        let current = self.launch_project.text().trim().to_owned();
        let mut roots = self
            .project_summaries()
            .into_iter()
            .map(|project| project.root)
            .collect::<Vec<_>>();
        if !current.is_empty() && !roots.iter().any(|root| root == &current) {
            roots.push(current.clone());
        }
        let mut values = Vec::new();
        let mut labels = Vec::new();
        if current.is_empty() {
            values.push(None);
            labels.push(PROJECT_PLACEHOLDER.to_owned());
        }
        for root in roots {
            values.push(Some(root.clone()));
            labels.push(project_name(&root));
        }
        let completion_items = values
            .iter()
            .zip(labels.iter())
            .filter_map(|(value, label)| value.clone().map(|value| (label.clone(), value)))
            .collect();
        *self.launch_project_values.borrow_mut() = values;
        self.updating_launch_choices.set(true);
        replace_string_list(&self.launch_project_choices, &labels);
        replace_completion_items(
            &self.launch_project_completion,
            &self.launch_project_completion_items,
            completion_items,
        );
        self.set_dropdown_value(
            &self.launch_project_dropdown,
            &self.launch_project_values.borrow(),
            (!current.is_empty()).then_some(current.as_str()),
        );
        self.updating_launch_choices.set(false);
    }

    pub(super) fn refresh_launch_worktree_choices(&self) {
        let root = launch_path_text(&self.launch_project).unwrap_or_default();
        let sequence = self.launch_choice_sequence.get().wrapping_add(1);
        self.launch_choice_sequence.set(sequence);
        if root.is_empty() {
            self.updating_launch_choices.set(true);
            let labels = vec![WORKTREE_PLACEHOLDER.to_owned()];
            *self.launch_worktree_values.borrow_mut() = vec![None];
            replace_string_list(&self.launch_worktree_choices, &labels);
            replace_completion_items(
                &self.launch_worktree_completion,
                &self.launch_worktree_completion_items,
                Vec::new(),
            );
            self.set_dropdown_value(
                &self.launch_worktree_dropdown,
                &self.launch_worktree_values.borrow(),
                None,
            );
            self.updating_launch_choices.set(false);
            return;
        }
        let manager = WorktreeManager::new(self.paths.attic_dir());
        let live_paths = self.live_worktree_paths();
        let initial = launch_model::worktree_choices(&root, [], true);
        let selected_worktree = self.launch_worktree.text().trim().to_owned();
        self.updating_launch_choices.set(true);
        *self.launch_worktree_values.borrow_mut() = initial
            .iter()
            .map(|choice| Some(choice.value.clone()))
            .collect();
        replace_string_list(
            &self.launch_worktree_choices,
            &initial
                .iter()
                .map(|choice| choice.label.clone())
                .collect::<Vec<_>>(),
        );
        replace_completion_items(
            &self.launch_worktree_completion,
            &self.launch_worktree_completion_items,
            initial
                .iter()
                .map(|choice| (choice.label.clone(), choice.value.clone()))
                .collect(),
        );
        self.set_dropdown_value(
            &self.launch_worktree_dropdown,
            &self.launch_worktree_values.borrow(),
            if self.launch_creating_worktree.get() {
                Some(NEW_WORKTREE)
            } else if selected_worktree.is_empty() {
                Some(root.as_str())
            } else {
                Some(selected_worktree.as_str())
            },
        );
        self.updating_launch_choices.set(false);
        let worker_root = root.clone();
        self.run_io(
            move || manager.list(Path::new(&worker_root), &live_paths),
            move |workspace, result| {
                if workspace.launch_choice_sequence.get() != sequence
                    || launch_path_text(&workspace.launch_project).as_deref() != Some(root.as_str())
                {
                    return;
                }
                let mut worktrees = Vec::new();
                if let Ok(inventory) = result {
                    worktrees = inventory
                        .worktrees
                        .into_iter()
                        .map(|worktree| {
                            let path = worktree.path.to_string_lossy().to_string();
                            let label = project_name(&path);
                            (path, label)
                        })
                        .collect::<Vec<_>>();
                }
                let choices = launch_model::worktree_choices(&root, worktrees, true);
                let labels = choices
                    .iter()
                    .map(|choice| choice.label.clone())
                    .collect::<Vec<_>>();
                workspace.updating_launch_choices.set(true);
                *workspace.launch_worktree_values.borrow_mut() = choices
                    .iter()
                    .map(|choice| Some(choice.value.clone()))
                    .collect();
                replace_string_list(&workspace.launch_worktree_choices, &labels);
                replace_completion_items(
                    &workspace.launch_worktree_completion,
                    &workspace.launch_worktree_completion_items,
                    choices
                        .iter()
                        .map(|choice| (choice.label.clone(), choice.value.clone()))
                        .collect(),
                );
                let selected = workspace.launch_worktree.text().trim().to_owned();
                workspace.set_dropdown_value(
                    &workspace.launch_worktree_dropdown,
                    &workspace.launch_worktree_values.borrow(),
                    if workspace.launch_creating_worktree.get() {
                        Some(NEW_WORKTREE)
                    } else if selected.is_empty() {
                        Some(root.as_str())
                    } else {
                        Some(selected.as_str())
                    },
                );
                workspace.updating_launch_choices.set(false);
            },
        );
    }

    pub(super) fn connect_launch_path_controls(&self) {
        let project_workspace = self.clone();
        self.launch_project_dropdown
            .connect_selected_notify(move |dropdown| {
                if project_workspace.updating_launch_choices.get() {
                    return;
                }
                let Some(root) = project_workspace.selected_project_path(dropdown) else {
                    return;
                };
                project_workspace.launch_project.set_text(&root);
                project_workspace.launch_project_custom.set(false);
                project_workspace.launch_cwd.set_text(&root);
                project_workspace.launch_worktree.set_text("");
                project_workspace.launch_creating_worktree.set(false);
                project_workspace
                    .launch_branch
                    .set_text(&generated_branch_name());
                project_workspace.launch_base_branch.set_text("main");
                if let Some(row) = project_workspace.launch_branch.parent() {
                    row.set_visible(false);
                }
                if let Some(row) = project_workspace.launch_base_branch.parent() {
                    row.set_visible(false);
                }
                project_workspace.refresh_launch_worktree_choices();
                project_workspace.refresh_launch_base_branches();
            });

        let worktree_workspace = self.clone();
        self.launch_worktree_dropdown
            .connect_selected_notify(move |dropdown| {
                if worktree_workspace.updating_launch_choices.get() {
                    return;
                }
                let Some(path) = worktree_workspace.selected_worktree_path(dropdown) else {
                    return;
                };
                if path == NEW_WORKTREE {
                    worktree_workspace.launch_worktree.set_text("");
                    worktree_workspace.launch_creating_worktree.set(true);
                    worktree_workspace
                        .launch_cwd
                        .set_text(worktree_workspace.launch_project.text().trim());
                    worktree_workspace
                        .launch_branch
                        .set_text(&generated_branch_name());
                    worktree_workspace.launch_base_branch.set_text("main");
                    if let Some(row) = worktree_workspace.launch_branch.parent() {
                        row.set_visible(true);
                    }
                    if let Some(row) = worktree_workspace.launch_base_branch.parent() {
                        row.set_visible(true);
                    }
                } else {
                    worktree_workspace.launch_creating_worktree.set(false);
                    worktree_workspace.launch_worktree.set_text(&path);
                    worktree_workspace.launch_cwd.set_text(&path);
                    if let Some(row) = worktree_workspace.launch_branch.parent() {
                        row.set_visible(false);
                    }
                    if let Some(row) = worktree_workspace.launch_base_branch.parent() {
                        row.set_visible(false);
                    }
                }
            });

        let worktree_entry_workspace = self.clone();
        self.launch_worktree.connect_changed(move |entry| {
            if worktree_entry_workspace.updating_launch_choices.get() {
                return;
            }
            let value = entry.text().trim().to_owned();
            if value == NEW_WORKTREE {
                entry.set_text("");
                worktree_entry_workspace.launch_creating_worktree.set(true);
            } else {
                worktree_entry_workspace.launch_creating_worktree.set(false);
            }
            let creating = worktree_entry_workspace.launch_creating_worktree.get();
            if creating {
                worktree_entry_workspace
                    .launch_cwd
                    .set_text(worktree_entry_workspace.launch_project.text().trim());
                if worktree_entry_workspace
                    .launch_branch
                    .text()
                    .trim()
                    .is_empty()
                {
                    worktree_entry_workspace
                        .launch_branch
                        .set_text(&generated_branch_name());
                }
                if worktree_entry_workspace
                    .launch_base_branch
                    .text()
                    .trim()
                    .is_empty()
                {
                    worktree_entry_workspace.launch_base_branch.set_text("main");
                }
            } else {
                worktree_entry_workspace.launch_cwd.set_text(&value);
            }
            if let Some(row) = worktree_entry_workspace.launch_branch.parent() {
                row.set_visible(creating);
            }
            if let Some(row) = worktree_entry_workspace.launch_base_branch.parent() {
                row.set_visible(creating);
            }
        });

        let project_workspace = self.clone();
        self.launch_project.connect_activate(move |_| {
            project_workspace.launch_project_custom.set(true);
            project_workspace.launch_creating_worktree.set(false);
            project_workspace.refresh_launch_project_choices();
            project_workspace.refresh_launch_worktree_choices();
            project_workspace.refresh_launch_base_branches();
            if let Some(row) = project_workspace.launch_branch.parent() {
                row.set_visible(false);
            }
            if let Some(row) = project_workspace.launch_base_branch.parent() {
                row.set_visible(false);
            }
        });

        let project_completion_workspace = self.clone();
        self.launch_project.connect_changed(move |entry| {
            if project_completion_workspace.updating_launch_choices.get()
                || !entry.has_focus()
                || !project_completion_workspace
                    .launch_project_values
                    .borrow()
                    .iter()
                    .any(|value| value.as_deref() == Some(entry.text().trim()))
            {
                return;
            }
            let root = entry.text().trim().to_owned();
            project_completion_workspace
                .launch_project_custom
                .set(false);
            project_completion_workspace.launch_cwd.set_text(&root);
            project_completion_workspace.launch_worktree.set_text("");
            project_completion_workspace
                .launch_creating_worktree
                .set(false);
            project_completion_workspace
                .launch_branch
                .set_text(&generated_branch_name());
            project_completion_workspace
                .launch_base_branch
                .set_text("main");
            if let Some(row) = project_completion_workspace.launch_branch.parent() {
                row.set_visible(false);
            }
            if let Some(row) = project_completion_workspace.launch_base_branch.parent() {
                row.set_visible(false);
            }
            project_completion_workspace.refresh_launch_worktree_choices();
            project_completion_workspace.refresh_launch_base_branches();
        });

        let base_workspace = self.clone();
        self.launch_base_branch_dropdown
            .connect_selected_notify(move |dropdown| {
                let Some(value) = dropdown
                    .selected_item()
                    .and_then(|item| item.downcast::<gtk::StringObject>().ok())
                    .map(|item| item.string().to_string())
                else {
                    return;
                };
                if !value.is_empty() {
                    base_workspace.launch_base_branch.set_text(&value);
                }
            });
    }

    fn refresh_launch_base_branches(&self) {
        let root = launch_path_text(&self.launch_project).unwrap_or_default();
        if root.is_empty() {
            replace_string_list(&self.launch_base_branch_choices, &[]);
            self.launch_base_branch.set_placeholder_text(Some("main"));
            replace_completion_items(
                &self.launch_base_branch_completion,
                &self.launch_base_branch_completion_items,
                Vec::new(),
            );
            return;
        }
        let manager = WorktreeManager::new(self.paths.attic_dir());
        self.run_io(
            move || manager.branch_names(Path::new(&root)),
            move |workspace, result| {
                let Ok(branches) = result else {
                    return;
                };
                replace_string_list(&workspace.launch_base_branch_choices, &branches);
                workspace
                    .launch_base_branch
                    .set_placeholder_text(Some(if branches.is_empty() {
                        "main"
                    } else {
                        "Search branches…"
                    }));
                replace_completion_items(
                    &workspace.launch_base_branch_completion,
                    &workspace.launch_base_branch_completion_items,
                    branches
                        .iter()
                        .map(|branch| (branch.clone(), branch.clone()))
                        .collect(),
                );
                if let Some(branch) = branches.first() {
                    if workspace.launch_base_branch.text().trim().is_empty()
                        || workspace.launch_base_branch.text() == "main"
                    {
                        workspace.launch_base_branch.set_text(branch);
                    }
                    workspace.launch_base_branch_dropdown.set_selected(0);
                }
            },
        );
    }

    fn set_dropdown_value(
        &self,
        dropdown: &gtk::DropDown,
        values: &[Option<String>],
        path: Option<&str>,
    ) {
        let selected = values
            .iter()
            .enumerate()
            .find(|(_, value)| value.as_deref() == path)
            .map(|(index, _)| index as u32)
            .unwrap_or(gtk::INVALID_LIST_POSITION);
        dropdown.set_selected(selected);
    }

    fn selected_project_path(&self, dropdown: &gtk::DropDown) -> Option<String> {
        self.launch_project_values
            .borrow()
            .get(dropdown.selected() as usize)
            .and_then(Clone::clone)
    }

    fn selected_worktree_path(&self, dropdown: &gtk::DropDown) -> Option<String> {
        self.launch_worktree_values
            .borrow()
            .get(dropdown.selected() as usize)
            .and_then(Clone::clone)
    }

    pub(super) fn set_launch_agent(&self, kind: SessionKind) {
        let name = session_kind_name(kind);
        let selected = (0..self.launch_agent_choices.n_items())
            .find(|index| {
                self.launch_agent_choices
                    .string(*index)
                    .is_some_and(|value| value == name)
            })
            .unwrap_or(0);
        self.launch_agent_dropdown.set_selected(selected);
        for (button_kind, button) in self.launch_agent_buttons.iter() {
            button.set_active(*button_kind == kind);
        }
        self.launch_agent_options.set_visible_child_name(name);
    }

    pub(super) fn selected_launch_agent(&self) -> SessionKind {
        match self
            .launch_agent_dropdown
            .selected_item()
            .and_then(|item| item.downcast::<gtk::StringObject>().ok())
            .map(|item| item.string().to_string())
            .as_deref()
        {
            Some("codex") => SessionKind::Codex,
            Some("claude") => SessionKind::Claude,
            Some("gemini") => SessionKind::Gemini,
            _ => SessionKind::Shell,
        }
    }

    fn current_launch_flags(&self, kind: SessionKind) -> BTreeMap<String, Value> {
        let mut flags = BTreeMap::new();
        match kind {
            SessionKind::Claude => {
                flags.insert(
                    "--permission-mode".to_owned(),
                    Value::String(dropdown_value(&self.launch_claude_permission)),
                );
                flags.insert(
                    "--dangerously-skip-permissions".to_owned(),
                    Value::Bool(self.launch_claude_danger.is_active()),
                );
            }
            SessionKind::Codex => {
                flags.insert(
                    "--ask-for-approval".to_owned(),
                    Value::String(dropdown_value(&self.launch_codex_approval)),
                );
                flags.insert(
                    "--sandbox".to_owned(),
                    Value::String(dropdown_value(&self.launch_codex_sandbox)),
                );
                flags.insert(
                    "--full-auto".to_owned(),
                    Value::Bool(self.launch_codex_full_auto.is_active()),
                );
                flags.insert(
                    "--dangerously-bypass-approvals-and-sandbox".to_owned(),
                    Value::Bool(self.launch_codex_bypass.is_active()),
                );
            }
            SessionKind::Gemini => {
                flags.insert(
                    "--approval-mode".to_owned(),
                    Value::String(dropdown_value(&self.launch_gemini_approval)),
                );
                flags.insert(
                    "--yolo".to_owned(),
                    Value::Bool(self.launch_gemini_yolo.is_active()),
                );
            }
            SessionKind::Shell | SessionKind::Custom => {}
        }
        flags
    }

    fn all_launch_flags(&self) -> BTreeMap<String, BTreeMap<String, Value>> {
        [SessionKind::Claude, SessionKind::Codex, SessionKind::Gemini]
            .into_iter()
            .map(|kind| {
                (
                    session_kind_name(kind).to_owned(),
                    self.current_launch_flags(kind),
                )
            })
            .collect()
    }

    fn apply_launch_flags(&self, flags: &BTreeMap<String, BTreeMap<String, Value>>) {
        if let Some(values) = flags.get("claude") {
            set_dropdown_value(
                &self.launch_claude_permission,
                values.get("--permission-mode"),
            );
            set_check_value(
                &self.launch_claude_danger,
                values.get("--dangerously-skip-permissions"),
            );
        }
        if let Some(values) = flags.get("codex") {
            set_dropdown_value(
                &self.launch_codex_approval,
                values.get("--ask-for-approval"),
            );
            set_dropdown_value(&self.launch_codex_sandbox, values.get("--sandbox"));
            set_check_value(&self.launch_codex_full_auto, values.get("--full-auto"));
            set_check_value(
                &self.launch_codex_bypass,
                values.get("--dangerously-bypass-approvals-and-sandbox"),
            );
        }
        if let Some(values) = flags.get("gemini") {
            set_dropdown_value(&self.launch_gemini_approval, values.get("--approval-mode"));
            set_check_value(&self.launch_gemini_yolo, values.get("--yolo"));
        }
    }
}

fn replace_string_list(model: &gtk::StringList, values: &[String]) {
    let values = values.iter().map(String::as_str).collect::<Vec<_>>();
    model.splice(0, model.n_items(), &values);
}

fn replace_completion_items(
    model: &gtk::ListStore,
    items: &Rc<RefCell<Vec<(String, String)>>>,
    values: Vec<(String, String)>,
) {
    *items.borrow_mut() = values.clone();
    while let Some(row) = model.iter_first() {
        model.remove(&row);
    }
    for (label, value) in values {
        let row = model.append();
        model.set(&row, &[(0, &label), (1, &value)]);
    }
}

fn optional_path(entry: &gtk::Entry) -> Option<PathBuf> {
    optional_text(entry).map(|path| launch_model::expand_user_path(&path))
}

fn launch_path_text(entry: &gtk::Entry) -> Option<String> {
    optional_path(entry).map(|path| path.to_string_lossy().into_owned())
}

fn optional_text(entry: &gtk::Entry) -> Option<String> {
    let text = entry.text();
    let text = text.trim();
    (!text.is_empty()).then(|| text.to_owned())
}

fn display_path(path: Option<&PathBuf>) -> String {
    path.map(|path| path.to_string_lossy().to_string())
        .unwrap_or_default()
}

fn parse_arguments(entry: &gtk::Entry) -> Result<Vec<String>, String> {
    let text = entry.text();
    let text = text.trim();
    if text.is_empty() {
        return Ok(Vec::new());
    }
    serde_json::from_str::<Vec<String>>(text)
        .map_err(|error| format!("Arguments must be a JSON string array: {error}"))
}

fn dropdown_value(dropdown: &gtk::DropDown) -> String {
    dropdown
        .selected_item()
        .and_then(|item| item.downcast::<gtk::StringObject>().ok())
        .map(|item| item.string().to_string())
        .unwrap_or_default()
}

fn set_dropdown_value(dropdown: &gtk::DropDown, value: Option<&Value>) {
    let Some(value) = value.and_then(Value::as_str) else {
        return;
    };
    let Some(model) = dropdown
        .model()
        .and_then(|model| model.downcast::<gtk::StringList>().ok())
    else {
        return;
    };
    if let Some(index) =
        (0..model.n_items()).find(|index| model.string(*index).is_some_and(|item| item == value))
    {
        dropdown.set_selected(index);
    }
}

fn set_check_value(check: &gtk::CheckButton, value: Option<&Value>) {
    if let Some(value) = value.and_then(Value::as_bool) {
        check.set_active(value);
    }
}

fn generated_branch_name() -> String {
    let millis = SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .unwrap_or_default()
        .as_secs();
    format!("work-{millis}")
}

fn project_name(path: &str) -> String {
    Path::new(path)
        .file_name()
        .and_then(|name| name.to_str())
        .filter(|name| !name.is_empty())
        .unwrap_or(path)
        .to_owned()
}
