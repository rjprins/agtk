use std::cell::{Cell, RefCell};
use std::collections::HashMap;
use std::error::Error;
use std::fs;
use std::io::Write;
use std::os::unix::fs::FileTypeExt;
use std::os::unix::net::UnixStream;
use std::path::{Path, PathBuf};
use std::process::{Command, Stdio};
use std::rc::Rc;
use std::time::{Duration, SystemTime, UNIX_EPOCH};

use adw::prelude::*;
use gtk::pango::FontDescription;
use vte::prelude::*;

use crate::control::{
    AppState, AttentionSummary, ControlCommand, ControlResponse, ControlServer, ErrorCode,
    PROTOCOL_VERSION, PendingRequest, SessionKind, SessionState, SessionSummary, WindowState,
};
use crate::history::{InputTracker, history_needle};
use crate::instance::InstancePaths;
use crate::session::receive_attachment;
use crate::terminal_text::cleanup_copied_text;

const EMPTY_PAGE: &str = "empty";
const PCRE2_LITERAL: u32 = 0x0200_0000;
const PCRE2_UTF: u32 = 0x0008_0000;

#[derive(Clone)]
struct Workspace {
    window: adw::ApplicationWindow,
    list: gtk::ListBox,
    stack: gtk::Stack,
    overlay: adw::ToastOverlay,
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
    terminal: vte::Terminal,
    page: gtk::ScrolledWindow,
    row: gtk::ListBoxRow,
    history: Vec<String>,
    _pty: vte::Pty,
    control: UnixStream,
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
    fn discover_sessions(&self) {
        let mut sockets = socket_files(&self.paths.sessions_dir());
        if self.paths.name().as_str() == "default"
            && let Some(legacy_dir) = self.paths.runtime_dir().parent()
        {
            sockets.extend(socket_files(legacy_dir));
        }
        sockets.sort();
        for socket in sockets {
            let _ = self.attach(&socket);
        }
    }

    fn launch_shell(&self) {
        let sessions_dir = self.paths.sessions_dir();
        if let Err(error) = fs::create_dir_all(&sessions_dir) {
            self.show_error(&format!("Could not create runtime directory: {error}"));
            return;
        }

        let id = self.next_session_id();
        let socket_path = sessions_dir.join(format!("{id}.sock"));
        let shell = std::env::var("SHELL").unwrap_or_else(|_| "/bin/bash".to_owned());
        let spawn = Command::new(&self.host_binary)
            .args(["--socket", socket_path.to_string_lossy().as_ref()])
            .args(["--", shell.as_str()])
            .stdin(Stdio::null())
            .stdout(Stdio::null())
            .stderr(Stdio::null())
            .spawn();
        if let Err(error) = spawn {
            self.show_error(&format!("Could not start session host: {error}"));
            return;
        }

        let workspace = self.clone();
        let attempts = Rc::new(Cell::new(0_u8));
        glib::timeout_add_local(Duration::from_millis(20), move || {
            match workspace.attach(&socket_path) {
                Ok(()) => glib::ControlFlow::Break,
                Err(_) if attempts.get() < 50 => {
                    attempts.set(attempts.get() + 1);
                    glib::ControlFlow::Continue
                }
                Err(error) => {
                    workspace.show_error(&format!("Could not attach session: {error}"));
                    glib::ControlFlow::Break
                }
            }
        });
    }

