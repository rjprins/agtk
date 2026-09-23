#![allow(deprecated)]

use std::cell::{Cell, RefCell};
use std::collections::{HashMap, HashSet};
use std::error::Error;
use std::fs;
use std::io::{self, Write};
use std::os::unix::fs::FileTypeExt;
use std::os::unix::net::UnixStream;
use std::path::{Path, PathBuf};
use std::process::Stdio;
use std::rc::Rc;
use std::thread;
use std::time::{Duration, SystemTime, UNIX_EPOCH};

use adw::prelude::*;
use gtk::pango::FontDescription;
use vte::prelude::*;

use crate::appearance::{AppearancePreferences, ThemeKey, theme};
use crate::azure::{AzurePr, PrContext, PrPreferences};
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
use crate::session::{SessionHostLaunchPlan, SessionLaunchPlan, receive_attachment};
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
mod modal;
mod preferences_ui;
mod projects_ui;
mod provider_icons;
mod search_ui;
mod sessions;
mod shortcuts_ui;
mod sidebar;
mod status_ui;
mod style;
mod worktrees_ui;

const EMPTY_PAGE: &str = "empty";
const MAX_INSPECTED_SESSIONS: usize = 500;
const PCRE2_LITERAL: u32 = 0x0200_0000;
const PCRE2_UTF: u32 = 0x0008_0000;
const PCRE2_MULTILINE: u32 = 0x0000_0400;

#[derive(Clone)]
struct Workspace {
    application: adw::Application,
    window: adw::ApplicationWindow,
    list: gtk::ListBox,
    stack: gtk::Stack,
    overlay: adw::ToastOverlay,
    new_shell_button: gtk::Button,
    content_title: adw::WindowTitle,
    git_button: gtk::MenuButton,
    claude_model_button: gtk::Button,
    claude_model_window: modal::Modal,
    claude_model_list: gtk::Box,
    selected_claude_preset: Rc<Cell<i32>>,
    pr_button: gtk::Button,
    pr_window: modal::Modal,
    pr_root: gtk::Entry,
    pr_auto_toggle: gtk::CheckButton,
    pr_list: gtk::Box,
    pr_loading: gtk::Box,
    pr_spinner: gtk::Spinner,
    updating_pr_toggle: Rc<Cell<bool>>,
    launch_button: gtk::Button,
    launch: launch_ui::LaunchDialog,
    worktree_button: gtk::Button,
    worktree_window: modal::Modal,
    worktree_root: gtk::Entry,
    worktree_branch: gtk::Entry,
    worktree_base: gtk::Entry,
    worktree_purpose: gtk::Entry,
    worktree_list: gtk::Box,
    agent_button: gtk::Button,
    agents: agents_ui::AgentDialog,
    history_button: gtk::Button,
    history_list: gtk::Box,
    history_window: modal::Modal,
    search_button: gtk::Button,
    search_window: modal::Modal,
    search_entry: gtk::Entry,
    theme_button: gtk::Button,
    preferences: preferences_ui::PreferencesDialog,
    shortcut_button: gtk::Button,
    session_context_bar: gtk::Box,
    context_input: gtk::Box,
    context_last_input: gtk::Label,
    context_pr: gtk::Box,
    context_pr_number: gtk::Button,
    context_pr_title: gtk::Label,
    context_pr_author: gtk::Label,
    context_pr_threads: gtk::Label,
    context_branch_review: gtk::Button,
    context_magit: gtk::Button,
    sidebar_panel: gtk::Box,
    sidebar_split: gtk::Paned,
    sidebar_header: gtk::Box,
    sidebar_controls: gtk::Box,
    sidebar_toggle: gtk::Button,
    surface_anchors: gtk::Box,
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
    pr_context_cache: Rc<RefCell<HashMap<String, PrContext>>>,
    claude_presets: Rc<RefCell<ClaudePresetPreferences>>,
    selected_pr: Rc<RefCell<Option<SelectedPrContext>>>,
}

#[derive(Clone)]
struct SelectedPrContext {
    session_id: String,
    pull_request: AzurePr,
}

