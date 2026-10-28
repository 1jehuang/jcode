use anyhow::{Context, Result, bail};
use rusqlite::{Connection, OptionalExtension, Transaction, TransactionBehavior, params};
use serde::{Serialize, de::DeserializeOwned};
use std::path::{Path, PathBuf};
use std::sync::{Mutex, MutexGuard};

use super::{SessionListOptions, SessionSearchCandidate, SessionStorage, SessionSummary};
use crate::session::journal::PersistVectorMode;
use crate::session::{Session, StoredMessage};

pub(super) struct SqliteSessionStorage {
    root: PathBuf,
    path: PathBuf,
    connection: Mutex<Connection>,
}

struct ExistingSession {
    revision: i64,
    summary: String,
    size_bytes: i64,
    transcript_revision: i64,
    metadata: String,
}

impl SqliteSessionStorage {
    pub(super) fn open(root: PathBuf) -> Result<Self> {
        crate::storage::ensure_dir(&root)?;
        jcode_core::fs::set_directory_permissions_owner_only(&root)?;
        let path = root.join("sessions.sqlite3");
        std::fs::OpenOptions::new()
            .create(true)
            .write(true)
            .truncate(false)
            .open(&path)?;
        jcode_core::fs::set_permissions_owner_only(&path)?;
        let mut connection = Connection::open(&path)?;
        connection.busy_timeout(std::time::Duration::from_secs(5))?;
        connection.execute_batch(
            "PRAGMA journal_mode=WAL; PRAGMA synchronous=FULL; PRAGMA foreign_keys=ON;",
        )?;
        let version: i64 = connection.query_row("PRAGMA user_version", [], |row| row.get(0))?;
        if version > 7 {
            bail!("Unsupported session database schema {version}");
        }
        if version == 0 {
            connection.execute_batch(
                "BEGIN IMMEDIATE;
                 CREATE TABLE IF NOT EXISTS sessions (
                   id TEXT PRIMARY KEY, metadata TEXT NOT NULL, summary TEXT NOT NULL,
                   updated_ms INTEGER NOT NULL, saved INTEGER NOT NULL,
                   updated_seconds INTEGER NOT NULL, updated_nanos INTEGER NOT NULL,
                   revision INTEGER NOT NULL, size_bytes INTEGER NOT NULL,
                   transcript_revision INTEGER NOT NULL,
                   discovery_seconds INTEGER NOT NULL DEFAULT 0,
                   discovery_nanos INTEGER NOT NULL DEFAULT 0,
                   sort_key INTEGER NOT NULL DEFAULT 0
                 );
                 CREATE INDEX IF NOT EXISTS sessions_discovery ON sessions(discovery_seconds DESC,discovery_nanos DESC,sort_key DESC,id DESC);
                 CREATE INDEX IF NOT EXISTS sessions_saved_discovery ON sessions(discovery_seconds DESC,discovery_nanos DESC,sort_key DESC,id DESC) WHERE saved=1;
                 CREATE INDEX IF NOT EXISTS sessions_updated_precise ON sessions(updated_seconds DESC,updated_nanos DESC,id);
                 CREATE INDEX IF NOT EXISTS sessions_saved_precise ON sessions(saved,updated_seconds DESC,updated_nanos DESC,id);
                 CREATE TABLE IF NOT EXISTS session_messages (
                   session_id TEXT NOT NULL REFERENCES sessions(id) ON DELETE CASCADE,
                   position INTEGER NOT NULL, data TEXT NOT NULL,
                   PRIMARY KEY(session_id, position)
                 );
                 CREATE TABLE IF NOT EXISTS session_replay (
                   session_id TEXT NOT NULL REFERENCES sessions(id) ON DELETE CASCADE,
                   position INTEGER NOT NULL, data TEXT NOT NULL,
                   PRIMARY KEY(session_id, position)
                 );
                 CREATE TABLE IF NOT EXISTS session_environment (
                   session_id TEXT NOT NULL REFERENCES sessions(id) ON DELETE CASCADE,
                   position INTEGER NOT NULL, data TEXT NOT NULL,
                   PRIMARY KEY(session_id, position)
                 );
                 CREATE TABLE IF NOT EXISTS session_memory (
                   session_id TEXT NOT NULL REFERENCES sessions(id) ON DELETE CASCADE,
                   position INTEGER NOT NULL, data TEXT NOT NULL,
                   PRIMARY KEY(session_id, position)
                 );
                 CREATE TABLE IF NOT EXISTS session_compaction (
                   session_id TEXT PRIMARY KEY REFERENCES sessions(id) ON DELETE CASCADE,
                   data TEXT NOT NULL
                 );
                 PRAGMA user_version=4;
                 COMMIT;",
            )?;
        }
        if version == 1 {
            migrate_precise_ordering(&mut connection)?;
        }
        if version == 1 || version == 2 {
            migrate_transcript_revision(&mut connection)?;
        }
        if version > 0 && version < 4 {
            migrate_discovery_ordering(&mut connection)?;
        }
        super::sqlite_fts::migrate(&mut connection)?;
        migrate_picker_flags(&mut connection)?;
        harden_database_files(&path)?;
        Ok(Self {
            root,
            path,
            connection: Mutex::new(connection),
        })
    }

