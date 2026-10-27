use std::cell::RefCell;
use std::collections::VecDeque;
use std::rc::Rc;
use std::sync::Once;
use std::time::{Duration, Instant};

use adw::prelude::*;
use serde::Deserialize;
use serde_json::Value;
use webkit6::prelude::*;
use webkit6::{
    NavigationPolicyDecision, NetworkSession, PolicyDecisionType, UserContentManager, WebContext,
    WebView,
};

const SCHEME: &str = "agtk-diff";
const ORIGIN: &str = "agtk-diff://viewer/";
const INDEX: &str = "agtk-diff://viewer/index.html";
const RESOURCE_PREFIX: &str = "/nl/rutger/Agtk/viewer/";
const MESSAGE_HANDLER: &str = "agtk";
const MAX_EVENT_BYTES: usize = 32 * 1024;
const MAX_DIFF_BYTES: usize = 4 * 1024 * 1024 + MAX_EVENT_BYTES;
const MAX_IMAGE_PAYLOAD_BYTES: usize =
    crate::changes::MAX_IMAGE_BYTES.div_ceil(3) * 4 + MAX_EVENT_BYTES;

/// The page stays down when its web process stops again this soon, since a new
/// one would likely stop the same way, for example after a library upgrade.
const CRASH_LOOP_WINDOW: Duration = Duration::from_secs(30);

static REGISTER_RESOURCES: Once = Once::new();

#[derive(Debug, Deserialize)]
#[serde(
    tag = "type",
    rename_all = "camelCase",
    rename_all_fields = "camelCase"
)]
pub(super) enum ViewerEvent {
    Ready {
        #[serde(default)]
        version: Option<String>,
    },
    DiffRendered {
        request_id: String,
        tab_id: String,
        line_changes: usize,
        character_changes: usize,
    },
    FileRendered {
        request_id: String,
        tab_id: String,
        line_count: usize,
    },
    ImageRendered {
        request_id: String,
        tab_id: String,
        width: u32,
        height: u32,
    },
    ViewState {
        tab_id: String,
        state: Value,
    },
    /// A link clicked in rendered Markdown, as written in the document.
    OpenLink {
        tab_id: String,
        href: String,
    },
    /// Go to definition or find references at a one-based line and column.
    LookUp {
        request_id: u32,
        document_id: String,
        lookup: crate::language_server::Lookup,
        line: u32,
        column: u32,
    },
    /// Monaco followed a definition or reference: within the shown text when `path` is absent.
    Jump {
        document_id: String,
        #[serde(default)]
        path: Option<String>,
        line: u32,
        column: u32,
        from_line: u32,
        from_column: u32,
    },
    /// Back or forward through earlier jumps, from the cursor at `line` and `column`.
    History {
        document_id: String,
        forward: bool,
        line: u32,
        column: u32,
    },
    /// Selected text with a comment for an agent session, from one-based
    /// `start_line` to `end_line`. `text` is absent when it was too long to send.
    Comment {
        comment_id: u32,
        document_id: String,
        session_id: String,
        comment: String,
        start_line: u32,
        end_line: u32,
        #[serde(default)]
        text: Option<String>,
    },
    Error {
        #[serde(default)]
        request_id: Option<String>,
        message: String,
    },
    /// The page's web process stopped. agtk loads the page again and shows the
    /// same document when `reloading`, and otherwise waits for something new to show.
    #[serde(skip_deserializing)]
    Stopped {
        reloading: bool,
    },
}

struct ViewerState {
    ready: bool,
    queued: VecDeque<ViewerCommand>,
    /// Whether rendered Markdown shows instead of Monaco.
    preview: bool,
    /// The commands that show the current document again in a reloaded page.
    appearance: Option<ViewerCommand>,
    comment_sessions: Option<ViewerCommand>,
    shown: Option<ViewerCommand>,
    presentation: Option<ViewerCommand>,
    /// When the web process last stopped, and whether the page stays down.
    stopped_at: Option<Instant>,
    stopped: bool,
}

