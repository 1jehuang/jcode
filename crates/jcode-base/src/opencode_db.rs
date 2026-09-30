//! Read-only reader for OpenCode's SQLite session store.
//!
//! OpenCode 1.17+ keeps sessions in `~/.local/share/opencode/opencode.db`
//! (tables `session`, `message`, `part`) instead of the legacy JSON files under
//! `storage/`. Every OpenCode reader in jcode (resume picker, import, session
//! search, onboarding detection) goes through this module so the SQL lives in
//! one place. The database is always opened read-only: it belongs to OpenCode
//! and may be in use (WAL mode) while we read it.

use anyhow::Result;
use chrono::{DateTime, Utc};
use rusqlite::{Connection, OpenFlags, params};
use std::path::{Path, PathBuf};
use std::time::Duration;

pub const DB_FILE_NAME: &str = "opencode.db";

#[derive(Debug, Clone, PartialEq)]
pub struct OpenCodeDbSession {
    pub id: String,
    pub title: Option<String>,
    pub directory: Option<String>,
    pub provider_id: Option<String>,
    pub model_id: Option<String>,
    pub created_at: DateTime<Utc>,
    pub updated_at: DateTime<Utc>,
    pub message_count: usize,
}

#[derive(Debug, Clone, PartialEq)]
pub struct OpenCodeDbMessage {
    pub id: String,
    /// "user" or "assistant".
    pub role: String,
    /// Concatenated non-synthetic `text` parts. May be empty (tool-only turns).
    pub text: String,
    pub created_at: Option<DateTime<Utc>>,
    pub provider_id: Option<String>,
    pub model_id: Option<String>,
}

/// Location of OpenCode's database. Honors `JCODE_HOME` sandboxing through
/// `user_home_path`, and `XDG_DATA_HOME` (OpenCode uses xdg-basedir) when not
/// sandboxed.
pub fn db_path() -> Option<PathBuf> {
    if std::env::var_os("JCODE_HOME").is_none()
        && let Some(xdg) = std::env::var_os("XDG_DATA_HOME")
        && Path::new(&xdg).is_absolute()
    {
        return Some(PathBuf::from(xdg).join("opencode").join(DB_FILE_NAME));
    }
    crate::storage::user_home_path(format!(".local/share/opencode/{DB_FILE_NAME}")).ok()
}

/// The database path, only if the file exists.
pub fn existing_db_path() -> Option<PathBuf> {
    db_path().filter(|path| path.is_file())
}

/// True when a resume target path points at an OpenCode database rather than
/// a legacy JSON session file.
pub fn is_db_path(path: &Path) -> bool {
    path.file_name().and_then(|name| name.to_str()) == Some(DB_FILE_NAME)
}

/// Open read-only. A WAL database whose `-shm` file is missing (OpenCode not
/// running) cannot be opened by a plain read-only connection because it would
/// have to create the `-shm`. In that case fall back to `immutable=1`, which is
/// safe exactly because no writer is live.
pub fn open(path: &Path) -> Result<Connection> {
    let flags = OpenFlags::SQLITE_OPEN_READ_ONLY | OpenFlags::SQLITE_OPEN_NO_MUTEX;
    if is_wal_without_shm(path) {
        let db = open_immutable(path, flags)?;
        db.busy_timeout(Duration::from_millis(500))?;
        return Ok(db);
    }
    // Opening is lazy; preparing a statement forces the first file access,
    // which is where a missing `-shm` surfaces as SQLITE_CANTOPEN.
    let db = match Connection::open_with_flags(path, flags).and_then(|db| {
        db.prepare("SELECT 1 FROM session LIMIT 0")?;
        Ok(db)
    }) {
        Ok(db) => db,
        Err(err) if is_cantopen(&err) && path.is_file() => open_immutable(path, flags)?,
        Err(err) => return Err(err.into()),
    };
    db.busy_timeout(Duration::from_millis(500))?;
    Ok(db)
}

fn open_immutable(path: &Path, flags: OpenFlags) -> rusqlite::Result<Connection> {
    let uri = format!("file:{}?mode=ro&immutable=1", uri_escape(path));
    Connection::open_with_flags(uri, flags | OpenFlags::SQLITE_OPEN_URI)
}