struct SessionView {
    record: SessionRecord,
    terminal: vte::Terminal,
    page: gtk::ScrolledWindow,
    row: gtk::ListBoxRow,
    label: gtk::Label,
    state_label: gtk::Label,
    elapsed_label: gtk::Label,
    history: Vec<String>,
    /// Last state an agent hook reported; None until the agent sends one.
    hook_signal: Option<crate::agent_status::Signal>,
    tracker: crate::agent_status::ScreenTracker,
    /// A sidebar rename still to be typed into the agent as `/rename`.
    pending_agent_name: Option<String>,
    _pty: Option<vte::Pty>,
    control: Option<UnixStream>,
}

pub fn build(app: &adw::Application, paths: InstancePaths) {
    let display = gtk::gdk::Display::default().expect("GTK application has no display");
    let style_manager = adw::StyleManager::for_display(&display);
    let chrome_style = style::ChromeStyle::install(&display);

    let list = gtk::ListBox::new();
    list.set_selection_mode(gtk::SelectionMode::Single);
    list.set_focus_on_click(false);
    list.add_css_class("navigation-sidebar");

    let sidebar = gtk::ScrolledWindow::builder()
        .hscrollbar_policy(gtk::PolicyType::Never)
        .vexpand(true)
        .child(&list)
        .build();
    let sidebar_brand_icon = provider_icons::brand_icon();
    let sidebar_heading = gtk::Label::new(Some("agmux"));
    sidebar_heading.add_css_class("title");
    let sidebar_header = gtk::Box::new(gtk::Orientation::Horizontal, 6);
    sidebar_header.append(&sidebar_brand_icon);
    sidebar_header.append(&sidebar_heading);
    let sidebar_bar = adw::HeaderBar::new();
    sidebar_bar.set_title_widget(Some(&sidebar_header));
    let sidebar_view = adw::ToolbarView::new();
    sidebar_view.add_top_bar(&sidebar_bar);
    sidebar_view.set_content(Some(&sidebar));
    let sidebar_panel = gtk::Box::new(gtk::Orientation::Vertical, 0);
    sidebar_panel.append(&sidebar_view);
    // Primary menu items; filled once the buttons exist further down.
    let sidebar_controls = gtk::Box::new(gtk::Orientation::Vertical, 0);

    let stack = gtk::Stack::builder()
        .hexpand(true)
        .vexpand(true)
        .transition_type(gtk::StackTransitionType::None)
        .build();
    let empty = adw::StatusPage::builder()
        .icon_name("utilities-terminal-symbolic")
        .title("No Active Session")
        .description("Press Ctrl+Shift+` for a new shell")
        .build();
    stack.add_named(&empty, Some(EMPTY_PAGE));

    let content_title = adw::WindowTitle::new("agmux", "");
    let content_bar = adw::HeaderBar::new();
    content_bar.set_title_widget(Some(&content_title));
    let session_context_bar = gtk::Box::new(gtk::Orientation::Horizontal, 0);
    session_context_bar.set_visible(false);
    let context_branch_review = menu_item("Branch review");
    context_branch_review
        .set_tooltip_text(Some("Open the selected worktree in Emacs branch-review"));
    let context_magit = menu_item("Magit");
    context_magit.set_tooltip_text(Some("Open the selected worktree in Emacs Magit"));
    let context_last_input = gtk::Label::new(Some("(none yet)"));
    context_last_input.set_visible(false);
    let context_input = gtk::Box::new(gtk::Orientation::Horizontal, 0);
    context_input.append(&context_last_input);
    session_context_bar.append(&context_input);
    // The last submitted prompt doubles as the subtitle under the session name.
    context_last_input
        .bind_property("label", &content_title, "subtitle")
        .transform_to(|_, text: String| {
            Some(if text == "(none yet)" {
                String::new()
            } else {
                text
            })
        })
        .sync_create()
        .build();
    content_bar.pack_end(&session_context_bar);

    let context_pr = gtk::Box::new(gtk::Orientation::Horizontal, 8);
    context_pr.add_css_class("toolbar");
    context_pr.set_visible(false);
    let context_pr_number = gtk::Button::with_label("PR #0");
    context_pr_number.add_css_class("flat");
    context_pr_number.add_css_class("accent");
    context_pr_number.set_tooltip_text(Some("Open this pull request in Azure DevOps"));
    let context_pr_title = gtk::Label::new(None);
    context_pr_title.set_xalign(0.0);
    context_pr_title.set_hexpand(true);
    context_pr_title.set_ellipsize(gtk::pango::EllipsizeMode::End);
    let context_pr_author = gtk::Label::new(None);
    context_pr_author.add_css_class("dim-label");
    let context_pr_threads = gtk::Label::new(None);
    context_pr_threads.add_css_class("warning");
    context_pr.append(&context_pr_number);
    context_pr.append(&context_pr_title);
    context_pr.append(&context_pr_author);
    context_pr.append(&context_pr_threads);

    let main_pane = adw::ToolbarView::new();
    main_pane.add_top_bar(&content_bar);
    main_pane.add_top_bar(&context_pr);
    main_pane.set_content(Some(&stack));

    // A plain paned keeps the sidebar drag-resizable; .sidebar-pane gives it the libadwaita look.
    sidebar_panel.add_css_class("sidebar-pane");
    sidebar_panel.set_size_request(sidebar::MIN_SIDEBAR_WIDTH, -1);
    sidebar_bar.set_show_end_title_buttons(false);
    content_bar.set_show_start_title_buttons(false);
    let split = gtk::Paned::new(gtk::Orientation::Horizontal);
    split.set_start_child(Some(&sidebar_panel));
    split.set_end_child(Some(&main_pane));
    split.set_resize_start_child(false);
    split.set_shrink_start_child(false);
    split.set_shrink_end_child(false);
    split.set_position(sidebar::DEFAULT_SIDEBAR_WIDTH);

    let new_shell = gtk::Button::builder()
        .icon_name("utilities-terminal-symbolic")
        .build();
    new_shell.add_css_class("flat");
    new_shell.set_tooltip_text(Some("Start a shell session"));
    let git_surface = gtk::Box::new(gtk::Orientation::Vertical, 0);
    git_surface.append(&context_magit);
    git_surface.append(&context_branch_review);
    let git_popover = menu_popover(&git_surface, &[&context_magit, &context_branch_review]);
    let git_button = gtk::MenuButton::builder()
        .icon_name("text-editor-symbolic")
        .popover(&git_popover)
        .sensitive(false)
        .build();
    git_button.set_tooltip_text(Some("Open the selected worktree in Emacs"));

    let claude_model_list = gtk::Box::new(gtk::Orientation::Vertical, 1);
    let claude_model_surface = gtk::Box::new(gtk::Orientation::Vertical, 0);
    claude_model_surface.set_margin_top(18);
    claude_model_surface.set_margin_bottom(18);
    claude_model_surface.set_margin_start(18);
    claude_model_surface.set_margin_end(18);
    claude_model_surface.append(&claude_model_list);
    let claude_model_button = gtk::Button::builder()
        .icon_name("power-profile-performance-symbolic")
        .sensitive(false)
        .build();
    claude_model_button.set_visible(false);
    claude_model_button.set_tooltip_text(Some("Choose a Claude model and effort preset"));

    let pr_surface = gtk::Box::new(gtk::Orientation::Vertical, 8);
    pr_surface.set_margin_top(18);
    pr_surface.set_margin_bottom(18);
    pr_surface.set_margin_start(18);
    pr_surface.set_margin_end(18);
    let pr_heading_row = gtk::Box::new(gtk::Orientation::Horizontal, 4);
    let pr_heading = gtk::Label::new(Some("Azure pull requests"));
    pr_heading.set_xalign(0.0);
    pr_heading.set_hexpand(true);
    pr_heading.add_css_class("heading");
    // The window title already names the dialog.
    pr_heading.set_visible(false);
    pr_heading_row.set_halign(gtk::Align::End);
    pr_heading_row.append(&pr_heading);
    let pr_refresh = gtk::Button::builder()
        .icon_name("view-refresh-symbolic")
        .build();
    pr_refresh.add_css_class("flat");
    pr_refresh.set_tooltip_text(Some("Refresh active pull requests"));
    pr_heading_row.append(&pr_refresh);
    pr_surface.append(&pr_heading_row);
    let pr_loading = gtk::Box::new(gtk::Orientation::Horizontal, 8);
    let pr_spinner = gtk::Spinner::new();
    pr_spinner.set_size_request(18, 18);
    pr_loading.append(&pr_spinner);
    let pr_loading_label = gtk::Label::new(Some("Loading active pull requests..."));
    pr_loading_label.set_xalign(0.0);
    pr_loading.append(&pr_loading_label);
    pr_loading.set_visible(false);
    pr_surface.append(&pr_loading);
    let pr_root = launch_entry("Project root", "absolute Azure DevOps repository root");
    pr_surface.append(&pr_root.0);
    let pr_auto_toggle = gtk::CheckButton::with_label("Auto-review new attention");
    pr_auto_toggle.set_tooltip_text(Some(
        "Opt in to launching Codex review sessions for later PR attention changes",
    ));
    pr_surface.append(&pr_auto_toggle);
    let pr_list = gtk::Box::new(gtk::Orientation::Vertical, 2);
    let pr_scroller = gtk::ScrolledWindow::builder()
        .hscrollbar_policy(gtk::PolicyType::Never)
        .min_content_width(1040)
        .min_content_height(560)
        .max_content_height(720)
        .child(&pr_list)
        .build();
    pr_surface.append(&pr_scroller);
    let pr_button = menu_item("Pull requests");
    pr_button.set_sensitive(false);
    pr_button.set_tooltip_text(Some(
        "Active Azure DevOps pull requests and review attention",
    ));

    let launch_button = gtk::Button::builder()
        .icon_name("list-add-symbolic")
        .sensitive(false)
        .build();
    launch_button.set_tooltip_text(Some("Launch a shell, Codex, Claude, or Gemini session"));

    let worktree_surface = gtk::Box::new(gtk::Orientation::Vertical, 4);
    worktree_surface.set_margin_top(18);
    worktree_surface.set_margin_bottom(18);
    worktree_surface.set_margin_start(18);
    worktree_surface.set_margin_end(18);
    let worktree_heading = gtk::Label::new(Some("Worktrees"));
    worktree_heading.set_xalign(0.0);
    worktree_heading.add_css_class("heading");
    worktree_heading.set_visible(false);
    worktree_surface.append(&worktree_heading);
    let worktree_root = launch_entry("Project root", "absolute repository root");
    let worktree_refresh = gtk::Button::builder()
        .icon_name("view-refresh-symbolic")
        .build();
    worktree_refresh.add_css_class("flat");
    worktree_root.0.append(&worktree_refresh);
    worktree_surface.append(&worktree_root.0);
    let worktree_branch = launch_entry("New branch", "concise-kebab-case");
    let worktree_base = launch_entry("Base", "default branch when empty");
    let worktree_purpose = launch_entry("Purpose", "why this worktree exists");
    worktree_surface.append(&worktree_branch.0);
    worktree_surface.append(&worktree_base.0);
    worktree_surface.append(&worktree_purpose.0);
    let worktree_create = gtk::Button::with_label("Create worktree");
    worktree_create.add_css_class("suggested-action");
    worktree_create.set_halign(gtk::Align::End);
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
    let worktree_button = menu_item("Worktrees");
    worktree_button.set_sensitive(false);
    worktree_button.set_tooltip_text(Some("Inspect, create, and safely reap worktrees"));

    let agent_button = menu_item("Recent agent sessions");
    agent_button.set_sensitive(false);
    agent_button.set_tooltip_text(Some("Preview and restore recent Codex and Claude sessions"));

    let history_list = gtk::Box::new(gtk::Orientation::Vertical, 2);
    let history_scroller = gtk::ScrolledWindow::builder()
        .hscrollbar_policy(gtk::PolicyType::Never)
        .min_content_width(360)
        .max_content_height(420)
        .child(&history_list)
        .build();
    let history_surface = gtk::Box::new(gtk::Orientation::Vertical, 0);
    history_surface.set_margin_top(18);
    history_surface.set_margin_bottom(18);
    history_surface.set_margin_start(18);
    history_surface.set_margin_end(18);
    history_surface.append(&history_scroller);
    let history_button = gtk::Button::builder()
        .label("History (0)")
        .sensitive(false)
        .build();
    history_button.set_tooltip_text(Some("Scroll to a submitted prompt"));

    let search_surface = gtk::Box::new(gtk::Orientation::Horizontal, 4);
    search_surface.set_margin_top(18);
    search_surface.set_margin_bottom(18);
    search_surface.set_margin_start(18);
    search_surface.set_margin_end(18);
    let search_entry = gtk::Entry::builder()
        .placeholder_text("Find literal text")
        .width_chars(36)
        .build();
    search_surface.append(&search_entry);
    let search_previous = gtk::Button::builder().icon_name("go-up-symbolic").build();
    search_previous.add_css_class("flat");
    search_surface.append(&search_previous);
    let search_next = gtk::Button::builder().icon_name("go-down-symbolic").build();
    search_next.add_css_class("flat");
    search_surface.append(&search_next);
    let search_button = gtk::Button::builder()
        .icon_name("edit-find-symbolic")
        .sensitive(false)
        .build();
    search_button.set_tooltip_text(Some("Search selected terminal scrollback"));

    let theme_button = menu_item("Preferences");
    theme_button.set_sensitive(false);
    theme_button.set_tooltip_text(Some("Terminal appearance and Claude presets"));

    let shortcut_button = menu_item("Keyboard Shortcuts");
    shortcut_button.set_sensitive(false);
    shortcut_button.set_tooltip_text(Some("Configure application shortcuts"));

    new_shell.set_visible(false);
    sidebar_controls.append(&agent_button);
    sidebar_controls.append(&worktree_button);
    sidebar_controls.append(&pr_button);
    sidebar_controls.append(&gtk::Separator::new(gtk::Orientation::Horizontal));
    sidebar_controls.append(&theme_button);
    sidebar_controls.append(&shortcut_button);
    let main_menu = menu_popover(
        &sidebar_controls,
        &[
            &agent_button,
            &worktree_button,
            &pr_button,
            &theme_button,
            &shortcut_button,
        ],
    );
    let main_menu_button = gtk::MenuButton::builder()
        .icon_name("open-menu-symbolic")
        .popover(&main_menu)
        .tooltip_text("Main menu")
        .build();
    sidebar_bar.pack_start(&launch_button);
    sidebar_bar.pack_end(&main_menu_button);

    session_context_bar.append(&claude_model_button);
    session_context_bar.append(&git_button);
    session_context_bar.append(&history_button);
    session_context_bar.append(&search_button);
    // Header bars only flatten their direct children, and these sit in a box.
    claude_model_button.add_css_class("flat");
    git_button.add_css_class("flat");
    history_button.add_css_class("flat");
    search_button.add_css_class("flat");
    // Kept for the inspection tree; nothing needs an off-screen anchor any more.
    let surface_anchors = gtk::Box::new(gtk::Orientation::Horizontal, 0);
    surface_anchors.set_visible(false);
    sidebar_panel.append(&surface_anchors);

    let sidebar_toggle = gtk::Button::builder()
        .icon_name("sidebar-show-symbolic")
        .tooltip_text("Toggle sidebar")
        .build();
    content_bar.pack_start(&sidebar_toggle);
    let toggled_sidebar = sidebar_panel.clone();
    sidebar_toggle.connect_clicked(move |_| {
        toggled_sidebar.set_visible(!toggled_sidebar.is_visible());
    });

    let overlay = adw::ToastOverlay::new();
    overlay.set_child(Some(&split));
    let window = adw::ApplicationWindow::builder()
        .application(app)
        .title("agmux")
        .default_width(1200)
        .default_height(800)
        .content(&overlay)
        .build();
    let close_application = app.clone();
    window.connect_close_request(move |_| quit_on_main_window_close(|| close_application.quit()));

    let claude_model_window = modal::Modal::new(
        &window,
        "Claude model presets",
        620,
        520,
        &claude_model_surface,
    );
    let launch = launch_ui::LaunchDialog::build(&window);
    let worktree_window = modal::Modal::new(&window, "Worktrees", 900, 700, &worktree_surface);
    let history_window = modal::Modal::new(&window, "Prompt history", 620, 600, &history_surface);
    let search_window = modal::Modal::new(&window, "Search terminal", 650, -1, &search_surface);
    let preferences = preferences_ui::PreferencesDialog::build(&window);
    let pr_window = modal::Modal::new(&window, "Azure pull requests", 1120, 760, &pr_surface);
    let agents = agents_ui::AgentDialog::build(&window);
    let workspace = Workspace {
        application: app.clone(),
        window: window.clone(),
        list: list.clone(),
        stack: stack.clone(),
        overlay,
        new_shell_button: new_shell.clone(),
        content_title: content_title.clone(),
        git_button: git_button.clone(),
        claude_model_button,
        claude_model_window,
        claude_model_list,
        selected_claude_preset: Rc::new(Cell::new(0)),
        pr_button,
        pr_window,
        pr_root: pr_root.1,
        pr_auto_toggle: pr_auto_toggle.clone(),
        pr_list,
        pr_loading,
        pr_spinner,
        updating_pr_toggle: Rc::new(Cell::new(false)),
        launch_button,
        launch,
        worktree_button,
        worktree_window,
        worktree_root: worktree_root.1,
        worktree_branch: worktree_branch.1,
        worktree_base: worktree_base.1,
        worktree_purpose: worktree_purpose.1,
        worktree_list,
        agent_button,
        agents,
        history_button,
        history_list,
        history_window,
        search_button,
        search_window,
        search_entry: search_entry.clone(),
        theme_button,
        preferences,
        shortcut_button,
        session_context_bar: session_context_bar.clone(),
        context_input: context_input.clone(),
        context_last_input: context_last_input.clone(),
        context_pr,
        context_pr_number,
        context_pr_title,
        context_pr_author,
        context_pr_threads,
        context_branch_review: context_branch_review.clone(),
        context_magit: context_magit.clone(),
        sidebar_panel: sidebar_panel.clone(),
        sidebar_split: split.clone(),
        sidebar_header: sidebar_header.clone(),
        sidebar_controls: sidebar_controls.clone(),
        sidebar_toggle: sidebar_toggle.clone(),
        surface_anchors: surface_anchors.clone(),
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
        pr_context_cache: Rc::new(RefCell::new(HashMap::new())),
        claude_presets: Rc::new(RefCell::new(ClaudePresetPreferences::default())),
        selected_pr: Rc::new(RefCell::new(None)),
    };

    let dialog_workspace = workspace.clone();
    workspace
        .claude_model_button
        .connect_clicked(move |_| dialog_workspace.open_or_cycle_claude_presets());
    let dialog_workspace = workspace.clone();
    workspace.launch_button.connect_clicked(move |_| {
        dialog_workspace.prepare_launch_panel();
        dialog_workspace.launch.modal.present();
    });
    let dialog_workspace = workspace.clone();
    workspace.worktree_button.connect_clicked(move |_| {
        dialog_workspace.prepare_worktree_panel();
        dialog_workspace.worktree_window.present();
    });
    let dialog_workspace = workspace.clone();
    workspace.agent_button.connect_clicked(move |_| {
        if let Some(root) = dialog_workspace.preferred_project_root() {
            dialog_workspace.open_agent_for_project(&root);
        } else {
            dialog_workspace.agents.modal.present();
        }
    });
    let dialog_workspace = workspace.clone();
    workspace.pr_button.connect_clicked(move |_| {
        if let Some(root) = dialog_workspace.preferred_project_root() {
            dialog_workspace.open_pr_for_project(&root);
        }
    });
    let dialog_workspace = workspace.clone();
    workspace.history_button.connect_clicked(move |_| {
        if dialog_workspace.history_button.is_sensitive() {
            dialog_workspace.history_window.present();
        }
    });
    let dialog_workspace = workspace.clone();
    workspace.search_button.connect_clicked(move |_| {
        if dialog_workspace.search_button.is_sensitive() {
            dialog_workspace.search_window.present();
            dialog_workspace.search_entry.grab_focus();
        }
    });
    let dialog_workspace = workspace.clone();
    workspace.theme_button.connect_clicked(move |_| {
        dialog_workspace
            .preferences
            .open(preferences_ui::APPEARANCE_PAGE);
    });
    let dialog_workspace = workspace.clone();
    workspace.shortcut_button.connect_clicked(move |_| {
        dialog_workspace
            .preferences
            .open(preferences_ui::SHORTCUTS_PAGE);
    });

    let sidebar_width_workspace = workspace.clone();
    split.connect_position_notify(move |split| {
        let position = split.position();
        if position >= sidebar::MIN_SIDEBAR_WIDTH {
            sidebar_width_workspace.save_preference(
                sidebar::SIDEBAR_WIDTH_PREFERENCE,
                serde_json::json!(position),
            );
        }
    });
    let pr_context_workspace = workspace.clone();
    workspace.context_pr_number.connect_clicked(move |_| {
        pr_context_workspace.open_selected_pr_context();
    });

    let selected_workspace = workspace.clone();
    list.connect_row_selected(move |_, row| {
        let Some(row) = row else { return };
        let id = row.widget_name();
        selected_workspace
            .selected_session
            .borrow_mut()
            .replace(id.to_string());
        selected_workspace.stack.set_visible_child_name(&id);
        selected_workspace.session_context_bar.set_visible(true);
        selected_workspace.refresh_content_title();
        if let Some(session) = selected_workspace.sessions.borrow().get(id.as_str()) {
            session.terminal.grab_focus();
        }
        selected_workspace.acknowledge_session(id.as_str());
        selected_workspace.render_history(Some(id.as_str()));
        selected_workspace.refresh_selected_pr_context(id.as_str());
        selected_workspace.search_button.set_sensitive(true);
        selected_workspace.save_preference("selectedSessionId", serde_json::json!(id.as_str()));
        selected_workspace.update_emacs_actions();
        selected_workspace.update_claude_actions();
    });

    workspace.connect_launch_path_controls();
    let launch_agent_workspace = workspace.clone();
    workspace
        .launch
        .agent_dropdown
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
                .launch
                .agent_options
                .set_visible_child_name(session_kind_name(kind));
        });
    let launch_submit_workspace = workspace.clone();
    workspace.launch.submit.connect_clicked(move |_| {
        launch_submit_workspace.launch_from_form(launch_submit_workspace.selected_launch_agent());
    });
    let launch_cancel_workspace = workspace.clone();
    workspace
        .launch
        .cancel
        .connect_clicked(move |_| launch_cancel_workspace.launch.modal.hide());
    for (kind, button) in workspace.launch.agent_buttons.iter() {
        let launch_agent_workspace = workspace.clone();
        let kind = *kind;
        button.connect_clicked(move |_| launch_agent_workspace.set_launch_agent(kind));
    }
    let launch_workspace = workspace.clone();
    workspace
        .launch
        .modal
        .connect_show(move || launch_workspace.prepare_launch_panel());

    let launch_workspace = workspace.clone();
    new_shell.connect_clicked(move |_| launch_workspace.launch_shell());

    let context_magit_workspace = workspace.clone();
    context_magit.connect_clicked(move |_| {
        if let Some(id) = context_magit_workspace.selected_session_id() {
            context_magit_workspace.open_session_in_emacs(
                &id,
                crate::emacs::EmacsAction::Magit,
                None,
            );
        }
    });
    let context_review_workspace = workspace.clone();
    context_branch_review.connect_clicked(move |_| {
        if let Some(id) = context_review_workspace.selected_session_id() {
            context_review_workspace.open_session_in_emacs(
                &id,
                crate::emacs::EmacsAction::BranchReview,
                None,
            );
        }
    });
    let worktree_workspace = workspace.clone();
    worktree_refresh.connect_clicked(move |_| worktree_workspace.refresh_worktree_panel());
    let worktree_workspace = workspace.clone();
    worktree_create.connect_clicked(move |_| worktree_workspace.create_worktree_from_panel());
    let worktree_workspace = workspace.clone();
    workspace.worktree_window.connect_show(move || {
        worktree_workspace.prepare_worktree_panel();
    });
    let pr_workspace = workspace.clone();
    pr_refresh.connect_clicked(move |_| pr_workspace.refresh_pr_panel(None));
    let pr_workspace = workspace.clone();
    workspace
        .pr_window
        .connect_show(move || pr_workspace.prepare_pr_panel());
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
    workspace.connect_agents();

    let search_workspace = workspace.clone();
    search_next.connect_clicked(move |_| search_workspace.search_selected(true));
    let search_workspace = workspace.clone();
    search_previous.connect_clicked(move |_| search_workspace.search_selected(false));
    let search_workspace = workspace.clone();
    search_entry.connect_activate(move |_| search_workspace.search_selected(true));

    let system_style_workspace = workspace.clone();
    style_manager.connect_dark_notify(move |_| system_style_workspace.apply_appearance());

    workspace.connect_preferences();
    workspace.install_status_timers();
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
                self.session_context_bar.set_visible(false);
                self.selected_session.borrow_mut().take();
                self.refresh_content_title();
                self.render_history(None);
                self.clear_selected_pr_context();
                self.search_button.set_sensitive(false);
                self.update_emacs_actions();
                self.update_claude_actions();
                self.save_preference("selectedSessionId", serde_json::Value::Null);
            }
        }
    }

    /// The content header names the selected session, or the app when nothing is selected.
    pub(super) fn refresh_content_title(&self) {
        let name = self.selected_session_id().and_then(|id| {
            self.sessions
                .borrow()
                .get(&id)
                .map(|session| session.record.name.clone())
        });
        self.content_title
            .set_title(name.as_deref().unwrap_or("agmux"));
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
    let label = gtk::Label::new(Some(label));
    label.set_xalign(0.0);
    label.set_width_chars(18);
    row.append(&label);
    let entry = gtk::Entry::builder()
        .placeholder_text(placeholder)
        .width_chars(42)
        .hexpand(true)
        .build();
    row.append(&entry);
    (row, entry)
}

