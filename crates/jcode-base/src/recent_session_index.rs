//! Durable metadata index for fast recent-session lists.
//!
//! Transcript snapshots can be hundreds of megabytes and a long-lived install
//! can contain 100k+ files. This SQLite index is updated beside normal session
//! persistence and can be queried across daemon, CLI, and API bridge processes.

use std::collections::HashSet;
use std::time::{Duration, UNIX_EPOCH};

use anyhow::Result;
use rusqlite::{Connection, OptionalExtension, params};

use crate::session::Session;

#[derive(Clone, Debug, PartialEq, Eq)]
pub struct RecentSessionMetadata {
    pub session_id: String,
    pub working_dir: Option<String>,
    pub generated_title: Option<String>,
    pub custom_title: Option<String>,
    pub todo_title: Option<String>,
    pub saved: bool,
    pub save_label: Option<String>,
    pub updated_at_ms: i64,
    pub last_active_at_ms: Option<i64>,
    /// None for index rows written before persisted status was indexed.
    pub status: Option<String>,
    /// None for index rows written before debug sessions were indexed.
    pub is_debug: Option<bool>,
}

impl RecentSessionMetadata {
    pub fn display_title(&self) -> Option<&str> {
        self.custom_title
            .as_deref()
            .and_then(non_empty)
            .or_else(|| {
                // Bookmarks labelled before labels doubled as titles.
                self.saved
                    .then(|| self.save_label.as_deref().and_then(non_empty))
                    .flatten()
            })
            .or_else(|| self.todo_title.as_deref().and_then(non_empty))
            .or_else(|| self.generated_title.as_deref().and_then(non_empty))
    }
}

fn non_empty(value: &str) -> Option<&str> {
    let value = value.trim();
    (!value.is_empty()).then_some(value)
}

fn open() -> Result<Connection> {
    let path = crate::storage::jcode_dir()?.join("session-metadata-v1.sqlite3");
    let connection = Connection::open(path)?;
    connection.busy_timeout(Duration::from_secs(2))?;
    connection.execute_batch(
        "PRAGMA journal_mode=WAL;
         PRAGMA synchronous=NORMAL;
         CREATE TABLE IF NOT EXISTS recent_sessions (
             session_id TEXT PRIMARY KEY NOT NULL,
             working_dir TEXT,
             generated_title TEXT,
             custom_title TEXT,
             todo_title TEXT,
             updated_at_ms INTEGER NOT NULL,
             last_active_at_ms INTEGER,
             saved INTEGER NOT NULL DEFAULT 0,
             save_label TEXT,
             status TEXT,
             is_debug INTEGER
         );
         CREATE INDEX IF NOT EXISTS recent_sessions_activity
         ON recent_sessions(COALESCE(last_active_at_ms, updated_at_ms) DESC);
         CREATE INDEX IF NOT EXISTS recent_sessions_updated
         ON recent_sessions(updated_at_ms DESC);",
    )?;
    // Additive migration for databases created before saved-session ordering
    // became part of the shared session-list contract.
    let _ = connection.execute(
        "ALTER TABLE recent_sessions ADD COLUMN saved INTEGER NOT NULL DEFAULT 0",
        [],
    );
    let _ = connection.execute("ALTER TABLE recent_sessions ADD COLUMN save_label TEXT", []);
    let _ = connection.execute("ALTER TABLE recent_sessions ADD COLUMN status TEXT", []);
    let _ = connection.execute(
        "ALTER TABLE recent_sessions ADD COLUMN is_debug INTEGER",
        [],
    );
    Ok(connection)
}

