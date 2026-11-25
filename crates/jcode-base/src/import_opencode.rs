//! OpenCode legacy JSON and SQLite import share the same history safeguards.
use super::*;

pub(super) fn find_opencode_session_file(session_id: &str) -> Result<PathBuf> {
    let root = crate::storage::user_home_path(".local/share/opencode/storage/session")?;
    for path in collect_files_recursive(&root, "json") {
        let Ok(value) = serde_json::from_reader::<_, serde_json::Value>(File::open(&path)?) else {
            continue;
        };
        if value.get("id").and_then(|v| v.as_str()) == Some(session_id) {
            return Ok(path);
        }
    }
    anyhow::bail!("OpenCode session {} not found", session_id)
}

pub fn import_opencode_session(session_id: &str) -> Result<Session> {
    let db_error = match import_existing_opencode_db_session(session_id) {
        Ok(Some(session)) => return Ok(session),
        Ok(None) => None,
        Err(error) => Some(error),
    };
    // A legacy session still imports when any SQLite history read fails.
    match find_opencode_session_file(session_id) {
        Ok(path) => import_opencode_session_from_path(&path, Some(session_id)),
        Err(not_found) => Err(db_error.unwrap_or(not_found)),
    }
}

/// Complete the SQLite read before deciding whether other stores are needed.
pub(super) fn import_existing_opencode_db_session(session_id: &str) -> Result<Option<Session>> {
    let Some(db) = crate::opencode_db::existing_db_path() else {
        return Ok(None);
    };
    match crate::opencode_db::load_session(&db, session_id)? {
        Some(_) => import_opencode_db_session(&db, session_id).map(Some),
        None => Ok(None),
    }
}

/// Import one session from OpenCode's SQLite store (OpenCode 1.17+).
pub fn import_opencode_db_session(db_path: &Path, session_id: &str) -> Result<Session> {
    let meta = crate::opencode_db::load_session(db_path, session_id)?.ok_or_else(|| {
        anyhow::anyhow!(
            "OpenCode session {} not found in {}",
            session_id,
            db_path.display()
        )
    })?;
    let mut session = Session::create_with_id(imported_opencode_session_id(session_id), None, None);
    session.provider_session_id = Some(session_id.to_string());
    session.working_dir = meta.directory.clone();
    session.title = Some(
        meta.title
            .as_deref()
            .map(truncate_title)
            .unwrap_or_else(|| format!("OpenCode session {}", session_id)),
    );
    let extracted = crate::opencode_db::load_import_messages(
        db_path,
        session_id,
        IMPORT_HISTORY_MAX_MESSAGES,
        IMPORT_HISTORY_MAX_TEXT_BYTES,
        IMPORT_BLOCK_MAX_TEXT_BYTES,
    )?;
    let mut provider_key = meta.provider_id.clone().or(extracted.provider_id);
    let mut model = meta.model_id.clone().or(extracted.model_id);
    for msg in extracted.messages {
        let role = if msg.role == "user" {
            Role::User
        } else {
            Role::Assistant
        };
        if provider_key.is_none() {
            provider_key = msg.provider_id.clone();
        }
        if model.is_none() {
            model = msg.model_id.clone();
        }
        if msg.text.trim().is_empty() {
            continue;
        }
        append_text_message(&mut session, role, msg.text, msg.created_at);
    }
    session.provider_key = provider_key.or_else(|| Some("opencode".to_string()));
    session.model = model;
    finalize_imported_session_with_omissions(
        session,
        meta.created_at,
        Some(meta.updated_at),
        extracted.omitted,
    )
}