impl ViewerState {
    fn remember(&mut self, command: &ViewerCommand) {
        match command {
            ViewerCommand::ShowDiff(_) | ViewerCommand::ShowFile(_) => {
                self.shown = Some(command.clone());
                self.presentation = None;
            }
            ViewerCommand::Clear => {
                self.shown = None;
                self.presentation = None;
            }
            ViewerCommand::SetPresentation { .. } => self.presentation = Some(command.clone()),
            ViewerCommand::SetAppearance { .. } => self.appearance = Some(command.clone()),
            ViewerCommand::SetCommentSessions(_) => self.comment_sessions = Some(command.clone()),
            _ => {}
        }
    }
}

#[derive(Clone)]
enum ViewerCommand {
    ShowDiff(String),
    ShowFile(String),
    Reveal {
        line: u32,
        column: Option<u32>,
    },
    Clear,
    Find {
        query: String,
        next: bool,
    },
    Copy,
    PreviousChange,
    NextChange,
    SaveViewState,
    SetPresentation {
        preview: bool,
    },
    SetAppearance {
        theme: String,
        font_family: String,
        font_size: u32,
    },
    AnswerLookup {
        request_id: u32,
        answer: String,
    },
    /// The agent sessions a comment can go to, as JSON.
    SetCommentSessions(String),
    AnswerComment {
        comment_id: u32,
        error: String,
    },
}

/// One Monaco WebView that shows either a diff or a whole file, read only.
pub(super) struct CodeViewer {
    web_view: WebView,
    state: Rc<RefCell<ViewerState>>,
}

impl CodeViewer {
    pub(super) fn new(on_event: impl Fn(ViewerEvent) + 'static) -> Self {
        REGISTER_RESOURCES.call_once(|| {
            gio::resources_register_include!("viewer.gresource")
                .expect("embedded code viewer resources are invalid");
        });

        let context = WebContext::new();
        if let Some(security) = context.security_manager() {
            security.register_uri_scheme_as_secure(SCHEME);
            security.register_uri_scheme_as_cors_enabled(SCHEME);
        }
        context.register_uri_scheme(SCHEME, serve_asset);

        let content_manager = UserContentManager::new();
        let network_session = NetworkSession::new_ephemeral();
        network_session.connect_download_started(|_, download| download.cancel());
        let web_view = WebView::builder()
            .hexpand(true)
            .vexpand(true)
            .web_context(&context)
            .network_session(&network_session)
            .user_content_manager(&content_manager)
            .default_content_security_policy(
                "default-src 'self' blob:; \
                 script-src 'self' blob:; \
                 style-src 'self' 'unsafe-inline'; \
                 font-src 'self' data:; \
                 img-src 'self' data: blob:; \
                 worker-src 'self' blob:; \
                 connect-src 'self'; \
                 object-src 'none'; base-uri 'none'; form-action 'none'",
            )
            .build();

        let state = Rc::new(RefCell::new(ViewerState {
            ready: false,
            queued: VecDeque::new(),
            preview: false,
            appearance: None,
            comment_sessions: None,
            shown: None,
            presentation: None,
            stopped_at: None,
            stopped: false,
        }));
        let message_state = state.clone();
        let message_view = web_view.clone();
        let on_event = Rc::new(on_event);
        let event_callback = on_event.clone();
        content_manager.connect_script_message_received(Some(MESSAGE_HANDLER), move |_, value| {
            let Some(json) = value.to_json(0) else {
                return;
            };
            if json.len() > MAX_EVENT_BYTES {
                eprintln!("Code viewer sent an oversized event");
                return;
            }
            let event = match serde_json::from_str::<ViewerEvent>(json.as_str()) {
                Ok(event) => event,
                Err(error) => {
                    eprintln!("Code viewer sent an invalid event: {error}");
                    return;
                }
            };
            if !valid_event(&event) {
                eprintln!("Code viewer sent an out-of-bounds event");
                return;
            }
            if matches!(event, ViewerEvent::Ready { .. }) {
                message_state.borrow_mut().ready = true;
                flush_queued(&message_view, &message_state);
            }
            event_callback(event);
        });
        assert!(content_manager.register_script_message_handler(MESSAGE_HANDLER, None));

        web_view.connect_decide_policy(move |_, decision, decision_type| {
            if decision_type != PolicyDecisionType::NavigationAction {
                return false;
            }
            let Some(navigation) = decision.downcast_ref::<NavigationPolicyDecision>() else {
                decision.ignore();
                return true;
            };
            let allowed = navigation
                .navigation_action()
                .and_then(|action| action.request())
                .and_then(|request| request.uri())
                .is_some_and(|uri| uri.starts_with(ORIGIN));
            if !allowed {
                decision.ignore();
                return true;
            }
            false
        });
        web_view.connect_create(|_, _| None);
        // Without this the view stays blank once its web process crashes.
        let stopped_state = state.clone();
        web_view.connect_web_process_terminated(move |web_view, reason| {
            eprintln!("Code viewer's web process stopped: {reason:?}");
            let reloading = {
                let mut state = stopped_state.borrow_mut();
                let again = state
                    .stopped_at
                    .is_some_and(|at| at.elapsed() < CRASH_LOOP_WINDOW);
                state.stopped_at = Some(Instant::now());
                state.ready = false;
                state.queued.clear();
                state.stopped = again;
                !again
            };
            if reloading {
                reload(web_view, &stopped_state);
            }
            on_event(ViewerEvent::Stopped { reloading });
        });
        web_view.load_uri(INDEX);

        Self { web_view, state }
    }

