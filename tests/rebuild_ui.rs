//! Requires a private Wayland compositor:
//! AGTK_TEST_DISPLAY=/path/to/socket cargo test --test rebuild_ui -- --ignored
use std::fs;
use std::os::unix::fs::PermissionsExt;
use std::process::{Child, Command, Stdio};
use std::thread;
use std::time::{Duration, Instant};

use agtk::control::{ControlClient, ResponseBody, decode_request};
use agtk::instance::{InstanceName, InstancePaths};
use serde_json::{Value, json};

struct App {
    child: Child,
    paths: InstancePaths,
}

impl App {
    fn request(&self, method: &str, params: Value) -> Option<Value> {
        let request = decode_request(
            &serde_json::to_vec(&json!({"version":1,"id":"test","method":method,"params":params}))
                .unwrap(),
        )
        .unwrap();
        match ControlClient::new(self.paths.control_socket())
            .send(&request)
            .ok()?
            .body
        {
            ResponseBody::Success(value) => Some(value),
            body => panic!("{method}: {body:?}"),
        }
    }

    fn activate_rebuild(&self) {
        let id = self.paths.name().application_id();
        let path = format!("/{}/window/1", id.replace('.', "/"));
        let output = Command::new("gdbus")
            .args([
                "call",
                "--session",
                "--dest",
                &id,
                "--object-path",
                &path,
                "--method",
                "org.gtk.Actions.Activate",
                "rebuild-and-restart",
                "[]",
                "{}",
            ])
            .output()
            .unwrap();
        assert!(
            output.status.success(),
            "{}",
            String::from_utf8_lossy(&output.stderr)
        );
    }
}

impl Drop for App {
    fn drop(&mut self) {
        if let Some(state) = self.request("app.get_state", json!({})) {
            for session in state["sessions"].as_array().unwrap() {
                self.request("session.close", json!({"sessionId":session["id"]}));
            }
        }
        let _ = self.child.kill();
        let _ = self.child.wait();
    }
}

fn wait(mut ready: impl FnMut() -> bool) {
    let deadline = Instant::now() + Duration::from_secs(15);
    while !ready() {
        assert!(
            Instant::now() < deadline,
            "timed out waiting for rebuild UI"
        );
        thread::sleep(Duration::from_millis(30));
    }
}

