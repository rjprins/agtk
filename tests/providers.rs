use std::collections::BTreeSet;
use std::fs;

use agmux_native::control::SessionKind;
use agmux_native::providers::{
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