pub fn import_opencode_session_from_path(
    session_path: &Path,
    session_id_hint: Option<&str>,
) -> Result<Session> {
    if crate::opencode_db::is_db_path(session_path) {
        let session_id = session_id_hint
            .ok_or_else(|| anyhow::anyhow!("OpenCode database import needs a session id"))?;
        return import_opencode_db_session(session_path, session_id);
    }
    let value: serde_json::Value = serde_json::from_reader(File::open(session_path)?)?;
    let session_id = value
        .get("id")
        .and_then(|v| v.as_str())
        .filter(|id| !id.is_empty())
        .or(session_id_hint)
        .ok_or_else(|| {
            anyhow::anyhow!("OpenCode session id missing in {}", session_path.display())
        })?;
    let created_at = value
        .get("time")
        .and_then(|time| time.get("created"))
        .and_then(|v| v.as_i64())
        .and_then(DateTime::<Utc>::from_timestamp_millis)
        .unwrap_or_else(Utc::now);
    let mut updated_at = value
        .get("time")
        .and_then(|time| time.get("updated"))
        .and_then(|v| v.as_i64())
        .and_then(DateTime::<Utc>::from_timestamp_millis)
        .or(Some(created_at));
    let mut session = Session::create_with_id(imported_opencode_session_id(session_id), None, None);
    session.provider_session_id = Some(session_id.to_string());
    session.provider_key = Some("opencode".to_string());
    session.working_dir = value
        .get("directory")
        .and_then(|v| v.as_str())
        .map(|s| s.to_string());
    session.title = value
        .get("title")
        .and_then(|v| v.as_str())
        .map(truncate_title);

    let messages_root = crate::storage::user_home_path(format!(
        ".local/share/opencode/storage/message/{}",
        session_id
    ))?;
    let parts_base = crate::storage::user_home_path(".local/share/opencode/storage/part")?;
    let mut messages: Vec<(Option<DateTime<Utc>>, Role, String)> = Vec::new();
    let mut model: Option<String> = None;
    let mut provider_key = session.provider_key.clone();

    if messages_root.exists() {
        for msg_path in collect_files_recursive(&messages_root, "json") {
            let Ok(msg_value) =
                serde_json::from_reader::<_, serde_json::Value>(File::open(&msg_path)?)
            else {
                continue;
            };
            let role = match msg_value.get("role").and_then(|v| v.as_str()) {
                Some("user") => Role::User,
                Some("assistant") => Role::Assistant,
                _ => continue,
            };
            // Modern OpenCode (Go storage) stores message body text in
            // storage/part/<messageID>/*.json; fall back to legacy inline
            // content/summary for older stores.
            let text = msg_value
                .get("id")
                .and_then(|v| v.as_str())
                .map(|id| extract_opencode_part_text(&parts_base, id, true))
                .filter(|text| !text.trim().is_empty())
                .or_else(|| {
                    msg_value
                        .get("content")
                        .map(extract_text_from_json_value)
                        .filter(|text| !text.trim().is_empty())
                })
                .or_else(|| msg_value.get("summary").map(extract_text_from_json_value))
                .unwrap_or_default();
            if model.is_none() {
                model = msg_value
                    .get("modelID")
                    .or_else(|| msg_value.get("model").and_then(|m| m.get("modelID")))
                    .and_then(|v| v.as_str())
                    .map(|s| s.to_string());
            }
            if provider_key.as_deref() == Some("opencode") {
                provider_key = msg_value
                    .get("providerID")
                    .or_else(|| msg_value.get("model").and_then(|m| m.get("providerID")))
                    .and_then(|v| v.as_str())
                    .map(|s| s.to_string())
                    .or(provider_key);
            }
            let timestamp = msg_value
                .get("time")
                .and_then(|time| time.get("created"))
                .and_then(|v| v.as_i64())
                .and_then(DateTime::<Utc>::from_timestamp_millis);
            if timestamp.is_some() {
                updated_at = timestamp;
            }
            messages.push((timestamp, role, text));
        }
    }

    messages.sort_by_key(|(timestamp, _, _)| *timestamp);
    for (timestamp, role, text) in messages {
        append_text_message(&mut session, role, text, timestamp);
    }

    if session.title.is_none() {
        session.title = Some(format!("OpenCode session {}", session_id));
    }
    session.provider_key = provider_key;
    session.model = model;
    finalize_imported_session(session, created_at, updated_at)
}