/// A WAL-mode database (header bytes 18/19 == 2) whose `-shm` is absent has no
/// live writer. A plain read-only open would create the `-shm` in OpenCode's
/// directory (or fail if it cannot), so read it as immutable instead.
fn is_wal_without_shm(path: &Path) -> bool {
    use std::io::Read;
    let mut shm = path.as_os_str().to_owned();
    shm.push("-shm");
    if Path::new(&shm).exists() {
        return false;
    }
    let mut header = [0u8; 20];
    std::fs::File::open(path)
        .and_then(|mut f| f.read_exact(&mut header))
        .is_ok()
        && header[18] == 2
        && header[19] == 2
}

fn is_cantopen(err: &rusqlite::Error) -> bool {
    matches!(
        err,
        rusqlite::Error::SqliteFailure(e, _) if e.code == rusqlite::ErrorCode::CannotOpen
    )
}

fn uri_escape(path: &Path) -> String {
    let mut out = String::new();
    for ch in path.to_string_lossy().chars() {
        match ch {
            '%' => out.push_str("%25"),
            '?' => out.push_str("%3f"),
            '#' => out.push_str("%23"),
            _ => out.push(ch),
        }
    }
    out
}

fn ms(value: Option<i64>) -> Option<DateTime<Utc>> {
    value.and_then(DateTime::<Utc>::from_timestamp_millis)
}

const SESSION_COLUMNS: &str = "s.id, s.title, s.directory, \
     json_extract(s.model, '$.providerID'), json_extract(s.model, '$.id'), \
     s.time_created, s.time_updated, \
     (SELECT COUNT(*) FROM message m WHERE m.session_id = s.id)";

fn session_from_row(row: &rusqlite::Row<'_>) -> rusqlite::Result<OpenCodeDbSession> {
    let created_at = ms(row.get(5)?).unwrap_or_else(Utc::now);
    Ok(OpenCodeDbSession {
        id: row.get(0)?,
        title: row
            .get::<_, Option<String>>(1)?
            .filter(|s| !s.trim().is_empty()),
        directory: row.get::<_, Option<String>>(2)?.filter(|s| !s.is_empty()),
        provider_id: row.get::<_, Option<String>>(3)?.filter(|s| !s.is_empty()),
        model_id: row.get::<_, Option<String>>(4)?.filter(|s| !s.is_empty()),
        created_at,
        updated_at: ms(row.get(6)?).unwrap_or(created_at),
        message_count: row.get::<_, i64>(7)?.max(0) as usize,
    })
}

/// Most recently updated top-level (non-subagent), non-archived sessions.
pub fn list_sessions(path: &Path, limit: usize) -> Result<Vec<OpenCodeDbSession>> {
    let db = open(path)?;
    let sql = format!(
        "SELECT {SESSION_COLUMNS} FROM session s \
         WHERE s.parent_id IS NULL AND s.time_archived IS NULL \
         ORDER BY s.time_updated DESC LIMIT ?1"
    );
    let mut stmt = db.prepare(&sql)?;
    let rows = stmt
        .query_map(
            params![limit.min(i64::MAX as usize) as i64],
            session_from_row,
        )?
        .collect::<rusqlite::Result<Vec<_>>>()?;
    Ok(rows)
}

pub fn load_session(path: &Path, session_id: &str) -> Result<Option<OpenCodeDbSession>> {
    let db = open(path)?;
    let sql = format!("SELECT {SESSION_COLUMNS} FROM session s WHERE s.id = ?1");
    let mut stmt = db.prepare(&sql)?;
    let mut rows = stmt.query_map(params![session_id], session_from_row)?;
    Ok(rows.next().transpose()?)
}

/// User/assistant messages of a session in chronological order, with the text
/// of their non-synthetic `text` parts. `limit` keeps only the most recent
/// messages (for cheap previews), still returned oldest-first.
pub fn load_messages(
    path: &Path,
    session_id: &str,
    limit: Option<usize>,
) -> Result<Vec<OpenCodeDbMessage>> {
    load_messages_with(path, session_id, limit, false)
}

/// Like [`load_messages`] without a limit, but for session search: with
/// `include_tools` the text also carries `reasoning` parts and `tool` part
/// input and output, exactly like the legacy JSON store's search text.
pub fn load_search_messages(
    path: &Path,
    session_id: &str,
    include_tools: bool,
) -> Result<Vec<OpenCodeDbMessage>> {
    load_messages_with(path, session_id, None, include_tools)
}

