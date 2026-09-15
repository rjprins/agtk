use std::cell::{Cell, RefCell};
use std::collections::HashMap;
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
use crate::control::{
    AppState, AppearanceSetParams, AppearanceSummary, AttentionSummary, Bounds, ControlCommand,
    ControlResponse, ControlServer, CreateSessionParams, ErrorCode, PROTOCOL_VERSION,
    PendingRequest, SessionKind, SessionState, SessionSummary, ShortcutSetParams, ShortcutSummary,
    TextSnapshot, UiInspection, UiNode, WindowState,
};
use crate::history::{InputTracker, history_needle};
use crate::instance::InstancePaths;
use crate::io_worker::IoWorker;
use crate::persist::{PersistResult, SessionRecord, Store};
use crate::session::{SessionLaunchPlan, receive_attachment};
use crate::shortcuts::{ShortcutAction, ShortcutPreferences};
use crate::terminal_text::{bounded_terminal_text, cleanup_copied_text};

mod appearance_ui;
mod capture;
mod controls;
mod history_ui;
mod inspection;
mod sessions;
mod shortcuts_ui;
mod style;

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
    history_button: gtk::MenuButton,
    history_list: gtk::Box,
    history_popover: gtk::Popover,
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
    sequence: Rc<Cell<u64>>,
    control_server: Rc<RefCell<Option<ControlServer>>>,
    io: IoWorker,
    store: Rc<RefCell<Option<Store>>>,
    appearance: Rc<RefCell<AppearancePreferences>>,
    chrome_style: style::ChromeStyle,
    style_manager: adw::StyleManager,
    shortcuts: Rc<RefCell<ShortcutPreferences>>,
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
    sidebar_heading.add_css_class("tui-sidebar-heading");
    let sidebar_panel = gtk::Box::new(gtk::Orientation::Vertical, 0);
    sidebar_panel.add_css_class("tui-sidebar");
    sidebar_panel.set_size_request(252, -1);
    sidebar_panel.append(&sidebar_heading);
    sidebar_panel.append(&sidebar);

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
    let new_shell = gtk::Button::with_label("[+ shell]");
    new_shell.add_css_class("tui-button");
    new_shell.set_tooltip_text(Some("Start a shell session"));
    header.pack_end(&new_shell);

    let history_list = gtk::Box::new(gtk::Orientation::Vertical, 2);
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
        .label("[history]")
        .popover(&history_popover)
        .sensitive(false)
        .build();
    history_popover.add_css_class("tui-popover");
    history_button.add_css_class("tui-button");
    history_button.set_tooltip_text(Some("Scroll to a submitted prompt"));
    header.pack_end(&history_button);

    let theme_list = gtk::Box::new(gtk::Orientation::Vertical, 1);
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
        let choice = gtk::Button::with_label(&format!("[{}]", theme(key).name));
        choice.add_css_class("tui-button");
        choice.set_tooltip_text(Some(&format!(
            "Use the {} terminal palette",
            theme(key).name
        )));
        theme_list.append(&choice);
        theme_buttons.push((key, choice));
    }
    let follow_system_toggle = gtk::CheckButton::with_label("[follow system light/dark]");
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
    let apply_font = gtk::Button::with_label("[apply font]");
    apply_font.add_css_class("tui-button");
    theme_list.append(&apply_font);
    let theme_popover = gtk::Popover::builder().child(&theme_list).build();
    theme_popover.add_css_class("tui-popover");
    let theme_button = gtk::MenuButton::builder()
        .label("[theme: neutral]")
        .popover(&theme_popover)
        .sensitive(false)
        .build();
    theme_button.add_css_class("tui-button");
    theme_button.set_tooltip_text(Some("Choose terminal colors and font"));
    header.pack_end(&theme_button);

    let shortcut_list = gtk::Box::new(gtk::Orientation::Vertical, 2);
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
        let set = gtk::Button::with_label("[set]");
        set.add_css_class("tui-button");
        set.set_tooltip_text(Some(&format!("Set shortcut for {}", action.label())));
        row.append(&set);
        let reset = gtk::Button::with_label("[reset]");
        reset.add_css_class("tui-button");
        reset.set_tooltip_text(Some(&format!("Reset shortcut for {}", action.label())));
        row.append(&reset);
        shortcut_list.append(&row);
        shortcut_entries.push((action, entry.clone()));
        shortcut_controls.push((action, entry, set, reset));
    }
    let shortcut_popover = gtk::Popover::builder().child(&shortcut_list).build();
    shortcut_popover.add_css_class("tui-popover");
    let shortcut_button = gtk::MenuButton::builder()
        .label("[keys]")
        .popover(&shortcut_popover)
        .sensitive(false)
        .build();
    shortcut_button.add_css_class("tui-button");
    shortcut_button.set_tooltip_text(Some("Configure application shortcuts"));
    header.pack_end(&shortcut_button);

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
        history_button,
        history_list,
        history_popover,
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
        sequence: Rc::new(Cell::new(0)),
        control_server: Rc::new(RefCell::new(None)),
        io: IoWorker::default(),
        store: Rc::new(RefCell::new(None)),
        appearance: Rc::new(RefCell::new(AppearancePreferences::default())),
        chrome_style,
        style_manager: style_manager.clone(),
        shortcuts: Rc::new(RefCell::new(ShortcutPreferences::default())),
    };

    let selected_workspace = workspace.clone();
    list.connect_row_selected(move |_, row| {
        let Some(row) = row else { return };
        let id = row.widget_name();
        selected_workspace.stack.set_visible_child_name(&id);
        if let Some(session) = selected_workspace.sessions.borrow().get(id.as_str()) {
            session.terminal.grab_focus();
        }
        selected_workspace.render_history(Some(id.as_str()));
        selected_workspace.save_preference("selectedSessionId", serde_json::json!(id.as_str()));
    });

    let launch_workspace = workspace.clone();
    new_shell.connect_clicked(move |_| launch_workspace.launch_shell());

    for (key, choice) in theme_buttons {
        let appearance_workspace = workspace.clone();
        choice.connect_clicked(move |_| {
            appearance_workspace.set_appearance(
                AppearanceSetParams {
                    theme: Some(key),
                    follow_system: None,
                    font: None,
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
        self.list.remove(&session.row);
        if was_selected {
            if let Some(row) = self.list.row_at_index(0) {
                self.list.select_row(Some(&row));
            } else {
                self.stack.set_visible_child_name(EMPTY_PAGE);
                self.render_history(None);
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
        self.list
            .selected_row()
            .map(|row| row.widget_name().to_string())
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
        SessionKind::Custom => "custom",
    }
}

const fn session_kind_short(kind: SessionKind) -> &'static str {
    match kind {
        SessionKind::Shell => "SH",
        SessionKind::Codex => "CX",
        SessionKind::Claude => "CL",
        SessionKind::Custom => "EX",
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

fn display_name(id: &str) -> String {
    id.strip_prefix("shell-")
        .map(|suffix| format!("Shell {}", suffix.rsplit('-').next().unwrap_or(suffix)))
        .unwrap_or_else(|| id.to_owned())
}
