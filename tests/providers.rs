use std::collections::BTreeSet;
use std::fs;

use agtk::control::SessionKind;
use agtk::providers::{
    AgentProvider, DiscoveryRoots, ProviderDiscovery, RestoreTarget, recent_mutated_paths,
};

#[test]
fn discovery_deduplicates_recent_logs_and_excludes_live_conversations() {
    let fixture = tempfile::tempdir().unwrap();
    let claude = fixture.path().join("claude");
    let codex = fixture.path().join("codex");
    let project = fixture.path().join("project");
    fs::create_dir_all(claude.join("projects/demo")).unwrap();
    fs::create_dir_all(codex.join("sessions/2026/09/15")).unwrap();
    fs::create_dir_all(&project).unwrap();

    fs::write(
        claude.join("projects/demo/claude-1.jsonl"),
        format!(
            "{{\"type\":\"user\",\"sessionId\":\"claude-1\",\"cwd\":{:?},\"message\":{{\"content\":[{{\"type\":\"text\",\"text\":\"Please investigate the native sidebar rendering\"}}]}}}}\n",
            project.to_string_lossy()
        ),
    )
    .unwrap();
    let codex_log = format!(
        "{{\"type\":\"session_meta\",\"payload\":{{\"id\":\"codex-1\",\"cwd\":{:?}}}}}\n{{\"type\":\"response_item\",\"payload\":{{\"role\":\"user\",\"content\":[{{\"type\":\"input_text\",\"text\":\"Build a robust provider restoration flow\"}}]}}}}\n",
        project.to_string_lossy()
    );
    fs::write(
        codex.join("sessions/2026/09/15/rollout-a.jsonl"),
        &codex_log,
    )
    .unwrap();
    fs::write(
        codex.join("sessions/2026/09/15/rollout-b.jsonl"),
        &codex_log,
    )
    .unwrap();
    fs::write(
        codex.join("sessions/2026/09/15/ancillary.jsonl"),
        "{\"type\":\"session_meta\",\"payload\":{\"id\":\"subagent\",\"source\":{\"thread_spawn\":{}}}}\n",
    )
    .unwrap();
    fs::write(
        codex.join("sessions/2026/09/15/malformed-id.jsonl"),
        "{\"type\":\"session_meta\",\"payload\":{\"id\":\"invalid session id\"}}\n",
    )
    .unwrap();

    let discovery = ProviderDiscovery::new(
        DiscoveryRoots {
            claude_config_dir: claude,
            codex_home_dir: codex,
        },
        500,
        90 * 24 * 60 * 60 * 1_000,
    );
    let now = std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .unwrap()
        .as_millis() as u64;
    let mut live = BTreeSet::new();
    live.insert((AgentProvider::Claude, "claude-1".to_owned()));
    let sessions = discovery.discover(now, &live).unwrap();

    assert!(
        discovery
            .has_log(AgentProvider::Claude, "claude-1")
            .unwrap()
    );
    assert!(discovery.has_log(AgentProvider::Codex, "codex-1").unwrap());
    assert!(!discovery.has_log(AgentProvider::Codex, "claude-1").unwrap());
    assert!(
        !discovery
            .has_log(AgentProvider::Claude, "never-started")
            .unwrap()
    );

    assert_eq!(sessions.len(), 1);
    assert_eq!(sessions[0].provider, AgentProvider::Codex);
    assert_eq!(sessions[0].provider_session_id, "codex-1");
    assert_eq!(sessions[0].name, "Build a robust provider restoration flow");
    assert_eq!(sessions[0].cwd.as_ref(), Some(&project));
}