    fn connection(&self) -> Result<MutexGuard<'_, Connection>> {
        let connection = self
            .connection
            .lock()
            .unwrap_or_else(|err| err.into_inner());
        // Establish a read transaction before hardening so SQLite has opened
        // any recreated WAL/SHM files, even under a group-writable host umask.
        connection.query_row("PRAGMA schema_version", [], |row| row.get::<_, i64>(0))?;
        harden_database_files(&self.path)?;
        Ok(connection)
    }

    fn import_legacy(&self, id: &str) -> Result<()> {
        if self.contains_native(id)? {
            return Ok(());
        }
        let path = crate::session::storage_paths::session_path_in_dir(&self.root, id);
        // Read-only replay does not repair or remove even a corrupt legacy journal.
        let mut session = Session::load_file_without_title_backfill(&path)
            .with_context(|| format!("Loading legacy session {id}"))?;
        if session.id != id {
            bail!("Legacy session id does not match its filename");
        }
        self.write(&mut session, true)?;
        Ok(())
    }

    fn read(&self, id: &str, stub: bool) -> Result<Session> {
        self.import_legacy(id)?;
        let mut connection = self.connection()?;
        let transaction = connection.transaction()?;
        let (metadata, revision, transcript_revision): (String, i64, i64) = transaction.query_row(
            "SELECT metadata, revision, transcript_revision FROM sessions WHERE id=?1",
            [id],
            |row| Ok((row.get(0)?, row.get(1)?, row.get(2)?)),
        )?;
        let mut session: Session = serde_json::from_str(&metadata)?;
        let compaction: Option<String> = transaction
            .query_row(
                "SELECT data FROM session_compaction WHERE session_id=?1",
                [id],
                |row| row.get(0),
            )
            .optional()?;
        session.compaction = compaction
            .map(|data| serde_json::from_str(&data))
            .transpose()?;
        if !stub {
            session.messages = read_vector(&transaction, "session_messages", id)?;
            session.replay_events = read_vector(&transaction, "session_replay", id)?;
            session.env_snapshots = read_vector(&transaction, "session_environment", id)?;
            session.memory_injections = read_vector(&transaction, "session_memory", id)?;
        }
        transaction.commit()?;
        session.backfill_prompt_title();
        session.reset_provider_messages_cache();
        session.mark_memory_profile_dirty();
        session.persist_state.store_key = Some(self.path.clone());
        session.persist_state.sqlite_revision = Some(revision);
        session.persist_state.sqlite_transcript_revision = Some(transcript_revision);
        session.persist_state.sqlite_metadata = Some(metadata);
        session.reset_persist_state(true);
        session.persist_state.transcript_stripped = stub;
        if !stub {
            session.decay_old_tool_images();
        }
        Ok(session)
    }

    fn write(&self, session: &mut Session, import_only: bool) -> Result<()> {
        let mut connection = self.connection()?;
        let transaction = connection.transaction_with_behavior(TransactionBehavior::Immediate)?;
        let existing: Option<ExistingSession> = transaction
            .query_row(
                "SELECT revision, summary, size_bytes, transcript_revision, metadata FROM sessions WHERE id=?1",
                [&session.id],
                |row| Ok(ExistingSession { revision: row.get(0)?, summary: row.get(1)?, size_bytes: row.get(2)?, transcript_revision: row.get(3)?, metadata: row.get(4)? }),
            )
            .optional()?;
        if import_only && existing.is_some() {
            return Ok(());
        }
        if session.persist_state.transcript_stripped {
            bail!(
                "Refusing to save a stripped remote-client session over its authoritative transcript"
            );
        }
        if existing.is_some()
            && session.messages.is_empty()
            && session.persist_state.messages_len > 0
        {
            bail!("Refusing to replace a persisted transcript with an empty session");
        }
        let same_store = session.persist_state.store_key.as_ref() == Some(&self.path);
        if let Some(previous) = &existing {
            if !same_store
                || session.persist_state.sqlite_transcript_revision
                    != Some(previous.transcript_revision)
            {
                bail!(
                    "Session {} changed in SQLite; reload it before saving",
                    session.id
                );
            }
        }
        let initial = existing.is_none();
        let revision = existing
            .as_ref()
            .map_or(1, |previous| previous.revision + 1);
        let state = &session.persist_state;
        let mut metadata = metadata_json(session)?;
        let rebased: Option<Session> = if let Some(previous) = &existing {
            if state.sqlite_revision != Some(previous.revision) {
                let baseline = state
                    .sqlite_metadata
                    .as_deref()
                    .context("Missing SQLite metadata baseline; reload before saving")?;
                metadata = rebase_metadata(baseline, &metadata, &previous.metadata)?;
                Some(serde_json::from_str(&metadata)?)
            } else {
                None
            }
        } else {
            None
        };
        let messages_full = initial
            || !same_store
            || state.messages_mode == PersistVectorMode::Full
            || session.messages.len() < state.messages_len;
        let durable_metadata = rebased.as_ref().unwrap_or(session);
        let mut summary = SessionSummary::metadata(durable_metadata);
        summary.discovery_modified_at = if import_only {
            state
                .store_key
                .as_deref()
                .and_then(super::file_discovery_modified_at)
        } else {
            Some(chrono::Utc::now())
        };
        if !messages_full {
            let previous: SessionSummary =
                serde_json::from_str(&existing.as_ref().unwrap().summary)?;
            summary.visible_message_count = previous.visible_message_count;
            summary.user_message_count = previous.user_message_count;
            summary.assistant_message_count = previous.assistant_message_count;
            summary.estimated_tokens = previous.estimated_tokens;
            summary.first_user_prompt = previous.first_user_prompt;
            summary.search_text = previous.search_text;
        }
        summary.add_messages(if messages_full {
            &session.messages
        } else {
            &session.messages[state.messages_len..]
        });
        let (discovery_seconds, discovery_nanos) = super::discovery_key(&summary);
        let sort_key = super::sqlite_sort_key(&session.id);
        let picker_debug = summary.is_debug;
        let picker_has_history = summary.visible_message_count > 0;
        let summary = serde_json::to_string(&summary)?;
        let compaction_changed = initial
            || state
                .last_meta
                .as_ref()
                .is_none_or(|previous| previous.compaction != session.compaction);
        let transcript_changed = messages_full
            || compaction_changed
            || state.replay_events_mode == PersistVectorMode::Full
            || state.replay_events_len != session.replay_events.len()
            || state.env_snapshots_mode == PersistVectorMode::Full
            || state.env_snapshots_len != session.env_snapshots.len()
            || state.memory_injections_mode == PersistVectorMode::Full
            || state.memory_injections_len != session.memory_injections.len()
            || state.messages_len != session.messages.len();
        let transcript_revision = existing.as_ref().map_or(1, |previous| {
            previous.transcript_revision + i64::from(transcript_changed)
        });
        transaction.execute(
            "INSERT INTO sessions(id,metadata,summary,updated_ms,saved,revision,size_bytes,updated_seconds,updated_nanos,transcript_revision,discovery_seconds,discovery_nanos,sort_key,picker_debug,picker_has_history)
             VALUES(?1,?2,?3,?4,?5,?6,0,?7,?8,?9,?10,?11,?12,?13,?14)
             ON CONFLICT(id) DO UPDATE SET metadata=excluded.metadata,summary=excluded.summary,
               updated_ms=excluded.updated_ms,saved=excluded.saved,revision=excluded.revision,
               updated_seconds=excluded.updated_seconds,updated_nanos=excluded.updated_nanos,
               transcript_revision=excluded.transcript_revision,
               discovery_seconds=excluded.discovery_seconds,discovery_nanos=excluded.discovery_nanos,
               sort_key=excluded.sort_key,picker_debug=excluded.picker_debug,
               picker_has_history=excluded.picker_has_history",
            params![
                session.id,
                metadata,
                summary,
                durable_metadata.updated_at.timestamp_millis(),
                durable_metadata.saved,
                revision,
                durable_metadata.updated_at.timestamp(),
                durable_metadata.updated_at.timestamp_subsec_nanos(),
                transcript_revision,
                discovery_seconds,
                discovery_nanos,
                sort_key,
                picker_debug,
                picker_has_history
            ],
        )?;
        let mut size_delta = write_vector(
            &transaction,
            "session_messages",
            &session.id,
            &session.messages,
            state.messages_len,
            state.messages_mode,
            initial || !same_store,
        )?;
        size_delta += write_vector(
            &transaction,
            "session_replay",
            &session.id,
            &session.replay_events,
            state.replay_events_len,
            state.replay_events_mode,
            initial || !same_store,
        )?;
        size_delta += write_vector(
            &transaction,
            "session_environment",
            &session.id,
            &session.env_snapshots,
            state.env_snapshots_len,
            state.env_snapshots_mode,
            initial || !same_store,
        )?;
        size_delta += write_vector(
            &transaction,
            "session_memory",
            &session.id,
            &session.memory_injections,
            state.memory_injections_len,
            state.memory_injections_mode,
            initial || !same_store,
        )?;
        if compaction_changed {
            let previous: i64 = transaction.query_row(
                "SELECT COALESCE(SUM(length(CAST(data AS BLOB))),0) FROM session_compaction WHERE session_id=?1",
                [&session.id], |row| row.get(0),
            )?;
            transaction.execute(
                "DELETE FROM session_compaction WHERE session_id=?1",
                [&session.id],
            )?;
            size_delta -= previous;
            if let Some(compaction) = &session.compaction {
                let data = serde_json::to_string(compaction)?;
                size_delta += data.len() as i64;
                transaction.execute(
                    "INSERT INTO session_compaction(session_id,data) VALUES(?1,?2)",
                    params![session.id, data],
                )?;
            }
        }
        let size_bytes = existing.as_ref().map_or(0, |previous| previous.size_bytes) + size_delta;
        transaction.execute(
            "UPDATE sessions SET size_bytes=?2 WHERE id=?1",
            params![session.id, size_bytes],
        )?;
        transaction.commit()?;
        if let Some(rebased) = rebased {
            let mut meta = rebased.journal_meta();
            meta.compaction_unchanged = true;
            session.created_at = rebased.created_at;
            session.route_api_method = rebased.route_api_method;
            session.extra = rebased.extra;
            session.apply_journal_meta(meta);
        }
        session.persist_state.store_key = Some(self.path.clone());
        session.persist_state.sqlite_revision = Some(revision);
        session.persist_state.sqlite_transcript_revision = Some(transcript_revision);
        session.persist_state.sqlite_metadata = Some(metadata);
        session.reset_persist_state(true);
        Ok(())
    }
}

