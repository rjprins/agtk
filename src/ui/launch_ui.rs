#![allow(deprecated)]

use super::*;
use crate::launch_model::{self, DEFAULT_BASE_BRANCH};
use crate::worktrees::WorktreeManager;
use serde_json::Value;
use std::collections::BTreeMap;

const PROJECT_PLACEHOLDER: &str = "Search projects or type a path…";
const WORKTREE_PLACEHOLDER: &str = "Search worktrees…";
// One width for every location control so their edges line up.
const LOCATION_CONTROL_WIDTH: i32 = 320;

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
struct WorktreeModePresentation {
    existing_row_visible: bool,
    new_row_visible: bool,
    existing_fields_enabled: bool,
    new_fields_enabled: bool,
}

const fn worktree_mode_presentation(creating: bool) -> WorktreeModePresentation {
    WorktreeModePresentation {
        existing_row_visible: true,
        new_row_visible: true,
        existing_fields_enabled: !creating,
        new_fields_enabled: creating,
    }
}

/// Everything the launch dialog shows or remembers between openings.
#[derive(Clone)]
pub(super) struct LaunchDialog {
    pub(super) modal: modal::Modal,
    pub(super) cancel: gtk::Button,
    pub(super) submit: gtk::Button,
    pub(super) agent_dropdown: gtk::DropDown,
    pub(super) agent_choices: gtk::StringList,
    pub(super) agent_buttons: Rc<Vec<(SessionKind, gtk::ToggleButton)>>,
    pub(super) agent_options: gtk::Stack,
    pub(super) claude_permission: adw::ComboRow,
    pub(super) claude_danger: adw::SwitchRow,
    pub(super) codex_approval: adw::ComboRow,
    pub(super) codex_sandbox: adw::ComboRow,
    pub(super) codex_full_auto: adw::SwitchRow,
    pub(super) codex_bypass: adw::SwitchRow,
    pub(super) gemini_approval: adw::ComboRow,
    pub(super) gemini_yolo: adw::SwitchRow,
    pub(super) cwd: gtk::Entry,
    pub(super) project: gtk::Entry,
    pub(super) project_dropdown: gtk::DropDown,
    pub(super) project_choices: gtk::StringList,
    pub(super) project_completion: gtk::ListStore,
    pub(super) project_completion_items: Rc<RefCell<Vec<(String, String)>>>,
    pub(super) worktree: gtk::Entry,
    pub(super) worktree_dropdown: gtk::DropDown,
    pub(super) worktree_choices: gtk::StringList,
    pub(super) worktree_completion: gtk::ListStore,
    pub(super) worktree_completion_items: Rc<RefCell<Vec<(String, String)>>>,
    pub(super) existing_worktree_radio: gtk::CheckButton,
    pub(super) new_worktree_radio: gtk::CheckButton,
    pub(super) existing_worktree_mode: adw::ActionRow,
    pub(super) new_worktree_mode: adw::ActionRow,
    pub(super) name: adw::EntryRow,
    pub(super) args: adw::EntryRow,
    pub(super) prompt: adw::EntryRow,
    pub(super) branch: gtk::Entry,
    pub(super) base_branch: gtk::Entry,
    pub(super) base_branch_dropdown: gtk::DropDown,
    pub(super) base_branch_choices: gtk::StringList,
    pub(super) base_branch_completion: gtk::ListStore,
    pub(super) base_branch_completion_items: Rc<RefCell<Vec<(String, String)>>>,
    pub(super) worktree_values: Rc<RefCell<Vec<Option<String>>>>,
    pub(super) project_values: Rc<RefCell<Vec<Option<String>>>>,
    pub(super) project_custom: Rc<Cell<bool>>,
    pub(super) creating_worktree: Rc<Cell<bool>>,
    pub(super) choice_sequence: Rc<Cell<u64>>,
    pub(super) updating_choices: Rc<Cell<bool>>,
}

