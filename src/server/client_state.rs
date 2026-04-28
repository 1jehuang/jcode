use super::ClientConnectionInfo;
use super::server_has_newer_binary;
use crate::agent::Agent;
use crate::bus::Bus;
use crate::message::{ContentBlock, Role};
use crate::protocol::{HistoryMessage, ServerEvent, SessionActivitySnapshot, encode_event};
use crate::provider::Provider;
use crate::session::{Session, SessionStatus};
use crate::transport::WriteHalf;
use anyhow::Result;
use std::collections::{BTreeMap, HashMap};
use std::sync::{Arc, LazyLock, Mutex as StdMutex};
use std::time::{Duration, Instant};
type SessionAgents = Arc<RwLock<HashMap<String, Arc<Mutex<Agent>>>>>;

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(super) enum HistoryPayloadMode {
    Full,
}
use tokio::io::AsyncWriteExt;
use tokio::sync::{Mutex, RwLock};

const ATTACH_MODEL_PREFETCH_DEBOUNCE_SECS: u64 = 15;
const RELOAD_RESTORE_MARKER_MAX_AGE: Duration = Duration::from_secs(60);

static LAST_ATTACH_MODEL_PREFETCH: LazyLock<StdMutex<HashMap<String, Instant>>> =
    LazyLock::new(|| StdMutex::new(HashMap::new()));

fn should_debounce_attach_model_prefetch(provider_name: &str) -> bool {
    let Ok(mut guard) = LAST_ATTACH_MODEL_PREFETCH.lock() else {
        return false;
    };

    let now = Instant::now();
    if let Some(last_run) = guard.get(provider_name)
        && now.duration_since(*last_run) < Duration::from_secs(ATTACH_MODEL_PREFETCH_DEBOUNCE_SECS)
    {
        return true;
    }

    guard.insert(provider_name.to_string(), now);
    false
}

pub(super) async fn handle_get_state(
    id: u64,
    client_session_id: &str,
    client_is_processing: bool,
    sessions: &SessionAgents,
    writer: &Arc<Mutex<WriteHalf>>,
) -> Result<()> {
    let session_count = {
        let sessions_guard = sessions.read().await;
        sessions_guard.len()
    };

    write_event(
        writer,
        &ServerEvent::State {
            id,
            session_id: client_session_id.to_string(),
            message_count: session_count,
            is_processing: client_is_processing,
        },
    )
    .await
}

#[expect(
    clippy::too_many_arguments,
    reason = "history fetch needs session state, client activity, provider handle, and server identity metadata"
)]
pub(super) async fn handle_get_history(
    id: u64,
    client_session_id: &str,
    client_is_processing: bool,
    agent: &Arc<Mutex<Agent>>,
    provider: &Arc<dyn Provider>,
    sessions: &SessionAgents,
    client_connections: &Arc<RwLock<HashMap<String, ClientConnectionInfo>>>,
    client_count: &Arc<RwLock<usize>>,
    writer: &Arc<Mutex<WriteHalf>>,
    server_name: &str,
    server_icon: &str,
    was_interrupted: Option<bool>,
) -> Result<()> {
    let history_start = Instant::now();
    let activity =
        session_activity_snapshot(client_connections, client_session_id, client_is_processing)
            .await;

    if agent.try_lock().is_err() {
        crate::logging::info(&format!(
            "handle_get_history: session {} busy, falling back to persisted remote-startup snapshot",
            client_session_id
        ));
        send_history_from_persisted_session(
            id,
            client_session_id,
            provider,
            sessions,
            client_count,
            writer,
            server_name,
            server_icon,
            was_interrupted,
            activity,
        )
        .await?;
        crate::logging::info(&format!(
            "[TIMING] handle_get_history: session={}, persisted_fallback total={}ms",
            client_session_id,
            history_start.elapsed().as_millis(),
        ));
        return Ok(());
    }

    send_history(
        id,
        client_session_id,
        agent,
        sessions,
        client_count,
        writer,
        server_name,
        server_icon,
        was_interrupted,
        activity,
        HistoryPayloadMode::Full,
        true,
    )
    .await?;
    let send_history_ms = history_start.elapsed().as_millis();

    let prefetch_start = Instant::now();
    spawn_model_prefetch_update(Arc::clone(provider), Arc::clone(agent));
    crate::logging::info(&format!(
        "[TIMING] handle_get_history: session={}, send_history={}ms, prefetch_spawn={}ms, total={}ms",
        client_session_id,
        send_history_ms,
        prefetch_start.elapsed().as_millis(),
        history_start.elapsed().as_millis(),
    ));
    Ok(())
}