fn rebase_metadata(baseline: &str, current: &str, latest: &str) -> Result<String> {
    let baseline: serde_json::Value = serde_json::from_str(baseline)?;
    let current: serde_json::Value = serde_json::from_str(current)?;
    let mut latest: serde_json::Value = serde_json::from_str(latest)?;
    let baseline = baseline
        .as_object()
        .context("Invalid baseline session metadata")?;
    let current = current
        .as_object()
        .context("Invalid current session metadata")?;
    let latest = latest
        .as_object_mut()
        .context("Invalid latest session metadata")?;
    let keys: std::collections::BTreeSet<_> = baseline.keys().chain(current.keys()).collect();
    for key in keys {
        if current.get(key) != baseline.get(key) {
            if let Some(value) = current.get(key) {
                latest.insert(key.clone(), value.clone());
            } else {
                latest.remove(key);
            }
        }
    }
    Ok(serde_json::to_string(latest)?)
}

fn migrate_transcript_revision(connection: &mut Connection) -> Result<()> {
    let transaction = connection.transaction_with_behavior(TransactionBehavior::Immediate)?;
    let version: i64 = transaction.query_row("PRAGMA user_version", [], |row| row.get(0))?;
    if version == 2 {
        transaction.execute_batch(
            "ALTER TABLE sessions ADD COLUMN transcript_revision INTEGER NOT NULL DEFAULT 0;
             PRAGMA user_version=3;",
        )?;
    }
    transaction.commit()?;
    Ok(())
}

