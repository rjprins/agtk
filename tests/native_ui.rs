//! Run against a private compositor, never the user's display:
//! AGTK_TEST_DISPLAY=/absolute/path/to/wayland-socket cargo test --test native_ui -- --ignored
use std::io::Write;
use std::os::unix::fs::PermissionsExt;
use std::os::unix::net::UnixStream;
use std::process::{Child, Command, Stdio};
use std::sync::atomic::{AtomicU64, Ordering};
use std::sync::{Mutex, MutexGuard};
use std::thread;
use std::time::{Duration, Instant};

use agtk::control::{ControlClient, ErrorCode, ResponseBody, decode_request};
use agtk::instance::{InstanceName, InstancePaths};
use agtk::persist::Store;
use serde_json::{Value, json};

struct App {
    _display_guard: MutexGuard<'static, ()>,
    directory: tempfile::TempDir,
    paths: InstancePaths,
    child: Option<Child>,
}

static TEST_SEQUENCE: AtomicU64 = AtomicU64::new(0);
static PRIVATE_DISPLAY_LOCK: Mutex<()> = Mutex::new(());

impl App {
    fn new() -> Self {
        let display_guard = PRIVATE_DISPLAY_LOCK
            .lock()
            .unwrap_or_else(std::sync::PoisonError::into_inner);
        let directory = tempfile::tempdir().unwrap();
        let fake_agent = directory.path().join("fake-agent");
        std::fs::write(
            &fake_agent,
            "#!/bin/sh\nprintf '__RESTORE_ARGS_%s_%s__\\n' \"$1\" \"$2\"\nwhile IFS= read -r line; do printf '__INPUT_%s__\\n' \"$line\"; done\n",
        )
        .unwrap();
        std::fs::set_permissions(&fake_agent, std::fs::Permissions::from_mode(0o700)).unwrap();
        let fake_emacs = directory.path().join("fake-emacsclient");
        std::fs::write(
            &fake_emacs,
            "#!/bin/sh\nprintf '%s\\n' \"$@\" > \"$AGTK_EMACS_ARGS_FILE\"\n",
        )
        .unwrap();
        std::fs::set_permissions(&fake_emacs, std::fs::Permissions::from_mode(0o700)).unwrap();
        let fake_az = directory.path().join("fake-az");
        std::fs::write(
            &fake_az,
            "#!/bin/sh\n[ -e \"$AGTK_AZURE_PRS_FILE.slow\" ] && sleep 10\ncase \"$1 $2 $3\" in\n  'account show --query') printf '%s\\n' 'reviewer@example.com' ;;\n  'repos pr list') cat \"$AGTK_AZURE_PRS_FILE\" ;;\n  'repos pr work-item') cat \"$AGTK_AZURE_PBIS_FILE\" ;;\n  'devops invoke --org') cat \"$AGTK_AZURE_THREADS_FILE\" ;;\n  *) printf '%s\\n' 'unexpected az command' >&2; exit 2 ;;\nesac\n",
        )
        .unwrap();
        std::fs::set_permissions(&fake_az, std::fs::Permissions::from_mode(0o700)).unwrap();
        std::fs::write(
            directory.path().join("gitconfig"),
            format!(
                "[agtk]\n\tworktreeTemplate = {}/worktrees/{{repo-name}}/{{branch}}\n",
                directory.path().canonicalize().unwrap().display()
            ),
        )
        .unwrap();
        std::fs::write(directory.path().join("azure-prs.json"), "[]").unwrap();
        std::fs::write(directory.path().join("azure-pbis.json"), "[]").unwrap();
        std::fs::write(
            directory.path().join("azure-threads.json"),
            r#"{"value":[]}"#,
        )
        .unwrap();
        let sequence = TEST_SEQUENCE.fetch_add(1, Ordering::Relaxed);
        let name =
            InstanceName::parse(&format!("ui-test-{}-{sequence}", std::process::id())).unwrap();
        let paths = InstancePaths::new(name, directory.path(), directory.path());
        let mut app = Self {
            _display_guard: display_guard,
            directory,
            paths,
            child: None,
        };
        app.start();
        app
    }

    fn start(&mut self) {
        let display = std::env::var("AGTK_TEST_DISPLAY").expect("private display required");
        assert!(std::path::Path::new(&display).is_absolute());
        self.child = Some(self.command().spawn().unwrap());
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

    fn command(&self) -> Command {
        let display = std::env::var("AGTK_TEST_DISPLAY").expect("private display required");
        assert!(std::path::Path::new(&display).is_absolute());
        let mut command = Command::new(env!("CARGO_BIN_EXE_agtk"));
        command
            .env("WAYLAND_DISPLAY", display)
            .env("GDK_BACKEND", "wayland")
            .env("GSK_RENDERER", "cairo")
            .env("AGTK_INSTANCE", self.paths.name().as_str())
            .env("AGTK_RUNTIME_ROOT", self.directory.path())
            .env("AGTK_STATE_ROOT", self.directory.path())
            .env(
                "PATH",
                std::env::join_paths(std::iter::once(self.directory.path().join("bin")).chain(
                    std::env::split_paths(&std::env::var_os("PATH").unwrap_or_default()),
                ))
                .unwrap(),
            )
            // Worktrees land in the test directory instead of ~/worktrees.
            .env("GIT_CONFIG_GLOBAL", self.directory.path().join("gitconfig"))
            .env("CLAUDE_CONFIG_DIR", self.directory.path().join("claude"))
            .env("CODEX_HOME", self.directory.path().join("codex"))
            .env("AGTK_CODEX_BIN", self.directory.path().join("fake-agent"))
            .env("AGTK_CLAUDE_BIN", self.directory.path().join("fake-agent"))
            .env(
                "AGTK_EMACSCLIENT",
                self.directory.path().join("fake-emacsclient"),
            )
            .env(
                "AGTK_EMACS_ARGS_FILE",
                self.directory.path().join("emacs-args"),
            )
            .env("AGTK_AZURE_BIN", self.directory.path().join("fake-az"))
            .env(
                "AGTK_AZURE_PRS_FILE",
                self.directory.path().join("azure-prs.json"),
            )
            .env(
                "AGTK_AZURE_THREADS_FILE",
                self.directory.path().join("azure-threads.json"),
            )
            .env(
                "AGTK_AZURE_PBIS_FILE",
                self.directory.path().join("azure-pbis.json"),
            )
            .stdin(Stdio::null())
            .stdout(Stdio::null())
            .stderr(Stdio::null());
        command
    }

    fn activate_existing(&self) {
        let mut child = self.command().spawn().unwrap();
        let deadline = Instant::now() + Duration::from_secs(10);
        while Instant::now() < deadline {
            if child.try_wait().unwrap().is_some() {
                return;
            }
            thread::sleep(Duration::from_millis(30));
        }
        let _ = child.kill();
        let _ = child.wait();
        panic!("UI did not start");
    }

    fn stop(&mut self) {
        if let Some(mut child) = self.child.take() {
            let _ = child.kill();
            let _ = child.wait();
        }
    }

    /// Ends every session host, as a reboot would, and waits for their sockets to go.
    fn kill_hosts(&self) {
        let sessions = self.paths.sessions_dir();
        for entry in std::fs::read_dir(&sessions).unwrap().flatten() {
            if let Ok(mut socket) = UnixStream::connect(entry.path()) {
                let _ = socket.set_read_timeout(Some(Duration::from_secs(1)));
                let _ = agtk::session::receive_attachment(&socket);
                let _ = socket.write_all(b"K");
            }
        }
        let deadline = Instant::now() + Duration::from_secs(5);
        while std::fs::read_dir(&sessions).unwrap().flatten().count() > 0 {
            assert!(Instant::now() < deadline, "session hosts did not stop");
            thread::sleep(Duration::from_millis(30));
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

    fn activate_action(&self, action: &str) {
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
                action,
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
                    let _ = agtk::session::receive_attachment(&socket);
                    let _ = socket.write_all(b"K");
                }
            }
        }
    }
}

#[test]
#[ignore = "requires AGTK_TEST_DISPLAY private Wayland compositor"]
fn controlled_launches_fill_an_empty_view_but_never_take_the_selection() {
    let app = App::new();
    let first = app.request(
        "session.create",
        json!({"kind":"shell","cwd":app.directory.path()}),
    );
    let first_id = first["id"].as_str().unwrap();
    let state = app.request("app.get_state", json!({}));
    assert_eq!(state["selectedSessionId"], first_id);
    let second = app.request(
        "session.create",
        json!({"kind":"shell","cwd":app.directory.path()}),
    );
    let second_id = second["id"].as_str().unwrap();
    let state = app.request("app.get_state", json!({}));
    assert_eq!(state["selectedSessionId"], first_id);
    let order = state["sessions"]
        .as_array()
        .unwrap()
        .iter()
        .map(|s| s["id"].as_str().unwrap().to_owned())
        .collect::<Vec<_>>();
    assert_eq!(order, vec![first_id, second_id]);
}