pub(super) async fn handle_get_compacted_history(
    id: u64,
    session_id: &str,
    agent: &Arc<Mutex<Agent>>,
    writer: &Arc<Mutex<WriteHalf>>,
    visible_messages: usize,
) -> Result<()> {
    let started = Instant::now();
    let (messages, images, compacted_info, source) = match agent.try_lock() {
        Ok(agent_guard) => {
            let (messages, images, info) = agent_guard
                .get_history_and_rendered_images_with_compacted_history(visible_messages);
            (messages, images, info, "live")
        }
        Err(_) => {
            let session = crate::session::Session::load_for_remote_startup(session_id)
                .or_else(|_| crate::session::Session::load_startup_stub(session_id))?;
            let (rendered_messages, images, info) =
                crate::session::render_messages_and_images_with_compacted_history(
                    &session,
                    visible_messages,
                );
            (
                rendered_messages
                    .into_iter()
                    .map(rendered_to_history_message)
                    .collect(),
                images,
                info,
                "persisted",
            )
        }
    };

    let compacted_info = compacted_info.unwrap_or(crate::session::RenderedCompactedHistoryInfo {
        total_messages: 0,
        visible_messages: 0,
        remaining_messages: 0,
    });
    crate::logging::info(&format!(
        "[TIMING] get_compacted_history: session={}, source={}, requested={}, visible={}, remaining={}, messages={}, total={}ms",
        session_id,
        source,
        visible_messages,
        compacted_info.visible_messages,
        compacted_info.remaining_messages,
        messages.len(),
        started.elapsed().as_millis(),
    ));

    write_event(
        writer,
        &ServerEvent::CompactedHistory {
            id,
            session_id: session_id.to_string(),
            messages,
            images,
            compacted_total: compacted_info.total_messages,
            compacted_visible: compacted_info.visible_messages,
            compacted_remaining: compacted_info.remaining_messages,
        },
    )
    .await
}

fn rendered_to_history_message(msg: crate::session::RenderedMessage) -> HistoryMessage {
    HistoryMessage {
        role: msg.role,
        content: msg.content,
        tool_calls: if msg.tool_calls.is_empty() {
            None
        } else {
            Some(msg.tool_calls)
        },
        tool_data: msg.tool_data,
    }
}

fn history_reload_recovery_snapshot(
    session_id: &str,
    was_interrupted: Option<bool>,
) -> Option<crate::protocol::ReloadRecoverySnapshot> {
    match super::reload_recovery::claim_pending_for_session(session_id) {
        Ok(Some(directive)) => {
            crate::logging::info(&format!(
                "history_reload_recovery_snapshot: using server-owned recovery intent for session={}",
                session_id
            ));
            return Some(directive);
        }
        Ok(None) => {}
        Err(err) => crate::logging::warn(&format!(
            "history_reload_recovery_snapshot: failed to claim server-owned recovery intent for session={}: {}",
            session_id, err
        )),
    }

    let reload_ctx = crate::tool::selfdev::ReloadContext::peek_for_session(session_id)
        .ok()
        .flatten();
    let inferred_interrupted = was_interrupted
        .unwrap_or_else(|| infer_persisted_session_interrupted_by_reload(session_id));
    let directive = crate::tool::selfdev::ReloadContext::recovery_directive_for_session(
        session_id,
        reload_ctx.as_ref(),
        inferred_interrupted,
        None,
    );
    crate::logging::info(&format!(
        "history_reload_recovery_snapshot: session={} explicit_was_interrupted={:?} inferred_was_interrupted={} has_reload_ctx={} directive={}",
        session_id,
        was_interrupted,
        inferred_interrupted,
        reload_ctx.is_some(),
        directive.is_some()
    ));
    directive
}

fn persisted_session_has_reload_interruption_marker(session: &Session) -> bool {
    let Some(last) = session.messages.last() else {
        return false;
    };

    last.content.iter().any(|block| match block {
        ContentBlock::Text { text, .. } => {
            text.ends_with("[generation interrupted - server reloading]")
        }
        ContentBlock::ToolResult {
            content, is_error, ..
        } => {
            content == "Reload initiated. Process restarting..."
                || (is_error.unwrap_or(false)
                    && (content.contains("interrupted by server reload")
                        || content.contains("Skipped - server reloading")))
        }
        _ => false,
    })
}