    pub(super) fn widget(&self) -> gtk::Widget {
        self.web_view.clone().upcast()
    }

    pub(super) fn show_diff(&self, diff: &Value) -> Result<(), String> {
        let json = serde_json::to_string(diff).map_err(|error| error.to_string())?;
        if json.len() > MAX_DIFF_BYTES {
            return Err("diff viewer payload is too large".to_owned());
        }
        self.state.borrow_mut().preview = wants_preview(diff);
        self.enqueue(ViewerCommand::ShowDiff(json));
        Ok(())
    }

    pub(super) fn show_file(&self, file: &Value) -> Result<(), String> {
        let json = serde_json::to_string(file).map_err(|error| error.to_string())?;
        let limit = if file.get("dataUrl").is_some() {
            MAX_IMAGE_PAYLOAD_BYTES
        } else {
            MAX_DIFF_BYTES
        };
        if json.len() > limit {
            return Err("file viewer payload is too large".to_owned());
        }
        self.state.borrow_mut().preview = wants_preview(file);
        self.enqueue(ViewerCommand::ShowFile(json));
        Ok(())
    }

    /// Moves the cursor of the shown text to `line`, and `column` when given.
    pub(super) fn reveal(&self, line: u32, column: Option<u32>) {
        self.enqueue(ViewerCommand::Reveal { line, column });
    }

    pub(super) fn clear(&self) {
        self.enqueue(ViewerCommand::Clear);
    }

    pub(super) fn find(&self, query: &str, next: bool) {
        self.enqueue(ViewerCommand::Find {
            query: query.to_owned(),
            next,
        });
    }

    pub(super) fn copy(&self) {
        if self.state.borrow().preview {
            self.web_view.execute_editing_command("Copy");
        } else {
            self.enqueue(ViewerCommand::Copy);
        }
    }

    /// Shows the shown Markdown rendered, or its source.
    pub(super) fn set_presentation(&self, preview: bool) {
        self.state.borrow_mut().preview = preview;
        self.enqueue(ViewerCommand::SetPresentation { preview });
    }

    pub(super) fn move_to_change(&self, next: bool) {
        self.enqueue(if next {
            ViewerCommand::NextChange
        } else {
            ViewerCommand::PreviousChange
        });
    }

    pub(super) fn save_view_state(&self) {
        self.enqueue(ViewerCommand::SaveViewState);
    }

    /// Answers a `LookUp` event with locations, or with an `error` to show.
    pub(super) fn answer_lookup(&self, request_id: u32, answer: &Value) {
        self.enqueue(ViewerCommand::AnswerLookup {
            request_id,
            answer: answer.to_string(),
        });
    }

    /// Offers `sessions`, as `(id, name)`, for comments on selected text, with
    /// `preferred` chosen first. Nothing is offered when the list is empty.
    pub(super) fn set_comment_sessions(
        &self,
        sessions: &[(String, String)],
        preferred: Option<&str>,
    ) {
        let json = serde_json::json!({
            "sessions": sessions
                .iter()
                .map(|(id, name)| serde_json::json!({ "id": id, "name": name }))
                .collect::<Vec<_>>(),
            "preferred": preferred,
        })
        .to_string();
        let unchanged = matches!(
            &self.state.borrow().comment_sessions,
            Some(ViewerCommand::SetCommentSessions(sent)) if *sent == json
        );
        if !unchanged {
            self.enqueue(ViewerCommand::SetCommentSessions(json));
        }
    }

