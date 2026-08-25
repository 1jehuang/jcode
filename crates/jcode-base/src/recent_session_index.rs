//! Durable metadata index for fast recent-session lists.
//!
//! Transcript snapshots can be hundreds of megabytes and a long-lived install
//! can contain 100k+ files. This SQLite index is updated beside normal session
//! persistence and can be queried across daemon, CLI, and API bridge processes.

use std::time::Duration;

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
    pub updated_at_ms: i64,
    pub last_active_at_ms: Option<i64>,
}

impl RecentSessionMetadata {
    pub fn display_title(&self) -> Option<&str> {
        self.custom_title
            .as_deref()
            .and_then(non_empty)
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
             last_active_at_ms INTEGER
         );
         CREATE INDEX IF NOT EXISTS recent_sessions_activity
         ON recent_sessions(COALESCE(last_active_at_ms, updated_at_ms) DESC);",
    )?;
    // Additive migration for databases created before saved-session ordering
    // became part of the shared session-list contract.
    let _ = connection.execute(
        "ALTER TABLE recent_sessions ADD COLUMN saved INTEGER NOT NULL DEFAULT 0",
        [],
    );
    Ok(connection)
}

pub fn recent(limit: usize) -> Result<Vec<RecentSessionMetadata>> {
    let connection = open()?;
    let mut statement = connection.prepare(
        "SELECT session_id, working_dir, generated_title, custom_title,
                todo_title, saved, updated_at_ms, last_active_at_ms
         FROM recent_sessions
         ORDER BY COALESCE(last_active_at_ms, updated_at_ms) DESC
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
            })
        })?
        .collect::<rusqlite::Result<Vec<_>>>()?;
    Ok(entries)
}

/// Update the index after a successful session persistence operation.
pub fn upsert_session(session: &Session) -> Result<()> {
    upsert(&RecentSessionMetadata {
        session_id: session.id.clone(),
        working_dir: session.working_dir.clone(),
        generated_title: session.title.clone(),
        custom_title: session.custom_title.clone(),
        todo_title: crate::todo::load_session_title(&session.id),
        saved: session.saved,
        updated_at_ms: session.updated_at.timestamp_millis(),
        last_active_at_ms: session.last_active_at.map(|time| time.timestamp_millis()),
    })
}

pub fn upsert(entry: &RecentSessionMetadata) -> Result<()> {
    open()?.execute(
        "INSERT INTO recent_sessions (
             session_id, working_dir, generated_title, custom_title, todo_title,
             saved, updated_at_ms, last_active_at_ms
         ) VALUES (?1, ?2, ?3, ?4, ?5, ?6, ?7, ?8)
         ON CONFLICT(session_id) DO UPDATE SET
             working_dir = excluded.working_dir,
             generated_title = excluded.generated_title,
             custom_title = excluded.custom_title,
             todo_title = excluded.todo_title,
             saved = excluded.saved,
             updated_at_ms = excluded.updated_at_ms,
             last_active_at_ms = excluded.last_active_at_ms",
        params![
            entry.session_id,
            entry.working_dir,
            entry.generated_title,
            entry.custom_title,
            entry.todo_title,
            entry.saved,
            entry.updated_at_ms,
            entry.last_active_at_ms,
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
            updated_at_ms: 1,
            last_active_at_ms: None,
        };
        assert_eq!(entry.display_title(), Some("Todo goal"));
        entry.custom_title = Some("Renamed".into());
        assert_eq!(entry.display_title(), Some("Renamed"));
    }
}