#[test]
#[ignore = "requires AGTK_TEST_DISPLAY private Wayland compositor"]
fn metadata_and_child_survive_ui_restart() {
    let mut app = App::new();
    let session = app.request("session.create", json!({"kind":"custom","command":"/bin/sh","args":["-c","stty -echo; printf '__AGTK_PID_%s__\\n' \"$$\"; exec /bin/sh -i"],"name":"before rename","cwd":app.directory.path(),"projectRoot":app.directory.path()}));
    let id = session["id"].as_str().unwrap();
    app.request(
        "session.rename",
        json!({"sessionId":id,"name":"durable name"}),
    );
    let text = app.wait_text(id, "__AGTK_PID_");
    let pid = text
        .lines()
        .find_map(|line| {
            line.trim()
                .strip_prefix("__AGTK_PID_")?
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
        json!({"sessionId":id,"text":"printf '__AGTK_PID_%s__\\n' \"$$\"","appendEnter":true}),
    );
    app.wait_text(id, &format!("__AGTK_PID_{pid}__"));
    app.request("session.close", json!({"sessionId":id}));
    assert!(
        app.request("app.get_state", json!({}))["sessions"]
            .as_array()
            .unwrap()
            .is_empty()
    );
}

#[test]
#[ignore = "requires AGTK_TEST_DISPLAY private Wayland compositor"]
fn workspace_inspection_preserves_two_pane_tui_structure() {
    let app = App::new();
    let inspection = app.request("ui.inspect", json!({}));
    let root = inspection["root"].as_object().unwrap();
    assert_eq!(root["isVisible"], true);
    let children = root["children"].as_array().unwrap();
    let surface_anchors = children[0]["children"]
        .as_array()
        .unwrap()
        .iter()
        .find(|node| node["id"] == "surface-anchors")
        .unwrap();
    let pull_requests = surface_anchors["children"]
        .as_array()
        .unwrap()
        .iter()
        .find(|node| node["id"] == "pull-requests")
        .unwrap();
    assert_eq!(pull_requests["isSelected"], false);
    let ids = children
        .iter()
        .filter_map(|node| node["id"].as_str())
        .collect::<Vec<_>>();
    assert_eq!(ids, ["sidebar", "terminal-pane", "status-bar"]);
    let sidebar = children
        .iter()
        .find(|node| node["id"] == "sidebar")
        .unwrap();
    assert_eq!(sidebar["role"], "complementary");
    assert_eq!(sidebar["children"][0]["id"], "brand");
    assert_eq!(sidebar["children"][1]["id"], "sessions");
    assert_eq!(sidebar["children"][2]["id"], "sidebar-controls");
    assert_eq!(children[1]["role"], "main");
    let sidebar_control_ids = sidebar["children"][2]["children"]
        .as_array()
        .unwrap()
        .iter()
        .filter_map(|node| node["id"].as_str())
        .collect::<Vec<_>>();
    assert_eq!(
        sidebar_control_ids,
        ["shortcuts", "appearance", "launch", "sidebar-toggle"]
    );
    let context_ids = children[1]["children"][0]["children"]
        .as_array()
        .unwrap()
        .iter()
        .filter_map(|node| node["id"].as_str())
        .collect::<Vec<_>>();
    assert_eq!(
        context_ids,
        ["branch-review", "magit", "context-input", "pr-context"]
    );
    let context_input_ids = children[1]["children"][0]["children"][2]["children"]
        .as_array()
        .unwrap()
        .iter()
        .filter_map(|node| node["id"].as_str())
        .collect::<Vec<_>>();
    assert_eq!(context_input_ids, ["last-input", "history"]);
    assert_eq!(
        app.request("ui.show", json!({"surface":"launch"}))["shown"],
        true
    );
    let inspection = app.request("ui.inspect", json!({}));
    let launch = inspection["root"]["children"]
        .as_array()
        .unwrap()
        .iter()
        .find(|node| node["id"] == "sidebar")
        .unwrap()["children"]
        .as_array()
        .unwrap()
        .iter()
        .find(|node| node["id"] == "sidebar-controls")
        .unwrap()["children"]
        .as_array()
        .unwrap()
        .iter()
        .find(|node| node["id"] == "launch")
        .unwrap();
    let launch_child_ids = launch["children"]
        .as_array()
        .unwrap()
        .iter()
        .filter_map(|node| node["id"].as_str())
        .collect::<Vec<_>>();
    assert_eq!(
        launch_child_ids,
        [
            "launch-existing-worktree",
            "launch-project-input",
            "launch-worktree-input",
            "launch-new-worktree",
            "launch-agent",
            "launch-branch",
            "launch-base-branch",
            "launch-cancel",
            "launch-submit"
        ]
    );
    let launch_child = |id: &str| {
        launch["children"]
            .as_array()
            .unwrap()
            .iter()
            .find(|node| node["id"] == id)
            .unwrap()
    };
    assert_eq!(launch_child("launch-existing-worktree")["isVisible"], true);
    assert_eq!(launch_child("launch-existing-worktree")["isSelected"], true);
    assert_eq!(launch_child("launch-worktree-input")["isEnabled"], true);
    assert_eq!(launch_child("launch-new-worktree")["isVisible"], true);
    assert_eq!(launch_child("launch-new-worktree")["isEnabled"], true);
    assert_eq!(launch_child("launch-branch")["isVisible"], true);
    assert_eq!(launch_child("launch-branch")["isEnabled"], false);
    assert_eq!(launch_child("launch-base-branch")["isVisible"], true);
    assert_eq!(launch_child("launch-base-branch")["isEnabled"], false);
    thread::sleep(Duration::from_millis(50));
    // The capture shows the open dialog window.
    let transient = app.request("ui.capture", json!({}));
    assert!(transient["path"].as_str().is_some());
}

#[test]
#[ignore = "requires AGTK_TEST_DISPLAY private Wayland compositor"]
fn reactivating_an_existing_app_presents_the_main_window_not_a_modal() {
    let app = App::new();
    let project = app.directory.path().join("reactivation-project");
    std::fs::create_dir(&project).unwrap();
    let session = app.request(
        "session.create",
        json!({
            "kind":"custom",
            "command":"/bin/sh",
            "args":["-c","exec sleep 30"],
            "cwd":project,
            "projectRoot":project
        }),
    );
    let id = session["id"].as_str().unwrap();
    assert_eq!(
        app.request("ui.show", json!({"surface":"pull-requests"}))["shown"],
        true
    );
    let inspection = app.request("ui.inspect", json!({}));
    assert!(
        inspection["root"]["children"]
            .as_array()
            .unwrap()
            .iter()
            .flat_map(|child| child["children"].as_array().into_iter().flatten())
            .flat_map(|child| child["children"].as_array().into_iter().flatten())
            .any(|node| node["id"] == "pull-requests" && node["isSelected"] == true)
    );

    app.activate_existing();

    let inspection = app.request("ui.inspect", json!({}));
    assert!(
        inspection["root"]["children"]
            .as_array()
            .unwrap()
            .iter()
            .flat_map(|child| child["children"].as_array().into_iter().flatten())
            .flat_map(|child| child["children"].as_array().into_iter().flatten())
            .any(|node| node["id"] == "pull-requests" && node["isSelected"] == false)
    );
    app.request("session.close", json!({"sessionId":id}));
}

#[test]
#[ignore = "requires AGTK_TEST_DISPLAY private Wayland compositor"]
fn sidebar_width_is_restored_on_ui_restart() {
    let mut app = App::new();
    app.stop();
    let store = Store::open(&app.paths.database()).unwrap();
    store.set_preference("sidebarWidth", &json!(480)).unwrap();
    app.start();

    let inspection = app.request("ui.inspect", json!({}));
    let sidebar_width = inspection["root"]["children"]
        .as_array()
        .unwrap()
        .iter()
        .find(|node| node["id"] == "sidebar")
        .unwrap()["bounds"]["width"]
        .as_f64()
        .unwrap();
    assert_eq!(sidebar_width, 480.0);
}

#[test]
#[ignore = "requires AGTK_TEST_DISPLAY private Wayland compositor"]
fn window_size_is_restored_on_ui_restart() {
    let mut app = App::new();
    app.stop();
    let store = Store::open(&app.paths.database()).unwrap();
    store
        .set_preference("windowSize", &json!({"width": 1300, "height": 700}))
        .unwrap();
    app.start();

    let inspection = app.request("ui.inspect", json!({}));
    let bounds = &inspection["root"]["bounds"];
    assert_eq!(
        (bounds["width"].as_f64(), bounds["height"].as_f64()),
        (Some(1300.0), Some(700.0))
    );
}

#[test]
#[ignore = "requires AGTK_TEST_DISPLAY private Wayland compositor"]
fn status_bar_follows_the_terminal_and_selected_conversation() {
    fn find<'a>(node: &'a Value, id: &str) -> Option<&'a Value> {
        if node["id"] == id {
            return Some(node);
        }
        node["children"]
            .as_array()?
            .iter()
            .find_map(|child| find(child, id))
    }
    let mut app = App::new();
    let session = app.request(
        "session.create",
        json!({"kind":"codex","cwd":app.directory.path(),"name":"usage test"}),
    );
    let id = session["id"].as_str().unwrap();
    let logs = app.directory.path().join("codex/sessions/2026/10/06");
    std::fs::create_dir_all(&logs).unwrap();
    std::fs::write(
        logs.join("rollout-usage-test.jsonl"),
        json!({"type":"event_msg","payload":{"type":"token_count","info":{
        "total_token_usage":{"input_tokens":1000,"output_tokens":200},
        "last_token_usage":{"total_tokens":250},"model_context_window":1000}}})
        .to_string()
            + "\n",
    )
    .unwrap();
    app.request(
        "session.set_state",
        json!({"sessionId":id,"state":"idle","conversationId":"usage-test"}),
    );
    app.request("session.select", json!({"sessionId":id}));
    for width in [1200, 800] {
        app.stop();
        Store::open(&app.paths.database())
            .unwrap()
            .set_preference("windowSize", &json!({"width":width,"height":700}))
            .unwrap();
        app.start();
        let deadline = Instant::now() + Duration::from_secs(10);
        loop {
            let inspection = app.request("ui.inspect", json!({}));
            let root = &inspection["root"];
            let session = find(root, "session-usage").unwrap();
            if session["label"].as_str().unwrap().contains("1.2k tokens") {
                assert!(session["label"].as_str().unwrap().contains("75% left"));
                let bounds = |name| find(root, name).unwrap()["bounds"].clone();
                let terminal = bounds("terminal-pane");
                let account = bounds("account-usage");
                let system = bounds("system-usage");
                let session = bounds("session-usage");
                let center =
                    |b: &Value| b["x"].as_f64().unwrap() + b["width"].as_f64().unwrap() / 2.0;
                assert!(
                    (center(&terminal) - center(&session)).abs() <= 2.0,
                    "{terminal} {session}"
                );
                assert!(
                    account["x"].as_f64().unwrap() + account["width"].as_f64().unwrap()
                        <= session["x"].as_f64().unwrap()
                );
                assert!(
                    session["x"].as_f64().unwrap() + session["width"].as_f64().unwrap()
                        <= system["x"].as_f64().unwrap()
                );
                break;
            }
            assert!(Instant::now() < deadline, "usage did not appear: {session}");
            thread::sleep(Duration::from_millis(50));
        }
    }
    let shell = app.request(
        "session.create",
        json!({"kind":"shell","cwd":app.directory.path()}),
    );
    app.request("session.select", json!({"sessionId":shell["id"]}));
    let inspection = app.request("ui.inspect", json!({}));
    assert!(
        !find(&inspection["root"], "session-usage").unwrap()["label"]
            .as_str()
            .unwrap()
            .contains("1.2k")
    );
}

#[test]
#[ignore = "requires AGTK_TEST_DISPLAY private Wayland compositor"]
fn a_narrow_window_keeps_the_stored_sidebar_width() {
    let mut app = App::new();
    app.stop();
    let store = Store::open(&app.paths.database()).unwrap();
    store.set_preference("sidebarWidth", &json!(600)).unwrap();
    store
        .set_preference("changesSidebarOpen", &json!(true))
        .unwrap();
    store
        .set_preference("changesSidebarWidth", &json!(500))
        .unwrap();
    // Too narrow for both panels at their stored widths, so GTK narrows the sidebar.
    store
        .set_preference("windowSize", &json!({"width": 1150, "height": 700}))
        .unwrap();
    app.start();

    let deadline = Instant::now() + Duration::from_secs(5);
    let mut shown = 0.0;
    while Instant::now() < deadline {
        let inspection = app.request("ui.inspect", json!({}));
        shown = inspection["root"]["children"]
            .as_array()
            .unwrap()
            .iter()
            .find(|node| node["id"] == "sidebar")
            .unwrap()["bounds"]["width"]
            .as_f64()
            .unwrap();
        if shown < 600.0 {
            break;
        }
        thread::sleep(Duration::from_millis(50));
    }
    assert!(shown < 600.0, "sidebar was not narrowed: {shown}");
    // Give a wrongly queued save time to land before checking.
    thread::sleep(Duration::from_millis(500));
    assert_eq!(store.preference("sidebarWidth").unwrap(), Some(json!(600)));
}

#[test]
#[ignore = "requires AGTK_TEST_DISPLAY private Wayland compositor"]
fn changes_sidebar_width_is_restored_on_ui_restart() {
    fn find<'a>(node: &'a Value, id: &str) -> Option<&'a Value> {
        if node["id"] == id {
            return Some(node);
        }
        node["children"]
            .as_array()?
            .iter()
            .find_map(|child| find(child, id))
    }

    let mut app = App::new();
    app.stop();
    let store = Store::open(&app.paths.database()).unwrap();
    store
        .set_preference("changesSidebarOpen", &json!(true))
        .unwrap();
    store
        .set_preference("changesSidebarWidth", &json!(300))
        .unwrap();
    app.start();

    // The sidebar slides open, so wait for the layout to settle.
    let deadline = Instant::now() + Duration::from_secs(5);
    let mut bounds = Value::Null;
    while Instant::now() < deadline {
        let inspection = app.request("ui.inspect", json!({}));
        let changes = find(&inspection["root"], "changes-sidebar").unwrap();
        assert_eq!(changes["isVisible"], true);
        bounds = changes["bounds"].clone();
        if bounds["width"] == 300.0 && bounds["x"] == 900.0 {
            return;
        }
        thread::sleep(Duration::from_millis(50));
    }
    panic!("changes sidebar did not settle at 300 px: {bounds}");
}

#[test]
#[ignore = "requires AGTK_TEST_DISPLAY private Wayland compositor"]
fn launch_surface_populates_project_and_git_worktree_choices() {
    let app = App::new();
    let project = app.directory.path().join("launch-project");
    std::fs::create_dir(&project).unwrap();
    let git = |args: &[&str]| {
        let status = Command::new("git")
            .args(args)
            .current_dir(&project)
            .status()
            .unwrap();
        assert!(status.success(), "git command failed: {args:?}");
    };
    git(&["init", "-q"]);
    git(&["config", "user.name", "agtk test"]);
    git(&["config", "user.email", "agtk@example.test"]);
    std::fs::write(project.join("README"), "launch probe\n").unwrap();
    git(&["add", "README"]);
    git(&["commit", "-qm", "initial"]);

    let session = app.request(
        "session.create",
        json!({
            "kind":"shell",
            "cwd":project,
            "projectRoot":project,
            "name":"launch choices"
        }),
    );
    let id = session["id"].as_str().unwrap();
    assert_eq!(
        app.request("ui.show", json!({"surface":"launch"}))["shown"],
        true
    );

    let deadline = Instant::now() + Duration::from_secs(5);
    while Instant::now() < deadline {
        let inspection = app.request("ui.inspect", json!({}));
        let launch = inspection["root"]["children"]
            .as_array()
            .unwrap()
            .iter()
            .find(|node| node["id"] == "sidebar")
            .unwrap()["children"]
            .as_array()
            .unwrap()
            .iter()
            .find(|node| node["id"] == "sidebar-controls")
            .unwrap()["children"]
            .as_array()
            .unwrap()
            .iter()
            .find(|node| node["id"] == "launch")
            .unwrap()
            .clone();
        let options = |input: &str| {
            launch["children"]
                .as_array()
                .unwrap()
                .iter()
                .find(|node| node["id"] == input)
                .unwrap()["children"]
                .as_array()
                .unwrap()
                .len()
        };
        if options("launch-project-input") == 1 && options("launch-worktree-input") == 1 {
            app.request("session.close", json!({"sessionId":id}));
            return;
        }
        thread::sleep(Duration::from_millis(30));
    }
    panic!("launch choices were not populated");
}

#[test]
#[ignore = "requires AGTK_TEST_DISPLAY private Wayland compositor"]
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
        json!({"theme":"tokyo-night","followSystem":false,"font":"Monospace 13","uiFontSize":15}),
    );
    assert_eq!(appearance["theme"], "tokyo-night");
    assert_eq!(appearance["effectiveTheme"], "tokyo-night");
    assert_eq!(appearance["font"], "Monospace 13");
    assert_eq!(appearance["uiFontSize"], 15);
    let after = app.request("ui.capture", json!({}));
    assert_ne!(before["sha256"], after["sha256"]);

    app.stop();
    app.start();
    let state = app.request("app.get_state", json!({}));
    assert_eq!(state["appearance"]["theme"], "tokyo-night");
    assert_eq!(state["appearance"]["effectiveTheme"], "tokyo-night");
    assert_eq!(state["appearance"]["followSystem"], false);
    assert_eq!(state["appearance"]["font"], "Monospace 13");
    assert_eq!(state["appearance"]["uiFontSize"], 15);
    app.request("session.close", json!({"sessionId":id}));
}

