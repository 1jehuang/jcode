#[cfg(test)]
use super::session_path;
use super::{
    Session, SessionBackend, SessionStatus, SessionStore, active_pids_dir, session_exists,
};
use crate::id::extract_session_name;
use crate::message::{ContentBlock, Role};
use crate::storage;
use anyhow::Result;
use chrono::{DateTime, Duration, Utc};
use serde::Deserialize;
use std::collections::HashSet;

const RELEVANT_CRASH_GROUP_WINDOW_SECS: i64 = 60;

fn stored_session_ids() -> Result<Vec<String>> {
    if SessionBackend::configured()? != SessionBackend::Files {
        return Ok(SessionStore::configured()?
            .list()?
            .into_iter()
            .map(|row| row.id)
            .collect());
    }
    let directory = storage::jcode_dir()?.join("sessions");
    if !directory.exists() {
        return Ok(Vec::new());
    }
    let mut ids = Vec::new();
    for entry in std::fs::read_dir(directory)? {
        let path = entry?.path();
        if path
            .extension()
            .is_some_and(|extension| extension == "json")
            && let Some(id) = path.file_stem().and_then(|id| id.to_str())
        {
            ids.push(id.to_owned());
        }
    }
    Ok(ids)
}

/// Recover crashed sessions from the most relevant crash group (text-only).
/// Returns new recovery session IDs (most recent first).
pub fn recover_crashed_sessions() -> Result<Vec<String>> {
    recover_crashed_sessions_matching(None)
}

/// Recover the specific crashed sessions chosen by the picker.
///
/// This is intentionally stricter than `recover_crashed_sessions`: the picker has
/// already guessed the relevant crash group, so this avoids restoring stale
/// crashed sessions that happen to still exist on disk.
pub fn recover_crashed_sessions_by_ids(session_ids: &[String]) -> Result<Vec<String>> {
    if session_ids.is_empty() {
        return Ok(Vec::new());
    }
    let allowed: HashSet<String> = session_ids.iter().cloned().collect();
    recover_crashed_sessions_for_allowed_ids(&allowed)
}

fn recover_crashed_sessions_for_allowed_ids(allowed_ids: &HashSet<String>) -> Result<Vec<String>> {
    let mut recovered_parents: HashSet<String> = HashSet::new();
    for id in stored_session_ids()? {
        if id.starts_with("session_recovery_")
            && let Ok(session) = Session::load_startup_stub(&id)
            && let Some(parent) = session.parent_id
        {
            recovered_parents.insert(parent);
        }
    }

    let mut crashed = Vec::with_capacity(allowed_ids.len());
    for session_id in allowed_ids {
        if recovered_parents.contains(session_id) || !session_exists(session_id) {
            continue;
        }
        let mut session = Session::load(session_id)?;
        if session.detect_crash() {
            let _ = session.save();
        }
        if matches!(session.status, SessionStatus::Crashed { .. }) {
            crashed.push(session);
        }
    }

    recover_loaded_crashed_sessions(crashed)
}

fn recover_crashed_sessions_matching(allowed_ids: Option<&HashSet<String>>) -> Result<Vec<String>> {
    let mut sessions: Vec<Session> = Vec::new();
    for id in stored_session_ids()? {
        if let Ok(mut session) = Session::load(&id) {
            if session.detect_crash() {
                let _ = session.save();
            }
            sessions.push(session);
        }
    }

    // Track existing recovery sessions to avoid duplicates
    let mut recovered_parents: HashSet<String> = HashSet::new();
    for s in &sessions {
        if s.id.starts_with("session_recovery_")
            && let Some(parent) = s.parent_id.as_ref()
        {
            recovered_parents.insert(parent.clone());
        }
    }

    let mut crashed: Vec<Session> = sessions
        .into_iter()
        .filter(|s| matches!(s.status, SessionStatus::Crashed { .. }))
        .filter(|s| !recovered_parents.contains(&s.id))
        .collect();

    if let Some(allowed_ids) = allowed_ids {
        crashed.retain(|s| allowed_ids.contains(&s.id));
    }

    if crashed.is_empty() {
        return Ok(Vec::new());
    }

    if allowed_ids.is_none() {
        retain_relevant_crash_group(&mut crashed);
    }

    recover_loaded_crashed_sessions(crashed)
}

