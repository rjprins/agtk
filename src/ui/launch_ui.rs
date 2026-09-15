use super::*;

impl Workspace {
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
        self.launch_popover.popup();
    }

    pub(super) fn open_launch_for_worktree(&self, project_root: &Path, worktree: &Path) {
        self.launch_project
            .set_text(project_root.to_string_lossy().as_ref());
        self.launch_worktree
            .set_text(worktree.to_string_lossy().as_ref());
        self.launch_cwd
            .set_text(worktree.to_string_lossy().as_ref());
        self.worktree_popover.popdown();
        self.launch_popover.popup();
    }
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
