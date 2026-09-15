use super::*;
use crate::worktrees::WorktreeManager;

const PROJECT_PLACEHOLDER: &str = "[choose project]";
const WORKTREE_PLACEHOLDER: &str = "[choose worktree]";

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
        self.launch_args.set_text(
            &serde_json::to_string(&preferences.args).unwrap_or_else(|_| "[]".to_owned()),
        );
        *self.quick_launch.borrow_mut() = preferences;
        self.refresh_launch_project_choices();
        self.refresh_launch_worktree_choices();
    }

    pub(super) fn launch_from_form(&self, kind: SessionKind) {
        let args = match parse_arguments(&self.launch_args) {
            Ok(args) => args,
            Err(error) => {
                self.show_error(&error);
                return;
            }
        };
        let preferences = QuickLaunchPreferences {
            kind,
            args,
            cwd: optional_path(&self.launch_cwd),
            project_root: optional_path(&self.launch_project),
            worktree_path: optional_path(&self.launch_worktree),
        };
        let name = optional_text(&self.launch_name);
        let initial_input = optional_text(&self.launch_prompt);
        if let Ok(value) = serde_json::to_value(&preferences) {
            self.save_preference("quickLaunch", value);
        }
        *self.quick_launch.borrow_mut() = preferences.clone();
        self.launch_popover.popdown();
        self.launch_controlled(
            CreateSessionParams {
                kind,
                command: None,
                args: preferences.args,
                cwd: preferences.cwd,
                name,
                project_root: preferences.project_root,
                worktree_path: preferences.worktree_path,
                initial_input,
            },
            None,
        );
    }

    pub(super) fn open_launch_for_project(&self, root: &str) {
        self.launch_project.set_text(root);
        self.launch_cwd.set_text(root);
        self.launch_worktree.set_text("");
        self.refresh_launch_project_choices();
        self.refresh_launch_worktree_choices();
        self.launch_popover.popup();
    }

    pub(super) fn open_launch_for_worktree(&self, project_root: &Path, worktree: &Path) {
        self.launch_project
            .set_text(project_root.to_string_lossy().as_ref());
        self.launch_worktree
            .set_text(worktree.to_string_lossy().as_ref());
        self.launch_cwd
            .set_text(worktree.to_string_lossy().as_ref());
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
        self.updating_launch_choices.set(true);
        if current.is_empty() {
            roots.insert(0, PROJECT_PLACEHOLDER.to_owned());
        }
        replace_string_list(&self.launch_project_choices, &roots);
        let selected = if current.is_empty() {
            PROJECT_PLACEHOLDER
        } else {
            current.as_str()
        };
        self.set_dropdown_path_unchecked(
            &self.launch_project_dropdown,
            &self.launch_project_choices,
            selected,
        );
        self.updating_launch_choices.set(false);
    }

    pub(super) fn refresh_launch_worktree_choices(&self) {
        let root = self.launch_project.text().trim().to_owned();
        let sequence = self.launch_choice_sequence.get().wrapping_add(1);
        self.launch_choice_sequence.set(sequence);
        if root.is_empty() {
            self.updating_launch_choices.set(true);
            let choices = vec![WORKTREE_PLACEHOLDER.to_owned()];
            replace_string_list(&self.launch_worktree_choices, &choices);
            self.set_dropdown_path_unchecked(
                &self.launch_worktree_dropdown,
                &self.launch_worktree_choices,
                WORKTREE_PLACEHOLDER,
            );
            self.updating_launch_choices.set(false);
            return;
        }
        let manager = WorktreeManager::new(self.paths.attic_dir());
        let live_paths = self.live_worktree_paths();
        let initial = vec![WORKTREE_PLACEHOLDER.to_owned(), root.clone()];
        let selected_worktree = self.launch_worktree.text().trim().to_owned();
        self.updating_launch_choices.set(true);
        replace_string_list(&self.launch_worktree_choices, &initial);
        self.set_dropdown_path_unchecked(
            &self.launch_worktree_dropdown,
            &self.launch_worktree_choices,
            if selected_worktree.is_empty() {
                WORKTREE_PLACEHOLDER
            } else {
                selected_worktree.as_str()
            },
        );
        self.updating_launch_choices.set(false);
        let worker_root = root.clone();
        self.run_io(
            move || manager.list(Path::new(&worker_root), &live_paths),
            move |workspace, result| {
                if workspace.launch_choice_sequence.get() != sequence
                    || workspace.launch_project.text().trim() != root
                {
                    return;
                }
                let mut choices = vec![WORKTREE_PLACEHOLDER.to_owned(), root.clone()];
                if let Ok(inventory) = result {
                    let additional = inventory
                        .worktrees
                        .into_iter()
                        .map(|worktree| worktree.path.to_string_lossy().to_string())
                        .filter(|path| !choices.iter().any(|choice| choice == path))
                        .collect::<Vec<_>>();
                    choices.extend(additional);
                }
                workspace.updating_launch_choices.set(true);
                replace_string_list(&workspace.launch_worktree_choices, &choices);
                workspace.set_dropdown_path_unchecked(
                    &workspace.launch_worktree_dropdown,
                    &workspace.launch_worktree_choices,
                    workspace.launch_worktree.text().trim(),
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
                let Some(root) = selected_string(dropdown) else {
                    return;
                };
                project_workspace.launch_project.set_text(&root);
                project_workspace.launch_cwd.set_text(&root);
                project_workspace.launch_worktree.set_text("");
                project_workspace.refresh_launch_worktree_choices();
            });

        let worktree_workspace = self.clone();
        self.launch_worktree_dropdown
            .connect_selected_notify(move |dropdown| {
                if worktree_workspace.updating_launch_choices.get() {
                    return;
                }
                let Some(path) = selected_string(dropdown) else {
                    return;
                };
                worktree_workspace.launch_worktree.set_text(&path);
                worktree_workspace.launch_cwd.set_text(&path);
            });

        let project_workspace = self.clone();
        self.launch_project.connect_activate(move |_| {
            project_workspace.refresh_launch_project_choices();
            project_workspace.refresh_launch_worktree_choices();
        });
    }

    fn set_dropdown_path_unchecked(
        &self,
        dropdown: &gtk::DropDown,
        choices: &gtk::StringList,
        path: &str,
    ) {
        let selected = (0..choices.n_items())
            .find(|index| choices.string(*index).is_some_and(|value| value == path))
            .unwrap_or(gtk::INVALID_LIST_POSITION);
        dropdown.set_selected(selected);
    }
}

fn replace_string_list(model: &gtk::StringList, values: &[String]) {
    let values = values.iter().map(String::as_str).collect::<Vec<_>>();
    model.splice(0, model.n_items(), &values);
}

fn selected_string(dropdown: &gtk::DropDown) -> Option<String> {
    let value = dropdown
        .selected_item()?
        .downcast::<gtk::StringObject>()
        .ok()
        .map(|item| item.string().to_string())?;
    (!value.starts_with("[choose ")).then_some(value)
}

fn optional_path(entry: &gtk::Entry) -> Option<PathBuf> {
    optional_text(entry).map(PathBuf::from)
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
