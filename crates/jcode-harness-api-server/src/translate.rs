//! Pure JSON-to-JSON translation between the harness API and the legacy
//! internal protocol. Kept side-effect free so it is trivially unit-testable.

use crate::background_progress::parse_background_notification;
use jcode_harness_api::{
    ApiEvent, ErrorCode, HistoryMessage, ModelRouteInfo, ServerFrame, SessionInfo, TextMatch,
};
use rusqlite::{Connection, params};
use serde::Deserialize;
use std::collections::{BTreeMap, BTreeSet};
use std::io::{Read, Seek, SeekFrom, Write};
use std::sync::{
    Mutex, MutexGuard, OnceLock,
    atomic::{AtomicU64, Ordering},
};
use std::time::{SystemTime, UNIX_EPOCH};

/// Default number of messages a `peek_session` returns. A preview is a glance,
/// so this is a tail rather than a transcript: enough to recognise which
/// conversation it is, few enough that peeking a dozen sessions stays cheap.
const PEEK_LIMIT: u64 = 12;

/// Requests the daemon only accepts on an attached (subscribed) connection.
///
/// `peek_session` and `list_sessions` are deliberately absent: they are served
/// from stored records precisely so a client can look around before attaching.
const REQUIRES_ATTACH: &[&str] = &[
    "send_message",
    "cancel",
    "soft_interrupt",
    "cancel_soft_interrupts",
    "clear",
    "rewind",
    "rewind_undo",
    "get_history",
    "list_models",
    "set_model",
    "set_reasoning_effort",
    "compact",
    "rename_session",
    "get_runtime_info",
    "fork_session",
    "read_file",
    "find_files",
    "search_text",
    "file_status",
];

const DEFAULT_FILE_BYTES: u64 = 1024 * 1024;
const MAX_FILE_BYTES: u64 = 8 * 1024 * 1024;
const DEFAULT_FIND_LIMIT: usize = 200;
const MAX_FIND_LIMIT: usize = 2_000;
const MAX_WALK_FILES: usize = 20_000;
const MAX_WALK_ENTRIES: usize = 50_000;
const MAX_SEARCH_BYTES: u64 = 64 * 1024 * 1024;

/// Flatten a stored message's `content` to plain text.
///
/// The daemon writes content either as a bare string or as an array of typed
/// blocks, so both shapes are accepted; anything without text (a tool call, an
/// image) contributes nothing rather than a placeholder.
fn flatten_content(content: &Value) -> String {
    if let Some(text) = content.as_str() {
        return text.to_string();
    }
    let Some(blocks) = content.as_array() else {
        return String::new();
    };
    blocks
        .iter()
        .filter_map(|block| block["text"].as_str())
        .collect::<Vec<_>>()
        .join("")
}
use serde_json::{Value, json};

/// Where a translated client request should go.
#[derive(Debug)]
pub enum Outbound {
    /// Forward to the legacy daemon connection.
    Legacy(Value),
    /// Answer the API client directly (no daemon round trip needed).
    Reply(ServerFrame),
}

type SessionFileStatus = (bool, String, Option<u64>, Option<u64>);
type SessionFileStatusResult = Result<SessionFileStatus, (ErrorCode, String)>;

// Legacy daemon replies are broadcast to every connected client. Request ids
// must therefore be unique across bridge connections, not merely within one
// `BridgeState`, or two panels issuing the same numbered request can each
// mistake the other's reply for their own.
static NEXT_LEGACY_ID: AtomicU64 = AtomicU64::new(1);

/// Per-connection translation state.
#[derive(Debug, Default)]
pub struct BridgeState {
    /// Whether an unannounced transport loss should crash the attached session.
    pub crash_on_disconnect: bool,
    /// Session id assigned by the daemon for this connection.
    pub session_id: Option<String>,
    /// Legacy id of the in-flight `message` request, so `done` maps to
    /// `turn_done`.
    pending_message_id: Option<u64>,
    /// Legacy and API ids for a context-only message. Its daemon completion
    /// event is a request reply, not a model turn boundary.
    pending_no_reply_message_id: Option<(u64, u64)>,
    /// Legacy id of an in-flight `create/attach` subscribe.
    pending_attach_id: Option<(u64, u64, Option<String>)>,
    /// Legacy and API ids for an in-flight session fork.
    pending_fork_id: Option<(u64, u64)>,
    /// Legacy id of the unsolicited model-catalog probe sent after attach. Its
    /// reply becomes a `model_info` event rather than a request reply, so it is
    /// tracked apart from `pending_simple`.
    pending_model_probe: Option<u64>,
    /// Legacy id -> API id for simple acked requests (ping, clear, ...).
    pending_simple: Vec<(u64, u64, SimpleKind)>,
    /// Every session the daemon has told us about, newest snapshot wins.
    ///
    /// The legacy protocol has no session-list request, but it volunteers the
    /// full set on every `state` event, so the bridge remembers it rather than
    /// answering `list_sessions` with only the one session this connection
    /// happens to be attached to.
    known_sessions: Vec<String>,
    /// Working directory per session, as far as it is known.
    session_dirs: std::collections::BTreeMap<String, String>,
    /// Models the daemon last reported for this session.
    ///
    /// The daemon volunteers the catalog on attach and again whenever it
    /// changes, so `list_models` is answered from here rather than by asking
    /// again: a picker that opens instantly is the difference between a usable
    /// model switcher and a spinner.
    available_models: Vec<String>,
    /// Model currently serving the session, tracked alongside the catalog so
    /// a picker can mark the active entry.
    current_model: Option<String>,
    current_provider: Option<String>,
    /// Reasoning effort last reported by the daemon, so identity events can
    /// carry it without a round trip.
    current_effort: Option<String>,
    available_routes: Vec<ModelRouteInfo>,
}

impl BridgeState {
    pub fn with_crash_on_disconnect(crash_on_disconnect: bool) -> Self {
        Self {
            crash_on_disconnect,
            ..Self::default()
        }
    }
}

#[derive(Debug, Default)]
struct ArchiveState {
    sessions: BTreeMap<String, u64>,
    archive_after_days: Option<u32>,
}

/// The small, canonical subset of a persisted `Session` needed by list and
/// attach responses. Serde skips the heavyweight transcript fields without
/// materializing them.
#[derive(Debug, Default, Deserialize)]
struct PersistedSessionMetadata {
    #[serde(default)]
    working_dir: Option<String>,
    /// Generated or imported title.
    #[serde(default)]
    title: Option<String>,
    /// User-provided rename, which is what `Session::display_title` prefers.
    #[serde(default)]
    custom_title: Option<String>,
    #[serde(default)]
    todo_title: Option<String>,
    #[serde(default)]
    saved: bool,
    #[serde(skip)]
    updated_at_ms: Option<i64>,
    #[serde(skip)]
    last_active_at_ms: Option<i64>,
}

impl PersistedSessionMetadata {
    fn display_title(&self) -> Option<String> {
        self.custom_title
            .as_deref()
            .and_then(Self::normalized_title)
            .or_else(|| self.todo_title.as_deref().and_then(Self::normalized_title))
            .or_else(|| self.title.as_deref().and_then(Self::normalized_title))
    }

    fn normalized_title(title: &str) -> Option<String> {
        let title = title.trim();
        (!title.is_empty()).then(|| title.to_string())
    }
}

#[derive(Clone, Debug)]
struct RecentSessionIndexEntry {
    session_id: String,
    working_dir: Option<String>,
    generated_title: Option<String>,
    custom_title: Option<String>,
    todo_title: Option<String>,
    saved: bool,
    updated_at_ms: i64,
    last_active_at_ms: Option<i64>,
}

impl From<&RecentSessionIndexEntry> for PersistedSessionMetadata {
    fn from(entry: &RecentSessionIndexEntry) -> Self {
        Self {
            working_dir: entry.working_dir.clone(),
            title: entry.generated_title.clone(),
            custom_title: entry.custom_title.clone(),
            todo_title: entry.todo_title.clone(),
            saved: entry.saved,
            updated_at_ms: Some(entry.updated_at_ms),
            last_active_at_ms: entry.last_active_at_ms,
        }
    }
}

#[derive(Debug, Clone, PartialEq)]
enum SimpleKind {
    Ping,
    History,
    Ok,
    /// Awaiting `model_changed`, which carries its own error field.
    Model,
    /// Awaiting `reasoning_effort_changed`.
    ReasoningEffort,
    /// Awaiting `compacted_history`.
    Compact,
    /// Awaiting the catalog reply that answers `list_models`.
    Models,
    Credential {
        provider: String,
        configured: bool,
    },
}

impl BridgeState {
    fn legacy_id(&mut self) -> u64 {
        NEXT_LEGACY_ID.fetch_add(1, Ordering::Relaxed)
    }