fn recover_loaded_crashed_sessions(mut crashed: Vec<Session>) -> Result<Vec<String>> {
    crashed.sort_by_key(|session| std::cmp::Reverse(crash_timestamp(session)));

    let mut new_ids = Vec::new();
    for mut old in crashed {
        let new_id = format!("session_recovery_{}", crate::id::new_id("rec"));
        let mut new_session =
            Session::create_with_id(new_id.clone(), Some(old.id.clone()), old.title.clone());
        new_session.custom_title = old.custom_title.clone();
        new_session.working_dir = old.working_dir.clone();
        new_session.provider_key = old.provider_key.clone();
        new_session.route_api_method = old.route_api_method.clone();
        new_session.model = old.model.clone();
        new_session.improve_mode = old.improve_mode;
        new_session.is_canary = old.is_canary;
        new_session.is_debug = old.is_debug;
        new_session.testing_build = old.testing_build.clone();
        new_session.saved = old.saved;
        new_session.save_label = old.save_label.clone();
        new_session.provider_session_id = None;
        new_session.status = SessionStatus::Closed;

        // Add a recovery header
        new_session.add_message(
            Role::User,
            vec![ContentBlock::Text {
                text: format!(
                    "Recovered from crashed session `{}` ({})",
                    old.id,
                    old.display_name()
                ),
                cache_control: None,
            }],
        );

        for msg in old.messages.drain(..) {
            let kept_blocks: Vec<ContentBlock> = msg
                .content
                .into_iter()
                .filter(|block| matches!(block, ContentBlock::Text { .. }))
                .collect();
            if kept_blocks.is_empty() {
                continue;
            }
            new_session.add_message(msg.role, kept_blocks);
        }

        new_session.save()?;
        new_ids.push(new_id);
    }

    Ok(new_ids)
}

fn crash_timestamp(session: &Session) -> DateTime<Utc> {
    session.last_active_at.unwrap_or(session.updated_at)
}

fn retain_relevant_crash_group(crashed: &mut Vec<Session>) -> Option<DateTime<Utc>> {
    let most_recent = crashed.iter().map(crash_timestamp).max()?;
    let crash_window = Duration::seconds(RELEVANT_CRASH_GROUP_WINDOW_SECS);
    crashed.retain(|s| {
        let delta = most_recent.signed_duration_since(crash_timestamp(s));
        delta >= Duration::zero() && delta <= crash_window
    });
    Some(most_recent)
}

/// Info about crashed sessions pending batch restore
#[derive(Debug, Clone)]
pub struct CrashedSessionsInfo {
    /// Session IDs in the guessed relevant restore group.
    pub session_ids: Vec<String>,
    /// Display names of sessions in the guessed relevant restore group.
    pub display_names: Vec<String>,
    /// When the most recent crash occurred
    pub most_recent_crash: DateTime<Utc>,
    /// Crashed sessions excluded because they were outside the relevant group.
    pub omitted_crashed_count: usize,
}

/// Detect crashed sessions that can be batch restored.
/// Returns info about crashed sessions within the crash window (60 seconds),
/// excluding any that have already been recovered.
pub fn detect_crashed_sessions() -> Result<Option<CrashedSessionsInfo>> {
    let mut sessions: Vec<Session> = Vec::new();
    for id in stored_session_ids()? {
        if let Ok(mut session) = Session::load(&id) {
            if session.detect_crash() {
                let _ = session.save();
            }
            sessions.push(session);
        }
    }

    // Track existing recovery sessions to avoid showing already-recovered crashes
    let mut recovered_parents: HashSet<String> = HashSet::new();
    for s in &sessions {
        if s.id.starts_with("session_recovery_")
            && let Some(parent) = s.parent_id.as_ref()
        {
            recovered_parents.insert(parent.clone());
        }
    }

    // Filter to crashed sessions that haven't been recovered
    let mut crashed: Vec<Session> = sessions
        .into_iter()
        .filter(|s| matches!(s.status, SessionStatus::Crashed { .. }))
        .filter(|s| !recovered_parents.contains(&s.id))
        .collect();

    if crashed.is_empty() {
        return Ok(None);
    }

    let total_unrecovered_crashed = crashed.len();
    let most_recent = retain_relevant_crash_group(&mut crashed).unwrap_or_else(Utc::now);

    if crashed.is_empty() {
        return Ok(None);
    }

    // Sort by most recent first
    crashed.sort_by_key(|session| std::cmp::Reverse(crash_timestamp(session)));

    let session_ids: Vec<String> = crashed.iter().map(|s| s.id.clone()).collect();
    let display_names: Vec<String> = crashed
        .iter()
        .map(|s| s.display_name().to_string())
        .collect();

    Ok(Some(CrashedSessionsInfo {
        session_ids,
        display_names,
        most_recent_crash: most_recent,
        omitted_crashed_count: total_unrecovered_crashed.saturating_sub(crashed.len()),
    }))
}

