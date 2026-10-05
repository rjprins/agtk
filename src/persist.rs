use std::fs;
use std::os::unix::fs::PermissionsExt;
use std::path::{Path, PathBuf};
use std::sync::{Arc, Mutex};
use std::time::{Duration, SystemTime, UNIX_EPOCH};

use rusqlite::{Connection, OptionalExtension, params};
use serde::de::DeserializeOwned;
use serde::{Deserialize, Serialize};
use serde_json::Value;

use crate::instance::ensure_private_dir;
use crate::session::{SessionKind, SessionState};

pub type PersistResult<T> = Result<T, Box<dyn std::error::Error + Send + Sync>>;

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct SessionRecord {
    pub id: String,
    pub name: String,
    pub kind: SessionKind,
    pub program: PathBuf,
    pub args: Vec<String>,
    pub cwd: Option<PathBuf>,
    pub project_root: Option<PathBuf>,
    pub worktree_path: Option<PathBuf>,
    pub conversation_id: Option<String>,
    pub socket_path: PathBuf,
    pub created_at: u64,
    pub state: SessionState,
    /// Zero for records saved before this field existed.
    #[serde(default)]
    pub state_changed_at: u64,
    pub position: i64,
    /// True once the user named the session, as opposed to a generated name.
    #[serde(default)]
    pub renamed: bool,
}

impl SessionRecord {
    pub fn discovered(id: &str, socket_path: PathBuf) -> Self {
        Self {
            id: id.into(),
            name: id.into(),
            kind: SessionKind::Shell,
            program: PathBuf::new(),
            args: Vec::new(),
            cwd: None,
            project_root: None,
            worktree_path: None,
            conversation_id: None,
            socket_path,
            created_at: now_millis(),
            state: SessionState::Running,
            state_changed_at: 0,
            position: 0,
            renamed: false,
        }
    }
}

#[derive(Clone)]
pub struct Store(Arc<Mutex<Connection>>);

impl Store {
    // Connection is Send but not Sync. Serialize access here, execute calls on the I/O worker.
    // https://docs.rs/rusqlite/0.40.2/rusqlite/struct.Connection.html
    pub fn open(path: &Path) -> PersistResult<Self> {
        if let Some(parent) = path.parent() {
            ensure_private_dir(parent)?;
        }
        let mut conn = Connection::open(path)?;
        fs::set_permissions(path, fs::Permissions::from_mode(0o600))?;
        conn.busy_timeout(Duration::from_secs(2))?;
        let version: i64 = conn.pragma_query_value(None, "user_version", |row| row.get(0))?;
        if version > 1 {
            return Err(format!("database schema {version} is newer than this application").into());
        }
        if version == 0 {
            let tx = conn.transaction()?;
            tx.execute_batch(
                "CREATE TABLE sessions (id TEXT PRIMARY KEY, position INTEGER NOT NULL, data TEXT NOT NULL);
                 CREATE TABLE preferences (key TEXT PRIMARY KEY, data TEXT NOT NULL);
                 PRAGMA user_version = 1;"
            )?;
            tx.commit()?;
        }
        Ok(Self(Arc::new(Mutex::new(conn))))
    }

    pub fn sessions(&self) -> PersistResult<Vec<SessionRecord>> {
        let conn = self.0.lock().map_err(|_| "database lock poisoned")?;
        let mut query = conn.prepare("SELECT data FROM sessions ORDER BY position, id")?;
        let rows = query.query_map([], |row| row.get::<_, String>(0))?;
        let mut sessions = Vec::new();
        for row in rows {
            // A record another agtk build wrote differently skips only itself.
            match serde_json::from_str(&row?) {
                Ok(session) => sessions.push(session),
                Err(error) => eprintln!("agtk: skipping a saved session: {error}"),
            }
        }
        Ok(sessions)
    }

    pub fn save_session(&self, session: &SessionRecord) -> PersistResult<()> {
        let data = serde_json::to_string(session)?;
        self.0
            .lock()
            .map_err(|_| "database lock poisoned")?
            .execute(
                "INSERT INTO sessions(id, position, data) VALUES (?1, ?2, ?3)
             ON CONFLICT(id) DO UPDATE SET position=excluded.position, data=excluded.data",
                params![session.id, session.position, data],
            )?;
        Ok(())
    }

    pub fn remove_session(&self, id: &str) -> PersistResult<()> {
        self.0
            .lock()
            .map_err(|_| "database lock poisoned")?
            .execute("DELETE FROM sessions WHERE id=?1", [id])?;
        Ok(())
    }

    pub fn preference(&self, key: &str) -> PersistResult<Option<Value>> {
        let data: Option<String> = self
            .0
            .lock()
            .map_err(|_| "database lock poisoned")?
            .query_row("SELECT data FROM preferences WHERE key=?1", [key], |row| {
                row.get(0)
            })
            .optional()?;
        data.map(|data| serde_json::from_str(&data).map_err(Into::into))
            .transpose()
    }

    /// A saved preference, or its default when it is missing or no longer
    /// parses, so one bad value cannot stop the workspace from loading.
    pub fn preference_or_default<T: DeserializeOwned + Default>(
        &self,
        key: &str,
    ) -> PersistResult<T> {
        let Some(value) = self.preference(key)? else {
            return Ok(T::default());
        };
        Ok(serde_json::from_value(value).unwrap_or_else(|error| {
            eprintln!("agtk: ignoring saved preference {key}: {error}");
            T::default()
        }))
    }

    pub fn set_preference(&self, key: &str, value: &Value) -> PersistResult<()> {
        self.0
            .lock()
            .map_err(|_| "database lock poisoned")?
            .execute(
                "INSERT INTO preferences(key, data) VALUES (?1, ?2)
             ON CONFLICT(key) DO UPDATE SET data=excluded.data",
                params![key, serde_json::to_string(value)?],
            )?;
        Ok(())
    }
}

pub fn now_millis() -> u64 {
    SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .unwrap_or_default()
        .as_millis() as u64
}
