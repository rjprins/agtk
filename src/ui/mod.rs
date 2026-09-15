#![allow(deprecated)]

use std::cell::{Cell, RefCell};
use std::collections::{HashMap, HashSet};
use std::error::Error;
use std::fs;
use std::io::{self, Write};
use std::os::unix::fs::FileTypeExt;
use std::os::unix::net::UnixStream;
use std::path::{Path, PathBuf};
use std::process::{Command, Stdio};
use std::rc::Rc;
use std::thread;
use std::time::{Duration, SystemTime, UNIX_EPOCH};

use adw::prelude::*;
use gtk::pango::FontDescription;
use vte::prelude::*;

use crate::appearance::{AppearancePreferences, ThemeKey, theme};
use crate::azure::PrPreferences;
use crate::claude_presets::ClaudePresetPreferences;
use crate::control::{
    AppState, AppearanceSetParams, AppearanceSummary, AttentionSummary, Bounds, ControlCommand,
    ControlResponse, ControlServer, CreateSessionParams, ErrorCode, PROTOCOL_VERSION,
    PendingRequest, SessionKind, SessionState, SessionSummary, ShortcutSetParams, ShortcutSummary,
    TextSnapshot, UiInspection, UiNode, UiSurface, WindowState,
};
use crate::history::{InputTracker, history_needle};
use crate::instance::InstancePaths;
use crate::io_worker::IoWorker;
use crate::launch_preferences::QuickLaunchPreferences;
use crate::persist::{PersistResult, SessionRecord, Store};
use crate::projects::ProjectPreferences;
use crate::providers::ProviderSession;
use crate::session::{SessionLaunchPlan, receive_attachment};
use crate::shortcuts::{ShortcutAction, ShortcutPreferences};
use crate::terminal_text::{bounded_terminal_text, copyable_selection};

mod agents_ui;
mod appearance_ui;
mod azure_ui;
mod capture;
mod claude_ui;
mod controls;
mod emacs_ui;
mod history_ui;
mod inspection;
mod launch_ui;
mod projects_ui;
mod provider_icons;
mod search_ui;
mod sessions;
mod shortcuts_ui;
mod style;
mod worktrees_ui;

const EMPTY_PAGE: &str = "empty";
const MAX_INSPECTED_SESSIONS: usize = 500;
const PCRE2_LITERAL: u32 = 0x0200_0000;
const PCRE2_UTF: u32 = 0x0008_0000;

#[derive(Clone)]
struct Workspace {
    application: adw::Application,
    window: adw::ApplicationWindow,
    list: gtk::ListBox,
    stack: gtk::Stack,
    overlay: adw::ToastOverlay,
    new_shell_button: gtk::Button,
    git_button: gtk::MenuButton,
    claude_model_button: gtk::MenuButton,
    claude_model_popover: gtk::Popover,
    claude_model_list: gtk::Box,
    selected_claude_preset: Rc<Cell<i32>>,
    claude_presets_entry: gtk::Entry,
    pr_button: gtk::MenuButton,
    pr_popover: gtk::Popover,
    pr_root: gtk::Entry,
    pr_auto_toggle: gtk::CheckButton,
    pr_list: gtk::Box,
    updating_pr_toggle: Rc<Cell<bool>>,
    launch_button: gtk::MenuButton,
    launch_popover: gtk::Popover,
    launch_cancel: gtk::Button,
    launch_submit: gtk::Button,
    launch_agent_dropdown: gtk::DropDown,
    launch_agent_choices: gtk::StringList,
    launch_agent_buttons: Rc<Vec<(SessionKind, gtk::ToggleButton)>>,
    launch_agent_options: gtk::Stack,
    launch_claude_permission: gtk::DropDown,
    launch_claude_danger: gtk::CheckButton,
    launch_codex_approval: gtk::DropDown,
    launch_codex_sandbox: gtk::DropDown,
    launch_codex_full_auto: gtk::CheckButton,
    launch_codex_bypass: gtk::CheckButton,
    launch_gemini_approval: gtk::DropDown,
    launch_gemini_yolo: gtk::CheckButton,
    launch_cwd: gtk::Entry,
    launch_project: gtk::Entry,
    launch_project_dropdown: gtk::DropDown,
    launch_project_choices: gtk::StringList,
    launch_project_completion: gtk::ListStore,
    launch_project_completion_items: Rc<RefCell<Vec<(String, String)>>>,
    launch_worktree: gtk::Entry,
    launch_worktree_dropdown: gtk::DropDown,
    launch_worktree_choices: gtk::StringList,
    launch_worktree_completion: gtk::ListStore,
    launch_worktree_completion_items: Rc<RefCell<Vec<(String, String)>>>,
    launch_name: gtk::Entry,
    launch_args: gtk::Entry,
    launch_prompt: gtk::Entry,
    launch_branch: gtk::Entry,
    launch_base_branch: gtk::Entry,
    launch_base_branch_dropdown: gtk::DropDown,
    launch_base_branch_choices: gtk::StringList,
    launch_base_branch_completion: gtk::ListStore,
    launch_base_branch_completion_items: Rc<RefCell<Vec<(String, String)>>>,
    launch_worktree_values: Rc<RefCell<Vec<Option<String>>>>,
    launch_project_values: Rc<RefCell<Vec<Option<String>>>>,
    launch_project_custom: Rc<Cell<bool>>,
    launch_creating_worktree: Rc<Cell<bool>>,
    worktree_button: gtk::MenuButton,
    worktree_popover: gtk::Popover,
    worktree_root: gtk::Entry,
    worktree_branch: gtk::Entry,
    worktree_base: gtk::Entry,
    worktree_purpose: gtk::Entry,
    worktree_list: gtk::Box,
    agent_button: gtk::MenuButton,
    agent_popover: gtk::Popover,
    agent_list: gtk::Box,
    agent_preview: gtk::Box,
    agent_restore_cwd: gtk::Entry,
    agent_restore_project: gtk::Entry,
    agent_restore_worktree: gtk::Entry,
    agent_restore_button: gtk::Button,
    selected_agent: Rc<RefCell<Option<ProviderSession>>>,
    history_button: gtk::MenuButton,
    history_list: gtk::Box,
    history_popover: gtk::Popover,
    search_button: gtk::MenuButton,
    search_popover: gtk::Popover,
    search_entry: gtk::Entry,
    theme_button: gtk::MenuButton,
    theme_popover: gtk::Popover,
    follow_system_toggle: gtk::CheckButton,
    font_entry: gtk::Entry,
    shortcut_button: gtk::MenuButton,
    shortcut_popover: gtk::Popover,
    shortcut_entries: Rc<Vec<(ShortcutAction, gtk::Entry)>>,
    top_bar: adw::HeaderBar,
    sidebar_panel: gtk::Box,
    status_bar: gtk::Box,
    paths: InstancePaths,
    host_binary: PathBuf,
    sessions: Rc<RefCell<HashMap<String, SessionView>>>,
    closing_sessions: Rc<RefCell<HashSet<String>>>,
    selected_session: Rc<RefCell<Option<String>>>,
    sequence: Rc<Cell<u64>>,
    control_server: Rc<RefCell<Option<ControlServer>>>,
    io: IoWorker,
    store: Rc<RefCell<Option<Store>>>,
    appearance: Rc<RefCell<AppearancePreferences>>,
    chrome_style: style::ChromeStyle,
    style_manager: adw::StyleManager,
    shortcuts: Rc<RefCell<ShortcutPreferences>>,
    projects: Rc<RefCell<ProjectPreferences>>,
    quick_launch: Rc<RefCell<QuickLaunchPreferences>>,
    pr_preferences: Rc<RefCell<PrPreferences>>,
    claude_presets: Rc<RefCell<ClaudePresetPreferences>>,
    launch_choice_sequence: Rc<Cell<u64>>,
    updating_launch_choices: Rc<Cell<bool>>,
}