/// Add indexed picker ordering columns (schema v4) in one transaction.
fn migrate_discovery_ordering(connection: &mut Connection) -> Result<()> {
    let transaction = connection.transaction_with_behavior(TransactionBehavior::Immediate)?;
    let version: i64 = transaction.query_row("PRAGMA user_version", [], |row| row.get(0))?;
    if version == 3 {
        transaction.execute_batch(
            "ALTER TABLE sessions ADD COLUMN discovery_seconds INTEGER NOT NULL DEFAULT 0;
             ALTER TABLE sessions ADD COLUMN discovery_nanos INTEGER NOT NULL DEFAULT 0;
             ALTER TABLE sessions ADD COLUMN sort_key INTEGER NOT NULL DEFAULT 0;",
        )?;
        let rows = {
            let mut statement = transaction.prepare(
                "SELECT id,json_extract(summary,'$.discovery_modified_at'),updated_seconds,updated_nanos FROM sessions",
            )?;
            let rows = statement.query_map([], |row| {
                Ok((
                    row.get::<_, String>(0)?,
                    row.get::<_, Option<String>>(1)?,
                    row.get::<_, i64>(2)?,
                    row.get::<_, i64>(3)?,
                ))
            })?;
            rows.collect::<rusqlite::Result<Vec<_>>>()?
        };
        let mut update = transaction.prepare(
            "UPDATE sessions SET discovery_seconds=?2,discovery_nanos=?3,sort_key=?4 WHERE id=?1",
        )?;
        for (id, discovery, seconds, nanos) in rows {
            let timestamp = match discovery {
                Some(text) => {
                    Some(chrono::DateTime::parse_from_rfc3339(&text)?.with_timezone(&chrono::Utc))
                }
                None => chrono::DateTime::from_timestamp(seconds, u32::try_from(nanos)?),
            }
            .context("Invalid session timestamp")?;
            let (discovery_seconds, discovery_nanos) = super::discovery_key_at(timestamp);
            update.execute(params![
                id,
                discovery_seconds,
                discovery_nanos,
                super::sqlite_sort_key(&id)
            ])?;
        }
        drop(update);
        transaction.execute_batch(
            "CREATE INDEX sessions_discovery ON sessions(discovery_seconds DESC,discovery_nanos DESC,sort_key DESC,id DESC);
             CREATE INDEX sessions_saved_discovery ON sessions(discovery_seconds DESC,discovery_nanos DESC,sort_key DESC,id DESC) WHERE saved=1;
             PRAGMA user_version=4;",
        )?;
    }
    transaction.commit()?;
    Ok(())
}