/// Lightweight session header for fast scanning (skips messages array).
/// Uses serde's `deny_unknown_fields` = false (default) so the large `messages`
/// field is silently ignored during deserialization.
#[derive(Debug, Clone, Deserialize)]
struct SessionHeader {
    id: String,
    #[serde(default)]
    parent_id: Option<String>,
    #[serde(rename = "created_at")]
    _created_at: DateTime<Utc>,
    updated_at: DateTime<Utc>,
    #[serde(default)]
    short_name: Option<String>,
    #[serde(default)]
    status: SessionStatus,
    #[serde(default)]
    last_active_at: Option<DateTime<Utc>>,
}

impl SessionHeader {
    fn display_name(&self) -> &str {
        if let Some(ref name) = self.short_name {
            name
        } else if let Some(name) = extract_session_name(&self.id) {
            name
        } else {
            &self.id
        }
    }
}

/// Find recent crashed sessions for showing resume hints.
///
/// Uses a fast O(n) scan of `~/.jcode/active_pids/` (typically 0-5 files)
/// instead of scanning the full sessions directory (tens of thousands).
/// Each file in active_pids/ contains a PID; if that PID is dead, the
/// session crashed. We then load only those specific session files.
///
/// Falls back to the legacy directory scan if active_pids/ doesn't exist
/// (first run after upgrade).
pub fn find_recent_crashed_sessions() -> Vec<(String, String)> {
    if let Some(results) = find_crashed_via_pid_files() {
        return results;
    }
    find_crashed_legacy_scan()
}

/// Fast path: check active_pids/ directory for dead PIDs.
fn find_crashed_via_pid_files() -> Option<Vec<(String, String)>> {
    let dir = active_pids_dir()?;
    if !dir.exists() {
        return None;
    }

    let entries = std::fs::read_dir(&dir).ok()?;
    let cutoff = Utc::now() - Duration::hours(24);
    let mut crashed: Vec<(String, String, DateTime<Utc>)> = Vec::new();

    for entry in entries.flatten() {
        let session_id = match entry.file_name().to_str() {
            Some(s) => s.to_string(),
            None => continue,
        };

        let pid_str = match std::fs::read_to_string(entry.path()) {
            Ok(s) => s,
            Err(_) => continue,
        };
        let pid: u32 = match pid_str.trim().parse() {
            Ok(p) => p,
            Err(_) => {
                let _ = std::fs::remove_file(entry.path());
                continue;
            }
        };

        if is_pid_running(pid) {
            continue;
        }

        match Session::load(&session_id) {
            Ok(mut session) => {
                // Idle-vs-interrupted disposition (upstream #988, extended to
                // the PID detector): losing the process is only a crash when
                // it interrupts unfinished work. A leftover streaming marker
                // proves the session was mid-turn when its owner died
                // (`StreamingGuard` clears the marker on every exit path —
                // normal return, `?`, interrupt, panic — so only a killed
                // process leaves one behind). No marker = idle = ordinary
                // close, same as the disconnect path's idle rule. This stops
                // daemon restarts stamping every owned idle session Crashed.
                // Fail toward crash when the marker dir is unreadable: crash
                // recovery is fail-noisy, never fail-silent.
                if had_streaming_marker(&session_id) {
                    session.mark_crashed(Some(format!(
                        "Process {} exited unexpectedly (no shutdown signal captured)",
                        pid
                    )));
                } else {
                    session.mark_closed();
                }
                let _ = session.save();
                let ts = session.last_active_at.unwrap_or(session.updated_at);
                if ts <= cutoff {
                    continue;
                }
                let name = extract_session_name(&session_id)
                    .unwrap_or(&session_id)
                    .to_string();
                crashed.push((session_id, name, ts));
            }
            Err(_) => {
                let _ = std::fs::remove_file(entry.path());
            }
        }
    }

    crashed.sort_by(|a, b| b.2.cmp(&a.2));
    Some(
        crashed
            .into_iter()
            .map(|(id, name, _)| (id, name))
            .collect(),
    )
}

/// True when a streaming marker file exists for this session — i.e. the
/// session was mid-turn when its owner died. Any content counts (even a
/// stale marker naming the dead owner): `StreamingGuard` removes the file
/// on every orderly exit, so presence alone proves interruption. Missing
/// marker dir or I/O error returns true (fail toward crash recovery, never
/// silently close a session that may hold interrupted work).
fn had_streaming_marker(session_id: &str) -> bool {
    let Some(dir) = storage::streaming_pids_dir() else {
        return true;
    };
    match std::fs::read_dir(&dir) {
        Ok(_) => dir.join(session_id).exists(),
        Err(_) => true,
    }
}

