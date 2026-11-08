//! Mobile transcript sync.
//!
//! When the user turns mobile sync on (`jcode mobile enable`), jcode uploads
//! session transcripts to the Jcode account API so the iOS app can show them.
//! Local sessions and sessions running on Jcode Cloud machines both upload with
//! the account API key. Uploads are incremental: per-session high-water marks
//! (message count + snapshot/journal revisions) live in `~/.jcode/mobile_sync.json`.
//! Successful session persistence wakes a debounced incremental pass. A 20s
//! reconciliation also catches saves made by other processes and retries.
//!
//! Only user-visible text is uploaded: user/assistant text, tool calls as a
//! one-line summary, and tool results truncated. Hidden reasoning, images and
//! provider-native blobs are never sent.

use crate::session::Session;
use crate::storage;
use crate::subscription_api;
use anyhow::{Context, Result};
use jcode_message_types::{ContentBlock, Role};
use serde::{Deserialize, Serialize};
use std::collections::{BTreeMap, BTreeSet};
use std::hash::{Hash, Hasher};
use std::path::{Path, PathBuf};
use std::sync::Mutex;
use std::time::{Duration, SystemTime, UNIX_EPOCH};

pub const SYNC_INTERVAL: Duration = Duration::from_secs(20);
const MAX_MESSAGES_PER_UPLOAD: usize = 200;
const MAX_TOOL_RESULT_CHARS: usize = 4_000;
const MAX_TEXT_CHARS: usize = 100_000;
/// Only sessions touched within this window are synced on enable.
const BACKFILL_WINDOW: Duration = Duration::from_secs(14 * 24 * 3600);
const SYNC_DEBOUNCE: Duration = Duration::from_millis(150);
// Also bounded when no background worker runs or mobile sync is disabled.
// Evicted hints are recovered by the periodic snapshot/journal reconciliation.
const MAX_DIRTY_SESSIONS: usize = 1024;
static PERSISTED: tokio::sync::Notify = tokio::sync::Notify::const_new();
static DIRTY_SESSIONS: Mutex<BTreeSet<String>> = Mutex::new(BTreeSet::new());
static SYNC_PASS: tokio::sync::Mutex<()> = tokio::sync::Mutex::const_new(());

/// Called only after a durable session write, never after sync-state writes.
/// Notify retains a permit while an upload is running. The set also retains
/// same-millisecond writes which the reconciliation mtime cannot distinguish.
pub(crate) fn notify_session_persisted(id: &str) {
    mark_dirty(id);
    PERSISTED.notify_one();
}

fn mark_dirty(id: &str) {
    if !valid_session_id(id) {
        return;
    }
    let mut dirty = DIRTY_SESSIONS.lock().unwrap_or_else(|e| e.into_inner());
    if dirty.len() == MAX_DIRTY_SESSIONS && !dirty.contains(id) {
        dirty.pop_first();
    }
    dirty.insert(id.to_owned());
}

fn state_lock() -> Result<std::fs::File> {
    let dir = storage::jcode_dir()?;
    std::fs::create_dir_all(&dir)?;
    let path = dir.join("mobile_sync.lock");
    let file = std::fs::OpenOptions::new()
        .create(true)
        .truncate(false)
        .read(true)
        .write(true)
        .open(path)?;
    file.lock()?;
    Ok(file)
}

#[derive(Debug, Clone, Default, Serialize, Deserialize, PartialEq)]
pub struct MobileSyncState {
    #[serde(default)]
    pub enabled: bool,
    /// Fences uploads started before a disable/re-enable cycle.
    #[serde(default)]
    pub generation: u64,
    /// session id -> progress
    #[serde(default)]
    pub sessions: BTreeMap<String, SessionProgress>,
}

#[derive(Debug, Clone, Default, Serialize, Deserialize, PartialEq)]
pub struct SessionProgress {
    /// Number of transcript entries already uploaded.
    pub uploaded: usize,
    /// Snapshot+journal mtime (unix ms) at the last successful pass.
    pub mtime_ms: u64,
    /// Snapshot and journal (nanosecond mtime, byte length). Older state files
    /// omit this and receive one full reconciliation on upgrade.
    #[serde(default)]
    pub revision: Option<[u64; 4]>,
    /// Hash of previously uploaded visible entries. Rewrites below the count
    /// high-water mark must be replayed, not mistaken for metadata-only saves.
    #[serde(default)]
    pub fingerprint: Option<u64>,
}

#[derive(Debug, Clone, Serialize, PartialEq, Hash)]
pub struct WireMessage {
    pub seq: usize,
    pub role: &'static str,
    pub text: String,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub created_at: Option<String>,
}

/// Session metadata returned by the account's mobile transcript API.
#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
pub struct SyncedSession {
    pub session_id: String,
    pub title: Option<String>,
    pub source: String,
    pub host: Option<String>,
    pub cwd: Option<String>,
    pub updated_at: Option<String>,
    pub created_at: Option<String>,
}

/// Bounded account inventory read for UI background workers.
pub async fn discover_cloud_sessions() -> Result<Vec<SyncedSession>> {
    let client = reqwest::Client::new();
    tokio::time::timeout(Duration::from_secs(8), list_cloud_sessions(&client))
        .await
        .context("mobile session discovery timed out")?
}

/// List cloud sessions synced to the configured Jcode account.
pub async fn list_cloud_sessions(client: &reqwest::Client) -> Result<Vec<SyncedSession>> {
    let api_key = crate::subscription_catalog::configured_api_key()
        .context("no Jcode account credential is configured")?;
    list_cloud_sessions_with_endpoint(client, &subscription_api::configured_api_base(), &api_key)
        .await
}