struct SessionView {
    record: SessionRecord,
    terminal: vte::Terminal,
    page: gtk::ScrolledWindow,
    row: gtk::ListBoxRow,
    label: gtk::Label,
    state_label: gtk::Label,
    history: Vec<String>,
    _pty: Option<vte::Pty>,
    control: Option<UnixStream>,
}

pub fn build(app: &adw::Application, paths: InstancePaths) {
    let display = gtk::gdk::Display::default().expect("GTK application has no display");
    let style_manager = adw::StyleManager::for_display(&display);
    let chrome_theme = if style_manager.is_dark() {
        theme(ThemeKey::Neutral)
    } else {
        theme(ThemeKey::NeutralLight)
    };
    let chrome_style = style::ChromeStyle::install(&display, chrome_theme.chrome);

    let list = gtk::ListBox::new();
    list.set_selection_mode(gtk::SelectionMode::Single);
    list.add_css_class("tui-session-list");

    let sidebar = gtk::ScrolledWindow::builder()
        .hscrollbar_policy(gtk::PolicyType::Never)
        .vexpand(true)
        .child(&list)
        .build();
    let sidebar_heading = gtk::Label::new(Some("SESSIONS"));
    sidebar_heading.set_xalign(0.0);
    sidebar_heading.set_hexpand(true);
    let sidebar_header = gtk::Box::new(gtk::Orientation::Horizontal, 2);
    sidebar_header.add_css_class("tui-sidebar-heading");
    sidebar_header.append(&sidebar_heading);
    let sidebar_actions = gtk::Box::new(gtk::Orientation::Horizontal, 2);
    sidebar_actions.add_css_class("tui-sidebar-actions");
    let sidebar_panel = gtk::Box::new(gtk::Orientation::Vertical, 0);
    sidebar_panel.add_css_class("tui-sidebar");
    sidebar_panel.set_size_request(252, -1);
    sidebar_panel.append(&sidebar_header);
    sidebar_panel.append(&sidebar_actions);
    sidebar_panel.append(&sidebar);
    let sidebar_controls = gtk::Box::new(gtk::Orientation::Horizontal, 2);
    sidebar_controls.add_css_class("tui-sidebar-controls");
    sidebar_panel.append(&sidebar_controls);

    let stack = gtk::Stack::builder()
        .hexpand(true)
        .vexpand(true)
        .transition_type(gtk::StackTransitionType::None)
        .build();
    stack.add_css_class("tui-main");
    let empty = gtk::Box::new(gtk::Orientation::Vertical, 4);
    empty.set_halign(gtk::Align::Center);
    empty.set_valign(gtk::Align::Center);
    empty.add_css_class("tui-empty");
    let empty_title = gtk::Label::new(Some("NO ACTIVE SESSION"));
    empty_title.add_css_class("tui-empty-title");
    let empty_hint = gtk::Label::new(Some("ctrl+shift+`  new shell"));
    empty.append(&empty_title);
    empty.append(&empty_hint);
    stack.add_named(&empty, Some(EMPTY_PAGE));

    let split = gtk::Paned::new(gtk::Orientation::Horizontal);
    split.set_start_child(Some(&sidebar_panel));
    split.set_end_child(Some(&stack));
    split.set_resize_start_child(false);
    split.set_shrink_start_child(false);
    split.set_position(260);

    let header = adw::HeaderBar::new();
    header.add_css_class("tui-topbar");
    header.set_show_title(false);
    let brand = gtk::Label::new(Some("agmux-native"));
    brand.add_css_class("tui-brand");
    header.pack_start(&brand);
    let new_shell = gtk::Button::builder()
        .icon_name("utilities-terminal-symbolic")
        .build();
    new_shell.add_css_class("tui-button");
    new_shell.set_tooltip_text(Some("Start a shell session"));
    sidebar_actions.append(&new_shell);
    let git_surface = gtk::Box::new(gtk::Orientation::Horizontal, 2);
    git_surface.add_css_class("tui-surface");
    let magit_button = gtk::Button::builder()
        .icon_name("applications-development-symbolic")
        .build();
    magit_button.add_css_class("tui-button");
    magit_button.set_tooltip_text(Some("Open the selected session worktree in Emacs Magit"));
    git_surface.append(&magit_button);
    let branch_review_button = gtk::Button::builder()
        .icon_name("document-edit-symbolic")
        .build();
    branch_review_button.add_css_class("tui-button");
    branch_review_button.set_tooltip_text(Some(
        "Open the selected session worktree in Emacs branch-review",
    ));
    git_surface.append(&branch_review_button);
    let git_popover = gtk::Popover::builder().child(&git_surface).build();
    git_popover.add_css_class("tui-popover");
    let git_button = gtk::MenuButton::builder()
        .icon_name("applications-development-symbolic")
        .popover(&git_popover)
        .sensitive(false)
        .build();
    git_button.add_css_class("tui-button");
    git_button.set_tooltip_text(Some("Open the selected worktree in Emacs"));
    sidebar_actions.append(&git_button);

    let claude_model_list = gtk::Box::new(gtk::Orientation::Vertical, 1);
    claude_model_list.add_css_class("tui-surface");
    let claude_model_popover = gtk::Popover::builder().child(&claude_model_list).build();
    claude_model_popover.add_css_class("tui-popover");
    let claude_model_button = gtk::MenuButton::builder()
        .icon_name("preferences-system-symbolic")
        .popover(&claude_model_popover)
        .sensitive(false)
        .build();
    claude_model_button.add_css_class("tui-button");
    claude_model_button.set_visible(false);
    claude_model_button.set_tooltip_text(Some("Choose a Claude model and effort preset"));
    header.pack_end(&claude_model_button);

    let pr_surface = gtk::Box::new(gtk::Orientation::Vertical, 4);
    pr_surface.add_css_class("tui-surface");
    pr_surface.set_margin_top(6);
    pr_surface.set_margin_bottom(6);
    pr_surface.set_margin_start(6);
    pr_surface.set_margin_end(6);
    let pr_heading_row = gtk::Box::new(gtk::Orientation::Horizontal, 4);
    let pr_heading = gtk::Label::new(Some("AZURE PULL REQUESTS"));
    pr_heading.set_xalign(0.0);
    pr_heading.set_hexpand(true);
    pr_heading.add_css_class("tui-sidebar-heading");
    pr_heading_row.append(&pr_heading);
    let pr_refresh = gtk::Button::builder()
        .icon_name("view-refresh-symbolic")
        .build();
    pr_refresh.add_css_class("tui-button");
    pr_heading_row.append(&pr_refresh);
    pr_surface.append(&pr_heading_row);
    let pr_root = launch_entry("Project root", "absolute Azure DevOps repository root");
    pr_surface.append(&pr_root.0);
    let pr_auto_toggle = gtk::CheckButton::with_label("Auto-review new attention");
    pr_auto_toggle.add_css_class("tui-setting-row");
    pr_auto_toggle.set_tooltip_text(Some(
        "Opt in to launching Codex review sessions for later PR attention changes",
    ));
    pr_surface.append(&pr_auto_toggle);
    let pr_list = gtk::Box::new(gtk::Orientation::Vertical, 2);
    let pr_scroller = gtk::ScrolledWindow::builder()
        .hscrollbar_policy(gtk::PolicyType::Never)
        .min_content_width(720)
        .min_content_height(260)
        .max_content_height(560)
        .child(&pr_list)
        .build();
    pr_surface.append(&pr_scroller);
    let pr_popover = gtk::Popover::builder().child(&pr_surface).build();
    pr_popover.add_css_class("tui-popover");
    pr_popover.set_position(gtk::PositionType::Left);
    let pr_button = gtk::MenuButton::builder()
        .icon_name("git-merge-symbolic")
        .popover(&pr_popover)
        .sensitive(false)
        .build();
    pr_button.add_css_class("tui-button");
    pr_button.set_tooltip_text(Some(
        "Active Azure DevOps pull requests and review attention",
    ));
    header.pack_end(&pr_button);

    let launch_form = gtk::Box::new(gtk::Orientation::Vertical, 4);
    launch_form.add_css_class("tui-surface");
    launch_form.set_margin_top(6);
    launch_form.set_margin_bottom(6);
    launch_form.set_margin_start(6);
    launch_form.set_margin_end(6);
    let launch_heading = gtk::Label::new(Some("Launch agent"));
    launch_heading.set_xalign(0.0);
    launch_heading.add_css_class("tui-sidebar-heading");
    launch_form.append(&launch_heading);
    let launch_agent_choices = gtk::StringList::new(&["claude", "codex", "gemini", "shell"]);
    let launch_agent_dropdown =
        gtk::DropDown::new(Some(launch_agent_choices.clone()), None::<&gtk::Expression>);
    launch_agent_dropdown.set_enable_search(true);
    launch_agent_dropdown.set_selected(0);
    launch_agent_dropdown.set_visible(false);
    let agent_row = gtk::Box::new(gtk::Orientation::Horizontal, 5);
    agent_row.add_css_class("tui-setting-row");
    let agent_label = gtk::Label::new(Some("Agent"));
    agent_label.set_xalign(0.0);
    agent_label.set_width_chars(18);
    agent_row.append(&agent_label);
    let launch_agent_button_box = gtk::Box::new(gtk::Orientation::Horizontal, 0);
    launch_agent_button_box.add_css_class("tui-agent-buttons");
    let mut launch_agent_buttons = Vec::new();
    let mut previous_agent_button: Option<gtk::ToggleButton> = None;
    for (kind, label) in [
        (SessionKind::Claude, "claude"),
        (SessionKind::Codex, "codex"),
        (SessionKind::Gemini, "gemini"),
        (SessionKind::Shell, "shell"),
    ] {
        let button = gtk::ToggleButton::with_label(label);
        button.add_css_class("tui-agent-button");
        if let Some(previous) = previous_agent_button.as_ref() {
            button.set_group(Some(previous));
        }
        if kind == SessionKind::Shell {
            button.set_active(true);
        }
        launch_agent_button_box.append(&button);
        previous_agent_button = Some(button.clone());
        launch_agent_buttons.push((kind, button));
    }
    agent_row.append(&launch_agent_button_box);
    agent_row.append(&launch_agent_dropdown);
    launch_form.append(&agent_row);

    let launch_agent_options = gtk::Stack::builder()
        .transition_type(gtk::StackTransitionType::None)
        .build();
    let (launch_claude_permission, launch_claude_danger) = claude_launch_options();
    let (launch_codex_approval, launch_codex_sandbox, launch_codex_full_auto, launch_codex_bypass) =
        codex_launch_options();
    let (launch_gemini_approval, launch_gemini_yolo) = gemini_launch_options();
    launch_claude_danger.set_active(true);
    launch_codex_full_auto.set_active(true);
    launch_agent_options.add_named(
        &launch_option_page(
            "Permission mode",
            &launch_claude_permission,
            "Skip permission prompts",
            &launch_claude_danger,
        ),
        Some("claude"),
    );
    launch_agent_options.add_named(
        &launch_option_page_with_two_dropdowns(
            "Ask for approval",
            &launch_codex_approval,
            "Sandbox",
            &launch_codex_sandbox,
            &[
                ("Full auto", &launch_codex_full_auto),
                ("Bypass approvals and sandbox", &launch_codex_bypass),
            ],
        ),
        Some("codex"),
    );
    launch_agent_options.add_named(
        &launch_option_page(
            "Approval mode",
            &launch_gemini_approval,
            "YOLO",
            &launch_gemini_yolo,
        ),
        Some("gemini"),
    );
    launch_agent_options.add_named(&gtk::Box::new(gtk::Orientation::Vertical, 0), Some("shell"));
    launch_form.append(&launch_agent_options);
    let launch_cwd = launch_entry("Working directory", "cwd");
    let launch_project = launch_entry("Project directory", "Search projects or type a path…");
    let launch_project_choices = gtk::StringList::new(&[]);
    let launch_project_dropdown = searchable_path_dropdown(&launch_project_choices);
    launch_project_dropdown.set_visible(false);
    launch_project.0.append(&launch_project_dropdown);
    let launch_project_completion =
        gtk::ListStore::new(&[String::static_type(), String::static_type()]);
    let launch_project_completion_items = Rc::new(RefCell::new(Vec::new()));
    install_choice_completion(
        &launch_project.1,
        &launch_project_completion,
        launch_project_completion_items.clone(),
        true,
    );
    let launch_worktree = launch_entry("Worktree", "Search worktrees…");
    let launch_worktree_choices = gtk::StringList::new(&[]);
    let launch_worktree_dropdown = searchable_path_dropdown(&launch_worktree_choices);
    launch_worktree_dropdown.set_visible(false);
    launch_worktree.0.append(&launch_worktree_dropdown);
    let launch_worktree_completion =
        gtk::ListStore::new(&[String::static_type(), String::static_type()]);
    let launch_worktree_completion_items = Rc::new(RefCell::new(Vec::new()));
    install_choice_completion(
        &launch_worktree.1,
        &launch_worktree_completion,
        launch_worktree_completion_items.clone(),
        true,
    );
    let launch_name = launch_entry("Name", "session name (optional)");
    let launch_args = launch_entry(
        "Arguments",
        "JSON array, for example [\"--model\",\"opus\"]",
    );
    let launch_prompt = launch_entry("Initial input", "initial prompt (optional)");
    let launch_branch = launch_entry_with_hint(
        "Branch name (optional)",
        "generated branch",
        "Worktree name will be based on the branch name.",
    );
    let launch_base_branch = launch_entry("Base branch", "main");
    let launch_base_branch_choices = gtk::StringList::new(&[]);
    let launch_base_branch_dropdown = searchable_path_dropdown(&launch_base_branch_choices);
    launch_base_branch_dropdown.set_visible(false);
    launch_base_branch.0.append(&launch_base_branch_dropdown);
    let launch_base_branch_completion =
        gtk::ListStore::new(&[String::static_type(), String::static_type()]);
    let launch_base_branch_completion_items = Rc::new(RefCell::new(Vec::new()));
    install_choice_completion(
        &launch_base_branch.1,
        &launch_base_branch_completion,
        launch_base_branch_completion_items.clone(),
        false,
    );
    launch_branch.0.set_visible(false);
    launch_base_branch.0.set_visible(false);
    for row in [
        &launch_project.0,
        &launch_worktree.0,
        &launch_branch.0,
        &launch_base_branch.0,
    ] {
        launch_form.append(row);
    }
    let launch_actions = gtk::Box::new(gtk::Orientation::Horizontal, 4);
    let launch_cancel = gtk::Button::with_label("Cancel");
    launch_cancel.add_css_class("tui-button");
    launch_actions.append(&launch_cancel);
    let launch_submit = gtk::Button::with_label("Launch");
    launch_submit.add_css_class("suggested-action");
    launch_actions.append(&launch_submit);
    launch_form.append(&launch_actions);
    let launch_popover = gtk::Popover::builder().child(&launch_form).build();
    launch_popover.add_css_class("tui-popover");
    let launch_button = gtk::MenuButton::builder()
        .icon_name("list-add-symbolic")
        .popover(&launch_popover)
        .sensitive(false)
        .build();
    launch_button.add_css_class("tui-button");
    launch_button.set_tooltip_text(Some("Launch a shell, Codex, Claude, or Gemini session"));
    header.pack_end(&launch_button);

    let worktree_surface = gtk::Box::new(gtk::Orientation::Vertical, 4);
    worktree_surface.add_css_class("tui-surface");
    worktree_surface.set_margin_top(6);
    worktree_surface.set_margin_bottom(6);
    worktree_surface.set_margin_start(6);
    worktree_surface.set_margin_end(6);
    let worktree_heading = gtk::Label::new(Some("WORKTREES"));
    worktree_heading.set_xalign(0.0);
    worktree_heading.add_css_class("tui-sidebar-heading");
    worktree_surface.append(&worktree_heading);
    let worktree_root = launch_entry("Project root", "absolute repository root");
    let worktree_refresh = gtk::Button::builder()
        .icon_name("view-refresh-symbolic")
        .build();
    worktree_refresh.add_css_class("tui-button");
    worktree_root.0.append(&worktree_refresh);
    worktree_surface.append(&worktree_root.0);
    let worktree_branch = launch_entry("New branch", "concise-kebab-case");
    let worktree_base = launch_entry("Base", "default branch when empty");
    let worktree_purpose = launch_entry("Purpose", "why this worktree exists");
    worktree_surface.append(&worktree_branch.0);
    worktree_surface.append(&worktree_base.0);
    worktree_surface.append(&worktree_purpose.0);
    let worktree_create = gtk::Button::builder()
        .icon_name("list-add-symbolic")
        .build();
    worktree_create.add_css_class("tui-button");
    worktree_surface.append(&worktree_create);
    let worktree_list = gtk::Box::new(gtk::Orientation::Vertical, 2);
    let worktree_scroller = gtk::ScrolledWindow::builder()
        .hscrollbar_policy(gtk::PolicyType::Never)
        .min_content_width(680)
        .min_content_height(180)
        .max_content_height(440)
        .child(&worktree_list)
        .build();
    worktree_surface.append(&worktree_scroller);
    let worktree_popover = gtk::Popover::builder().child(&worktree_surface).build();
    worktree_popover.add_css_class("tui-popover");
    let worktree_button = gtk::MenuButton::builder()
        .icon_name("folder-open-symbolic")
        .popover(&worktree_popover)
        .sensitive(false)
        .build();
    worktree_button.add_css_class("tui-button");
    worktree_button.set_tooltip_text(Some("Inspect, create, and safely reap worktrees"));
    header.pack_end(&worktree_button);

    let agent_surface = gtk::Box::new(gtk::Orientation::Vertical, 4);
    agent_surface.add_css_class("tui-surface");
    agent_surface.set_margin_top(6);
    agent_surface.set_margin_bottom(6);
    agent_surface.set_margin_start(6);
    agent_surface.set_margin_end(6);
    let agent_heading_row = gtk::Box::new(gtk::Orientation::Horizontal, 4);
    let agent_heading = gtk::Label::new(Some("RECENT AGENT SESSIONS"));
    agent_heading.set_xalign(0.0);
    agent_heading.set_hexpand(true);
    agent_heading.add_css_class("tui-sidebar-heading");
    agent_heading_row.append(&agent_heading);
    let agent_refresh = gtk::Button::builder()
        .icon_name("view-refresh-symbolic")
        .build();
    agent_refresh.add_css_class("tui-button");
    agent_heading_row.append(&agent_refresh);
    agent_surface.append(&agent_heading_row);
    let agent_list = gtk::Box::new(gtk::Orientation::Vertical, 2);
    let agent_list_scroller = gtk::ScrolledWindow::builder()
        .hscrollbar_policy(gtk::PolicyType::Never)
        .min_content_width(310)
        .min_content_height(360)
        .child(&agent_list)
        .build();
    let agent_detail = gtk::Box::new(gtk::Orientation::Vertical, 3);
    let agent_preview = gtk::Box::new(gtk::Orientation::Vertical, 4);
    let agent_preview_scroller = gtk::ScrolledWindow::builder()
        .hscrollbar_policy(gtk::PolicyType::Never)
        .min_content_width(520)
        .min_content_height(230)
        .vexpand(true)
        .child(&agent_preview)
        .build();
    agent_detail.append(&agent_preview_scroller);
    let agent_restore_cwd = launch_entry("Working directory", "original cwd");
    let agent_restore_project = launch_entry("Project root", "optional project root");
    let agent_restore_worktree = launch_entry("Worktree", "optional worktree path");
    agent_detail.append(&agent_restore_cwd.0);
    agent_detail.append(&agent_restore_project.0);
    agent_detail.append(&agent_restore_worktree.0);
    let agent_restore_button = gtk::Button::builder()
        .icon_name("document-revert-symbolic")
        .build();
    agent_restore_button.add_css_class("tui-button");
    agent_restore_button.set_sensitive(false);
    agent_detail.append(&agent_restore_button);
    let agent_split = gtk::Paned::new(gtk::Orientation::Horizontal);
    agent_split.set_start_child(Some(&agent_list_scroller));
    agent_split.set_end_child(Some(&agent_detail));
    agent_split.set_resize_start_child(false);
    agent_split.set_shrink_start_child(false);
    agent_split.set_position(320);
    agent_surface.append(&agent_split);
    let agent_popover = gtk::Popover::builder().child(&agent_surface).build();
    agent_popover.add_css_class("tui-popover");
    let agent_button = gtk::MenuButton::builder()
        .icon_name("document-open-recent-symbolic")
        .popover(&agent_popover)
        .sensitive(false)
        .build();
    agent_button.add_css_class("tui-button");
    agent_button.set_tooltip_text(Some("Preview and restore recent Codex and Claude sessions"));
    header.pack_end(&agent_button);

    let history_list = gtk::Box::new(gtk::Orientation::Vertical, 2);
    history_list.add_css_class("tui-surface");
    history_list.set_margin_top(6);
    history_list.set_margin_bottom(6);
    history_list.set_margin_start(6);
    history_list.set_margin_end(6);
    let history_scroller = gtk::ScrolledWindow::builder()
        .hscrollbar_policy(gtk::PolicyType::Never)
        .min_content_width(360)
        .max_content_height(420)
        .child(&history_list)
        .build();
    let history_popover = gtk::Popover::builder().child(&history_scroller).build();
    let history_button = gtk::MenuButton::builder()
        .icon_name("view-list-symbolic")
        .popover(&history_popover)
        .sensitive(false)
        .build();
    history_popover.add_css_class("tui-popover");
    history_button.add_css_class("tui-button");
    history_button.set_tooltip_text(Some("Scroll to a submitted prompt"));
    header.pack_end(&history_button);

    let search_surface = gtk::Box::new(gtk::Orientation::Horizontal, 4);
    search_surface.add_css_class("tui-surface");
    search_surface.set_margin_top(5);
    search_surface.set_margin_bottom(5);
    search_surface.set_margin_start(5);
    search_surface.set_margin_end(5);
    let search_entry = gtk::Entry::builder()
        .placeholder_text("Find literal text")
        .width_chars(36)
        .build();
    search_entry.add_css_class("tui-setting-entry");
    search_surface.append(&search_entry);
    let search_previous = gtk::Button::builder().icon_name("go-up-symbolic").build();
    search_previous.add_css_class("tui-button");
    search_surface.append(&search_previous);
    let search_next = gtk::Button::builder().icon_name("go-down-symbolic").build();
    search_next.add_css_class("tui-button");
    search_surface.append(&search_next);
    let search_popover = gtk::Popover::builder().child(&search_surface).build();
    search_popover.add_css_class("tui-popover");
    let search_button = gtk::MenuButton::builder()
        .icon_name("edit-find-symbolic")
        .popover(&search_popover)
        .sensitive(false)
        .build();
    search_button.add_css_class("tui-button");
    search_button.set_tooltip_text(Some("Search selected terminal scrollback"));
    header.pack_end(&search_button);

    let theme_list = gtk::Box::new(gtk::Orientation::Vertical, 1);
    theme_list.add_css_class("tui-surface");
    theme_list.set_margin_top(5);
    theme_list.set_margin_bottom(5);
    theme_list.set_margin_start(5);
    theme_list.set_margin_end(5);
    let theme_heading = gtk::Label::new(Some("TERMINAL APPEARANCE"));
    theme_heading.set_xalign(0.0);
    theme_heading.add_css_class("tui-sidebar-heading");
    theme_list.append(&theme_heading);
    let mut theme_buttons = Vec::new();
    for key in ThemeKey::ALL {
        let choice = gtk::Button::with_label(theme(key).name);
        choice.add_css_class("tui-button");
        choice.set_tooltip_text(Some(&format!(
            "Use the {} terminal palette",
            theme(key).name
        )));
        theme_list.append(&choice);
        theme_buttons.push((key, choice));
    }
    let follow_system_toggle = gtk::CheckButton::with_label("Follow system light/dark");
    follow_system_toggle.add_css_class("tui-setting-row");
    follow_system_toggle.set_tooltip_text(Some(
        "Resolve the selected terminal palette to its light or dark partner",
    ));
    theme_list.append(&follow_system_toggle);
    let font_entry = gtk::Entry::builder()
        .text("Monospace 11")
        .placeholder_text("Terminal font")
        .build();
    font_entry.add_css_class("tui-setting-entry");
    font_entry.set_tooltip_text(Some("Pango terminal font description"));
    theme_list.append(&font_entry);
    let apply_font = gtk::Button::with_label("Apply font");
    apply_font.add_css_class("tui-button");
    theme_list.append(&apply_font);
    let theme_popover = gtk::Popover::builder().child(&theme_list).build();
    theme_popover.add_css_class("tui-popover");
    let theme_button = gtk::MenuButton::builder()
        .icon_name("preferences-desktop-theme-symbolic")
        .popover(&theme_popover)
        .sensitive(false)
        .build();
    theme_button.add_css_class("tui-button");
    theme_button.set_tooltip_text(Some("Choose terminal colors and font"));
    header.pack_end(&theme_button);

    let shortcut_list = gtk::Box::new(gtk::Orientation::Vertical, 2);
    shortcut_list.add_css_class("tui-surface");
    shortcut_list.set_margin_top(5);
    shortcut_list.set_margin_bottom(5);
    shortcut_list.set_margin_start(5);
    shortcut_list.set_margin_end(5);
    let shortcut_heading = gtk::Label::new(Some("APPLICATION SHORTCUTS"));
    shortcut_heading.set_xalign(0.0);
    shortcut_heading.add_css_class("tui-sidebar-heading");
    shortcut_list.append(&shortcut_heading);
    let mut shortcut_entries = Vec::new();
    let mut shortcut_controls = Vec::new();
    for action in ShortcutAction::ALL {
        let row = gtk::Box::new(gtk::Orientation::Horizontal, 4);
        row.add_css_class("tui-setting-row");
        let label = gtk::Label::new(Some(action.label()));
        label.set_xalign(0.0);
        label.set_width_chars(24);
        row.append(&label);
        let entry = gtk::Entry::builder()
            .text(action.default_accelerator())
            .width_chars(25)
            .build();
        entry.add_css_class("tui-setting-entry");
        row.append(&entry);
        let set = gtk::Button::with_label("Set");
        set.add_css_class("tui-button");
        set.set_tooltip_text(Some(&format!("Set shortcut for {}", action.label())));
        row.append(&set);
        let reset = gtk::Button::with_label("Reset");
        reset.add_css_class("tui-button");
        reset.set_tooltip_text(Some(&format!("Reset shortcut for {}", action.label())));
        row.append(&reset);
        shortcut_list.append(&row);
        shortcut_entries.push((action, entry.clone()));
        shortcut_controls.push((action, entry, set, reset));
    }
    let claude_presets_row = gtk::Box::new(gtk::Orientation::Horizontal, 4);
    claude_presets_row.add_css_class("tui-setting-row");
    let claude_presets_label = gtk::Label::new(Some("Claude presets JSON"));
    claude_presets_label.set_xalign(0.0);
    claude_presets_label.set_width_chars(24);
    claude_presets_row.append(&claude_presets_label);
    let claude_presets_entry = gtk::Entry::builder()
        .placeholder_text("JSON array of named model and effort presets")
        .width_chars(48)
        .hexpand(true)
        .build();
    claude_presets_entry.add_css_class("tui-setting-entry");
    claude_presets_row.append(&claude_presets_entry);
    let apply_claude_presets = gtk::Button::with_label("Apply");
    apply_claude_presets.add_css_class("tui-button");
    claude_presets_row.append(&apply_claude_presets);
    shortcut_list.append(&claude_presets_row);
    let shortcut_popover = gtk::Popover::builder().child(&shortcut_list).build();
    shortcut_popover.add_css_class("tui-popover");
    let shortcut_button = gtk::MenuButton::builder()
        .icon_name("preferences-desktop-keyboard-shortcuts-symbolic")
        .popover(&shortcut_popover)
        .sensitive(false)
        .build();
    shortcut_button.add_css_class("tui-button");
    shortcut_button.set_tooltip_text(Some("Configure application shortcuts"));
    header.pack_end(&shortcut_button);

    sidebar_actions.remove(&new_shell);
    sidebar_controls.append(&new_shell);
    let sidebar_keys = gtk::Button::builder()
        .icon_name("preferences-desktop-keyboard-shortcuts-symbolic")
        .build();
    sidebar_keys.add_css_class("tui-button");
    sidebar_keys.set_tooltip_text(Some("Configure application shortcuts"));
    sidebar_controls.append(&sidebar_keys);
    let sidebar_settings = gtk::Button::builder()
        .icon_name("preferences-desktop-theme-symbolic")
        .build();
    sidebar_settings.add_css_class("tui-button");
    sidebar_settings.set_tooltip_text(Some("Choose terminal appearance"));
    sidebar_controls.append(&sidebar_settings);
    let sidebar_toggle = gtk::Button::builder()
        .icon_name("sidebar-show-symbolic")
        .build();
    sidebar_toggle.add_css_class("tui-button");
    sidebar_toggle.set_tooltip_text(Some("Collapse sidebar"));
    sidebar_controls.append(&sidebar_toggle);

    let sidebar_keys_shortcut = shortcut_popover.clone();
    sidebar_keys.connect_clicked(move |_| sidebar_keys_shortcut.popup());
    let sidebar_settings_theme = theme_popover.clone();
    sidebar_settings.connect_clicked(move |_| sidebar_settings_theme.popup());
    let sidebar_collapsed = Rc::new(Cell::new(false));
    let sidebar_collapsed_state = sidebar_collapsed.clone();
    let sidebar_split = split.clone();
    sidebar_toggle.connect_clicked(move |button| {
        let collapsed = !sidebar_collapsed_state.get();
        sidebar_collapsed_state.set(collapsed);
        sidebar_split.set_position(if collapsed { 0 } else { 260 });
        button.set_icon_name(if collapsed {
            "sidebar-show-symbolic"
        } else {
            "sidebar-hide-symbolic"
        });
        button.set_tooltip_text(Some(if collapsed {
            "Expand sidebar"
        } else {
            "Collapse sidebar"
        }));
    });

    let toolbar = adw::ToolbarView::new();
    toolbar.add_top_bar(&header);
    toolbar.set_content(Some(&split));
    let status_bar = gtk::Box::new(gtk::Orientation::Horizontal, 10);
    status_bar.add_css_class("tui-statusbar");
    let shortcuts = gtk::Label::new(Some(
        "ctrl+shift+` new   ctrl+shift+q close   ctrl+shift+\\ sidebar   ctrl+shift+[ ] sessions",
    ));
    shortcuts.set_xalign(0.0);
    shortcuts.set_hexpand(true);
    let instance = gtk::Label::new(Some(paths.name().as_str()));
    instance.add_css_class("tui-status-accent");
    status_bar.append(&shortcuts);
    status_bar.append(&instance);
    toolbar.add_bottom_bar(&status_bar);

    let overlay = adw::ToastOverlay::new();
    overlay.add_css_class("tui-root");
    overlay.set_child(Some(&toolbar));
    let window = adw::ApplicationWindow::builder()
        .application(app)
        .title("agmux native")
        .default_width(1200)
        .default_height(800)
        .content(&overlay)
        .build();

    let workspace = Workspace {
        application: app.clone(),
        window: window.clone(),
        list: list.clone(),
        stack: stack.clone(),
        overlay,
        new_shell_button: new_shell.clone(),
        git_button: git_button.clone(),
        claude_model_button,
        claude_model_popover,
        claude_model_list,
        selected_claude_preset: Rc::new(Cell::new(0)),
        claude_presets_entry: claude_presets_entry.clone(),
        pr_button,
        pr_popover,
        pr_root: pr_root.1,
        pr_auto_toggle: pr_auto_toggle.clone(),
        pr_list,
        updating_pr_toggle: Rc::new(Cell::new(false)),
        launch_button,
        launch_popover,
        launch_cancel: launch_cancel.clone(),
        launch_submit: launch_submit.clone(),
        launch_agent_dropdown,
        launch_agent_choices,
        launch_agent_buttons: Rc::new(launch_agent_buttons),
        launch_agent_options,
        launch_claude_permission,
        launch_claude_danger,
        launch_codex_approval,
        launch_codex_sandbox,
        launch_codex_full_auto,
        launch_codex_bypass,
        launch_gemini_approval,
        launch_gemini_yolo,
        launch_cwd: launch_cwd.1,
        launch_project: launch_project.1,
        launch_project_dropdown,
        launch_project_choices,
        launch_project_completion,
        launch_project_completion_items,
        launch_worktree: launch_worktree.1,
        launch_worktree_dropdown,
        launch_worktree_choices,
        launch_worktree_completion,
        launch_worktree_completion_items,
        launch_name: launch_name.1,
        launch_args: launch_args.1,
        launch_prompt: launch_prompt.1,
        launch_branch: launch_branch.1,
        launch_base_branch: launch_base_branch.1,
        launch_base_branch_dropdown,
        launch_base_branch_choices,
        launch_base_branch_completion,
        launch_base_branch_completion_items,
        launch_worktree_values: Rc::new(RefCell::new(Vec::new())),
        launch_project_values: Rc::new(RefCell::new(Vec::new())),
        launch_project_custom: Rc::new(Cell::new(false)),
        launch_creating_worktree: Rc::new(Cell::new(false)),
        worktree_button,
        worktree_popover,
        worktree_root: worktree_root.1,
        worktree_branch: worktree_branch.1,
        worktree_base: worktree_base.1,
        worktree_purpose: worktree_purpose.1,
        worktree_list,
        agent_button,
        agent_popover,
        agent_list,
        agent_preview,
        agent_restore_cwd: agent_restore_cwd.1,
        agent_restore_project: agent_restore_project.1,
        agent_restore_worktree: agent_restore_worktree.1,
        agent_restore_button: agent_restore_button.clone(),
        selected_agent: Rc::new(RefCell::new(None)),
        history_button,
        history_list,
        history_popover,
        search_button,
        search_popover,
        search_entry: search_entry.clone(),
        theme_button,
        theme_popover,
        follow_system_toggle: follow_system_toggle.clone(),
        font_entry: font_entry.clone(),
        shortcut_button,
        shortcut_popover,
        shortcut_entries: Rc::new(shortcut_entries),
        top_bar: header,
        sidebar_panel: sidebar_panel.clone(),
        status_bar: status_bar.clone(),
        paths,
        host_binary: sibling_binary("agmux-session"),
        sessions: Rc::new(RefCell::new(HashMap::new())),
        closing_sessions: Rc::new(RefCell::new(HashSet::new())),
        selected_session: Rc::new(RefCell::new(None)),
        sequence: Rc::new(Cell::new(0)),
        control_server: Rc::new(RefCell::new(None)),
        io: IoWorker::default(),
        store: Rc::new(RefCell::new(None)),
        appearance: Rc::new(RefCell::new(AppearancePreferences::default())),
        chrome_style,
        style_manager: style_manager.clone(),
        shortcuts: Rc::new(RefCell::new(ShortcutPreferences::default())),
        projects: Rc::new(RefCell::new(ProjectPreferences::default())),
        quick_launch: Rc::new(RefCell::new(QuickLaunchPreferences::default())),
        pr_preferences: Rc::new(RefCell::new(PrPreferences::default())),
        claude_presets: Rc::new(RefCell::new(ClaudePresetPreferences::default())),
        launch_choice_sequence: Rc::new(Cell::new(0)),
        updating_launch_choices: Rc::new(Cell::new(false)),
    };

    let selected_workspace = workspace.clone();
    list.connect_row_selected(move |_, row| {
        let Some(row) = row else { return };
        let id = row.widget_name();
        selected_workspace
            .selected_session
            .borrow_mut()
            .replace(id.to_string());
        selected_workspace.stack.set_visible_child_name(&id);
        if let Some(session) = selected_workspace.sessions.borrow().get(id.as_str()) {
            session.terminal.grab_focus();
        }
        selected_workspace.render_history(Some(id.as_str()));
        selected_workspace.search_button.set_sensitive(true);
        selected_workspace.save_preference("selectedSessionId", serde_json::json!(id.as_str()));
        selected_workspace.update_emacs_actions();
        selected_workspace.update_claude_actions();
    });

    workspace.connect_launch_path_controls();
    let launch_agent_workspace = workspace.clone();
    workspace
        .launch_agent_dropdown
        .connect_selected_notify(move |dropdown| {
            let kind = match dropdown
                .selected_item()
                .and_then(|item| item.downcast::<gtk::StringObject>().ok())
                .map(|item| item.string().to_string())
                .as_deref()
            {
                Some("codex") => SessionKind::Codex,
                Some("claude") => SessionKind::Claude,
                Some("gemini") => SessionKind::Gemini,
                _ => SessionKind::Shell,
            };
            launch_agent_workspace
                .launch_agent_options
                .set_visible_child_name(session_kind_name(kind));
        });
    let launch_submit_workspace = workspace.clone();
    launch_submit.connect_clicked(move |_| {
        launch_submit_workspace.launch_from_form(launch_submit_workspace.selected_launch_agent());
    });
    let launch_cancel_workspace = workspace.clone();
    launch_cancel.connect_clicked(move |_| launch_cancel_workspace.launch_popover.popdown());
    for (kind, button) in workspace.launch_agent_buttons.iter() {
        let launch_agent_workspace = workspace.clone();
        let kind = *kind;
        button.connect_clicked(move |_| launch_agent_workspace.set_launch_agent(kind));
    }
    let launch_workspace = workspace.clone();
    workspace
        .launch_popover
        .connect_show(move |_| launch_workspace.prepare_launch_panel());

    let launch_workspace = workspace.clone();
    new_shell.connect_clicked(move |_| launch_workspace.launch_shell());

    let magit_workspace = workspace.clone();
    magit_button.connect_clicked(move |_| {
        if let Some(id) = magit_workspace.selected_session_id() {
            magit_workspace.open_session_in_emacs(&id, crate::emacs::EmacsAction::Magit, None);
        }
    });
    let review_workspace = workspace.clone();
    branch_review_button.connect_clicked(move |_| {
        if let Some(id) = review_workspace.selected_session_id() {
            review_workspace.open_session_in_emacs(
                &id,
                crate::emacs::EmacsAction::BranchReview,
                None,
            );
        }
    });
    let preset_workspace = workspace.clone();
    apply_claude_presets
        .connect_clicked(move |_| preset_workspace.apply_claude_presets_from_entry());
    let preset_workspace = workspace.clone();
    claude_presets_entry
        .connect_activate(move |_| preset_workspace.apply_claude_presets_from_entry());

    let worktree_workspace = workspace.clone();
    worktree_refresh.connect_clicked(move |_| worktree_workspace.refresh_worktree_panel());
    let worktree_workspace = workspace.clone();
    worktree_create.connect_clicked(move |_| worktree_workspace.create_worktree_from_panel());
    let worktree_workspace = workspace.clone();
    workspace.worktree_popover.connect_show(move |_| {
        worktree_workspace.prepare_worktree_panel();
    });
    let pr_workspace = workspace.clone();
    pr_refresh.connect_clicked(move |_| pr_workspace.refresh_pr_panel(None));
    let pr_workspace = workspace.clone();
    workspace
        .pr_popover
        .connect_show(move |_| pr_workspace.prepare_pr_panel());
    let pr_workspace = workspace.clone();
    pr_auto_toggle.connect_toggled(move |toggle| {
        if pr_workspace.updating_pr_toggle.get() {
            return;
        }
        let root = pr_workspace.pr_root.text().trim().to_owned();
        if root.is_empty() {
            pr_workspace.show_error("Select a project before changing auto-review");
            return;
        }
        pr_workspace.set_auto_review(
            crate::control::PrSetAutoReviewParams {
                project_root: PathBuf::from(root),
                enabled: toggle.is_active(),
            },
            None,
        );
    });
    let agent_workspace = workspace.clone();
    agent_refresh.connect_clicked(move |_| agent_workspace.refresh_agent_panel());
    let agent_workspace = workspace.clone();
    agent_restore_button.connect_clicked(move |_| agent_workspace.restore_agent_from_panel());
    let agent_workspace = workspace.clone();
    workspace
        .agent_popover
        .connect_show(move |_| agent_workspace.refresh_agent_panel());

    let search_workspace = workspace.clone();
    search_next.connect_clicked(move |_| search_workspace.search_selected(true));
    let search_workspace = workspace.clone();
    search_previous.connect_clicked(move |_| search_workspace.search_selected(false));
    let search_workspace = workspace.clone();
    search_entry.connect_activate(move |_| search_workspace.search_selected(true));

    for (key, choice) in theme_buttons {
        let appearance_workspace = workspace.clone();
        choice.connect_clicked(move |_| {
            appearance_workspace.set_appearance(
                AppearanceSetParams {
                    theme: Some(key),
                    follow_system: None,
                    font: None,
                    ui_font_size: None,
                },
                None,
            );
        });
    }

    let system_workspace = workspace.clone();
    follow_system_toggle.connect_toggled(move |toggle| {
        system_workspace.set_appearance(
            AppearanceSetParams {
                theme: None,
                follow_system: Some(toggle.is_active()),
                font: None,
                ui_font_size: None,
            },
            None,
        );
    });

    let font_workspace = workspace.clone();
    apply_font.connect_clicked(move |_| {
        font_workspace.set_appearance(
            AppearanceSetParams {
                theme: None,
                follow_system: None,
                font: Some(font_workspace.font_entry.text().to_string()),
                ui_font_size: None,
            },
            None,
        );
    });
    let font_workspace = workspace.clone();
    font_entry.connect_activate(move |_| {
        font_workspace.set_appearance(
            AppearanceSetParams {
                theme: None,
                follow_system: None,
                font: Some(font_workspace.font_entry.text().to_string()),
                ui_font_size: None,
            },
            None,
        );
    });

    let system_style_workspace = workspace.clone();
    style_manager.connect_dark_notify(move |_| system_style_workspace.apply_appearance());

    for (action, entry, set, reset) in shortcut_controls {
        let shortcut_workspace = workspace.clone();
        let shortcut_entry = entry.clone();
        set.connect_clicked(move |_| {
            shortcut_workspace.set_shortcut(
                ShortcutSetParams {
                    action,
                    accelerator: Some(shortcut_entry.text().to_string()),
                    reset: false,
                },
                None,
            );
        });
        let shortcut_workspace = workspace.clone();
        reset.connect_clicked(move |_| {
            shortcut_workspace.set_shortcut(
                ShortcutSetParams {
                    action,
                    accelerator: None,
                    reset: true,
                },
                None,
            );
        });
    }

    workspace.install_shortcut_actions();
    workspace.discover_sessions();

    window.present();
}