fn infer_persisted_session_interrupted_by_reload(session_id: &str) -> bool {
    let session = match Session::load_for_remote_startup(session_id)
        .or_else(|_| Session::load_startup_stub(session_id))
    {
        Ok(session) => session,
        Err(err) => {
            crate::logging::warn(&format!(
                "history_reload_recovery_snapshot: could not inspect persisted session {} for reload interruption fallback: {}",
                session_id, err
            ));
            return false;
        }
    };

    let last_is_user = session
        .messages
        .last()
        .map(|message| message.role == Role::User)
        .unwrap_or(false);
    let marker_active = crate::server::reload_marker_active(RELOAD_RESTORE_MARKER_MAX_AGE);
    let interrupted = matches!(session.status, SessionStatus::Crashed { .. })
        || (matches!(session.status, SessionStatus::Active) && last_is_user && marker_active)
        || (matches!(session.status, SessionStatus::Closed) && last_is_user && marker_active)
        || persisted_session_has_reload_interruption_marker(&session);

    crate::logging::info(&format!(
        "history_reload_recovery_snapshot: fallback inspect session={} status={} last_is_user={} marker_active={} interrupted={}",
        session_id,
        session.status.display(),
        last_is_user,
        marker_active,
        interrupted
    ));

    interrupted
}

#[expect(
    clippy::too_many_arguments,
    reason = "persisted history fallback still needs session/client/server metadata for a usable bootstrap payload"
)]
async fn send_history_from_persisted_session(
    id: u64,
    session_id: &str,
    provider: &Arc<dyn Provider>,
    sessions: &SessionAgents,
    client_count: &Arc<RwLock<usize>>,
    writer: &Arc<Mutex<WriteHalf>>,
    server_name: &str,
    server_icon: &str,
    was_interrupted: Option<bool>,
    activity: Option<SessionActivitySnapshot>,
) -> Result<()> {
    let session = crate::session::Session::load_for_remote_startup(session_id)
        .or_else(|_| crate::session::Session::load_startup_stub(session_id))?;
    let (rendered_messages, images) = crate::session::render_messages_and_images(&session);
    let messages = rendered_messages
        .into_iter()
        .map(|msg| crate::protocol::HistoryMessage {
            role: msg.role,
            content: msg.content,
            tool_calls: if msg.tool_calls.is_empty() {
                None
            } else {
                Some(msg.tool_calls)
            },
            tool_data: msg.tool_data,
        })
        .collect();
    let side_panel = crate::side_panel::snapshot_for_session(session_id).unwrap_or_default();

    let (all_sessions, current_client_count) = {
        let sessions_guard = sessions.read().await;
        let mut all: Vec<String> = sessions_guard.keys().cloned().collect();
        all.sort();
        let count = *client_count.read().await;
        (all, count)
    };

    let history_event = ServerEvent::History {
        id,
        session_id: session_id.to_string(),
        messages,
        images,
        provider_name: Some(provider.name().to_string()),
        provider_model: session.model.clone().or_else(|| Some(provider.model())),
        subagent_model: session.subagent_model.clone(),
        autoreview_enabled: session.autoreview_enabled,
        autojudge_enabled: session.autojudge_enabled,
        available_models: Vec::new(),
        available_model_routes: Vec::new(),
        mcp_servers: Vec::new(),
        skills: Vec::new(),
        total_tokens: None,
        all_sessions,
        client_count: Some(current_client_count),
        is_canary: Some(session.is_canary),
        server_version: Some(env!("JCODE_VERSION").to_string()),
        server_name: Some(server_name.to_string()),
        server_icon: Some(server_icon.to_string()),
        server_has_update: Some(server_has_newer_binary()),
        was_interrupted,
        reload_recovery: history_reload_recovery_snapshot(session_id, was_interrupted),
        connection_type: None,
        status_detail: None,
        upstream_provider: None,
        reasoning_effort: None,
        service_tier: None,
        compaction_mode: crate::config::config().compaction.mode.clone(),
        activity,
        side_panel,
    };

    write_event(writer, &history_event).await
}

