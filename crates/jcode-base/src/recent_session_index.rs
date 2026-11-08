//! Durable metadata index for fast recent-session lists.
//!
//! Transcript snapshots can be hundreds of megabytes and a long-lived install
//! can contain 100k+ files. This SQLite index is updated beside normal session
//! persistence and can be queried across daemon, CLI, and API bridge processes.

use std::time::Duration;

use anyhow::Result;
use rusqlite::{Connection, OptionalExtension, params};

use crate::session::Session;
use crate::session::resume_cache::{Entry as ResumeEntry, SessionFingerprint};

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

/// One schema-initialized connection per (process, database path), shared by
/// every caller. Opening a fresh connection per upsert re-ran the schema DDL
/// and migrations (write transactions) on every session save, so a reload
/// storm's ~40 concurrent saves serialized on SQLite's write lock and each
/// waited out the 2s busy timeout ("database is locked"). Sharing the
/// connection serializes writers in-process instead, where each write is a
/// sub-millisecond upsert.
struct SharedConnection {
    path: std::path::PathBuf,
    connection: Connection,
}

static SHARED: std::sync::Mutex<Option<SharedConnection>> = std::sync::Mutex::new(None);

fn with_connection<R>(f: impl FnOnce(&Connection) -> Result<R>) -> Result<R> {
    let path = crate::storage::jcode_dir()?.join("session-metadata-v1.sqlite3");
    let mut guard = SHARED
        .lock()
        .unwrap_or_else(|poisoned| poisoned.into_inner());
    // JCODE_HOME can change (tests); reopen when the path moves.
    match guard.as_ref() {
        Some(shared) if shared.path == path => f(&shared.connection),
        _ => {
            let shared = guard.insert(SharedConnection {
                connection: open_at(&path)?,
                path,
            });
            f(&shared.connection)
        }
    }
}

fn open_at(path: &std::path::Path) -> Result<Connection> {
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
    // Serialize migration checks with other processes so a duplicate column is
    // never mistaken for a real migration failure (or vice versa).
    let transaction = rusqlite::Transaction::new_unchecked(
        &connection,
        rusqlite::TransactionBehavior::Immediate,
    )?;
    let columns = {
        let mut statement = transaction.prepare("PRAGMA table_info(recent_sessions)")?;
        statement
            .query_map([], |row| row.get::<_, String>(1))?
            .collect::<rusqlite::Result<std::collections::HashSet<_>>>()?
    };
    for (column, declaration) in [
        ("saved", "INTEGER NOT NULL DEFAULT 0"),
        ("save_label", "TEXT"),
        ("resume_short_name", "TEXT"),
        ("resume_fingerprint", "TEXT"),
        ("resume_updated_at", "TEXT"),
    ] {
        if !columns.contains(column) {
            transaction.execute(
                &format!("ALTER TABLE recent_sessions ADD COLUMN {column} {declaration}"),
                [],
            )?;
        }
    }
    transaction.commit()?;
    Ok(connection)
}

pub fn recent(limit: usize) -> Result<Vec<RecentSessionMetadata>> {
    with_connection(|connection| recent_with(connection, limit))
}

fn recent_with(connection: &Connection, limit: usize) -> Result<Vec<RecentSessionMetadata>> {
    let mut statement = connection.prepare(
        "SELECT session_id, working_dir, generated_title, custom_title,
                todo_title, saved, updated_at_ms, last_active_at_ms, save_label
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
                save_label: row.get(8)?,
            })
        })?
        .collect::<rusqlite::Result<Vec<_>>>()?;
    Ok(entries)
}

/// Update the index after a successful session persistence operation.
pub fn upsert_session(session: &Session) -> Result<()> {
    let fingerprint = SessionFingerprint::read(&crate::session::session_path(&session.id)?);
    index_session(session, fingerprint.as_ref(), false)
}