/// Legacy fallback: scan the full sessions directory.
/// Used only on the first launch after upgrading to the active_pids system.
fn find_crashed_legacy_scan() -> Vec<(String, String)> {
    if SessionBackend::configured().ok() == Some(SessionBackend::Sqlite) {
        let Ok(store) = SessionStore::configured() else {
            return Vec::new();
        };
        let Ok(summaries) = store.list() else {
            return Vec::new();
        };
        let recovered: HashSet<_> = summaries
            .iter()
            .filter(|row| row.id.starts_with("session_recovery_"))
            .filter_map(|row| row.parent_id.as_deref())
            .collect();
        let cutoff = Utc::now() - Duration::hours(24);
        let mut crashed: Vec<_> = summaries
            .iter()
            .filter(|row| matches!(row.status, SessionStatus::Crashed { .. }))
            .filter(|row| !recovered.contains(row.id.as_str()))
            .filter(|row| row.last_active_at.unwrap_or(row.updated_at) > cutoff)
            .collect();
        crashed.sort_by_key(|row| std::cmp::Reverse(row.updated_at));
        return crashed
            .into_iter()
            .map(|row| {
                let name = row
                    .short_name
                    .as_deref()
                    .or_else(|| extract_session_name(&row.id))
                    .unwrap_or(&row.id)
                    .to_owned();
                (row.id.clone(), name)
            })
            .collect();
    }
    let sessions_dir = match storage::jcode_dir() {
        Ok(d) => d.join("sessions"),
        Err(_) => return Vec::new(),
    };
    if !sessions_dir.exists() {
        return Vec::new();
    }

    let cutoff = Utc::now() - Duration::hours(24);
    let cutoff_system = std::time::SystemTime::now()
        .checked_sub(std::time::Duration::from_secs(24 * 3600))
        .unwrap_or(std::time::SystemTime::UNIX_EPOCH);
    let filename_cutoff_ms: u64 = (chrono::Utc::now() - Duration::hours(48))
        .timestamp_millis()
        .max(0) as u64;

    let mut recovered_parents: HashSet<String> = HashSet::new();
    let mut candidates: Vec<SessionHeader> = Vec::new();

    let entries = match std::fs::read_dir(&sessions_dir) {
        Ok(e) => e,
        Err(_) => return Vec::new(),
    };

    for entry in entries.flatten() {
        if let Some(fname) = entry.file_name().to_str()
            && let Some(ts) = extract_timestamp_from_filename(fname)
            && ts < filename_cutoff_ms
        {
            continue;
        }

        let path = entry.path();
        if !path.extension().map(|e| e == "json").unwrap_or(false) {
            continue;
        }

        let meta = match entry.metadata() {
            Ok(m) => m,
            Err(_) => continue,
        };
        if let Ok(mtime) = meta.modified()
            && mtime < cutoff_system
        {
            continue;
        }
        if meta.len() == 0 {
            continue;
        }

        let content = match std::fs::read_to_string(&path) {
            Ok(c) => c,
            Err(_) => continue,
        };

        let has_crashed = content.contains("\"Crashed\"");
        let is_recovery = content.contains("\"session_recovery_\"");

        if !has_crashed && !is_recovery {
            continue;
        }

        if let Ok(header) = serde_json::from_str::<SessionHeader>(&content) {
            if header.id.starts_with("session_recovery_")
                && let Some(parent) = header.parent_id.as_ref()
            {
                recovered_parents.insert(parent.clone());
            }
            if has_crashed {
                candidates.push(header);
            }
        }
    }

    let mut crashed: Vec<SessionHeader> = candidates
        .into_iter()
        .filter(|s| matches!(s.status, SessionStatus::Crashed { .. }))
        .filter(|s| !recovered_parents.contains(&s.id))
        .filter(|s| {
            let ts = s.last_active_at.unwrap_or(s.updated_at);
            ts > cutoff
        })
        .collect();

    crashed.sort_by(|a, b| b.updated_at.cmp(&a.updated_at));

    crashed
        .into_iter()
        .map(|s| {
            let name = s.display_name().to_string();
            let id = s.id.clone();
            (id, name)
        })
        .collect()
}

/// Extract the epoch-ms timestamp embedded in a session filename.
/// Handles formats like:
///   "session_fox_1772405007295.json" (memorable id)
///   "session_1772405007295_hash.json" (legacy)
///   "session_recovery_1772405007295.json"
fn extract_timestamp_from_filename(filename: &str) -> Option<u64> {
    let stem = filename.strip_suffix(".json").unwrap_or(filename);
    // Walk the underscore-separated parts and find the first one that
    // looks like a plausible epoch-ms (13+ digits, starts with '1').
    for part in stem.split('_') {
        if part.len() >= 13 && part.starts_with('1') && part.chars().all(|c| c.is_ascii_digit()) {
            return part.parse::<u64>().ok();
        }
    }
    None
}

pub(super) fn is_pid_running(pid: u32) -> bool {
    crate::platform::is_process_running(pid)
}