#[test]
#[ignore = "requires AGTK_TEST_DISPLAY private Wayland compositor"]
fn rebuild_failure_keeps_window_open_and_success_preserves_session_process() {
    let directory = tempfile::tempdir().unwrap();
    let root = directory.path();
    let cargo = root.join("cargo");
    // Gate the build so the test can verify the window stays responsive while it runs.
    fs::write(&cargo, format!("#!/bin/sh\ntouch '{}'\nwhile [ ! -f '{}' ]; do sleep 0.05; done\nif [ -f '{}' ]; then echo test-build-failed >&2; exit 1; fi\n", root.join("building").display(), root.join("continue").display(), root.join("fail").display())).unwrap();
    fs::set_permissions(&cargo, fs::Permissions::from_mode(0o755)).unwrap();
    let instance = InstanceName::parse(&format!("rebuild-test-{}", std::process::id())).unwrap();
    let paths = InstancePaths::new(instance, root, root);
    let display = std::env::var("AGTK_TEST_DISPLAY").expect("private display required");
    assert!(std::path::Path::new(&display).is_absolute());
    let child = Command::new(env!("CARGO_BIN_EXE_agtk"))
        .env("WAYLAND_DISPLAY", &display)
        .env("GDK_BACKEND", "wayland")
        .env("GSK_RENDERER", "cairo")
        .env("AGTK_INSTANCE", paths.name().as_str())
        .env("AGTK_RUNTIME_ROOT", root)
        .env("AGTK_STATE_ROOT", root)
        .env("CODEX_HOME", root.join("codex"))
        .env("CLAUDE_CONFIG_DIR", root.join("claude"))
        .env("AGTK_CLAUDE_BIN", "/nonexistent")
        .env("AGTK_CODEX_BIN", "/nonexistent")
        .env("CARGO", cargo)
        .stdin(Stdio::null())
        .stdout(Stdio::null())
        .stderr(Stdio::inherit())
        .spawn()
        .unwrap();
    let app = App { child, paths };
    wait(|| app.request("app.get_state", json!({})).is_some());
    let session = app.request("session.create", json!({"kind":"custom","command":"/bin/sh","args":["-c","stty -echo; exec /bin/sh -i"],"name":"survives rebuild","cwd":root})).unwrap();
    let id = session["id"].as_str().unwrap();
    app.request(
        "session.send_input",
        json!({"sessionId":id,"text":"printf '__PID_%s__\\n' \"$$\"", "appendEnter":true}),
    )
    .unwrap();
    let mut pid = String::new();
    wait(|| {
        let text = app
            .request("session.get_text", json!({"sessionId":id,"lines":200}))
            .unwrap();
        if let Some(line) = text["text"]
            .as_str()
            .unwrap()
            .lines()
            .find(|line| line.starts_with("__PID_"))
        {
            pid = line.to_owned();
            true
        } else {
            false
        }
    });
    let original_socket = fs::metadata(app.paths.control_socket())
        .unwrap()
        .modified()
        .unwrap();
    fs::write(root.join("fail"), "").unwrap();
    app.activate_rebuild();
    wait(|| root.join("building").exists());
    assert!(app.request("app.get_state", json!({})).is_some());
    fs::write(root.join("continue"), "").unwrap();
    // The action is enabled again only after the failure callback keeps the window open.
    wait(|| {
        let id = app.paths.name().application_id();
        let path = format!("/{}/window/1", id.replace('.', "/"));
        let output = Command::new("gdbus")
            .args([
                "call",
                "--session",
                "--dest",
                &id,
                "--object-path",
                &path,
                "--method",
                "org.gtk.Actions.Describe",
                "rebuild-and-restart",
            ])
            .output()
            .unwrap();
        String::from_utf8_lossy(&output.stdout).contains("((true,")
    });
    assert_eq!(
        fs::metadata(app.paths.control_socket())
            .unwrap()
            .modified()
            .unwrap(),
        original_socket
    );
    // Reactivating the app closes the failure dialog.
    assert!(
        Command::new(env!("CARGO_BIN_EXE_agtk"))
            .env("WAYLAND_DISPLAY", &display)
            .env("GDK_BACKEND", "wayland")
            .env("AGTK_INSTANCE", app.paths.name().as_str())
            .status()
            .unwrap()
            .success()
    );
    fs::remove_file(root.join("building")).unwrap();
    fs::remove_file(root.join("continue")).unwrap();
    fs::remove_file(root.join("fail")).unwrap();
    app.activate_rebuild();
    wait(|| root.join("building").exists());
    assert!(app.request("app.get_state", json!({})).is_some());
    fs::write(root.join("continue"), "").unwrap();
    wait(|| {
        fs::metadata(app.paths.control_socket())
            .is_ok_and(|metadata| metadata.modified().unwrap() != original_socket)
            && app
                .request("app.get_state", json!({}))
                .is_some_and(|state| {
                    state["sessions"]
                        .as_array()
                        .unwrap()
                        .iter()
                        .any(|session| session["id"] == id)
                })
    });
    app.request(
        "session.send_input",
        json!({"sessionId":id,"text":"printf '__STILL_%s__\\n' \"$$\"", "appendEnter":true}),
    )
    .unwrap();
    let expected = pid.replace("__PID_", "__STILL_");
    wait(|| {
        app.request("session.get_text", json!({"sessionId":id,"lines":200}))
            .is_some_and(|text| text["text"].as_str().unwrap().contains(&expected))
    });
}
