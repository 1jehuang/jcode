use crate::agent::Agent;
use crate::protocol::ServerEvent;
use crate::provider::Provider;
use crate::server::{
    SessionInterruptQueues, SwarmMember, VersionedPlan, broadcast_swarm_status,
    register_session_interrupt_queue, swarm_id_for_dir,
};
use crate::tool::Registry;
use anyhow::Result;
use std::collections::{HashMap, HashSet};
use std::sync::Arc;
use std::time::Instant;
use tokio::sync::{Mutex, RwLock};

type SessionAgents = Arc<RwLock<HashMap<String, Arc<Mutex<Agent>>>>>;

#[expect(
    clippy::too_many_arguments,
    reason = "headless session creation wires provider, global session, swarm state, interrupts, and MCP pool together"
)]
pub(super) async fn create_headless_session(
    sessions: &SessionAgents,
    global_session_id: &Arc<RwLock<String>>,
    provider_template: &Arc<dyn Provider>,
    command: &str,
    swarm_members: &Arc<RwLock<HashMap<String, SwarmMember>>>,
    swarms_by_id: &Arc<RwLock<HashMap<String, HashSet<String>>>>,
    swarm_coordinators: &Arc<RwLock<HashMap<String, String>>>,
    _swarm_plans: &Arc<RwLock<HashMap<String, VersionedPlan>>>,
    soft_interrupt_queues: &SessionInterruptQueues,
    selfdev_requested: bool,
    model_override: Option<String>,
    mcp_pool: Option<Arc<crate::mcp::SharedMcpPool>>,
    report_back_to_session_id: Option<String>,
) -> Result<String> {
    let memory_enabled = crate::config::config().features.memory;
    let swarm_enabled = crate::config::config().features.swarm;

    let working_dir = if let Some(path_str) = command.strip_prefix("create_session:") {
        let path_str = path_str.trim();
        if !path_str.is_empty() {
            Some(std::path::PathBuf::from(path_str))
        } else {
            None
        }
    } else {
        None
    };

    let provider = provider_template.fork();
    let registry = Registry::new(provider.clone()).await;

    registry.enable_memory_test_mode().await;

    if selfdev_requested {
        registry.register_selfdev_tools().await;
    }

    registry
        .register_mcp_tools(None, mcp_pool, Some("headless".to_string()))
        .await;

    let mut new_agent = Agent::new(Arc::clone(&provider), registry);
    new_agent.set_memory_enabled(memory_enabled);
    let client_session_id = new_agent.session_id().to_string();

    if let Some(model) = model_override
        && let Err(e) = new_agent.set_model(&model)
    {
        crate::logging::warn(&format!(
            "Failed to set headless session model override '{}': {}",
            model, e
        ));
    }

    if let Some(ref dir) = working_dir
        && let Some(path) = dir.to_str()
    {
        new_agent.set_working_dir(path);
    }

    new_agent.set_debug(true);

    if let Some(ref dir) = working_dir {
        if let Some(dir_str) = dir.to_str() {
            new_agent.set_working_dir(dir_str);
        } else {
            new_agent.set_working_dir(&dir.display().to_string());
        }
    }

    if selfdev_requested {
        new_agent.set_canary("self-dev");
    }

    {
        let mut current = global_session_id.write().await;
        if current.is_empty() {
            *current = client_session_id.clone();
        }
    }

    let agent = Arc::new(Mutex::new(new_agent));
    {
        let mut sessions_guard = sessions.write().await;
        sessions_guard.insert(client_session_id.clone(), Arc::clone(&agent));
    }
    {
        let agent_guard = agent.lock().await;
        register_session_interrupt_queue(
            soft_interrupt_queues,
            &client_session_id,
            agent_guard.soft_interrupt_queue(),
        )
        .await;
    }

    let swarm_id = if swarm_enabled {
        swarm_id_for_dir(working_dir.clone())
    } else {
        None
    };
    let friendly_name = crate::id::extract_session_name(&client_session_id)
        .map(|s| s.to_string())
        .unwrap_or_else(|| client_session_id[..8.min(client_session_id.len())].to_string());

    let (event_tx, mut event_rx) = tokio::sync::mpsc::unbounded_channel::<ServerEvent>();
    tokio::spawn(async move {
        while event_rx.recv().await.is_some() {
            // Drain events to keep channel alive
        }
    });

    {
        let now = Instant::now();
        let mut members = swarm_members.write().await;
        members.insert(
            client_session_id.clone(),
            SwarmMember {
                session_id: client_session_id.clone(),
                event_tx: event_tx.clone(),
                event_txs: HashMap::new(),
                working_dir: working_dir.clone(),
                swarm_id: swarm_id.clone(),
                swarm_enabled,
                status: "ready".to_string(),
                detail: None,
                friendly_name: Some(friendly_name.clone()),
                report_back_to_session_id: report_back_to_session_id.clone(),
                role: "agent".to_string(),
                joined_at: now,
                last_status_change: now,
                is_headless: true,
            },
        );
    }

    if let Some(ref id) = swarm_id {
        let mut swarms = swarms_by_id.write().await;
        swarms
            .entry(id.clone())
            .or_insert_with(HashSet::new)
            .insert(client_session_id.clone());
    }

    // Headless sessions never auto-claim coordinator; only TUI-connected sessions do.
    let is_new_coordinator = false;
    let _ = swarm_coordinators;
    if is_new_coordinator {
        let mut members = swarm_members.write().await;
        if let Some(m) = members.get_mut(&client_session_id) {
            m.role = "coordinator".to_string();
        }
    }

    if let Some(ref id) = swarm_id {
        broadcast_swarm_status(id, swarm_members, swarms_by_id).await;
    }

    Ok(serde_json::json!({
        "session_id": client_session_id,
        "working_dir": working_dir,
        "swarm_id": swarm_id,
        "friendly_name": friendly_name,
        "is_canary": selfdev_requested,
    })
    .to_string())
}