impl Workspace {
    fn remove_session_view(&self, id: &str) {
        let was_selected = self.selected_session_id().as_deref() == Some(id);
        let Some(session) = self.sessions.borrow_mut().remove(id) else {
            return;
        };
        self.stack.remove(&session.page);
        if session.row.parent().is_some() {
            self.list.remove(&session.row);
        }
        self.rebuild_sidebar();
        if was_selected {
            let row = self
                .sessions
                .borrow()
                .values()
                .min_by_key(|session| session.record.position)
                .map(|session| session.row.clone());
            if let Some(row) = row {
                self.list.select_row(Some(&row));
            } else {
                self.stack.set_visible_child_name(EMPTY_PAGE);
                self.selected_session.borrow_mut().take();
                self.render_history(None);
                self.search_button.set_sensitive(false);
                self.update_emacs_actions();
                self.update_claude_actions();
                self.save_preference("selectedSessionId", serde_json::Value::Null);
            }
        }
    }

    fn run_io<T: Send + 'static>(
        &self,
        work: impl FnOnce() -> PersistResult<T> + Send + 'static,
        done: impl FnOnce(&Self, PersistResult<T>) + 'static,
    ) {
        let result = self.io.submit(work);
        let workspace = self.clone();
        glib::spawn_future_local(async move {
            let result = result
                .await
                .unwrap_or_else(|_| Err("I/O worker stopped".into()));
            done(&workspace, result);
        });
    }

    fn save_preference(&self, key: &'static str, value: serde_json::Value) {
        let Some(store) = self.store.borrow().clone() else {
            return;
        };
        self.run_io(
            move || store.set_preference(key, &value),
            |workspace, result| {
                if let Err(error) = result {
                    workspace.show_error(&format!("Could not save preference: {error}"));
                }
            },
        );
    }

    fn persist_record(&self, record: SessionRecord) {
        let Some(store) = self.store.borrow().clone() else {
            return;
        };
        self.run_io(
            move || store.save_session(&record),
            |workspace, result| {
                if let Err(error) = result {
                    workspace.show_error(&format!("Could not save session: {error}"));
                }
            },
        );
    }

    fn selected_session_id(&self) -> Option<String> {
        self.selected_session.borrow().clone()
    }

    fn next_session_id(&self, kind: SessionKind) -> String {
        let sequence = self.sequence.get();
        self.sequence.set(sequence + 1);
        let millis = SystemTime::now()
            .duration_since(UNIX_EPOCH)
            .unwrap_or_default()
            .as_millis();
        format!("{}-{millis}-{sequence}", session_kind_name(kind))
    }

    fn report_failure(
        &self,
        pending: Option<PendingRequest>,
        code: ErrorCode,
        message: &'static str,
        reason: String,
    ) {
        if let Some(pending) = pending {
            respond_failure(
                pending,
                code,
                message,
                Some(serde_json::json!({ "reason": reason })),
            );
        } else {
            self.show_error(&format!("{message}: {reason}"));
        }
    }

    fn report_launch_failure(
        &self,
        pending: Option<PendingRequest>,
        message: &'static str,
        reason: String,
    ) {
        self.report_failure(pending, ErrorCode::InternalError, message, reason);
    }

    fn show_error(&self, message: &str) {
        self.overlay.add_toast(adw::Toast::new(message));
    }
}