#[test]
fn preview_is_bounded_and_restore_uses_provider_specific_direct_arguments() {
    let fixture = tempfile::tempdir().unwrap();
    let claude = fixture.path().join("claude");
    let codex = fixture.path().join("codex");
    let original = fixture.path().join("original");
    let target = fixture.path().join("target");
    fs::create_dir_all(claude.join("projects/demo")).unwrap();
    fs::create_dir_all(codex.join("sessions/2026/09/15")).unwrap();
    fs::create_dir_all(&original).unwrap();
    fs::create_dir_all(&target).unwrap();
    let log = claude.join("projects/demo/session-42.jsonl");
    fs::write(
        &log,
        format!(
            "{{\"type\":\"user\",\"sessionId\":\"session-42\",\"cwd\":{:?},\"message\":{{\"content\":\"first question\"}}}}\n{{\"type\":\"assistant\",\"sessionId\":\"session-42\",\"message\":{{\"content\":[{{\"type\":\"text\",\"text\":\"first answer\"}}]}}}}\n{{\"type\":\"user\",\"sessionId\":\"session-42\",\"message\":{{\"content\":\"second question\"}}}}\n",
            original.to_string_lossy()
        ),
    )
    .unwrap();
    fs::write(
        codex.join("sessions/2026/09/15/codex-9.jsonl"),
        format!(
            "{{\"type\":\"session_meta\",\"payload\":{{\"id\":\"codex-9\",\"cwd\":{:?}}}}}\n{{\"type\":\"response_item\",\"payload\":{{\"role\":\"user\",\"content\":\"resume codex directly\"}}}}\n",
            original.to_string_lossy()
        ),
    )
    .unwrap();
    let discovery = ProviderDiscovery::new(
        DiscoveryRoots {
            claude_config_dir: claude,
            codex_home_dir: codex,
        },
        50,
        90 * 24 * 60 * 60 * 1_000,
    );
    let preview = discovery
        .preview(AgentProvider::Claude, "session-42", 2)
        .unwrap();
    assert_eq!(preview.messages.len(), 2);
    assert_eq!(preview.messages[0].text, "first answer");
    assert_eq!(preview.messages[1].text, "second question");
    assert!(preview.is_truncated);

    let plan = preview.session.restore_plan(RestoreTarget {
        cwd: Some(target.clone()),
        project_root: Some(original.clone()),
        worktree_path: Some(target.clone()),
        name: Some("Restored review".to_owned()),
    });
    assert_eq!(plan.params.kind, SessionKind::Claude);
    assert_eq!(plan.params.args, ["--resume", "session-42"]);
    assert_eq!(plan.params.cwd, Some(target.clone()));
    assert_eq!(plan.params.worktree_path, Some(target));
    assert_eq!(plan.conversation_id, "session-42");

    let codex = discovery
        .preview(AgentProvider::Codex, "codex-9", 5)
        .unwrap()
        .session;
    assert_eq!(
        codex.restore_plan(RestoreTarget::default()).params.args,
        ["resume", "codex-9"]
    );
}

#[test]
fn a_fork_copies_the_conversation_under_a_new_id() {
    let fork = AgentProvider::Claude.fork_plan("parent-1").unwrap();
    let id = fork.conversation_id.clone().unwrap();
    assert_eq!(
        fork.args,
        [
            "--resume",
            "parent-1",
            "--fork-session",
            "--session-id",
            id.as_str()
        ]
    );
    // Claude only accepts a version 4 UUID for --session-id.
    let groups = id.split('-').map(str::len).collect::<Vec<_>>();
    assert_eq!(groups, [8, 4, 4, 4, 12]);
    assert!(
        id.chars()
            .all(|c| c == '-' || c.is_ascii_digit() || ('a'..='f').contains(&c))
    );
    assert_eq!(&id[14..15], "4");
    assert!("89ab".contains(&id[19..20]));
    assert_ne!(
        AgentProvider::Claude
            .fork_plan("parent-1")
            .unwrap()
            .conversation_id,
        Some(id)
    );

    let fork = AgentProvider::Codex.fork_plan("codex-9").unwrap();
    assert_eq!(fork.args, ["fork", "codex-9"]);
    assert_eq!(fork.conversation_id, None);
}

#[test]
fn recent_mutated_paths_only_tracks_claude_file_mutations_newest_first() {
    let fixture = tempfile::tempdir().unwrap();
    let log = fixture.path().join("session.jsonl");
    fs::write(
        &log,
        concat!(
            "{\"type\":\"assistant\",\"message\":{\"content\":[{\"type\":\"tool_use\",\"name\":\"Read\",\"input\":{\"file_path\":\"/repo/read-only.rs\"}},{\"type\":\"tool_use\",\"name\":\"Edit\",\"input\":{\"file_path\":\"/repo/older.rs\"}}]}}\n",
            "{\"type\":\"assistant\",\"message\":{\"content\":[{\"type\":\"tool_use\",\"name\":\"Write\",\"input\":{\"file_path\":\"/repo/newer.rs\"}},{\"type\":\"tool_use\",\"name\":\"Edit\",\"input\":{\"file_path\":\"/repo/older.rs\"}}]}}\n"
        ),
    )
    .unwrap();

    assert_eq!(
        recent_mutated_paths(&log, 20).unwrap(),
        [
            std::path::PathBuf::from("/repo/newer.rs"),
            std::path::PathBuf::from("/repo/older.rs")
        ]
    );
}