    /// Answers a `Comment` event: sent when `error` is empty.
    pub(super) fn answer_comment(&self, comment_id: u32, error: &str) {
        self.enqueue(ViewerCommand::AnswerComment {
            comment_id,
            error: error.to_owned(),
        });
    }

    pub(super) fn focus(&self) {
        self.web_view.grab_focus();
    }

    pub(super) fn set_appearance(&self, theme: &str, font_family: &str, font_size: u32) {
        self.enqueue(ViewerCommand::SetAppearance {
            theme: if theme == "dark" { "dark" } else { "light" }.to_owned(),
            font_family: font_family.to_owned(),
            font_size,
        });
    }

    fn enqueue(&self, command: ViewerCommand) {
        let stopped = {
            let mut state = self.state.borrow_mut();
            state.remember(&command);
            state.stopped
        };
        // Something new to show gives a stopped page another try.
        if stopped
            && matches!(
                command,
                ViewerCommand::ShowDiff(_) | ViewerCommand::ShowFile(_)
            )
        {
            reload(&self.web_view, &self.state);
            return;
        }
        if self.state.borrow().ready {
            call_command(&self.web_view, command);
            return;
        }
        let mut state = self.state.borrow_mut();
        match command {
            // Only the last shown document matters once the page is ready.
            ViewerCommand::ShowDiff(_) | ViewerCommand::ShowFile(_) => {
                state.queued.retain(|queued| {
                    !matches!(
                        queued,
                        ViewerCommand::ShowDiff(_) | ViewerCommand::ShowFile(_)
                    )
                });
                state.queued.push_back(command);
            }
            ViewerCommand::SetAppearance { .. } => {
                state
                    .queued
                    .retain(|queued| !matches!(queued, ViewerCommand::SetAppearance { .. }));
                state.queued.push_back(command);
            }
            ViewerCommand::SetPresentation { .. } => {
                state
                    .queued
                    .retain(|queued| !matches!(queued, ViewerCommand::SetPresentation { .. }));
                state.queued.push_back(command);
            }
            ViewerCommand::SetCommentSessions(_) => {
                state
                    .queued
                    .retain(|queued| !matches!(queued, ViewerCommand::SetCommentSessions(_)));
                state.queued.push_back(command);
            }
            ViewerCommand::Find { .. }
            | ViewerCommand::Copy
            | ViewerCommand::PreviousChange
            | ViewerCommand::NextChange
            | ViewerCommand::Reveal { .. }
            | ViewerCommand::SaveViewState
            | ViewerCommand::AnswerLookup { .. }
            | ViewerCommand::AnswerComment { .. }
            | ViewerCommand::Clear => state.queued.push_back(command),
        }
        while state.queued.len() > 16 {
            state.queued.pop_front();
        }
    }
}

/// Loads the page in a new web process and queues what the old one showed.
fn reload(web_view: &WebView, state: &RefCell<ViewerState>) {
    {
        let state = &mut *state.borrow_mut();
        state.ready = false;
        state.stopped = false;
        state.queued = [
            &state.appearance,
            &state.comment_sessions,
            &state.shown,
            &state.presentation,
        ]
        .into_iter()
        .flatten()
        .cloned()
        .collect();
    }
    web_view.load_uri(INDEX);
}

fn flush_queued(web_view: &WebView, state: &Rc<RefCell<ViewerState>>) {
    let commands = state.borrow_mut().queued.drain(..).collect::<Vec<_>>();
    for command in commands {
        call_command(web_view, command);
    }
}