async fn list_cloud_sessions_with_endpoint(
    client: &reqwest::Client,
    api_base: &str,
    api_key: &str,
) -> Result<Vec<SyncedSession>> {
    #[derive(Deserialize)]
    struct SessionsResponse {
        sessions: Vec<SyncedSession>,
    }

    let response = client
        .get(format!(
            "{}/mobile/sessions",
            api_base.trim_end_matches('/')
        ))
        .query(&[("limit", "500")])
        .bearer_auth(api_key)
        .timeout(Duration::from_secs(30))
        .send()
        .await
        .context("mobile session listing failed")?
        .error_for_status()
        .context("mobile session listing returned an error")?;
    let response: SessionsResponse = response
        .json()
        .await
        .context("invalid mobile session listing response")?;
    Ok(response
        .sessions
        .into_iter()
        .filter(|session| session.source == "cloud" && valid_session_id(&session.session_id))
        .collect())
}

#[derive(Debug, Clone, Serialize)]
struct UploadBody<'a> {
    title: Option<&'a str>,
    source: &'static str,
    host: Option<String>,
    cwd: Option<&'a str>,
    model: Option<&'a str>,
    created_at: String,
    updated_at: String,
    messages: &'a [WireMessage],
}

fn state_path() -> Result<PathBuf> {
    Ok(storage::jcode_dir()?.join("mobile_sync.json"))
}

pub fn load_state() -> MobileSyncState {
    let state = (|| -> Result<MobileSyncState> {
        let path = state_path()?;
        if !path.exists() {
            return Ok(MobileSyncState::default());
        }
        storage::read_json(&path)
    })();
    match state {
        Ok(state) => state,
        Err(error) => {
            crate::logging::warn(&format!("Could not load mobile sync state: {error:#}"));
            MobileSyncState::default()
        }
    }
}

pub fn save_state(state: &MobileSyncState) -> Result<()> {
    let _lock = state_lock()?;
    // Opt-in/out is authoritative, but an enable based on a stale read must
    // not discard progress committed by another daemon in the meantime.
    let mut next = load_state();
    if next.enabled != state.enabled {
        next.generation = next.generation.wrapping_add(1);
    }
    next.enabled = state.enabled;
    if state.enabled {
        if next.generation == state.generation {
            merge_progress(&mut next, &state.sessions);
        }
    } else {
        next.sessions.clear();
    }
    storage::write_json_secret(&state_path()?, &next)
}

fn merge_progress(state: &mut MobileSyncState, updates: &BTreeMap<String, SessionProgress>) {
    for (id, progress) in updates {
        let version = |p: &SessionProgress| {
            p.revision
                .map(|r| r[0].max(r[2]))
                .unwrap_or(p.mtime_ms.saturating_mul(1_000_000))
        };
        if state.sessions.get(id).is_none_or(|old| {
            version(old) < version(progress)
                || (version(old) == version(progress) && old.uploaded <= progress.uploaded)
        }) {
            state.sessions.insert(id.clone(), progress.clone());
        }
    }
}

fn commit_progress(generation: u64, updates: &BTreeMap<String, SessionProgress>) -> Result<()> {
    let _lock = state_lock()?;
    let mut current = load_state();
    if current.enabled && current.generation == generation {
        merge_progress(&mut current, updates);
        storage::write_json_secret(&state_path()?, &current)?;
    }
    Ok(())
}

pub fn is_enabled() -> bool {
    load_state().enabled
}

fn sync_still_enabled(generation: u64) -> bool {
    let state = load_state();
    state.enabled && state.generation == generation
}

/// `cloud` when running on a Jcode Cloud machine, `local` otherwise.
pub fn session_source() -> &'static str {
    if std::env::var_os("JCODE_CLOUD_ENV_SESSION").is_some()
        || std::env::var_os("JCODE_CLOUD_MACHINE").is_some()
        || std::env::var("JCODE_SERVER_NAME").as_deref() == Ok("jcode-cloud")
        || Path::new("/opt/jcode-cloud/ready").exists()
    {
        "cloud"
    } else {
        "local"
    }
}

fn host_name() -> Option<String> {
    std::env::var_os("HOSTNAME")
        .map(|value| value.to_string_lossy().into_owned())
        .or_else(|| match std::fs::read_to_string("/etc/hostname") {
            Ok(value) => Some(value),
            Err(error) if error.kind() == std::io::ErrorKind::NotFound => None,
            Err(error) => {
                crate::logging::debug(&format!("Mobile hostname unavailable: {error}"));
                None
            }
        })
        .map(|s| s.trim().to_string())
        .filter(|s| !s.is_empty())
}

fn truncate(text: &str, max: usize) -> String {
    if text.chars().count() <= max {
        return text.to_string();
    }
    let mut out: String = text.chars().take(max).collect();
    out.push_str("\n… (truncated)");
    out
}

fn tool_summary(name: &str, input: &serde_json::Value) -> String {
    let detail = input
        .get("intent")
        .or_else(|| input.get("command"))
        .or_else(|| input.get("file_path"))
        .or_else(|| input.get("query"))
        .and_then(|v| v.as_str())
        .unwrap_or("");
    if detail.is_empty() {
        format!("🔧 {name}")
    } else {
        format!("🔧 {name}: {}", truncate(detail, 200))
    }
}