fn widget_bounds(widget: &impl IsA<gtk::Widget>, window: &impl IsA<gtk::Widget>) -> Bounds {
    let Some(bounds) = widget.compute_bounds(window) else {
        return Bounds::default();
    };
    Bounds {
        x: bounds.x(),
        y: bounds.y(),
        width: bounds.width(),
        height: bounds.height(),
    }
}

const fn session_kind_name(kind: SessionKind) -> &'static str {
    match kind {
        SessionKind::Shell => "shell",
        SessionKind::Codex => "codex",
        SessionKind::Claude => "claude",
        SessionKind::Gemini => "gemini",
        SessionKind::Custom => "custom",
    }
}

fn respond_failure(
    pending: PendingRequest,
    code: ErrorCode,
    message: &'static str,
    details: Option<serde_json::Value>,
) {
    let request_id = pending.request.id.clone();
    let _ = pending.respond(ControlResponse::failure(request_id, code, message, details));
}

fn session_not_found(request_id: String, session_id: &str) -> ControlResponse {
    ControlResponse::failure(
        request_id,
        ErrorCode::SessionNotFound,
        "No session exists with that ID",
        Some(serde_json::json!({ "sessionId": session_id })),
    )
}

fn socket_files(directory: &Path) -> Vec<PathBuf> {
    let Ok(entries) = fs::read_dir(directory) else {
        return Vec::new();
    };
    entries
        .flatten()
        .filter(|entry| entry.file_type().is_ok_and(|kind| kind.is_socket()))
        .map(|entry| entry.path())
        .collect()
}

