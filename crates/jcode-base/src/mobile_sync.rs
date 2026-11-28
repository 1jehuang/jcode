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
//! one-line summary, and tool results truncated. Assistant markdown images in
//! the generated-image store and visible image attachments become bounded
//! thumbnails. Hidden reasoning, arbitrary local files and provider-native
//! blobs are never sent.

use crate::session::Session;
use crate::storage;
use crate::subscription_api;
use anyhow::{Context, Result};
use jcode_message_types::{ContentBlock, Role};
use serde::{Deserialize, Serialize};
use std::collections::{BTreeMap, BTreeSet};
use std::hash::{Hash, Hasher};
use std::path::{Path, PathBuf};
use std::sync::{Arc, Mutex};
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
const MAX_STRUCTURED_TOOLS: usize = 32;
const MAX_TOOL_INPUT_BYTES: usize = 8 * 1024;
const MAX_TOOL_ID_BYTES: usize = 256;
const MAX_STRUCTURED_BYTES: usize = 240 * 1024;
// Bump when existing cloud rows need replaying, even if snapshot bytes and
// visible-message fingerprints are unchanged. Missing versions deserialize as 0.
const WIRE_VERSION: u32 = 2;
/// Only sessions touched within this window are synced on enable.
const BACKFILL_WINDOW: Duration = Duration::from_secs(14 * 24 * 3600);
const SYNC_DEBOUNCE: Duration = Duration::from_millis(150);
// Also bounded when no background worker runs or mobile sync is disabled.
// Evicted hints are recovered by the periodic snapshot/journal reconciliation.
const MAX_DIRTY_SESSIONS: usize = 1024;
static PERSISTED: tokio::sync::Notify = tokio::sync::Notify::const_new();
static DIRTY_SESSIONS: Mutex<BTreeSet<String>> = Mutex::new(BTreeSet::new());
static SYNC_PASS: tokio::sync::Mutex<()> = tokio::sync::Mutex::const_new(());
static FAILURE_BACKOFF: Mutex<Option<FailureBackoff>> = Mutex::new(None);

// Process-local only: restarting, changing account/endpoint/home, or toggling
// sync starts fresh. Failed uploads never advance durable successful progress.
#[derive(Default)]
struct FailureBackoff {
    scope: (PathBuf, u64, String, u64),
    sessions: BTreeMap<String, FailedSession>,
}

struct FailedSession {
    revision: [u64; 4],
    attempts: u32,
    retry_at: Instant,
}

impl FailureBackoff {
    fn deferred(&mut self, id: &str, revision: [u64; 4], fresh: bool, now: Instant) -> bool {
        if fresh
            || self
                .sessions
                .get(id)
                .is_some_and(|failure| failure.revision != revision)
        {
            self.sessions.remove(id);
        }
        self.sessions
            .get(id)
            .is_some_and(|failure| now < failure.retry_at)
    }

    fn failed(&mut self, id: &str, revision: [u64; 4], now: Instant) {
        let attempts = self
            .sessions
            .get(id)
            .map_or(1, |f| f.attempts.saturating_add(1));
        if self.sessions.len() >= MAX_DIRTY_SESSIONS && !self.sessions.contains_key(id) {
            self.sessions.pop_first();
        }
        let seconds = 20u64
            .saturating_mul(1u64 << attempts.saturating_sub(1).min(4))
            .min(300);
        self.sessions.insert(
            id.to_owned(),
            FailedSession {
                revision,
                attempts,
                retry_at: now + Duration::from_secs(seconds),
            },
        );
    }
}

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
    /// The user turned off messages from the phone to sessions running here
    /// (`jcode mobile messages off`). Transcript sync is unaffected.
    #[serde(default, skip_serializing_if = "std::ops::Not::not")]
    pub phone_messages_off: bool,
    /// session id -> progress
    #[serde(default)]
    pub sessions: BTreeMap<String, SessionProgress>,
}

#[derive(Debug, Clone, Default, Serialize, Deserialize, PartialEq)]
pub struct SessionProgress {
    /// Wire schema successfully uploaded for this snapshot. Independent of file
    /// revisions so representation upgrades cannot take the unchanged fast path.
    #[serde(default)]
    pub wire_version: u32,
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
    pub display_text: Option<String>,
    #[serde(skip_serializing_if = "Vec::is_empty")]
    pub tool_calls: Vec<WireToolCall>,
    #[serde(skip_serializing_if = "Vec::is_empty")]
    pub tool_results: Vec<WireToolResult>,
    #[serde(skip_serializing_if = "Vec::is_empty")]
    pub images: Vec<WireImage>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub created_at: Option<String>,
}