impl LaunchDialog {
    pub(super) fn build(parent: &adw::ApplicationWindow) -> Self {
        let page = adw::PreferencesPage::new();

        let agent_choices = gtk::StringList::new(&["claude", "codex", "gemini", "shell"]);
        let agent_dropdown =
            gtk::DropDown::new(Some(agent_choices.clone()), None::<&gtk::Expression>);
        agent_dropdown.set_selected(0);
        agent_dropdown.set_visible(false);
        let agent_box = gtk::Box::new(gtk::Orientation::Horizontal, 0);
        agent_box.add_css_class("linked");
        agent_box.set_valign(gtk::Align::Center);
        let mut agent_buttons = Vec::new();
        let mut previous: Option<gtk::ToggleButton> = None;
        for (kind, label) in [
            (SessionKind::Claude, "Claude"),
            (SessionKind::Codex, "Codex"),
            (SessionKind::Gemini, "Gemini"),
            (SessionKind::Shell, "Shell"),
        ] {
            let button = gtk::ToggleButton::with_label(label);
            if let Some(previous) = previous.as_ref() {
                button.set_group(Some(previous));
            }
            button.set_active(kind == SessionKind::Shell);
            agent_box.append(&button);
            previous = Some(button.clone());
            agent_buttons.push((kind, button));
        }
        agent_box.append(&agent_dropdown);
        let agent_row = adw::ActionRow::builder().title("Agent").build();
        agent_row.add_suffix(&agent_box);
        let agent_group = adw::PreferencesGroup::new();
        agent_group.add(&agent_row);
        page.add(&agent_group);

        let (claude_permission, claude_danger) = claude_launch_options();
        let (codex_approval, codex_sandbox, codex_full_auto, codex_bypass) = codex_launch_options();
        let (gemini_approval, gemini_yolo) = gemini_launch_options();
        // Keep the original agmux choices and default to its on-request policy.
        codex_approval.set_selected(2);
        claude_danger.set_active(true);
        codex_full_auto.set_active(true);
        let bypass = codex_bypass.clone();
        codex_full_auto.connect_active_notify(move |toggle| {
            if toggle.is_active() {
                bypass.set_active(false);
            }
        });
        let full_auto = codex_full_auto.clone();
        codex_bypass.connect_active_notify(move |toggle| {
            if toggle.is_active() {
                full_auto.set_active(false);
            }
        });
        let agent_options = gtk::Stack::builder()
            .transition_type(gtk::StackTransitionType::None)
            .vhomogeneous(false)
            .build();
        agent_options.add_named(
            &option_list(&[claude_permission.upcast_ref(), claude_danger.upcast_ref()]),
            Some("claude"),
        );
        agent_options.add_named(
            &option_list(&[
                codex_approval.upcast_ref(),
                codex_sandbox.upcast_ref(),
                codex_full_auto.upcast_ref(),
                codex_bypass.upcast_ref(),
            ]),
            Some("codex"),
        );
        agent_options.add_named(
            &option_list(&[gemini_approval.upcast_ref(), gemini_yolo.upcast_ref()]),
            Some("gemini"),
        );
        agent_options.add_named(&gtk::Box::new(gtk::Orientation::Vertical, 0), Some("shell"));
        let options_group = adw::PreferencesGroup::builder().title("Options").build();
        options_group.add(&agent_options);
        let shown_options = options_group.clone();
        agent_options.connect_visible_child_name_notify(move |stack| {
            shown_options.set_visible(stack.visible_child_name().as_deref() != Some("shell"));
        });
        page.add(&options_group);

        let project = path_entry("Project directory", PROJECT_PLACEHOLDER, 26);
        let project_choices = gtk::StringList::new(&[]);
        let project_dropdown = searchable_path_dropdown(&project_choices);
        let project_completion =
            gtk::ListStore::new(&[String::static_type(), String::static_type()]);
        let project_completion_items = Rc::new(RefCell::new(Vec::new()));
        install_choice_completion(
            &project,
            &project_completion,
            project_completion_items.clone(),
            true,
        );
        let project_control =
            editable_choice_control(&project, &project_dropdown, "Choose a project");
        let project_row = adw::ActionRow::builder().title("Project").build();
        project_row.add_suffix(&project_control);

        let worktree = path_entry("Existing worktree", WORKTREE_PLACEHOLDER, 26);
        let worktree_choices = gtk::StringList::new(&[]);
        let worktree_dropdown = searchable_path_dropdown(&worktree_choices);
        let worktree_completion =
            gtk::ListStore::new(&[String::static_type(), String::static_type()]);
        let worktree_completion_items = Rc::new(RefCell::new(Vec::new()));
        install_choice_completion(
            &worktree,
            &worktree_completion,
            worktree_completion_items.clone(),
            true,
        );
        let worktree_control =
            editable_choice_control(&worktree, &worktree_dropdown, "Choose an existing worktree");
        let existing_worktree_radio = gtk::CheckButton::new();
        let new_worktree_radio = gtk::CheckButton::new();
        new_worktree_radio.set_group(Some(&existing_worktree_radio));
        existing_worktree_radio.set_active(true);
        let existing_worktree_mode = adw::ActionRow::builder()
            .title("Existing worktree")
            .activatable_widget(&existing_worktree_radio)
            .build();
        existing_worktree_mode.add_prefix(&existing_worktree_radio);
        existing_worktree_mode.add_suffix(&worktree_control);

        let branch = path_entry("New worktree name", "concise-kebab-case", 26);
        branch.set_tooltip_text(Some("Used as the branch and worktree name"));
        let base_branch = path_entry("Base branch", DEFAULT_BASE_BRANCH, 22);
        base_branch.set_text(DEFAULT_BASE_BRANCH);
        let base_branch_choices = gtk::StringList::new(&[]);
        let base_branch_dropdown = searchable_path_dropdown(&base_branch_choices);
        let base_branch_completion =
            gtk::ListStore::new(&[String::static_type(), String::static_type()]);
        let base_branch_completion_items = Rc::new(RefCell::new(Vec::new()));
        install_choice_completion(
            &base_branch,
            &base_branch_completion,
            base_branch_completion_items.clone(),
            false,
        );
        let base_control = editable_choice_control(
            &base_branch,
            &base_branch_dropdown,
            "Choose the base branch",
        );
        fit_location_control(&branch);
        let new_worktree_mode = adw::ActionRow::builder()
            .title("New worktree")
            .activatable_widget(&new_worktree_radio)
            .build();
        new_worktree_mode.add_prefix(&new_worktree_radio);
        new_worktree_mode.add_suffix(&branch);
        let base_row = adw::ActionRow::builder()
            .title("Base branch")
            .subtitle("For a new worktree")
            .build();
        base_row.add_suffix(&base_control);

        let location_group = adw::PreferencesGroup::builder().title("Location").build();
        location_group.add(&project_row);
        location_group.add(&existing_worktree_mode);
        location_group.add(&new_worktree_mode);
        location_group.add(&base_row);
        page.add(&location_group);

        let name = adw::EntryRow::builder().title("Session Name").build();
        let prompt = adw::EntryRow::builder().title("Initial Prompt").build();
        let args = adw::EntryRow::builder()
            .title("Extra Arguments (JSON array)")
            .build();
        args.set_tooltip_text(Some(
            "For example [\"--model\", \"opus\"]; remembered per launch",
        ));
        let advanced = adw::ExpanderRow::builder()
            .title("Advanced")
            .subtitle("Name, initial prompt and extra arguments")
            .build();
        advanced.add_row(&name);
        advanced.add_row(&prompt);
        advanced.add_row(&args);
        let advanced_group = adw::PreferencesGroup::new();
        advanced_group.add(&advanced);
        page.add(&advanced_group);

        let cancel = gtk::Button::with_label("Cancel");
        let submit = gtk::Button::with_label("Launch");
        submit.add_css_class("suggested-action");
        let modal = modal::Modal::new(parent, "Launch Session", 720, 760, &page);
        let header = modal.header();
        header.set_show_start_title_buttons(false);
        header.set_show_end_title_buttons(false);
        header.pack_start(&cancel);
        header.pack_end(&submit);
        modal.set_default_widget(&submit);

        Self {
            modal,
            cancel,
            submit,
            agent_dropdown,
            agent_choices,
            agent_buttons: Rc::new(agent_buttons),
            agent_options,
            claude_permission,
            claude_danger,
            codex_approval,
            codex_sandbox,
            codex_full_auto,
            codex_bypass,
            gemini_approval,
            gemini_yolo,
            // Not shown; they carry values between launches and from quick launch.
            cwd: gtk::Entry::new(),
            project,
            project_dropdown,
            project_choices,
            project_completion,
            project_completion_items,
            worktree,
            worktree_dropdown,
            worktree_choices,
            worktree_completion,
            worktree_completion_items,
            existing_worktree_radio,
            new_worktree_radio,
            existing_worktree_mode,
            new_worktree_mode,
            name,
            args,
            prompt,
            branch,
            base_branch,
            base_branch_dropdown,
            base_branch_choices,
            base_branch_completion,
            base_branch_completion_items,
            worktree_values: Rc::new(RefCell::new(Vec::new())),
            project_values: Rc::new(RefCell::new(Vec::new())),
            project_custom: Rc::new(Cell::new(false)),
            creating_worktree: Rc::new(Cell::new(false)),
            choice_sequence: Rc::new(Cell::new(0)),
            updating_choices: Rc::new(Cell::new(false)),
        }
    }
}

