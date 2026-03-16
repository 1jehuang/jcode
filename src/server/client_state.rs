use super::server_has_newer_binary;
use crate::agent::Agent;
use crate::protocol::{ServerEvent, encode_event};
use crate::provider::Provider;
use crate::transport::WriteHalf;
use anyhow::Result;
use std::collections::{BTreeMap, HashMap};
use std::sync::Arc;
use tokio::io::AsyncWriteExt;
use tokio::sync::{Mutex, RwLock};

pub(super) async fn handle_get_state(
    id: u64,
    client_session_id: &str,
    client_is_processing: bool,
    sessions: &Arc<RwLock<HashMap<String, Arc<Mutex<Agent>>>>>,
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

pub(super) async fn handle_get_history(
    id: u64,
    client_session_id: &str,
    agent: &Arc<Mutex<Agent>>,
    provider: &Arc<dyn Provider>,
    sessions: &Arc<RwLock<HashMap<String, Arc<Mutex<Agent>>>>>,
    client_count: &Arc<RwLock<usize>>,
    writer: &Arc<Mutex<WriteHalf>>,
    server_name: &str,
    server_icon: &str,
) -> Result<()> {
    send_history(
        id,
        client_session_id,
        agent,
        sessions,
        client_count,
        writer,
        server_name,
        server_icon,
        None,
    )
    .await?;

    spawn_model_prefetch_update(Arc::clone(provider), Arc::clone(agent), Arc::clone(writer));
    Ok(())
}

pub(super) async fn send_history(
    id: u64,
    session_id: &str,
    agent: &Arc<Mutex<Agent>>,
    sessions: &Arc<RwLock<HashMap<String, Arc<Mutex<Agent>>>>>,
    client_count: &Arc<RwLock<usize>>,
    writer: &Arc<Mutex<WriteHalf>>,
    server_name: &str,
    server_icon: &str,
    was_interrupted: Option<bool>,
) -> Result<()> {
    let (
        messages,
        is_canary,
        provider_name,
        provider_model,
        available_models,
        available_model_routes,
        tool_names,
        upstream_provider,
        connection_type,
        reasoning_effort,
        service_tier,
        compaction_mode,
        side_panel,
    ) = {
        let agent_guard = agent.lock().await;
        let provider = agent_guard.provider_handle();
        (
            agent_guard.get_history(),
            agent_guard.is_canary(),
            agent_guard.provider_name(),
            agent_guard.provider_model(),
            agent_guard.available_models_display(),
            agent_guard.model_routes(),
            agent_guard.tool_names().await,
            agent_guard.last_upstream_provider(),
            agent_guard.last_connection_type(),
            provider.reasoning_effort(),
            provider.service_tier(),
            agent_guard.compaction_mode().await,
            crate::side_panel::snapshot_for_session(session_id).unwrap_or_default(),
        )
    };

    let mut mcp_map: BTreeMap<String, usize> = BTreeMap::new();
    for name in &tool_names {
        if let Some(rest) = name.strip_prefix("mcp__") {
            if let Some((server, _tool)) = rest.split_once("__") {
                *mcp_map.entry(server.to_string()).or_default() += 1;
            }
        }
    }
    let mcp_servers: Vec<String> = mcp_map
        .into_iter()
        .map(|(name, count)| format!("{name}:{count}"))
        .collect();

    let skills = crate::skill::SkillRegistry::load()
        .map(|registry| {
            registry
                .list()
                .iter()
                .map(|skill| skill.name.clone())
                .collect()
        })
        .unwrap_or_default();

    let (all_sessions, current_client_count) = {
        let sessions_guard = sessions.read().await;
        let all: Vec<String> = sessions_guard.keys().cloned().collect();
        let count = *client_count.read().await;
        (all, count)
    };

    write_event(
        writer,
        &ServerEvent::History {
            id,
            session_id: session_id.to_string(),
            messages,
            provider_name: Some(provider_name),
            provider_model: Some(provider_model),
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
            connection_type,
            upstream_provider,
            reasoning_effort,
            service_tier,
            compaction_mode,
            side_panel,
        },
    )
    .await
}

async fn write_event(writer: &Arc<Mutex<WriteHalf>>, event: &ServerEvent) -> Result<()> {
    let json = encode_event(event);
    let mut writer = writer.lock().await;
    writer.write_all(json.as_bytes()).await?;
    Ok(())
}

pub(super) fn spawn_model_prefetch_update(
    provider: Arc<dyn Provider>,
    agent: Arc<Mutex<Agent>>,
    writer: Arc<Mutex<WriteHalf>>,
) {
    tokio::spawn(async move {
        if provider.prefetch_models().await.is_err() {
            return;
        }

        let (available_models, available_model_routes) = {
            let agent_guard = agent.lock().await;
            (
                agent_guard.available_models_display(),
                agent_guard.model_routes(),
            )
        };

        let _ = write_event(
            &writer,
            &ServerEvent::AvailableModelsUpdated {
                available_models,
                available_model_routes,
            },
        )
        .await;
    });
}
