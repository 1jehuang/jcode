//! Read-only handlers backing the remote session picker (`/resume` over SSH).
//!
//! `list_sessions` returns summary metadata for every daemon-owned session so
//! a remote TUI can render the picker without reading server-side storage.
//! `session_preview` lazily returns a bounded message preview for the row the
//! user is inspecting. Both mirror the local picker semantics: sessions are
//! sorted by last activity descending, imported transcripts are excluded (the
//! picker groups those under their own external sources), and empty sessions
//! are hidden.

use super::ClientConnectionInfo;
use crate::protocol::{ServerEvent, SessionListEntry, SessionPreviewMessage};
use crate::session::Session;
use crate::util::truncate_str;
use jcode_message_types::Role;
use std::collections::{HashMap, HashSet};
use std::path::Path;
use std::sync::Arc;
use tokio::sync::{RwLock, mpsc};

type SessionAgents = Arc<RwLock<HashMap<String, Arc<tokio::sync::Mutex<crate::agent::Agent>>>>>;
type ClientConnections = Arc<RwLock<HashMap<String, ClientConnectionInfo>>>;

/// Preview rows per `session_preview` request. Mirrors the local picker's
/// `build_messages_preview` bound.
const REMOTE_PREVIEW_MESSAGES: usize = 20;
/// Cap for `first_user_prompt` so the wire payload stays compact.
const FIRST_PROMPT_MAX_CHARS: usize = 200;

/// Reply to `list_sessions`. Snapshots live-session state up front, then scans
/// the sessions directory on the blocking pool and replies with one
/// `SessionList` event.
pub(super) async fn handle_list_sessions(
    id: u64,
    sessions: &SessionAgents,
    client_connections: &ClientConnections,
    client_event_tx: &mpsc::UnboundedSender<ServerEvent>,
) {
    let live_attached: HashSet<String> = sessions.read().await.keys().cloned().collect();
    let live_processing: HashSet<String> = client_connections
        .read()
        .await
        .values()
        .filter(|info| info.is_processing)
        .map(|info| info.session_id.clone())
        .collect();

    let tx = client_event_tx.clone();
    tokio::task::spawn_blocking(move || {
        let sessions = collect_session_list_entries(&live_attached, &live_processing);
        let _ = tx.send(ServerEvent::SessionList { id, sessions });
    });
}

/// Reply to `session_preview` with a bounded, rendered preview for one
/// session. An empty `messages` vec means the session could not be loaded.
pub(super) fn handle_session_preview(
    id: u64,
    session_id: String,
    client_event_tx: &mpsc::UnboundedSender<ServerEvent>,
) {
    let tx = client_event_tx.clone();
    tokio::task::spawn_blocking(move || {
        let messages = Session::load(&session_id)
            .map(|session| build_remote_preview(&session))
            .unwrap_or_default();
        let _ = tx.send(ServerEvent::SessionPreviewResult {
            id,
            session_id,
            messages,
        });
    });
}

/// Reply to `set_session_saved`. Loads the session file on the server, updates
/// the saved flag/label, and persists it — the remote client cannot write the
/// server's session directory directly.
pub(super) fn handle_set_session_saved(
    id: u64,
    session_id: String,
    saved: bool,
    save_label: Option<String>,
    client_event_tx: &mpsc::UnboundedSender<ServerEvent>,
) {
    let tx = client_event_tx.clone();
    tokio::task::spawn_blocking(move || {
        let event = match update_session_saved(&session_id, saved, save_label) {
            Ok(()) => ServerEvent::Ack { id },
            Err(error) => ServerEvent::Error {
                id,
                message: format!("set_session_saved failed: {error}"),
                retry_after_secs: None,
            },
        };
        let _ = tx.send(event);
    });
}

fn update_session_saved(
    session_id: &str,
    saved: bool,
    save_label: Option<String>,
) -> anyhow::Result<()> {
    let mut session = Session::load(session_id)?;
    if saved {
        session.mark_saved(save_label);
    } else {
        session.unmark_saved();
    }
    session.save()?;
    Ok(())
}

/// Reply to `get_todos` with the session's stored todo items. An empty vec
/// means the session has no saved todo state.
pub(super) fn handle_get_todos(
    id: u64,
    session_id: String,
    client_event_tx: &mpsc::UnboundedSender<ServerEvent>,
) {
    let tx = client_event_tx.clone();
    tokio::task::spawn_blocking(move || {
        let todos = crate::todo::load_todos(&session_id).unwrap_or_default();
        let goals = crate::todo::load_goals(&session_id).unwrap_or_default();
        let plan = crate::todo::load_plan(&session_id).ok();
        let _ = tx.send(ServerEvent::Todos {
            id,
            todos,
            goals,
            plan,
        });
    });
}