fn option_list(rows: &[&gtk::ListBoxRow]) -> gtk::ListBox {
    let list = gtk::ListBox::new();
    list.set_selection_mode(gtk::SelectionMode::None);
    list.add_css_class("boxed-list");
    for row in rows {
        list.append(*row);
    }
    list
}

fn combo_row(title: &str, flag: &str, values: &[&str]) -> adw::ComboRow {
    adw::ComboRow::builder()
        .title(title)
        .subtitle(flag)
        .model(&gtk::StringList::new(values))
        .enable_search(true)
        .build()
}

fn switch_row(title: &str, flag: &str) -> adw::SwitchRow {
    adw::SwitchRow::builder()
        .title(title)
        .subtitle(flag)
        .build()
}

fn fit_location_control(widget: &impl IsA<gtk::Widget>) {
    widget.set_size_request(LOCATION_CONTROL_WIDTH, -1);
    widget.set_hexpand(false);
    widget.set_valign(gtk::Align::Center);
}

fn path_entry(label: &str, placeholder: &str, width: i32) -> gtk::Entry {
    let entry = gtk::Entry::builder()
        .placeholder_text(placeholder)
        .width_chars(width)
        .build();
    entry.update_property(&[
        gtk::accessible::Property::Label(label),
        gtk::accessible::Property::Autocomplete(gtk::AccessibleAutocomplete::List),
    ]);
    entry
}

impl Workspace {
    fn set_launch_worktree_mode(&self, creating: bool) {
        self.launch.creating_worktree.set(creating);
        let presentation = worktree_mode_presentation(creating);
        self.launch
            .existing_worktree_mode
            .set_visible(presentation.existing_row_visible);
        self.launch
            .new_worktree_mode
            .set_visible(presentation.new_row_visible);
        self.launch
            .worktree
            .set_sensitive(presentation.existing_fields_enabled);
        self.launch
            .worktree_dropdown
            .set_sensitive(presentation.existing_fields_enabled);
        self.launch
            .branch
            .set_sensitive(presentation.new_fields_enabled);
        self.launch
            .base_branch
            .set_sensitive(presentation.new_fields_enabled);
        self.launch
            .base_branch_dropdown
            .set_sensitive(presentation.new_fields_enabled);
        if creating {
            self.launch.cwd.set_text(self.launch.project.text().trim());
            if self.launch.branch.text().trim().is_empty() {
                self.launch.branch.set_text(&generated_branch_name());
            }
            if self.launch.base_branch.text().trim().is_empty()
                || self.launch.base_branch.text() == "main"
            {
                self.launch.base_branch.set_text(DEFAULT_BASE_BRANCH);
            }
            self.launch.new_worktree_radio.set_active(true);
        } else {
            let worktree = self.launch.worktree.text().trim().to_owned();
            let worktree = if worktree.is_empty() {
                self.launch.project.text().trim().to_owned()
            } else {
                worktree
            };
            self.launch.worktree.set_text(&worktree);
            self.launch.cwd.set_text(&worktree);
            self.launch.existing_worktree_radio.set_active(true);
        }
    }