/// Convert stored messages into the mobile wire format. One stored message
/// becomes at most one wire message; `seq` is the stored index so uploads are
/// stable and idempotent.
pub fn wire_messages(session: &Session, start: usize) -> Vec<WireMessage> {
    session
        .messages
        .iter()
        .enumerate()
        .skip(start)
        .filter_map(|(seq, message)| {
            let mut parts: Vec<String> = Vec::new();
            let mut is_tool_result = false;
            for block in &message.content {
                match block {
                    ContentBlock::Text { text, .. }
                        if !text.trim().is_empty()
                            && !text.trim_start().starts_with("<system-reminder>") =>
                    {
                        parts.push(text.clone())
                    }
                    ContentBlock::ToolUse { name, input, .. } => {
                        parts.push(tool_summary(name, input))
                    }
                    ContentBlock::ToolResult { content, .. } => {
                        is_tool_result = true;
                        parts.push(truncate(content, MAX_TOOL_RESULT_CHARS));
                    }
                    _ => {}
                }
            }
            if parts.is_empty() {
                return None;
            }
            let role = if is_tool_result {
                "tool"
            } else if message.display_role.is_some() {
                "system"
            } else {
                match message.role {
                    Role::User => "user",
                    Role::Assistant => "assistant",
                }
            };
            Some(WireMessage {
                seq,
                role,
                text: truncate(&parts.join("\n\n"), MAX_TEXT_CHARS),
                created_at: message.timestamp.map(|t| t.to_rfc3339()),
            })
        })
        .collect()
}

fn wire_fingerprint(messages: &[WireMessage], end: usize) -> u64 {
    let mut hash = std::collections::hash_map::DefaultHasher::new();
    for message in messages.iter().take_while(|message| message.seq < end) {
        message.hash(&mut hash);
    }
    hash.finish()
}

fn incremental_messages(session: &Session, progress: &SessionProgress) -> (Vec<WireMessage>, u64) {
    let mut messages = wire_messages(session, 0);
    let fingerprint = wire_fingerprint(&messages, session.messages.len());
    let unchanged_prefix = progress.uploaded <= session.messages.len()
        && progress.fingerprint == Some(wire_fingerprint(&messages, progress.uploaded));
    if unchanged_prefix {
        messages.retain(|message| message.seq >= progress.uploaded);
    }
    (messages, fingerprint)
}

fn mtime_ms(path: &Path) -> u64 {
    match std::fs::metadata(path).and_then(|metadata| metadata.modified()) {
        Ok(modified) => match modified.duration_since(UNIX_EPOCH) {
            Ok(duration) => duration.as_millis() as u64,
            Err(error) => {
                crate::logging::debug(&format!("Mobile snapshot predates epoch: {error}"));
                0
            }
        },
        Err(error) if error.kind() == std::io::ErrorKind::NotFound => 0,
        Err(error) => {
            crate::logging::debug(&format!("Mobile snapshot mtime unavailable: {error}"));
            0
        }
    }
}

fn session_mtime_ms(snapshot: &Path) -> u64 {
    mtime_ms(snapshot).max(mtime_ms(
        &crate::session::session_journal_path_from_snapshot(snapshot),
    ))
}

fn session_revision(snapshot: &Path) -> [u64; 4] {
    fn stamp(path: &Path) -> (u64, u64) {
        let Ok(meta) = std::fs::metadata(path) else {
            return (0, 0);
        };
        let modified = meta
            .modified()
            .ok()
            .and_then(|t| t.duration_since(UNIX_EPOCH).ok())
            .map(|d| d.as_nanos().min(u64::MAX as u128) as u64)
            .unwrap_or(0);
        (modified, meta.len())
    }
    let (snapshot_time, snapshot_len) = stamp(snapshot);
    let (journal_time, journal_len) = stamp(&crate::session::session_journal_path_from_snapshot(
        snapshot,
    ));
    [snapshot_time, snapshot_len, journal_time, journal_len]
}

/// Candidate sessions: real (non-debug) snapshots modified since the window.
fn candidate_sessions(since_ms: u64) -> Result<Vec<(String, PathBuf, u64)>> {
    let dir = storage::jcode_dir()?.join("sessions");
    let mut out = Vec::new();
    let Ok(entries) = std::fs::read_dir(&dir) else {
        return Ok(out);
    };
    for entry in entries.flatten() {
        let path = entry.path();
        if path.extension().and_then(|e| e.to_str()) != Some("json") {
            continue;
        }
        let Some(id) = path
            .file_stem()
            .and_then(|s| s.to_str())
            .map(str::to_string)
        else {
            continue;
        };
        let mtime = session_mtime_ms(&path);
        if mtime >= since_ms {
            out.push((id, path, mtime));
        }
    }
    out.sort_by(|a, b| b.2.cmp(&a.2));
    Ok(out)
}

#[derive(Debug, Default, Clone, PartialEq, Eq)]
pub struct SyncReport {
    pub sessions_uploaded: usize,
    pub messages_uploaded: usize,
    pub errors: usize,
}

async fn upload(
    client: &reqwest::Client,
    api_base: &str,
    api_key: &str,
    session_id: &str,
    body: &UploadBody<'_>,
) -> Result<()> {
    let url = format!(
        "{}/mobile/sessions/{}",
        api_base.trim_end_matches('/'),
        session_id
    );
    let response = client
        .post(url)
        .bearer_auth(api_key)
        .json(body)
        .timeout(Duration::from_secs(30))
        .send()
        .await
        .context("mobile sync upload failed")?;
    let status = response.status();
    if !status.is_success() {
        let text = response.text().await.with_context(|| {
            format!("mobile sync upload returned {status}, response body unreadable")
        })?;
        anyhow::bail!(
            "mobile sync upload returned {status}: {}",
            text.chars().take(300).collect::<String>()
        );
    }
    Ok(())
}

