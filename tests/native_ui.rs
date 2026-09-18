//! Run against a private compositor, never the user's display:
//! AGMUX_TEST_DISPLAY=/absolute/path/to/wayland-socket cargo test --test native_ui -- --ignored
use std::io::Write;
use std::os::unix::fs::PermissionsExt;
use std::os::unix::net::UnixStream;
use std::process::{Child, Command, Stdio};
use std::sync::atomic::{AtomicU64, Ordering};
use std::sync::{Mutex, MutexGuard};
use std::thread;
use std::time::{Duration, Instant};

use agmux_native::control::{ControlClient, ErrorCode, ResponseBody, decode_request};
use agmux_native::instance::{InstanceName, InstancePaths};
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
            "#!/bin/sh\nprintf '%s\\n' \"$@\" > \"$AGMUX_EMACS_ARGS_FILE\"\n",
        )
        .unwrap();
        std::fs::set_permissions(&fake_emacs, std::fs::Permissions::from_mode(0o700)).unwrap();
        let fake_az = directory.path().join("fake-az");
        std::fs::write(
            &fake_az,
            "#!/bin/sh\ncase \"$1 $2 $3\" in\n  'account show --query') printf '%s\\n' 'reviewer@example.com' ;;\n  'repos pr list') cat \"$AGMUX_AZURE_PRS_FILE\" ;;\n  'devops invoke --org') cat \"$AGMUX_AZURE_THREADS_FILE\" ;;\n  *) printf '%s\\n' 'unexpected az command' >&2; exit 2 ;;\nesac\n",
        )
        .unwrap();
        std::fs::set_permissions(&fake_az, std::fs::Permissions::from_mode(0o700)).unwrap();
        std::fs::write(directory.path().join("azure-prs.json"), "[]").unwrap();
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
                .env("CLAUDE_CONFIG_DIR", self.directory.path().join("claude"))
                .env("CODEX_HOME", self.directory.path().join("codex"))
                .env("AGMUX_CODEX_BIN", self.directory.path().join("fake-agent"))
                .env("AGMUX_CLAUDE_BIN", self.directory.path().join("fake-agent"))
                .env(
                    "AGMUX_EMACSCLIENT",
                    self.directory.path().join("fake-emacsclient"),
                )
                .env(
                    "AGMUX_EMACS_ARGS_FILE",
                    self.directory.path().join("emacs-args"),
                )
                .env("AGMUX_AZURE_BIN", self.directory.path().join("fake-az"))
                .env(
                    "AGMUX_AZURE_PRS_FILE",
                    self.directory.path().join("azure-prs.json"),
                )
                .env(
                    "AGMUX_AZURE_THREADS_FILE",
                    self.directory.path().join("azure-threads.json"),
                )
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
    assert_eq!(ids, ["sidebar", "terminal-pane"]);
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
    assert_eq!(context_ids, ["branch-review", "magit", "context-input"]);
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
            "launch-project-input",
            "launch-worktree-input",
            "launch-agent",
            "launch-branch",
            "launch-base-branch",
            "launch-cancel",
            "launch-submit"
        ]
    );
    thread::sleep(Duration::from_millis(50));
    let transient = app.request("ui.capture", json!({}));
    assert!(transient["width"].as_i64().unwrap() < 1280);
    assert!(transient["height"].as_i64().unwrap() < 800);
}

#[test]
#[ignore = "requires AGMUX_TEST_DISPLAY private Wayland compositor"]
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
    git(&["config", "user.name", "agmux test"]);
    git(&["config", "user.email", "agmux@example.test"]);
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
        let labels = launch["children"]
            .as_array()
            .unwrap()
            .iter()
            .filter_map(|node| node["label"].as_str())
            .collect::<Vec<_>>();
        if labels
            .iter()
            .any(|label| label.starts_with("Known project directories (1)"))
            && labels
                .iter()
                .any(|label| label.starts_with("Known Git worktrees (2)"))
        {
            app.request("session.close", json!({"sessionId":id}));
            return;
        }
        thread::sleep(Duration::from_millis(30));
    }
    panic!("launch choices were not populated");
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
#[ignore = "requires AGMUX_TEST_DISPLAY private Wayland compositor"]
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
#[ignore = "requires AGMUX_TEST_DISPLAY private Wayland compositor"]
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
    app.wait_text(&id, "__RESTORE_ARGS___");

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
    assert!(capture["width"].as_i64().unwrap() < 1280);

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
#[ignore = "requires AGMUX_TEST_DISPLAY private Wayland compositor"]
fn azure_pr_attention_and_review_launch_use_the_project_context() {
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
    std::fs::write(project.join("tracked"), "base").unwrap();
    for args in [
        &["add", "tracked"][..],
        &["commit", "-qm", "base"][..],
        &["branch", "-M", "feature"][..],
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

    let review = app.request(
        "pr.launch_review",
        json!({"projectRoot":project,"pullRequestId":42}),
    );
    assert_eq!(review["name"], "review: PR #42");
    assert_eq!(review["cwd"], project.to_str().unwrap());
    let review_id = review["id"].as_str().unwrap();

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
        capture["width"].as_i64().unwrap() < 1280,
        "unexpected capture: {capture}"
    );
    assert!(capture["height"].as_i64().unwrap() < 800);

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
    app.request("session.close", json!({"sessionId":review_id}));
    app.request("session.close", json!({"sessionId":auto_review_id}));
}

#[test]
#[ignore = "requires AGMUX_TEST_DISPLAY private Wayland compositor"]
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
#[ignore = "requires AGMUX_TEST_DISPLAY private Wayland compositor"]
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

#[test]
#[ignore = "requires AGMUX_TEST_DISPLAY private Wayland compositor"]
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
#[ignore = "requires AGMUX_TEST_DISPLAY private Wayland compositor"]
fn control_api_completes_disposable_worktree_lifecycle() {
    let app = App::new();
    let repo = app.directory.path().join("repo");
    std::fs::create_dir(&repo).unwrap();
    for arguments in [
        vec!["init", "-b", "main"],
        vec!["config", "user.name", "Agmux Test"],
        vec!["config", "user.email", "agmux@example.invalid"],
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
    assert!(worktree_surface["width"].as_i64().unwrap() < 1280);
    assert!(worktree_surface["height"].as_i64().unwrap() < 800);
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
#[ignore = "requires AGMUX_TEST_DISPLAY private Wayland compositor"]
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
    assert!(capture["width"].as_i64().unwrap() < 1280);
    assert!(capture["height"].as_i64().unwrap() < 800);

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
    assert_eq!(restored["state"], "busy");
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
    assert_eq!(session["state"], "busy");
    let ready = app.request("session.set_state", json!({"sessionId":id,"state":"ready"}));
    assert_eq!(ready["state"], "ready");
    assert_eq!(
        app.request("app.get_state", json!({}))["attention"]["count"],
        1
    );
    app.stop();
    app.start();
    let state = app.request("app.get_state", json!({}));
    assert_eq!(state["sessions"][0]["state"], "ready");
    app.request("session.close", json!({"sessionId":id}));
}