#[test]
#[ignore = "requires a private Mutter display and AGTK_TEST_BUS session bus"]
fn ctrl_scroll_resizes_each_surface_independently() {
    use glib::variant::ToVariant;

    let display = std::env::var("AGTK_TEST_DISPLAY").unwrap();
    assert!(display.contains("agtk-scroll-"));
    assert_eq!(
        std::env::var("DBUS_SESSION_BUS_ADDRESS").unwrap(),
        std::env::var("AGTK_TEST_BUS").unwrap()
    );
    let mut app = App::new();
    let project = app.directory.path().join("viewer-project");
    std::fs::create_dir(&project).unwrap();
    std::fs::write(project.join("sample.rs"), "fn before() {}\n").unwrap();
    for args in [
        vec!["init", "-q"],
        vec!["add", "sample.rs"],
        vec![
            "-c",
            "user.name=Test",
            "-c",
            "user.email=test@example.com",
            "commit",
            "-qm",
            "base",
        ],
    ] {
        assert!(
            Command::new("git")
                .current_dir(&project)
                .args(args)
                .status()
                .unwrap()
                .success()
        );
    }
    std::fs::write(project.join("sample.rs"), "fn after() {}\n").unwrap();
    std::fs::write(
        project.join("notes.md"),
        "# Font resizing\n\nPreview text follows the viewer size.\n\n```rust\nfn after() {}\n```\n",
    )
    .unwrap();
    let input_path = app.directory.path().join("wheel-input");
    let session = app.request(
        "session.create",
        json!({
            "kind":"custom", "command":"/bin/sh", "cwd":project,
            "args":["-c", "stty raw -echo; printf '\\033[?1000h\\033[?1006h__FONT_SCROLL__\\r\\n'; cat > \"$1\"", "font-scroll", input_path]
        }),
    );
    let id = session["id"].as_str().unwrap();
    app.wait_text(id, "__FONT_SCROLL__");
    let bus = gio::bus_get_sync(gio::BusType::Session, None::<&gio::Cancellable>).unwrap();
    let destination = "org.gnome.Mutter.RemoteDesktop";
    let remote = bus
        .call_sync(
            Some(destination),
            "/org/gnome/Mutter/RemoteDesktop",
            destination,
            "CreateSession",
            None,
            None,
            gio::DBusCallFlags::NONE,
            5000,
            None::<&gio::Cancellable>,
        )
        .unwrap();
    let path = remote.child_value(0).str().unwrap().to_owned();
    let call = |method: &str, parameters: Option<glib::Variant>| {
        bus.call_sync(
            Some(destination),
            &path,
            "org.gnome.Mutter.RemoteDesktop.Session",
            method,
            parameters.as_ref(),
            None,
            gio::DBusCallFlags::NONE,
            5000,
            None::<&gio::Cancellable>,
        )
        .unwrap();
    };
    call("Start", None);
    let key = |code: u32, state: bool| {
        call("NotifyKeyboardKeycode", Some((code, state).to_variant()));
    };
    let wheel = |steps: i32| {
        call(
            "NotifyPointerAxisDiscrete",
            Some((0_u32, steps).to_variant()),
        );
    };
    let move_to = |x: f64, y: f64| {
        call(
            "NotifyPointerMotionRelative",
            Some((-10000.0_f64, -10000.0_f64).to_variant()),
        );
        call("NotifyPointerMotionRelative", Some((x, y).to_variant()));
        thread::sleep(Duration::from_millis(100));
    };
    let assert_sizes = |ui: u8, font: &str, viewer: u8| {
        let deadline = Instant::now() + Duration::from_secs(3);
        loop {
            let appearance = app.request("app.get_state", json!({}))["appearance"].clone();
            if appearance["uiFontSize"] == ui
                && appearance["font"] == font
                && appearance["viewerFontSize"] == viewer
            {
                break;
            }
            assert!(
                Instant::now() < deadline,
                "expected UI {ui}, terminal {font}, viewer {viewer}: {appearance}"
            );
            thread::sleep(Duration::from_millis(30));
        }
    };
    thread::sleep(Duration::from_millis(300));
    // Super+Up maximizes the disposable window so coordinates start at (0, 0).
    key(125, true);
    key(103, true);
    key(103, false);
    key(125, false);
    thread::sleep(Duration::from_millis(300));
    let input_before_zoom = std::fs::read_to_string(&input_path).unwrap();

    // The terminal keeps keyboard focus while the pointer is over the sidebar.
    move_to(120.0, 250.0);
    key(29, true);
    wheel(-1);
    assert_sizes(14, "Monospace 11", 11);
    wheel(1);
    assert_sizes(13, "Monospace 11", 11);

    move_to(700.0, 350.0);
    for _ in 0..3 {
        wheel(-1);
    }
    assert_sizes(13, "Monospace 14", 11);
    wheel(1);
    assert_sizes(13, "Monospace 13", 11);
    // Smooth touchpad deltas accumulate, including sub-step motion.
    for _ in 0..5 {
        call(
            "NotifyPointerAxis",
            Some((0.0_f64, -10.0_f64, 4_u32).to_variant()),
        );
    }
    call(
        "NotifyPointerAxis",
        Some((0.0_f64, 0.0_f64, 5_u32).to_variant()),
    );
    assert_sizes(13, "Monospace 14", 11);
    assert_eq!(
        std::fs::read_to_string(&input_path).unwrap(),
        input_before_zoom,
        "zoom must not send terminal input"
    );
    key(29, false);
    wheel(-1);
    thread::sleep(Duration::from_millis(100));
    assert_sizes(13, "Monospace 14", 11);
    let input = std::fs::read_to_string(&input_path).unwrap();
    assert!(
        input.contains("\x1b[<64;"),
        "ordinary scrolling must still reach the terminal: {input:?}"
    );

    // Limits apply independently: UI at its maximum must not block terminal zoom.
    app.request(
        "appearance.set",
        json!({"uiFontSize":24, "font":"Monospace 31"}),
    );
    key(29, true);
    wheel(-1);
    assert_sizes(24, "Monospace 32", 11);
    wheel(-1);
    move_to(120.0, 250.0);
    wheel(-1);
    thread::sleep(Duration::from_millis(100));
    assert_sizes(24, "Monospace 32", 11);
    app.request(
        "appearance.set",
        json!({"uiFontSize":9, "font":"Monospace 6"}),
    );
    wheel(1);
    move_to(700.0, 350.0);
    wheel(1);
    thread::sleep(Duration::from_millis(100));
    assert_sizes(9, "Monospace 6", 11);
    key(29, false);
    app.request(
        "appearance.set",
        json!({"uiFontSize":13, "font":"Monospace 14"}),
    );
    app.activate_action("increase-font-size");
    assert_sizes(14, "Monospace 15", 12);
    let wait_for_viewer = || {
        let deadline = Instant::now() + Duration::from_secs(15);
        loop {
            let inspection = app.request("ui.inspect", json!({}));
            if inspection.to_string().contains("rendered:") {
                break;
            }
            assert!(
                Instant::now() < deadline,
                "viewer did not render: {inspection}"
            );
            thread::sleep(Duration::from_millis(30));
        }
    };
    app.request("ui.open_file", json!({"sessionId":id,"path":"sample.rs"}));
    wait_for_viewer();
    move_to(700.0, 350.0);
    let before = app.request("ui.capture", json!({}));
    key(29, true);
    wheel(-1);
    assert_sizes(14, "Monospace 15", 13);
    key(29, false);
    let after = app.request("ui.capture", json!({}));
    assert_ne!(before["sha256"], after["sha256"], "viewer must resize live");
    if let Ok(target) = std::env::var("AGTK_TEST_CAPTURE_DIR") {
        for (name, capture) in [("font-before.png", &before), ("font-after.png", &after)] {
            std::fs::copy(
                capture["path"].as_str().unwrap(),
                std::path::Path::new(&target).join(name),
            )
            .unwrap();
        }
    }

    app.request(
        "ui.open_diff",
        json!({"sessionId":id,"path":"sample.rs","scope":"unstaged"}),
    );
    wait_for_viewer();
    move_to(700.0, 350.0);
    key(29, true);
    wheel(-1);
    assert_sizes(14, "Monospace 15", 14);
    wheel(-100);
    assert_sizes(14, "Monospace 15", 48);
    wheel(100);
    assert_sizes(14, "Monospace 15", 8);
    wheel(-6);
    assert_sizes(14, "Monospace 15", 14);
    key(29, false);

    app.request("ui.open_file", json!({"sessionId":id,"path":"notes.md"}));
    wait_for_viewer();
    move_to(700.0, 350.0);
    let before = app.request("ui.capture", json!({}));
    key(29, true);
    wheel(-3);
    assert_sizes(14, "Monospace 15", 17);
    key(29, false);
    let after = app.request("ui.capture", json!({}));
    assert_ne!(
        before["sha256"], after["sha256"],
        "Markdown preview must resize live"
    );
    if let Ok(target) = std::env::var("AGTK_TEST_CAPTURE_DIR") {
        for (name, capture) in [
            ("markdown-before.png", &before),
            ("markdown-after.png", &after),
        ] {
            std::fs::copy(
                capture["path"].as_str().unwrap(),
                std::path::Path::new(&target).join(name),
            )
            .unwrap();
        }
    }
    call("Stop", None);

    let store = Store::open(&app.paths.database()).unwrap();
    let deadline = Instant::now() + Duration::from_secs(3);
    while store.preference("appearance").unwrap().unwrap()["viewerFontSize"] != 17 {
        assert!(Instant::now() < deadline, "font size was not saved");
        thread::sleep(Duration::from_millis(30));
    }
    app.stop();
    app.start();
    let appearance = app.request("app.get_state", json!({}))["appearance"].clone();
    assert_eq!(appearance["uiFontSize"], 14);
    assert_eq!(appearance["font"], "Monospace 15");
    assert_eq!(appearance["viewerFontSize"], 17);
    app.request("session.close", json!({"sessionId":id}));
}

#[test]
#[ignore = "requires AGTK_TEST_DISPLAY private Wayland compositor"]
fn emacs_actions_use_the_selected_sessions_repository_context() {
    let app = App::new();
    let project = app.directory.path().join("emacs-project");
    std::fs::create_dir(&project).unwrap();
    for args in [
        &["init", "-q"][..],
        &["config", "user.name", "Test"][..],
        &["config", "user.email", "test@example.com"][..],
    ] {
        assert!(
            Command::new("git")
                .args(args)
                .current_dir(&project)
                .status()
                .unwrap()
                .success()
        );
    }
    std::fs::write(project.join("tracked"), "base").unwrap();
    for args in [&["add", "tracked"][..], &["commit", "-qm", "base"][..]] {
        assert!(
            Command::new("git")
                .args(args)
                .current_dir(&project)
                .status()
                .unwrap()
                .success()
        );
    }
    let session = app.request(
        "session.create",
        json!({
            "kind":"custom",
            "command":"/bin/sh",
            "args":["-c","exec sleep 30"],
            "cwd":project,
            "projectRoot":project,
            "worktreePath":project,
            "name":"emacs probe"
        }),
    );
    let id = session["id"].as_str().unwrap();
    let inspection = app.request("ui.inspect", json!({}));
    let context = &inspection["root"]["children"][1]["children"][0]["children"];
    assert!(context[1]["isEnabled"].as_bool().unwrap());

    let magit = app.request("session.open_magit", json!({"sessionId":id}));
    assert_eq!(magit["action"], "magit");
    assert_eq!(magit["path"], project.to_str().unwrap());
    let args = std::fs::read_to_string(app.directory.path().join("emacs-args")).unwrap();
    assert!(args.contains("magit-status"));
    let review = app.request("session.open_branch_review", json!({"sessionId":id}));
    assert_eq!(review["action"], "branch-review");
    let args = std::fs::read_to_string(app.directory.path().join("emacs-args")).unwrap();
    assert!(args.contains("branch-review"));
    app.request("session.close", json!({"sessionId":id}));
}

#[test]
#[ignore = "requires AGTK_TEST_DISPLAY private Wayland compositor"]
fn claude_model_presets_are_exact_provider_only_and_durable() {
    let mut app = App::new();
    let session = app.request(
        "session.create",
        json!({
            "kind":"claude",
            "cwd":app.directory.path(),
            "name":"claude preset probe"
        }),
    );
    let id = session["id"].as_str().unwrap().to_owned();
    // A fresh Claude session only gets the agtk hook settings.
    app.wait_text(&id, "__RESTORE_ARGS_--settings_");

    let inspection = app.request("ui.inspect", json!({}));
    let claude_model = inspection["root"]["children"][0]["children"][3]["children"]
        .as_array()
        .unwrap()
        .iter()
        .find(|node| node["id"] == "claude-model")
        .unwrap();
    assert_eq!(claude_model["isVisible"], true);
    assert_eq!(claude_model["isEnabled"], true);
    assert_eq!(
        app.request("ui.show", json!({"surface":"claude-models"}))["shown"],
        true
    );
    let capture = app.request("ui.capture", json!({}));
    assert!(capture["path"].as_str().is_some());

    let applied = app.request(
        "claude.preset_apply",
        json!({"sessionId":id,"presetId":"opus-high"}),
    );
    assert_eq!(applied["commandsSent"], 2);
    app.wait_text(&id, "__INPUT_/model opus__");
    app.wait_text(&id, "__INPUT_/effort high__");

    app.request(
        "claude.presets_set",
        json!({"presets":[{
            "id":"focused",
            "name":"Focused",
            "model":"sonnet",
            "effort":"xhigh"
        }]}),
    );
    app.stop();
    app.start();
    app.request(
        "claude.preset_apply",
        json!({"sessionId":id,"presetId":"focused"}),
    );
    app.wait_text(&id, "__INPUT_/model sonnet__");
    app.wait_text(&id, "__INPUT_/effort xhigh__");

    let shell = app.request(
        "session.create",
        json!({
            "kind":"custom",
            "command":"/bin/sh",
            "args":["-c","exec sleep 30"],
            "cwd":app.directory.path(),
            "name":"not claude"
        }),
    );
    let shell_id = shell["id"].as_str().unwrap();
    match app.request_body(
        "claude.preset_apply",
        json!({"sessionId":shell_id,"presetId":"focused"}),
    ) {
        ResponseBody::Failure(error) => assert_eq!(error.code, ErrorCode::OperationRefused),
        response => panic!("preset unexpectedly reached a non-Claude session: {response:?}"),
    }
    app.request("session.close", json!({"sessionId":shell_id}));
    app.request("session.close", json!({"sessionId":id}));
}