fn sibling_binary(name: &str) -> PathBuf {
    std::env::current_exe()
        .ok()
        .and_then(|path| path.parent().map(|parent| parent.join(name)))
        .unwrap_or_else(|| PathBuf::from(name))
}

fn launch_entry(label: &str, placeholder: &str) -> (gtk::Box, gtk::Entry) {
    let row = gtk::Box::new(gtk::Orientation::Horizontal, 5);
    row.add_css_class("tui-setting-row");
    let label = gtk::Label::new(Some(label));
    label.set_xalign(0.0);
    label.set_width_chars(18);
    row.append(&label);
    let entry = gtk::Entry::builder()
        .placeholder_text(placeholder)
        .width_chars(42)
        .hexpand(true)
        .build();
    entry.add_css_class("tui-setting-entry");
    row.append(&entry);
    (row, entry)
}

fn launch_entry_with_hint(label: &str, placeholder: &str, hint: &str) -> (gtk::Box, gtk::Entry) {
    let row = gtk::Box::new(gtk::Orientation::Horizontal, 5);
    row.add_css_class("tui-setting-row");
    let label = gtk::Label::new(Some(label));
    label.set_xalign(0.0);
    label.set_width_chars(18);
    row.append(&label);
    let controls = gtk::Box::new(gtk::Orientation::Vertical, 1);
    controls.set_hexpand(true);
    let entry = gtk::Entry::builder()
        .placeholder_text(placeholder)
        .width_chars(42)
        .hexpand(true)
        .build();
    entry.add_css_class("tui-setting-entry");
    controls.append(&entry);
    let hint_label = gtk::Label::new(Some(hint));
    hint_label.set_xalign(0.0);
    hint_label.add_css_class("tui-muted");
    controls.append(&hint_label);
    row.append(&controls);
    (row, entry)
}

