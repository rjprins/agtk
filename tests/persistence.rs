use agmux_native::control::{SessionKind, SessionState};
use agmux_native::persist::{SessionRecord, Store};

#[test]
fn session_identity_associations_order_and_preferences_survive_reopen() {
    let dir = tempfile::tempdir().unwrap();
    let path = dir.path().join("state/agmux.db");
    let mut record = SessionRecord::discovered("session-a", dir.path().join("a.sock"));
    record.name = "Review native UI".into();
    record.kind = SessionKind::Codex;
    record.cwd = Some("/tmp/project".into());
    record.project_root = record.cwd.clone();
    record.worktree_path = Some("/tmp/project-review".into());
    record.args = vec!["resume".into(), "conversation-id".into()];
    record.state = SessionState::Ready;
    record.position = 3;
    let store = Store::open(&path).unwrap();
    store.save_session(&record).unwrap();
    store
        .set_preference("selectedSessionId", &serde_json::json!("session-a"))
        .unwrap();
    drop(store);
    let reopened = Store::open(&path).unwrap();
    assert_eq!(reopened.sessions().unwrap(), vec![record]);
    assert_eq!(
        reopened.preference("selectedSessionId").unwrap(),
        Some(serde_json::json!("session-a"))
    );
    reopened.remove_session("session-a").unwrap();
    assert!(reopened.sessions().unwrap().is_empty());
}

#[test]
fn refuses_a_database_from_a_newer_application_without_modifying_it() {
    let dir = tempfile::tempdir().unwrap();
    let path = dir.path().join("agmux.db");
    let conn = rusqlite::Connection::open(&path).unwrap();
    conn.pragma_update(None, "user_version", 999).unwrap();
    drop(conn);
    assert!(Store::open(&path).is_err());
    let conn = rusqlite::Connection::open(path).unwrap();
    assert_eq!(
        conn.pragma_query_value(None, "user_version", |r| r.get::<_, i64>(0))
            .unwrap(),
        999
    );
}