#[test]
fn titles_prefer_the_given_name_then_the_provider_title_then_the_command() {
    let fixture = tempfile::tempdir().unwrap();
    let claude = fixture.path().join("claude");
    let codex = fixture.path().join("codex");
    let project = fixture.path().join("project");
    fs::create_dir_all(claude.join("projects/demo")).unwrap();
    fs::create_dir_all(codex.join("sessions/2026/09/29")).unwrap();
    fs::create_dir_all(&project).unwrap();
    let cwd = serde_json::to_string(&project).unwrap();
    let line = |value: serde_json::Value| format!("{value}\n");
    // Claude writes the role before the content, which a sorted json! map would not.
    let prompt = |text: &str| {
        format!(
            "{{\"type\":\"user\",\"sessionId\":\"named\",\"cwd\":{cwd},\"gitBranch\":\"cleanup\",\"message\":{{\"role\":\"user\",\"content\":{}}}}}\n",
            serde_json::to_string(text).unwrap()
        )
    };
    let mut named = prompt("Why do we even have the metrics flag?");
    named += &line(
        serde_json::json!({"type":"assistant","message":{"content":[{"type":"text","text":"It guards the exporter."}]}}),
    );
    named += "{\"type\":\"user\",\"message\":{\"role\":\"user\",\"content\":[{\"tool_use_id\":\"t\",\"type\":\"tool_result\",\"content\":\"ok\"}]}}\n";
    named += &prompt("Write a plan for it");
    named +=
        &line(serde_json::json!({"type":"ai-title","aiTitle":"Metrics flag","sessionId":"named"}));
    named += &line(
        serde_json::json!({"type":"custom-title","customTitle":"Alerting","sessionId":"named"}),
    );
    named += &line(
        serde_json::json!({"type":"last-prompt","lastPrompt":"Write a plan for it","sessionId":"named"}),
    );
    fs::write(claude.join("projects/demo/named.jsonl"), named).unwrap();

    let command = format!(
        "{{\"type\":\"user\",\"sessionId\":\"command\",\"cwd\":{cwd},\"message\":{{\"role\":\"user\",\"content\":\"<command-message>review-pr</command-message>\\n<command-name>/review-pr</command-name>\\n<command-args>103601</command-args>\"}}}}\n"
    );
    fs::write(claude.join("projects/demo/command.jsonl"), command).unwrap();

    fs::write(
        codex.join("sessions/2026/09/29/rollout.jsonl"),
        format!(
            "{{\"type\":\"session_meta\",\"payload\":{{\"id\":\"codex-7\",\"cwd\":{cwd},\"git\":{{\"branch\":\"dashboards\"}}}}}}\n{{\"type\":\"response_item\",\"payload\":{{\"role\":\"user\",\"content\":[{{\"type\":\"input_text\",\"text\":\"Build a Grafana dashboard\"}}]}}}}\n"
        ),
    )
    .unwrap();
    fs::write(
        codex.join("session_index.jsonl"),
        "{\"id\":\"codex-7\",\"thread_name\":\"Old name\"}\n{\"id\":\"codex-7\",\"thread_name\":\"Grafana Log Dashboard\"}\n",
    )
    .unwrap();

    let cache = fixture.path().join("cache.json");
    let discovery = ProviderDiscovery::new(
        DiscoveryRoots {
            claude_config_dir: claude,
            codex_home_dir: codex,
        },
        50,
        90 * 24 * 60 * 60 * 1_000,
    )
    .with_cache(cache.clone());
    let now = std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .unwrap()
        .as_millis() as u64;
    let sessions = discovery.discover(now, &BTreeSet::new()).unwrap();
    let find = |id: &str| {
        sessions
            .iter()
            .find(|session| session.provider_session_id == id)
            .unwrap()
    };

    let named = find("named");
    assert_eq!(named.name, "Alerting");
    assert_eq!(named.ai_title.as_deref(), Some("Metrics flag"));
    assert_eq!(
        named.first_prompt.as_deref(),
        Some("Why do we even have the metrics flag?")
    );
    assert_eq!(named.last_prompt.as_deref(), Some("Write a plan for it"));
    assert_eq!(named.branch.as_deref(), Some("cleanup"));
    assert_eq!(named.prompt_count, 2);

    let command = find("command");
    assert_eq!(command.name, "/review-pr 103601");
    assert_eq!(command.prompt_count, 1);

    let codex = find("codex-7");
    assert_eq!(codex.name, "Grafana Log Dashboard");
    assert_eq!(codex.branch.as_deref(), Some("dashboards"));
    assert_eq!(codex.prompt_count, 1);
    assert!(cache.exists());
}