#[derive(Debug, Clone, Serialize, PartialEq, Hash)]
pub struct WireToolCall {
    pub id: String,
    pub name: String,
    pub input: serde_json::Value,
}

#[derive(Debug, Clone, Serialize, PartialEq, Hash)]
pub struct WireToolResult {
    pub tool_use_id: String,
    pub content: String,
    pub is_error: bool,
}

#[derive(Debug, Clone, Serialize, PartialEq, Hash)]
pub struct WireImage {
    pub media_type: String,
    pub data: String,
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
    (*load_state_snapshot()).clone()
}

// One path and at most 1 MiB of source text are retained, never metadata-only
// authorization. Every caller reads the current file before comparing bytes.
const MAX_CACHED_STATE_BYTES: usize = 1024 * 1024;
struct ParsedState {
    path: PathBuf,
    contents: String,
    state: Arc<MobileSyncState>,
}
static PARSED_STATE: Mutex<Option<ParsedState>> = Mutex::new(None);

fn load_state_snapshot() -> Arc<MobileSyncState> {
    let state = (|| -> Result<Arc<MobileSyncState>> {
        let path = state_path()?;
        if !path.exists() {
            return Ok(Arc::new(MobileSyncState::default()));
        }
        let contents = std::fs::read_to_string(&path)?;
        {
            let cached = PARSED_STATE.lock().unwrap_or_else(|e| e.into_inner());
            if let Some(cached) = cached.as_ref()
                && cached.path == path
                && cached.contents == contents
            {
                return Ok(Arc::clone(&cached.state));
            }
        }
        // Invalid schemas must take the original recovery/logging path. Do not
        // cache failures or a recovered backup under the invalid primary bytes.
        let parsed = match serde_json::from_str::<MobileSyncState>(&contents) {
            Ok(parsed) => Arc::new(parsed),
            Err(_) => return storage::read_json(&path).map(Arc::new),
        };
        let mut cached = PARSED_STATE.lock().unwrap_or_else(|e| e.into_inner());
        *cached = (contents.len() <= MAX_CACHED_STATE_BYTES).then(|| ParsedState {
            path,
            contents,
            state: Arc::clone(&parsed),
        });
        Ok(parsed)
    })();
    match state {
        Ok(state) => state,
        Err(error) => {
            crate::logging::warn(&format!("Could not load mobile sync state: {error:#}"));
            Arc::new(MobileSyncState::default())
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

/// Allow or refuse messages from the phone to sessions running here.
/// `save_state` never changes this, so a sync pass working from an older read
/// cannot undo the user's choice.
pub fn set_phone_messages_off(off: bool) -> Result<MobileSyncState> {
    let _lock = state_lock()?;
    let mut next = load_state();
    next.phone_messages_off = off;
    storage::write_json_secret(&state_path()?, &next)?;
    Ok(next)
}

fn merge_progress(
    state: &mut MobileSyncState,
    updates: &BTreeMap<String, SessionProgress>,
) -> bool {
    let mut changed = false;
    for (id, progress) in updates {
        if progress::replaces(state.sessions.get(id), progress) {
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
    let current = load_state_snapshot();
    if let Some(commit) = progress::ProgressCommit::new(&current, generation, updates) {
        storage::write_json_secret(&state_path()?, &commit)?;
    }
    Ok(())
}

pub fn is_enabled() -> bool {
    load_state_snapshot().enabled
}

fn sync_still_enabled(generation: u64) -> bool {
    let state = load_state_snapshot();
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

pub(crate) fn host_name() -> Option<String> {
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
    let mut out = BoundedText::new(max);
    out.push(text);
    out.text
}

/// Assemble the exact prefix of joined blocks without visiting or copying their
/// discarded tails. Keep checking block kinds separately for role selection.
struct BoundedText {
    text: String,
    remaining: usize,
    truncated: bool,
}

impl BoundedText {
    fn new(max: usize) -> Self {
        Self {
            text: String::new(),
            remaining: max,
            truncated: false,
        }
    }

    fn push(&mut self, text: &str) {
        if self.truncated {
            return;
        }
        // Byte length is an inexpensive upper bound on the scalar count. Most
        // small blocks fit immediately, without a scalar-by-scalar walk.
        if text.len() <= self.remaining {
            self.remaining -= text.chars().count();
            self.text.push_str(text);
            return;
        }
        // Count a bounded byte prefix with str's optimized scalar counter,
        // then inspect only the few additional scalars needed for Unicode.
        let mut prefix_end = self.remaining;
        while !text.is_char_boundary(prefix_end) {
            prefix_end -= 1;
        }
        let prefix_chars = text[..prefix_end].chars().count();
        let mut chars = text[prefix_end..].char_indices();
        if let Some((end, _)) = chars.nth(self.remaining - prefix_chars) {
            self.text.push_str(&text[..prefix_end + end]);
            self.text.push_str("\n… (truncated)");
            self.remaining = 0;
            self.truncated = true;
        } else {
            self.remaining -= text.chars().count();
            self.text.push_str(text);
        }
    }
}

fn append_tool_summary(out: &mut BoundedText, name: &str, input: &serde_json::Value) {
    let detail = input
        .get("intent")
        .or_else(|| input.get("command"))
        .or_else(|| input.get("file_path"))
        .or_else(|| input.get("query"))
        .and_then(|v| v.as_str())
        .unwrap_or("");
    out.push("🔧 ");
    out.push(name);
    if !detail.is_empty() {
        out.push(": ");
        if !out.truncated {
            out.push(&truncate(detail, 200));
        }
    }
}

/// Bound serialization work as well as the retained input. Large inputs become
/// a valid JSON string preview, never an invalid/truncated JSON object.
fn bounded_tool_input(input: &serde_json::Value) -> serde_json::Value {
    struct Preview(Vec<u8>);
    impl std::io::Write for Preview {
        fn write(&mut self, bytes: &[u8]) -> std::io::Result<usize> {
            let available = MAX_TOOL_INPUT_BYTES.saturating_sub(self.0.len());
            self.0
                .extend_from_slice(&bytes[..bytes.len().min(available)]);
            if bytes.len() > available {
                Err(std::io::Error::other("tool input preview full"))
            } else {
                Ok(bytes.len())
            }
        }
        fn flush(&mut self) -> std::io::Result<()> {
            Ok(())
        }
    }
    let mut preview = Preview(Vec::new());
    if serde_json::to_writer(&mut preview, input).is_ok() {
        match serde_json::from_slice(&preview.0) {
            Ok(messages) => messages,
            Err(error) => {
                crate::logging::debug(&format!("Cannot decode mobile preview cache: {error}"));
                serde_json::Value::Null
            }
        }
    } else {
        serde_json::Value::String(format!(
            "{}\n… (truncated)",
            String::from_utf8_lossy(&preview.0)
        ))
    }
}

// Use the same serializer as the wire representation without allocating and
// copying a JSON buffer that would immediately be discarded.
fn serialized_byte_len(value: &impl Serialize) -> serde_json::Result<usize> {
    struct Counter(usize);
    impl std::io::Write for Counter {
        fn write(&mut self, bytes: &[u8]) -> std::io::Result<usize> {
            self.0 += bytes.len();
            Ok(bytes.len())
        }

        fn write_all(&mut self, bytes: &[u8]) -> std::io::Result<()> {
            // This sink always consumes a complete slice and cannot return
            // Interrupted or WriteZero. Avoid the generic partial-write loop.
            self.0 += bytes.len();
            Ok(())
        }

        fn flush(&mut self) -> std::io::Result<()> {
            Ok(())
        }
    }
    let mut counter = Counter(0);
    serde_json::to_writer(&mut counter, value)?;
    Ok(counter.0)
}

#[cfg(test)]
#[path = "mobile_sync_byte_count_tests.rs"]
mod byte_count_tests;

#[cfg(test)]
#[path = "mobile_sync_direct_count_tests.rs"]
mod direct_count_tests;

fn structured_tools(
    blocks: &[ContentBlock],
) -> Option<(String, Vec<WireToolCall>, Vec<WireToolResult>)> {
    if !blocks.iter().any(|block| {
        matches!(
            block,
            ContentBlock::ToolUse { .. } | ContentBlock::ToolResult { .. }
        )
    }) {
        return None;
    }
    let mut prose = BoundedText::new(MAX_TEXT_CHARS);
    let mut has_prose = false;
    let mut calls = Vec::new();
    let mut results = Vec::new();
    for block in blocks {
        match block {
            ContentBlock::Text { text, .. }
                if !text.trim_start().is_empty()
                    && !text.trim_start().starts_with("<system-reminder>") =>
            {
                if has_prose {
                    prose.push("\n\n");
                }
                prose.push(text);
                has_prose = true;
            }
            ContentBlock::ToolUse {
                id, name, input, ..
            } => {
                // Fall back to the existing summaries as a whole if identities
                // cannot be represented safely. Never truncate IDs into aliases.
                if calls.len() == MAX_STRUCTURED_TOOLS
                    || id.is_empty()
                    || id.len() > MAX_TOOL_ID_BYTES
                    || name.is_empty()
                {
                    return None;
                }
                calls.push(WireToolCall {
                    id: id.clone(),
                    name: name.chars().take(128).collect(),
                    input: bounded_tool_input(input),
                });
            }
            ContentBlock::ToolResult {
                tool_use_id,
                content,
                is_error,
            } => {
                if results.len() == MAX_STRUCTURED_TOOLS
                    || tool_use_id.is_empty()
                    || tool_use_id.len() > MAX_TOOL_ID_BYTES
                {
                    return None;
                }
                results.push(WireToolResult {
                    tool_use_id: tool_use_id.clone(),
                    content: truncate(content, MAX_TOOL_RESULT_CHARS),
                    is_error: is_error.unwrap_or(false),
                });
            }
            // Reasoning, signatures, images and hidden reminders are not wire prose.
            _ => {}
        }
    }
    if calls.is_empty() && results.is_empty() {
        return None;
    }
    // Backend limits are UTF-16 code units for prose and 256 KiB serialized
    // metadata. Reserve room for the JSON property names/envelope.
    if prose.text.encode_utf16().count() > 200_000 {
        return None;
    }
    let structured_bytes = match serialized_byte_len(&(&calls, &results)) {
        Ok(bytes) => bytes,
        Err(error) => {
            crate::logging::debug(&format!("Cannot serialize mobile tool metadata: {error}"));
            return None;
        }
    };
    if structured_bytes > MAX_STRUCTURED_BYTES {
        return None;
    }
    Some((prose.text, calls, results))
}
#[path = "mobile_sync/wire_payload.rs"]
mod wire_payload;
pub use wire_payload::wire_messages;
#[cfg(test)]
use wire_payload::{attached_images, generated_images};
use wire_payload::{image_thumbnail, message_chunks};
fn wire_fingerprint(messages: &[WireMessage], end: usize) -> u64 {
    let mut hash = std::collections::hash_map::DefaultHasher::new();
    for message in messages.iter().take_while(|message| message.seq < end) {
        message.hash(&mut hash);
    }
    hash.finish()
}

#[path = "mobile_sync_incremental_cache.rs"]
mod incremental_cache;

fn incremental_messages(session: &Session, progress: &SessionProgress) -> (Vec<WireMessage>, u64) {
    incremental_cache::messages(session, progress)
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
    // Version the visible wire representation without changing mtime ordering.
    // Epoch 1 (bit 63) added generated images. Epoch 2 (bits 63+62) also
    // reconciles unchanged stored sessions with structured tool metadata.
    [
        snapshot_time,
        snapshot_len ^ (3_u64 << 62),
        journal_time,
        journal_len,
    ]
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

// Fixed labels distinguish actionable failures without logging response bodies,
// account identity, endpoint URLs, or credentials.
fn upload_http_outcome(status: reqwest::StatusCode) -> &'static str {
    match status.as_u16() {
        401 => "unauthorized",
        403 => "forbidden",
        404 => "route_missing",
        413 => "payload_too_large",
        429 => "rate_limited",
        500..=599 => "server_error",
        _ => "http_error",
    }
}

#[cfg(test)]
#[path = "mobile_sync_diagnostics_tests.rs"]
mod diagnostics_tests;

async fn upload(
    client: &reqwest::Client,
    api_base: &str,
    api_key: &str,
    session_id: &str,
    body: &UploadBody<'_>,
) -> Result<()> {
    let mut timing = SyncTiming::new("http_upload");
    timing.outcome = "transport_error";
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
        .inspect_err(|error| {
            timing.outcome = if error.is_timeout() {
                "timeout"
            } else if error.is_connect() {
                "connect_error"
            } else {
                "transport_error"
            };
        })
        .context("mobile sync upload failed")?;
    let status = response.status();
    if !status.is_success() {
        timing.outcome = upload_http_outcome(status);
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

pub(crate) fn valid_session_id(id: &str) -> bool {
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
    // Keep the parsed inventory shared across unchanged passes and fences.
    let state = load_state_snapshot();
    if !state.enabled {
        timing.outcome = "skipped_disabled";
        return Ok(SyncReport::default());
    }
    let api_key = crate::subscription_catalog::configured_api_key().with_context(|| {
        timing.outcome = "missing_account";
        "mobile sync needs a Jcode account: run `jcode account login`"
    })?;
    let api_base = subscription_api::configured_api_base();
    let mut credential_hash = std::collections::hash_map::DefaultHasher::new();
    api_key.hash(&mut credential_hash);
    let scope = (
        storage::jcode_dir()?,
        state.generation,
        api_base.clone(),
        credential_hash.finish(),
    );
    let mut failures = FAILURE_BACKOFF
        .lock()
        .unwrap_or_else(|e| e.into_inner())
        .take()
        .filter(|cache| cache.scope == scope)
        .unwrap_or_else(|| FailureBackoff {
            scope,
            ..Default::default()
        });
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
    let mut pending = dirty.clone();
    if reconcile {
        pending.extend(failures.sessions.keys().cloned());
    }
    for id in &pending {
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
        if failures.deferred(&id, revision, dirty.contains(&id), Instant::now()) {
            unchanged_count += 1;
            continue;
        }
        if progress.wire_version == WIRE_VERSION
            && progress.revision == Some(revision)
            && !dirty.contains(&id)
        {
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
                failures.failed(&id, revision, Instant::now());
                continue;
            }
        };
        if session.is_debug || session.messages.is_empty() {
            failures.sessions.remove(&id);
            upload_timing.outcome = "skipped_empty_or_debug";
            updates.insert(
                id,
                SessionProgress {
                    wire_version: WIRE_VERSION,
                    uploaded: progress.uploaded,
                    mtime_ms: mtime,
                    revision: Some(revision),
                    fingerprint: progress.fingerprint,
                },
            );
            continue;
        }
        let (messages, fingerprint) = incremental_messages(&session, &progress);
        let todo_title = crate::todo::load_session_title(&session.id);
        let title = synced_session_title(&session, todo_title.as_deref());
        let mut ok = true;
        // Always send at least one request so metadata (title, updated_at) refreshes.
        let chunks: Vec<&[WireMessage]> = if messages.is_empty() {
            vec![&[]]
        } else {
            message_chunks(&messages)
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
            failures.sessions.remove(&id);
            upload_timing.outcome = "success";
            report.sessions_uploaded += 1;
            updates.insert(
                id,
                SessionProgress {
                    wire_version: WIRE_VERSION,
                    uploaded: session.messages.len(),
                    mtime_ms: mtime,
                    revision: Some(revision),
                    fingerprint: Some(fingerprint),
                },
            );
        } else if upload_timing.outcome == "error" {
            failures.failed(&id, revision, Instant::now());
        } else {
            // A generation fence is not an upload failure. Preserve fresh work.
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
    *FAILURE_BACKOFF.lock().unwrap_or_else(|e| e.into_inner()) = Some(failures);
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
/// Match `RecentSessionMetadata::display_title` precedence:
/// custom_title -> saved save_label -> todo plan title -> generated/prompt title.
fn synced_session_title<'a>(session: &'a Session, todo_title: Option<&'a str>) -> Option<&'a str> {
    let non_empty = |value: &'a str| {
        let trimmed = value.trim();
        (!trimmed.is_empty()).then_some(trimmed)
    };
    session
        .custom_title
        .as_deref()
        .and_then(non_empty)
        .or_else(|| {
            session
                .saved
                .then(|| session.save_label.as_deref().and_then(non_empty))
                .flatten()
        })
        .or_else(|| todo_title.and_then(non_empty))
        .or_else(|| session.title.as_deref().and_then(non_empty))
}

pub fn spawn_background_sync() {
    tokio::spawn(background_sync());
}

/// The two daemon mobile loops use exactly reqwest's default configuration.
/// Share their TLS configuration and connection pool, not request credentials.
/// Initialization stays eager at loop startup even when mobile sync is disabled.
/// Do not substitute the provider client: its defaults are intentionally different.
pub(crate) fn daemon_http_client() -> reqwest::Client {
    static CLIENT: std::sync::OnceLock<reqwest::Client> = std::sync::OnceLock::new();
    CLIENT.get_or_init(reqwest::Client::new).clone()
}

#[cfg(test)]
#[path = "mobile_http_tests.rs"]
mod mobile_http_tests;

mod background;
use background::background_sync;

#[path = "mobile_sync_progress.rs"]
mod progress;

#[path = "mobile_sync_thumbnail_cache.rs"]
mod thumbnail_cache;

#[cfg(test)]
#[path = "mobile_sync_backoff_tests.rs"]
mod backoff_tests;
#[cfg(test)]
#[path = "mobile_sync_http_tests.rs"]
mod http_tests;
#[cfg(test)]
#[path = "mobile_sync_tests.rs"]
mod tests;