    pub(super) fn prepare_launch_panel(&self) {
        if self.launch.project.text().trim().is_empty()
            && let Some(root) = self.preferred_project_root()
        {
            self.launch.project.set_text(&root);
            self.launch.cwd.set_text(&root);
        }
        self.refresh_launch_project_choices();
        self.refresh_launch_worktree_choices();
        self.refresh_launch_base_branches();
        self.set_launch_worktree_mode(self.launch.creating_worktree.get());
        self.set_launch_agent(self.quick_launch.borrow().kind);
    }

    pub(super) fn load_quick_launch(&self, preferences: QuickLaunchPreferences) {
        self.launch.cwd.set_text(&display_path(
            preferences
                .cwd
                .as_ref()
                .or(preferences.worktree_path.as_ref()),
        ));
        self.launch
            .project
            .set_text(&display_path(preferences.project_root.as_ref()));
        self.launch
            .worktree
            .set_text(&display_path(preferences.worktree_path.as_ref()));
        self.set_launch_worktree_mode(false);
        self.launch.args.set_text(&if preferences.args.is_empty() {
            String::new()
        } else {
            serde_json::to_string(&preferences.args).unwrap_or_default()
        });
        self.apply_launch_flags(&preferences.flags);
        self.set_launch_agent(preferences.kind);
        self.launch.project_custom.set(false);
        *self.quick_launch.borrow_mut() = preferences;
        self.refresh_launch_project_choices();
        self.refresh_launch_worktree_choices();
    }