fn load_messages_with(
    path: &Path,
    session_id: &str,
    limit: Option<usize>,
    include_tools: bool,
) -> Result<Vec<OpenCodeDbMessage>> {
    let db = open(path)?;
    let limit = limit.map(|n| n.min(i64::MAX as usize) as i64).unwrap_or(-1);
    let mut stmt = db.prepare(
        "SELECT m.id, m.time_created, \
                json_extract(m.data, '$.role'), \
                json_extract(m.data, '$.providerID'), \
                json_extract(m.data, '$.modelID'), \
                json_extract(p.data, '$.type'), \
                CASE WHEN json_extract(p.data, '$.type') = 'text' \
                     THEN json_extract(p.data, '$.text') ELSE p.data END \
         FROM (SELECT id, time_created, data FROM message \
               WHERE session_id = ?1 \
                 AND json_extract(data, '$.role') IN ('user', 'assistant') \
               ORDER BY time_created DESC, id DESC LIMIT ?2) m \
         LEFT JOIN part p ON p.message_id = m.id \
              AND (json_extract(p.data, '$.type') = 'text' \
                   OR (?3 AND json_extract(p.data, '$.type') IN ('reasoning', 'tool'))) \
              AND COALESCE(json_extract(p.data, '$.synthetic'), 0) = 0 \
         ORDER BY m.time_created, m.id, p.id",
    )?;
    let mut rows = stmt.query(params![session_id, limit, include_tools])?;
    let mut messages: Vec<OpenCodeDbMessage> = Vec::new();
    while let Some(row) = rows.next()? {
        let id: String = row.get(0)?;
        let part_type: Option<String> = row.get(5)?;
        let payload: Option<String> = row.get(6)?;
        if messages.last().map(|m| m.id.as_str()) != Some(id.as_str()) {
            messages.push(OpenCodeDbMessage {
                id,
                role: row.get(2)?,
                text: String::new(),
                created_at: ms(row.get(1)?),
                provider_id: row.get(3)?,
                model_id: row.get(4)?,
            });
        }
        let texts = match (part_type.as_deref(), payload) {
            (Some("text"), Some(text)) => vec![text],
            (Some(_), Some(json)) => search_part_texts(&json),
            _ => Vec::new(),
        };
        let Some(message) = messages.last_mut() else {
            continue;
        };
        for text in texts {
            let text = if include_tools { text.trim() } else { text.as_str() };
            if text.trim().is_empty() {
                continue;
            }
            if !message.text.is_empty() {
                message.text.push('\n');
            }
            message.text.push_str(text);
        }
    }
    Ok(messages)
}

/// Searchable text of a `reasoning` or `tool` part, mirroring
/// `jcode_import_core::extract_opencode_part_text` for the legacy store.
fn search_part_texts(json: &str) -> Vec<String> {
    let Ok(part) = serde_json::from_str::<serde_json::Value>(json) else {
        return Vec::new();
    };
    let mut out = Vec::new();
    match part.get("type").and_then(|v| v.as_str()) {
        Some("reasoning") => {
            if let Some(text) = part.get("text").and_then(|v| v.as_str()) {
                out.push(text.to_string());
            }
        }
        Some("tool") => {
            if let Some(state) = part.get("state") {
                if let Some(input) = state.get("input") {
                    out.push(jcode_import_core::extract_external_text_from_json(
                        input, true,
                    ));
                }
                if let Some(output) = state.get("output").and_then(|v| v.as_str()) {
                    out.push(output.to_string());
                }
            }
        }
        _ => {}
    }
    out
}