// ---------------------------------------------------------------------------
// Active PID tracking
// ---------------------------------------------------------------------------
// Lightweight files in ~/.jcode/active_pids/<session_id> containing the PID.
// Written on mark_active(), removed on mark_closed()/mark_crashed().
// On startup we only need to scan this tiny directory (usually 0-5 files)
// instead of the entire sessions/ directory (tens of thousands of files).

fn normalize_resume_lookup_text(text: &str) -> String {
    text.split_whitespace()
        .collect::<Vec<_>>()
        .join(" ")
        .to_lowercase()
}

fn session_matches_resume_title(session: &Session, normalized_query: &str) -> bool {
    if normalized_query.is_empty() {
        return false;
    }

    session
        .display_title()
        .map(normalize_resume_lookup_text)
        .is_some_and(|title| title == normalized_query || title.contains(normalized_query))
}

/// Given a *bare* external provider id, return the id of a locally-imported
/// snapshot (`imported_<tool>_<id>`) if one already exists on disk.
///
/// External CLI sessions (OpenCode, Codex, Claude Code) are imported into the
/// jcode store under a stable `imported_<tool>_<provider_id>` stem. Resuming the
/// full imported id always works, but a *bare* provider id (e.g. the OpenCode
/// `ses_...` shown in the resume picker / reload handoff) previously only
/// resolved by re-importing from the external tool's own storage. Once the user
/// removed or reinstalled that tool, the re-import failed and resume hard-exited
/// with "No session found matching 'ses_...'" even though the imported snapshot
/// was still sitting in `~/.jcode/sessions` (issue #336).
///
/// Pi sessions are intentionally excluded: their imported id is a hash of the
/// session *path*, not the provider id, so there is no bare-id mapping.
fn resolve_imported_snapshot_id(provider_id: &str) -> Option<String> {
    // Don't double-prefix an id that is already an imported stem.
    if provider_id.starts_with("imported_") {
        return None;
    }

    [
        crate::import::imported_opencode_session_id(provider_id),
        crate::import::imported_codex_session_id(provider_id),
        crate::import::imported_claude_code_session_id(provider_id),
        crate::import::imported_cursor_session_id(provider_id),
    ]
    .into_iter()
    .find(|candidate| session_exists(candidate))
}

/// Find a session by ID, memorable name, generated title, or custom rename.
/// Existing exact snapshot IDs are resolved without reading their contents.
/// The authoritative session load still validates snapshots and replays journals.
/// Otherwise scan recent session snapshots for the newest matching name/title.
/// Returns the full session ID if found.
pub fn find_session_by_name_or_id(name_or_id: &str) -> Result<String> {
    // IDs are filename stems, never paths. Titles containing separators remain
    // searchable, but cannot escape the sessions directory via exact lookup.
    let is_stem = !name_or_id.is_empty()
        && name_or_id != "."
        && name_or_id != ".."
        && !name_or_id.contains(['/', '\\', '\0']);
    // Resolution is not validation. Reading a multi-MB snapshot here repeats
    // startup metadata reads and server restore, and can trigger client-side
    // backup recovery. Leave corruption detection to the authoritative load.
    if is_stem && session_exists(name_or_id) {
        return Ok(name_or_id.to_string());
    }

    // A *bare* external provider id (e.g. an OpenCode `ses_...` or a Codex/Claude
    // session id) may already have a locally-imported snapshot stored under an
    // `imported_<tool>_<id>` stem. Resolve to that snapshot before falling back to
    // re-importing from the external tool's storage, which fails outright once the
    // user has removed/reinstalled that tool (issue #336).
    if let Some(imported_id) = is_stem
        .then(|| resolve_imported_snapshot_id(name_or_id))
        .flatten()
    {
        return Ok(imported_id);
    }

    // A full generated id (`session_<name>_<millis>_<hex>`) can never match a
    // short name or title, so the scan below (which parses every session file
    // on disk) is pure overhead. With thousands of sessions that scan took
    // ~15s and stalled reload handoffs for ids that only exist server-side.
    if crate::id::is_generated_session_id(name_or_id) {
        anyhow::bail!("No session found matching '{}'", name_or_id);
    }

    // Otherwise, search for a session with matching short name or title.
    if SessionBackend::configured()? == SessionBackend::Files
        && !storage::jcode_dir()?.join("sessions").exists()
    {
        anyhow::bail!("No sessions found");
    }
    let normalized_query = normalize_resume_lookup_text(name_or_id);
    let mut exact_matches: Vec<(String, chrono::DateTime<chrono::Utc>)> = Vec::new();
    let mut title_matches: Vec<(String, chrono::DateTime<chrono::Utc>)> = Vec::new();

    for id in stored_session_ids()? {
        let stem = id.as_str();
        let short_name_matches =
            extract_session_name(stem).is_some_and(|short| short == name_or_id);
        if short_name_matches {
            if let Ok(session) = Session::load_startup_stub(stem).or_else(|_| Session::load(stem)) {
                exact_matches.push((stem.to_string(), session.updated_at));
            }
            continue;
        }

        let Ok(session) = Session::load_startup_stub(stem).or_else(|_| Session::load(stem)) else {
            continue;
        };
        if session.short_name.as_deref() == Some(name_or_id) {
            exact_matches.push((stem.to_string(), session.updated_at));
        } else if session_matches_resume_title(&session, &normalized_query) {
            title_matches.push((stem.to_string(), session.updated_at));
        }
    }

    let matches = if exact_matches.is_empty() {
        &mut title_matches
    } else {
        &mut exact_matches
    };

    if matches.is_empty() {
        anyhow::bail!("No session found matching '{}'", name_or_id);
    }

    // Sort by updated_at descending and return the most recent match.
    matches.sort_by(|a, b| b.1.cmp(&a.1));
    Ok(matches[0].0.clone())
}