#[test]
#[ignore = "requires AGTK_TEST_DISPLAY private Wayland compositor"]
fn azure_pr_attention_and_review_launch_use_the_project_context() {
    fn find<'a>(node: &'a Value, id: &str) -> Option<&'a Value> {
        if node["id"] == id {
            return Some(node);
        }
        node["children"]
            .as_array()?
            .iter()
            .find_map(|child| find(child, id))
    }

    let app = App::new();
    let project = app.directory.path().join("azure-project");
    std::fs::create_dir(&project).unwrap();
    for args in [
        &["init", "-q"][..],
        &["config", "user.name", "Test"][..],
        &["config", "user.email", "test@example.com"][..],
    ] {
        assert!(
            Command::new("git")
                .args(args)
                .current_dir(&project)
                .status()
                .unwrap()
                .success()
        );
    }
    // The origin is an Azure URL for detection, rewritten to a local bare
    // repository so review checkouts can fetch the PR branches.
    let origin = app.directory.path().join("azure-origin.git");
    let origin_text = origin.to_str().unwrap().to_owned();
    let rewrite = format!("url.{origin_text}.insteadOf");
    assert!(
        Command::new("git")
            .args(["init", "-q", "--bare", &origin_text])
            .status()
            .unwrap()
            .success()
    );
    std::fs::write(project.join("tracked"), "base").unwrap();
    for args in [
        &["add", "tracked"][..],
        &["commit", "-qm", "base"][..],
        &["branch", "-M", "feature"][..],
        &["branch", "later-attention"][..],
        &[
            "remote",
            "add",
            "origin",
            "https://dev.azure.com/org/project/_git/repo",
        ][..],
        &[
            "config",
            &rewrite,
            "https://dev.azure.com/org/project/_git/repo",
        ][..],
        &["push", "-q", "origin", "feature", "later-attention"][..],
    ] {
        assert!(
            Command::new("git")
                .args(args)
                .current_dir(&project)
                .status()
                .unwrap()
                .success()
        );
    }
    let project = project.canonicalize().unwrap();
    let review_checkout = |id: u64| {
        app.directory
            .path()
            .canonicalize()
            .unwrap()
            .join(format!("worktrees/azure-project/pr-{id}"))
    };

    let pr = |id: u64, branch: &str| {
        json!({
            "pullRequestId":id,
            "title":format!("Review {branch}"),
            "sourceRefName":format!("refs/heads/{branch}"),
            "targetRefName":"refs/heads/main",
            "creationDate":"2026-09-15T10:00:00Z",
            "isDraft":false,
            "createdBy":{"displayName":"Colleague","uniqueName":"colleague@example.com"},
            "lastMergeSourceCommit":{"commitId":"0123456789abcdef"},
            "mergeStatus":"succeeded",
            "reviewers":[]
        })
    };
    let pr_file = app.directory.path().join("azure-prs.json");
    std::fs::write(
        &pr_file,
        serde_json::to_vec(&json!([pr(42, "feature")])).unwrap(),
    )
    .unwrap();

    let baseline = app.request("pr.list", json!({"projectRoot":project}));
    assert_eq!(baseline["pullRequests"][0]["attention"], Value::Null);
    assert_eq!(
        baseline["pullRequests"][0]["worktreePath"],
        project.to_str().unwrap()
    );

    std::fs::write(
        &pr_file,
        serde_json::to_vec(&json!([pr(42, "feature"), pr(43, "review-me")])).unwrap(),
    )
    .unwrap();
    let changed = app.request("pr.list", json!({"projectRoot":project}));
    let added = changed["pullRequests"]
        .as_array()
        .unwrap()
        .iter()
        .find(|item| item["pullRequest"]["id"] == 43)
        .unwrap();
    assert_eq!(added["attention"], "new");
    assert_eq!(
        app.request("app.get_state", json!({}))["attention"]["count"],
        1
    );
    assert_eq!(
        app.request(
            "pr.acknowledge",
            json!({"projectRoot":project,"pullRequestId":43,"marker":"published"})
        )["acknowledged"],
        false
    );
    assert_eq!(
        app.request(
            "pr.acknowledge",
            json!({"projectRoot":project,"pullRequestId":43,"marker":"new"})
        )["acknowledged"],
        true
    );
    assert_eq!(
        app.request("app.get_state", json!({}))["attention"]["count"],
        0
    );

    // The user is busy in another session when the review starts.
    app.request(
        "session.create",
        json!({"kind":"shell","cwd":app.directory.path()}),
    );
    let review = app.request(
        "pr.launch_review",
        json!({"projectRoot":project,"pullRequestId":42}),
    );
    assert_eq!(review["name"], "review: PR #42");
    // Each review runs in its own detached checkout at the PR tip.
    assert_eq!(review["cwd"], review_checkout(42).to_str().unwrap());
    assert_eq!(
        review["worktreePath"],
        review_checkout(42).to_str().unwrap()
    );
    assert_eq!(review["projectRoot"], project.to_str().unwrap());
    // A review never takes the selection away from what the user was doing.
    assert_eq!(review["isSelected"], false);
    let review_id = review["id"].as_str().unwrap();
    app.request("session.select", json!({"sessionId":review_id}));

    let deadline = Instant::now() + Duration::from_secs(5);
    loop {
        let inspection = app.request("ui.inspect", json!({}));
        let pr_context = inspection["root"]["children"][1]["children"][0]["children"]
            .as_array()
            .unwrap()
            .iter()
            .find(|node| node["id"] == "pr-context")
            .unwrap();
        if pr_context["isVisible"] == true {
            assert_eq!(pr_context["label"], "PR #42: Review feature");
            break;
        }
        assert!(
            Instant::now() < deadline,
            "PR context did not become visible"
        );
        thread::sleep(Duration::from_millis(30));
    }
    let row_pr = |app: &App, id: &str| -> Value {
        let inspection = app.request("ui.inspect", json!({}));
        find(&inspection["root"], &format!("session-pr-{id}"))
            .cloned()
            .expect("session row has a PR node")
    };
    let review_pr = row_pr(&app, review_id);
    assert_eq!(review_pr["isVisible"], true, "{review_pr}");
    assert_eq!(review_pr["label"], "#42");

    assert_eq!(
        app.request(
            "pr.set_auto_review",
            json!({"projectRoot":project,"enabled":true})
        )["enabled"],
        true
    );
    std::fs::write(
        &pr_file,
        serde_json::to_vec(&json!([
            pr(42, "feature"),
            pr(43, "review-me"),
            pr(44, "later-attention")
        ]))
        .unwrap(),
    )
    .unwrap();
    app.request("pr.list", json!({"projectRoot":project}));
    let deadline = Instant::now() + Duration::from_secs(5);
    let auto_review_id = loop {
        let state = app.request("app.get_state", json!({}));
        if let Some(id) = state["sessions"]
            .as_array()
            .unwrap()
            .iter()
            .find_map(|session| {
                (session["name"] == "review: PR #44")
                    .then(|| session["id"].as_str().unwrap().to_owned())
            })
        {
            break id;
        }
        assert!(
            Instant::now() < deadline,
            "automatic PR review did not launch"
        );
        thread::sleep(Duration::from_millis(30));
    };
    let state = app.request("app.get_state", json!({}));
    assert_ne!(state["selectedSessionId"], auto_review_id.as_str());
    // The detached review checkout is on no branch, so the review links to its
    // PR by name: in the row at once, and in the PR bar once selected.
    let deadline = Instant::now() + Duration::from_secs(5);
    loop {
        let node = row_pr(&app, &auto_review_id);
        if node["isVisible"] == true {
            assert_eq!(node["label"], "#44");
            break;
        }
        assert!(Instant::now() < deadline, "row PR button did not appear");
        thread::sleep(Duration::from_millis(30));
    }
    app.request("session.select", json!({"sessionId":auto_review_id}));
    let deadline = Instant::now() + Duration::from_secs(5);
    loop {
        let inspection = app.request("ui.inspect", json!({}));
        let pr_context = find(&inspection["root"], "pr-context").unwrap();
        if pr_context["isVisible"] == true && pr_context["label"] != "PR #42: Review feature" {
            assert_eq!(pr_context["label"], "PR #44: Review later-attention");
            break;
        }
        assert!(
            Instant::now() < deadline,
            "PR bar did not show the reviewed PR"
        );
        thread::sleep(Duration::from_millis(30));
    }
    thread::sleep(Duration::from_millis(50));
    let capture = app.request("ui.capture", json!({}));
    if let Ok(target) = std::env::var("AGTK_TEST_CAPTURE_DIR") {
        let path = std::path::Path::new(capture["path"].as_str().unwrap());
        let _ = std::fs::copy(path, std::path::Path::new(&target).join("session-pr.png"));
    }

    // The dialog opens last, so the capture above shows the main window.
    assert_eq!(
        app.request("ui.show", json!({"surface":"pull-requests"}))["shown"],
        true
    );
    thread::sleep(Duration::from_millis(100));
    let inspection = app.request("ui.inspect", json!({}));
    let pr_node = inspection["root"]["children"][0]["children"][3]["children"]
        .as_array()
        .unwrap()
        .iter()
        .find(|node| node["id"] == "pull-requests")
        .unwrap();
    assert_eq!(pr_node["isSelected"], true, "PR inspection node: {pr_node}");
    let capture = app.request("ui.capture", json!({}));
    assert!(
        capture["path"].as_str().is_some(),
        "unexpected capture: {capture}"
    );
    app.request("session.close", json!({"sessionId":review_id}));
    app.request("session.close", json!({"sessionId":auto_review_id}));
    assert!(review_checkout(42).is_dir());
    assert!(review_checkout(44).is_dir());
}

#[test]
#[ignore = "requires AGTK_TEST_DISPLAY private Wayland compositor"]
fn session_pr_attention_follows_comments_acknowledgement_and_restart() {
    let mut app = App::new();
    let project = app.directory.path().join("azure-project");
    std::fs::create_dir(&project).unwrap();
    for args in [
        &["init", "-q", "-b", "feature"][..],
        &[
            "remote",
            "add",
            "origin",
            "https://dev.azure.com/org/project/_git/repo",
        ][..],
    ] {
        assert!(
            Command::new("git")
                .args(args)
                .current_dir(&project)
                .status()
                .unwrap()
                .success()
        );
    }
    let pr_file = app.directory.path().join("azure-prs.json");
    let mut prs = json!([
        {
            "pullRequestId":42, "title":"Review feature",
            "sourceRefName":"refs/heads/feature", "targetRefName":"refs/heads/main",
            "creationDate":"2026-09-15T10:00:00Z", "isDraft":false,
            "createdBy":{"displayName":"Colleague"}, "reviewers":[]
        },
        {
            "pullRequestId":43, "title":"Review later",
            "sourceRefName":"refs/heads/later", "targetRefName":"refs/heads/main",
            "creationDate":"2026-09-15T10:00:00Z", "isDraft":false,
            "createdBy":{"displayName":"Colleague"}, "reviewers":[]
        }
    ]);
    std::fs::write(&pr_file, serde_json::to_vec(&json!([prs[0]])).unwrap()).unwrap();
    app.request("pr.list", json!({"projectRoot":project}));
    let threads_file = app.directory.path().join("azure-threads.json");
    // PR #103630 had only system events, not review comments.
    std::fs::write(
        &threads_file,
        serde_json::to_vec(&json!({"value":[{
            "status":null, "comments":[{
                "commentType":"system", "content":"Branch updated",
                "publishedDate":"2026-10-01T09:00:00Z"
            }]
        }]}))
        .unwrap(),
    )
    .unwrap();
    std::fs::write(&pr_file, serde_json::to_vec(&prs).unwrap()).unwrap();
    let new = app.request("pr.list", json!({"projectRoot":project}));
    assert!(
        new["pullRequests"]
            .as_array()
            .unwrap()
            .iter()
            .any(|item| { item["pullRequest"]["id"] == 43 && item["attention"] == "new" })
    );
    let mut ids = Vec::new();
    for name in ["feature session", "review: PR #43"] {
        let session = app.request(
            "session.create",
            json!({"kind":"shell","cwd":project,"projectRoot":project,"name":name}),
        );
        ids.push(session["id"].as_str().unwrap().to_owned());
    }
    // Both linked sessions remain in the background as their comments arrive.
    let other = app.request(
        "session.create",
        json!({"kind":"shell","cwd":app.directory.path()}),
    );
    app.request("session.select", json!({"sessionId":other["id"]}));
    let attention = |app: &App, id: &str| -> Value {
        let inspection = app.request("ui.inspect", json!({}));
        let sessions = &inspection["root"]["children"][0]["children"][1]["children"];
        let session = sessions
            .as_array()
            .unwrap()
            .iter()
            .find(|node| node["id"] == format!("session-{id}"))
            .unwrap();
        let pr = session["children"]
            .as_array()
            .unwrap()
            .iter()
            .find(|node| node["id"] == format!("session-pr-{id}"))
            .unwrap();
        pr["children"]
            .as_array()
            .unwrap()
            .iter()
            .find(|node| node["id"] == format!("session-pr-attention-{id}"))
            .expect("session PR button exposes its activity marker")
            .clone()
    };
    assert_eq!(attention(&app, &ids[0])["isVisible"], false);
    assert_eq!(attention(&app, &ids[1])["isVisible"], false);

    prs[1]["isDraft"] = json!(true);
    std::fs::write(&pr_file, serde_json::to_vec(&prs).unwrap()).unwrap();
    app.request("pr.list", json!({"projectRoot":project}));
    prs[1]["isDraft"] = json!(false);
    std::fs::write(&pr_file, serde_json::to_vec(&prs).unwrap()).unwrap();
    let published = app.request("pr.list", json!({"projectRoot":project}));
    assert!(
        published["pullRequests"]
            .as_array()
            .unwrap()
            .iter()
            .any(|item| { item["pullRequest"]["id"] == 43 && item["attention"] == "published" })
    );
    assert_eq!(attention(&app, &ids[1])["isVisible"], false);

    for date in ["2026-10-01T10:00:00Z", "2026-10-01T11:00:00Z"] {
        std::fs::write(
            &threads_file,
            serde_json::to_vec(&json!({"value":[{
                "status":"active", "comments":[{
                    "commentType":"text", "content":"Please fix this",
                    "publishedDate":date
                }]
            }]}))
            .unwrap(),
        )
        .unwrap();
        app.request("pr.list", json!({"projectRoot":project}));
        assert_eq!(attention(&app, &ids[0])["isVisible"], true);
        assert_eq!(attention(&app, &ids[1])["isVisible"], true);
        if date == "2026-10-01T11:00:00Z"
            && let Ok(target) = std::env::var("AGTK_TEST_CAPTURE_DIR")
        {
            let capture = app.request("ui.capture", json!({}));
            std::fs::copy(
                capture["path"].as_str().unwrap(),
                std::path::Path::new(&target).join("session-pr-attention.png"),
            )
            .unwrap();
        }
        app.request(
            "pr.acknowledge",
            json!({"projectRoot":project,"pullRequestId":42,"marker":"review"}),
        );
        assert_eq!(attention(&app, &ids[0])["isVisible"], false);
        // Acknowledging one PR leaves the other session's marker visible.
        assert_eq!(attention(&app, &ids[1])["isVisible"], true);
    }
    app.stop();
    app.start();
    assert_eq!(attention(&app, &ids[0])["isVisible"], false);
    assert_eq!(attention(&app, &ids[1])["isVisible"], true);
    std::fs::write(&pr_file, "[]").unwrap();
    app.request("pr.list", json!({"projectRoot":project}));
    assert_eq!(attention(&app, &ids[1])["isVisible"], false);
}