    /// Translate one API request (raw JSON) into outbound actions.
    pub fn api_request_to_legacy(&mut self, request: &Value) -> Vec<Outbound> {
        let api_id = request["id"].as_u64().unwrap_or(0);
        let req = request["req"].as_str().unwrap_or("");

        // Stateful requests only mean something once this connection is
        // attached. Forwarding one before then is not merely useless: the
        // daemon answers "Client must Subscribe with a working_dir before
        // sending stateful requests" and *closes the connection*, so a client
        // that mistypes a session id loses every other session it was
        // streaming, and the SDK reports a bare EPIPE. Answer locally instead,
        // with the code that actually says what went wrong.
        // `pending_attach_id` means a subscribe is already on the wire, so the
        // daemon will have a session by the time this arrives: a client that
        // pipelines `create_session` and `send_message` without awaiting must
        // still work. Only a connection that never asked to attach is refused.
        if self.session_id.is_none()
            && self.pending_attach_id.is_none()
            && REQUIRES_ATTACH.contains(&req)
        {
            let requested = request["session_id"].as_str().unwrap_or("");
            return vec![Outbound::Reply(ServerFrame::reply(
                api_id,
                ApiEvent::Error {
                    code: ErrorCode::UnknownSession,
                    message: if requested.is_empty() {
                        format!(
                            "`{req}` needs an attached session; call create_session or attach_session first"
                        )
                    } else {
                        format!(
                            "not attached to session `{requested}`; call attach_session first (it is not attached, or does not exist)"
                        )
                    },
                },
            ))];
        }

        // A request naming a session other than the attached one would be
        // silently applied to the attached session, because the legacy
        // protocol has no session field: `clear` on a typo'd id would wipe the
        // wrong transcript. Refuse rather than destroy the wrong thing.
        if let Some(attached) = self.session_id.as_deref()
            && REQUIRES_ATTACH.contains(&req)
            && let Some(requested) = request["session_id"].as_str()
            && !requested.is_empty()
            && requested != attached
        {
            return vec![Outbound::Reply(ServerFrame::reply(
                api_id,
                ApiEvent::Error {
                    code: ErrorCode::UnknownSession,
                    message: format!(
                        "this connection is attached to `{attached}`, not `{requested}`; attach to it first or use another connection"
                    ),
                },
            ))];
        }

        match req {
            "archive_session" => {
                let session_id = request["session_id"].as_str().unwrap_or_default();
                if Self::session_record_path(session_id).is_none_or(|path| !path.is_file()) {
                    return Self::error_reply(
                        api_id,
                        ErrorCode::UnknownSession,
                        "session does not exist",
                    );
                }
                let _write_guard = Self::state_write_guard();
                let mut archive = Self::load_archive_state();
                archive
                    .sessions
                    .insert(session_id.to_string(), Self::now_ms());
                match Self::save_archive_state(&archive) {
                    Ok(()) => vec![Outbound::Reply(ServerFrame::reply(api_id, ApiEvent::Ok))],
                    Err(message) => Self::error_reply(api_id, ErrorCode::Internal, &message),
                }
            }
            "restore_session" => {
                let session_id = request["session_id"].as_str().unwrap_or_default();
                if Self::session_record_path(session_id).is_none_or(|path| !path.is_file()) {
                    return Self::error_reply(
                        api_id,
                        ErrorCode::UnknownSession,
                        "session does not exist",
                    );
                }
                let _write_guard = Self::state_write_guard();
                let mut archive = Self::load_archive_state();
                archive.sessions.remove(session_id);
                match Self::save_archive_state(&archive) {
                    Ok(()) => vec![Outbound::Reply(ServerFrame::reply(api_id, ApiEvent::Ok))],
                    Err(message) => Self::error_reply(api_id, ErrorCode::Internal, &message),
                }
            }
            "set_retention_policy" => {
                let days = match request.get("archive_after_days") {
                    None | Some(Value::Null) => None,
                    Some(value) => match value.as_u64() {
                        Some(days @ 1..=36_500) => Some(days as u32),
                        _ => {
                            return Self::error_reply(
                                api_id,
                                ErrorCode::InvalidRequest,
                                "archive_after_days must be 1..=36500, or null to disable",
                            );
                        }
                    },
                };
                let _write_guard = Self::state_write_guard();
                let mut archive = Self::load_archive_state();
                archive.archive_after_days = days;
                match Self::save_archive_state(&archive) {
                    Ok(()) => vec![Outbound::Reply(ServerFrame::reply(api_id, ApiEvent::Ok))],
                    Err(message) => Self::error_reply(api_id, ErrorCode::Internal, &message),
                }
            }
            "create_session" | "attach_session" => {
                let id = self.legacy_id();
                let state_id = self.legacy_id();
                let catalog_id = self.legacy_id();
                let requested_session = (req == "attach_session")
                    .then(|| request["session_id"].as_str().map(str::to_string))
                    .flatten();
                self.pending_attach_id = Some((state_id, api_id, requested_session));
                self.pending_model_probe = Some(catalog_id);
                let working_dir =
                    request["working_dir"]
                        .as_str()
                        .map(str::to_string)
                        .or_else(|| {
                            std::env::current_dir()
                                .ok()
                                .map(|d| d.display().to_string())
                        });
                let mut subscribe = json!({
                    "type": "subscribe",
                    "id": id,
                    "working_dir": working_dir,
                });
                if self.crash_on_disconnect {
                    subscribe["crash_on_disconnect"] = json!(true);
                }
                // Sessions rooted inside a jcode checkout are self-dev
                // sessions: the daemon only enables the self-dev tools and
                // prompt when the subscribe says so, and a client that opens
                // the repo without saying so gets an agent that cannot build
                // the very app it is running in.
                if working_dir
                    .as_deref()
                    .is_some_and(Self::path_is_inside_jcode_repo)
                {
                    subscribe["selfdev"] = json!(true);
                }
                if req == "attach_session"
                    && let Some(target) = request["session_id"].as_str()
                {
                    subscribe["target_session_id"] = json!(target);
                }
                // The daemon assigns the session during subscribe but reports
                // the id via `state`, so chase the subscribe with get_state.
                // The model identity arrives the same way, via the catalog
                // reply, so ask for it now rather than making the client poll.
                vec![
                    Outbound::Legacy(subscribe),
                    Outbound::Legacy(json!({"type": "state", "id": state_id})),
                    Outbound::Legacy(json!({"type": "get_model_catalog", "id": catalog_id})),
                ]
            }
            "send_message" => {
                let id = self.legacy_id();
                let no_reply = request["no_reply"].as_bool().unwrap_or(false);
                if no_reply {
                    self.pending_no_reply_message_id = Some((id, api_id));
                } else {
                    self.pending_message_id = Some(id);
                }
                let mut message = json!({
                    "type": "message",
                    "id": id,
                    "content": request["content"].as_str().unwrap_or(""),
                });
                if no_reply {
                    message["no_reply"] = json!(true);
                }
                if let Some(images) = request["images"].as_array()
                    && !images.is_empty()
                {
                    message["images"] = json!(images);
                }
                vec![Outbound::Legacy(message)]
            }
            "fork_session" => {
                let id = self.legacy_id();
                self.pending_fork_id = Some((id, api_id));
                vec![Outbound::Legacy(json!({"type": "split", "id": id}))]
            }
            "cancel" => {
                let id = self.legacy_id();
                self.pending_simple.push((id, api_id, SimpleKind::Ok));
                vec![Outbound::Legacy(json!({"type": "cancel", "id": id}))]
            }
            "soft_interrupt" => {
                let id = self.legacy_id();
                self.pending_simple.push((id, api_id, SimpleKind::Ok));
                vec![Outbound::Legacy(json!({
                    "type": "soft_interrupt",
                    "id": id,
                    "content": request["content"].as_str().unwrap_or(""),
                    "urgent": request["urgent"].as_bool().unwrap_or(false),
                }))]
            }
            "clear" => {
                let id = self.legacy_id();
                self.pending_simple.push((id, api_id, SimpleKind::Ok));
                vec![Outbound::Legacy(json!({"type": "clear", "id": id}))]
            }
            "rewind" => {
                let id = self.legacy_id();
                self.pending_simple.push((id, api_id, SimpleKind::Ok));
                vec![Outbound::Legacy(json!({
                    "type": "rewind",
                    "id": id,
                    "message_index": request["message_index"].as_u64().unwrap_or(1),
                }))]
            }
            "get_history" => {
                let id = self.legacy_id();
                self.pending_simple.push((id, api_id, SimpleKind::History));
                vec![Outbound::Legacy(json!({"type": "get_history", "id": id}))]
            }
            // Answered from the stored record rather than the daemon: the
            // legacy protocol can only speak about the attached session, and
            // attaching to a session merely to read it would disturb the very
            // thing being previewed.
            "peek_session" => {
                let session_id = request["session_id"].as_str().unwrap_or_default();
                let limit = request["limit"].as_u64().unwrap_or(PEEK_LIMIT) as usize;
                vec![Outbound::Reply(ServerFrame::reply(
                    api_id,
                    ApiEvent::History {
                        session_id: session_id.to_string(),
                        messages: Self::stored_tail(session_id, limit),
                        images: Vec::new(),
                    },
                ))]
            }
            // Answered locally before attach. The daemon treats `ping` as a
            // "lightweight control" request: when it arrives as the first
            // frame on a connection it is answered and the connection is then
            // closed, which would tear down the client's whole session. A
            // liveness probe must never cost the caller its connection, and
            // reaching the bridge already proves the socket is alive.
            "ping" if self.session_id.is_none() => {
                vec![Outbound::Reply(ServerFrame::reply(api_id, ApiEvent::Pong))]
            }
            "ping" => {
                let id = self.legacy_id();
                self.pending_simple.push((id, api_id, SimpleKind::Ping));
                vec![Outbound::Legacy(json!({"type": "ping", "id": id}))]
            }
            "list_sessions" => {
                let list_started = std::time::Instant::now();
                // A fresh API connection has not received a daemon `state`
                // snapshot. Start with every persisted record, then merge the
                // live snapshot so unattached dashboards and global event
                // subscribers discover complete session state.
                let limit = request["limit"].as_u64().map(|limit| limit as usize);
                let mut ids: BTreeSet<String> =
                    Self::stored_session_ids(limit).into_iter().collect();
                let ids_loaded = list_started.elapsed();
                ids.extend(self.known_sessions.iter().cloned());
                if let Some(attached) = self.session_id.clone() {
                    ids.insert(attached);
                }
                if let Some(limit) = limit {
                    let mut recent: Vec<_> = ids.into_iter().collect();
                    recent.sort_unstable_by_key(|id| {
                        std::cmp::Reverse(Self::session_modified_ms(id))
                    });
                    recent.truncate(limit);
                    ids = recent.into_iter().collect();
                }
                // Titles are deliberately not cached. A rename is persisted
                // before `SessionRenamed` is broadcast, and every list call
                // should reflect that newest canonical value even on another
                // API connection.
                let indexed_metadata: BTreeMap<_, _> = Self::recent_session_index_entries()
                    .into_iter()
                    .map(|entry| (entry.session_id.clone(), entry))
                    .collect();
                let metadata: BTreeMap<String, PersistedSessionMetadata> = ids
                    .iter()
                    .filter_map(|id| {
                        indexed_metadata
                            .get(id)
                            .map(PersistedSessionMetadata::from)
                            .or_else(|| Self::resolve_session_metadata(id))
                            .map(|metadata| (id.clone(), metadata))
                    })
                    .collect();
                let metadata_loaded = list_started.elapsed();
                for id in &ids {
                    if !self.session_dirs.contains_key(id)
                        && let Some(dir) = metadata
                            .get(id)
                            .and_then(|metadata| metadata.working_dir.clone())
                    {
                        self.session_dirs.insert(id.clone(), dir);
                    }
                }
                let _write_guard = Self::state_write_guard();
                let mut archive = Self::load_archive_state();
                if let Some(days) = archive.archive_after_days {
                    let cutoff = Self::now_ms().saturating_sub(u64::from(days) * 86_400_000);
                    let mut changed = false;
                    for id in &ids {
                        if self.session_id.as_ref() == Some(id) || archive.sessions.contains_key(id)
                        {
                            continue;
                        }
                        if Self::session_modified_ms(id).is_some_and(|modified| modified < cutoff) {
                            archive.sessions.insert(id.clone(), Self::now_ms());
                            changed = true;
                        }
                    }
                    if changed && let Err(message) = Self::save_archive_state(&archive) {
                        return Self::error_reply(api_id, ErrorCode::Internal, &message);
                    }
                }
                let include_archived = request["include_archived"].as_bool().unwrap_or(false);
                let sessions: Vec<_> = ids
                    .into_iter()
                    .filter(|session_id| {
                        include_archived || !archive.sessions.contains_key(session_id)
                    })
                    .map(|session_id| SessionInfo {
                        working_dir: self.session_dirs.get(&session_id).cloned(),
                        title: metadata
                            .get(&session_id)
                            .and_then(PersistedSessionMetadata::display_title),
                        status: if self.session_id.as_ref() == Some(&session_id) {
                            "attached".into()
                        } else {
                            "idle".into()
                        },
                        transcript_bytes: Self::transcript_bytes(&session_id),
                        saved: metadata.get(&session_id).is_some_and(|value| value.saved),
                        updated_at_ms: metadata
                            .get(&session_id)
                            .and_then(|value| value.updated_at_ms)
                            .or_else(|| {
                                Self::session_modified_ms(&session_id).map(|value| value as i64)
                            }),
                        last_active_at_ms: metadata
                            .get(&session_id)
                            .and_then(|value| value.last_active_at_ms),
                        archived: archive.sessions.contains_key(&session_id),
                        archived_at_ms: archive.sessions.get(&session_id).copied(),
                        session_id,
                    })
                    .collect();
                let completed = list_started.elapsed();
                eprintln!(
                    "harness API bridge: list_sessions ids={:.1}ms metadata={:.1}ms total={:.1}ms count={}",
                    ids_loaded.as_secs_f64() * 1_000.0,
                    metadata_loaded.as_secs_f64() * 1_000.0,
                    completed.as_secs_f64() * 1_000.0,
                    sessions.len()
                );
                vec![Outbound::Reply(ServerFrame::reply(
                    api_id,
                    ApiEvent::Sessions { sessions },
                ))]
            }
            // Answered from the cached catalog. The daemon pushes it on attach
            // and on every change, so asking again would add a round trip to
            // an interaction (opening a picker) that must feel instant.
            "list_models" => {
                // Attach pushes the catalog, but a client that asks in the
                // same breath as attaching can beat it. Returning an empty
                // list would look like "no models exist", so ask the daemon
                // and answer when the catalog lands.
                if self.available_models.is_empty() {
                    let id = self.legacy_id();
                    self.pending_simple.push((id, api_id, SimpleKind::Models));
                    return vec![Outbound::Legacy(
                        json!({"type": "get_model_catalog", "id": id}),
                    )];
                }
                vec![Outbound::Reply(ServerFrame::reply(
                    api_id,
                    ApiEvent::Models {
                        session_id: self.session_id.clone().unwrap_or_default(),
                        models: self.available_models.clone(),
                        current: self.current_model.clone(),
                    },
                ))]
            }
            "get_runtime_info" => vec![Outbound::Reply(ServerFrame::reply(
                api_id,
                ApiEvent::RuntimeInfo {
                    session_id: self.session_id.clone().unwrap_or_default(),
                    provider: self.current_provider.clone(),
                    model: self.current_model.clone(),
                    reasoning_effort: self.current_effort.clone(),
                    routes: self.available_routes.clone(),
                },
            ))],
            "set_api_key" | "clear_api_key" => {
                let provider = request["provider"].as_str().unwrap_or_default();
                let Some((provider, env_keys, file_name)) = Self::credential_binding(provider)
                else {
                    return Self::error_reply(
                        api_id,
                        ErrorCode::InvalidRequest,
                        "unsupported API-key provider; supported: claude-api, openai-api, openrouter, cursor, gemini, jcode",
                    );
                };
                let configured = req == "set_api_key";
                let key = request["api_key"].as_str().unwrap_or_default();
                if configured
                    && (key.trim().is_empty()
                        || key.trim() != key
                        || key.contains(['\n', '\r', '\0']))
                {
                    return Self::error_reply(
                        api_id,
                        ErrorCode::InvalidRequest,
                        "api_key must be a non-empty, trimmed, non-NUL single line",
                    );
                }
                if let Err(message) =
                    Self::write_credential(file_name, env_keys, configured.then_some(key))
                {
                    return Self::error_reply(api_id, ErrorCode::Internal, &message);
                }
                let id = self.legacy_id();
                self.pending_simple.push((
                    id,
                    api_id,
                    SimpleKind::Credential {
                        provider: provider.to_string(),
                        configured,
                    },
                ));
                vec![Outbound::Legacy(json!({
                    "type": "notify_auth_changed",
                    "id": id,
                    "provider": provider,
                    "auth": {
                        "provider": provider,
                        "credential_source": "api_key_file",
                        "auth_method": "remote_tui_paste_api_key"
                    }
                }))]
            }
            "read_file" => {
                let session_id = request["session_id"].as_str().unwrap_or_default();
                let relative = request["path"].as_str().unwrap_or_default();
                let max = request["max_bytes"]
                    .as_u64()
                    .unwrap_or(DEFAULT_FILE_BYTES)
                    .min(MAX_FILE_BYTES);
                match Self::read_session_file(session_id, relative, max) {
                    Ok((content, size, truncated)) => vec![Outbound::Reply(ServerFrame::reply(
                        api_id,
                        ApiEvent::FileContent {
                            session_id: session_id.to_string(),
                            path: relative.to_string(),
                            content,
                            size,
                            truncated,
                        },
                    ))],
                    Err((code, message)) => Self::error_reply(api_id, code, &message),
                }
            }
            "find_files" => {
                let session_id = request["session_id"].as_str().unwrap_or_default();
                let query = request["query"].as_str().unwrap_or_default();
                let limit = request["limit"]
                    .as_u64()
                    .unwrap_or(DEFAULT_FIND_LIMIT as u64)
                    .min(MAX_FIND_LIMIT as u64) as usize;
                match Self::find_session_files(session_id, query, limit) {
                    Ok(paths) => vec![Outbound::Reply(ServerFrame::reply(
                        api_id,
                        ApiEvent::Files {
                            session_id: session_id.to_string(),
                            paths,
                        },
                    ))],
                    Err((code, message)) => Self::error_reply(api_id, code, &message),
                }
            }
            "search_text" => {
                let session_id = request["session_id"].as_str().unwrap_or_default();
                let query = request["query"].as_str().unwrap_or_default();
                let under = request["path"].as_str();
                let limit = request["limit"]
                    .as_u64()
                    .unwrap_or(DEFAULT_FIND_LIMIT as u64)
                    .min(MAX_FIND_LIMIT as u64) as usize;
                match Self::search_session_text(session_id, query, under, limit) {
                    Ok(matches) => vec![Outbound::Reply(ServerFrame::reply(
                        api_id,
                        ApiEvent::TextMatches {
                            session_id: session_id.to_string(),
                            matches,
                        },
                    ))],
                    Err((code, message)) => Self::error_reply(api_id, code, &message),
                }
            }
            "file_status" => {
                let session_id = request["session_id"].as_str().unwrap_or_default();
                let relative = request["path"].as_str().unwrap_or_default();
                match Self::session_file_status(session_id, relative) {
                    Ok((exists, kind, size, modified_ms)) => {
                        vec![Outbound::Reply(ServerFrame::reply(
                            api_id,
                            ApiEvent::FileStatus {
                                session_id: session_id.to_string(),
                                path: relative.to_string(),
                                exists,
                                kind,
                                size,
                                modified_ms,
                            },
                        ))]
                    }
                    Err((code, message)) => Self::error_reply(api_id, code, &message),
                }
            }
            "set_model" => {
                let model = request["model"].as_str().unwrap_or("");
                if model.is_empty() {
                    return vec![Outbound::Reply(ServerFrame::reply(
                        api_id,
                        ApiEvent::Error {
                            code: ErrorCode::InvalidRequest,
                            message: "set_model needs a non-empty `model`".into(),
                        },
                    ))];
                }
                let id = self.legacy_id();
                self.pending_simple.push((id, api_id, SimpleKind::Model));
                vec![Outbound::Legacy(json!({
                    "type": "set_model",
                    "id": id,
                    "model": model,
                }))]
            }
            "set_reasoning_effort" => {
                let effort = request["effort"].as_str().unwrap_or("");
                if effort.is_empty() {
                    return vec![Outbound::Reply(ServerFrame::reply(
                        api_id,
                        ApiEvent::Error {
                            code: ErrorCode::InvalidRequest,
                            message: "set_reasoning_effort needs a non-empty `effort`".into(),
                        },
                    ))];
                }
                let id = self.legacy_id();
                self.pending_simple
                    .push((id, api_id, SimpleKind::ReasoningEffort));
                vec![Outbound::Legacy(json!({
                    "type": "set_reasoning_effort",
                    "id": id,
                    "effort": effort,
                }))]
            }
            "compact" => {
                let id = self.legacy_id();
                self.pending_simple.push((id, api_id, SimpleKind::Compact));
                vec![Outbound::Legacy(json!({"type": "compact", "id": id}))]
            }
            "rename_session" => {
                let id = self.legacy_id();
                self.pending_simple.push((id, api_id, SimpleKind::Ok));
                let mut rename = json!({"type": "rename_session", "id": id});
                // An absent title clears it and restores the generated one, so
                // null and "" must stay distinguishable on the wire.
                if let Some(title) = request["title"].as_str() {
                    rename["title"] = json!(title);
                }
                vec![Outbound::Legacy(rename)]
            }
            "rewind_undo" => {
                let id = self.legacy_id();
                self.pending_simple.push((id, api_id, SimpleKind::Ok));
                vec![Outbound::Legacy(json!({"type": "rewind_undo", "id": id}))]
            }
            "cancel_soft_interrupts" => {
                let id = self.legacy_id();
                self.pending_simple.push((id, api_id, SimpleKind::Ok));
                vec![Outbound::Legacy(
                    json!({"type": "cancel_soft_interrupts", "id": id}),
                )]
            }
            "detach_session" => {
                let id = self.legacy_id();
                vec![
                    Outbound::Legacy(json!({"type": "prepare_disconnect", "id": id})),
                    Outbound::Reply(ServerFrame::reply(api_id, ApiEvent::Ok)),
                ]
            }
            "permission_response" => {
                // The legacy protocol does not surface permission prompts on
                // this path, so the bridge never emits `permission_request`
                // and there is nothing for a response to answer. Say that,
                // rather than "not supported", which reads like a bug the
                // caller should work around. Clients discover this up front
                // via the absence of the `permissions` capability in `hello`.
                vec![Outbound::Reply(ServerFrame::reply(
                    api_id,
                    ApiEvent::Error {
                        code: ErrorCode::InvalidRequest,
                        message: "this server does not issue permission prompts \
                                  (no `permissions` capability), so there is nothing to respond to"
                            .into(),
                    },
                ))]
            }
            other => vec![Outbound::Reply(ServerFrame::reply(
                api_id,
                ApiEvent::Error {
                    code: ErrorCode::UnknownRequest,
                    message: format!("unknown request: {other}"),
                },
            ))],
        }
    }