fn migrate_picker_flags(connection: &mut Connection) -> Result<()> {
    let version: i64 = connection.query_row("PRAGMA user_version", [], |row| row.get(0))?;
    if version >= 7 {
        return Ok(());
    }
    let transaction = connection.transaction_with_behavior(TransactionBehavior::Immediate)?;
    let version: i64 = transaction.query_row("PRAGMA user_version", [], |row| row.get(0))?;
    if version == 6 {
        transaction.execute_batch(
            "ALTER TABLE sessions ADD COLUMN picker_debug INTEGER NOT NULL DEFAULT 0;
             ALTER TABLE sessions ADD COLUMN picker_has_history INTEGER NOT NULL DEFAULT 0;
             UPDATE sessions SET picker_debug=coalesce(json_extract(summary,'$.is_debug'),0),
               picker_has_history=coalesce(json_extract(summary,'$.visible_message_count'),0)>0;
             PRAGMA user_version=7;",
        )?;
    }
    transaction.commit()?;
    Ok(())
}

fn harden_database_files(path: &Path) -> Result<()> {
    for suffix in ["", "-wal", "-shm"] {
        let mut file = path.as_os_str().to_owned();
        file.push(suffix);
        let file = PathBuf::from(file);
        let metadata = match std::fs::metadata(&file) {
            Ok(metadata) => metadata,
            Err(error) if !suffix.is_empty() && error.kind() == std::io::ErrorKind::NotFound => {
                continue;
            }
            Err(error) => return Err(error.into()),
        };
        #[cfg(unix)]
        {
            use std::os::unix::fs::PermissionsExt;
            if metadata.permissions().mode() & 0o777 == 0o600 {
                continue;
            }
        }
        #[cfg(not(unix))]
        let _ = metadata;
        jcode_core::fs::set_permissions_owner_only(&file)?;
    }
    Ok(())
}