#[allow(deprecated)]
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
    });
}

fn launch_dropdown(values: &[&str]) -> gtk::DropDown {
    let model = gtk::StringList::new(values);
    let dropdown = gtk::DropDown::new(Some(model), None::<&gtk::Expression>);
    dropdown.set_enable_search(true);
    dropdown.set_hexpand(true);
    dropdown.add_css_class("tui-path-dropdown");
    dropdown
}

fn launch_option_row(label: &str, control: &impl IsA<gtk::Widget>) -> gtk::Box {
    let row = gtk::Box::new(gtk::Orientation::Horizontal, 5);
    row.add_css_class("tui-setting-row");
    let label_widget = gtk::Label::new(Some(label));
    label_widget.set_xalign(0.0);
    label_widget.set_width_chars(18);
    row.append(&label_widget);
    row.append(control);
    row
}

fn claude_launch_options() -> (gtk::DropDown, gtk::CheckButton) {
    (
        launch_dropdown(&["default", "acceptEdits", "bypassPermissions", "plan"]),
        gtk::CheckButton::with_label("--dangerously-skip-permissions"),
    )
}

fn codex_launch_options() -> (
    gtk::DropDown,
    gtk::DropDown,
    gtk::CheckButton,
    gtk::CheckButton,
) {
    (
        launch_dropdown(&["on-request", "never"]),
        launch_dropdown(&["read-only", "workspace-write", "danger-full-access"]),
        gtk::CheckButton::with_label("Full auto (approve for me)"),
        gtk::CheckButton::with_label("--dangerously-bypass-approvals-and-sandbox"),
    )
}

