//! Run against a private compositor, never the user's display:
//! AGMUX_TEST_DISPLAY=/absolute/path/to/wayland-socket cargo test --test native_ui -- --ignored
use std::io::Write;
use std::os::unix::net::UnixStream;
use std::process::{Child, Command, Stdio};
use std::sync::atomic::{AtomicU64, Ordering};
use std::thread;
use std::time::{Duration, Instant};

use agmux_native::control::{ControlClient, ResponseBody, decode_request};
use agmux_native::instance::{InstanceName, InstancePaths};
use serde_json::{Value, json};

struct App {
    directory: tempfile::TempDir,
    paths: InstancePaths,
    child: Option<Child>,
}

static TEST_SEQUENCE: AtomicU64 = AtomicU64::new(0);

impl App {
    fn new() -> Self {
        let directory = tempfile::tempdir().unwrap();
        let sequence = TEST_SEQUENCE.fetch_add(1, Ordering::Relaxed);
        let name =
            InstanceName::parse(&format!("ui-test-{}-{sequence}", std::process::id())).unwrap();
        let paths = InstancePaths::new(name, directory.path(), directory.path());
        let mut app = Self {
            directory,
            paths,
            child: None,
        };
        app.start();
        app
    }

    fn start(&mut self) {
        let display = std::env::var("AGMUX_TEST_DISPLAY").expect("private display required");
        assert!(std::path::Path::new(&display).is_absolute());
        self.child = Some(
            Command::new(env!("CARGO_BIN_EXE_agmux-native"))
                .env("WAYLAND_DISPLAY", display)
                .env("GDK_BACKEND", "wayland")
                .env("GSK_RENDERER", "cairo")
                .env("AGMUX_INSTANCE", self.paths.name().as_str())
                .env("AGMUX_RUNTIME_ROOT", self.directory.path())
                .env("AGMUX_STATE_ROOT", self.directory.path())
                .stdin(Stdio::null())
                .stdout(Stdio::null())
                .spawn()
                .unwrap(),
        );
        let deadline = Instant::now() + Duration::from_secs(10);
        while Instant::now() < deadline {
            assert!(
                self.child.as_mut().unwrap().try_wait().unwrap().is_none(),
                "UI crashed"
            );
            let request = decode_request(
                br#"{"version":1,"id":"probe","method":"app.get_state","params":{}}"#,
            )
            .unwrap();
            if ControlClient::new(self.paths.control_socket())
                .send(&request)
                .is_ok()
            {
                return;
            }
            thread::sleep(Duration::from_millis(30));
        }
        panic!("UI did not start");
    }

    fn stop(&mut self) {
        if let Some(mut child) = self.child.take() {
            let _ = child.kill();
            let _ = child.wait();
        }
    }

    fn request(&self, method: &str, params: Value) -> Value {
        match self.request_body(method, params) {
            ResponseBody::Success(result) => result,
            body => panic!("{method}: {body:?}"),
        }
    }

    fn request_body(&self, method: &str, params: Value) -> ResponseBody {
        let request = decode_request(
            &serde_json::to_vec(&json!({"version":1,"id":"test","method":method,"params":params}))
                .unwrap(),
        )
        .unwrap();
        ControlClient::new(self.paths.control_socket())
            .send(&request)
            .unwrap()
            .body
    }

    fn wait_text(&self, id: &str, needle: &str) -> String {
        let deadline = Instant::now() + Duration::from_secs(5);
        while Instant::now() < deadline {
            let result = self.request("session.get_text", json!({"sessionId":id,"lines":200}));
            if result["text"].as_str().unwrap().contains(needle) {
                return result["text"].as_str().unwrap().into();
            }
            thread::sleep(Duration::from_millis(30));
        }
        panic!("terminal did not contain {needle}");
    }
}

impl Drop for App {
    fn drop(&mut self) {
        self.stop();
        if let Ok(entries) = std::fs::read_dir(self.paths.sessions_dir()) {
            for entry in entries.flatten() {
                if let Ok(mut socket) = UnixStream::connect(entry.path()) {
                    let _ = socket.set_read_timeout(Some(Duration::from_secs(1)));
                    let _ = agmux_native::session::receive_attachment(&socket);
                    let _ = socket.write_all(b"K");
                }
            }
        }
    }
}

#[test]
#[ignore = "requires AGMUX_TEST_DISPLAY private Wayland compositor"]
fn metadata_and_child_survive_ui_restart() {
    let mut app = App::new();
    let session = app.request("session.create", json!({"kind":"custom","command":"/bin/sh","args":["-c","stty -echo; printf '__AGMUX_PID_%s__\\n' \"$$\"; exec /bin/sh -i"],"name":"before rename","cwd":app.directory.path(),"projectRoot":app.directory.path()}));
    let id = session["id"].as_str().unwrap();
    app.request(
        "session.rename",
        json!({"sessionId":id,"name":"durable name"}),
    );
    let text = app.wait_text(id, "__AGMUX_PID_");
    let pid = text
        .lines()
        .find_map(|line| {
            line.trim()
                .strip_prefix("__AGMUX_PID_")?
                .strip_suffix("__")?
                .parse::<u32>()
                .ok()
        })
        .expect("rendered child PID");
    app.stop();
    app.start();
    let state = app.request("app.get_state", json!({}));
    let recovered = state["sessions"]
        .as_array()
        .unwrap()
        .iter()
        .find(|s| s["id"] == id)
        .unwrap();
    assert_eq!(recovered["name"], "durable name");
    assert_eq!(recovered["kind"], "custom");
    assert_eq!(recovered["cwd"], app.directory.path().to_str().unwrap());
    assert_eq!(state["selectedSessionId"], id);
    app.request(
        "session.send_input",
        json!({"sessionId":id,"text":"printf '__AGMUX_PID_%s__\\n' \"$$\"","appendEnter":true}),
    );
    app.wait_text(id, &format!("__AGMUX_PID_{pid}__"));
    app.request("session.close", json!({"sessionId":id}));
    assert!(
        app.request("app.get_state", json!({}))["sessions"]
            .as_array()
            .unwrap()
            .is_empty()
    );
}