    /// Translate one legacy server event (raw JSON) into API frames.
    pub fn legacy_event_to_api(&mut self, event: &Value) -> Vec<ServerFrame> {
        let kind = event["type"].as_str().unwrap_or("");
        let session = |state: &Self| state.session_id.clone().unwrap_or_default();
        match kind {
            "session" => {
                let session_id = event["session_id"].as_str().unwrap_or("").to_string();
                // `session` is a broadcast lifecycle notification. The daemon
                // sends it for other live sessions too, so it cannot identify
                // which session this connection is attached to. Attachment
                // identity comes from the correlated `state` reply below.
                // Treating this broadcast as connection state made a panel for
                // session A start rejecting its own commands after session B
                // announced itself.
                vec![ServerFrame::event(ApiEvent::SessionStatus {
                    session_id,
                    status: "attached".into(),
                })]
            }
            "state" => {
                let session_id = event["session_id"].as_str().unwrap_or("").to_string();
                let id = event["id"].as_u64().unwrap_or(0);
                if let Some((state_id, api_id, requested_session)) = &self.pending_attach_id
                    && *state_id == id
                    && requested_session
                        .as_deref()
                        .is_none_or(|requested| requested == session_id)
                {
                    let api_id = *api_id;
                    self.pending_attach_id = None;
                    // `state` snapshots can also be broadcast for other live
                    // sessions. Only the reply correlated with this connection's
                    // attach request establishes its identity. Otherwise opening
                    // a second panel can retarget the first panel's bridge and
                    // make its next command fail with a wrong-session error.
                    if !session_id.is_empty() {
                        self.session_id = Some(session_id.clone());
                    }
                    let metadata = Self::resolve_session_metadata(&session_id);
                    return vec![ServerFrame::reply(
                        api_id,
                        ApiEvent::Attached {
                            session: SessionInfo {
                                transcript_bytes: Self::transcript_bytes(&session_id),
                                saved: metadata.as_ref().is_some_and(|value| value.saved),
                                updated_at_ms: metadata
                                    .as_ref()
                                    .and_then(|value| value.updated_at_ms)
                                    .or_else(|| {
                                        Self::session_modified_ms(&session_id)
                                            .map(|value| value as i64)
                                    }),
                                last_active_at_ms: metadata
                                    .as_ref()
                                    .and_then(|value| value.last_active_at_ms),
                                session_id,
                                working_dir: metadata
                                    .as_ref()
                                    .and_then(|metadata| metadata.working_dir.clone()),
                                title: metadata
                                    .as_ref()
                                    .and_then(PersistedSessionMetadata::display_title),
                                status: if event["is_processing"].as_bool().unwrap_or(false) {
                                    "processing".into()
                                } else {
                                    "idle".into()
                                },
                                archived: false,
                                archived_at_ms: None,
                            },
                        },
                    )];
                }
                vec![]
            }
            "text_delta" => vec![ServerFrame::event(ApiEvent::TextDelta {
                session_id: session(self),
                text: event["text"].as_str().unwrap_or("").to_string(),
            })],
            "reasoning_delta" => vec![ServerFrame::event(ApiEvent::ReasoningDelta {
                session_id: session(self),
                text: event["text"].as_str().unwrap_or("").to_string(),
            })],
            "reasoning_done" => vec![ServerFrame::event(ApiEvent::ReasoningDone {
                session_id: session(self),
                duration_secs: event["duration_secs"].as_f64(),
            })],
            "connection_phase" => vec![ServerFrame::event(ApiEvent::ConnectionPhase {
                session_id: session(self),
                phase: event["phase"].as_str().unwrap_or("connecting").to_string(),
            })],
            "tool_start" => vec![ServerFrame::event(ApiEvent::ToolStart {
                session_id: session(self),
                call_id: event["id"].as_str().unwrap_or("").to_string(),
                name: event["name"].as_str().unwrap_or("").to_string(),
            })],
            "tool_input" => vec![ServerFrame::event(ApiEvent::ToolInputDelta {
                session_id: session(self),
                call_id: String::new(),
                delta: event["delta"].as_str().unwrap_or("").to_string(),
            })],
            "tool_exec" => vec![ServerFrame::event(ApiEvent::ToolExec {
                session_id: session(self),
                call_id: event["id"].as_str().unwrap_or("").to_string(),
                name: event["name"].as_str().unwrap_or("").to_string(),
            })],
            "tool_done" => vec![ServerFrame::event(ApiEvent::ToolDone {
                session_id: session(self),
                call_id: event["id"].as_str().unwrap_or("").to_string(),
                name: event["name"].as_str().unwrap_or("").to_string(),
                output: event["output"].as_str().unwrap_or("").to_string(),
                error: event["error"].as_str().map(str::to_string),
            })],
            "side_pane_images" => vec![ServerFrame::event(ApiEvent::SidePaneImages {
                session_id: event["session_id"]
                    .as_str()
                    .map(str::to_string)
                    .unwrap_or_else(|| session(self)),
                images: serde_json::from_value(event["images"].clone()).unwrap_or_default(),
            })],
            "tokens" => vec![ServerFrame::event(ApiEvent::TokenUsage {
                session_id: session(self),
                input: event["input"].as_u64().unwrap_or(0),
                output: event["output"].as_u64().unwrap_or(0),
                cache_read_input: event["cache_read_input"].as_u64(),
            })],
            "done" => {
                let id = event["id"].as_u64().unwrap_or(0);
                // Subscribe and other requests also emit `done`; only a
                // completed `message` is a turn boundary.
                if self.pending_message_id == Some(id) {
                    self.pending_message_id = None;
                    vec![ServerFrame::event(ApiEvent::TurnDone {
                        session_id: session(self),
                    })]
                } else {
                    vec![]
                }
            }
            "wake_requested" => vec![ServerFrame::event(ApiEvent::WakeRequested {
                session_id: event["session_id"]
                    .as_str()
                    .map(str::to_string)
                    .unwrap_or_else(|| session(self)),
                reason: event["reason"].as_str().unwrap_or("").to_string(),
                notification: event["notification"].as_str().unwrap_or("").to_string(),
            })],
            "context_message_added" => {
                let id = event["id"].as_u64().unwrap_or(0);
                if self
                    .pending_no_reply_message_id
                    .is_some_and(|(legacy_id, _)| legacy_id == id)
                {
                    let (_, api_id) = self.pending_no_reply_message_id.take().unwrap();
                    vec![ServerFrame::reply(api_id, ApiEvent::Ok)]
                } else {
                    vec![]
                }
            }
            "split_response" => {
                let id = event["id"].as_u64().unwrap_or(0);
                let Some((_, api_id)) = self
                    .pending_fork_id
                    .take()
                    .filter(|(legacy_id, _)| *legacy_id == id)
                else {
                    return vec![];
                };
                let session_id = event["new_session_id"]
                    .as_str()
                    .unwrap_or_default()
                    .to_string();
                let metadata = Self::resolve_session_metadata(&session_id);
                vec![ServerFrame::reply(
                    api_id,
                    ApiEvent::SessionForked {
                        session: SessionInfo {
                            transcript_bytes: Self::transcript_bytes(&session_id),
                            saved: metadata.as_ref().is_some_and(|value| value.saved),
                            updated_at_ms: Self::session_modified_ms(&session_id)
                                .map(|value| value as i64),
                            last_active_at_ms: metadata
                                .as_ref()
                                .and_then(|value| value.last_active_at_ms),
                            session_id,
                            working_dir: metadata
                                .as_ref()
                                .and_then(|value| value.working_dir.clone()),
                            title: event["new_session_name"]
                                .as_str()
                                .map(str::to_string)
                                .or_else(|| {
                                    metadata
                                        .as_ref()
                                        .and_then(PersistedSessionMetadata::display_title)
                                }),
                            status: "idle".into(),
                            archived: false,
                            archived_at_ms: None,
                        },
                    },
                )]
            }
            "pong" => self
                .take_simple(event["id"].as_u64().unwrap_or(0), SimpleKind::Ping)
                .map(|api_id| vec![ServerFrame::reply(api_id, ApiEvent::Pong)])
                .unwrap_or_default(),
            "history" => {
                let id = event["id"].as_u64().unwrap_or(0);
                // The daemon volunteers the full session set on `history`,
                // which is the only place it appears: remember it so
                // `list_sessions` can answer with more than this connection.
                self.note_sessions(event);
                // The catalog probe rides the same `history` reply shape but
                // carries no messages: it is model identity, not transcript.
                if self.pending_model_probe == Some(id) {
                    self.pending_model_probe = None;
                    self.note_models(event);
                    return vec![ServerFrame::event(self.model_info(session(self), event))];
                }
                // A `list_models` that arrived before the catalog did: the
                // client asked too early, so answer it now that it is here.
                if let Some(api_id) = self.take_simple(id, SimpleKind::Models) {
                    self.note_models(event);
                    return vec![ServerFrame::reply(
                        api_id,
                        ApiEvent::Models {
                            session_id: session(self),
                            models: self.available_models.clone(),
                            current: self.current_model.clone(),
                        },
                    )];
                }
                let Some(api_id) = self.take_simple(id, SimpleKind::History) else {
                    return vec![];
                };
                let messages = event["messages"]
                    .as_array()
                    .map(|messages| {
                        messages
                            .iter()
                            .map(|m| HistoryMessage {
                                role: m["role"].as_str().unwrap_or("").to_string(),
                                content: m["content"].as_str().unwrap_or("").to_string(),
                            })
                            .collect()
                    })
                    .unwrap_or_default();
                let images = serde_json::from_value(event["images"].clone()).unwrap_or_default();
                vec![ServerFrame::reply(
                    api_id,
                    ApiEvent::History {
                        session_id: session(self),
                        messages,
                        images,
                    },
                )]
            }
            // The model can change mid-session (`/model`, a cycle, or an auth
            // change re-resolving the route), so both pushes are forwarded.
            "model_changed" => {
                let id = event["id"].as_u64().unwrap_or(0);
                let reply_to = self.take_simple(id, SimpleKind::Model);
                // The daemon reports a rejected switch in-band, as an `error`
                // field on the success event rather than as an error frame.
                // Relayed as a real error so a client sees a failed request
                // instead of a silent no-op that leaves the picker wrong.
                if let Some(error) = event["error"].as_str() {
                    let frame = ApiEvent::Error {
                        code: ErrorCode::InvalidRequest,
                        message: error.to_string(),
                    };
                    return match reply_to {
                        Some(api_id) => vec![ServerFrame::reply(api_id, frame)],
                        None => vec![],
                    };
                }
                if let Some(model) = event["model"].as_str() {
                    self.current_model = Some(model.to_string());
                }
                if let Some(provider) = event["provider_name"].as_str() {
                    self.current_provider = Some(provider.to_string());
                }
                let info = ApiEvent::ModelInfo {
                    session_id: session(self),
                    provider: event["provider_name"].as_str().map(str::to_string),
                    model: event["model"].as_str().map(str::to_string),
                    reasoning_effort: self.current_effort.clone(),
                };
                // Both a reply and a broadcast: the caller needs its request
                // resolved, and every other client watching the session needs
                // to know the model moved under them.
                match reply_to {
                    Some(api_id) => vec![
                        ServerFrame::reply(api_id, ApiEvent::Ok),
                        ServerFrame::event(info),
                    ],
                    None => vec![ServerFrame::event(info)],
                }
            }
            "reasoning_effort_changed" => {
                let id = event["id"].as_u64().unwrap_or(0);
                // Remember the new effort even when the change was requested by
                // another client, so later identity events stay truthful.
                let changed = event["error"].as_str().is_none()
                    && event["effort"].as_str().is_some_and(|effort| {
                        let effort = Some(effort.to_string());
                        let moved = self.current_effort != effort;
                        self.current_effort = effort;
                        moved
                    });
                // A successful change is also broadcast as identity, mirroring
                // model_changed: every attached client needs to know the
                // effort moved under it, not only the one that asked.
                let info = changed.then(|| {
                    ServerFrame::event(ApiEvent::ModelInfo {
                        session_id: session(self),
                        provider: self.current_provider.clone(),
                        model: self.current_model.clone(),
                        reasoning_effort: self.current_effort.clone(),
                    })
                });
                let Some(api_id) = self.take_simple(id, SimpleKind::ReasoningEffort) else {
                    return info.into_iter().collect();
                };
                match event["error"].as_str() {
                    Some(error) => vec![ServerFrame::reply(
                        api_id,
                        ApiEvent::Error {
                            code: ErrorCode::InvalidRequest,
                            message: error.to_string(),
                        },
                    )],
                    None => std::iter::once(ServerFrame::reply(api_id, ApiEvent::Ok))
                        .chain(info)
                        .collect(),
                }
            }
            // Compaction is scheduled, not performed inline, and the daemon
            // reports refusal via `success: false` with a reason rather than
            // an error frame. Relayed as a real error so a client is not told
            // "done" about work that never happened.
            "compact_result" => {
                let id = event["id"].as_u64().unwrap_or(0);
                let Some(api_id) = self.take_simple(id, SimpleKind::Compact) else {
                    return vec![];
                };
                let message = event["message"].as_str().unwrap_or("").to_string();
                if event["success"].as_bool() == Some(false) {
                    return vec![ServerFrame::reply(
                        api_id,
                        ApiEvent::Error {
                            code: ErrorCode::InvalidRequest,
                            message,
                        },
                    )];
                }
                vec![ServerFrame::reply(
                    api_id,
                    ApiEvent::Compacted {
                        session_id: session(self),
                        message,
                    },
                )]
            }
            "session_renamed" => {
                let session_id = event["session_id"]
                    .as_str()
                    .map(str::to_string)
                    .unwrap_or_else(|| session(self));
                vec![ServerFrame::event(ApiEvent::SessionRenamed {
                    session_id,
                    title: event["title"].as_str().map(str::to_string),
                    display_title: event["display_title"].as_str().unwrap_or("").to_string(),
                })]
            }
            "available_models_updated" => {
                self.note_models(event);
                vec![ServerFrame::event(self.model_info(session(self), event))]
            }
            // Background-task traffic reaches clients as a notification whose
            // body is the markdown the TUI renders. The API refuses to make
            // clients re-derive a progress bar from prose, so the two shapes
            // that matter (a progress tick and a task finishing) are parsed
            // into one typed event and everything else is dropped.
            "notification" => {
                let message = event["message"].as_str().unwrap_or("");
                match parse_background_notification(message) {
                    Some(mut progress) => {
                        progress.session_id = session(self);
                        vec![ServerFrame::event(progress.into_event())]
                    }
                    None => vec![],
                }
            }
            "ack" => {
                let id = event["id"].as_u64().unwrap_or(0);
                // The daemon acking the in-flight `message` is the proof the
                // agent has the text: report it as its own event so a client
                // can move a message from "sent" to "acknowledged" without
                // waiting for the first token of the reply.
                if self.pending_message_id == Some(id) {
                    return vec![ServerFrame::event(ApiEvent::MessageAccepted {
                        session_id: session(self),
                    })];
                }
                let Some(index) = self
                    .pending_simple
                    .iter()
                    .position(|(legacy_id, _, _)| *legacy_id == id)
                else {
                    return vec![];
                };
                let (_, api_id, kind) = self.pending_simple.remove(index);
                match kind {
                    SimpleKind::Ok => vec![ServerFrame::reply(api_id, ApiEvent::Ok)],
                    SimpleKind::Credential {
                        provider,
                        configured,
                    } => vec![ServerFrame::reply(
                        api_id,
                        ApiEvent::CredentialUpdated {
                            provider,
                            configured,
                        },
                    )],
                    // Acks are not terminal replies for richer request kinds.
                    other => {
                        self.pending_simple.push((id, api_id, other));
                        vec![]
                    }
                }
            }
            "error" => {
                let id = event["id"].as_u64().unwrap_or(0);
                let message = event["message"].as_str().unwrap_or("").to_string();
                // A turn that fails ends with `error` *instead of* `done`, so
                // the turn is over: forget the pending message, or a later
                // unrelated `done` carrying the same id would be reported as
                // this turn finishing.
                if self.pending_message_id == Some(id) {
                    self.pending_message_id = None;
                }
                let no_reply_api_id = self
                    .pending_no_reply_message_id
                    .filter(|(legacy_id, _)| *legacy_id == id)
                    .map(|(_, api_id)| api_id);
                if no_reply_api_id.is_some() {
                    self.pending_no_reply_message_id = None;
                }
                let fork_api_id = self
                    .pending_fork_id
                    .filter(|(legacy_id, _)| *legacy_id == id)
                    .map(|(_, api_id)| api_id);
                if fork_api_id.is_some() {
                    self.pending_fork_id = None;
                }
                // Route to a pending request when possible, else stream it.
                let reply_to = no_reply_api_id.or(fork_api_id).or_else(|| {
                    self.pending_simple
                        .iter()
                        .position(|(legacy_id, _, _)| *legacy_id == id)
                        .map(|index| self.pending_simple.remove(index).1)
                });
                let frame_event = ApiEvent::Error {
                    code: ErrorCode::Internal,
                    message,
                };
                vec![match reply_to {
                    Some(api_id) => ServerFrame::reply(api_id, frame_event),
                    None => ServerFrame::event(frame_event),
                }]
            }
            // Everything else on the legacy stream is not part of the stable
            // API surface yet; drop it.
            _ => vec![],
        }
    }