fn collect_session_list_entries(
    live_attached: &HashSet<String>,
    live_processing: &HashSet<String>,
) -> Vec<SessionListEntry> {
    let Ok(sessions_dir) = crate::storage::jcode_dir().map(|dir| dir.join("sessions")) else {
        return Vec::new();
    };
    let Ok(read_dir) = std::fs::read_dir(&sessions_dir) else {
        return Vec::new();
    };

    let mut entries: Vec<SessionListEntry> = Vec::new();
    for dir_entry in read_dir.flatten() {
        let path = dir_entry.path();
        if !path.is_file() || !is_session_snapshot(&path) {
            continue;
        }
        let Some(stem) = path.file_stem().and_then(|s| s.to_str()) else {
            continue;
        };
        // Imported external transcripts are surfaced by the picker's own
        // external-source scans; listing them here would double-report them.
        if stem.starts_with("imported_") {
            continue;
        }
        if let Some(entry) = session_list_entry(stem, live_attached, live_processing) {
            entries.push(entry);
        }
    }

    entries.sort_by(|a, b| b.last_message_time.cmp(&a.last_message_time));
    entries
}

fn is_session_snapshot(path: &Path) -> bool {
    path.extension().and_then(|ext| ext.to_str()) == Some("json")
        // `<id>.json.pre-wipe-*.bak` files end in `.bak`, not `.json`, but a
        // stem like `session_x.json` also parses here; keep it defensive.
        && path
            .file_name()
            .and_then(|name| name.to_str())
            .is_some_and(|name| !name.contains(".pre-wipe-"))
}

fn session_list_entry(
    session_id: &str,
    live_attached: &HashSet<String>,
    live_processing: &HashSet<String>,
) -> Option<SessionListEntry> {
    let session = Session::load(session_id).ok()?;
    let visible = session.visible_conversation_messages();
    if visible.is_empty() {
        return None;
    }

    let mut user_message_count = 0usize;
    let mut assistant_message_count = 0usize;
    let mut estimated_tokens = 0usize;
    let mut first_user_prompt: Option<String> = None;
    for message in &visible {
        match message.role {
            Role::User => {
                user_message_count += 1;
                if first_user_prompt.is_none() {
                    first_user_prompt = Some(
                        truncate_str(message.content_preview().trim(), FIRST_PROMPT_MAX_CHARS)
                            .to_string(),
                    );
                }
            }
            Role::Assistant => assistant_message_count += 1,
        }
        if let Some(usage) = &message.token_usage {
            estimated_tokens = estimated_tokens
                .saturating_add(usage.input_tokens as usize)
                .saturating_add(usage.output_tokens as usize);
        }
    }

    // Title precedence mirrors the local picker: custom rename, then the
    // todo-derived session title, then the generated title, then short name.
    let short_name = session
        .short_name
        .clone()
        .or_else(|| crate::id::extract_session_name(session_id).map(str::to_string))
        .unwrap_or_else(|| session_id.to_string());
    let title = session
        .custom_title
        .clone()
        .or_else(|| {
            crate::todo::load_session_title(session_id)
                .map(|title| truncate_str(title.trim(), 72).to_string())
        })
        .or_else(|| session.title.clone())
        .unwrap_or_else(|| short_name.clone());

    Some(SessionListEntry {
        id: session_id.to_string(),
        parent_id: session.parent_id.clone(),
        short_name: Some(short_name),
        title,
        message_count: visible.len(),
        user_message_count,
        assistant_message_count,
        created_at: session.created_at,
        last_message_time: session.updated_at,
        last_active_at: session.last_active_at,
        working_dir: session.working_dir.clone(),
        model: session.model.clone(),
        provider_key: session.provider_key.clone(),
        is_canary: session.is_canary,
        is_debug: session.is_debug,
        saved: session.saved,
        save_label: session.save_label.clone(),
        status: session.status.clone(),
        estimated_tokens,
        first_user_prompt,
        live_attached: live_attached.contains(session_id),
        live_processing: live_processing.contains(session_id),
    })
}

fn build_remote_preview(session: &Session) -> Vec<SessionPreviewMessage> {
    crate::session::render_messages(session)
        .into_iter()
        .rev()
        .take(REMOTE_PREVIEW_MESSAGES)
        .rev()
        .map(|message| SessionPreviewMessage {
            role: message.role,
            content: message.content,
            tool_calls: message.tool_calls,
            timestamp: None,
        })
        .collect()
}