// A backfill must not overwrite a newer concurrent save. Its supplied
// fingerprint is the one captured around the authoritative metadata read.
pub(crate) fn backfill_resume_session(
    session: &Session,
    fingerprint: &SessionFingerprint,
) -> Result<()> {
    index_session(session, Some(fingerprint), true)
}

fn index_session(
    session: &Session,
    fingerprint: Option<&SessionFingerprint>,
    backfill: bool,
) -> Result<()> {
    let entry = RecentSessionMetadata {
        session_id: session.id.clone(),
        working_dir: session.working_dir.clone(),
        generated_title: session.title.clone(),
        custom_title: session.custom_title.clone(),
        todo_title: crate::todo::load_session_title(&session.id),
        saved: session.saved,
        save_label: session.save_label.clone(),
        updated_at_ms: session.updated_at.timestamp_millis(),
        last_active_at_ms: session.last_active_at.map(|time| time.timestamp_millis()),
    };
    let encoded_fingerprint = fingerprint.map(serde_json::to_string).transpose()?;
    with_connection(|connection| {
        // Acquire the SQLite write lock before validating the read. A newer
        // save either already changed the files (we skip it) or publishes its
        // own index row after this short transaction commits.
        let transaction = backfill
            .then(|| {
                rusqlite::Transaction::new_unchecked(
                    connection,
                    rusqlite::TransactionBehavior::Immediate,
                )
            })
            .transpose()?;
        if backfill
            && SessionFingerprint::read(&crate::session::session_path(&session.id)?).as_ref()
                != fingerprint
        {
            return Ok(());
        }
        upsert_with_resume(
            connection,
            &entry,
            session.short_name.as_deref(),
            encoded_fingerprint.as_deref(),
            Some(&session.updated_at.to_rfc3339()),
        )?;
        if let Some(transaction) = transaction {
            transaction.commit()?;
        }
        Ok(())
    })
}

pub fn upsert(entry: &RecentSessionMetadata) -> Result<()> {
    with_connection(|connection| upsert_with(connection, entry))
}

fn upsert_with(connection: &Connection, entry: &RecentSessionMetadata) -> Result<()> {
    upsert_with_resume(connection, entry, None, None, None)
}

fn upsert_with_resume(
    connection: &Connection,
    entry: &RecentSessionMetadata,
    short_name: Option<&str>,
    fingerprint: Option<&str>,
    updated_at: Option<&str>,
) -> Result<()> {
    connection.execute(
        "INSERT INTO recent_sessions (
             session_id, working_dir, generated_title, custom_title, todo_title,
             saved, updated_at_ms, last_active_at_ms, save_label,
             resume_short_name, resume_fingerprint, resume_updated_at
         ) VALUES (?1, ?2, ?3, ?4, ?5, ?6, ?7, ?8, ?9, ?10, ?11, ?12)
         ON CONFLICT(session_id) DO UPDATE SET
             working_dir = excluded.working_dir,
             generated_title = excluded.generated_title,
             custom_title = excluded.custom_title,
             todo_title = excluded.todo_title,
             saved = excluded.saved,
             updated_at_ms = excluded.updated_at_ms,
             last_active_at_ms = excluded.last_active_at_ms,
             save_label = excluded.save_label,
             resume_short_name = excluded.resume_short_name,
             resume_fingerprint = excluded.resume_fingerprint,
             resume_updated_at = excluded.resume_updated_at",
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
            short_name,
            fingerprint,
            updated_at,
        ],
    )?;
    Ok(())
}