pub fn recent(limit: usize) -> Result<Vec<RecentSessionMetadata>> {
    let connection = open()?;
    let mut statement = connection.prepare(
        "SELECT session_id, working_dir, generated_title, custom_title,
                todo_title, saved, updated_at_ms, last_active_at_ms, save_label,
                status, is_debug
         FROM recent_sessions
         ORDER BY updated_at_ms DESC, session_id DESC
         LIMIT ?1",
    )?;
    let entries = statement
        .query_map([i64::try_from(limit).unwrap_or(i64::MAX)], |row| {
            Ok(RecentSessionMetadata {
                session_id: row.get(0)?,
                working_dir: row.get(1)?,
                generated_title: row.get(2)?,
                custom_title: row.get(3)?,
                todo_title: row.get(4)?,
                saved: row.get(5)?,
                updated_at_ms: row.get(6)?,
                last_active_at_ms: row.get(7)?,
                save_label: row.get(8)?,
                status: row.get(9)?,
                is_debug: row.get(10)?,
            })
        })?
        .collect::<rusqlite::Result<Vec<_>>>()?;
    Ok(entries)
}

/// Return recent native jcode sessions, hydrating rows created before newer
/// index columns existed and bootstrapping the index on older installations.
pub fn recent_persisted(limit: usize) -> Result<Vec<RecentSessionMetadata>> {
    let limit = limit.min(500);
    if limit == 0 {
        return Ok(Vec::new());
    }

    let fetch_limit = limit.saturating_mul(4).min(500);
    bootstrap_from_snapshots(fetch_limit)?;
    let entries = recent(500)?;

    let mut hydrated = Vec::with_capacity(entries.len());
    for mut entry in entries {
        if entry.session_id.starts_with("imported_")
            || !crate::session::session_exists(&entry.session_id)
        {
            continue;
        }
        if entry.status.is_none() || entry.is_debug.is_none() {
            let Ok(snapshot) = metadata_from_snapshot(&entry.session_id) else {
                // Unknown debug state is not safe to expose as a normal session.
                continue;
            };
            entry = snapshot;
            let _ = upsert(&entry);
        }
        if entry.is_debug != Some(false) {
            continue;
        }
        hydrated.push(entry);
    }

    hydrated.sort_by(|a, b| {
        b.updated_at_ms
            .cmp(&a.updated_at_ms)
            .then_with(|| b.session_id.cmp(&a.session_id))
    });
    hydrated.truncate(limit);
    Ok(hydrated)
}

fn bootstrap_from_snapshots(limit: usize) -> Result<()> {
    let sessions_dir = crate::storage::jcode_dir()?.join("sessions");
    let connection = open()?;
    let mut statement = connection.prepare("SELECT session_id FROM recent_sessions")?;
    let existing = statement
        .query_map([], |row| row.get::<_, String>(0))?
        .collect::<rusqlite::Result<HashSet<_>>>()?;
    drop(statement);
    drop(connection);
    let Ok(directory) = std::fs::read_dir(sessions_dir) else {
        return Ok(());
    };
    let mut candidates = directory
        .flatten()
        .filter_map(|entry| {
            let path = entry.path();
            if !entry.file_type().ok()?.is_file() || path.extension()?.to_str()? != "json" {
                return None;
            }
            let session_id = path.file_stem()?.to_str()?.to_string();
            if session_id.starts_with("imported_")
                || existing.contains(session_id.as_str())
                || !valid_session_id(&session_id)
            {
                return None;
            }
            let modified = path
                .metadata()
                .and_then(|metadata| metadata.modified())
                .unwrap_or(UNIX_EPOCH);
            Some((modified, session_id))
        })
        .collect::<Vec<_>>();
    candidates.sort_unstable_by_key(|(modified, _)| std::cmp::Reverse(*modified));

    let mut indexed = 0usize;
    for (_, session_id) in candidates {
        if let Ok(metadata) = metadata_from_snapshot(&session_id) {
            let _ = upsert(&metadata);
            if metadata.is_debug == Some(false) {
                indexed += 1;
                if indexed >= limit {
                    break;
                }
            }
        }
    }
    Ok(())
}