/// Ids among `session_ids` whose parts could match a search, so only those
/// histories get loaded. A session qualifies when at least `min_term_matches`
/// of `terms` occur somewhere in its searchable parts (a superset of the
/// per-message matcher, which needs the same terms inside one message).
/// Matching is ASCII case-insensitive in SQLite, so a term with non-ASCII
/// characters is treated as present to never drop a real match.
pub fn sessions_matching_terms(
    path: &Path,
    session_ids: &[String],
    terms: &[String],
    min_term_matches: usize,
    include_tools: bool,
) -> Result<std::collections::HashSet<String>> {
    let mut matched = std::collections::HashSet::new();
    if session_ids.is_empty() {
        return Ok(matched);
    }
    let assumed = terms.iter().filter(|t| !t.is_ascii()).count();
    let checked: Vec<String> = terms
        .iter()
        .filter(|t| t.is_ascii())
        .map(|t| t.to_ascii_lowercase())
        .collect();
    let needed = min_term_matches.saturating_sub(assumed);
    if needed == 0 {
        matched.extend(session_ids.iter().cloned());
        return Ok(matched);
    }
    if checked.is_empty() {
        return Ok(matched);
    }
    let field = if include_tools {
        "p.data"
    } else {
        "json_extract(p.data, '$.text')"
    };
    let types = if include_tools {
        "('text', 'reasoning', 'tool')"
    } else {
        "('text')"
    };
    let hits = (0..checked.len())
        .map(|i| format!("MAX(instr(lower({field}), ?{}) > 0)", i + 1))
        .collect::<Vec<_>>()
        .join(" + ");
    let offset = checked.len();
    let ids = (0..session_ids.len())
        .map(|i| format!("?{}", offset + i + 1))
        .collect::<Vec<_>>()
        .join(", ");
    let sql = format!(
        "SELECT p.session_id, {hits} FROM part p \
         WHERE p.session_id IN ({ids}) AND json_extract(p.data, '$.type') IN {types} \
         GROUP BY p.session_id"
    );
    let db = open(path)?;
    let mut stmt = db.prepare(&sql)?;
    let values: Vec<&dyn rusqlite::ToSql> = checked
        .iter()
        .map(|t| t as &dyn rusqlite::ToSql)
        .chain(session_ids.iter().map(|id| id as &dyn rusqlite::ToSql))
        .collect();
    let mut rows = stmt.query(values.as_slice())?;
    while let Some(row) = rows.next()? {
        let hits: i64 = row.get(1)?;
        if hits.max(0) as usize >= needed {
            matched.insert(row.get(0)?);
        }
    }
    Ok(matched)
}

#[cfg(test)]
#[path = "opencode_db_tests.rs"]
mod tests;

/// Builders for OpenCode database fixtures in tests (real 1.18 column set).
#[cfg(any(test, feature = "test-support"))]
pub mod fixture {
    use rusqlite::{Connection, params};
    use std::path::Path;

    pub const SCHEMA: &str = "
CREATE TABLE `session` (
  `id` text PRIMARY KEY, `project_id` text NOT NULL, `workspace_id` text, `parent_id` text,
  `slug` text NOT NULL, `directory` text NOT NULL, `path` text, `title` text NOT NULL,
  `version` text NOT NULL, `share_url` text, `summary_additions` integer, `summary_deletions` integer,
  `summary_files` integer, `summary_diffs` text, `metadata` text, `cost` real DEFAULT 0 NOT NULL,
  `tokens_input` integer DEFAULT 0 NOT NULL, `tokens_output` integer DEFAULT 0 NOT NULL,
  `tokens_reasoning` integer DEFAULT 0 NOT NULL, `tokens_cache_read` integer DEFAULT 0 NOT NULL,
  `tokens_cache_write` integer DEFAULT 0 NOT NULL, `revert` text, `permission` text, `agent` text,
  `model` text, `time_created` integer NOT NULL, `time_updated` integer NOT NULL,
  `time_compacting` integer, `time_archived` integer);
CREATE INDEX `session_parent_idx` ON `session` (`parent_id`);
CREATE TABLE `message` (`id` text PRIMARY KEY, `session_id` text NOT NULL, `time_created` integer NOT NULL,
  `time_updated` integer NOT NULL, `data` text NOT NULL);
CREATE INDEX `message_session_time_created_id_idx` ON `message` (`session_id`,`time_created`,`id`);
CREATE TABLE `part` (`id` text PRIMARY KEY, `message_id` text NOT NULL, `session_id` text NOT NULL,
  `time_created` integer NOT NULL, `time_updated` integer NOT NULL, `data` text NOT NULL);
CREATE INDEX `part_message_id_id_idx` ON `part` (`message_id`,`id`);
CREATE INDEX `part_session_idx` ON `part` (`session_id`);
";