#[test]
#[ignore = "requires AGTK_TEST_DISPLAY private Wayland compositor"]
fn session_pr_bar_uses_cache_during_refresh_and_after_restart() {
    let mut app = App::new();
    let project = app.directory.path().join("azure-project");
    std::fs::create_dir(&project).unwrap();
    for args in [
        &["init", "-q", "-b", "feature"][..],
        &[
            "remote",
            "add",
            "origin",
            "https://dev.azure.com/org/project/_git/repo",
        ][..],
    ] {
        assert!(
            Command::new("git")
                .args(args)
                .current_dir(&project)
                .status()
                .unwrap()
                .success()
        );
    }
    let pr_file = app.directory.path().join("azure-prs.json");
    let pbi_file = app.directory.path().join("azure-pbis.json");
    std::fs::write(&pbi_file, serde_json::to_vec(&json!([
        {"id":237561,"fields":{"System.WorkItemType":"Product Backlog Item","System.Title":"Monitor workflows"}},
        {"id":237562,"fields":{"System.WorkItemType":"Product Backlog Item","System.Title":"Alert on failures"}},
        {"id":237563,"fields":{"System.WorkItemType":"Task","System.Title":"Implementation task"}}
    ])).unwrap()).unwrap();
    let mut pr = json!({
        "pullRequestId":42, "title":"Cached review",
        "sourceRefName":"refs/heads/feature", "targetRefName":"refs/heads/main",
        "creationDate":"2026-09-15T10:00:00Z", "isDraft":false,
        "createdBy":{"displayName":"Colleague"}, "reviewers":[]
    });
    std::fs::write(&pr_file, serde_json::to_vec(&json!([pr])).unwrap()).unwrap();
    let other = app.request(
        "session.create",
        json!({"kind":"shell","cwd":app.directory.path()}),
    );
    let other_id = other["id"].as_str().unwrap();
    let session = app.request(
        "session.create",
        json!({"kind":"shell","cwd":project,"projectRoot":project}),
    );
    let id = session["id"].as_str().unwrap();
    app.request("session.select", json!({"sessionId":id}));
    let context = |app: &App| {
        let inspection = app.request("ui.inspect", json!({}));
        inspection["root"]["children"][1]["children"][0]["children"]
            .as_array()
            .unwrap()
            .iter()
            .find(|node| node["id"] == "pr-context")
            .unwrap()
            .clone()
    };
    let wait_label = |app: &App, label: &str, timeout: Duration| {
        let deadline = Instant::now() + timeout;
        loop {
            let node = context(app);
            if node["isVisible"] == true && node["label"] == label {
                break;
            }
            assert!(
                Instant::now() < deadline,
                "PR bar did not show {label}: {node}"
            );
            thread::sleep(Duration::from_millis(30));
        }
    };
    wait_label(&app, "PR #42: Cached review", Duration::from_secs(5));
    let assert_pbis = |node: &Value| {
        let labels = node["children"]
            .as_array()
            .unwrap()
            .iter()
            .map(|child| {
                assert_eq!(child["role"], "link");
                assert_eq!(child["isVisible"], true);
                child["label"].as_str().unwrap()
            })
            .collect::<Vec<_>>();
        assert_eq!(labels, ["PBI #237561", "PBI #237562"]);
    };
    assert_pbis(&context(&app));

    let slow = app.directory.path().join("azure-prs.json.slow");
    std::fs::write(&slow, "").unwrap();
    app.request("session.select", json!({"sessionId":other_id}));
    assert_eq!(context(&app)["isVisible"], false);
    app.request("session.select", json!({"sessionId":id}));
    let node = context(&app);
    assert_eq!(
        node["isVisible"], true,
        "cached PR should appear before the slow Azure lookup"
    );
    assert_eq!(node["label"], "PR #42: Cached review");
    assert_pbis(&node);

    app.stop();
    app.start();
    let node = context(&app);
    assert_eq!(
        node["isVisible"], true,
        "saved PR should appear immediately after restarting"
    );
    assert_eq!(node["label"], "PR #42: Cached review");
    assert_pbis(&node);

    pr["title"] = json!("Refreshed review");
    std::fs::write(&pr_file, serde_json::to_vec(&json!([pr])).unwrap()).unwrap();
    std::fs::remove_file(&slow).unwrap();
    wait_label(&app, "PR #42: Refreshed review", Duration::from_secs(15));

    std::fs::write(&pr_file, "invalid JSON").unwrap();
    app.request("session.select", json!({"sessionId":other_id}));
    app.request("session.select", json!({"sessionId":id}));
    // This lookup queues behind the session refresh, so its failure is a completion barrier.
    assert!(matches!(
        app.request_body("pr.list", json!({"projectRoot":project})),
        ResponseBody::Failure(_)
    ));
    let node = context(&app);
    assert_eq!(
        node["isVisible"], true,
        "failed refresh should retain the cached PR"
    );
    assert_eq!(node["label"], "PR #42: Refreshed review");
    assert_pbis(&node);

    std::fs::write(&pbi_file, "[]").unwrap();
    std::fs::write(&pr_file, serde_json::to_vec(&json!([pr])).unwrap()).unwrap();
    app.request("session.select", json!({"sessionId":other_id}));
    app.request("session.select", json!({"sessionId":id}));
    let deadline = Instant::now() + Duration::from_secs(5);
    while !context(&app)["children"].as_array().unwrap().is_empty() {
        assert!(
            Instant::now() < deadline,
            "unlinked PBIs should disappear from the PR bar"
        );
        thread::sleep(Duration::from_millis(30));
    }
    assert_eq!(context(&app)["isVisible"], true);

    std::fs::write(&pr_file, "[]").unwrap();
    app.request("session.select", json!({"sessionId":other_id}));
    app.request("session.select", json!({"sessionId":id}));
    let deadline = Instant::now() + Duration::from_secs(5);
    while context(&app)["isVisible"] == true {
        assert!(
            Instant::now() < deadline,
            "PR bar should disappear when the PR is no longer active"
        );
        thread::sleep(Duration::from_millis(30));
    }
    std::fs::write(&slow, "").unwrap();
    app.request("session.select", json!({"sessionId":other_id}));
    app.request("session.select", json!({"sessionId":id}));
    assert_eq!(
        context(&app)["isVisible"],
        false,
        "removed PR should also be evicted from the cache"
    );
    std::fs::remove_file(slow).unwrap();
    app.request("session.close", json!({"sessionId":id}));
    app.request("session.close", json!({"sessionId":other_id}));
}

#[test]
#[ignore = "requires AGTK_TEST_DISPLAY private Wayland compositor"]
fn closing_a_session_does_not_wait_for_a_slow_pr_lookup() {
    let app = App::new();
    let project = app.directory.path().join("azure-project");
    std::fs::create_dir(&project).unwrap();
    for args in [
        &["init", "-q"][..],
        &[
            "remote",
            "add",
            "origin",
            "https://dev.azure.com/org/project/_git/repo",
        ][..],
    ] {
        assert!(
            Command::new("git")
                .args(args)
                .current_dir(&project)
                .status()
                .unwrap()
                .success()
        );
    }
    // Selecting a session in an Azure project looks up its PR through the slow fake az.
    let slow = app.directory.path().join("azure-prs.json.slow");
    std::fs::write(&slow, "").unwrap();
    let session = app.request(
        "session.create",
        json!({"kind":"shell","cwd":project,"projectRoot":project}),
    );
    let id = session["id"].as_str().unwrap().to_owned();

    let started = Instant::now();
    app.request("session.close", json!({"sessionId":id}));
    assert!(
        started.elapsed() < Duration::from_secs(4),
        "close waited {:?} behind the PR lookup",
        started.elapsed()
    );
    std::fs::remove_file(slow).unwrap();
}

#[test]
#[ignore = "requires AGTK_TEST_DISPLAY private Wayland compositor"]
fn renaming_an_agent_session_renames_it_in_the_agent_once_idle() {
    let app = App::new();
    let session = app.request(
        "session.create",
        json!({
            "kind":"claude",
            "cwd":app.directory.path(),
            "name":"before rename"
        }),
    );
    let id = session["id"].as_str().unwrap().to_owned();
    app.wait_text(&id, "__RESTORE_ARGS_");
    app.request(
        "session.send_input",
        json!({"sessionId":id,"text":"start a turn","appendEnter":true}),
    );
    app.wait_text(&id, "__INPUT_start a turn__");

    let renamed = app.request("session.rename", json!({"sessionId":id,"name":"Planner"}));
    assert_eq!(renamed["state"], "busy");
    app.request(
        "session.send_input",
        json!({"sessionId":id,"text":"still busy","appendEnter":true}),
    );
    let text = app.wait_text(&id, "__INPUT_still busy__");
    assert!(!text.contains("/rename"), "rename reached a busy agent");

    app.request("session.set_state", json!({"sessionId":id,"state":"ready"}));
    app.wait_text(&id, "__INPUT_/rename Planner__");
    app.request("session.close", json!({"sessionId":id}));
}