#[cfg(test)]
mod batch_crash_tests {
    use super::*;

    struct HomeGuard(Option<std::ffi::OsString>);

    impl HomeGuard {
        fn set(path: &std::path::Path) -> Self {
            let previous = std::env::var_os("JCODE_HOME");
            crate::env::set_var("JCODE_HOME", path);
            Self(previous)
        }
    }

    impl Drop for HomeGuard {
        fn drop(&mut self) {
            if let Some(previous) = &self.0 {
                crate::env::set_var("JCODE_HOME", previous);
            } else {
                crate::env::remove_var("JCODE_HOME");
            }
        }
    }

    struct BackendGuard(Option<std::ffi::OsString>);

    impl Drop for BackendGuard {
        fn drop(&mut self) {
            if let Some(value) = &self.0 {
                crate::env::set_var("JCODE_SESSION_BACKEND", value);
            } else {
                crate::env::remove_var("JCODE_SESSION_BACKEND");
            }
        }
    }

    #[test]
    fn sqlite_only_session_name_lookup_and_crash_recovery() -> Result<()> {
        let _lock = crate::storage::lock_test_env();
        let home = tempfile::tempdir()?;
        let _home = HomeGuard::set(home.path());
        let _backend = BackendGuard(std::env::var_os("JCODE_SESSION_BACKEND"));
        crate::env::set_var("JCODE_SESSION_BACKEND", "sqlite");
        let mut session = Session::create_with_id(
            "session_sqlite_recovery_1234567890123".into(),
            None,
            Some("Database-only recovery".into()),
        );
        session.add_message(
            Role::User,
            vec![ContentBlock::Text {
                text: "Keep this message through recovery".into(),
                cache_control: None,
            }],
        );
        session.status = SessionStatus::Crashed {
            message: Some("interrupted".into()),
        };
        session.save()?;
        assert!(
            !home
                .path()
                .join("sessions")
                .join(format!("{}.json", session.id))
                .exists()
        );
        assert_eq!(find_session_by_name_or_id(&session.id)?, session.id);
        assert_eq!(
            find_session_by_name_or_id("Database-only recovery")?,
            session.id
        );
        let recovered = recover_crashed_sessions_by_ids(&[session.id.clone()])?;
        assert_eq!(recovered.len(), 1);
        let restored = Session::load(&recovered[0])?;
        assert_eq!(restored.parent_id.as_deref(), Some(session.id.as_str()));
        assert!(restored.messages.iter().flat_map(|message| &message.content).any(|block| {
            matches!(block, ContentBlock::Text { text, .. } if text == "Keep this message through recovery")
        }));
        assert!(recover_crashed_sessions_by_ids(&[session.id])?.is_empty());
        Ok(())
    }

    #[test]
    fn test_crashed_sessions_info_struct() {
        let info = CrashedSessionsInfo {
            session_ids: vec!["session_test_1".to_string(), "session_test_2".to_string()],
            display_names: vec!["fox".to_string(), "oak".to_string()],
            most_recent_crash: Utc::now(),
            omitted_crashed_count: 0,
        };
        assert_eq!(info.session_ids.len(), 2);
        assert_eq!(info.display_names.len(), 2);
        assert_eq!(info.display_names[0], "fox");
    }

    #[test]
    fn idle_dead_pid_without_streaming_marker_is_not_a_crash() {
        // Upstream #988 extended to the PID detector: a dead owner PID with
        // no leftover streaming marker means the session was idle — ordinary
        // close, not crash. Daemon restarts must not stamp idle sessions.
        let _guard = crate::storage::lock_test_env();
        let temp = tempfile::tempdir().expect("tempdir");
        crate::env::set_var("JCODE_HOME", temp.path());

        let dir = storage::streaming_pids_dir().expect("streaming dir");
        std::fs::create_dir_all(&dir).expect("marker dir");
        assert!(
            !had_streaming_marker("session_idle_1"),
            "no marker file means idle"
        );

        crate::env::remove_var("JCODE_HOME");
    }