    /// Read provider/model identity out of any legacy event that carries the
    /// `provider_name`/`provider_model` pair (the catalog reply and the
    /// available-models push both do).
    /// Remember the catalog carried by any legacy event that has one.
    ///
    /// The daemon reports models on attach and on every change; caching here
    /// is what lets `list_models` answer without a round trip.
    fn note_models(&mut self, event: &Value) {
        if let Some(models) = event["available_models"].as_array() {
            let names: Vec<String> = models
                .iter()
                .filter_map(|value| value.as_str().map(str::to_string))
                .collect();
            if !names.is_empty() {
                self.available_models = names;
            }
        }
        if let Some(model) = event["provider_model"].as_str() {
            self.current_model = Some(model.to_string());
        }
        if let Some(provider) = event["provider_name"].as_str() {
            self.current_provider = Some(provider.to_string());
        }
        if let Some(effort) = event["reasoning_effort"].as_str() {
            self.current_effort = Some(effort.to_string());
        }
        if let Some(routes) = event["available_model_routes"].as_array() {
            self.available_routes = routes
                .iter()
                .filter_map(|route| {
                    Some(ModelRouteInfo {
                        model: route["model"].as_str()?.to_string(),
                        provider: route["provider"].as_str()?.to_string(),
                        api_method: route["api_method"].as_str()?.to_string(),
                        available: route["available"].as_bool().unwrap_or(false),
                        detail: route["detail"].as_str().unwrap_or_default().to_string(),
                    })
                })
                .collect();
        }
    }