fn metadata_from_snapshot(session_id: &str) -> Result<RecentSessionMetadata> {
    let session = Session::load_startup_stub(session_id)?;
    Ok(metadata_from_session(&session))
}

fn valid_session_id(session_id: &str) -> bool {
    !session_id.is_empty()
        && session_id.len() <= 128
        && session_id
            .chars()
            .all(|character| character.is_ascii_alphanumeric() || matches!(character, '_' | '-'))
}

fn metadata_from_session(session: &Session) -> RecentSessionMetadata {
    RecentSessionMetadata {
        session_id: session.id.clone(),
        working_dir: session.working_dir.clone(),
        generated_title: session.title.clone(),
        custom_title: session.custom_title.clone(),
        todo_title: crate::todo::load_session_title(&session.id),
        saved: session.saved,
        save_label: session.save_label.clone(),
        updated_at_ms: session.updated_at.timestamp_millis(),
        last_active_at_ms: session.last_active_at.map(|time| time.timestamp_millis()),
        status: Some(session.status.display().to_string()),
        is_debug: Some(session.is_debug),
    }
}

/// Update the index after a successful session persistence operation.
pub fn upsert_session(session: &Session) -> Result<()> {
    upsert(&metadata_from_session(session))
}

pub fn upsert(entry: &RecentSessionMetadata) -> Result<()> {
    open()?.execute(
        "INSERT INTO recent_sessions (
             session_id, working_dir, generated_title, custom_title, todo_title,
             saved, updated_at_ms, last_active_at_ms, save_label, status, is_debug
         ) VALUES (?1, ?2, ?3, ?4, ?5, ?6, ?7, ?8, ?9, ?10, ?11)
         ON CONFLICT(session_id) DO UPDATE SET
             working_dir = excluded.working_dir,
             generated_title = excluded.generated_title,
             custom_title = excluded.custom_title,
             todo_title = excluded.todo_title,
             saved = excluded.saved,
             updated_at_ms = excluded.updated_at_ms,
             last_active_at_ms = excluded.last_active_at_ms,
             save_label = excluded.save_label,
             status = excluded.status,
             is_debug = excluded.is_debug",
        params![
            entry.session_id,
            entry.working_dir,
            entry.generated_title,
            entry.custom_title,
            entry.todo_title,
            entry.saved,
            entry.updated_at_ms,
            entry.last_active_at_ms,
            entry.save_label,
            entry.status,
            entry.is_debug,
        ],
    )?;
    Ok(())
}