#[expect(
    clippy::too_many_arguments,
    reason = "history payload assembly includes agent state, sessions, counts, writer, activity, payload mode, and server identity"
)]
pub(super) async fn send_history(
    id: u64,
    session_id: &str,
    agent: &Arc<Mutex<Agent>>,
    sessions: &SessionAgents,
    client_count: &Arc<RwLock<usize>>,
    writer: &Arc<Mutex<WriteHalf>>,
    server_name: &str,
    server_icon: &str,
    was_interrupted: Option<bool>,
    activity: Option<SessionActivitySnapshot>,
    payload_mode: HistoryPayloadMode,
    include_model_catalog: bool,
) -> Result<()> {
    let history_start = Instant::now();
    let agent_lock_start = Instant::now();
    let (
        messages,
        images,
        is_canary,
        provider_name,
        provider_model,
        subagent_model,
        autoreview_enabled,
        autojudge_enabled,
        available_models,
        available_model_routes,
        skills,
        tool_names,
        upstream_provider,
        connection_type,
        status_detail,
        reasoning_effort,
        service_tier,
        compaction_mode,
        agent_lock_ms,
        history_snapshot_ms,
        image_render_ms,
        tool_names_ms,
        available_models_ms,
        model_routes_ms,
        skills_ms,
        provider_meta_ms,
        compaction_mode_ms,
    ) = {
        let agent_guard = agent.lock().await;
        let agent_lock_ms = agent_lock_start.elapsed().as_millis();
        let provider = agent_guard.provider_handle();

        let history_snapshot_start = Instant::now();
        let (messages, images) = agent_guard.get_history_and_rendered_images();
        let history_snapshot_ms = history_snapshot_start.elapsed().as_millis();
        let image_render_ms = 0;

        let tool_names_start = Instant::now();
        let tool_names = agent_guard.tool_names().await;
        let tool_names_ms = tool_names_start.elapsed().as_millis();

        let (available_models, available_models_ms) = if include_model_catalog {
            let available_models_start = Instant::now();
            let available_models = agent_guard.available_models_display();
            (
                available_models,
                available_models_start.elapsed().as_millis(),
            )
        } else {
            (Vec::new(), 0)
        };

        // Model-route expansion can be relatively expensive (provider/account routing,
        // endpoint cache reads, etc.). The TUI already supports later
        // AvailableModelsUpdated events, so keep the initial History payload fast and
        // let the background refresh populate detailed routes asynchronously.
        let available_model_routes = Vec::new();
        let model_routes_ms = 0;

        let skills_start = Instant::now();
        let skills = agent_guard.available_skill_names();
        let skills_ms = skills_start.elapsed().as_millis();

        let provider_meta_start = Instant::now();
        let reasoning_effort = provider.reasoning_effort();
        let service_tier = provider.service_tier();
        let provider_meta_ms = provider_meta_start.elapsed().as_millis();

        let compaction_mode_start = Instant::now();
        let compaction_mode = agent_guard.compaction_mode().await;
        let compaction_mode_ms = compaction_mode_start.elapsed().as_millis();

        (
            messages,
            images,
            agent_guard.is_canary(),
            agent_guard.provider_name(),
            agent_guard.provider_model(),
            agent_guard.subagent_model(),
            agent_guard.autoreview_enabled(),
            agent_guard.autojudge_enabled(),
            available_models,
            available_model_routes,
            skills,
            tool_names,
            agent_guard.last_upstream_provider(),
            agent_guard.last_connection_type(),
            agent_guard.last_status_detail(),
            reasoning_effort,
            service_tier,
            compaction_mode,
            agent_lock_ms,
            history_snapshot_ms,
            image_render_ms,
            tool_names_ms,
            available_models_ms,
            model_routes_ms,
            skills_ms,
            provider_meta_ms,
            compaction_mode_ms,
        )
    };

    let side_panel_start = Instant::now();
    let side_panel = crate::side_panel::snapshot_for_session(session_id).unwrap_or_default();
    let side_panel_ms = side_panel_start.elapsed().as_millis();

    let mut mcp_map: BTreeMap<String, usize> = BTreeMap::new();
    for name in &tool_names {
        if let Some(rest) = name.strip_prefix("mcp__")
            && let Some((server, _tool)) = rest.split_once("__")
        {
            *mcp_map.entry(server.to_string()).or_default() += 1;
        }
    }
    let mcp_servers: Vec<String> = mcp_map
        .into_iter()
        .map(|(name, count)| format!("{name}:{count}"))
        .collect();

    let (all_sessions, current_client_count) = {
        let sessions_snapshot_start = Instant::now();
        let sessions_guard = sessions.read().await;
        let all: Vec<String> = sessions_guard.keys().cloned().collect();
        let count = *client_count.read().await;
        let sessions_snapshot_ms = sessions_snapshot_start.elapsed().as_millis();
        crate::logging::info(&format!(
            "[TIMING] send_history prep: session={}, mode={:?}, messages={}, images={}, mcp_servers={}, agent_lock={}ms, history={}ms, images={}ms, tool_names={}ms, models={}ms, routes={}ms, skills={}ms, provider_meta={}ms, compaction={}ms, side_panel={}ms, sessions={}ms, total={}ms",
            session_id,
            payload_mode,
            messages.len(),
            images.len(),
            mcp_servers.len(),
            agent_lock_ms,
            history_snapshot_ms,
            image_render_ms,
            tool_names_ms,
            available_models_ms,
            model_routes_ms,
            skills_ms,
            provider_meta_ms,
            compaction_mode_ms,
            side_panel_ms,
            sessions_snapshot_ms,
            history_start.elapsed().as_millis(),
        ));
        (all, count)
    };

    let history_event = ServerEvent::History {
        id,
        session_id: session_id.to_string(),
        messages,
        images,
        provider_name: Some(provider_name),
        provider_model: Some(provider_model),
        subagent_model,
        autoreview_enabled,
        autojudge_enabled,
        available_models,
        available_model_routes,
        mcp_servers,
        skills,
        total_tokens: None,
        all_sessions,
        client_count: Some(current_client_count),
        is_canary: Some(is_canary),
        server_version: Some(env!("JCODE_VERSION").to_string()),
        server_name: Some(server_name.to_string()),
        server_icon: Some(server_icon.to_string()),
        server_has_update: Some(server_has_newer_binary()),
        was_interrupted,
        reload_recovery: history_reload_recovery_snapshot(session_id, was_interrupted),
        connection_type,
        status_detail,
        upstream_provider,
        reasoning_effort,
        service_tier,
        compaction_mode,
        activity,
        side_panel,
    };
    let encode_start = Instant::now();
    let json = encode_event(&history_event);
    let encode_ms = encode_start.elapsed().as_millis();
    let writer_lock_start = Instant::now();
    let mut writer_guard = writer.lock().await;
    let writer_lock_ms = writer_lock_start.elapsed().as_millis();
    let write_start = Instant::now();
    let result = writer_guard.write_all(json.as_bytes()).await;
    let write_ms = write_start.elapsed().as_millis();

    crate::logging::info(&format!(
        "[TIMING] send_history write: session={}, bytes={}, encode={}ms, writer_lock={}ms, write={}ms, total={}ms",
        session_id,
        json.len(),
        encode_ms,
        writer_lock_ms,
        write_ms,
        history_start.elapsed().as_millis(),
    ));

    result.map_err(Into::into)
}