/// All fingerprint-bearing rows, with no recency cutoff. Resume intentionally
/// ignores todo-derived titles, matching Session::display_title rather than
/// the broader recent-session picker's display-title policy.
pub(crate) fn resume_entries() -> Result<std::collections::HashMap<String, ResumeEntry>> {
    with_connection(|connection| {
        let mut statement = connection.prepare(
            "SELECT session_id, resume_short_name, generated_title, custom_title,
                    saved, save_label, resume_updated_at, resume_fingerprint
             FROM recent_sessions WHERE resume_fingerprint IS NOT NULL",
        )?;
        let rows = statement.query_map([], |row| {
            Ok((
                row.get::<_, String>(0)?,
                row.get::<_, Option<String>>(1)?,
                row.get::<_, Option<String>>(2)?,
                row.get::<_, Option<String>>(3)?,
                row.get::<_, bool>(4)?,
                row.get::<_, Option<String>>(5)?,
                row.get::<_, Option<String>>(6)?,
                row.get::<_, String>(7)?,
            ))
        })?;
        let mut entries = std::collections::HashMap::new();
        for row in rows {
            let (id, short_name, generated, custom, saved, label, updated, fingerprint) = row?;
            let Some(updated) = updated else {
                crate::logging::debug(&format!("Resume index row {id} has no timestamp"));
                continue;
            };
            let updated_at = match updated.parse() {
                Ok(updated_at) => updated_at,
                Err(error) => {
                    crate::logging::debug(&format!(
                        "Resume index row {id} has an invalid timestamp: {error}"
                    ));
                    continue;
                }
            };
            let fingerprint = match serde_json::from_str(&fingerprint) {
                Ok(fingerprint) => fingerprint,
                Err(error) => {
                    crate::logging::debug(&format!(
                        "Resume index row {id} has an invalid fingerprint: {error}"
                    ));
                    continue;
                }
            };
            let title = custom
                .as_deref()
                .and_then(non_empty)
                .or_else(|| {
                    saved
                        .then(|| label.as_deref().and_then(non_empty))
                        .flatten()
                })
                .or_else(|| generated.as_deref().and_then(non_empty))
                .map(str::to_owned);
            entries.insert(
                id,
                ResumeEntry {
                    fingerprint,
                    short_name,
                    title,
                    updated_at,
                },
            );
        }
        Ok(entries)
    })
}

pub(crate) fn invalidate_resume_entries(ids: &[&str]) -> Result<()> {
    if ids.is_empty() {
        return Ok(());
    }
    with_connection(|connection| {
        let transaction = connection.unchecked_transaction()?;
        {
            let mut statement = transaction.prepare(
                "UPDATE recent_sessions SET resume_fingerprint = NULL,
                    resume_short_name = NULL, resume_updated_at = NULL WHERE session_id = ?1",
            )?;
            for id in ids {
                statement.execute([id])?;
            }
        }
        transaction.commit()?;
        Ok(())
    })
}

/// Populate older snapshots in the daemon, never on its startup critical path.
pub fn backfill_resume_metadata_in_background() {
    static STARTED: std::sync::atomic::AtomicBool = std::sync::atomic::AtomicBool::new(false);
    if STARTED.swap(true, std::sync::atomic::Ordering::Relaxed) {
        return;
    }
    if let Err(error) = std::thread::Builder::new()
        .name("resume-index-backfill".into())
        .spawn(|| {
            if let Err(error) = crate::session::resume_cache::backfill() {
                crate::logging::warn(&format!("Resume metadata backfill failed: {error}"));
            }
        })
    {
        STARTED.store(false, std::sync::atomic::Ordering::Relaxed);
        crate::logging::warn(&format!("Cannot start resume metadata backfill: {error}"));
    }
}

/// Refresh only the derived title after the todo or plan file changes.
pub fn refresh_todo_title(session_id: &str) -> Result<()> {
    with_connection(|connection| refresh_todo_title_with(connection, session_id))
}

fn refresh_todo_title_with(connection: &Connection, session_id: &str) -> Result<()> {
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
mod resume_tests;

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
        };
        assert_eq!(entry.display_title(), Some("yc mcp"));
        entry.custom_title = Some("Renamed".into());
        assert_eq!(entry.display_title(), Some("Renamed"));
        entry.custom_title = None;
        entry.saved = false;
        assert_eq!(entry.display_title(), Some("Todo goal"));
    }
}