/// Refresh only the derived title after the todo or plan file changes.
pub fn refresh_todo_title(session_id: &str) -> Result<()> {
    let connection = open()?;
    let exists = connection
        .query_row(
            "SELECT 1 FROM recent_sessions WHERE session_id = ?1",
            [session_id],
            |_| Ok(()),
        )
        .optional()?
        .is_some();
    if exists {
        connection.execute(
            "UPDATE recent_sessions SET todo_title = ?2 WHERE session_id = ?1",
            params![session_id, crate::todo::load_session_title(session_id)],
        )?;
    }
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn display_title_uses_custom_then_todo_then_generated() {
        let mut entry = RecentSessionMetadata {
            session_id: "session_test".into(),
            working_dir: None,
            generated_title: Some("Generated".into()),
            custom_title: None,
            todo_title: Some("Todo goal".into()),
            saved: false,
            save_label: None,
            updated_at_ms: 1,
            last_active_at_ms: None,
            status: None,
            is_debug: None,
        };
        assert_eq!(entry.display_title(), Some("Todo goal"));
        entry.custom_title = Some("Renamed".into());
        assert_eq!(entry.display_title(), Some("Renamed"));
    }

    #[test]
    fn display_title_prefers_save_label_over_derived_titles() {
        let mut entry = RecentSessionMetadata {
            session_id: "session_test".into(),
            working_dir: None,
            generated_title: Some("Generated".into()),
            custom_title: None,
            todo_title: Some("Todo goal".into()),
            saved: true,
            save_label: Some("yc mcp".into()),
            updated_at_ms: 1,
            last_active_at_ms: None,
            status: None,
            is_debug: None,
        };
        assert_eq!(entry.display_title(), Some("yc mcp"));
        entry.custom_title = Some("Renamed".into());
        assert_eq!(entry.display_title(), Some("Renamed"));
        entry.custom_title = None;
        entry.saved = false;
        assert_eq!(entry.display_title(), Some("Todo goal"));
    }

    #[test]
    fn recent_persisted_bootstraps_orders_and_filters_sessions() {
        let _env_lock = crate::storage::lock_test_env();
        let previous_home = std::env::var_os("JCODE_HOME");
        let home = tempfile::tempdir().expect("create temporary jcode home");
        crate::env::set_var("JCODE_HOME", home.path());

        let mut old = Session::create_with_id(
            "session_old_1".to_string(),
            None,
            Some("Old session".to_string()),
        );
        old.working_dir = Some("/srv/old".to_string());
        old.status = crate::session::SessionStatus::Closed;
        old.save_prepared().expect("persist old session");
        std::thread::sleep(Duration::from_millis(2));

        let mut new = Session::create_with_id(
            "session_new_2".to_string(),
            None,
            Some("New session".to_string()),
        );
        new.working_dir = Some("/srv/new".to_string());
        new.status = crate::session::SessionStatus::Crashed { message: None };
        new.save_prepared().expect("persist new session");
        std::thread::sleep(Duration::from_millis(2));

        let mut debug = Session::create_with_id(
            "session_debug_3".to_string(),
            None,
            Some("Debug session".to_string()),
        );
        debug.is_debug = true;
        debug.save_prepared().expect("persist debug session");

        // Simulate an installation created before the metadata index existed.
        for suffix in ["", "-wal", "-shm"] {
            let _ = std::fs::remove_file(
                home.path()
                    .join(format!("session-metadata-v1.sqlite3{suffix}")),
            );
        }

        let sessions = recent_persisted(2).expect("list recent sessions");
        assert_eq!(sessions.len(), 2);
        assert_eq!(sessions[0].session_id, "session_new_2");
        assert_eq!(sessions[0].display_title(), Some("New session"));
        assert_eq!(sessions[0].working_dir.as_deref(), Some("/srv/new"));
        assert_eq!(sessions[0].status.as_deref(), Some("crashed"));
        assert_eq!(sessions[1].session_id, "session_old_1");
        assert_eq!(sessions[1].status.as_deref(), Some("closed"));

        match previous_home {
            Some(value) => crate::env::set_var("JCODE_HOME", value),
            None => crate::env::remove_var("JCODE_HOME"),
        }
    }

    #[test]
    fn recent_persisted_recovers_missing_rows_past_invalid_snapshots() {
        let _env_lock = crate::storage::lock_test_env();
        let previous_home = std::env::var_os("JCODE_HOME");
        let home = tempfile::tempdir().expect("create temporary jcode home");
        crate::env::set_var("JCODE_HOME", home.path());

        for index in 0..4 {
            let mut session = Session::create_with_id(
                format!("session_indexed_{index}"),
                None,
                Some(format!("Indexed {index}")),
            );
            session.save_prepared().expect("persist indexed session");
        }

        std::thread::sleep(Duration::from_millis(2));
        let mut missing = Session::create_with_id(
            "session_missing_newest".to_string(),
            None,
            Some("Recovered session".to_string()),
        );
        missing.save_prepared().expect("persist missing session");
        open()
            .expect("open metadata index")
            .execute(
                "DELETE FROM recent_sessions WHERE session_id = ?1",
                [missing.id.as_str()],
            )
            .expect("simulate failed index write");

        std::thread::sleep(Duration::from_millis(2));
        let invalid_path =
            crate::session::session_path("session_invalid_newer").expect("invalid snapshot path");
        std::fs::write(invalid_path, b"{invalid").expect("write invalid snapshot");

        assert_eq!(recent(4).expect("read full index").len(), 4);
        let sessions = recent_persisted(1).expect("recover missing session");
        assert_eq!(sessions.len(), 1);
        assert_eq!(sessions[0].session_id, "session_missing_newest");

        match previous_home {
            Some(value) => crate::env::set_var("JCODE_HOME", value),
            None => crate::env::remove_var("JCODE_HOME"),
        }
    }

    #[test]
    fn recent_orders_by_update_time_not_activity_time() {
        let _env_lock = crate::storage::lock_test_env();
        let previous_home = std::env::var_os("JCODE_HOME");
        let home = tempfile::tempdir().expect("create temporary jcode home");
        crate::env::set_var("JCODE_HOME", home.path());

        let now = chrono::Utc::now();
        let mut updated = Session::create_with_id("session_updated".to_string(), None, None);
        updated.updated_at = now;
        updated.last_active_at = Some(now - chrono::Duration::days(2));
        upsert_session(&updated).expect("index updated session");

        let mut active = Session::create_with_id("session_active".to_string(), None, None);
        active.updated_at = now - chrono::Duration::days(1);
        active.last_active_at = Some(now + chrono::Duration::days(1));
        upsert_session(&active).expect("index active session");

        let sessions = recent(1).expect("list recent sessions");
        assert_eq!(sessions[0].session_id, "session_updated");

        match previous_home {
            Some(value) => crate::env::set_var("JCODE_HOME", value),
            None => crate::env::remove_var("JCODE_HOME"),
        }
    }

    #[test]
    fn snapshot_metadata_uses_top_level_fields_and_unknown_debug_is_hidden() {
        let _env_lock = crate::storage::lock_test_env();
        let previous_home = std::env::var_os("JCODE_HOME");
        let home = tempfile::tempdir().expect("create temporary jcode home");
        crate::env::set_var("JCODE_HOME", home.path());

        let mut session = Session::create_with_id(
            "session_top_level".to_string(),
            None,
            Some("Top level".to_string()),
        );
        session.working_dir = Some("/top-level".to_string());
        let mut snapshot = serde_json::to_value(&session).expect("serialize session");
        snapshot["messages"] = serde_json::json!([{
            "working_dir": "/nested",
            "is_debug": true,
            "status": "crashed"
        }]);
        std::fs::create_dir_all(home.path().join("sessions")).expect("create session directory");
        std::fs::write(
            crate::session::session_path(&session.id).expect("session path"),
            serde_json::to_vec(&snapshot).expect("encode snapshot"),
        )
        .expect("write snapshot");

        let metadata = metadata_from_snapshot(&session.id).expect("read top-level metadata");
        assert_eq!(metadata.working_dir.as_deref(), Some("/top-level"));
        assert_eq!(metadata.is_debug, Some(false));
        assert_eq!(metadata.status.as_deref(), Some("active"));

        let unknown_id = "session_unknown_debug";
        std::fs::write(
            crate::session::session_path(unknown_id).expect("unknown snapshot path"),
            b"{invalid",
        )
        .expect("write invalid unknown snapshot");
        upsert(&RecentSessionMetadata {
            session_id: unknown_id.to_string(),
            working_dir: None,
            generated_title: None,
            custom_title: None,
            todo_title: None,
            saved: false,
            save_label: None,
            updated_at_ms: chrono::Utc::now().timestamp_millis(),
            last_active_at_ms: None,
            status: None,
            is_debug: None,
        })
        .expect("index legacy row");
        assert!(
            recent_persisted(10)
                .expect("list recent sessions")
                .iter()
                .all(|entry| entry.session_id != unknown_id)
        );

        match previous_home {
            Some(value) => crate::env::set_var("JCODE_HOME", value),
            None => crate::env::remove_var("JCODE_HOME"),
        }
    }
}