    #[test]
    fn leftover_streaming_marker_means_interrupted() {
        // A marker file left behind (StreamingGuard clears it on every
        // orderly exit) proves the owner died mid-turn — genuine crash.
        let _guard = crate::storage::lock_test_env();
        let temp = tempfile::tempdir().expect("tempdir");
        crate::env::set_var("JCODE_HOME", temp.path());

        let dir = storage::streaming_pids_dir().expect("streaming dir");
        std::fs::create_dir_all(&dir).expect("marker dir");
        std::fs::write(dir.join("session_midturn_1"), "999999").expect("marker");
        assert!(
            had_streaming_marker("session_midturn_1"),
            "leftover marker means interrupted"
        );
        assert!(
            !had_streaming_marker("session_other_1"),
            "other sessions unaffected"
        );

        crate::env::remove_var("JCODE_HOME");
    }

    #[test]
    fn exact_resume_resolution_defers_corruption_to_authoritative_load() -> anyhow::Result<()> {
        let _guard = crate::storage::lock_test_env();
        let temp = tempfile::tempdir()?;
        let _home = HomeGuard::set(temp.path());
        std::fs::create_dir_all(temp.path().join("sessions"))?;
        for id in ["session_corrupt_1770000000000", "imported_codex_corrupt"] {
            let path = session_path(id)?;
            std::fs::write(&path, b"not JSON")?;
            assert_eq!(find_session_by_name_or_id(id)?, id);
            assert!(
                Session::load(id).is_err(),
                "authoritative load must reject corruption"
            );
            assert_eq!(std::fs::read(&path)?, b"not JSON");
        }
        Ok(())
    }

    #[test]
    fn exact_resume_resolution_rejects_paths_and_directories() -> anyhow::Result<()> {
        let _guard = crate::storage::lock_test_env();
        let temp = tempfile::tempdir()?;
        let _home = HomeGuard::set(temp.path());
        std::fs::create_dir_all(temp.path().join("sessions"))?;
        std::fs::write(temp.path().join("outside.json"), b"{}")?;
        std::fs::create_dir(temp.path().join("sessions/session_directory_1770000000000"))?;
        std::fs::create_dir(
            temp.path()
                .join("sessions/session_directory_1770000000000.json"),
        )?;
        for query in [
            "../outside".to_string(),
            temp.path().join("outside").to_string_lossy().into_owned(),
            "..\\outside".to_string(),
            "session_directory_1770000000000".to_string(),
        ] {
            assert!(
                find_session_by_name_or_id(&query).is_err(),
                "accepted {query:?}"
            );
        }
        Ok(())
    }

    #[test]
    fn exact_resume_resolution_preserves_journal_freshness() -> anyhow::Result<()> {
        let _guard = crate::storage::lock_test_env();
        let temp = tempfile::tempdir()?;
        let _home = HomeGuard::set(temp.path());
        let id = "session_resume_journal_1770000000000";
        let mut session = Session::create_with_id(id.to_string(), None, None);
        session.add_message(
            Role::User,
            vec![ContentBlock::Text {
                text: "x".repeat(1024 * 1024),
                cache_control: None,
            }],
        );
        session.save()?;
        let snapshot = std::fs::read(session_path(id)?)?;
        session.custom_title = Some("Newest journal title".to_string());
        session.status = SessionStatus::Closed;
        session.save()?;
        assert_eq!(std::fs::read(session_path(id)?)?, snapshot);
        assert_eq!(find_session_by_name_or_id(id)?, id);
        let loaded = Session::load(id)?;
        assert_eq!(loaded.custom_title.as_deref(), Some("Newest journal title"));
        assert!(matches!(loaded.status, SessionStatus::Closed));
        let stub = Session::load_startup_stub(id)?;
        assert_eq!(stub.custom_title, loaded.custom_title);
        assert_eq!(find_session_by_name_or_id("Newest journal title")?, id);
        Ok(())
    }