    pub(super) fn launch_from_form(&self, kind: SessionKind) {
        let raw_args = match parse_arguments(&self.launch.args) {
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
        let creating_worktree = self.launch.creating_worktree.get();
        let project_root = optional_path(&self.launch.project);
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
                    optional_path(&self.launch.worktree).as_deref(),
                )
                .or_else(|| optional_path(&self.launch.cwd))
            },
            project_root: project_root.clone(),
            worktree_path: (!creating_worktree)
                .then(|| {
                    launch_model::effective_worktree_path(
                        project_root.as_deref(),
                        optional_path(&self.launch.worktree).as_deref(),
                    )
                })
                .flatten(),
        };
        let name = optional_text(&self.launch.name);
        let initial_input = optional_text(&self.launch.prompt);
        // Name and prompt belong to one launch; arguments are remembered.
        self.launch.name.set_text("");
        self.launch.prompt.set_text("");
        if let Ok(value) = serde_json::to_value(&preferences) {
            self.save_preference("quickLaunch", value);
        }
        *self.quick_launch.borrow_mut() = preferences.clone();
        if creating_worktree {
            let root = project_root.expect("validated project root");
            let branch = optional_text(&self.launch.branch).unwrap_or_else(generated_branch_name);
            let base = optional_text(&self.launch.base_branch);
            let purpose = name.clone().unwrap_or_else(|| branch.clone());
            let manager = WorktreeManager::new(self.paths.attic_dir());
            self.launch.modal.hide();
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
            self.launch.modal.hide();
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
        self.launch.project_custom.set(false);
        self.launch.project.set_text(root);
        self.launch.cwd.set_text(root);
        self.launch.worktree.set_text(root);
        self.set_launch_worktree_mode(false);
        self.launch.branch.set_text(&generated_branch_name());
        self.launch.base_branch.set_text(DEFAULT_BASE_BRANCH);
        self.refresh_launch_project_choices();
        self.refresh_launch_worktree_choices();
        self.launch.modal.present();
    }

    pub(super) fn open_launch_for_worktree(&self, project_root: &Path, worktree: &Path) {
        self.launch.project_custom.set(false);
        self.launch
            .project
            .set_text(project_root.to_string_lossy().as_ref());
        self.launch
            .worktree
            .set_text(worktree.to_string_lossy().as_ref());
        self.set_launch_worktree_mode(false);
        self.launch
            .cwd
            .set_text(worktree.to_string_lossy().as_ref());
        self.refresh_launch_project_choices();
        self.refresh_launch_worktree_choices();
        self.worktrees.modal.hide();
        self.launch.modal.present();
    }

    pub(super) fn refresh_launch_project_choices(&self) {
        let current = self.launch.project.text().trim().to_owned();
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
        *self.launch.project_values.borrow_mut() = values;
        self.launch.updating_choices.set(true);
        replace_string_list(&self.launch.project_choices, &labels);
        replace_completion_items(
            &self.launch.project_completion,
            &self.launch.project_completion_items,
            completion_items,
        );
        self.set_dropdown_value(
            &self.launch.project_dropdown,
            &self.launch.project_values.borrow(),
            (!current.is_empty()).then_some(current.as_str()),
        );
        self.launch.updating_choices.set(false);
    }

    pub(super) fn refresh_launch_worktree_choices(&self) {
        let root = launch_path_text(&self.launch.project).unwrap_or_default();
        let sequence = self.launch.choice_sequence.get().wrapping_add(1);
        self.launch.choice_sequence.set(sequence);
        if root.is_empty() {
            self.launch.updating_choices.set(true);
            let labels = vec![WORKTREE_PLACEHOLDER.to_owned()];
            *self.launch.worktree_values.borrow_mut() = vec![None];
            replace_string_list(&self.launch.worktree_choices, &labels);
            replace_completion_items(
                &self.launch.worktree_completion,
                &self.launch.worktree_completion_items,
                Vec::new(),
            );
            self.set_dropdown_value(
                &self.launch.worktree_dropdown,
                &self.launch.worktree_values.borrow(),
                None,
            );
            self.launch.updating_choices.set(false);
            return;
        }
        let manager = WorktreeManager::new(self.paths.attic_dir());
        let initial = launch_model::worktree_choices(&root, []);
        let selected_worktree = self.launch.worktree.text().trim().to_owned();
        self.launch.updating_choices.set(true);
        *self.launch.worktree_values.borrow_mut() = initial
            .iter()
            .map(|choice| Some(choice.value.clone()))
            .collect();
        replace_string_list(
            &self.launch.worktree_choices,
            &initial
                .iter()
                .map(|choice| choice.label.clone())
                .collect::<Vec<_>>(),
        );
        replace_completion_items(
            &self.launch.worktree_completion,
            &self.launch.worktree_completion_items,
            initial
                .iter()
                .map(|choice| (choice.label.clone(), choice.value.clone()))
                .collect(),
        );
        self.set_dropdown_value(
            &self.launch.worktree_dropdown,
            &self.launch.worktree_values.borrow(),
            if selected_worktree.is_empty() {
                Some(root.as_str())
            } else {
                Some(selected_worktree.as_str())
            },
        );
        self.launch.updating_choices.set(false);
        let worker_root = root.clone();
        self.run_io(
            move || manager.linked_paths(Path::new(&worker_root)),
            move |workspace, result| {
                if workspace.launch.choice_sequence.get() != sequence
                    || launch_path_text(&workspace.launch.project).as_deref() != Some(root.as_str())
                {
                    return;
                }
                let mut worktrees = Vec::new();
                if let Ok(paths) = result {
                    worktrees = paths
                        .into_iter()
                        .map(|path| {
                            let path = path.to_string_lossy().to_string();
                            let label = project_name(&path);
                            (path, label)
                        })
                        .collect::<Vec<_>>();
                }
                let choices = launch_model::worktree_choices(&root, worktrees);
                let labels = choices
                    .iter()
                    .map(|choice| choice.label.clone())
                    .collect::<Vec<_>>();
                workspace.launch.updating_choices.set(true);
                *workspace.launch.worktree_values.borrow_mut() = choices
                    .iter()
                    .map(|choice| Some(choice.value.clone()))
                    .collect();
                replace_string_list(&workspace.launch.worktree_choices, &labels);
                replace_completion_items(
                    &workspace.launch.worktree_completion,
                    &workspace.launch.worktree_completion_items,
                    choices
                        .iter()
                        .map(|choice| (choice.label.clone(), choice.value.clone()))
                        .collect(),
                );
                let selected = workspace.launch.worktree.text().trim().to_owned();
                workspace.set_dropdown_value(
                    &workspace.launch.worktree_dropdown,
                    &workspace.launch.worktree_values.borrow(),
                    if selected.is_empty() {
                        Some(root.as_str())
                    } else {
                        Some(selected.as_str())
                    },
                );
                workspace.launch.updating_choices.set(false);
            },
        );
    }

    pub(super) fn connect_launch_path_controls(&self) {
        let existing_workspace = self.clone();
        self.launch
            .existing_worktree_radio
            .connect_toggled(move |radio| {
                if radio.is_active() {
                    existing_workspace.set_launch_worktree_mode(false);
                }
            });

        let new_workspace = self.clone();
        self.launch
            .new_worktree_radio
            .connect_toggled(move |radio| {
                if radio.is_active() {
                    new_workspace.set_launch_worktree_mode(true);
                }
            });

        let project_workspace = self.clone();
        self.launch
            .project_dropdown
            .connect_selected_notify(move |dropdown| {
                if project_workspace.launch.updating_choices.get() {
                    return;
                }
                let Some(root) = project_workspace.selected_project_path(dropdown) else {
                    return;
                };
                project_workspace.launch.project.set_text(&root);
                project_workspace.launch.project_custom.set(false);
                project_workspace.launch.cwd.set_text(&root);
                // Selecting a different project also selects its main
                // directory. This keeps the visible worktree control and the
                // launch target in sync until the user picks another tree.
                project_workspace.launch.worktree.set_text(&root);
                project_workspace.set_launch_worktree_mode(false);
                project_workspace
                    .launch
                    .branch
                    .set_text(&generated_branch_name());
                project_workspace
                    .launch
                    .base_branch
                    .set_text(DEFAULT_BASE_BRANCH);
                project_workspace.refresh_launch_worktree_choices();
                project_workspace.refresh_launch_base_branches();
            });

        let worktree_workspace = self.clone();
        self.launch
            .worktree_dropdown
            .connect_selected_notify(move |dropdown| {
                if worktree_workspace.launch.updating_choices.get() {
                    return;
                }
                let Some(path) = worktree_workspace.selected_worktree_path(dropdown) else {
                    return;
                };
                worktree_workspace.set_launch_worktree_mode(false);
                worktree_workspace.launch.worktree.set_text(&path);
                worktree_workspace.launch.cwd.set_text(&path);
            });

        let worktree_entry_workspace = self.clone();
        self.launch.worktree.connect_changed(move |entry| {
            if worktree_entry_workspace.launch.updating_choices.get()
                || worktree_entry_workspace.launch.creating_worktree.get()
            {
                return;
            }
            let value = entry.text().trim().to_owned();
            worktree_entry_workspace.launch.cwd.set_text(&value);
        });

        let project_workspace = self.clone();
        self.launch.project.connect_activate(move |_| {
            project_workspace.launch.project_custom.set(true);
            project_workspace.set_launch_worktree_mode(false);
            let project_root = project_workspace.launch.project.text().trim().to_owned();
            if !project_root.is_empty() {
                project_workspace.launch.worktree.set_text(&project_root);
                project_workspace.launch.cwd.set_text(&project_root);
            }
            project_workspace.refresh_launch_project_choices();
            project_workspace.refresh_launch_worktree_choices();
            project_workspace.refresh_launch_base_branches();
        });

        let project_completion_workspace = self.clone();
        self.launch.project.connect_changed(move |entry| {
            if project_completion_workspace.launch.updating_choices.get()
                || !entry.has_focus()
                || !project_completion_workspace
                    .launch
                    .project_values
                    .borrow()
                    .iter()
                    .any(|value| value.as_deref() == Some(entry.text().trim()))
            {
                return;
            }
            let root = entry.text().trim().to_owned();
            project_completion_workspace
                .launch
                .project_custom
                .set(false);
            project_completion_workspace.launch.cwd.set_text(&root);
            project_completion_workspace.launch.worktree.set_text(&root);
            project_completion_workspace.set_launch_worktree_mode(false);
            project_completion_workspace
                .launch
                .branch
                .set_text(&generated_branch_name());
            project_completion_workspace
                .launch
                .base_branch
                .set_text(DEFAULT_BASE_BRANCH);
            project_completion_workspace.refresh_launch_worktree_choices();
            project_completion_workspace.refresh_launch_base_branches();
        });

        let base_workspace = self.clone();
        self.launch
            .base_branch_dropdown
            .connect_selected_notify(move |dropdown| {
                if base_workspace.launch.updating_choices.get() {
                    return;
                }
                let Some(value) = dropdown
                    .selected_item()
                    .and_then(|item| item.downcast::<gtk::StringObject>().ok())
                    .map(|item| item.string().to_string())
                else {
                    return;
                };
                if !value.is_empty() {
                    base_workspace.launch.base_branch.set_text(&value);
                }
            });
    }

    fn refresh_launch_base_branches(&self) {
        let root = launch_path_text(&self.launch.project).unwrap_or_default();
        if root.is_empty() {
            let branches = base_branch_choices(Vec::new());
            self.launch.updating_choices.set(true);
            replace_string_list(&self.launch.base_branch_choices, &branches);
            self.launch
                .base_branch
                .set_placeholder_text(Some(DEFAULT_BASE_BRANCH));
            replace_completion_items(
                &self.launch.base_branch_completion,
                &self.launch.base_branch_completion_items,
                branches
                    .iter()
                    .map(|branch| (branch.clone(), branch.clone()))
                    .collect(),
            );
            self.launch.base_branch_dropdown.set_selected(0);
            self.launch.updating_choices.set(false);
            return;
        }
        let manager = WorktreeManager::new(self.paths.attic_dir());
        self.run_io(
            move || manager.branch_names(Path::new(&root)),
            move |workspace, result| {
                let Ok(branches) = result else {
                    return;
                };
                let branches = base_branch_choices(branches);
                workspace.launch.updating_choices.set(true);
                replace_string_list(&workspace.launch.base_branch_choices, &branches);
                workspace
                    .launch
                    .base_branch
                    .set_placeholder_text(Some(if branches.is_empty() {
                        DEFAULT_BASE_BRANCH
                    } else {
                        "Search branches…"
                    }));
                replace_completion_items(
                    &workspace.launch.base_branch_completion,
                    &workspace.launch.base_branch_completion_items,
                    branches
                        .iter()
                        .map(|branch| (branch.clone(), branch.clone()))
                        .collect(),
                );
                if workspace.launch.base_branch.text().trim().is_empty() {
                    workspace.launch.base_branch.set_text(DEFAULT_BASE_BRANCH);
                }
                let selected = branches
                    .iter()
                    .position(|branch| branch == workspace.launch.base_branch.text().trim())
                    .map(|index| index as u32)
                    .unwrap_or(gtk::INVALID_LIST_POSITION);
                workspace.launch.base_branch_dropdown.set_selected(selected);
                workspace.launch.updating_choices.set(false);
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
        self.launch
            .project_values
            .borrow()
            .get(dropdown.selected() as usize)
            .and_then(Clone::clone)
    }

    fn selected_worktree_path(&self, dropdown: &gtk::DropDown) -> Option<String> {
        self.launch
            .worktree_values
            .borrow()
            .get(dropdown.selected() as usize)
            .and_then(Clone::clone)
    }

    pub(super) fn set_launch_agent(&self, kind: SessionKind) {
        let name = session_kind_name(kind);
        let selected = (0..self.launch.agent_choices.n_items())
            .find(|index| {
                self.launch
                    .agent_choices
                    .string(*index)
                    .is_some_and(|value| value == name)
            })
            .unwrap_or(0);
        self.launch.agent_dropdown.set_selected(selected);
        for (button_kind, button) in self.launch.agent_buttons.iter() {
            button.set_active(*button_kind == kind);
        }
        self.launch.agent_options.set_visible_child_name(name);
    }

    pub(super) fn selected_launch_agent(&self) -> SessionKind {
        match self
            .launch
            .agent_dropdown
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
                    Value::String(dropdown_value(&self.launch.claude_permission)),
                );
                flags.insert(
                    "--dangerously-skip-permissions".to_owned(),
                    Value::Bool(self.launch.claude_danger.is_active()),
                );
            }
            SessionKind::Codex => {
                flags.insert(
                    "--ask-for-approval".to_owned(),
                    Value::String(dropdown_value(&self.launch.codex_approval)),
                );
                flags.insert(
                    "--sandbox".to_owned(),
                    Value::String(dropdown_value(&self.launch.codex_sandbox)),
                );
                flags.insert(
                    "--full-auto".to_owned(),
                    Value::Bool(self.launch.codex_full_auto.is_active()),
                );
                flags.insert(
                    "--dangerously-bypass-approvals-and-sandbox".to_owned(),
                    Value::Bool(self.launch.codex_bypass.is_active()),
                );
            }
            SessionKind::Gemini => {
                flags.insert(
                    "--approval-mode".to_owned(),
                    Value::String(dropdown_value(&self.launch.gemini_approval)),
                );
                flags.insert(
                    "--yolo".to_owned(),
                    Value::Bool(self.launch.gemini_yolo.is_active()),
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
                &self.launch.claude_permission,
                values.get("--permission-mode"),
            );
            set_check_value(
                &self.launch.claude_danger,
                values.get("--dangerously-skip-permissions"),
            );
        }
        if let Some(values) = flags.get("codex") {
            set_dropdown_value(
                &self.launch.codex_approval,
                values.get("--ask-for-approval"),
            );
            set_dropdown_value(&self.launch.codex_sandbox, values.get("--sandbox"));
            set_check_value(&self.launch.codex_full_auto, values.get("--full-auto"));
            set_check_value(
                &self.launch.codex_bypass,
                values.get("--dangerously-bypass-approvals-and-sandbox"),
            );
        }
        if let Some(values) = flags.get("gemini") {
            set_dropdown_value(&self.launch.gemini_approval, values.get("--approval-mode"));
            set_check_value(&self.launch.gemini_yolo, values.get("--yolo"));
        }
    }
}