#[test]
fn prompt_log_reader_follows_appended_codex_prompts() {
    use agtk::providers::PromptLogReader;
    use std::io::Write;

    let fixture = tempfile::tempdir().unwrap();
    let path = fixture.path().join("rollout-a.jsonl");
    let mut log = fs::File::create(&path).unwrap();
    writeln!(
        log,
        r#"{{"timestamp":"2026-09-23T10:03:07.757Z","type":"session_meta","payload":{{"id":"codex-1","cwd":"/work"}}}}"#
    )
    .unwrap();
    // Before the agtk session existed: an earlier life of the conversation.
    writeln!(
        log,
        r#"{{"timestamp":"2026-09-23T10:03:08.000Z","type":"response_item","payload":{{"type":"message","role":"user","content":[{{"type":"input_text","text":"old prompt"}}]}}}}"#
    )
    .unwrap();
    writeln!(
        log,
        r#"{{"timestamp":"2026-09-23T10:04:10.000Z","type":"response_item","payload":{{"type":"message","role":"user","content":[{{"type":"input_text","text":"<environment_context>x</environment_context>"}}]}}}}"#
    )
    .unwrap();
    writeln!(
        log,
        r#"{{"timestamp":"2026-09-23T10:04:10.401Z","type":"response_item","payload":{{"type":"message","role":"user","content":[{{"type":"input_text","text":"  fix the build\nthen test  "}}]}}}}"#
    )
    .unwrap();
    // Older logs repeat the prompt as an event right after the item.
    writeln!(
        log,
        r#"{{"timestamp":"2026-09-23T10:04:10.401Z","type":"event_msg","payload":{{"type":"user_message","message":"fix the build\nthen test"}}}}"#
    )
    .unwrap();
    write!(log, r#"{{"timestamp":"2026-09-23T10:05:00.000Z","type":"response_item","payload":{{"type":"message","role":"user","content":[{{"type":"input_text","text":"partial"#).unwrap();
    log.flush().unwrap();

    let since = 1_790_157_849_000; // 2026-09-23T10:04:09Z
    let mut reader = PromptLogReader::new(path.clone(), since);
    assert_eq!(reader.read_new().unwrap(), ["fix the build\nthen test"]);
    // Nothing new, and the unfinished line is not consumed.
    assert_eq!(reader.read_new().unwrap(), Vec::<String>::new());

    writeln!(log, r#" line"}}]}}}}"#).unwrap();
    writeln!(
        log,
        r#"{{"timestamp":"2026-09-23T10:05:01.000Z","type":"response_item","payload":{{"type":"message","role":"assistant","content":[{{"type":"output_text","text":"done"}}]}}}}"#
    )
    .unwrap();
    log.flush().unwrap();
    assert_eq!(reader.read_new().unwrap(), ["partial line"]);
    assert_eq!(reader.path(), path);
}

#[test]
fn codex_log_is_located_by_conversation_or_by_directory_and_launch_time() {
    let fixture = tempfile::tempdir().unwrap();
    let codex = fixture.path().join("codex");
    let day = codex.join("sessions/2026/09/23");
    fs::create_dir_all(&day).unwrap();
    let rollout = |id: &str, started: &str, cwd: &str| {
        format!(
            "{{\"timestamp\":\"{started}\",\"type\":\"session_meta\",\"payload\":{{\"id\":\"{id}\",\"timestamp\":\"{started}\",\"cwd\":\"{cwd}\"}}}}\n"
        )
    };
    let older = day.join("rollout-2026-09-23T11-00-00-older.jsonl");
    let mine = day.join("rollout-2026-09-23T12-03-07-mine.jsonl");
    let later = day.join("rollout-2026-09-23T12-10-00-later.jsonl");
    let elsewhere = day.join("rollout-2026-09-23T12-03-08-elsewhere.jsonl");
    fs::write(
        &older,
        rollout("older", "2026-09-23T09:00:00.000Z", "/work"),
    )
    .unwrap();
    fs::write(&mine, rollout("mine", "2026-09-23T10:03:07.757Z", "/work")).unwrap();
    fs::write(
        &later,
        rollout("later", "2026-09-23T10:10:00.000Z", "/work"),
    )
    .unwrap();
    fs::write(
        &elsewhere,
        rollout("elsewhere", "2026-09-23T10:03:08.000Z", "/other"),
    )
    .unwrap();
    let discovery = ProviderDiscovery::new(
        DiscoveryRoots {
            claude_config_dir: fixture.path().join("claude"),
            codex_home_dir: codex,
        },
        100,
        u64::MAX,
    );
    let launched_at = 1_790_157_787_000; // 2026-09-23T10:03:07Z
    let none = BTreeSet::new();

    assert_eq!(
        discovery
            .locate_codex_log(None, std::path::Path::new("/work"), launched_at, &none)
            .unwrap(),
        Some(("mine".to_owned(), mine.clone()))
    );
    let claimed = BTreeSet::from(["mine".to_owned()]);
    assert_eq!(
        discovery
            .locate_codex_log(None, std::path::Path::new("/work"), launched_at, &claimed)
            .unwrap(),
        Some(("later".to_owned(), later))
    );
    assert_eq!(
        discovery
            .locate_codex_log(
                Some("older"),
                std::path::Path::new("/work"),
                launched_at,
                &none
            )
            .unwrap(),
        Some(("older".to_owned(), older))
    );
    assert_eq!(
        discovery
            .locate_codex_log(None, std::path::Path::new("/nowhere"), launched_at, &none)
            .unwrap(),
        None
    );
}
