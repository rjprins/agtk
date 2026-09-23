use std::cell::RefCell;
use std::collections::VecDeque;
use std::path::PathBuf;
use std::rc::Rc;
use std::sync::Once;

use adw::prelude::*;
use serde::Deserialize;
use serde_json::Value;
use webkit6::prelude::*;
use webkit6::{
    NavigationPolicyDecision, NetworkSession, PolicyDecisionType, UserContentManager, WebContext,
    WebView,
};

const SCHEME: &str = "agmux-diff";
const ORIGIN: &str = "agmux-diff://viewer/";
const RESOURCE_PREFIX: &str = "/nl/rutger/AgmuxNative/viewer/";
const MESSAGE_HANDLER: &str = "agmux";
const MAX_EVENT_BYTES: usize = 32 * 1024;
const MAX_DIFF_BYTES: usize = 4 * 1024 * 1024 + MAX_EVENT_BYTES;

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
    ViewState {
        tab_id: String,
        state: Value,
    },
    Error {
        #[serde(default)]
        request_id: Option<String>,
        message: String,
    },
}

struct ViewerState {
    ready: bool,
    queued: VecDeque<ViewerCommand>,
}

enum ViewerCommand {
    ShowDiff(String),
    Clear,
    Find {
        query: String,
        next: bool,
    },
    PreviousChange,
    NextChange,
    SaveViewState,
    SetAppearance {
        theme: String,
        font_family: String,
        font_size: u32,
    },
}

pub(super) struct DiffViewer {
    web_view: WebView,
    state: Rc<RefCell<ViewerState>>,
}

impl DiffViewer {
    pub(super) fn new(on_event: impl Fn(ViewerEvent) + 'static) -> Self {
        REGISTER_RESOURCES.call_once(|| {
            gio::resources_register_include!("viewer.gresource")
                .expect("embedded Changes viewer resources are invalid");
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
                eprintln!("Changes viewer sent an oversized event");
                return;
            }
            let event = match serde_json::from_str::<ViewerEvent>(json.as_str()) {
                Ok(event) => event,
                Err(error) => {
                    eprintln!("Changes viewer sent an invalid event: {error}");
                    return;
                }
            };
            if !valid_event(&event) {
                eprintln!("Changes viewer sent an out-of-bounds event");
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
        web_view.load_uri("agmux-diff://viewer/index.html");

        Self { web_view, state }
    }

    pub(super) fn widget(&self) -> gtk::Widget {
        self.web_view.clone().upcast()
    }

    pub(super) fn is_ready(&self) -> bool {
        self.state.borrow().ready
    }

    pub(super) fn show_diff(&self, diff: &Value) -> Result<(), String> {
        let json = serde_json::to_string(diff).map_err(|error| error.to_string())?;
        if json.len() > MAX_DIFF_BYTES {
            return Err("diff viewer payload is too large".to_owned());
        }
        self.enqueue(ViewerCommand::ShowDiff(json));
        Ok(())
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

    pub(super) fn snapshot_to_png(
        &self,
        path: PathBuf,
        completed: impl FnOnce(Result<(), String>) + 'static,
    ) {
        self.web_view.snapshot(
            webkit6::SnapshotRegion::Visible,
            webkit6::SnapshotOptions::NONE,
            None::<&gio::Cancellable>,
            move |result| {
                let result = result
                    .map_err(|error| error.to_string())
                    .and_then(|texture| {
                        texture.save_to_png(path).map_err(|error| error.to_string())
                    });
                completed(result);
            },
        );
    }

    fn enqueue(&self, command: ViewerCommand) {
        if self.state.borrow().ready {
            call_command(&self.web_view, command);
            return;
        }
        let mut state = self.state.borrow_mut();
        match command {
            ViewerCommand::ShowDiff(_) => {
                state
                    .queued
                    .retain(|queued| !matches!(queued, ViewerCommand::ShowDiff(_)));
                state.queued.push_back(command);
            }
            ViewerCommand::SetAppearance { .. } => {
                state
                    .queued
                    .retain(|queued| !matches!(queued, ViewerCommand::SetAppearance { .. }));
                state.queued.push_back(command);
            }
            ViewerCommand::Find { .. }
            | ViewerCommand::PreviousChange
            | ViewerCommand::NextChange
            | ViewerCommand::SaveViewState => state.queued.push_back(command),
            ViewerCommand::Clear => state.queued.push_back(command),
        }
        while state.queued.len() > 16 {
            state.queued.pop_front();
        }
    }
}

fn flush_queued(web_view: &WebView, state: &Rc<RefCell<ViewerState>>) {
    let commands = state.borrow_mut().queued.drain(..).collect::<Vec<_>>();
    for command in commands {
        call_command(web_view, command);
    }
}

fn call_command(web_view: &WebView, command: ViewerCommand) {
    let (body, arguments) = match command {
        ViewerCommand::ShowDiff(json) => {
            let args = glib::VariantDict::new(None);
            args.insert("diffJson", json);
            (
                "window.agmuxDiffViewer.showDiff(JSON.parse(diffJson))",
                Some(args.end()),
            )
        }
        ViewerCommand::Clear => ("window.agmuxDiffViewer.clearDiff()", None),
        ViewerCommand::Find { query, next } => {
            let args = glib::VariantDict::new(None);
            args.insert("query", query);
            args.insert("next", next);
            ("window.agmuxDiffViewer.find(query, next)", Some(args.end()))
        }
        ViewerCommand::PreviousChange => ("window.agmuxDiffViewer.moveToChange(false)", None),
        ViewerCommand::NextChange => ("window.agmuxDiffViewer.moveToChange(true)", None),
        ViewerCommand::SaveViewState => ("window.agmuxDiffViewer.saveViewState()", None),
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
                "window.agmuxDiffViewer.setAppearance(theme, fontFamily, fontSize)",
                Some(args.end()),
            )
        }
    };
    web_view.call_async_javascript_function(
        body,
        arguments.as_ref(),
        None,
        None,
        None::<&gio::Cancellable>,
        |result| {
            if let Err(error) = result {
                eprintln!("Changes viewer call failed: {error}");
            }
        },
    );
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
        ViewerEvent::ViewState { tab_id, state } => {
            !tab_id.is_empty() && tab_id.len() <= 128 && state.to_string().len() <= 24 * 1024
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
    }
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