fn call_command(web_view: &WebView, command: ViewerCommand) {
    let (body, arguments) = match command {
        ViewerCommand::ShowDiff(json) => (show_script("showDiff"), Some(document_args(json))),
        ViewerCommand::ShowFile(json) => (show_script("showFile"), Some(document_args(json))),
        ViewerCommand::Reveal { line, column } => {
            let args = glib::VariantDict::new(None);
            args.insert("line", line);
            args.insert("column", column.unwrap_or(0));
            (
                "window.agtkViewer.reveal(line, column)".to_owned(),
                Some(args.end()),
            )
        }
        ViewerCommand::Clear => ("window.agtkViewer.clear()".to_owned(), None),
        ViewerCommand::Find { query, next } => {
            let args = glib::VariantDict::new(None);
            args.insert("query", query);
            args.insert("next", next);
            (
                "window.agtkViewer.find(query, next)".to_owned(),
                Some(args.end()),
            )
        }
        ViewerCommand::Copy => ("window.agtkViewer.copySelection()".to_owned(), None),
        ViewerCommand::PreviousChange => ("window.agtkViewer.moveToChange(false)".to_owned(), None),
        ViewerCommand::NextChange => ("window.agtkViewer.moveToChange(true)".to_owned(), None),
        ViewerCommand::SaveViewState => ("window.agtkViewer.saveViewState()".to_owned(), None),
        ViewerCommand::SetPresentation { preview } => (
            format!(
                "window.agtkViewer.setPresentation('{}')",
                if preview { "preview" } else { "source" }
            ),
            None,
        ),
        ViewerCommand::SetAppearance {
            theme,
            font_family,
            font_size,
        } => {
            let args = glib::VariantDict::new(None);
            args.insert("theme", theme);
            args.insert("fontFamily", font_family);
            args.insert("fontSize", font_size);
            (
                "window.agtkViewer.setAppearance(theme, fontFamily, fontSize)".to_owned(),
                Some(args.end()),
            )
        }
        ViewerCommand::AnswerLookup { request_id, answer } => {
            let args = glib::VariantDict::new(None);
            args.insert("requestId", request_id);
            args.insert("answerJson", answer);
            (
                "window.agtkViewer.answerLookup(requestId, JSON.parse(answerJson))".to_owned(),
                Some(args.end()),
            )
        }
        ViewerCommand::SetCommentSessions(json) => {
            let args = glib::VariantDict::new(None);
            args.insert("sessionsJson", json);
            (
                "window.agtkViewer.setCommentSessions(JSON.parse(sessionsJson))".to_owned(),
                Some(args.end()),
            )
        }
        ViewerCommand::AnswerComment { comment_id, error } => {
            let args = glib::VariantDict::new(None);
            args.insert("commentId", comment_id);
            args.insert("error", error);
            (
                "window.agtkViewer.answerComment(commentId, error)".to_owned(),
                Some(args.end()),
            )
        }
    };
    web_view.call_async_javascript_function(
        &body,
        arguments.as_ref(),
        None,
        None,
        None::<&gio::Cancellable>,
        |result| {
            if let Err(error) = result {
                eprintln!("Code viewer call failed: {error}");
            }
        },
    );
}

fn wants_preview(document: &Value) -> bool {
    document.get("presentation").and_then(Value::as_str) == Some("preview")
}

fn document_args(json: String) -> glib::Variant {
    let args = glib::VariantDict::new(None);
    args.insert("documentJson", json);
    args.end()
}

/// Parses the document in the page so a bad payload reports an error event instead of failing silently.
fn show_script(function: &str) -> String {
    format!(
        "try {{ const data = JSON.parse(documentJson); window.agtkViewer.{function}(data); }} \
         catch (error) {{ let requestId = null; try {{ requestId = JSON.parse(documentJson).requestId; }} catch (_) {{}} \
         window.webkit?.messageHandlers?.agtk?.postMessage({{ type: 'error', requestId, message: String(error).slice(0, 2048) }}); }}"
    )
}