    fn model_info(&self, session_id: String, event: &Value) -> ApiEvent {
        ApiEvent::ModelInfo {
            session_id,
            provider: event["provider_name"].as_str().map(str::to_string),
            model: event["provider_model"].as_str().map(str::to_string),
            reasoning_effort: event["reasoning_effort"]
                .as_str()
                .map(str::to_string)
                .or_else(|| self.current_effort.clone()),
        }
    }

    /// True when `path`, or any ancestor, looks like a jcode source checkout.
    ///
    /// Matched by content (a workspace manifest next to the crates directory)
    /// rather than by name, so a clone in any directory is recognised.
    fn path_is_inside_jcode_repo(path: &str) -> bool {
        let mut current = Some(std::path::Path::new(path));
        while let Some(dir) = current {
            if dir.join("Cargo.toml").is_file() && dir.join("crates/jcode-base").is_dir() {
                return true;
            }
            current = dir.parent();
        }
        false
    }

    /// Path of a session's persisted record, or `None` if the id is not a
    /// plain session id.
    ///
    /// One funnel for three reasons. It honours `JCODE_HOME`, without which a
    /// launched instance reads the *user's* sessions: `peek_session` served
    /// the real transcripts of the jcode the user runs interactively, which
    /// defeats the isolation an embedded instance exists to provide. It
    /// rejects ids that are not bare session ids, since the id comes straight
    /// off the wire and is interpolated into a path, so `../../.ssh/id_rsa`
    /// would otherwise be read and returned. And it keeps the three callers
    /// from drifting apart, which is how the first two problems survived
    /// being fixed anywhere else.
    fn session_record_path(session_id: &str) -> Option<std::path::PathBuf> {
        // Session ids are `session_<name>_<millis>_<hex>`; anything with a
        // separator or a parent reference is not one.
        if session_id.is_empty()
            || session_id.len() > 128
            || !session_id
                .chars()
                .all(|ch| ch.is_ascii_alphanumeric() || ch == '_' || ch == '-')
        {
            return None;
        }
        let home = match std::env::var_os("JCODE_HOME") {
            Some(home) => std::path::PathBuf::from(home),
            None => std::path::Path::new(&std::env::var_os("HOME")?).join(".jcode"),
        };
        Some(home.join("sessions").join(format!("{session_id}.json")))
    }