fn migrate_precise_ordering(connection: &mut Connection) -> Result<()> {
    let transaction = connection.transaction_with_behavior(TransactionBehavior::Immediate)?;
    let version: i64 = transaction.query_row("PRAGMA user_version", [], |row| row.get(0))?;
    if version == 1 {
        transaction.execute_batch(
            "ALTER TABLE sessions ADD COLUMN updated_seconds INTEGER NOT NULL DEFAULT 0;
             ALTER TABLE sessions ADD COLUMN updated_nanos INTEGER NOT NULL DEFAULT 0;",
        )?;
        let rows = {
            let mut statement = transaction.prepare("SELECT id,metadata FROM sessions")?;
            let rows = statement.query_map([], |row| {
                Ok((row.get::<_, String>(0)?, row.get::<_, String>(1)?))
            })?;
            rows.collect::<rusqlite::Result<Vec<_>>>()?
        };
        #[derive(serde::Deserialize)]
        struct Timestamp {
            updated_at: chrono::DateTime<chrono::Utc>,
        }
        for (id, metadata) in rows {
            let timestamp: Timestamp = serde_json::from_str(&metadata)?;
            transaction.execute(
                "UPDATE sessions SET updated_seconds=?2,updated_nanos=?3 WHERE id=?1",
                params![
                    id,
                    timestamp.updated_at.timestamp(),
                    timestamp.updated_at.timestamp_subsec_nanos()
                ],
            )?;
        }
        transaction.execute_batch(
            "CREATE INDEX sessions_updated_precise ON sessions(updated_seconds DESC,updated_nanos DESC,id);
             CREATE INDEX sessions_saved_precise ON sessions(saved,updated_seconds DESC,updated_nanos DESC,id);
             DROP INDEX IF EXISTS sessions_updated;
             DROP INDEX IF EXISTS sessions_saved;
             PRAGMA user_version=2;"
        )?;
    }
    transaction.commit()?;
    Ok(())
}

#[derive(Serialize)]
struct SessionMetadata<'a> {
    id: &'a str,
    created_at: chrono::DateTime<chrono::Utc>,
    messages: &'a [StoredMessage],
    #[serde(flatten)]
    meta: crate::session::journal::SessionJournalMeta,
}

fn metadata_json(session: &Session) -> Result<String> {
    let mut meta = session.journal_meta();
    meta.compaction = None;
    Ok(serde_json::to_string(&SessionMetadata {
        id: &session.id,
        created_at: session.created_at,
        messages: &[],
        meta,
    })?)
}

fn read_vector<T: DeserializeOwned>(
    transaction: &Transaction<'_>,
    table: &str,
    id: &str,
) -> Result<Vec<T>> {
    let mut statement = transaction.prepare_cached(&format!(
        "SELECT data FROM {table} WHERE session_id=?1 ORDER BY position"
    ))?;
    let rows = statement.query_map([id], |row| row.get::<_, String>(0))?;
    rows.map(|row| Ok(serde_json::from_str(&row?)?)).collect()
}

fn write_vector<T: Serialize>(
    transaction: &Transaction<'_>,
    table: &str,
    id: &str,
    items: &[T],
    previous_len: usize,
    mode: PersistVectorMode,
    initial: bool,
) -> Result<i64> {
    let full = initial || mode == PersistVectorMode::Full || items.len() < previous_len;
    let mut size_delta = 0i64;
    if full {
        size_delta -= transaction.query_row(&format!(
            "SELECT COALESCE(SUM(length(CAST(data AS BLOB))),0) FROM {table} WHERE session_id=?1"), [id], |row| row.get::<_, i64>(0))?;
        transaction.execute(&format!("DELETE FROM {table} WHERE session_id=?1"), [id])?;
    }
    let start = if full { 0 } else { previous_len };
    let mut insert = transaction.prepare_cached(&format!(
        "INSERT INTO {table}(session_id,position,data) VALUES(?1,?2,?3)"
    ))?;
    for (position, item) in items.iter().enumerate().skip(start) {
        let data = serde_json::to_string(item)?;
        size_delta += data.len() as i64;
        insert.execute(params![id, position as i64, data])?;
    }
    Ok(size_delta)
}