    #[test]
    #[ignore = "read-only real local corpus benchmark, requires JCODE_RESUME_BENCH_IDS"]
    fn benchmark_exact_resume_resolution_real_corpus() -> anyhow::Result<()> {
        // Run alone with --local and --ignored --nocapture. Never save, restore,
        // clear caches, or call the full loader (which may recover a backup).
        // This measures resolution helpers only, NOT end-to-end TUI startup.
        let ids = std::env::var("JCODE_RESUME_BENCH_IDS")?;
        for id in ids.split(',') {
            let path = session_path(id)?;
            let before = std::fs::metadata(&path)?;
            let mut old_us = Vec::new();
            let mut new_us = Vec::new();
            for _ in 0..9 {
                let start = std::time::Instant::now();
                assert!(Session::load_startup_stub(id).is_ok());
                old_us.push(start.elapsed().as_micros());
                let start = std::time::Instant::now();
                assert_eq!(find_session_by_name_or_id(id)?, id);
                new_us.push(start.elapsed().as_micros());
            }
            let after = std::fs::metadata(&path)?;
            assert_eq!(before.len(), after.len());
            assert_eq!(before.modified()?, after.modified()?);
            let first_baseline_us = old_us[0];
            old_us.sort_unstable();
            new_us.sort_unstable();
            println!(
                "resolution_helper id={id} bytes={} samples=9 baseline_stub_us_p50={} pmax={} exact_stat_us_p50={} pmax={} first_baseline_us={} cache=uncontrolled_no_eviction",
                before.len(),
                old_us[4],
                old_us[8],
                new_us[4],
                new_us[8],
                first_baseline_us
            );
        }
        Ok(())
    }

    #[test]
    fn find_session_by_name_or_id_matches_custom_title() {
        let _guard = crate::storage::lock_test_env();
        let temp = tempfile::tempdir().expect("tempdir");
        crate::env::set_var("JCODE_HOME", temp.path());

        let session_id = "session_renamecli_1770000000000";
        let mut session = Session::create_with_id(
            session_id.to_string(),
            None,
            Some("Generated planning title".to_string()),
        );
        session.status = SessionStatus::Closed;
        session.rename_title(Some("RenameTest".to_string()));
        session.save().expect("save renamed session");

        assert_eq!(
            find_session_by_name_or_id("renametest").expect("resolve custom title"),
            session_id
        );
        assert_eq!(
            find_session_by_name_or_id("Rename").expect("resolve title fragment"),
            session_id
        );

        crate::env::remove_var("JCODE_HOME");
    }

    #[test]
    fn find_session_by_name_or_id_accepts_imported_session_ids() -> anyhow::Result<()> {
        let _guard = crate::storage::lock_test_env();
        let temp = tempfile::tempdir()?;
        crate::env::set_var("JCODE_HOME", temp.path());

        let imported_id = "imported_codex_test_resume";
        let mut session =
            Session::create_with_id(imported_id.to_string(), None, Some("Imported".to_string()));
        session.status = SessionStatus::Closed;
        session.save()?;

        let resolved = find_session_by_name_or_id(imported_id)?;
        assert_eq!(resolved, imported_id);

        crate::env::remove_var("JCODE_HOME");
        Ok(())
    }

    /// Regression test for issue #336: resuming a *bare* external provider id
    /// (e.g. an OpenCode `ses_...`) must resolve to its already-imported local
    /// snapshot even when the external tool's storage has been removed, instead
    /// of hard-failing with "No session found matching 'ses_...'".
    #[test]
    fn find_session_by_name_or_id_resolves_bare_opencode_id_to_imported_snapshot()
    -> anyhow::Result<()> {
        let _guard = crate::storage::lock_test_env();
        let temp = tempfile::tempdir()?;
        crate::env::set_var("JCODE_HOME", temp.path());

        let provider_id = "ses_2c72f8f4cffee6Qh7GId7D81Se";
        let imported_id = crate::import::imported_opencode_session_id(provider_id);
        let mut session = Session::create_with_id(
            imported_id.clone(),
            None,
            Some("Analyzing /provider routing".to_string()),
        );
        session.status = SessionStatus::Closed;
        session.provider_session_id = Some(provider_id.to_string());
        session.provider_key = Some("opencode".to_string());
        session.save()?;

        // No external OpenCode store exists under JCODE_HOME/external, so the only
        // way to resolve the bare id is via the local imported snapshot.
        let resolved = find_session_by_name_or_id(provider_id)?;
        assert_eq!(resolved, imported_id);

        crate::env::remove_var("JCODE_HOME");
        Ok(())
    }

    /// A bare provider id that has *no* imported snapshot on disk must still fail
    /// (so the caller can fall back to external re-import or surface an error).
    #[test]
    fn find_session_by_name_or_id_bare_id_without_snapshot_still_errors() -> anyhow::Result<()> {
        let _guard = crate::storage::lock_test_env();
        let temp = tempfile::tempdir()?;
        crate::env::set_var("JCODE_HOME", temp.path());
        std::fs::create_dir_all(temp.path().join("sessions"))?;

        let err = find_session_by_name_or_id("ses_does_not_exist_anywhere");
        assert!(err.is_err(), "expected unknown bare id to error");

        crate::env::remove_var("JCODE_HOME");
        Ok(())
    }
}
