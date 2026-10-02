//! Durable metadata index for fast recent-session lists.
//!
//! Transcript snapshots can be hundreds of megabytes and a long-lived install
//! can contain 100k+ files. This SQLite index is updated beside normal session
//! persistence and can be queried across daemon, CLI, and API bridge processes.

use std::collections::HashSet;
use std::io::{Read, Seek, SeekFrom};
use std::path::PathBuf;
use std::time::{Duration, UNIX_EPOCH};

use anyhow::{Context, Result, bail};
use chrono::{DateTime, Utc};
use rusqlite::{Connection, OptionalExtension, params};
use serde::{Deserialize, Serialize};

use crate::session::Session;

const MAX_METADATA_HEAD_BYTES: usize = 64 * 1024;
const MAX_METADATA_TAIL_BYTES: usize = 4 * 1024 * 1024;
const MAX_PENDING_METADATA_BYTES: u64 = 64 * 1024;
const LEGACY_BOOTSTRAP_KEY: &str = "legacy-bootstrap-v1";

#[derive(Deserialize)]
struct SnapshotMetadataHeader {
    id: String,
    #[serde(default)]
    title: Option<String>,
    #[serde(default)]
    custom_title: Option<String>,
    updated_at: DateTime<Utc>,
}

#[derive(Deserialize)]
struct SnapshotMetadataSuffix {
    #[allow(dead_code)]
    is_canary: bool,
    #[serde(default)]
    working_dir: Option<String>,
    status: crate::session::SessionStatus,
    #[serde(default)]
    last_active_at: Option<DateTime<Utc>>,
    is_debug: bool,
    saved: bool,
    #[serde(default)]
    save_label: Option<String>,
}

#[derive(Clone, Debug, Deserialize, PartialEq, Eq, Serialize)]
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
         ON recent_sessions(updated_at_ms DESC);
         CREATE TABLE IF NOT EXISTS recent_session_state (
             key TEXT PRIMARY KEY NOT NULL,
             value INTEGER NOT NULL
         );",
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
    query_recent(limit, false)
}

fn recent_eligible(limit: usize) -> Result<Vec<RecentSessionMetadata>> {
    query_recent(limit, true)
}