fn replace_string_list(model: &gtk::StringList, values: &[String]) {
    let values = values.iter().map(String::as_str).collect::<Vec<_>>();
    model.splice(0, model.n_items(), &values);
}

fn base_branch_choices(mut branches: Vec<String>) -> Vec<String> {
    branches.retain(|branch| branch != DEFAULT_BASE_BRANCH);
    branches.insert(0, DEFAULT_BASE_BRANCH.to_owned());
    branches
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

fn optional_path(entry: &impl IsA<gtk::Editable>) -> Option<PathBuf> {
    optional_text(entry).map(|path| launch_model::expand_user_path(&path))
}

fn launch_path_text(entry: &impl IsA<gtk::Editable>) -> Option<String> {
    optional_path(entry).map(|path| path.to_string_lossy().into_owned())
}

fn optional_text(entry: &impl IsA<gtk::Editable>) -> Option<String> {
    let text = entry.text();
    let text = text.trim();
    (!text.is_empty()).then(|| text.to_owned())
}

fn display_path(path: Option<&PathBuf>) -> String {
    path.map(|path| path.to_string_lossy().to_string())
        .unwrap_or_default()
}

fn parse_arguments(entry: &impl IsA<gtk::Editable>) -> Result<Vec<String>, String> {
    let text = entry.text();
    let text = text.trim();
    if text.is_empty() {
        return Ok(Vec::new());
    }
    serde_json::from_str::<Vec<String>>(text)
        .map_err(|error| format!("Arguments must be a JSON string array: {error}"))
}

fn dropdown_value(dropdown: &adw::ComboRow) -> String {
    dropdown
        .selected_item()
        .and_then(|item| item.downcast::<gtk::StringObject>().ok())
        .map(|item| item.string().to_string())
        .unwrap_or_default()
}

fn set_dropdown_value(dropdown: &adw::ComboRow, value: Option<&Value>) {
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

fn set_check_value(check: &adw::SwitchRow, value: Option<&Value>) {
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

fn editable_choice_control(
    entry: &gtk::Entry,
    dropdown: &gtk::DropDown,
    dropdown_label: &str,
) -> gtk::Box {
    let selected_factory = gtk::SignalListItemFactory::new();
    selected_factory.connect_setup(|_, item| {
        let Some(item) = item.downcast_ref::<gtk::ListItem>() else {
            return;
        };
        item.set_child(Some(&gtk::Label::new(None)));
    });
    dropdown.set_factory(Some(&selected_factory));

    let list_factory = gtk::SignalListItemFactory::new();
    list_factory.connect_setup(|_, item| {
        let Some(item) = item.downcast_ref::<gtk::ListItem>() else {
            return;
        };
        let label = gtk::Label::new(None);
        label.set_xalign(0.0);
        item.set_child(Some(&label));
    });
    list_factory.connect_bind(|_, item| {
        let Some(item) = item.downcast_ref::<gtk::ListItem>() else {
            return;
        };
        let Some(label) = item
            .child()
            .and_then(|child| child.downcast::<gtk::Label>().ok())
        else {
            return;
        };
        let Some(value) = item
            .item()
            .and_then(|value| value.downcast::<gtk::StringObject>().ok())
        else {
            return;
        };
        label.set_label(&value.string());
    });
    dropdown.set_list_factory(Some(&list_factory));
    dropdown.set_hexpand(false);
    dropdown.set_size_request(34, -1);
    dropdown.set_tooltip_text(Some(dropdown_label));
    dropdown.update_property(&[gtk::accessible::Property::Label(dropdown_label)]);

    let control = gtk::Box::new(gtk::Orientation::Horizontal, 0);
    control.add_css_class("linked");
    fit_location_control(&control);
    entry.set_hexpand(true);
    entry.set_width_chars(1);
    control.append(entry);
    control.append(dropdown);
    control
}

fn install_choice_completion(
    entry: &gtk::Entry,
    model: &gtk::ListStore,
    items: Rc<RefCell<Vec<(String, String)>>>,
    allow_paths: bool,
) {
    let completion = gtk::EntryCompletion::builder()
        .model(model)
        .minimum_key_length(0)
        .popup_completion(true)
        .inline_completion(false)
        .text_column(0)
        .build();
    completion.set_match_func(|completion, key, iter| {
        let Some(model) = completion.model() else {
            return false;
        };
        let label = model.get_value(iter, 0).get::<String>().unwrap_or_default();
        let value = model.get_value(iter, 1).get::<String>().unwrap_or_default();
        crate::launch_model::choice_matches(key, &label, &value)
    });
    let selected_entry = entry.clone();
    completion.connect_match_selected(move |_, model, iter| {
        let Some(value) = model.get_value(iter, 1).get::<String>().ok() else {
            return glib::Propagation::Proceed;
        };
        selected_entry.set_text(&value);
        glib::Propagation::Stop
    });
    entry.set_completion(Some(&completion));
    let focus_completion = completion.clone();
    entry.connect_has_focus_notify(move |entry| {
        if entry.has_focus() {
            focus_completion.complete();
        }
    });
    let completion_model = model.clone();
    let changed_completion = completion.clone();
    entry.connect_changed(move |entry| {
        while let Some(row) = completion_model.iter_first() {
            completion_model.remove(&row);
        }
        for (label, value) in items.borrow().iter() {
            let row = completion_model.append();
            completion_model.set(&row, &[(0, label), (1, value)]);
        }
        if allow_paths {
            for completion in crate::launch_model::path_completions(entry.text().as_str()) {
                let row = completion_model.append();
                completion_model.set(&row, &[(0, &completion), (1, &completion)]);
            }
        }
        if entry.has_focus() {
            changed_completion.complete();
        }
    });
}

fn claude_launch_options() -> (adw::ComboRow, adw::SwitchRow) {
    (
        combo_row(
            "Permission mode",
            "--permission-mode",
            &["default", "acceptEdits", "bypassPermissions", "plan"],
        ),
        switch_row("Skip permission prompts", "--dangerously-skip-permissions"),
    )
}

fn codex_launch_options() -> (adw::ComboRow, adw::ComboRow, adw::SwitchRow, adw::SwitchRow) {
    (
        combo_row(
            "Ask for approval",
            "--ask-for-approval",
            &["untrusted", "on-failure", "on-request", "never"],
        ),
        combo_row(
            "Sandbox",
            "--sandbox",
            &["read-only", "workspace-write", "danger-full-access"],
        ),
        switch_row("Full auto", "--full-auto"),
        switch_row(
            "Bypass approvals and sandbox",
            "--dangerously-bypass-approvals-and-sandbox",
        ),
    )
}

fn gemini_launch_options() -> (adw::ComboRow, adw::SwitchRow) {
    (
        combo_row(
            "Approval mode",
            "--approval-mode",
            &["default", "auto_edit", "yolo", "plan"],
        ),
        switch_row("YOLO", "--yolo"),
    )
}

fn searchable_path_dropdown(model: &gtk::StringList) -> gtk::DropDown {
    let dropdown = gtk::DropDown::new(Some(model.clone()), None::<&gtk::Expression>);
    dropdown.set_enable_search(true);
    dropdown.set_show_arrow(true);
    dropdown.set_tooltip_text(Some("Search known paths or choose one"));
    dropdown
}

#[cfg(test)]
mod tests {
    use super::{base_branch_choices, worktree_mode_presentation};

    #[test]
    fn both_worktree_mode_rows_remain_visible() {
        let existing = worktree_mode_presentation(false);
        assert!(existing.existing_row_visible);
        assert!(existing.new_row_visible);
        assert!(existing.existing_fields_enabled);
        assert!(!existing.new_fields_enabled);

        let new = worktree_mode_presentation(true);
        assert!(new.existing_row_visible);
        assert!(new.new_row_visible);
        assert!(!new.existing_fields_enabled);
        assert!(new.new_fields_enabled);
    }

    #[test]
    fn origin_main_is_the_first_base_branch_choice() {
        assert_eq!(
            base_branch_choices(vec!["feature/test".to_owned(), "main".to_owned()]),
            ["origin/main", "feature/test", "main"]
        );
    }
}
