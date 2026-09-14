//! Run against a private compositor, never the user's display:
//! AGMUX_TEST_DISPLAY=/absolute/path/to/wayland-socket cargo test --test native_ui -- --ignored
use std::io::Write;
use std::os::unix::net::UnixStream;
use std::process::{Child, Command, Stdio};
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

impl App {
    fn new() -> Self {
        let directory = tempfile::tempdir().unwrap();
        let name = InstanceName::parse(&format!("ui-test-{}", std::process::id())).unwrap();
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
        let request = decode_request(
            &serde_json::to_vec(&json!({"version":1,"id":"test","method":method,"params":params}))
                .unwrap(),
        )
        .unwrap();
        match ControlClient::new(self.paths.control_socket())
            .send(&request)
            .unwrap()
            .body
        {
            ResponseBody::Success(result) => result,
            body => panic!("{method}: {body:?}"),
        }
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