    fn attach(&self, socket_path: &Path) -> Result<(), Box<dyn Error>> {
        let id = socket_path
            .file_stem()
            .and_then(|name| name.to_str())
            .ok_or("invalid session socket name")?
            .to_owned();
        if self.sessions.borrow().contains_key(&id) {
            return Ok(());
        }

        let control = UnixStream::connect(socket_path)?;
        let attachment = receive_attachment(&control)?;
        let terminal = vte::Terminal::new();
        terminal.set_hexpand(true);
        terminal.set_vexpand(true);
        terminal.set_scrollback_lines(50_000);
        terminal.set_scroll_on_keystroke(true);
        terminal.set_font(Some(&FontDescription::from_string("Monospace 11")));
        terminal.feed(&attachment.replay);

        let pty = vte::Pty::foreign_sync(attachment.pty, None::<&gio::Cancellable>)?;
        terminal.set_pty(Some(&pty));
        terminal.connect_selection_changed(|terminal| {
            let Some(selection) = terminal.text_selected(vte::Format::Text) else {
                return;
            };
            let cleaned = cleanup_copied_text(selection.as_str());
            if !cleaned.is_empty() {
                terminal.clipboard().set_text(&cleaned);
            }
        });

        let input_tracker = Rc::new(RefCell::new(InputTracker::default()));
        let tracker = input_tracker.clone();
        let history_workspace = self.clone();
        let history_id = id.clone();
        terminal.connect_commit(move |_, text, _| {
            if let Some(input) = tracker.borrow_mut().push(text) {
                history_workspace.record_input(&history_id, input);
            }
        });

        let scroll = gtk::ScrolledWindow::builder().child(&terminal).build();
        self.stack.add_named(&scroll, Some(&id));

        let name = display_name(&id);
        let row = gtk::ListBoxRow::new();
        row.set_widget_name(&id);
        let content = gtk::Box::new(gtk::Orientation::Horizontal, 10);
        content.set_margin_top(10);
        content.set_margin_bottom(10);
        content.set_margin_start(12);
        content.set_margin_end(12);
        content.append(&gtk::Image::from_icon_name("utilities-terminal-symbolic"));
        let label = gtk::Label::new(Some(&name));
        label.set_xalign(0.0);
        label.set_hexpand(true);
        content.append(&label);
        let close = gtk::Button::from_icon_name("window-close-symbolic");
        close.add_css_class("flat");
        close.set_tooltip_text(Some("Close this shell session"));
        content.append(&close);
        row.set_child(Some(&content));
        self.list.append(&row);

        let close_workspace = self.clone();
        let close_id = id.clone();
        close.connect_clicked(move |_| close_workspace.stop_session(&close_id));

        self.sessions.borrow_mut().insert(
            id.clone(),
            SessionView {
                name,
                terminal,
                page: scroll,
                row: row.clone(),
                history: Vec::new(),
                _pty: pty,
                control,
            },
        );
        self.list.select_row(Some(&row));
        Ok(())
    }

    fn start_control_server(&self) {
        let socket_path = self.paths.control_socket();
        let (server, requests) = match ControlServer::bind(&socket_path) {
            Ok(bound) => bound,
            Err(error) => {
                self.show_error(&format!("Could not start control socket: {error}"));
                return;
            }
        };
        self.control_server.borrow_mut().replace(server);

        let workspace = self.clone();
        // GLib documents timeout_add_local as scheduling on the default main loop.
        // Source: https://gtk-rs.org/gtk-rs-core/stable/latest/docs/glib/source/fn.timeout_add_local.html
        glib::timeout_add_local(Duration::from_millis(10), move || {
            for pending in requests.try_iter() {
                workspace.handle_control_request(pending);
            }
            glib::ControlFlow::Continue
        });
    }

    fn handle_control_request(&self, pending: PendingRequest) {
        let id = pending.request.id.clone();
        let response = match &pending.request.command {
            ControlCommand::AppGetState => match serde_json::to_value(self.app_state()) {
                Ok(state) => ControlResponse::success(id, state),
                Err(error) => ControlResponse::failure(
                    id,
                    ErrorCode::InternalError,
                    "Could not serialize application state",
                    Some(serde_json::json!({ "reason": error.to_string() })),
                ),
            },
            command => ControlResponse::failure(
                id,
                ErrorCode::NotImplemented,
                "Control method is not implemented yet",
                Some(serde_json::json!({ "method": command.method() })),
            ),
        };
        let _ = pending.respond(response);
    }