fn quit_on_main_window_close(quit: impl FnOnce()) -> glib::Propagation {
    quit();
    glib::Propagation::Proceed
}

/// A flat, left-aligned text button that reads as a menu entry inside a popover.
fn menu_item(label: &str) -> gtk::Button {
    let button = gtk::Button::with_label(label);
    button.add_css_class("flat");
    button.set_halign(gtk::Align::Fill);
    if let Some(label) = button.child().and_downcast::<gtk::Label>() {
        label.set_xalign(0.0);
    }
    button
}

pub(super) fn set_menu_item_label(button: &gtk::Button, text: &str) {
    button.set_label(text);
    if let Some(label) = button.child().and_downcast::<gtk::Label>() {
        label.set_xalign(0.0);
    }
}

/// Wraps menu items in a popover that closes itself when one of them is used.
fn menu_popover(surface: &gtk::Box, items: &[&gtk::Button]) -> gtk::Popover {
    let popover = gtk::Popover::builder().child(surface).build();
    popover.add_css_class("menu");
    for item in items {
        let popover = popover.clone();
        item.connect_clicked(move |_| popover.popdown());
    }
    popover
}

fn display_name(id: &str) -> String {
    id.strip_prefix("shell-")
        .map(|suffix| format!("Shell {}", suffix.rsplit('-').next().unwrap_or(suffix)))
        .unwrap_or_else(|| id.to_owned())
}

#[cfg(test)]
mod tests {
    use super::quit_on_main_window_close;

    #[test]
    fn closing_the_main_window_requests_application_quit() {
        let mut quit_requested = false;

        let propagation = quit_on_main_window_close(|| quit_requested = true);

        assert!(quit_requested);
        assert_eq!(propagation, glib::Propagation::Proceed);
    }
}