    /// Metadata of a session, read from its persisted record.
    ///
    /// The legacy `history` event lists session *ids* only, but the strip
    /// groups by directory, so the bridge resolves them from the same files
    /// the daemon persists. Best-effort by design: an unreadable or missing
    /// record simply leaves the session ungrouped rather than failing the
    /// list. Session records can be hundreds of megabytes, while the fields we
    /// need are written in the small header and trailer. Reading bounded windows
    /// avoids parsing every transcript message just to populate the sidebar.
    fn resolve_session_metadata(session_id: &str) -> Option<PersistedSessionMetadata> {
        let path = Self::session_record_path(session_id)?;
        // A missing or malformed record is expected (a session may predate the
        // fields, or be mid-write), and the only cost is missing metadata, so
        // this degrades rather than failing the whole session list or attach.
        const WINDOW: usize = 64 * 1024;
        let mut file = std::fs::File::open(path).ok()?;
        let len = file.metadata().ok()?.len();
        let head_len = usize::try_from(len.min(WINDOW as u64)).ok()?;
        let mut head = vec![0; head_len];
        file.read_exact(&mut head).ok()?;
        let tail = if len > WINDOW as u64 {
            file.seek(SeekFrom::End(-(WINDOW as i64))).ok()?;
            let mut tail = vec![0; WINDOW];
            file.read_exact(&mut tail).ok()?;
            tail
        } else {
            Vec::new()
        };
        Some(PersistedSessionMetadata {
            title: Self::metadata_string(&head, "title", false),
            custom_title: Self::metadata_string(&tail, "custom_title", true)
                .or_else(|| Self::metadata_string(&head, "custom_title", true)),
            todo_title: None,
            saved: Self::metadata_bool(&tail, "saved", true)
                .or_else(|| Self::metadata_bool(&head, "saved", false))
                .unwrap_or(false),
            updated_at_ms: None,
            last_active_at_ms: None,
            working_dir: Self::metadata_string(&tail, "working_dir", true)
                .or_else(|| Self::metadata_string(&head, "working_dir", true)),
        })
    }

    fn metadata_string(bytes: &[u8], field: &str, last: bool) -> Option<String> {
        let needle = format!("\"{field}\":");
        let mut starts = bytes
            .windows(needle.len())
            .enumerate()
            .filter_map(|(at, window)| (window == needle.as_bytes()).then_some(at + needle.len()));
        let start = if last { starts.last()? } else { starts.next()? };
        Option::<String>::deserialize(&mut serde_json::Deserializer::from_slice(&bytes[start..]))
            .ok()
            .flatten()
    }

    fn metadata_bool(bytes: &[u8], field: &str, last: bool) -> Option<bool> {
        let needle = format!("\"{field}\":");
        let mut starts = bytes
            .windows(needle.len())
            .enumerate()
            .filter_map(|(at, window)| (window == needle.as_bytes()).then_some(at + needle.len()));
        let start = if last {
            starts.next_back()?
        } else {
            starts.next()?
        };
        bool::deserialize(&mut serde_json::Deserializer::from_slice(&bytes[start..])).ok()
    }

    fn resolve_working_dir(session_id: &str) -> Option<String> {
        Self::resolve_session_metadata(session_id)?.working_dir
    }

    /// Size of a session's stored record, in bytes.
    ///
    /// A stat rather than a parse: this runs for every session on every list
    /// request, and deserializing a dozen multi-megabyte transcripts to count
    /// their characters would make the cheap call expensive. The file is
    /// almost entirely message content, so its size tracks the conversation
    /// closely enough for a client to size or sort by.
    fn transcript_bytes(session_id: &str) -> Option<u64> {
        let path = Self::session_record_path(session_id)?;
        std::fs::metadata(path).ok().map(|meta| meta.len())
    }

    fn session_modified_ms(session_id: &str) -> Option<u64> {
        Self::session_record_path(session_id)?
            .metadata()
            .ok()?
            .modified()
            .ok()?
            .duration_since(UNIX_EPOCH)
            .ok()
            .map(|duration| duration.as_millis() as u64)
    }

    fn now_ms() -> u64 {
        SystemTime::now()
            .duration_since(UNIX_EPOCH)
            .unwrap_or_default()
            .as_millis() as u64
    }

    fn jcode_home() -> Option<std::path::PathBuf> {
        std::env::var_os("JCODE_HOME")
            .map(std::path::PathBuf::from)
            .or_else(|| {
                std::env::var_os("HOME").map(|home| std::path::Path::new(&home).join(".jcode"))
            })
    }

    fn recent_session_index_path() -> Option<std::path::PathBuf> {
        Some(Self::jcode_home()?.join("session-metadata-v1.sqlite3"))
    }

