use super::*;

/// Window actions behind the menus. Disabling an action greys out every entry that uses it.
#[derive(Clone)]
pub(super) struct Menus {
    pub(super) agents: gio::SimpleAction,
    pub(super) worktrees: gio::SimpleAction,
    pub(super) pull_requests: gio::SimpleAction,
    pub(super) preferences: gio::SimpleAction,
    pub(super) shortcuts: gio::SimpleAction,
    pub(super) magit: gio::SimpleAction,
    pub(super) branch_review: gio::SimpleAction,
    pub(super) restart_agents: gio::SimpleAction,
    /// The section whose Pull Requests label carries the attention count.
    surfaces: gio::Menu,
    pub(super) main_button: gtk::MenuButton,
    pub(super) git_button: gtk::MenuButton,
}

impl Menus {
    pub(super) fn build() -> Self {
        let action = |name: &str| {
            let action = gio::SimpleAction::new(name, None);
            action.set_enabled(false);
            action
        };
        let surfaces = gio::Menu::new();
        surfaces.append(Some("Recent Sessions"), Some("win.show-agents"));
        surfaces.append(Some("Worktrees"), Some("win.show-worktrees"));
        surfaces.append(Some("Pull Requests"), Some("win.show-pull-requests"));
        let agents = gio::Menu::new();
        agents.append(Some("Restart Idle Agents"), Some("win.restart-idle-agents"));
        let settings = gio::Menu::new();
        settings.append(Some("Preferences"), Some("win.preferences"));
        settings.append(Some("Keyboard Shortcuts"), Some("win.show-shortcuts"));
        let main = gio::Menu::new();
        main.append_section(None, &surfaces);
        main.append_section(None, &agents);
        main.append_section(None, &settings);
        let main_button = gtk::MenuButton::builder()
            .icon_name("open-menu-symbolic")
            .menu_model(&main)
            .primary(true)
            .tooltip_text("Main Menu")
            .build();

        let emacs = gio::Menu::new();
        emacs.append(Some("Magit"), Some("win.magit"));
        emacs.append(Some("Branch Review"), Some("win.branch-review"));
        let git_button = gtk::MenuButton::builder()
            .icon_name("text-editor-symbolic")
            .menu_model(&emacs)
            .tooltip_text("Open the selected worktree in Emacs")
            .build();
        git_button.add_css_class("flat");

        Self {
            agents: action("show-agents"),
            worktrees: action("show-worktrees"),
            pull_requests: action("show-pull-requests"),
            preferences: action("preferences"),
            shortcuts: action("show-shortcuts"),
            magit: action("magit"),
            branch_review: action("branch-review"),
            restart_agents: action("restart-idle-agents"),
            surfaces,
            main_button,
            git_button,
        }
    }

    pub(super) fn set_pull_request_attention(&self, count: usize) {
        let label = if count == 0 {
            "Pull Requests".to_owned()
        } else {
            format!("Pull Requests ({count})")
        };
        self.surfaces.remove(2);
        self.surfaces
            .insert(2, Some(&label), Some("win.show-pull-requests"));
    }

    pub(super) fn enable_surfaces(&self) {
        for action in [
            &self.agents,
            &self.pull_requests,
            &self.preferences,
            &self.shortcuts,
            &self.restart_agents,
        ] {
            action.set_enabled(true);
        }
    }
}

/// A menu entry that passes a string, such as a session ID, to its action.
pub(super) fn targeted_item(label: &str, action: &str, target: &str) -> gio::MenuItem {
    let item = gio::MenuItem::new(Some(label), None);
    item.set_action_and_target_value(Some(action), Some(&target.to_variant()));
    item
}

/// Right-clicking `widget` opens the same menu as its overflow button.
pub(super) fn open_menu_on_right_click(widget: &impl IsA<gtk::Widget>, button: &gtk::MenuButton) {
    let click = gtk::GestureClick::builder()
        .button(gtk::gdk::BUTTON_SECONDARY)
        .build();
    let button = button.downgrade();
    click.connect_pressed(move |gesture, _, _, _| {
        if let Some(button) = button.upgrade() {
            gesture.set_state(gtk::EventSequenceState::Claimed);
            button.popup();
        }
    });
    widget.add_controller(click);
}

