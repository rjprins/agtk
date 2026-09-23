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
mod menus;
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
    /// One key per sidebar row; the list is bound to it so updates only touch changed rows.
    sidebar_model: gio::ListStore,
    stack: gtk::Stack,
    overlay: adw::ToastOverlay,
    new_shell_button: gtk::Button,
    content_title: adw::WindowTitle,
    menus: menus::Menus,
    claude_model_button: gtk::Button,
    claude_model_window: modal::Modal,
    claude_model_list: gtk::ListBox,
    selected_claude_preset: Rc<Cell<i32>>,
    prs: azure_ui::PrDialog,
    launch_button: gtk::Button,
    launch: launch_ui::LaunchDialog,
    worktrees: worktrees_ui::WorktreeDialog,
    agents: agents_ui::AgentDialog,
    history_button: gtk::Button,
    history_list: gtk::ListBox,
    history_window: modal::Modal,
    search_button: gtk::Button,
    search_bar: gtk::SearchBar,
    search_entry: gtk::SearchEntry,
    preferences: preferences_ui::PreferencesDialog,
    session_context_bar: gtk::Box,
    context_input: gtk::Box,
    context_last_input: gtk::Label,
    context_pr: gtk::Box,
    context_pr_number: gtk::Button,
    context_pr_title: gtk::Label,
    context_pr_author: gtk::Label,
    context_pr_threads: gtk::Label,
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
    let sidebar_controls = gtk::Box::new(gtk::Orientation::Horizontal, 0);

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
    let menus = menus::Menus::build();
    let claude_model_list = boxed_list();
    let claude_model_surface = dialog_page(
        &claude_model_list,
        "Ctrl+Shift+M moves to the next preset and Enter applies it.",
    );
    let claude_model_button = gtk::Button::builder()
        .icon_name("power-profile-performance-symbolic")
        .sensitive(false)
        .build();
    claude_model_button.set_visible(false);
    claude_model_button.set_tooltip_text(Some("Choose a Claude model and effort preset"));

    let launch_button = gtk::Button::builder()
        .icon_name("list-add-symbolic")
        .sensitive(false)
        .build();
    launch_button.set_tooltip_text(Some("Launch a shell, Codex, Claude, or Gemini session"));

    let history_list = boxed_list();
    let history_surface = dialog_page(
        &history_list,
        "Select a prompt to scroll the terminal back to it.",
    );
    let history_button = gtk::Button::builder()
        .label("History (0)")
        .sensitive(false)
        .build();
    history_button.set_tooltip_text(Some("Scroll to a submitted prompt"));

    let search_entry = gtk::SearchEntry::builder()
        .placeholder_text("Find in terminal")
        .width_chars(32)
        .build();
    let search_previous = gtk::Button::builder()
        .icon_name("go-up-symbolic")
        .tooltip_text("Previous match (Shift+Ctrl+G)")
        .build();
    let search_next = gtk::Button::builder()
        .icon_name("go-down-symbolic")
        .tooltip_text("Next match (Ctrl+G)")
        .build();
    let search_controls = gtk::Box::new(gtk::Orientation::Horizontal, 0);
    search_controls.add_css_class("linked");
    search_controls.append(&search_entry);
    search_controls.append(&search_previous);
    search_controls.append(&search_next);
    // An inline bar like GNOME Console's, so the terminal stays visible while searching.
    let search_bar = gtk::SearchBar::builder()
        .child(&search_controls)
        .show_close_button(true)
        .build();
    search_bar.connect_entry(&search_entry);
    main_pane.add_top_bar(&search_bar);
    let search_button = gtk::Button::builder()
        .icon_name("edit-find-symbolic")
        .sensitive(false)
        .build();
    search_button.set_tooltip_text(Some("Search selected terminal scrollback"));

    new_shell.set_visible(false);
    sidebar_controls.append(&menus.main_button);
    sidebar_bar.pack_start(&launch_button);
    sidebar_bar.pack_end(&sidebar_controls);

    session_context_bar.append(&claude_model_button);
    session_context_bar.append(&menus.git_button);
    session_context_bar.append(&history_button);
    session_context_bar.append(&search_button);
    // Header bars only flatten their direct children, and these sit in a box.
    claude_model_button.add_css_class("flat");
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

    let claude_model_window =
        modal::Modal::new(&window, "Claude Model", 520, 340, &claude_model_surface);
    let launch = launch_ui::LaunchDialog::build(&window);
    let worktrees = worktrees_ui::WorktreeDialog::build(&window);
    let history_window = modal::Modal::new(&window, "Prompt History", 620, 560, &history_surface);
    let preferences = preferences_ui::PreferencesDialog::build(&window);
    let prs = azure_ui::PrDialog::build(&window);
    let agents = agents_ui::AgentDialog::build(&window);
    let workspace = Workspace {
        application: app.clone(),
        window: window.clone(),
        list: list.clone(),
        sidebar_model: gio::ListStore::new::<gtk::StringObject>(),
        stack: stack.clone(),
        overlay,
        new_shell_button: new_shell.clone(),
        content_title: content_title.clone(),
        menus,
        claude_model_button,
        claude_model_window,
        claude_model_list,
        selected_claude_preset: Rc::new(Cell::new(0)),
        prs,
        launch_button,
        launch,
        worktrees,
        agents,
        history_button,
        history_list,
        history_window,
        search_button,
        search_bar: search_bar.clone(),
        search_entry: search_entry.clone(),
        preferences,
        session_context_bar: session_context_bar.clone(),
        context_input: context_input.clone(),
        context_last_input: context_last_input.clone(),
        context_pr,
        context_pr_number,
        context_pr_title,
        context_pr_author,
        context_pr_threads,
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
    workspace.history_button.connect_clicked(move |_| {
        if dialog_workspace.history_button.is_sensitive() {
            dialog_workspace.history_window.present();
        }
    });
    let dialog_workspace = workspace.clone();
    workspace.search_button.connect_clicked(move |_| {
        let open = !dialog_workspace.search_bar.is_search_mode();
        dialog_workspace.search_bar.set_search_mode(open);
        if open {
            dialog_workspace.search_entry.grab_focus();
        }
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

    workspace.connect_worktrees();
    workspace.connect_prs();
    workspace.connect_agents();

    let search_workspace = workspace.clone();
    search_next.connect_clicked(move |_| search_workspace.search_selected(true));
    let search_workspace = workspace.clone();
    search_previous.connect_clicked(move |_| search_workspace.search_selected(false));
    let search_workspace = workspace.clone();
    search_entry.connect_activate(move |_| search_workspace.search_selected(true));
    let search_workspace = workspace.clone();
    search_entry.connect_next_match(move |_| search_workspace.search_selected(true));
    let search_workspace = workspace.clone();
    search_entry.connect_previous_match(move |_| search_workspace.search_selected(false));
    let search_workspace = workspace.clone();
    search_entry.connect_stop_search(move |_| search_workspace.close_search());

    let system_style_workspace = workspace.clone();
    style_manager.connect_dark_notify(move |_| system_style_workspace.apply_appearance());

    workspace.connect_preferences();
    workspace.install_status_timers();
    workspace.bind_sidebar_model();
    workspace.install_menu_actions();
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
                self.search_bar.set_search_mode(false);
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

fn quit_on_main_window_close(quit: impl FnOnce()) -> glib::Propagation {
    quit();
    glib::Propagation::Proceed
}

fn boxed_list() -> gtk::ListBox {
    let list = gtk::ListBox::new();
    list.add_css_class("boxed-list");
    list.set_selection_mode(gtk::SelectionMode::None);
    list
}

/// A scrolling libadwaita page with one described group around `list`.
fn dialog_page(list: &gtk::ListBox, description: &str) -> adw::PreferencesPage {
    let group = adw::PreferencesGroup::builder()
        .description(description)
        .build();
    group.add(list);
    let page = adw::PreferencesPage::new();
    page.add(&group);
    page
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