fn valid_session_id(id: &str) -> bool {
    !id.is_empty()
        && id.len() <= 128
        && id
            .chars()
            .all(|c| c.is_ascii_alphanumeric() || matches!(c, '.' | '_' | ':' | '-'))
}

/// One incremental sync pass. Safe to call repeatedly.
pub async fn sync_once(client: &reqwest::Client) -> Result<SyncReport> {
    sync_pass(client, true).await
}

async fn sync_pass(client: &reqwest::Client, reconcile: bool) -> Result<SyncReport> {
    let _pass = SYNC_PASS.lock().await;
    let mut state = load_state();
    if !state.enabled {
        return Ok(SyncReport::default());
    }
    let api_key = crate::subscription_catalog::configured_api_key()
        .context("mobile sync needs a Jcode account: run `jcode account login`")?;
    let api_base = subscription_api::configured_api_base();
    let now_ms = SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .map(|d| d.as_millis() as u64)
        .unwrap_or(0);
    let since = now_ms.saturating_sub(BACKFILL_WINDOW.as_millis() as u64);
    let host = host_name();
    let source = session_source();
    let mut report = SyncReport::default();
    let mut candidates = if reconcile {
        candidate_sessions(since)?
    } else {
        Vec::new()
    };
    let dirty = std::mem::take(&mut *DIRTY_SESSIONS.lock().unwrap_or_else(|e| e.into_inner()));
    for id in &dirty {
        if valid_session_id(id) && !candidates.iter().any(|(candidate, _, _)| candidate == id) {
            let path = storage::jcode_dir()?
                .join("sessions")
                .join(format!("{id}.json"));
            candidates.push((id.clone(), path.clone(), session_mtime_ms(&path)));
        }
    }
    candidates.sort_by_key(|(id, _, _)| !dirty.contains(id));

    let mut candidates = candidates.into_iter();
    while let Some((id, path, mtime)) = candidates.next() {
        if !sync_still_enabled(state.generation) {
            break;
        }
        if !valid_session_id(&id) {
            continue;
        }
        // No previous upload means a new session, not an error.
        let progress = state
            .sessions
            .get(&id)
            .cloned()
            .unwrap_or_else(SessionProgress::default);
        let revision = session_revision(&path);
        if progress.revision == Some(revision) && !dirty.contains(&id) {
            continue;
        }
        let session = match Session::load_from_path(&path) {
            Ok(s) => s,
            Err(_) => {
                mark_dirty(&id);
                continue;
            }
        };
        if session.is_debug || session.messages.is_empty() {
            state.sessions.insert(
                id,
                SessionProgress {
                    uploaded: progress.uploaded,
                    mtime_ms: mtime,
                    revision: Some(revision),
                    fingerprint: progress.fingerprint,
                },
            );
            continue;
        }
        let (messages, fingerprint) = incremental_messages(&session, &progress);
        let title = session.custom_title.as_deref().or(session.title.as_deref());
        let mut ok = true;
        // Always send at least one request so metadata (title, updated_at) refreshes.
        let chunks: Vec<&[WireMessage]> = if messages.is_empty() {
            vec![&[]]
        } else {
            messages.chunks(MAX_MESSAGES_PER_UPLOAD).collect()
        };
        for chunk in chunks {
            if !sync_still_enabled(state.generation) {
                ok = false;
                break;
            }
            let body = UploadBody {
                title,
                source,
                host: host.clone(),
                cwd: session.working_dir.as_deref(),
                model: session.model.as_deref(),
                created_at: session.created_at.to_rfc3339(),
                updated_at: session.updated_at.to_rfc3339(),
                messages: chunk,
            };
            if let Err(err) = upload(client, &api_base, &api_key, &id, &body).await {
                crate::logging::debug(&format!("mobile sync {id}: {err:#}"));
                report.errors += 1;
                ok = false;
                break;
            }
            report.messages_uploaded += chunk.len();
        }
        if ok {
            report.sessions_uploaded += 1;
            state.sessions.insert(
                id,
                SessionProgress {
                    uploaded: session.messages.len(),
                    mtime_ms: mtime,
                    revision: Some(revision),
                    fingerprint: Some(fingerprint),
                },
            );
        } else {
            mark_dirty(&id);
        }
        if reconcile
            && !DIRTY_SESSIONS
                .lock()
                .unwrap_or_else(|e| e.into_inner())
                .is_empty()
        {
            // Finish this session, commit its progress, then let the event pass
            // handle fresh saves. Unvisited backlog remains discoverable on the
            // next periodic reconciliation. Preserve any consumed dirty hints.
            for (id, _, _) in candidates {
                if dirty.contains(&id) {
                    mark_dirty(&id);
                }
            }
            break;
        }
    }
    // Merge under a cross-process lock, never write a stale enabled flag or
    // overwrite another process's unrelated session progress.
    commit_progress(state.generation, &state.sessions)?;
    Ok(report)
}

/// Delete everything this account has stored for mobile.
pub async fn wipe_remote(client: &reqwest::Client) -> Result<()> {
    let api_key = crate::subscription_catalog::configured_api_key()
        .context("no Jcode account credential is configured")?;
    let url = format!(
        "{}/mobile/sessions",
        subscription_api::configured_api_base().trim_end_matches('/')
    );
    let response = client
        .delete(url)
        .bearer_auth(api_key)
        .timeout(Duration::from_secs(30))
        .send()
        .await?;
    if !response.status().is_success() {
        anyhow::bail!("server returned {}", response.status());
    }
    Ok(())
}