fn valid_event(event: &ViewerEvent) -> bool {
    match event {
        ViewerEvent::Ready { version } => {
            version.as_ref().is_none_or(|version| version.len() <= 64)
        }
        ViewerEvent::DiffRendered {
            request_id,
            tab_id,
            line_changes,
            character_changes,
        } => {
            !request_id.is_empty()
                && request_id.len() <= 128
                && !tab_id.is_empty()
                && tab_id.len() <= 128
                && *line_changes <= 50_000
                && *character_changes <= 100_000
        }
        ViewerEvent::FileRendered {
            request_id,
            tab_id,
            line_count,
        } => {
            !request_id.is_empty()
                && request_id.len() <= 128
                && !tab_id.is_empty()
                && tab_id.len() <= 128
                && *line_count <= 50_001
        }
        ViewerEvent::ViewState { tab_id, state } => {
            !tab_id.is_empty() && tab_id.len() <= 128 && state.to_string().len() <= 24 * 1024
        }
        ViewerEvent::ImageRendered {
            request_id,
            tab_id,
            width,
            height,
        } => {
            !request_id.is_empty()
                && request_id.len() <= 128
                && !tab_id.is_empty()
                && tab_id.len() <= 128
                && *width > 0
                && *height > 0
        }
        ViewerEvent::OpenLink { tab_id, href } => {
            !tab_id.is_empty() && tab_id.len() <= 128 && !href.is_empty() && href.len() <= 4096
        }
        ViewerEvent::LookUp {
            document_id,
            line,
            column,
            ..
        } => valid_document_id(document_id) && valid_position(*line, *column),
        ViewerEvent::Jump {
            document_id,
            path,
            line,
            column,
            from_line,
            from_column,
        } => {
            valid_document_id(document_id)
                && path
                    .as_ref()
                    .is_none_or(|path| path.starts_with('/') && path.len() <= 4096)
                && valid_position(*line, *column)
                && valid_position(*from_line, *from_column)
        }
        ViewerEvent::History {
            document_id,
            line,
            column,
            ..
        } => valid_document_id(document_id) && valid_position(*line, *column),
        ViewerEvent::Comment {
            document_id,
            session_id,
            comment,
            start_line,
            end_line,
            text,
            ..
        } => {
            valid_document_id(document_id)
                && !session_id.is_empty()
                && session_id.len() <= 128
                && !comment.trim().is_empty()
                && valid_position(*start_line, 1)
                && valid_position(*end_line, 1)
                && start_line <= end_line
                && text.as_ref().is_none_or(|text| !text.is_empty())
        }
        ViewerEvent::Error {
            request_id,
            message,
        } => {
            request_id
                .as_ref()
                .is_none_or(|request_id| request_id.len() <= 128)
                && message.len() <= 2048
        }
        // Only agtk reports this, never the page.
        ViewerEvent::Stopped { .. } => false,
    }
}

/// Document IDs are the request IDs of shown diffs and files.
fn valid_document_id(document_id: &str) -> bool {
    !document_id.is_empty() && document_id.len() <= 128
}

fn valid_position(line: u32, column: u32) -> bool {
    (1..=1_000_000).contains(&line) && (1..=1_000_000).contains(&column)
}

fn serve_asset(request: &webkit6::URISchemeRequest) {
    let uri = request.uri().map(|uri| uri.to_string()).unwrap_or_default();
    let path = request
        .path()
        .map(|path| path.to_string())
        .unwrap_or_default();
    let clean_path = path.trim_start_matches('/');
    let path_is_safe = !clean_path.is_empty()
        && clean_path.split('/').all(|part| {
            !part.is_empty()
                && part != "."
                && part != ".."
                && part.chars().all(|character| {
                    character.is_ascii_alphanumeric() || matches!(character, '_' | '-' | '.')
                })
        });
    let resource_path = format!("{RESOURCE_PREFIX}{clean_path}");
    let result = if uri.starts_with(ORIGIN) && path_is_safe {
        gio::resources_lookup_data(&resource_path, gio::ResourceLookupFlags::NONE)
    } else {
        Err(glib::Error::new(
            gio::IOErrorEnum::PermissionDenied,
            "resource path is outside the viewer bundle",
        ))
    };
    match result {
        Ok(bytes) => {
            let mime_type = mime_type(clean_path);
            let stream = gio::MemoryInputStream::from_bytes(&bytes);
            request.finish(&stream, bytes.len() as i64, Some(mime_type));
        }
        Err(error) => request.finish_error(&mut error.clone()),
    }
}

fn mime_type(path: &str) -> &'static str {
    match path.rsplit_once('.').map(|(_, extension)| extension) {
        Some("html") => "text/html; charset=utf-8",
        Some("js") => "text/javascript; charset=utf-8",
        Some("css") => "text/css; charset=utf-8",
        Some("json") => "application/json",
        Some("svg") => "image/svg+xml",
        Some("png") => "image/png",
        Some("ttf") => "font/ttf",
        Some("woff") => "font/woff",
        Some("woff2") => "font/woff2",
        Some("wasm") => "application/wasm",
        _ => "application/octet-stream",
    }
}