#[test]
#[ignore = "requires AGTK_TEST_DISPLAY private Wayland compositor"]
fn shortcut_overrides_are_validated_and_survive_ui_restart() {
    let mut app = App::new();
    assert_eq!(
        app.request("ui.show", json!({"surface":"shortcuts"}))["shown"],
        true
    );
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
    let inspection = app.request("ui.inspect", json!({}));
    let shortcuts = inspection["root"]["children"][0]["children"][2]["children"]
        .as_array()
        .unwrap()
        .iter()
        .find(|node| node["id"] == "shortcuts")
        .unwrap();
    assert_eq!(shortcuts["isSelected"], true);

    let duplicate = app.request_body(
        "shortcut.set",
        json!({"action":"launch-in-project","accelerator":"<Alt>b"}),
    );
    assert!(matches!(
        duplicate,
        ResponseBody::Failure(ref error) if error.code == agtk::control::ErrorCode::InvalidParams
    ));
    let unmodified = app.request_body(
        "shortcut.set",
        json!({"action":"launch-in-project","accelerator":"n"}),
    );
    assert!(matches!(
        unmodified,
        ResponseBody::Failure(ref error) if error.code == agtk::control::ErrorCode::InvalidParams
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

#[test]
#[ignore = "requires AGTK_TEST_DISPLAY private Wayland compositor"]
fn ready_session_navigation_skips_collapsed_projects() {
    let app = App::new();
    let hidden_project = app.directory.path().join("hidden");
    let hidden_worktree = app.directory.path().join("hidden-worktree");
    std::fs::create_dir(&hidden_project).unwrap();
    std::fs::create_dir(&hidden_worktree).unwrap();
    let agent = |cwd: &std::path::Path, project: &std::path::Path, state: &str| {
        let session = app.request(
            "session.create",
            json!({"kind":"codex","cwd":cwd,"projectRoot":project,"worktreePath":cwd}),
        );
        let id = session["id"].as_str().unwrap().to_owned();
        if state == "ready" {
            app.request("session.set_state", json!({"sessionId":id,"state":"busy"}));
        }
        assert_eq!(
            app.request("session.set_state", json!({"sessionId":id,"state":state}))["state"],
            state
        );
        id
    };
    let hidden_ready = agent(&hidden_project, &hidden_project, "ready");
    agent(&hidden_worktree, &hidden_project, "waiting");
    let visible_ready = agent(app.directory.path(), app.directory.path(), "ready");
    let visible_waiting = agent(app.directory.path(), app.directory.path(), "waiting");
    let idle = agent(app.directory.path(), app.directory.path(), "idle");
    app.request(
        "project.set",
        json!({"root":hidden_project,"isCollapsed":true}),
    );
    app.request("session.select", json!({"sessionId":idle}));

    for expected in [&visible_ready, &visible_waiting, &visible_ready] {
        app.activate_action("next-ready-session");
        assert_eq!(
            app.request("app.get_state", json!({}))["selectedSessionId"],
            expected.as_str()
        );
    }

    app.request(
        "project.set",
        json!({"root":app.directory.path(),"isCollapsed":true}),
    );
    app.activate_action("next-ready-session");
    assert_eq!(
        app.request("app.get_state", json!({}))["selectedSessionId"],
        visible_ready
    );

    app.request(
        "project.set",
        json!({"root":hidden_project,"isCollapsed":false}),
    );
    app.activate_action("next-ready-session");
    assert_eq!(
        app.request("app.get_state", json!({}))["selectedSessionId"],
        hidden_ready
    );
}

#[test]
#[ignore = "requires AGTK_TEST_DISPLAY private Wayland compositor"]
fn relative_and_recent_session_navigation_skip_collapsed_projects() {
    let app = App::new();
    let hidden_project = app.directory.path().join("hidden");
    std::fs::create_dir(&hidden_project).unwrap();
    let session = |project: &std::path::Path| {
        app.request(
            "session.create",
            json!({"kind":"codex","cwd":project,"projectRoot":project}),
        )["id"]
            .as_str()
            .unwrap()
            .to_owned()
    };
    let first = session(app.directory.path());
    let hidden = session(&hidden_project);
    let second = session(app.directory.path());
    for id in [&first, &hidden, &second] {
        app.request("session.select", json!({"sessionId":id}));
    }
    app.request(
        "project.set",
        json!({"root":hidden_project,"isCollapsed":true}),
    );

    for (action, expected) in [
        ("last-session", &first),
        ("last-session", &second),
        ("next-session", &first),
        ("previous-session", &second),
    ] {
        app.activate_action(action);
        assert_eq!(
            app.request("app.get_state", json!({}))["selectedSessionId"],
            expected.as_str()
        );
    }

    // A hidden current session starts navigation from the corresponding end.
    for (action, expected) in [("next-session", &first), ("previous-session", &second)] {
        app.request("session.select", json!({"sessionId":hidden}));
        app.activate_action(action);
        assert_eq!(
            app.request("app.get_state", json!({}))["selectedSessionId"],
            expected.as_str()
        );
    }

    app.request(
        "project.set",
        json!({"root":app.directory.path(),"isCollapsed":true}),
    );
    for action in ["last-session", "next-session", "previous-session"] {
        app.activate_action(action);
        assert_eq!(
            app.request("app.get_state", json!({}))["selectedSessionId"],
            second
        );
    }

    app.request(
        "project.set",
        json!({"root":hidden_project,"isCollapsed":false}),
    );
    app.activate_action("last-session");
    assert_eq!(
        app.request("app.get_state", json!({}))["selectedSessionId"],
        hidden
    );
}

#[test]
#[ignore = "requires AGTK_TEST_DISPLAY private Wayland compositor"]
fn cwd_only_launch_infers_and_persists_its_project_and_worktree() {
    let mut app = App::new();
    let root = app.directory.path().join("project");
    let cwd = root.join("src/deep");
    std::fs::create_dir_all(&cwd).unwrap();
    assert!(
        Command::new("git")
            .args(["init", "--quiet"])
            .arg(&root)
            .status()
            .unwrap()
            .success()
    );
    let session = app.request(
        "session.create",
        json!({
            "kind":"custom",
            "command":"/bin/sh",
            "args":["-c", "exec sleep 30"],
            "cwd":cwd,
            "name":"cwd-only"
        }),
    );
    assert_eq!(session["cwd"], cwd.to_string_lossy().as_ref());
    assert_eq!(session["projectRoot"], root.to_string_lossy().as_ref());
    assert_eq!(session["worktreePath"], root.to_string_lossy().as_ref());

    app.stop();
    app.start();
    let state = app.request("app.get_state", json!({}));
    assert_eq!(
        state["projects"][0]["root"],
        root.to_string_lossy().as_ref()
    );
    let restored = state["sessions"]
        .as_array()
        .unwrap()
        .iter()
        .find(|record| record["id"] == session["id"])
        .unwrap();
    assert_eq!(restored["projectRoot"], session["projectRoot"]);
    assert_eq!(restored["worktreePath"], session["worktreePath"]);
    app.request("session.close", json!({"sessionId":session["id"]}));
}

#[test]
#[ignore = "requires AGTK_TEST_DISPLAY private Wayland compositor"]
fn sessions_group_by_project_and_worktree_with_durable_project_state() {
    let mut app = App::new();
    let worktree = app.directory.path().join("feature-worktree");
    std::fs::create_dir(&worktree).unwrap();
    let main = app.request(
        "session.create",
        json!({
            "kind":"custom",
            "command":"/bin/sh",
            "args":["-c", "exec sleep 30"],
            "name":"main session",
            "cwd":app.directory.path(),
            "projectRoot":app.directory.path()
        }),
    );
    let feature = app.request(
        "session.create",
        json!({
            "kind":"custom",
            "command":"/bin/sh",
            "args":["-c", "exec sleep 30"],
            "name":"feature session",
            "cwd":worktree,
            "projectRoot":app.directory.path(),
            "worktreePath":worktree
        }),
    );
    let root = app.directory.path().to_string_lossy();
    let state = app.request("app.get_state", json!({}));
    assert_eq!(state["projects"].as_array().unwrap().len(), 1);
    assert_eq!(state["projects"][0]["root"], root.as_ref());
    assert_eq!(state["worktreeGroups"].as_array().unwrap().len(), 1);
    assert_eq!(
        state["worktreeGroups"][0]["path"],
        worktree.to_string_lossy().as_ref()
    );

    app.request(
        "project.set",
        json!({"root":root.as_ref(),"isPinned":true,"isCollapsed":true}),
    );
    app.stop();
    app.start();
    let state = app.request("app.get_state", json!({}));
    assert_eq!(state["projects"][0]["isPinned"], true);
    assert_eq!(state["projects"][0]["isCollapsed"], true);
    app.request(
        "session.close",
        json!({"sessionId":main["id"].as_str().unwrap()}),
    );
    app.request(
        "session.close",
        json!({"sessionId":feature["id"].as_str().unwrap()}),
    );
}

#[test]
#[ignore = "requires AGTK_TEST_DISPLAY private Wayland compositor"]
fn a_project_without_sessions_stays_listed_as_inactive_until_removed() {
    let mut app = App::new();
    let project = app.directory.path().join("old-project");
    std::fs::create_dir(&project).unwrap();
    let root = project.to_string_lossy().to_string();
    let listed = |app: &App| {
        app.request("app.get_state", json!({}))["projects"]
            .as_array()
            .unwrap()
            .iter()
            .find(|listed| listed["root"] == root.as_str())
            .cloned()
    };
    let session = app.request(
        "session.create",
        json!({
            "kind":"custom",
            "command":"/bin/sh",
            "args":["-c", "exec sleep 30"],
            "cwd":project,
            "projectRoot":project
        }),
    );
    assert_eq!(listed(&app).unwrap()["isActive"], true);
    match app.request_body("project.remove", json!({"root":root})) {
        ResponseBody::Failure(error) => assert_eq!(error.code, ErrorCode::OperationRefused),
        body => panic!("removed a project that has sessions: {body:?}"),
    }

    app.request(
        "session.close",
        json!({"sessionId":session["id"].as_str().unwrap()}),
    );
    assert_eq!(listed(&app).unwrap()["isActive"], false);
    app.stop();
    app.start();
    assert_eq!(listed(&app).unwrap()["isActive"], false);

    app.request("project.set", json!({"root":root,"isPinned":true}));
    assert_eq!(listed(&app).unwrap()["isActive"], true);
    app.request("project.set", json!({"root":root,"isPinned":false}));
    assert_eq!(listed(&app).unwrap()["isActive"], false);

    let removed = app.request("project.remove", json!({"root":root}));
    assert!(
        removed["projects"]
            .as_array()
            .unwrap()
            .iter()
            .all(|listed| listed["root"] != root.as_str())
    );
    match app.request_body("project.remove", json!({"root":root})) {
        ResponseBody::Failure(error) => assert_eq!(error.code, ErrorCode::InvalidParams),
        body => panic!("removed a project twice: {body:?}"),
    }
    app.stop();
    app.start();
    assert!(listed(&app).is_none());
}

#[test]
#[ignore = "requires AGTK_TEST_DISPLAY private Wayland compositor"]
fn slow_worktree_lookup_does_not_delay_or_overwrite_a_session_rename() {
    let app = App::new();
    let repo = app.directory.path().join("repo");
    let target = app.directory.path().join("target");
    for root in [&repo, &target] {
        assert!(
            Command::new("git")
                .args(["init", "-q"])
                .arg(root)
                .status()
                .unwrap()
                .success()
        );
    }
    let session = app.request(
        "session.create",
        json!({
            "kind":"custom", "command":"/bin/sh", "args":["-c", "exec sleep 30"],
            "cwd":repo, "projectRoot":repo, "worktreePath":repo
        }),
    );
    let id = session["id"].as_str().unwrap().to_owned();
    let bin = app.directory.path().join("bin");
    std::fs::create_dir(&bin).unwrap();
    let git = bin.join("git");
    std::fs::write(
        &git,
        r#"#!/bin/sh
case "$*" in
  *'rev-parse --show-toplevel'*)
    if [ -e "$PWD/.slow-git" ]; then
      : > "$PWD/.slow-git-started"
      while [ -e "$PWD/.slow-git" ]; do sleep 0.02; done
    fi ;;
esac
exec /usr/bin/git "$@"
"#,
    )
    .unwrap();
    std::fs::set_permissions(&git, std::fs::Permissions::from_mode(0o700)).unwrap();
    std::fs::write(target.join(".slow-git"), "").unwrap();
    let request = decode_request(
        &serde_json::to_vec(&json!({
            "version":1, "id":"move", "method":"session.set_worktree",
            "params":{"sessionId":id, "worktreePath":target}
        }))
        .unwrap(),
    )
    .unwrap();
    let client = ControlClient::new(app.paths.control_socket());
    let moving = thread::spawn(move || client.send(&request));
    let deadline = Instant::now() + Duration::from_secs(3);
    while !target.join(".slow-git-started").exists() {
        assert!(Instant::now() < deadline, "worktree lookup did not start");
        thread::sleep(Duration::from_millis(10));
    }
    let rename = decode_request(
        &serde_json::to_vec(&json!({
            "version":1, "id":"rename", "method":"session.rename",
            "params":{"sessionId":id, "name":"chosen name"}
        }))
        .unwrap(),
    )
    .unwrap();
    let renamed = ControlClient::new(app.paths.control_socket())
        .send_with_timeout(&rename, Duration::from_secs(1));
    std::fs::remove_file(target.join(".slow-git")).unwrap();
    let moved = moving.join().unwrap().unwrap();

    assert!(matches!(renamed.unwrap().body, ResponseBody::Success(_)));
    let ResponseBody::Success(moved) = moved.body else {
        panic!("move failed");
    };
    assert_eq!(moved["name"], "chosen name");
    assert_eq!(moved["worktreePath"], target.to_string_lossy().as_ref());
}

#[test]
#[ignore = "requires AGTK_TEST_DISPLAY private Wayland compositor"]
fn a_session_can_move_to_another_worktree_and_keeps_it_across_restart() {
    let mut app = App::new();
    let repo = app.directory.path().join("repo");
    std::fs::create_dir(&repo).unwrap();
    std::fs::write(repo.join("README.md"), "fixture\n").unwrap();
    for arguments in [
        vec!["init", "-b", "main"],
        vec!["config", "user.name", "Agtk Test"],
        vec!["config", "user.email", "agtk@example.invalid"],
        vec!["add", "README.md"],
        vec!["commit", "-m", "fixture"],
        vec!["worktree", "add", "-b", "fix", "../repo-fix"],
    ] {
        let output = Command::new("git")
            .args(arguments)
            .current_dir(&repo)
            .output()
            .unwrap();
        assert!(output.status.success());
    }
    let repo = repo.canonicalize().unwrap();
    let fix = app
        .directory
        .path()
        .join("repo-fix")
        .canonicalize()
        .unwrap();
    let session = app.request(
        "session.create",
        json!({
            "kind":"custom",
            "command":"/bin/sh",
            "args":["-c", "exec sleep 30"],
            "cwd":repo,
            "projectRoot":repo,
            "worktreePath":repo
        }),
    );
    let id = session["id"].as_str().unwrap().to_owned();
    assert_eq!(session["name"], "repo");

    // A registered ancestor must not claim this repository's sibling worktree.
    app.request(
        "project.set",
        json!({"root":app.directory.path(),"isPinned":true}),
    );

    let moved = app.request(
        "session.set_worktree",
        json!({"sessionId":id,"worktreePath":fix}),
    );
    assert_eq!(moved["worktreePath"], fix.to_string_lossy().as_ref());
    assert_eq!(moved["projectRoot"], repo.to_string_lossy().as_ref());
    assert_eq!(
        moved["name"], "repo-fix",
        "a generated name follows the worktree"
    );
    let state = app.request("app.get_state", json!({}));
    assert_eq!(
        state["worktreeGroups"][0]["path"],
        fix.to_string_lossy().as_ref()
    );

    let refused = app.request_body(
        "session.set_worktree",
        json!({"sessionId":id,"worktreePath":app.directory.path()}),
    );
    assert!(
        matches!(refused, ResponseBody::Failure(ref error) if error.code == ErrorCode::OperationRefused),
        "{refused:?}"
    );

    app.stop();
    app.start();
    let state = app.request("app.get_state", json!({}));
    let recovered = state["sessions"]
        .as_array()
        .unwrap()
        .iter()
        .find(|s| s["id"] == id)
        .unwrap();
    assert_eq!(recovered["worktreePath"], fix.to_string_lossy().as_ref());
    assert_eq!(recovered["projectRoot"], repo.to_string_lossy().as_ref());
    app.request("session.close", json!({"sessionId":id}));
}

#[test]
#[ignore = "requires AGTK_TEST_DISPLAY private Wayland compositor"]
fn closing_the_last_session_in_a_worktree_offers_to_remove_it() {
    fn find<'a>(node: &'a Value, id: &str) -> Option<&'a Value> {
        if node["id"] == id {
            return Some(node);
        }
        node["children"]
            .as_array()?
            .iter()
            .find_map(|child| find(child, id))
    }

    let app = App::new();
    let repo = app.directory.path().join("repo");
    std::fs::create_dir(&repo).unwrap();
    std::fs::write(repo.join("README.md"), "fixture\n").unwrap();
    for arguments in [
        vec!["init", "-b", "main"],
        vec!["config", "user.name", "Agtk Test"],
        vec!["config", "user.email", "agtk@example.invalid"],
        vec!["add", "README.md"],
        vec!["commit", "-m", "fixture"],
        vec!["worktree", "add", "-b", "fix", "../repo-fix"],
        vec!["worktree", "add", "--detach", "../repo-missing"],
    ] {
        let output = Command::new("git")
            .args(arguments)
            .current_dir(&repo)
            .output()
            .unwrap();
        assert!(output.status.success());
    }
    // A stale registration for another checkout must not block this one's scan.
    std::fs::remove_dir_all(app.directory.path().join("repo-missing")).unwrap();
    let repo = repo.canonicalize().unwrap();
    let fix = app
        .directory
        .path()
        .join("repo-fix")
        .canonicalize()
        .unwrap();
    std::fs::write(fix.join("notes.txt"), "uncommitted\n").unwrap();
    let create = |app: &App, worktree: &std::path::Path| {
        let session = app.request(
            "session.create",
            json!({
                "kind":"custom",
                "command":"/bin/sh",
                "args":["-c", "exec sleep 30"],
                "cwd":worktree,
                "projectRoot":repo,
                "worktreePath":worktree
            }),
        );
        session["id"].as_str().unwrap().to_owned()
    };

    // The main checkout never offers removal.
    let main = create(&app, &repo);
    app.request("session.select", json!({"sessionId":main}));
    let shown = app.request("ui.show", json!({"surface":"close-session"}));
    assert_eq!(shown["shown"], false);

    let first = create(&app, &fix);
    let second = create(&app, &fix);
    app.request("session.select", json!({"sessionId":first}));
    let shown = app.request("ui.show", json!({"surface":"close-session"}));
    assert_eq!(
        shown["shown"], false,
        "another session still uses the worktree"
    );
    app.request("session.close", json!({"sessionId":second}));

    let shown = app.request("ui.show", json!({"surface":"close-session"}));
    assert_eq!(shown["shown"], true);
    let deadline = Instant::now() + Duration::from_secs(10);
    let prompt = loop {
        let inspection = app.request("ui.inspect", json!({}));
        let prompt = find(&inspection["root"], "close-prompt")
            .expect("close prompt is open")
            .clone();
        assert_eq!(prompt["isVisible"], true);
        assert_eq!(
            find(&prompt, "close-prompt-remove").unwrap()["isEnabled"],
            true
        );
        assert_eq!(
            find(&prompt, "close-prompt-remove-regardless").unwrap()["isEnabled"],
            true
        );
        if find(&prompt, "close-prompt-status").unwrap()["label"] != "Checking the worktree…" {
            break prompt;
        }
        assert!(
            Instant::now() < deadline,
            "worktree scan did not finish: {prompt}"
        );
        thread::sleep(Duration::from_millis(50));
    };
    let status = find(&prompt, "close-prompt-status").unwrap()["label"]
        .as_str()
        .unwrap()
        .to_owned();
    assert!(status.contains("uncommitted changes"), "{status}");
    let branch = find(&prompt, "close-prompt-delete-branch").unwrap();
    assert_eq!(branch["isVisible"], true);
    assert_eq!(branch["label"], "Also delete branch fix");
    assert_eq!(
        branch["isSelected"], false,
        "an unmerged branch is kept by default"
    );
    thread::sleep(Duration::from_millis(50));
    let capture = app.request("ui.capture", json!({}));
    if let Ok(target) = std::env::var("AGTK_TEST_CAPTURE_DIR") {
        let path = std::path::Path::new(capture["path"].as_str().unwrap());
        let _ = std::fs::copy(path, std::path::Path::new(&target).join("close-prompt.png"));
    }

    app.request("session.close", json!({"sessionId":first}));
    app.request("session.close", json!({"sessionId":main}));
    assert!(fix.exists(), "control closes never remove worktrees");
}