/// Background loop used by the jcode server daemon.
pub fn spawn_background_sync() {
    tokio::spawn(background_sync());
}

async fn background_sync() {
    let client = reqwest::Client::new();
    let mut ticks = tokio::time::interval(SYNC_INTERVAL);
    ticks.set_missed_tick_behavior(tokio::time::MissedTickBehavior::Skip);
    loop {
        let reconcile = tokio::select! {
            _ = ticks.tick() => true,
            _ = PERSISTED.notified() => {
                // A bounded coalescing window, not a trailing debounce:
                // continuous token saves cannot postpone sync forever.
                tokio::time::sleep(SYNC_DEBOUNCE).await;
                false
            }
        };
        if !is_enabled() {
            continue;
        }
        if let Err(err) = sync_pass(&client, reconcile).await {
            crate::logging::debug(&format!("mobile sync pass failed: {err:#}"));
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use jcode_session_types::StoredMessage;

    struct TestEnv(Vec<(&'static str, Option<std::ffi::OsString>)>);

    impl TestEnv {
        fn set(values: &[(&'static str, &std::ffi::OsStr)]) -> Self {
            Self(
                values
                    .iter()
                    .map(|(name, value)| {
                        let previous = std::env::var_os(name);
                        crate::env::set_var(name, value);
                        (*name, previous)
                    })
                    .collect(),
            )
        }
    }

    impl Drop for TestEnv {
        fn drop(&mut self) {
            for (name, value) in &self.0 {
                if let Some(value) = value {
                    crate::env::set_var(name, value);
                } else {
                    crate::env::remove_var(name);
                }
            }
        }
    }

    #[test]
    fn disabled_dirty_hints_are_bounded_without_reading_sync_state() {
        let _lock = storage::lock_test_env();
        let home = tempfile::tempdir().unwrap();
        let _env = TestEnv::set(&[("JCODE_HOME", home.path().as_os_str())]);
        DIRTY_SESSIONS.lock().unwrap().clear();
        for index in 0..MAX_DIRTY_SESSIONS * 2 {
            notify_session_persisted(&format!("disabled-{index}"));
        }
        notify_session_persisted("../invalid");
        let mut dirty = DIRTY_SESSIONS.lock().unwrap();
        assert_eq!(dirty.len(), MAX_DIRTY_SESSIONS);
        assert!(dirty.contains(&format!("disabled-{}", MAX_DIRTY_SESSIONS * 2 - 1)));
        assert!(!home.path().join("mobile_sync.json").exists());
        dirty.clear();
    }

    #[test]
    fn state_commits_merge_concurrent_progress_and_never_reenable() {
        let _lock = storage::lock_test_env();
        let home = tempfile::tempdir().unwrap();
        let _env = TestEnv::set(&[("JCODE_HOME", home.path().as_os_str())]);
        save_state(&MobileSyncState {
            enabled: true,
            ..Default::default()
        })
        .unwrap();
        let generation = load_state().generation;
        let progress = |uploaded, mtime_ms| SessionProgress {
            uploaded,
            mtime_ms,
            ..Default::default()
        };
        let stale = BTreeMap::from([("first".into(), progress(1, 1))]);
        commit_progress(
            generation,
            &BTreeMap::from([("second".into(), progress(2, 2))]),
        )
        .unwrap();
        commit_progress(generation, &stale).unwrap();
        assert_eq!(load_state().sessions.len(), 2);
        commit_progress(
            generation,
            &BTreeMap::from([("first".into(), progress(3, 3))]),
        )
        .unwrap();
        commit_progress(generation, &stale).unwrap();
        assert_eq!(load_state().sessions["first"].uploaded, 3);
        let barrier = std::sync::Arc::new(std::sync::Barrier::new(8));
        let writers: Vec<_> = (0..8)
            .map(|index| {
                let barrier = barrier.clone();
                std::thread::spawn(move || {
                    barrier.wait();
                    commit_progress(
                        generation,
                        &BTreeMap::from([(format!("parallel-{index}"), progress(index, 10))]),
                    )
                    .unwrap();
                })
            })
            .collect();
        for writer in writers {
            writer.join().unwrap();
        }
        assert_eq!(load_state().sessions.len(), 10);
        save_state(&MobileSyncState::default()).unwrap();
        commit_progress(generation, &stale).unwrap();
        assert!(!load_state().enabled);
        assert!(load_state().sessions.is_empty());
        save_state(&MobileSyncState {
            enabled: true,
            ..Default::default()
        })
        .unwrap();
        commit_progress(generation, &stale).unwrap();
        assert!(
            load_state().sessions.is_empty(),
            "old opt-in generation cannot restore wiped progress"
        );
    }

    #[tokio::test]
    async fn persistence_notifications_upload_under_one_second_and_retain_inflight_writes() {
        use tokio::io::{AsyncReadExt, AsyncWriteExt};
        let _lock = storage::lock_test_env();
        let home = tempfile::tempdir().unwrap();
        let listener = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
        let base = format!("http://{}", listener.local_addr().unwrap());
        let _env = TestEnv::set(&[
            ("JCODE_HOME", home.path().as_os_str()),
            ("JCODE_API_KEY", std::ffi::OsStr::new("test-key")),
            ("JCODE_API_BASE", std::ffi::OsStr::new(&base)),
        ]);
        let (requests_tx, mut requests) = tokio::sync::mpsc::channel(8);
        let server = tokio::spawn(async move {
            loop {
                let (mut socket, _) = listener.accept().await.unwrap();
                let mut bytes = Vec::new();
                let mut buffer = [0; 4096];
                let body_start = loop {
                    let count = socket.read(&mut buffer).await.unwrap();
                    assert_ne!(count, 0);
                    bytes.extend_from_slice(&buffer[..count]);
                    if let Some(index) = bytes.windows(4).position(|w| w == b"\r\n\r\n") {
                        break index + 4;
                    }
                };
                let header = String::from_utf8_lossy(&bytes[..body_start]);
                let length: usize = header
                    .lines()
                    .find_map(|line| {
                        line.to_ascii_lowercase()
                            .strip_prefix("content-length:")
                            .map(|s| s.trim().parse().unwrap())
                    })
                    .unwrap();
                while bytes.len() < body_start + length {
                    let count = socket.read(&mut buffer).await.unwrap();
                    assert_ne!(count, 0);
                    bytes.extend_from_slice(&buffer[..count]);
                }
                let body: serde_json::Value =
                    serde_json::from_slice(&bytes[body_start..body_start + length]).unwrap();
                let (release, wait) = tokio::sync::oneshot::channel::<()>();
                requests_tx.send((body, release)).await.unwrap();
                wait.await.unwrap();
                socket
                    .write_all(
                        b"HTTP/1.1 200 OK\r\nContent-Length: 2\r\nConnection: close\r\n\r\n{}",
                    )
                    .await
                    .unwrap();
            }
        });
        let background = tokio::spawn(background_sync());
        // Consume the initial reconciliation while disabled.
        tokio::time::sleep(Duration::from_millis(30)).await;
        let mut session = Session::create_with_id("event-sync".into(), None, None);
        session.add_message(
            Role::User,
            vec![ContentBlock::Text {
                text: "first".into(),
                cache_control: None,
            }],
        );
        session.save().unwrap();
        assert!(!is_enabled());
        assert!(
            tokio::time::timeout(Duration::from_millis(220), requests.recv())
                .await
                .is_err()
        );
        save_state(&MobileSyncState {
            enabled: true,
            ..Default::default()
        })
        .unwrap();
        let started = std::time::Instant::now();
        session.add_message(
            Role::Assistant,
            vec![ContentBlock::Text {
                text: "second".into(),
                cache_control: None,
            }],
        );
        session.save().unwrap();
        for title in ["coalesce one", "coalesce two", "coalesce final"] {
            session.custom_title = Some(title.into());
            session.save().unwrap();
        }
        let (first, release) = tokio::time::timeout(Duration::from_secs(1), requests.recv())
            .await
            .unwrap()
            .unwrap();
        eprintln!(
            "mobile persistence-to-upload notification: {}ms",
            started.elapsed().as_millis()
        );
        assert!(started.elapsed() < Duration::from_secs(1));
        assert_eq!(first["messages"].as_array().unwrap().len(), 2);
        assert_eq!(first["title"], "coalesce final");
        // Persist while the first upload is still blocked in the network.
        session.add_message(
            Role::User,
            vec![ContentBlock::Text {
                text: "during-upload".into(),
                cache_control: None,
            }],
        );
        session.save().unwrap();
        release.send(()).unwrap();
        let (second, release) = tokio::time::timeout(Duration::from_secs(1), requests.recv())
            .await
            .unwrap()
            .unwrap();
        assert_eq!(second["messages"].as_array().unwrap().len(), 1);
        assert_eq!(second["messages"][0]["seq"], 2);
        assert_eq!(second["messages"][0]["text"], "during-upload");
        release.send(()).unwrap();
        // Progress persistence itself must not schedule another upload.
        assert!(
            tokio::time::timeout(Duration::from_millis(350), requests.recv())
                .await
                .is_err()
        );
        assert_eq!(load_state().sessions["event-sync"].uploaded, 3);
        // Simulate a writer in another process: no notification hook in this
        // process, so only the reconciliation path can discover the update.
        session.add_message(
            Role::User,
            vec![ContentBlock::Text {
                text: "external writer".into(),
                cache_control: None,
            }],
        );
        let path = home.path().join("sessions/event-sync.json");
        storage::write_json_fast(&path, &session).unwrap();
        session = Session::load_from_path(&path).unwrap();
        let reconcile = tokio::spawn(async { sync_once(&reqwest::Client::new()).await.unwrap() });
        let (external, release) = tokio::time::timeout(Duration::from_secs(1), requests.recv())
            .await
            .unwrap()
            .unwrap();
        assert_eq!(external["messages"].as_array().unwrap().len(), 1);
        assert_eq!(external["messages"][0]["text"], "external writer");
        release.send(()).unwrap();
        assert_eq!(reconcile.await.unwrap().messages_uploaded, 1);
        // Old snapshots have no local dirty hints, as with another process's
        // backlog. Block the first old upload, then durably save a fresh session.
        for id in ["old-a", "old-b", "old-c"] {
            let mut old = Session::create_with_id(id.into(), None, None);
            old.custom_title = Some(id.into());
            old.messages = vec![msg(
                Role::User,
                vec![ContentBlock::Text {
                    text: id.into(),
                    cache_control: None,
                }],
            )];
            storage::write_json_fast(&home.path().join(format!("sessions/{id}.json")), &old)
                .unwrap();
        }
        let reconcile = tokio::spawn(async { sync_once(&reqwest::Client::new()).await.unwrap() });
        let (old, release) = tokio::time::timeout(Duration::from_secs(1), requests.recv())
            .await
            .unwrap()
            .unwrap();
        let old_id = old["title"].as_str().unwrap().to_owned();
        assert!(old_id.starts_with("old-"));
        let mut fresh = Session::create_with_id("fresh-save".into(), None, None);
        fresh.custom_title = Some("fresh-save".into());
        fresh.messages = vec![msg(
            Role::User,
            vec![ContentBlock::Text {
                text: "fresh".into(),
                cache_control: None,
            }],
        )];
        fresh.save().unwrap();
        release.send(()).unwrap();
        let (next, release) = tokio::time::timeout(Duration::from_secs(1), requests.recv())
            .await
            .unwrap()
            .unwrap();
        assert_eq!(
            next["title"], "fresh-save",
            "fresh save must bypass old backlog"
        );
        assert_eq!(reconcile.await.unwrap().sessions_uploaded, 1);
        assert_eq!(load_state().sessions[&old_id].uploaded, 1);
        release.send(()).unwrap();
        tokio::time::sleep(Duration::from_millis(220)).await;
        // The next periodic-style pass resumes the unvisited backlog, without
        // uploading the completed old session again.
        let resume = tokio::spawn(async { sync_once(&reqwest::Client::new()).await.unwrap() });
        for _ in 0..2 {
            let (remaining, release) =
                tokio::time::timeout(Duration::from_secs(1), requests.recv())
                    .await
                    .unwrap()
                    .unwrap();
            assert!(remaining["title"].as_str().unwrap().starts_with("old-"));
            assert_ne!(remaining["title"], old_id);
            release.send(()).unwrap();
        }
        assert_eq!(resume.await.unwrap().sessions_uploaded, 2);
        session.custom_title = Some("metadata only".into());
        session.save().unwrap();
        let (metadata, release) = tokio::time::timeout(Duration::from_secs(1), requests.recv())
            .await
            .unwrap()
            .unwrap();
        assert_eq!(metadata["title"], "metadata only");
        assert!(metadata["messages"].as_array().unwrap().is_empty());
        // A disable racing an upload must not be undone by its completion.
        save_state(&MobileSyncState::default()).unwrap();
        release.send(()).unwrap();
        tokio::time::sleep(Duration::from_millis(220)).await;
        assert!(!load_state().enabled);
        assert!(load_state().sessions.is_empty());
        session.add_message(
            Role::Assistant,
            vec![ContentBlock::Text {
                text: "disabled".into(),
                cache_control: None,
            }],
        );
        session.save().unwrap();
        assert!(
            tokio::time::timeout(Duration::from_millis(250), requests.recv())
                .await
                .is_err()
        );
        background.abort();
        server.abort();
        let _ = background.await;
        let _ = server.await;
    }

    #[test]
    fn reconciliation_does_not_starve_old_candidates_or_miss_same_millisecond_writes() {
        let _lock = storage::lock_test_env();
        let home = tempfile::tempdir().unwrap();
        let _env = TestEnv::set(&[("JCODE_HOME", home.path().as_os_str())]);
        let sessions = home.path().join("sessions");
        std::fs::create_dir_all(&sessions).unwrap();
        for index in 0..205 {
            std::fs::write(sessions.join(format!("session-{index}.json")), b"{}").unwrap();
        }
        assert_eq!(candidate_sessions(0).unwrap().len(), 205);
        let path = sessions.join("session-0.json");
        let file = std::fs::OpenOptions::new().write(true).open(&path).unwrap();
        let original = file.metadata().unwrap().modified().unwrap();
        let before = session_revision(&path);
        let before_ms = session_mtime_ms(&path);
        std::fs::write(&path, b"{\"changed\":true}").unwrap();
        file.set_times(std::fs::FileTimes::new().set_modified(original))
            .unwrap();
        assert_eq!(session_mtime_ms(&path), before_ms);
        assert_ne!(session_revision(&path), before);
        let journal = crate::session::session_journal_path_from_snapshot(&path);
        std::fs::write(&journal, b"journal update\n").unwrap();
        let with_journal = session_revision(&path);
        std::fs::remove_file(journal).unwrap();
        assert_ne!(session_revision(&path), with_journal);
        assert_eq!(SYNC_INTERVAL, Duration::from_secs(20));
    }

    async fn mock_listing(status: &str, body: &str) -> (String, tokio::task::JoinHandle<String>) {
        use tokio::io::{AsyncReadExt, AsyncWriteExt};

        let listener = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
        let base = format!("http://{}/", listener.local_addr().unwrap());
        let response = format!(
            "HTTP/1.1 {status}\r\nContent-Type: application/json\r\nContent-Length: {}\r\nConnection: close\r\n\r\n{body}",
            body.len()
        );
        let task = tokio::spawn(async move {
            let (mut socket, _) = listener.accept().await.unwrap();
            let mut request = Vec::new();
            let mut buffer = [0; 1024];
            while !request.windows(4).any(|window| window == b"\r\n\r\n") {
                let count = socket.read(&mut buffer).await.unwrap();
                assert_ne!(count, 0);
                request.extend_from_slice(&buffer[..count]);
            }
            socket.write_all(response.as_bytes()).await.unwrap();
            String::from_utf8(request).unwrap()
        });
        (base, task)
    }

    #[tokio::test]
    async fn cloud_session_listing_filters_and_preserves_metadata() {
        let body = serde_json::json!({"sessions": [
            {"session_id": "cloud-1", "source": "cloud", "title": "Title",
             "host": "worker", "cwd": "/repo", "updated_at": "updated", "created_at": "created"},
            {"session_id": "cloud-2", "source": "cloud"},
            {"session_id": "local-1", "source": "local"},
            {"session_id": "../unsafe", "source": "cloud"},
            {"session_id": "", "source": "cloud"},
            {"session_id": "x".repeat(129), "source": "cloud"}
        ]})
        .to_string();
        let (base, request) = mock_listing("200 OK", &body).await;
        let sessions =
            list_cloud_sessions_with_endpoint(&reqwest::Client::new(), &base, "test-key")
                .await
                .unwrap();
        assert_eq!(sessions.len(), 2);
        assert_eq!(sessions[0].session_id, "cloud-1");
        assert_eq!(sessions[0].title.as_deref(), Some("Title"));
        assert_eq!(sessions[0].host.as_deref(), Some("worker"));
        assert_eq!(sessions[0].cwd.as_deref(), Some("/repo"));
        assert_eq!(sessions[0].updated_at.as_deref(), Some("updated"));
        assert_eq!(sessions[0].created_at.as_deref(), Some("created"));
        assert_eq!(sessions[1].title, None);
        assert_eq!(sessions[1].created_at, None);
        let request = request.await.unwrap().to_ascii_lowercase();
        assert!(request.starts_with("get /mobile/sessions?limit=500 http/1.1\r\n"));
        assert!(request.contains("\r\nauthorization: bearer test-key\r\n"));
    }

    #[tokio::test]
    async fn cloud_session_listing_rejects_http_and_schema_errors() {
        for (status, body) in [
            ("401 Unauthorized", "{\"sessions\":[]}"),
            ("200 OK", "not json"),
            ("200 OK", "{}"),
            ("200 OK", "{\"sessions\":[{\"source\":\"cloud\"}]}"),
            ("200 OK", "{\"sessions\":[{\"session_id\":\"s1\"}]}"),
        ] {
            let (base, request) = mock_listing(status, body).await;
            assert!(
                list_cloud_sessions_with_endpoint(&reqwest::Client::new(), &base, "key")
                    .await
                    .is_err()
            );
            request.await.unwrap();
        }
        let (base, request) = mock_listing("200 OK", "{\"sessions\":[]}").await;
        assert!(
            list_cloud_sessions_with_endpoint(&reqwest::Client::new(), &base, "key")
                .await
                .unwrap()
                .is_empty()
        );
        request.await.unwrap();
    }

    fn msg(role: Role, content: Vec<ContentBlock>) -> StoredMessage {
        StoredMessage {
            id: "m".into(),
            role,
            content,
            display_role: None,
            timestamp: None,
            tool_duration_ms: None,
            token_usage: None,
        }
    }

    #[test]
    fn wire_messages_keep_visible_text_and_skip_hidden() {
        let mut session = Session::create_with_id("s1".into(), None, None);
        session.messages = vec![
            msg(
                Role::User,
                vec![ContentBlock::Text {
                    text: "hi".into(),
                    cache_control: None,
                }],
            ),
            msg(
                Role::Assistant,
                vec![ContentBlock::Reasoning {
                    text: "secret".into(),
                }],
            ),
            msg(
                Role::Assistant,
                vec![
                    ContentBlock::Text {
                        text: "running".into(),
                        cache_control: None,
                    },
                    ContentBlock::ToolUse {
                        id: "t".into(),
                        name: "bash".into(),
                        input: serde_json::json!({"command": "ls"}),
                        thought_signature: None,
                    },
                ],
            ),
            msg(
                Role::User,
                vec![ContentBlock::ToolResult {
                    tool_use_id: "t".into(),
                    content: "x".repeat(MAX_TOOL_RESULT_CHARS + 10),
                    is_error: None,
                }],
            ),
        ];
        let wire = wire_messages(&session, 0);
        assert_eq!(
            wire.iter().map(|m| m.seq).collect::<Vec<_>>(),
            vec![0, 2, 3]
        );
        assert_eq!(wire[0].role, "user");
        assert_eq!(wire[1].text, "running\n\n🔧 bash: ls");
        assert_eq!(wire[2].role, "tool");
        assert!(wire[2].text.ends_with("(truncated)"));
        assert!(!wire.iter().any(|m| m.text.contains("secret")));
        assert_eq!(wire_messages(&session, 3).len(), 1);
    }

    #[test]
    fn session_ids_are_validated() {
        assert!(valid_session_id("session_abc-123.x"));
        assert!(!valid_session_id("bad id"));
        assert!(!valid_session_id(""));
    }

    #[test]
    fn incremental_updates_replay_rewritten_and_shrunk_prefixes() {
        let mut session = Session::create_with_id("rewrite".into(), None, None);
        for text in ["first", "second"] {
            session.add_message(
                Role::User,
                vec![ContentBlock::Text {
                    text: text.into(),
                    cache_control: None,
                }],
            );
        }
        let (initial, fingerprint) = incremental_messages(&session, &SessionProgress::default());
        assert_eq!(initial.len(), 2);
        let progress = SessionProgress {
            uploaded: 2,
            fingerprint: Some(fingerprint),
            ..Default::default()
        };
        assert!(incremental_messages(&session, &progress).0.is_empty());
        session.add_message(
            Role::Assistant,
            vec![ContentBlock::Text {
                text: "appended".into(),
                cache_control: None,
            }],
        );
        let appended = incremental_messages(&session, &progress).0;
        assert_eq!(appended.len(), 1);
        assert_eq!(appended[0].seq, 2);
        session.messages[0].content = vec![ContentBlock::Text {
            text: "rewritten".into(),
            cache_control: None,
        }];
        let rewritten = incremental_messages(&session, &progress).0;
        assert_eq!(rewritten.len(), 3);
        assert_eq!(rewritten[0].text, "rewritten");
        session.truncate_messages(1);
        assert_eq!(incremental_messages(&session, &progress).0.len(), 1);
    }
}