impl SessionStorage for SqliteSessionStorage {
    fn load(&self, id: &str) -> Result<Session> {
        self.read(id, false)
    }
    fn load_startup_stub(&self, id: &str) -> Result<Session> {
        self.read(id, true)
    }
    fn save(&self, session: &mut Session) -> Result<()> {
        self.write(session, false)
    }
    fn import(&self, session: &mut Session) -> Result<()> {
        self.write(session, true)
    }
    fn contains_native(&self, id: &str) -> Result<bool> {
        Ok(self
            .connection()?
            .query_row("SELECT 1 FROM sessions WHERE id=?1", [id], |_| Ok(()))
            .optional()?
            .is_some())
    }
    fn list(&self) -> Result<Vec<SessionSummary>> {
        self.list_summaries(&SessionListOptions::default())
    }
    fn list_summaries(&self, options: &SessionListOptions) -> Result<Vec<SessionSummary>> {
        let connection = self.connection()?;
        let mut query = String::from("SELECT summary FROM sessions WHERE ");
        let mut values: Vec<rusqlite::types::Value> = Vec::new();
        if let Some(since) = options.updated_since {
            query.push_str(if options.include_saved_beyond_window {
                "((updated_seconds,updated_nanos)>=(?,?) OR saved=1)"
            } else {
                "(updated_seconds,updated_nanos)>=(?,?)"
            });
            values.push(since.timestamp().into());
            values.push(i64::from(since.timestamp_subsec_nanos()).into());
        } else {
            query.push('1');
        }
        for prefix in &options.exclude_id_prefixes {
            query.push_str(" AND instr(id,?)!=1");
            values.push(prefix.clone().into());
        }
        query.push_str(" ORDER BY updated_seconds DESC,updated_nanos DESC,id LIMIT ?");
        values.push(i64::try_from(options.limit).unwrap_or(i64::MAX).into());
        let mut statement = connection.prepare_cached(&query)?;
        let rows = statement.query_map(rusqlite::params_from_iter(values), |row| {
            row.get::<_, String>(0)
        })?;
        rows.map(|row| Ok(serde_json::from_str(&row?)?)).collect()
    }
    fn list_discovered_summaries(
        &self,
        limit: usize,
        include_saved: bool,
    ) -> Result<Vec<SessionSummary>> {
        let connection = self.connection()?;
        let limit = i64::try_from(limit).unwrap_or(i64::MAX);
        let parse = |row: &rusqlite::Row<'_>| -> rusqlite::Result<(String, i64, i64, i64, String)> {
            Ok((
                row.get(0)?,
                row.get(1)?,
                row.get(2)?,
                row.get(3)?,
                row.get(4)?,
            ))
        };
        let mut statement = connection.prepare_cached(
            "SELECT id,discovery_seconds,discovery_nanos,sort_key,summary FROM sessions
             WHERE instr(id,'imported_')!=1
             ORDER BY discovery_seconds DESC,discovery_nanos DESC,sort_key DESC,id DESC LIMIT ?1",
        )?;
        let window = statement
            .query_map([limit], parse)?
            .collect::<rusqlite::Result<Vec<_>>>()?;
        let mut rows = window.iter().map(|row| row.4.clone()).collect::<Vec<_>>();
        if include_saved && i64::try_from(window.len()).is_ok_and(|len| len >= limit) {
            let mut saved = connection.prepare_cached(
                "SELECT summary FROM sessions
                 WHERE saved=1 AND instr(id,'imported_')!=1
                   AND (discovery_seconds,discovery_nanos,sort_key,id)<(?1,?2,?3,?4)
                 ORDER BY discovery_seconds DESC,discovery_nanos DESC,sort_key DESC,id DESC",
            )?;
            let (seconds, nanos, key, id) = match window.last() {
                Some((id, seconds, nanos, key, _)) => (*seconds, *nanos, *key, id.as_str()),
                None => (i64::MAX, i64::MAX, i64::MAX, "\u{10FFFF}"),
            };
            for row in saved.query_map(params![seconds, nanos, key, id], |row| {
                row.get::<_, String>(0)
            })? {
                rows.push(row?);
            }
        }
        rows.iter()
            .map(|row| Ok(serde_json::from_str(row)?))
            .collect()
    }
    fn list_picker_summaries(
        &self,
        visible_limit: usize,
        candidate_limit: usize,
        include_saved: bool,
    ) -> Result<Vec<SessionSummary>> {
        let connection = self.connection()?;
        let mut statement = connection.prepare_cached(
            "WITH recent AS MATERIALIZED (
               SELECT id,discovery_seconds,discovery_nanos,sort_key,saved,picker_debug,picker_has_history
               FROM sessions WHERE instr(id,'imported_')!=1
               ORDER BY discovery_seconds DESC,discovery_nanos DESC,sort_key DESC,id DESC LIMIT ?2
             ), pool AS (
               SELECT * FROM recent UNION ALL
               SELECT id,discovery_seconds,discovery_nanos,sort_key,saved,picker_debug,picker_has_history
               FROM sessions WHERE ?3 AND saved=1 AND instr(id,'imported_')!=1
                 AND id NOT IN (SELECT id FROM recent)
             ), ranked AS (
               SELECT *,sum(1-picker_debug) OVER ordering AS visible_rank,
                 sum(picker_debug) OVER ordering AS debug_rank
               FROM pool WHERE picker_has_history=1
               WINDOW ordering AS (ORDER BY discovery_seconds DESC,discovery_nanos DESC,sort_key DESC,id DESC ROWS UNBOUNDED PRECEDING)
             )
             SELECT s.summary FROM ranked r JOIN sessions s ON s.id=r.id
             WHERE ((visible_rank<?1 OR (visible_rank=?1 AND r.picker_debug=0))
                    AND (r.picker_debug=0 OR debug_rank<=?1))
                OR (r.saved=1 AND visible_rank>=?1)
             ORDER BY r.discovery_seconds DESC,r.discovery_nanos DESC,r.sort_key DESC,r.id DESC",
        )?;
        let rows = statement.query_map(
            params![
                i64::try_from(visible_limit).unwrap_or(i64::MAX),
                i64::try_from(candidate_limit).unwrap_or(i64::MAX),
                include_saved
            ],
            |row| row.get::<_, String>(0),
        )?;
        rows.map(|row| Ok(serde_json::from_str(&row?)?)).collect()
    }
    fn list_search_candidates(&self, max: usize) -> Result<Vec<SessionSearchCandidate>> {
        let connection = self.connection()?;
        let mut statement = connection.prepare_cached("SELECT id,updated_ms,revision,size_bytes FROM sessions ORDER BY updated_seconds DESC,updated_nanos DESC,id LIMIT ?1")?;
        let rows = statement.query_map([i64::try_from(max).unwrap_or(i64::MAX)], |row| {
            Ok(SessionSearchCandidate {
                id: row.get(0)?,
                updated_at_ms: row.get(1)?,
                version: row.get::<_, i64>(2)? as u64,
                size_bytes: row.get::<_, i64>(3)? as u64,
            })
        })?;
        Ok(rows.collect::<rusqlite::Result<_>>()?)
    }
    fn indexed_search_candidates(&self, query: &str) -> Result<Option<Vec<String>>> {
        let profile = jcode_session_types::SessionSearchQueryProfile::new(query);
        if !profile.is_actionable()
            || profile.terms.len() > 64
            || profile
                .terms
                .iter()
                .any(|term| !term.is_ascii() || term.len() < 3)
        {
            return Ok(None);
        }
        let connection = self.connection()?;
        Ok(Some(super::sqlite_fts::candidates(&connection, query)?))
    }
    fn search_text(&self, id: &str) -> Result<String> {
        self.import_legacy(id)?;
        let mut connection = self.connection()?;
        let transaction = connection.transaction()?;
        let mut text: String =
            transaction.query_row("SELECT metadata FROM sessions WHERE id=?1", [id], |row| {
                row.get(0)
            })?;
        for table in [
            "session_messages",
            "session_replay",
            "session_environment",
            "session_memory",
        ] {
            let mut statement = transaction.prepare_cached(&format!(
                "SELECT data FROM {table} WHERE session_id=?1 ORDER BY position"
            ))?;
            let rows = statement.query_map([id], |row| row.get::<_, String>(0))?;
            for row in rows {
                text.push('\n');
                text.push_str(&row?);
            }
        }
        let compaction: Option<String> = transaction
            .query_row(
                "SELECT data FROM session_compaction WHERE session_id=?1",
                [id],
                |row| row.get(0),
            )
            .optional()?;
        if let Some(compaction) = compaction {
            text.push('\n');
            text.push_str(&compaction);
        }
        transaction.commit()?;
        Ok(text)
    }
}
