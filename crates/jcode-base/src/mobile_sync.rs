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
use std::time::{Duration, Instant, SystemTime, UNIX_EPOCH};

// Only compile-time labels enter timing records. Never include session IDs,
// endpoints, credentials, payloads, or error strings here. Drop also covers
// early returns, propagated errors and cancellation without changing budgets.
struct SyncTiming {
    stage: &'static str,
    outcome: &'static str,
    started: Instant,
}

impl SyncTiming {
    fn new(stage: &'static str) -> Self {
        Self {
            stage,
            outcome: "incomplete",
            started: Instant::now(),
        }
    }

    fn record(&self) -> String {
        format!(
            "mobile_sync stage={} outcome={} duration_ms={}",
            self.stage,
            self.outcome,
            self.started.elapsed().as_millis()
        )
    }
}

impl Drop for SyncTiming {
    fn drop(&mut self) {
        crate::logging::info(&self.record());
    }
}

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
    let mut timing = SyncTiming::new("http_list");
    timing.outcome = "error";
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
    timing.outcome = "success";
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

fn merge_progress(
    state: &mut MobileSyncState,
    updates: &BTreeMap<String, SessionProgress>,
) -> bool {
    let mut changed = false;
    for (id, progress) in updates {
        let version = |p: &SessionProgress| {
            p.revision
                .map(|r| r[0].max(r[2]))
                .unwrap_or(p.mtime_ms.saturating_mul(1_000_000))
        };
        if state.sessions.get(id).is_none_or(|old| {
            old != progress
                && (version(old) < version(progress)
                    || (version(old) == version(progress) && old.uploaded <= progress.uploaded))
        }) {
            state.sessions.insert(id.clone(), progress.clone());
            changed = true;
        }
    }
    changed
}

fn commit_progress(generation: u64, updates: &BTreeMap<String, SessionProgress>) -> Result<()> {
    if updates.is_empty() {
        return Ok(());
    }
    let _lock = state_lock()?;
    let mut current = load_state();
    if current.enabled && current.generation == generation && merge_progress(&mut current, updates)
    {
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
    let mut timing = SyncTiming::new("http_upload");
    timing.outcome = "error";
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
        timing.outcome = "http_error";
        // Response bodies can echo private content or credentials.
        anyhow::bail!("mobile sync upload returned {status}");
    }
    // Dropping an unread HTTP/1 body closes the pooled connection. HTTP/2
    // streams can be cancelled without sacrificing their shared connection.
    if matches!(
        response.version(),
        reqwest::Version::HTTP_10 | reqwest::Version::HTTP_11
    ) {
        drain_upload_response(response).await;
    }
    timing.outcome = "success";
    Ok(())
}

const UPLOAD_RESPONSE_DRAIN_LIMIT: usize = 64 * 1024;
const UPLOAD_RESPONSE_DRAIN_TIMEOUT: Duration = Duration::from_millis(100);

async fn drain_upload_response(mut response: reqwest::Response) -> &'static str {
    let mut timing = SyncTiming::new("http_upload_body");
    // The server has already accepted the upload. A broken, oversized or slow
    // acknowledgment must not turn success into a retry of accepted messages.
    timing.outcome = match tokio::time::timeout(UPLOAD_RESPONSE_DRAIN_TIMEOUT, async {
        let mut bytes = 0usize;
        loop {
            match response.chunk().await {
                Ok(Some(chunk)) => {
                    bytes = bytes.saturating_add(chunk.len());
                    if bytes > UPLOAD_RESPONSE_DRAIN_LIMIT {
                        return "size_limit";
                    }
                }
                Ok(None) => return "success",
                Err(_) => return "read_error",
            }
        }
    })
    .await
    {
        Ok(outcome) => outcome,
        Err(_) => "timeout",
    };
    timing.outcome
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
    let mut queue = SyncTiming::new("queue_wait");
    let _pass = SYNC_PASS.lock().await;
    queue.outcome = "acquired";
    drop(queue);
    let mut timing = SyncTiming::new(if reconcile {
        "pass_reconcile"
    } else {
        "pass_incremental"
    });
    timing.outcome = "error";
    let state = load_state();
    if !state.enabled {
        timing.outcome = "skipped_disabled";
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

    let candidate_count = candidates.len();
    let mut unchanged_count = 0;
    let mut updates = BTreeMap::new();
    let mut candidates = candidates.into_iter();
    while let Some((id, path, mtime)) = candidates.next() {
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
            unchanged_count += 1;
            continue;
        }
        // Unchanged candidates cannot upload or mutate progress. Do not reload
        // the entire state for each one. Keep uncached generation fences for
        // changed sessions and, crucially, immediately before every request.
        let mut upload_timing = SyncTiming::new("upload");
        if !sync_still_enabled(state.generation) {
            upload_timing.outcome = "skipped_disabled";
            break;
        }
        let session = match Session::load_from_path(&path) {
            Ok(s) => s,
            Err(_) => {
                upload_timing.outcome = "load_error";
                mark_dirty(&id);
                continue;
            }
        };
        if session.is_debug || session.messages.is_empty() {
            upload_timing.outcome = "skipped_empty_or_debug";
            updates.insert(
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
                upload_timing.outcome = "skipped_disabled";
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
            if upload(client, &api_base, &api_key, &id, &body)
                .await
                .is_err()
            {
                upload_timing.outcome = "error";
                report.errors += 1;
                ok = false;
                break;
            }
            report.messages_uploaded += chunk.len();
        }
        if ok {
            upload_timing.outcome = "success";
            report.sessions_uploaded += 1;
            updates.insert(
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
    let mut commit = SyncTiming::new("progress_commit");
    commit.outcome = "error";
    commit_progress(state.generation, &updates)?;
    commit.outcome = if updates.is_empty() {
        "skipped_unchanged"
    } else {
        "success"
    };
    crate::logging::info(&format!(
        "mobile_sync candidates={candidate_count} unchanged={unchanged_count} progress_updates={}",
        updates.len()
    ));
    timing.outcome = if report.errors == 0 {
        "success"
    } else {
        "partial_failure"
    };
    Ok(report)
}

/// Delete everything this account has stored for mobile.
pub async fn wipe_remote(client: &reqwest::Client) -> Result<()> {
    let mut timing = SyncTiming::new("http_wipe");
    timing.outcome = "error";
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
        timing.outcome = "http_error";
        anyhow::bail!("server returned {}", response.status());
    }
    timing.outcome = "success";
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
                let mut debounce = SyncTiming::new("debounce_wait");
                tokio::time::sleep(SYNC_DEBOUNCE).await;
                debounce.outcome = "success";
                false
            }
        };
        // sync_pass records failures and disabled skips without error text.
        if sync_pass(&client, reconcile).await.is_err() {
            crate::logging::debug("mobile sync pass failed (see stage timing)");
        }
    }
}

#[cfg(test)]
#[path = "mobile_sync_http_tests.rs"]
mod http_tests;
#[cfg(test)]
#[path = "mobile_sync_tests.rs"]
mod tests;