fn query_recent(limit: usize, eligible_only: bool) -> Result<Vec<RecentSessionMetadata>> {
    let connection = open()?;
    let query = format!(
        "SELECT session_id, working_dir, generated_title, custom_title,
                todo_title, saved, updated_at_ms, last_active_at_ms, save_label,
                status, is_debug
         FROM recent_sessions
         {}
         ORDER BY updated_at_ms DESC, session_id DESC
         LIMIT ?1",
        if eligible_only {
            "WHERE is_debug = 0"
        } else {
            ""
        }
    );
    let mut statement = connection.prepare(&query)?;
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

    recover_pending()?;
    bootstrap_from_snapshots_once()?;
    let entries = recent(500)?;

    for mut entry in entries {
        if entry.status.is_none() || entry.is_debug.is_none() {
            let Ok(snapshot) = metadata_from_snapshot(&entry.session_id) else {
                // Unknown debug state is not safe to expose as a normal session.
                continue;
            };
            entry.status = snapshot.status;
            entry.is_debug = snapshot.is_debug;
            let _ = upsert(&entry);
        }
    }

    let entries = recent_eligible(limit.saturating_mul(4).min(500))?;
    let mut hydrated = Vec::with_capacity(entries.len());
    for entry in entries {
        if entry.session_id.starts_with("imported_")
            || !crate::session::session_exists(&entry.session_id)
        {
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

fn bootstrap_from_snapshots_once() -> Result<()> {
    let connection = open()?;
    let complete = connection
        .query_row(
            "SELECT value FROM recent_session_state WHERE key = ?1",
            [LEGACY_BOOTSTRAP_KEY],
            |row| row.get::<_, i64>(0),
        )
        .optional()?
        .is_some_and(|value| value != 0);
    if complete {
        return Ok(());
    }

    let sessions_dir = crate::storage::jcode_dir()?.join("sessions");
    let mut statement = connection.prepare("SELECT session_id FROM recent_sessions")?;
    let existing = statement
        .query_map([], |row| row.get::<_, String>(0))?
        .collect::<rusqlite::Result<HashSet<_>>>()?;
    drop(statement);
    drop(connection);
    let Ok(directory) = std::fs::read_dir(sessions_dir) else {
        mark_legacy_bootstrap_complete()?;
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

    for (_, session_id) in candidates {
        if let Ok(metadata) = metadata_from_snapshot(&session_id) {
            let _ = upsert(&metadata);
        }
    }
    mark_legacy_bootstrap_complete()?;
    Ok(())
}

fn metadata_from_snapshot(session_id: &str) -> Result<RecentSessionMetadata> {
    let path = crate::session::session_path(session_id)?;
    let mut file = std::fs::File::open(path)?;
    let file_len = file.metadata()?.len();

    let head_len = usize::try_from(file_len.min(MAX_METADATA_HEAD_BYTES as u64))?;
    let mut head = vec![0; head_len];
    file.read_exact(&mut head)?;
    let header = snapshot_metadata_header(&head)?;
    if header.id != session_id {
        bail!("session snapshot ID does not match its filename");
    }

    let tail_len = usize::try_from(file_len.min(MAX_METADATA_TAIL_BYTES as u64))?;
    file.seek(SeekFrom::End(-i64::try_from(tail_len)?))?;
    let mut tail = vec![0; tail_len];
    file.read_exact(&mut tail)?;
    let suffix = snapshot_metadata_suffix(&tail)
        .context("session snapshot has no bounded metadata suffix")?;

    Ok(RecentSessionMetadata {
        session_id: header.id,
        working_dir: suffix.working_dir,
        generated_title: header.title,
        custom_title: header.custom_title,
        todo_title: crate::todo::load_session_title(session_id),
        saved: suffix.saved,
        save_label: suffix.save_label,
        updated_at_ms: header.updated_at.timestamp_millis(),
        last_active_at_ms: suffix.last_active_at.map(|time| time.timestamp_millis()),
        status: Some(suffix.status.display().to_string()),
        is_debug: Some(suffix.is_debug),
    })
}

fn snapshot_metadata_header(bytes: &[u8]) -> Result<SnapshotMetadataHeader> {
    const MESSAGES_FIELD: &[u8] = b"\"messages\":";
    let field_start = bytes
        .windows(MESSAGES_FIELD.len())
        .position(|window| window == MESSAGES_FIELD)
        .context("session snapshot header has no messages field")?;
    let mut json = bytes[..field_start + MESSAGES_FIELD.len()].to_vec();
    json.extend_from_slice(b"[]}");
    Ok(serde_json::from_slice(&json)?)
}

fn snapshot_metadata_suffix(bytes: &[u8]) -> Option<SnapshotMetadataSuffix> {
    const ANCHOR: &[u8] = b"\"is_canary\":";
    bytes
        .windows(ANCHOR.len())
        .enumerate()
        .filter_map(|(start, window)| (window == ANCHOR).then_some(start))
        .find_map(|start| parse_snapshot_metadata_suffix(&bytes[start..]))
}

fn parse_snapshot_metadata_suffix(bytes: &[u8]) -> Option<SnapshotMetadataSuffix> {
    const FIELD_ORDER: &[&str] = &[
        "is_canary",
        "testing_build",
        "working_dir",
        "short_name",
        "status",
        "last_pid",
        "last_active_at",
        "is_debug",
        "saved",
        "save_label",
    ];
    const FOLLOWING_FIELDS: &[&str] = &[
        "env_snapshots",
        "memory_injections",
        "replay_events",
        "migration_epoch",
    ];

    let mut cursor = 0usize;
    let mut last_order = None;
    let mut saw_status = false;
    let mut saw_debug = false;
    let mut saw_saved = false;
    let mut object = vec![b'{'];
    loop {
        let (field, after_field) = json_field_name(bytes, cursor)?;
        let Some(order) = FIELD_ORDER.iter().position(|candidate| *candidate == field) else {
            break;
        };
        if last_order.is_some_and(|previous| order <= previous) {
            return None;
        }
        let colon = skip_json_whitespace(bytes, after_field);
        if bytes.get(colon) != Some(&b':') {
            return None;
        }
        let value_start = skip_json_whitespace(bytes, colon + 1);
        let value_end = json_value_end(bytes, value_start)?;
        if object.len() > 1 {
            object.push(b',');
        }
        object.extend_from_slice(&bytes[cursor..value_end]);
        saw_status |= field == "status";
        saw_debug |= field == "is_debug";
        saw_saved |= field == "saved";
        last_order = Some(order);

        cursor = skip_json_whitespace(bytes, value_end);
        if bytes.get(cursor) != Some(&b',') {
            break;
        }
        cursor = skip_json_whitespace(bytes, cursor + 1);
    }
    if !saw_status || !saw_debug || !saw_saved {
        return None;
    }
    if bytes.get(cursor) != Some(&b'}') {
        let (next_field, _) = json_field_name(bytes, cursor)?;
        if !FOLLOWING_FIELDS.contains(&next_field) {
            return None;
        }
    }
    object.push(b'}');
    serde_json::from_slice(&object).ok()
}

fn json_field_name(bytes: &[u8], start: usize) -> Option<(&str, usize)> {
    let start = skip_json_whitespace(bytes, start);
    if bytes.get(start) != Some(&b'"') {
        return None;
    }
    let end = bytes[start + 1..].iter().position(|byte| *byte == b'"')? + start + 1;
    let field = std::str::from_utf8(&bytes[start + 1..end]).ok()?;
    Some((field, end + 1))
}

fn skip_json_whitespace(bytes: &[u8], mut cursor: usize) -> usize {
    while bytes.get(cursor).is_some_and(u8::is_ascii_whitespace) {
        cursor += 1;
    }
    cursor
}

fn json_value_end(bytes: &[u8], start: usize) -> Option<usize> {
    let first = *bytes.get(start)?;
    if first == b'"' {
        let mut escaped = false;
        for (offset, byte) in bytes[start + 1..].iter().enumerate() {
            if escaped {
                escaped = false;
            } else if *byte == b'\\' {
                escaped = true;
            } else if *byte == b'"' {
                return Some(start + offset + 2);
            }
        }
        return None;
    }
    if matches!(first, b'{' | b'[') {
        let mut depth = 0usize;
        let mut in_string = false;
        let mut escaped = false;
        for (offset, byte) in bytes[start..].iter().enumerate() {
            if in_string {
                if escaped {
                    escaped = false;
                } else if *byte == b'\\' {
                    escaped = true;
                } else if *byte == b'"' {
                    in_string = false;
                }
                continue;
            }
            match *byte {
                b'"' => in_string = true,
                b'{' | b'[' => depth += 1,
                b'}' | b']' => {
                    depth = depth.checked_sub(1)?;
                    if depth == 0 {
                        return Some(start + offset + 1);
                    }
                }
                _ => {}
            }
        }
        return None;
    }
    bytes[start..]
        .iter()
        .position(|byte| matches!(*byte, b',' | b'}' | b']') || byte.is_ascii_whitespace())
        .map(|offset| start + offset)
        .or(Some(bytes.len()))
}

fn mark_legacy_bootstrap_complete() -> Result<()> {
    open()?.execute(
        "INSERT INTO recent_session_state (key, value) VALUES (?1, 1)
         ON CONFLICT(key) DO UPDATE SET value = 1",
        [LEGACY_BOOTSTRAP_KEY],
    )?;
    Ok(())
}

fn pending_dir() -> Result<PathBuf> {
    Ok(crate::storage::jcode_dir()?.join("session-index-pending"))
}

fn pending_path(session_id: &str) -> Result<PathBuf> {
    if !valid_session_id(session_id) {
        bail!("invalid session ID for pending metadata");
    }
    Ok(pending_dir()?.join(format!("{session_id}.json")))
}

fn write_pending(entry: &RecentSessionMetadata) -> Result<()> {
    if !valid_session_id(&entry.session_id) {
        bail!("invalid session ID for pending metadata");
    }
    let directory = pending_dir()?;
    std::fs::create_dir_all(&directory)?;
    crate::storage::write_json_fast(&directory.join(format!("{}.json", entry.session_id)), entry)
}

fn clear_pending(session_id: &str) {
    let Ok(path) = pending_path(session_id) else {
        return;
    };
    let _ = std::fs::remove_file(path);
}

fn recover_pending() -> Result<()> {
    let directory = pending_dir()?;
    let Ok(entries) = std::fs::read_dir(directory) else {
        return Ok(());
    };
    for path in entries.flatten().map(|entry| entry.path()) {
        let Some(session_id) = path.file_stem().and_then(|stem| stem.to_str()) else {
            continue;
        };
        if !valid_session_id(session_id) {
            continue;
        }
        if path
            .metadata()
            .is_ok_and(|metadata| metadata.len() > MAX_PENDING_METADATA_BYTES)
        {
            continue;
        }
        let Ok(bytes) = std::fs::read(&path) else {
            continue;
        };
        let Ok(metadata) = serde_json::from_slice::<RecentSessionMetadata>(&bytes) else {
            continue;
        };
        if metadata.session_id != session_id {
            continue;
        }
        let indexed_timestamp = open()
            .and_then(|connection| {
                connection
                    .query_row(
                        "SELECT updated_at_ms FROM recent_sessions WHERE session_id = ?1",
                        [session_id],
                        |row| row.get::<_, i64>(0),
                    )
                    .optional()
                    .map_err(Into::into)
            })
            .ok()
            .flatten();
        if indexed_timestamp.is_some_and(|timestamp| timestamp >= metadata.updated_at_ms) {
            clear_pending(session_id);
            continue;
        }
        if upsert(&metadata).is_ok() {
            clear_pending(session_id);
        }
    }
    Ok(())
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
    let metadata = metadata_from_session(session);
    match upsert(&metadata) {
        Ok(()) => {
            clear_pending(&metadata.session_id);
            Ok(())
        }
        Err(error) => {
            let _ = write_pending(&metadata);
            Err(error)
        }
    }
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
             is_debug = excluded.is_debug
         WHERE excluded.updated_at_ms >= recent_sessions.updated_at_ms",
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
    fn debug_rows_do_not_consume_the_picker_limit() {
        let _env_lock = crate::storage::lock_test_env();
        let previous_home = std::env::var_os("JCODE_HOME");
        let home = tempfile::tempdir().expect("create temporary jcode home");
        crate::env::set_var("JCODE_HOME", home.path());

        let mut normal = Session::create_with_id(
            "session_normal_older".to_string(),
            None,
            Some("Normal".to_string()),
        );
        normal.save_prepared().expect("persist normal session");
        let normal_updated_at = normal.updated_at.timestamp_millis();

        for index in 0..501 {
            upsert(&RecentSessionMetadata {
                session_id: format!("session_debug_newer_{index}"),
                working_dir: None,
                generated_title: None,
                custom_title: None,
                todo_title: None,
                saved: false,
                save_label: None,
                updated_at_ms: normal_updated_at + i64::from(index) + 1,
                last_active_at_ms: None,
                status: Some("active".to_string()),
                is_debug: Some(true),
            })
            .expect("index debug session");
        }
        mark_legacy_bootstrap_complete().expect("mark bootstrap complete");

        let sessions = recent_persisted(1).expect("list eligible session");
        assert_eq!(sessions.len(), 1);
        assert_eq!(sessions[0].session_id, normal.id);

        match previous_home {
            Some(value) => crate::env::set_var("JCODE_HOME", value),
            None => crate::env::remove_var("JCODE_HOME"),
        }
    }

    #[test]
    fn legacy_hydration_preserves_newer_index_timestamp() {
        let _env_lock = crate::storage::lock_test_env();
        let previous_home = std::env::var_os("JCODE_HOME");
        let home = tempfile::tempdir().expect("create temporary jcode home");
        crate::env::set_var("JCODE_HOME", home.path());

        let mut session = Session::create_with_id(
            "session_newer_index".to_string(),
            None,
            Some("Newer index".to_string()),
        );
        session.save_prepared().expect("persist snapshot");
        let newer_timestamp = session.updated_at.timestamp_millis() + 10_000;
        open()
            .expect("open index")
            .execute(
                "UPDATE recent_sessions
                 SET updated_at_ms = ?2, status = NULL, is_debug = NULL
                 WHERE session_id = ?1",
                params![session.id, newer_timestamp],
            )
            .expect("simulate newer journal-backed legacy row");
        mark_legacy_bootstrap_complete().expect("mark bootstrap complete");

        let sessions = recent_persisted(1).expect("hydrate legacy row");
        assert_eq!(sessions.len(), 1);
        assert_eq!(sessions[0].updated_at_ms, newer_timestamp);
        let indexed = recent(1).expect("read hydrated row");
        assert_eq!(indexed[0].updated_at_ms, newer_timestamp);
        assert_eq!(indexed[0].is_debug, Some(false));

        match previous_home {
            Some(value) => crate::env::set_var("JCODE_HOME", value),
            None => crate::env::remove_var("JCODE_HOME"),
        }
    }

    #[test]
    fn completed_bootstrap_uses_pending_metadata_without_rescanning_snapshots() {
        let _env_lock = crate::storage::lock_test_env();
        let previous_home = std::env::var_os("JCODE_HOME");
        let home = tempfile::tempdir().expect("create temporary jcode home");
        crate::env::set_var("JCODE_HOME", home.path());
        std::fs::create_dir_all(home.path().join("sessions")).expect("create session directory");
        mark_legacy_bootstrap_complete().expect("mark bootstrap complete");

        let mut session = Session::create_with_id(
            "session_pending".to_string(),
            None,
            Some("Pending".to_string()),
        );
        session.working_dir = Some("/pending".to_string());
        crate::storage::write_json_fast(
            &crate::session::session_path(&session.id).expect("session path"),
            &session,
        )
        .expect("write snapshot without index");
        assert!(
            recent_persisted(1)
                .expect("skip completed legacy scan")
                .is_empty()
        );

        let metadata = metadata_from_session(&session);
        write_pending(&metadata).expect("write failed-index recovery metadata");
        let sessions = recent_persisted(1).expect("recover pending metadata");
        assert_eq!(sessions.len(), 1);
        assert_eq!(sessions[0].session_id, session.id);
        assert!(!pending_path(&session.id).expect("pending path").exists());

        let oversized_id = "session_oversized_legacy";
        std::fs::write(
            crate::session::session_path(oversized_id).expect("oversized snapshot path"),
            vec![b' '; MAX_METADATA_TAIL_BYTES + 1],
        )
        .expect("write oversized snapshot");
        assert!(metadata_from_snapshot(oversized_id).is_err());

        match previous_home {
            Some(value) => crate::env::set_var("JCODE_HOME", value),
            None => crate::env::remove_var("JCODE_HOME"),
        }
    }

    #[test]
    fn bootstrap_indexes_all_candidates_and_reads_large_transcripts_boundedly() {
        let _env_lock = crate::storage::lock_test_env();
        let previous_home = std::env::var_os("JCODE_HOME");
        let home = tempfile::tempdir().expect("create temporary jcode home");
        crate::env::set_var("JCODE_HOME", home.path());
        std::fs::create_dir_all(home.path().join("sessions")).expect("create session directory");

        for index in 0..6 {
            let session = Session::create_with_id(
                format!("session_legacy_{index}"),
                None,
                Some(format!("Legacy {index}")),
            );
            let encoded = serde_json::to_vec(&session).expect("serialize legacy session");
            std::fs::write(
                crate::session::session_path(&session.id).expect("legacy session path"),
                encoded,
            )
            .expect("write legacy snapshot");
        }

        let large = Session::create_with_id(
            "session_large_legacy".to_string(),
            None,
            Some("Large legacy".to_string()),
        );
        let encoded = serde_json::to_string(&large).expect("serialize large session");
        let large_messages = format!("\"messages\":[{{\"blob\":\"{}\"}}]", "x".repeat(512 * 1024));
        let encoded = encoded.replacen("\"messages\":[]", &large_messages, 1);
        assert!(encoded.len() > 256 * 1024);
        std::fs::write(
            crate::session::session_path(&large.id).expect("large session path"),
            encoded,
        )
        .expect("write large legacy snapshot");

        recent_persisted(1).expect("bootstrap all legacy sessions");
        let indexed = recent(20).expect("read complete legacy index");
        assert_eq!(indexed.len(), 7);
        assert!(indexed.iter().any(|entry| entry.session_id == large.id));

        match previous_home {
            Some(value) => crate::env::set_var("JCODE_HOME", value),
            None => crate::env::remove_var("JCODE_HOME"),
        }
    }

    #[test]
    fn pending_recovery_does_not_overwrite_newer_index_metadata() {
        let _env_lock = crate::storage::lock_test_env();
        let previous_home = std::env::var_os("JCODE_HOME");
        let home = tempfile::tempdir().expect("create temporary jcode home");
        crate::env::set_var("JCODE_HOME", home.path());

        let mut session = Session::create_with_id(
            "session_recovery_race".to_string(),
            None,
            Some("Older title".to_string()),
        );
        session.save_prepared().expect("persist session");
        let mut stale = metadata_from_session(&session);
        stale.updated_at_ms -= 1;

        session.title = Some("Newer title".to_string());
        session.save_prepared().expect("persist newer session");
        let newer_timestamp = session.updated_at.timestamp_millis();
        write_pending(&stale).expect("write stale pending metadata after newer save");

        recover_pending().expect("recover pending metadata");
        let indexed = recent(1).expect("read recovered index");
        assert_eq!(indexed[0].generated_title.as_deref(), Some("Newer title"));
        assert_eq!(indexed[0].updated_at_ms, newer_timestamp);
        assert!(!pending_path(&session.id).expect("pending path").exists());

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
        let snapshot = serde_json::to_string(&session)
            .expect("serialize session")
            .replacen(
                "\"messages\":[]",
                r#""messages":[{"working_dir":"/nested","is_debug":true,"status":"crashed"}]"#,
                1,
            );
        std::fs::create_dir_all(home.path().join("sessions")).expect("create session directory");
        std::fs::write(
            crate::session::session_path(&session.id).expect("session path"),
            snapshot,
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