    fn recent_session_index_entries() -> Vec<RecentSessionIndexEntry> {
        let Some(path) = Self::recent_session_index_path() else {
            return Vec::new();
        };
        let Ok(connection) = Connection::open(path) else {
            return Vec::new();
        };
        if connection
            .execute_batch(
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
                     saved INTEGER NOT NULL DEFAULT 0
                 );
                 CREATE INDEX IF NOT EXISTS recent_sessions_activity
                 ON recent_sessions(COALESCE(last_active_at_ms, updated_at_ms) DESC);",
            )
            .is_err()
        {
            return Vec::new();
        }
        let _ = connection.execute(
            "ALTER TABLE recent_sessions ADD COLUMN saved INTEGER NOT NULL DEFAULT 0",
            [],
        );
        let Ok(mut statement) = connection.prepare(
            "SELECT session_id, working_dir, generated_title, custom_title,
                    todo_title, saved, updated_at_ms, last_active_at_ms
             FROM recent_sessions
             ORDER BY COALESCE(last_active_at_ms, updated_at_ms) DESC
             LIMIT 500",
        ) else {
            return Vec::new();
        };
        statement
            .query_map([], |row| {
                Ok(RecentSessionIndexEntry {
                    session_id: row.get(0)?,
                    working_dir: row.get(1)?,
                    generated_title: row.get(2)?,
                    custom_title: row.get(3)?,
                    todo_title: row.get(4)?,
                    saved: row.get(5)?,
                    updated_at_ms: row.get(6)?,
                    last_active_at_ms: row.get(7)?,
                })
            })
            .and_then(|rows| rows.collect())
            .unwrap_or_default()
    }

    fn write_bootstrap_recent_session_index(ids: &[(SystemTime, String)]) {
        let Some(path) = Self::recent_session_index_path() else {
            return;
        };
        let Ok(mut connection) = Connection::open(path) else {
            return;
        };
        let Ok(transaction) = connection.transaction() else {
            return;
        };
        for (modified, session_id) in ids.iter().take(500) {
            let metadata = Self::resolve_session_metadata(session_id).unwrap_or_default();
            let updated_at_ms = modified
                .duration_since(UNIX_EPOCH)
                .map(|duration| duration.as_millis().min(i64::MAX as u128) as i64)
                .unwrap_or_default();
            let _ = transaction.execute(
                "INSERT INTO recent_sessions (
                     session_id, working_dir, generated_title, custom_title,
                     todo_title, updated_at_ms, last_active_at_ms
                 ) VALUES (?1, ?2, ?3, ?4, NULL, ?5, ?5)
                 ON CONFLICT(session_id) DO NOTHING",
                params![
                    session_id,
                    metadata.working_dir,
                    metadata.title,
                    metadata.custom_title,
                    updated_at_ms,
                ],
            );
        }
        let _ = transaction.commit();
    }

    fn stored_session_ids(limit: Option<usize>) -> Vec<String> {
        let indexed = Self::recent_session_index_entries();
        if !indexed.is_empty() && limit.is_some_and(|limit| indexed.len() >= limit) {
            return indexed
                .into_iter()
                .take(limit.unwrap_or(usize::MAX))
                .map(|entry| entry.session_id)
                .collect();
        }
        let Some(home) = Self::jcode_home() else {
            return Vec::new();
        };
        let Ok(entries) = std::fs::read_dir(home.join("sessions")) else {
            return Vec::new();
        };
        let candidates: Vec<(String, std::path::PathBuf)> = entries
            .flatten()
            .filter_map(|entry| {
                let kind = entry.file_type().ok()?;
                let path = entry.path();
                if !kind.is_file() || path.extension()?.to_str()? != "json" {
                    return None;
                }
                let id = path.file_stem()?.to_str()?.to_string();
                Self::session_record_path(&id)
                    .is_some()
                    .then_some((id, path))
            })
            .collect();
        // `stat` is the dominant cost with 100k+ sessions. Match the TUI picker
        // by doing those independent filesystem calls concurrently rather than
        // serially blocking the API reply long enough for clients to time out.
        let workers = std::thread::available_parallelism()
            .map(usize::from)
            .unwrap_or(1)
            .min(candidates.len().max(1));
        let chunk_size = candidates.len().div_ceil(workers);
        let mut ids = std::thread::scope(|scope| {
            let handles = candidates.chunks(chunk_size).map(|chunk| {
                scope.spawn(move || {
                    chunk
                        .iter()
                        .map(|(id, path)| {
                            let modified = path
                                .metadata()
                                .and_then(|metadata| metadata.modified())
                                .unwrap_or(UNIX_EPOCH);
                            (modified, id.clone())
                        })
                        .collect::<Vec<_>>()
                })
            });
            handles
                .flat_map(|handle| handle.join().unwrap_or_default())
                .collect::<Vec<_>>()
        });
        ids.sort_unstable_by(|left, right| right.0.cmp(&left.0));
        Self::write_bootstrap_recent_session_index(&ids);
        if let Some(limit) = limit {
            ids.truncate(limit);
        }
        ids.into_iter().map(|(_, id)| id).collect()
    }

    fn archive_state_path() -> Option<std::path::PathBuf> {
        Some(Self::jcode_home()?.join("sdk-archive.json"))
    }

    fn state_write_guard() -> MutexGuard<'static, ()> {
        static STATE_WRITES: OnceLock<Mutex<()>> = OnceLock::new();
        STATE_WRITES
            .get_or_init(|| Mutex::new(()))
            .lock()
            .unwrap_or_else(|poisoned| poisoned.into_inner())
    }

    fn load_archive_state() -> ArchiveState {
        let Some(path) = Self::archive_state_path() else {
            return ArchiveState::default();
        };
        let Ok(text) = std::fs::read_to_string(path) else {
            return ArchiveState::default();
        };
        let Ok(value) = serde_json::from_str::<Value>(&text) else {
            return ArchiveState::default();
        };
        let sessions = value["sessions"]
            .as_object()
            .map(|sessions| {
                sessions
                    .iter()
                    .filter_map(|(id, at)| at.as_u64().map(|at| (id.clone(), at)))
                    .collect()
            })
            .unwrap_or_default();
        ArchiveState {
            sessions,
            archive_after_days: value["archive_after_days"].as_u64().map(|days| days as u32),
        }
    }

    fn save_archive_state(state: &ArchiveState) -> Result<(), String> {
        let path =
            Self::archive_state_path().ok_or_else(|| "could not resolve jcode home".to_string())?;
        let parent = path
            .parent()
            .ok_or_else(|| "invalid archive path".to_string())?;
        std::fs::create_dir_all(parent).map_err(|error| error.to_string())?;
        let value = json!({
            "sessions": state.sessions,
            "archive_after_days": state.archive_after_days,
        });
        Self::atomic_owner_write(
            &path,
            serde_json::to_string_pretty(&value)
                .map_err(|error| error.to_string())?
                .as_bytes(),
        )
    }

    fn app_config_dir() -> Option<std::path::PathBuf> {
        if let Some(home) = std::env::var_os("JCODE_HOME") {
            return Some(std::path::Path::new(&home).join("config/jcode"));
        }
        #[cfg(target_os = "macos")]
        return Some(
            std::path::Path::new(&std::env::var_os("HOME")?)
                .join("Library/Application Support/jcode"),
        );
        #[cfg(target_os = "windows")]
        return Some(std::path::Path::new(&std::env::var_os("APPDATA")?).join("jcode"));
        #[cfg(not(any(target_os = "macos", target_os = "windows")))]
        Some(
            std::env::var_os("XDG_CONFIG_HOME")
                .map(std::path::PathBuf::from)
                .unwrap_or_else(|| {
                    std::path::Path::new(&std::env::var_os("HOME").unwrap_or_default())
                        .join(".config")
                })
                .join("jcode"),
        )
    }

    fn credential_binding(
        provider: &str,
    ) -> Option<(&'static str, &'static [&'static str], &'static str)> {
        match provider.trim().to_ascii_lowercase().as_str() {
            "claude-api" | "anthropic-api" | "anthropic-key" | "claude-key" => {
                Some(("claude-api", &["ANTHROPIC_API_KEY"], "anthropic.env"))
            }
            "openai-api" | "openai-key" | "openai-apikey" => {
                Some(("openai-api", &["OPENAI_API_KEY"], "openai.env"))
            }
            "openrouter" => Some(("openrouter", &["OPENROUTER_API_KEY"], "openrouter.env")),
            "cursor" => Some(("cursor", &["CURSOR_API_KEY"], "cursor.env")),
            "gemini" | "gemini-api" | "google-gemini" => Some((
                "gemini",
                &["GEMINI_API_KEY", "GOOGLE_API_KEY"],
                "gemini.env",
            )),
            "jcode" | "subscription" | "jcode-subscription" => {
                Some(("jcode", &["JCODE_API_KEY"], "jcode-subscription.env"))
            }
            _ => None,
        }
    }

    fn write_credential(
        file_name: &str,
        env_keys: &[&str],
        value: Option<&str>,
    ) -> Result<(), String> {
        let _write_guard = Self::state_write_guard();
        let dir = Self::app_config_dir()
            .ok_or_else(|| "could not resolve jcode config directory".to_string())?;
        Self::ensure_owner_only_dir(&dir)?;
        let path = dir.join(file_name);
        Self::validate_owner_file_if_exists(&path)?;
        let existing = match std::fs::read_to_string(&path) {
            Ok(existing) => existing,
            Err(error) if error.kind() == std::io::ErrorKind::NotFound => String::new(),
            Err(error) => return Err(format!("read credential file: {error}")),
        };
        let mut lines: Vec<String> = existing
            .lines()
            .filter(|line| {
                line.split_once('=')
                    .is_none_or(|(key, _)| !env_keys.contains(&key.trim()))
            })
            .map(str::to_string)
            .collect();
        if let Some(value) = value {
            if value.contains(['\n', '\r', '\0']) || value.trim() != value {
                return Err("API key must be a trimmed, non-NUL single line".into());
            }
            lines.push(format!("{}={value}", env_keys[0]));
        }
        let mut content = lines.join("\n");
        if !content.is_empty() {
            content.push('\n');
        }
        Self::atomic_owner_write(&path, content.as_bytes())
    }

    fn atomic_owner_write(path: &std::path::Path, bytes: &[u8]) -> Result<(), String> {
        let parent = path
            .parent()
            .ok_or_else(|| "owner-only write path has no parent".to_string())?;
        Self::ensure_owner_only_dir(parent)?;
        Self::validate_owner_file_if_exists(path)?;
        let temp = path.with_extension(format!(
            "tmp-{}-{}",
            std::process::id(),
            SystemTime::now()
                .duration_since(UNIX_EPOCH)
                .unwrap_or_default()
                .as_nanos()
        ));
        let mut options = std::fs::OpenOptions::new();
        options.write(true).create_new(true);
        #[cfg(unix)]
        {
            use std::os::unix::fs::OpenOptionsExt;
            options.mode(0o600);
        }
        let mut file = options.open(&temp).map_err(|error| error.to_string())?;
        let write_result = file.write_all(bytes).and_then(|()| file.sync_all());
        drop(file);
        if let Err(error) = write_result {
            let _ = std::fs::remove_file(&temp);
            return Err(error.to_string());
        }
        Self::atomic_replace(&temp, path).map_err(|error| {
            let _ = std::fs::remove_file(&temp);
            error.to_string()
        })?;
        #[cfg(unix)]
        {
            use std::os::unix::fs::PermissionsExt;
            std::fs::set_permissions(path, std::fs::Permissions::from_mode(0o600))
                .map_err(|error| error.to_string())?;
        }
        #[cfg(unix)]
        std::fs::File::open(parent)
            .and_then(|directory| directory.sync_all())
            .map_err(|error| format!("sync owner-only directory: {error}"))?;
        Ok(())
    }

    fn ensure_owner_only_dir(path: &std::path::Path) -> Result<(), String> {
        std::fs::create_dir_all(path).map_err(|error| error.to_string())?;
        let metadata = std::fs::symlink_metadata(path).map_err(|error| error.to_string())?;
        if metadata.file_type().is_symlink() || !metadata.is_dir() {
            return Err(format!(
                "{} must be a non-symlink directory",
                path.display()
            ));
        }
        #[cfg(unix)]
        {
            use std::os::unix::fs::{MetadataExt, PermissionsExt};
            if metadata.uid() != unsafe { libc::geteuid() } {
                return Err(format!(
                    "{} is not owned by the current user",
                    path.display()
                ));
            }
            std::fs::set_permissions(path, std::fs::Permissions::from_mode(0o700))
                .map_err(|error| error.to_string())?;
        }
        Ok(())
    }

    fn validate_owner_file_if_exists(path: &std::path::Path) -> Result<(), String> {
        let metadata = match std::fs::symlink_metadata(path) {
            Ok(metadata) => metadata,
            Err(error) if error.kind() == std::io::ErrorKind::NotFound => return Ok(()),
            Err(error) => return Err(error.to_string()),
        };
        if metadata.file_type().is_symlink() || !metadata.is_file() {
            return Err(format!(
                "{} must be a regular, non-symlink file",
                path.display()
            ));
        }
        #[cfg(unix)]
        {
            use std::os::unix::fs::MetadataExt;
            if metadata.uid() != unsafe { libc::geteuid() } {
                return Err(format!(
                    "{} is not owned by the current user",
                    path.display()
                ));
            }
        }
        Ok(())
    }

    #[cfg(not(windows))]
    fn atomic_replace(from: &std::path::Path, to: &std::path::Path) -> std::io::Result<()> {
        std::fs::rename(from, to)
    }

    #[cfg(windows)]
    fn atomic_replace(from: &std::path::Path, to: &std::path::Path) -> std::io::Result<()> {
        use std::os::windows::ffi::OsStrExt;
        unsafe extern "system" {
            fn MoveFileExW(from: *const u16, to: *const u16, flags: u32) -> i32;
        }
        const MOVEFILE_REPLACE_EXISTING: u32 = 0x1;
        const MOVEFILE_WRITE_THROUGH: u32 = 0x8;
        let from: Vec<u16> = from.as_os_str().encode_wide().chain(Some(0)).collect();
        let to: Vec<u16> = to.as_os_str().encode_wide().chain(Some(0)).collect();
        let replaced = unsafe {
            MoveFileExW(
                from.as_ptr(),
                to.as_ptr(),
                MOVEFILE_REPLACE_EXISTING | MOVEFILE_WRITE_THROUGH,
            )
        };
        if replaced == 0 {
            Err(std::io::Error::last_os_error())
        } else {
            Ok(())
        }
    }

    fn session_root(session_id: &str) -> Result<std::path::PathBuf, (ErrorCode, String)> {
        let root = Self::resolve_working_dir(session_id).ok_or((
            ErrorCode::UnknownSession,
            "session has no working directory".into(),
        ))?;
        std::fs::canonicalize(root).map_err(|error| {
            (
                ErrorCode::InvalidRequest,
                format!("working directory is unavailable: {error}"),
            )
        })
    }

    fn safe_session_path(
        session_id: &str,
        relative: &str,
    ) -> Result<std::path::PathBuf, (ErrorCode, String)> {
        if relative.is_empty() || std::path::Path::new(relative).is_absolute() {
            return Err((
                ErrorCode::InvalidRequest,
                "path must be a non-empty relative path".into(),
            ));
        }
        if std::path::Path::new(relative)
            .components()
            .any(|part| matches!(part, std::path::Component::ParentDir))
        {
            return Err((
                ErrorCode::InvalidRequest,
                "path may not contain `..`".into(),
            ));
        }
        let root = Self::session_root(session_id)?;
        let candidate = root.join(relative);
        if candidate.exists() {
            let canonical = std::fs::canonicalize(&candidate)
                .map_err(|error| (ErrorCode::InvalidRequest, error.to_string()))?;
            if !canonical.starts_with(&root) {
                return Err((
                    ErrorCode::InvalidRequest,
                    "path escapes the session working directory".into(),
                ));
            }
            return Ok(canonical);
        }
        // A missing final path can still be safely statted when its nearest
        // existing ancestor remains under the canonical root.
        let mut ancestor = candidate.as_path();
        while !ancestor.exists() {
            ancestor = ancestor
                .parent()
                .ok_or((ErrorCode::InvalidRequest, "invalid path".into()))?;
        }
        let canonical = std::fs::canonicalize(ancestor)
            .map_err(|error| (ErrorCode::InvalidRequest, error.to_string()))?;
        if !canonical.starts_with(&root) {
            return Err((
                ErrorCode::InvalidRequest,
                "path escapes the session working directory".into(),
            ));
        }
        Ok(candidate)
    }

    fn read_session_file(
        session_id: &str,
        relative: &str,
        max: u64,
    ) -> Result<(String, u64, bool), (ErrorCode, String)> {
        let path = Self::safe_session_path(session_id, relative)?;
        let metadata = path
            .metadata()
            .map_err(|error| (ErrorCode::InvalidRequest, error.to_string()))?;
        if !metadata.is_file() {
            return Err((
                ErrorCode::InvalidRequest,
                "path is not a regular file".into(),
            ));
        }
        let size = metadata.len();
        let file = std::fs::File::open(path)
            .map_err(|error| (ErrorCode::InvalidRequest, error.to_string()))?;
        let mut bytes = Vec::new();
        file.take(max.saturating_add(1))
            .read_to_end(&mut bytes)
            .map_err(|error| (ErrorCode::Internal, error.to_string()))?;
        let truncated = bytes.len() as u64 > max;
        bytes.truncate(max as usize);
        let content = match std::str::from_utf8(&bytes) {
            Ok(content) => content.to_string(),
            Err(error) if truncated && error.error_len().is_none() => {
                String::from_utf8(bytes[..error.valid_up_to()].to_vec())
                    .expect("valid UTF-8 prefix")
            }
            Err(_) => {
                return Err((ErrorCode::InvalidRequest, "file is not UTF-8 text".into()));
            }
        };
        Ok((content, size, truncated))
    }

    fn walk_files(root: &std::path::Path, limit: usize) -> Vec<std::path::PathBuf> {
        let mut files = Vec::new();
        let mut pending = vec![root.to_path_buf()];
        let mut scanned_entries = 0usize;
        while let Some(dir) = pending.pop() {
            let Ok(entries) = std::fs::read_dir(dir) else {
                continue;
            };
            let mut entries: Vec<_> = entries.flatten().collect();
            entries.sort_by_key(|entry| entry.file_name());
            for entry in entries {
                scanned_entries += 1;
                if scanned_entries > MAX_WALK_ENTRIES {
                    return files;
                }
                let name = entry.file_name();
                if matches!(
                    name.to_str(),
                    Some(".git" | "node_modules" | "target" | ".next")
                ) {
                    continue;
                }
                let Ok(kind) = entry.file_type() else {
                    continue;
                };
                if kind.is_symlink() {
                    continue;
                }
                if kind.is_dir() {
                    pending.push(entry.path());
                }
                if kind.is_file() {
                    files.push(entry.path());
                    if files.len() >= limit {
                        return files;
                    }
                }
            }
        }
        files
    }

    fn find_session_files(
        session_id: &str,
        query: &str,
        limit: usize,
    ) -> Result<Vec<String>, (ErrorCode, String)> {
        if limit == 0 {
            return Ok(Vec::new());
        }
        let root = Self::session_root(session_id)?;
        let needle = query.to_ascii_lowercase();
        // Scan a bounded project-wide set before applying the name filter.
        // Stopping at `limit * 10` made a query incorrectly return no results
        // whenever the matching directory happened to sort later in a large
        // repository.
        Ok(Self::walk_files(&root, MAX_WALK_FILES)
            .into_iter()
            .filter_map(|path| {
                let relative = path
                    .strip_prefix(&root)
                    .ok()?
                    .to_string_lossy()
                    .replace('\\', "/");
                relative
                    .to_ascii_lowercase()
                    .contains(&needle)
                    .then_some(relative)
            })
            .take(limit)
            .collect())
    }

    fn search_session_text(
        session_id: &str,
        query: &str,
        under: Option<&str>,
        limit: usize,
    ) -> Result<Vec<TextMatch>, (ErrorCode, String)> {
        if query.is_empty() {
            return Err((ErrorCode::InvalidRequest, "query must not be empty".into()));
        }
        if limit == 0 {
            return Ok(Vec::new());
        }
        let root = Self::session_root(session_id)?;
        let search_root = match under {
            Some(relative) => Self::safe_session_path(session_id, relative)?,
            None => root.clone(),
        };
        let files = if search_root.is_file() {
            vec![search_root]
        } else {
            Self::walk_files(&search_root, MAX_WALK_FILES)
        };
        let mut matches = Vec::new();
        let mut remaining_bytes = MAX_SEARCH_BYTES;
        for path in files {
            let Some(size) = path.metadata().ok().map(|meta| meta.len()) else {
                continue;
            };
            if size > DEFAULT_FILE_BYTES || size > remaining_bytes {
                continue;
            }
            remaining_bytes -= size;
            let Ok(text) = std::fs::read_to_string(&path) else {
                continue;
            };
            for (line_index, line) in text.lines().enumerate() {
                for (byte_column, _) in line.match_indices(query) {
                    matches.push(TextMatch {
                        path: path
                            .strip_prefix(&root)
                            .unwrap_or(&path)
                            .to_string_lossy()
                            .replace('\\', "/"),
                        line: line_index as u32 + 1,
                        column: line[..byte_column].chars().count() as u32 + 1,
                        preview: line.chars().take(500).collect(),
                    });
                    if matches.len() >= limit {
                        return Ok(matches);
                    }
                }
            }
        }
        Ok(matches)
    }

    fn session_file_status(session_id: &str, relative: &str) -> SessionFileStatusResult {
        let path = Self::safe_session_path(session_id, relative)?;
        let Ok(meta) = path.metadata() else {
            return Ok((false, "missing".into(), None, None));
        };
        let kind = if meta.is_file() {
            "file"
        } else if meta.is_dir() {
            "directory"
        } else {
            "other"
        };
        let modified = meta
            .modified()
            .ok()
            .and_then(|time| time.duration_since(UNIX_EPOCH).ok())
            .map(|d| d.as_millis() as u64);
        Ok((
            true,
            kind.into(),
            meta.is_file().then_some(meta.len()),
            modified,
        ))
    }

    fn error_reply(api_id: u64, code: ErrorCode, message: &str) -> Vec<Outbound> {
        vec![Outbound::Reply(ServerFrame::reply(
            api_id,
            ApiEvent::Error {
                code,
                message: message.to_string(),
            },
        ))]
    }

    /// The last `limit` messages of a session, read from its stored record.
    ///
    /// Content blocks are flattened to their text, which is what a preview
    /// wants: a reader glancing at another session needs the words, not the
    /// tool-call structure around them.
    fn stored_tail(session_id: &str, limit: usize) -> Vec<HistoryMessage> {
        let Some(path) = Self::session_record_path(session_id) else {
            return vec![];
        };
        let Ok(text) = std::fs::read_to_string(path) else {
            return vec![];
        };
        let Ok(value) = serde_json::from_str::<Value>(&text) else {
            return vec![];
        };
        let Some(messages) = value["messages"].as_array() else {
            return vec![];
        };
        messages
            .iter()
            .rev()
            .filter_map(|message| {
                let role = message["role"].as_str()?;
                // Only the conversation: a preview of tool traffic would be
                // noise where the point is to recognise which conversation
                // this is.
                if role != "user" && role != "assistant" {
                    return None;
                }
                let content = flatten_content(&message["content"]);
                (!content.trim().is_empty()).then(|| HistoryMessage {
                    role: role.to_string(),
                    content,
                })
            })
            .take(limit)
            .collect::<Vec<_>>()
            .into_iter()
            .rev()
            .collect()
    }

    /// Record the session set the daemon reported, plus any working
    /// directory it mentioned. Kept separate so both the attach probe and an
    /// explicit history request feed the same list.
    fn note_sessions(&mut self, event: &Value) {
        if let Some(all) = event["all_sessions"].as_array() {
            let listed: Vec<String> = all
                .iter()
                .filter_map(|v| v.as_str())
                .map(str::to_string)
                .collect();
            if !listed.is_empty() {
                self.known_sessions = listed;
            }
        }
        if let Some(dir) = event["working_dir"].as_str()
            && let Some(session_id) = event["session_id"].as_str()
        {
            self.session_dirs
                .insert(session_id.to_string(), dir.to_string());
        }
    }

    fn take_simple(&mut self, legacy_id: u64, kind: SimpleKind) -> Option<u64> {
        let index = self
            .pending_simple
            .iter()
            .position(|(id, _, k)| *id == legacy_id && *k == kind)?;
        Some(self.pending_simple.remove(index).1)
    }
}

#[cfg(test)]
#[path = "translate_tests.rs"]
mod tests;
