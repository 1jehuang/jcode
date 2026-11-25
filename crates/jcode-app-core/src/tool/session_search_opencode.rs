//! OpenCode SQLite and legacy stores share one ranked search allowance.
use super::*;

/// Where an OpenCode scan candidate lives.
enum OpenCodeCandidate {
    Db(crate::opencode_db::OpenCodeDbSession),
    Legacy(PathBuf),
}

#[cfg(test)]
thread_local! {
    /// Number of OpenCode database histories loaded by session search.
    pub(super) static OPENCODE_DB_LOADS: std::cell::Cell<usize> = const { std::cell::Cell::new(0) };
}

pub(super) fn collect_opencode_external_sessions(
    records: &mut Vec<ExternalSessionRecord>,
    report: &mut SearchReport,
    query: &QueryProfile,
    options: &SearchOptions,
) {
    if !source_matches_filter("opencode", options) {
        return;
    }
    // OpenCode 1.17+ keeps sessions in SQLite; older installs use JSON files.
    // Both stores share ONE scan allowance: the most recent sessions across
    // them, deduped by id with database rows winning.
    let mut candidates: Vec<(DateTime<Utc>, String, OpenCodeCandidate)> = Vec::new();
    let db = crate::opencode_db::existing_db_path();
    let mut found_store = false;
    let mut database_listed = false;
    if let Some(db) = db.as_deref() {
        found_store = true;
        match crate::opencode_db::list_sessions(db, options.max_scan_sessions.saturating_add(1)) {
            Ok(rows) => {
                database_listed = true;
                candidates.extend(
                    rows.into_iter()
                        .map(|row| (row.updated_at, row.id.clone(), OpenCodeCandidate::Db(row))),
                );
            }
            Err(_) => report.parse_errors += 1,
        }
    }
    let legacy_root = crate::storage::user_home_path(".local/share/opencode/storage/session");
    if let Ok(root) = legacy_root.as_deref()
        && root.exists()
    {
        found_store = true;
        let db_ids: std::collections::HashSet<String> =
            candidates.iter().map(|(_, id, _)| id.clone()).collect();
        // Exact stored-time ranking requires inspecting every legacy metadata file:
        // an unread file can contain the newest timestamp or a duplicate ID. The
        // scan cap bounds selected session histories, not this cold metadata pass.
        // A bounded metadata pass would require an authoritative index or migration,
        // not an mtime window (copied/restored files can have unrelated mtimes).
        let mut legacy_by_id: HashMap<String, (DateTime<Utc>, PathBuf)> = HashMap::new();
        for path in jcode_import_core::collect_files_recursive(root, "json") {
            let Ok(value) = std::fs::File::open(&path)
                .map_err(anyhow::Error::from)
                .and_then(|file| serde_json::from_reader::<_, Value>(file).map_err(Into::into))
            else {
                report.parse_errors += 1;
                continue;
            };
            let Some(id) = value
                .get("id")
                .and_then(Value::as_str)
                .filter(|id| !id.is_empty())
            else {
                continue;
            };
            if db_ids.contains(id) {
                continue;
            }
            let updated = value
                .get("time")
                .and_then(|time| time.get("updated"))
                .and_then(Value::as_i64)
                .and_then(DateTime::<Utc>::from_timestamp_millis)
                .or_else(|| jcode_import_core::file_modified_datetime(&path))
                .unwrap_or(DateTime::<Utc>::UNIX_EPOCH);
            // Copies of one session consume one slot. Keep its newest stored
            // update, with the lexically first path breaking equal-time ties.
            let entry = legacy_by_id
                .entry(id.to_string())
                .or_insert_with(|| (updated, path.clone()));
            if updated > entry.0 || (updated == entry.0 && path < entry.1) {
                *entry = (updated, path);
            }
        }
        if database_listed && let Some(db) = db.as_deref() {
            let ids: Vec<_> = legacy_by_id.keys().cloned().collect();
            for chunk in ids.chunks(500) {
                match crate::opencode_db::session_ids_present(db, chunk) {
                    Ok(present) => {
                        for id in present {
                            legacy_by_id.remove(&id);
                        }
                    }
                    Err(_) => report.parse_errors += 1,
                }
            }
        }
        candidates.extend(
            legacy_by_id
                .into_iter()
                .map(|(id, (updated, path))| (updated, id, OpenCodeCandidate::Legacy(path))),
        );
    }
    if found_store {
        report.external_sources.push("opencode");
    }
    candidates.sort_by(|a, b| b.0.cmp(&a.0).then_with(|| a.1.cmp(&b.1)));
    if candidates.len() > options.max_scan_sessions {
        candidates.truncate(options.max_scan_sessions);
        report.truncated = true;
    }

    // Only load database histories whose parts can match the query.
    let db_ids: Vec<String> = candidates
        .iter()
        .filter(|(_, _, c)| matches!(c, OpenCodeCandidate::Db(_)))
        .map(|(_, id, _)| id.clone())
        .collect();
    let db_matches = match db.as_deref() {
        Some(db) if !db_ids.is_empty() => {
            let mut matched = std::collections::HashSet::new();
            let mut failed = false;
            for chunk in db_ids.chunks(500) {
                match crate::opencode_db::sessions_matching_terms(
                    db,
                    chunk,
                    &query.terms,
                    query.min_term_matches,
                    options.include_tools,
                ) {
                    Ok(ids) => matched.extend(ids),
                    Err(_) => failed = true,
                }
            }
            if failed {
                report.parse_errors += 1;
            }
            matched
        }
        _ => std::collections::HashSet::new(),
    };

    let messages_base = crate::storage::user_home_path(".local/share/opencode/storage/message");
    let parts_base = crate::storage::user_home_path(".local/share/opencode/storage/part");
    for (_, id, candidate) in candidates {
        match candidate {
            OpenCodeCandidate::Db(row) => {
                let Some(db) = db.as_deref() else { continue };
                // Title/metadata matches still need a record, but no history.
                let load_history = db_matches.contains(&id);
                match load_opencode_db_external_session(
                    db,
                    row,
                    load_history,
                    options.include_tools,
                ) {
                    Ok(record) => records.push(record),
                    Err(_) => report.parse_errors += 1,
                }
            }
            OpenCodeCandidate::Legacy(path) => {
                let (Ok(messages_base), Ok(parts_base)) = (&messages_base, &parts_base) else {
                    continue;
                };
                match load_opencode_external_session(
                    &path,
                    messages_base,
                    parts_base,
                    options.include_tools,
                    options.max_scan_sessions,
                ) {
                    Ok(Some(record)) => records.push(record),
                    Ok(None) => {}
                    Err(_) => report.parse_errors += 1,
                }
            }
        }
    }
}