    pub struct Fixture(pub Connection);

    impl Fixture {
        pub fn create(path: &Path) -> Self {
            std::fs::create_dir_all(path.parent().expect("parent")).expect("mkdir");
            let db = Connection::open(path).expect("create fixture db");
            db.execute_batch(SCHEMA).expect("schema");
            Self(db)
        }

        #[allow(clippy::too_many_arguments)]
        pub fn session(
            &self,
            id: &str,
            parent: Option<&str>,
            title: &str,
            dir: &str,
            provider: &str,
            model: &str,
            updated: i64,
            archived: Option<i64>,
        ) -> &Self {
            let model_json = serde_json::json!({"id": model, "providerID": provider}).to_string();
            self.0
                .execute(
                    "INSERT INTO session (id, project_id, parent_id, slug, directory, title, version, model, time_created, time_updated, time_archived) \
                     VALUES (?1, 'proj', ?2, 'slug', ?3, ?4, '1.18.33', ?5, ?6, ?7, ?8)",
                    params![id, parent, dir, title, model_json, updated - 1000, updated, archived],
                )
                .expect("insert session");
            self
        }

        pub fn message(&self, session: &str, id: &str, role: &str, time: i64) -> &Self {
            let data = serde_json::json!({"role": role, "modelID": "m", "providerID": "p", "time": {"created": time}}).to_string();
            self.0
                .execute(
                    "INSERT INTO message (id, session_id, time_created, time_updated, data) VALUES (?1, ?2, ?3, ?3, ?4)",
                    params![id, session, time, data],
                )
                .expect("insert message");
            self
        }

        pub fn part(
            &self,
            session: &str,
            message: &str,
            id: &str,
            data: serde_json::Value,
        ) -> &Self {
            self.0
                .execute(
                    "INSERT INTO part (id, message_id, session_id, time_created, time_updated, data) VALUES (?1, ?2, ?3, 0, 0, ?4)",
                    params![id, message, session, data.to_string()],
                )
                .expect("insert part");
            self
        }

        pub fn text(&self, session: &str, message: &str, id: &str, text: &str) -> &Self {
            self.part(
                session,
                message,
                id,
                serde_json::json!({"type": "text", "text": text}),
            )
        }
    }

    /// Standard fixture: one top-level session with a user turn and an
    /// assistant turn (plus reasoning/tool/synthetic noise), one subagent child,
    /// one archived session, and an older second top-level session.
    pub fn standard(path: &Path) -> Fixture {
        let f = Fixture::create(path);
        f.session(
            "ses_main",
            None,
            "Main task",
            "/tmp/oc-main",
            "anthropic",
            "claude-x",
            3_000_000,
            None,
        )
        .session(
            "ses_child",
            Some("ses_main"),
            "Subagent",
            "/tmp/oc-main",
            "anthropic",
            "claude-x",
            4_000_000,
            None,
        )
        .session(
            "ses_arch",
            None,
            "Archived",
            "/tmp/oc-arch",
            "openai",
            "gpt",
            5_000_000,
            Some(5_000_001),
        )
        .session(
            "ses_old",
            None,
            "Older task",
            "/tmp/oc-old",
            "openai",
            "gpt-5",
            2_000_000,
            None,
        )
        .message("ses_main", "msg_a", "user", 100)
        .message("ses_main", "msg_b", "assistant", 200)
        .text("ses_main", "msg_a", "prt_a1", "hello opencode")
        .part(
            "ses_main",
            "msg_a",
            "prt_a2",
            serde_json::json!({"type": "text", "text": "SYNTHETIC", "synthetic": true}),
        )
        .part(
            "ses_main",
            "msg_b",
            "prt_b0",
            serde_json::json!({"type": "step-start"}),
        )
        .part(
            "ses_main",
            "msg_b",
            "prt_b1",
            serde_json::json!({"type": "reasoning", "text": "THINKING"}),
        )
        .part(
            "ses_main",
            "msg_b",
            "prt_b2",
            serde_json::json!({"type": "tool", "tool": "bash"}),
        )
        .text("ses_main", "msg_b", "prt_b3", "hi from assistant")
        .message("ses_old", "msg_o", "user", 50)
        .text("ses_old", "msg_o", "prt_o", "old prompt");
        f
    }
}