#[test]
#[ignore = "requires AGTK_TEST_DISPLAY private Wayland compositor"]
fn control_api_completes_disposable_worktree_lifecycle() {
    let app = App::new();
    let repo = app.directory.path().join("repo");
    std::fs::create_dir(&repo).unwrap();
    for arguments in [
        vec!["init", "-b", "main"],
        vec!["config", "user.name", "Agtk Test"],
        vec!["config", "user.email", "agtk@example.invalid"],
    ] {
        let output = Command::new("git")
            .args(arguments)
            .current_dir(&repo)
            .output()
            .unwrap();
        assert!(output.status.success());
    }
    std::fs::write(repo.join("README.md"), "fixture\n").unwrap();
    for arguments in [vec!["add", "README.md"], vec!["commit", "-m", "fixture"]] {
        let output = Command::new("git")
            .args(arguments)
            .current_dir(&repo)
            .output()
            .unwrap();
        assert!(output.status.success());
    }

    let initial = app.request("worktree.list", json!({"projectRoot":repo}));
    assert_eq!(initial["worktrees"].as_array().unwrap().len(), 1);
    app.request("project.set", json!({"root":repo,"isPinned":true}));
    assert_eq!(
        app.request("ui.show", json!({"surface":"worktrees"}))["shown"],
        true
    );
    thread::sleep(Duration::from_millis(50));
    let worktree_surface = app.request("ui.capture", json!({}));
    assert!(worktree_surface["path"].as_str().is_some());
    let created = app.request(
        "worktree.create",
        json!({
            "projectRoot":repo,
            "branch":"native-control",
            "baseBranch":"main",
            "purpose":"exercise isolated native control"
        }),
    );
    let created_path = created["path"].as_str().unwrap();
    assert!(std::path::Path::new(created_path).exists());
    for arguments in [
        vec!["config", "branch.native-control.remote", "origin"],
        vec![
            "config",
            "branch.native-control.merge",
            "refs/heads/native-control",
        ],
    ] {
        let output = Command::new("git")
            .args(arguments)
            .current_dir(&repo)
            .output()
            .unwrap();
        assert!(output.status.success());
    }

    let inventory = app.request("worktree.list", json!({"projectRoot":repo}));
    let preview = inventory["worktrees"]
        .as_array()
        .unwrap()
        .iter()
        .find(|worktree| worktree["path"] == created_path)
        .unwrap();
    let result = app.request(
        "worktree.reap",
        json!({
            "path":created_path,
            "expectedHead":preview["head"],
            "expectedStatusHash":preview["statusHash"],
            "deleteBranch":"force"
        }),
    );
    assert_eq!(result["ok"], true);
    assert_eq!(result["branchDeleted"], true);
    assert!(!std::path::Path::new(created_path).exists());
    assert!(result["atticTag"].as_str().unwrap().starts_with("attic/"));
}

#[test]
#[ignore = "requires AGTK_TEST_DISPLAY private Wayland compositor"]
fn recent_agent_surface_uses_isolated_logs_and_readiness_survives_restart() {
    let mut app = App::new();
    let codex_logs = app.directory.path().join("codex/sessions/2026/09/15");
    let project = app.directory.path().join("provider-project");
    std::fs::create_dir_all(&codex_logs).unwrap();
    std::fs::create_dir_all(&project).unwrap();
    std::fs::write(
        codex_logs.join("session.jsonl"),
        format!(
            "{{\"type\":\"session_meta\",\"payload\":{{\"id\":\"codex-native-1\",\"cwd\":{:?}}}}}\n{{\"type\":\"response_item\",\"payload\":{{\"role\":\"user\",\"content\":\"Inspect the restored native workspace\"}}}}\n{{\"type\":\"response_item\",\"payload\":{{\"role\":\"assistant\",\"content\":\"The workspace is ready.\"}}}}\n",
            project.to_string_lossy()
        ),
    )
    .unwrap();
    let discovered = app.request("agent.list", json!({"limit":20,"maxAgeDays":30}));
    assert_eq!(discovered.as_array().unwrap().len(), 1);
    assert_eq!(discovered[0]["providerSessionId"], "codex-native-1");
    let preview = app.request(
        "agent.preview",
        json!({"provider":"codex","providerSessionId":"codex-native-1","maxMessages":10}),
    );
    assert_eq!(preview["messages"].as_array().unwrap().len(), 2);
    assert_eq!(
        app.request("ui.show", json!({"surface":"agents"}))["shown"],
        true
    );
    thread::sleep(Duration::from_millis(120));
    let capture = app.request("ui.capture", json!({}));
    assert!(capture["width"].as_i64().unwrap() <= 1400);
    assert!(capture["height"].as_i64().unwrap() <= 900);

    let restored = app.request(
        "agent.restore",
        json!({
            "provider":"codex",
            "providerSessionId":"codex-native-1",
            "cwd":project,
            "projectRoot":project,
            "name":"restored provider"
        }),
    );
    let restored_id = restored["id"].as_str().unwrap();
    assert_eq!(restored["state"], "idle");
    app.wait_text(restored_id, "__RESTORE_ARGS_resume_codex-native-1__");
    assert!(
        app.request("agent.list", json!({"limit":20,"maxAgeDays":30}))
            .as_array()
            .unwrap()
            .is_empty()
    );
    app.request("session.close", json!({"sessionId":restored_id}));

    let session = app.request(
        "session.create",
        json!({
            "kind":"codex",
            "command":"/bin/sh",
            "args":["-c","exec sleep 30"],
            "cwd":project,
            "name":"readiness probe"
        }),
    );
    let id = session["id"].as_str().unwrap();
    assert_eq!(session["state"], "idle");
    // A turn that finishes in front of the user counts as viewed, so look elsewhere.
    let other = app.request(
        "session.create",
        json!({
            "kind":"custom",
            "command":"/bin/sh",
            "args":["-c","exec sleep 30"],
            "cwd":project,
            "name":"elsewhere"
        }),
    );
    let other_id = other["id"].as_str().unwrap();
    let busy = app.request("session.set_state", json!({"sessionId":id,"state":"busy"}));
    assert_eq!(busy["state"], "busy");
    let ready = app.request("session.set_state", json!({"sessionId":id,"state":"ready"}));
    assert_eq!(ready["state"], "ready");
    assert_eq!(
        app.request("app.get_state", json!({}))["attention"]["count"],
        1
    );
    app.stop();
    app.start();
    let state = app.request("app.get_state", json!({}));
    let probe = state["sessions"]
        .as_array()
        .unwrap()
        .iter()
        .find(|s| s["id"] == id)
        .unwrap();
    assert_eq!(probe["state"], "ready");
    app.request("session.close", json!({"sessionId":other_id}));
    app.request("session.close", json!({"sessionId":id}));
}

#[test]
#[ignore = "requires a private display"]
fn an_exited_agent_offers_to_resume_its_reported_conversation() {
    let mut app = App::new();
    let session = app.request(
        "session.create",
        json!({
            "kind":"codex",
            "command":"/bin/sh",
            "args":["-c","printf '__READY__\\n'; read line"],
            "cwd":app.directory.path(),
            "name":"resumable"
        }),
    );
    let id = session["id"].as_str().unwrap();
    app.wait_text(id, "__READY__");
    app.request(
        "session.set_state",
        json!({"sessionId":id,"state":"busy","conversationId":"codex-conv-1"}),
    );
    app.request(
        "session.send_input",
        json!({"sessionId":id,"text":"done","appendEnter":true}),
    );
    app.wait_text(id, "Press Enter to resume the conversation");

    // The reported conversation is saved with the row, so it outlives the UI.
    app.stop();
    app.start();
    app.wait_text(id, "Press Enter to resume the conversation");
}