pub(super) async fn session_activity_snapshot(
    client_connections: &Arc<RwLock<HashMap<String, ClientConnectionInfo>>>,
    session_id: &str,
    fallback_processing: bool,
) -> Option<SessionActivitySnapshot> {
    let snapshot = {
        let connections = client_connections.read().await;
        let mut processing_without_tool = false;
        let mut tool_name = None;
        for info in connections.values() {
            if info.session_id != session_id || !info.is_processing {
                continue;
            }
            if let Some(current_tool_name) = info.current_tool_name.clone() {
                tool_name = Some(current_tool_name);
                break;
            }
            processing_without_tool = true;
        }

        tool_name
            .map(|current_tool_name| SessionActivitySnapshot {
                is_processing: true,
                current_tool_name: Some(current_tool_name),
            })
            .or_else(|| {
                processing_without_tool.then_some(SessionActivitySnapshot {
                    is_processing: true,
                    current_tool_name: None,
                })
            })
    };

    snapshot.or_else(|| {
        fallback_processing.then_some(SessionActivitySnapshot {
            is_processing: true,
            current_tool_name: None,
        })
    })
}

async fn write_event(writer: &Arc<Mutex<WriteHalf>>, event: &ServerEvent) -> Result<()> {
    let json = encode_event(event);
    let mut writer = writer.lock().await;
    writer.write_all(json.as_bytes()).await?;
    Ok(())
}

pub(super) fn spawn_model_prefetch_update(provider: Arc<dyn Provider>, agent: Arc<Mutex<Agent>>) {
    tokio::spawn(async move {
        let (provider_name, initial_models) = {
            let agent_guard = agent.lock().await;
            (
                agent_guard.provider_name(),
                agent_guard.available_models_display(),
            )
        };

        if !initial_models.is_empty() {
            return;
        }

        if should_debounce_attach_model_prefetch(&provider_name) {
            crate::logging::info(&format!(
                "Skipping attach-time model prefetch for {} because a recent refresh already ran",
                provider_name
            ));
            return;
        }

        if provider.prefetch_models().await.is_err() {
            return;
        }

        let refreshed = {
            let agent_guard = agent.lock().await;
            (
                agent_guard.available_models_display(),
                agent_guard.model_routes(),
            )
        };

        if refreshed.0 == initial_models && refreshed.1.is_empty() {
            return;
        }

        let _ = refreshed;
        Bus::global().publish_models_updated();
    });
}

#[cfg(test)]
#[path = "client_state_tests.rs"]
mod client_state_tests;
