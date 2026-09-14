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

use crate::control::{
    AppState, AttentionSummary, Bounds, ControlCommand, ControlResponse, ControlServer,
    CreateSessionParams, ErrorCode, PROTOCOL_VERSION, PendingRequest, SessionKind, SessionState,
    SessionSummary, TextSnapshot, UiInspection, UiNode, WindowState,
};
use crate::history::{InputTracker, history_needle};
use crate::instance::InstancePaths;
use crate::session::{SessionLaunchPlan, receive_attachment};
use crate::terminal_text::{bounded_terminal_text, cleanup_copied_text};

mod capture;
mod controls;
mod history_ui;
mod inspection;
mod sessions;

const EMPTY_PAGE: &str = "empty";
const MAX_INSPECTED_SESSIONS: usize = 500;
const PCRE2_LITERAL: u32 = 0x0200_0000;
const PCRE2_UTF: u32 = 0x0008_0000;

#[derive(Clone)]
struct Workspace {
    window: adw::ApplicationWindow,
    list: gtk::ListBox,
    stack: gtk::Stack,
    overlay: adw::ToastOverlay,
    new_shell_button: gtk::Button,
    history_button: gtk::MenuButton,
    history_list: gtk::Box,
    history_popover: gtk::Popover,
    paths: InstancePaths,
    host_binary: PathBuf,
    sessions: Rc<RefCell<HashMap<String, SessionView>>>,
    sequence: Rc<Cell<u64>>,
    control_server: Rc<RefCell<Option<ControlServer>>>,
}

struct SessionView {
    name: String,
    kind: SessionKind,
    terminal: vte::Terminal,
    page: gtk::ScrolledWindow,
    row: gtk::ListBoxRow,
    label: gtk::Label,
    history: Vec<String>,
    _pty: vte::Pty,
    control: UnixStream,
}

#[derive(Clone)]
struct SessionMetadata {
    kind: SessionKind,
    name: Option<String>,
}

pub fn build(app: &adw::Application, paths: InstancePaths) {
    let list = gtk::ListBox::new();
    list.set_selection_mode(gtk::SelectionMode::Single);
    list.add_css_class("navigation-sidebar");
    list.set_size_request(240, -1);

    let sidebar = gtk::ScrolledWindow::builder()
        .hscrollbar_policy(gtk::PolicyType::Never)
        .child(&list)
        .build();

    let stack = gtk::Stack::builder()
        .hexpand(true)
        .vexpand(true)
        .transition_type(gtk::StackTransitionType::Crossfade)
        .build();
    let empty = adw::StatusPage::builder()
        .icon_name("utilities-terminal-symbolic")
        .title("No sessions")
        .description("Start a shell to open an embedded terminal")
        .build();
    stack.add_named(&empty, Some(EMPTY_PAGE));

    let split = gtk::Paned::new(gtk::Orientation::Horizontal);
    split.set_start_child(Some(&sidebar));
    split.set_end_child(Some(&stack));
    split.set_resize_start_child(false);
    split.set_shrink_start_child(false);
    split.set_position(260);

    let header = adw::HeaderBar::new();
    header.set_title_widget(Some(&adw::WindowTitle::new(
        "agmux native",
        "Terminal prototype",
    )));
    let new_shell = gtk::Button::with_label("New shell");
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
        .label("History")
        .popover(&history_popover)
        .sensitive(false)
        .build();
    history_button.set_tooltip_text(Some("Scroll to a submitted prompt"));
    header.pack_end(&history_button);

    let toolbar = adw::ToolbarView::new();
    toolbar.add_top_bar(&header);
    toolbar.set_content(Some(&split));

    let overlay = adw::ToastOverlay::new();
    overlay.set_child(Some(&toolbar));
    let window = adw::ApplicationWindow::builder()
        .application(app)
        .title("agmux native")
        .default_width(1200)
        .default_height(800)
        .content(&overlay)
        .build();

    let workspace = Workspace {
        window: window.clone(),
        list: list.clone(),
        stack: stack.clone(),
        overlay,
        new_shell_button: new_shell.clone(),
        history_button,
        history_list,
        history_popover,
        paths,
        host_binary: sibling_binary("agmux-session"),
        sessions: Rc::new(RefCell::new(HashMap::new())),
        sequence: Rc::new(Cell::new(0)),
        control_server: Rc::new(RefCell::new(None)),
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
    });

    let launch_workspace = workspace.clone();
    new_shell.connect_clicked(move |_| launch_workspace.launch_shell());

    workspace.discover_sessions();
    workspace.start_control_server();
    if workspace.sessions.borrow().is_empty() && workspace.paths.name().as_str() == "default" {
        workspace.launch_shell();
    }

    window.present();
}

impl Workspace {
    fn stop_session(&self, id: &str) -> io::Result<()> {
        let Some(mut session) = self.sessions.borrow_mut().remove(id) else {
            return Ok(());
        };
        let shutdown = session.control.write_all(b"K");
        self.stack.remove(&session.page);
        self.list.remove(&session.row);

        if let Some(row) = self.list.row_at_index(0) {
            self.list.select_row(Some(&row));
        } else {
            self.stack.set_visible_child_name(EMPTY_PAGE);
            self.render_history(None);
        }
        shutdown
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

    fn report_launch_failure(
        &self,
        pending: Option<PendingRequest>,
        message: &'static str,
        reason: String,
    ) {
        if let Some(pending) = pending {
            respond_failure(
                pending,
                ErrorCode::InternalError,
                message,
                Some(serde_json::json!({ "reason": reason })),
            );
        } else {
            self.show_error(&format!("{message}: {reason}"));
        }
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