fn gemini_launch_options() -> (gtk::DropDown, gtk::CheckButton) {
    (
        launch_dropdown(&["default", "auto_edit", "yolo", "plan"]),
        gtk::CheckButton::with_label("--yolo"),
    )
}

fn launch_option_page(
    dropdown_label: &str,
    dropdown: &gtk::DropDown,
    _check_label: &str,
    check: &gtk::CheckButton,
) -> gtk::Box {
    let page = gtk::Box::new(gtk::Orientation::Vertical, 2);
    page.append(&launch_option_row(dropdown_label, dropdown));
    page.append(check);
    page
}

fn launch_option_page_with_two_dropdowns(
    first_label: &str,
    first: &gtk::DropDown,
    second_label: &str,
    second: &gtk::DropDown,
    checks: &[(&str, &gtk::CheckButton)],
) -> gtk::Box {
    let page = gtk::Box::new(gtk::Orientation::Vertical, 2);
    page.append(&launch_option_row(first_label, first));
    page.append(&launch_option_row(second_label, second));
    for (_, check) in checks {
        page.append(*check);
    }
    page
}

fn searchable_path_dropdown(model: &gtk::StringList) -> gtk::DropDown {
    let dropdown = gtk::DropDown::new(Some(model.clone()), None::<&gtk::Expression>);
    dropdown.set_enable_search(true);
    dropdown.set_show_arrow(true);
    dropdown.set_tooltip_text(Some("Search known paths or choose one"));
    dropdown.set_size_request(220, -1);
    dropdown.add_css_class("tui-path-dropdown");
    dropdown
}

fn display_name(id: &str) -> String {
    id.strip_prefix("shell-")
        .map(|suffix| format!("Shell {}", suffix.rsplit('-').next().unwrap_or(suffix)))
        .unwrap_or_else(|| id.to_owned())
}