#[test]
#[ignore = "requires AGMUX_TEST_DISPLAY private Wayland compositor"]
fn workspace_inspection_preserves_two_pane_tui_structure() {
    let app = App::new();
    let inspection = app.request("ui.inspect", json!({}));
    let root = inspection["root"].as_object().unwrap();
    let children = root["children"].as_array().unwrap();
    let ids = children
        .iter()
        .filter_map(|node| node["id"].as_str())
        .collect::<Vec<_>>();
    assert_eq!(ids, ["top-bar", "sidebar", "terminal-pane", "status-bar"]);
    let sidebar = children
        .iter()
        .find(|node| node["id"] == "sidebar")
        .unwrap();
    assert_eq!(sidebar["role"], "complementary");
    assert_eq!(sidebar["children"][0]["id"], "sessions");
    assert_eq!(children[2]["role"], "main");
    assert!(children[3]["label"].as_str().unwrap().contains("new"));
    let top_bar_ids = children[0]["children"]
        .as_array()
        .unwrap()
        .iter()
        .filter_map(|node| node["id"].as_str())
        .collect::<Vec<_>>();
    assert_eq!(
        top_bar_ids,
        ["new-shell", "shortcuts", "appearance", "history"]
    );
}

#[test]
#[ignore = "requires AGMUX_TEST_DISPLAY private Wayland compositor"]
fn terminal_appearance_updates_live_and_survives_ui_restart() {
    let mut app = App::new();
    let session = app.request(
        "session.create",
        json!({
            "kind":"custom",
            "command":"/bin/sh",
            "args":["-c", "printf '\\033[31m__THEME_PROBE__\\033[0m\\n'; exec /bin/sh -i"],
            "name":"theme probe",
            "cwd":app.directory.path()
        }),
    );
    let id = session["id"].as_str().unwrap();
    app.wait_text(id, "__THEME_PROBE__");
    let before = app.request("ui.capture", json!({}));

    let appearance = app.request(
        "appearance.set",
        json!({"theme":"tokyo-night","followSystem":false,"font":"Monospace 13"}),
    );
    assert_eq!(appearance["theme"], "tokyo-night");
    assert_eq!(appearance["effectiveTheme"], "tokyo-night");
    assert_eq!(appearance["font"], "Monospace 13");
    let after = app.request("ui.capture", json!({}));
    assert_ne!(before["sha256"], after["sha256"]);

    app.stop();
    app.start();
    let state = app.request("app.get_state", json!({}));
    assert_eq!(state["appearance"]["theme"], "tokyo-night");
    assert_eq!(state["appearance"]["effectiveTheme"], "tokyo-night");
    assert_eq!(state["appearance"]["followSystem"], false);
    assert_eq!(state["appearance"]["font"], "Monospace 13");
    app.request("session.close", json!({"sessionId":id}));
}

#[test]
#[ignore = "requires AGMUX_TEST_DISPLAY private Wayland compositor"]
fn shortcut_overrides_are_validated_and_survive_ui_restart() {
    let mut app = App::new();
    let result = app.request(
        "shortcut.set",
        json!({"action":"toggle-sidebar","accelerator":"<Alt>b"}),
    );
    let shortcut = result["shortcuts"]
        .as_array()
        .unwrap()
        .iter()
        .find(|shortcut| shortcut["action"] == "toggle-sidebar")
        .unwrap();
    assert_eq!(shortcut["accelerator"], "<Alt>b");

    let duplicate = app.request_body(
        "shortcut.set",
        json!({"action":"new-shell","accelerator":"<Alt>b"}),
    );
    assert!(matches!(
        duplicate,
        ResponseBody::Failure(ref error) if error.code == agmux_native::control::ErrorCode::InvalidParams
    ));
    let unmodified = app.request_body(
        "shortcut.set",
        json!({"action":"new-shell","accelerator":"n"}),
    );
    assert!(matches!(
        unmodified,
        ResponseBody::Failure(ref error) if error.code == agmux_native::control::ErrorCode::InvalidParams
    ));

    app.stop();
    app.start();
    let state = app.request("app.get_state", json!({}));
    let shortcut = state["shortcuts"]
        .as_array()
        .unwrap()
        .iter()
        .find(|shortcut| shortcut["action"] == "toggle-sidebar")
        .unwrap();
    assert_eq!(shortcut["accelerator"], "<Alt>b");
    app.request(
        "shortcut.set",
        json!({"action":"toggle-sidebar","reset":true}),
    );
}