#[test]
#[ignore = "requires a private display"]
fn a_restarted_agent_resumes_its_conversation_in_the_same_place() {
    let app = App::new();
    let agent = |name: &str| {
        let session = app.request(
            "session.create",
            json!({
                "kind":"codex",
                "command":"/bin/sh",
                "args":["-c","printf '__READY__\\n'; read line"],
                "cwd":app.directory.path(),
                "name":name
            }),
        );
        let id = session["id"].as_str().unwrap().to_owned();
        app.wait_text(&id, "__READY__");
        id
    };
    let first = agent("first");
    let second = agent("second");
    for (id, conversation) in [(&first, "codex-conv-1"), (&second, "codex-conv-2")] {
        app.request(
            "session.set_state",
            json!({"sessionId":id,"state":"idle","conversationId":conversation}),
        );
    }
    // Only the first conversation has a log, so only it can be resumed.
    let logs = app.directory.path().join("codex/sessions/2026/09/30");
    std::fs::create_dir_all(&logs).unwrap();
    std::fs::write(
        logs.join("rollout-2026-09-30T10-00-00-codex-conv-1.jsonl"),
        "{\"type\":\"session_meta\",\"payload\":{\"id\":\"codex-conv-1\"}}\n",
    )
    .unwrap();

    let restarted = app.request("session.restart", json!({"sessionId":first}));
    let restarted = restarted["id"].as_str().unwrap();
    assert_ne!(restarted, first);
    app.wait_text(restarted, "__RESTORE_ARGS_resume_codex-conv-1__");
    let fresh = app.request("session.restart", json!({"sessionId":second}));
    app.wait_text(fresh["id"].as_str().unwrap(), "__RESTORE_ARGS___");

    let state = app.request("app.get_state", json!({}));
    let names = state["sessions"]
        .as_array()
        .unwrap()
        .iter()
        .map(|session| session["name"].as_str().unwrap())
        .collect::<Vec<_>>();
    assert_eq!(names, ["first", "second"]);
    assert!(
        state["sessions"]
            .as_array()
            .unwrap()
            .iter()
            .all(|session| session["id"] != first.as_str() && session["id"] != second.as_str())
    );
}

#[test]
#[ignore = "requires a private display"]
fn a_shell_runs_its_initial_command_once_it_starts() {
    let app = App::new();
    let session = app.request(
        "session.create",
        json!({
            "kind":"shell",
            "cwd":app.directory.path(),
            "initialInput":"echo __SHELL_$((6*7))__"
        }),
    );
    // The shell computes 42, so this is its output, not the echoed input.
    app.wait_text(session["id"].as_str().unwrap(), "__SHELL_42__");
}

#[test]
#[ignore = "requires a private display"]
fn a_forked_agent_copies_its_conversation_into_a_new_row_below_it() {
    let app = App::new();
    let agent = |kind: &str, name: &str| {
        let session = app.request(
            "session.create",
            json!({
                "kind":kind,
                "command":"/bin/sh",
                "args":["-c","printf '__READY__\\n'; read line"],
                "cwd":app.directory.path(),
                "name":name
            }),
        );
        let id = session["id"].as_str().unwrap().to_owned();
        app.wait_text(&id, "__READY__");
        id
    };
    let claude = agent("claude", "claude");
    let codex = agent("codex", "codex");
    let fresh = agent("claude", "fresh");
    for (id, conversation) in [
        (&claude, "claude-conv-1"),
        (&codex, "codex-conv-1"),
        (&fresh, "claude-conv-2"),
    ] {
        app.request(
            "session.set_state",
            json!({"sessionId":id,"state":"idle","conversationId":conversation}),
        );
    }
    // A conversation without a log has not started, so it has nothing to copy.
    let claude_logs = app.directory.path().join("claude/projects/demo");
    std::fs::create_dir_all(&claude_logs).unwrap();
    std::fs::write(
        claude_logs.join("claude-conv-1.jsonl"),
        "{\"type\":\"user\",\"sessionId\":\"claude-conv-1\"}\n",
    )
    .unwrap();
    let codex_logs = app.directory.path().join("codex/sessions/2026/09/30");
    std::fs::create_dir_all(&codex_logs).unwrap();
    std::fs::write(
        codex_logs.join("rollout-2026-09-30T10-00-00-codex-conv-1.jsonl"),
        "{\"type\":\"session_meta\",\"payload\":{\"id\":\"codex-conv-1\"}}\n",
    )
    .unwrap();

    let claude_fork = app.request("session.fork", json!({"sessionId":claude}));
    let claude_fork = claude_fork["id"].as_str().unwrap().to_owned();
    app.wait_text(&claude_fork, "__RESTORE_ARGS_--resume_claude-conv-1__");
    let codex_fork = app.request("session.fork", json!({"sessionId":codex}));
    let codex_fork = codex_fork["id"].as_str().unwrap().to_owned();
    app.wait_text(&codex_fork, "__RESTORE_ARGS_fork_codex-conv-1__");
    match app.request_body("session.fork", json!({"sessionId":fresh})) {
        ResponseBody::Failure(error) => assert_eq!(error.code, ErrorCode::OperationRefused),
        body => panic!("forked a conversation that has no log: {body:?}"),
    }

    // Each fork sits below its original, which keeps running and keeps the selection.
    let state = app.request("app.get_state", json!({}));
    assert_eq!(state["selectedSessionId"], claude.as_str());
    let mut sessions = state["sessions"].as_array().unwrap().clone();
    sessions.sort_by_key(|session| session["position"].as_i64().unwrap());
    let names = sessions
        .iter()
        .map(|session| session["name"].as_str().unwrap())
        .collect::<Vec<_>>();
    assert_eq!(
        names,
        ["claude", "claude fork", "codex", "codex fork", "fresh"]
    );
    assert!(sessions.iter().all(|session| session["state"] != "exited"));

    // The Claude fork holds its own conversation from the start, so a restart
    // resumes the copy. Codex names its copy itself, found later in the logs.
    let records = Store::open(&app.paths.database())
        .unwrap()
        .sessions()
        .unwrap();
    let record = |id: &str| records.iter().find(|record| record.id == id).unwrap();
    let copy = record(&claude_fork).conversation_id.clone().unwrap();
    assert_ne!(copy, "claude-conv-1");
    assert_eq!(
        record(&claude_fork).args,
        [
            "--resume",
            "claude-conv-1",
            "--fork-session",
            "--session-id",
            copy.as_str()
        ]
    );
    assert_eq!(record(&codex_fork).conversation_id, None);
    assert_eq!(
        record(&claude).conversation_id.as_deref(),
        Some("claude-conv-1")
    );
}

#[test]
#[ignore = "requires a private display"]
fn sessions_that_lost_their_host_are_offered_for_resume_at_startup() {
    fn find<'a>(node: &'a Value, id: &str) -> Option<&'a Value> {
        if node["id"] == id {
            return Some(node);
        }
        node["children"]
            .as_array()?
            .iter()
            .find_map(|child| find(child, id))
    }

    let mut app = App::new();
    let create = |app: &App, kind: &str, name: &str, script: &str| {
        let session = app.request(
            "session.create",
            json!({
                "kind":kind,
                "command":"/bin/sh",
                "args":["-c",script],
                "cwd":app.directory.path(),
                "name":name
            }),
        );
        session["id"].as_str().unwrap().to_owned()
    };
    let agent = create(&app, "codex", "agent", "printf '__READY__\\n'; read line");
    app.wait_text(&agent, "__READY__");
    app.request(
        "session.set_state",
        json!({"sessionId":agent,"state":"idle","conversationId":"codex-conv-1"}),
    );
    let shell = create(&app, "shell", "shell", "printf '__SHELL__\\n'; read line");
    app.wait_text(&shell, "__SHELL__");
    // A session that ended on its own was not open, so it is not offered.
    let finished = create(&app, "shell", "finished", "sleep 0.3");
    let deadline = Instant::now() + Duration::from_secs(5);
    loop {
        let state = app.request("app.get_state", json!({}));
        let exited = state["sessions"]
            .as_array()
            .unwrap()
            .iter()
            .any(|session| session["id"] == finished.as_str() && session["state"] == "exited");
        if exited {
            break;
        }
        assert!(Instant::now() < deadline, "finished session did not exit");
        thread::sleep(Duration::from_millis(30));
    }

    app.stop();
    app.kill_hosts();
    app.start();

    let state = app.request("app.get_state", json!({}));
    for session in state["sessions"].as_array().unwrap() {
        assert_eq!(session["state"], "exited", "{session}");
    }
    let inspection = app.request("ui.inspect", json!({}));
    let prompt = find(&inspection["root"], "resume-prompt").expect("resume prompt is offered");
    assert_eq!(prompt["isVisible"], true);
    let offered = prompt["children"]
        .as_array()
        .unwrap()
        .iter()
        .map(|choice| {
            assert_eq!(choice["isSelected"], true, "{choice}");
            choice["id"].as_str().unwrap().to_owned()
        })
        .collect::<Vec<_>>();
    assert_eq!(
        offered,
        [
            format!("resume-session-{agent}"),
            format!("resume-session-{shell}")
        ]
    );
    thread::sleep(Duration::from_millis(50));
    let capture = app.request("ui.capture", json!({}));
    if let Ok(target) = std::env::var("AGTK_TEST_CAPTURE_DIR") {
        let path = std::path::Path::new(capture["path"].as_str().unwrap());
        let _ = std::fs::copy(
            path,
            std::path::Path::new(&target).join("resume-prompt.png"),
        );
    }

    // Rows that were already exited at startup are not offered again.
    app.stop();
    app.start();
    let inspection = app.request("ui.inspect", json!({}));
    assert!(find(&inspection["root"], "resume-prompt").is_none());
}

#[test]
#[ignore = "requires AGTK_TEST_DISPLAY private Wayland compositor"]
fn files_page_lists_the_worktree_and_opens_files_read_only() {
    fn find<'a>(node: &'a Value, id: &str) -> Option<&'a Value> {
        if node["id"] == id {
            return Some(node);
        }
        node["children"]
            .as_array()?
            .iter()
            .find_map(|child| find(child, id))
    }
    fn git(project: &std::path::Path, args: &[&str]) {
        assert!(
            Command::new("git")
                .args(args)
                .current_dir(project)
                .status()
                .unwrap()
                .success()
        );
    }

    let app = App::new();
    let project = app.directory.path().join("files-project");
    std::fs::create_dir_all(project.join("src")).unwrap();
    git(&project, &["init", "-q"]);
    git(&project, &["config", "user.name", "Test"]);
    git(&project, &["config", "user.email", "test@example.com"]);
    std::fs::write(project.join("src/lib.rs"), "fn one() {}\nfn two() {}\n").unwrap();
    std::fs::write(project.join(".gitignore"), "target/\n").unwrap();
    git(&project, &["add", "."]);
    git(&project, &["commit", "-qm", "base"]);
    std::fs::create_dir(project.join("target")).unwrap();
    std::fs::write(project.join("target/ignored"), "x").unwrap();
    std::fs::write(project.join("notes.md"), "untracked\n").unwrap();
    let session = app.request(
        "session.create",
        json!({
            "kind":"custom",
            "command":"/bin/sh",
            "args":["-c","exec sleep 30"],
            "cwd":project,
            "projectRoot":project,
            "worktreePath":project,
            "name":"files probe"
        }),
    );
    let id = session["id"].as_str().unwrap();
    assert_eq!(
        app.request("ui.show", json!({"surface":"files"}))["shown"],
        true
    );

    let wait_for = |id: &str, needle: &str| -> Value {
        let deadline = Instant::now() + Duration::from_secs(10);
        let mut last = Value::Null;
        while Instant::now() < deadline {
            let inspection = app.request("ui.inspect", json!({}));
            last = find(&inspection["root"], id)
                .cloned()
                .unwrap_or(Value::Null);
            if last["label"]
                .as_str()
                .is_some_and(|label| label.contains(needle))
            {
                return last;
            }
            thread::sleep(Duration::from_millis(50));
        }
        panic!("{id} never showed {needle}: {last}");
    };
    // .gitignore, notes.md and src/lib.rs; target/ is ignored.
    let page = wait_for("files-page", "3 files");
    assert_eq!(page["isVisible"], true);
    assert_eq!(wait_for("changes-header", "Files")["role"], "tablist");
    wait_for("files-tree", "tree: 3 rows");

    let opened = app.request(
        "ui.open_file",
        json!({"sessionId":id,"path":"src/lib.rs","line":2,"column":4}),
    );
    assert_eq!(opened["opened"], true);
    wait_for("diff-viewer", "rendered: 3 lines");
    let tab = wait_for(opened["tabId"].as_str().unwrap(), "src/lib.rs (File)");
    assert_eq!(tab["isSelected"], true);

    // The same file-opening route handles image bytes and returns to text cleanly.
    std::fs::write(project.join("image.png"), include_bytes!("../galaxy.png")).unwrap();
    app.request("ui.open_file", json!({"sessionId":id,"path":"image.png"}));
    wait_for("diff-viewer", "rendered: image 512 × 512");
    // Cross the text bridge's size limit with a valid image, and retain SVG's
    // intrinsic dimensions even though the viewer fits it into a smaller pane.
    let mut svg = br#"<svg xmlns="http://www.w3.org/2000/svg" width="1600" height="900"><rect width="1600" height="900" fill="red"/></svg>"#.to_vec();
    svg.resize(4 * 1024 * 1024, b' ');
    std::fs::write(project.join("large.svg"), svg).unwrap();
    app.request("ui.open_file", json!({"sessionId":id,"path":"large.svg"}));
    wait_for("diff-viewer", "rendered: image 1600 × 900");
    std::fs::write(project.join("bad.png"), b"not an image").unwrap();
    app.request("ui.open_file", json!({"sessionId":id,"path":"bad.png"}));
    wait_for("diff-viewer", "error: Could not display this image");
    app.request("ui.open_file", json!({"sessionId":id,"path":"src/lib.rs"}));
    wait_for("diff-viewer", "rendered: 3 lines");

    let missing = app.request_body(
        "ui.open_file",
        json!({"sessionId":id,"path":"src/missing.rs"}),
    );
    assert!(matches!(missing, ResponseBody::Failure(_)), "{missing:?}");
    app.request("session.close", json!({"sessionId":id}));
}