fn load_opencode_db_external_session(
    db: &Path,
    row: crate::opencode_db::OpenCodeDbSession,
    load_history: bool,
    include_tools: bool,
) -> Result<ExternalSessionRecord> {
    let messages = if load_history {
        #[cfg(test)]
        OPENCODE_DB_LOADS.with(|loads| loads.set(loads.get() + 1));
        crate::opencode_db::load_search_messages(db, &row.id, include_tools)?
            .into_iter()
            .filter(|msg| !msg.text.trim().is_empty())
            .map(|msg| jcode_import_core::ExternalMessageRecord {
                role: msg.role,
                text: msg.text,
                timestamp: msg.created_at,
                id: Some(msg.id),
            })
            .collect()
    } else {
        Vec::new()
    };
    let short = jcode_core::util::truncate_str(&row.id, 8).to_string();
    Ok(ExternalSessionRecord {
        source: "opencode",
        short_name: Some(format!("opencode {short}")),
        title: Some(
            row.title
                .unwrap_or_else(|| format!("OpenCode session {short}")),
        ),
        working_dir: row.directory,
        provider_key: Some(row.provider_id.unwrap_or_else(|| "opencode".to_string())),
        model: row.model_id,
        created_at: row.created_at,
        updated_at: row.updated_at,
        path: db.to_path_buf(),
        messages,
        session_id: row.id,
    })
}