impl Workspace {
    pub(super) fn install_menu_actions(&self) {
        let menus = &self.menus;
        let workspace = self.clone();
        menus.agents.connect_activate(move |_, _| {
            workspace.open_agents();
        });
        let workspace = self.clone();
        menus.worktrees.connect_activate(move |_, _| {
            workspace.prepare_worktree_panel();
            workspace.worktrees.modal.present();
        });
        let workspace = self.clone();
        menus.pull_requests.connect_activate(move |_, _| {
            if let Some(root) = workspace.preferred_project_root() {
                workspace.open_pr_for_project(&root);
            }
        });
        let workspace = self.clone();
        menus.preferences.connect_activate(move |_, _| {
            workspace.preferences.open(preferences_ui::APPEARANCE_PAGE);
        });
        let workspace = self.clone();
        menus.shortcuts.connect_activate(move |_, _| {
            workspace.preferences.open(preferences_ui::SHORTCUTS_PAGE);
        });
        let workspace = self.clone();
        menus.restart_agents.connect_activate(move |_, _| {
            workspace.restart_idle_agents();
        });
        let workspace = self.clone();
        menus.magit.connect_activate(move |_, _| {
            if let Some(id) = workspace.selected_session_id() {
                workspace.open_session_in_emacs(&id, crate::emacs::EmacsAction::Magit, None);
            }
        });
        let workspace = self.clone();
        menus.branch_review.connect_activate(move |_, _| {
            if let Some(id) = workspace.selected_session_id() {
                workspace.open_session_in_emacs(&id, crate::emacs::EmacsAction::BranchReview, None);
            }
        });
        for action in [
            &menus.agents,
            &menus.worktrees,
            &menus.pull_requests,
            &menus.preferences,
            &menus.shortcuts,
            &menus.magit,
            &menus.branch_review,
            &menus.restart_agents,
        ] {
            self.window.add_action(action);
        }
        self.application
            .set_accels_for_action("win.preferences", &["<Control>comma"]);

        self.add_target_action("session-rename", |workspace, id| {
            workspace.start_session_rename(id)
        });
        self.add_target_action("session-launch-here", |workspace, id| {
            if let Some((root, worktree)) = workspace.session_launch_target(id) {
                workspace.open_launch_for_worktree(&root, &worktree);
            }
        });
        self.add_target_action("session-resume", |workspace, id| {
            workspace.resume_exited_session(id)
        });
        self.add_target_action("session-restart", |workspace, id| {
            workspace.restart_agent(id, None)
        });
        self.add_target_action("session-close", |workspace, id| {
            workspace.stop_session(id, None)
        });
        self.add_target_action("project-resume", |workspace, root| {
            workspace.open_agent_for_project(root)
        });
        self.add_target_action("project-worktrees", |workspace, root| {
            workspace.open_worktrees_for_project(root)
        });
        self.add_target_action("project-pull-requests", |workspace, root| {
            workspace.open_pr_for_project(root)
        });
        self.add_target_action("open-in-emacs", |workspace, path| {
            workspace.open_file_in_emacs(PathBuf::from(path), None, None)
        });
        self.add_target_action("copy-text", |workspace, text| {
            workspace.window.clipboard().set_text(text);
        });
        self.add_target_action("project-pin", |workspace, root| {
            let pinned = workspace
                .project_summaries()
                .into_iter()
                .any(|project| project.root == root && project.is_pinned);
            workspace.set_project_preferences(root.to_owned(), Some(!pinned), None, None);
        });
    }

    fn add_target_action(&self, name: &str, activate: impl Fn(&Self, &str) + 'static) {
        let action = gio::SimpleAction::new(name, Some(glib::VariantTy::STRING));
        let workspace = self.clone();
        action.connect_activate(move |_, target| {
            if let Some(target) = target.and_then(|target| target.get::<String>()) {
                activate(&workspace, &target);
            }
        });
        self.window.add_action(&action);
    }
}