    fn app_state(&self) -> AppState {
        let selected_session_id = self.selected_session_id();
        let mut sessions = self
            .sessions
            .borrow()
            .iter()
            .map(|(id, session)| SessionSummary {
                id: id.clone(),
                name: session.name.clone(),
                kind: SessionKind::Shell,
                state: SessionState::Running,
                is_selected: selected_session_id.as_deref() == Some(id.as_str()),
                history_count: session.history.len(),
            })
            .collect::<Vec<_>>();
        sessions.sort_by(|left, right| left.id.cmp(&right.id));

        AppState {
            instance: self.paths.name().as_str().to_owned(),
            protocol_version: PROTOCOL_VERSION,
            selected_session_id,
            window: WindowState {
                width: self.window.width(),
                height: self.window.height(),
            },
            projects: Vec::new(),
            worktree_groups: Vec::new(),
            sessions,
            attention: AttentionSummary { count: 0 },
        }
    }

    fn record_input(&self, id: &str, input: String) {
        let mut sessions = self.sessions.borrow_mut();
        let Some(session) = sessions.get_mut(id) else {
            return;
        };
        if session.history.last() != Some(&input) {
            session.history.push(input);
            if session.history.len() > 200 {
                session.history.remove(0);
            }
        }
        drop(sessions);

        if self.selected_session_id().as_deref() == Some(id) {
            self.render_history(Some(id));
        }
    }

    fn render_history(&self, id: Option<&str>) {
        while let Some(child) = self.history_list.first_child() {
            self.history_list.remove(&child);
        }

        let Some(id) = id else {
            self.history_button.set_sensitive(false);
            return;
        };
        let sessions = self.sessions.borrow();
        let Some(session) = sessions.get(id) else {
            self.history_button.set_sensitive(false);
            return;
        };
        self.history_button
            .set_sensitive(!session.history.is_empty());
        for input in session.history.iter().rev() {
            let button = gtk::Button::with_label(input);
            button.add_css_class("flat");
            button.set_halign(gtk::Align::Fill);
            button.set_tooltip_text(Some("Scroll the terminal to this prompt"));

            let terminal = session.terminal.clone();
            let popover = self.history_popover.clone();
            let overlay = self.overlay.clone();
            let input = input.clone();
            button.connect_clicked(move |_| {
                let needle = history_needle(&input, 60);
                let Ok(regex) = vte::Regex::for_search(&needle, PCRE2_UTF | PCRE2_LITERAL) else {
                    overlay.add_toast(adw::Toast::new("Could not search terminal scrollback"));
                    return;
                };
                terminal.search_set_regex(Some(&regex), 0);
                terminal.search_set_wrap_around(false);
                if !terminal.search_find_previous() {
                    overlay.add_toast(adw::Toast::new("Prompt is no longer in scrollback"));
                }
                popover.popdown();
                terminal.grab_focus();
            });
            self.history_list.append(&button);
        }
    }

    fn stop_session(&self, id: &str) {
        let Some(mut session) = self.sessions.borrow_mut().remove(id) else {
            return;
        };
        if let Err(error) = session.control.write_all(b"K") {
            self.show_error(&format!("Could not stop session: {error}"));
        }
        self.stack.remove(&session.page);
        self.list.remove(&session.row);

        if let Some(row) = self.list.row_at_index(0) {
            self.list.select_row(Some(&row));
        } else {
            self.stack.set_visible_child_name(EMPTY_PAGE);
            self.render_history(None);
        }
    }

    fn selected_session_id(&self) -> Option<String> {
        self.list
            .selected_row()
            .map(|row| row.widget_name().to_string())
    }

    fn next_session_id(&self) -> String {
        let sequence = self.sequence.get();
        self.sequence.set(sequence + 1);
        let millis = SystemTime::now()
            .duration_since(UNIX_EPOCH)
            .unwrap_or_default()
            .as_millis();
        format!("shell-{millis}-{sequence}")
    }

    fn show_error(&self, message: &str) {
        self.overlay.add_toast(adw::Toast::new(message));
    }
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
