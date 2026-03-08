use super::{
    broadcast_swarm_status, create_headless_session, record_swarm_event, SwarmEvent,
    SwarmEventType, SwarmMember, VersionedPlan,
};
use crate::agent::Agent;
use crate::provider::Provider;
use anyhow::Result;
use std::collections::{HashMap, HashSet};
use std::sync::Arc;
use tokio::sync::{broadcast, Mutex, RwLock};

#[allow(clippy::too_many_arguments)]
pub(super) async fn maybe_handle_session_admin_command(
    cmd: &str,
    sessions: &Arc<RwLock<HashMap<String, Arc<Mutex<Agent>>>>>,
    session_id: &Arc<RwLock<String>>,
    provider: &Arc<dyn Provider>,
    swarm_members: &Arc<RwLock<HashMap<String, SwarmMember>>>,
    swarms_by_id: &Arc<RwLock<HashMap<String, HashSet<String>>>>,
    swarm_coordinators: &Arc<RwLock<HashMap<String, String>>>,
    swarm_plans: &Arc<RwLock<HashMap<String, VersionedPlan>>>,
    event_history: &Arc<RwLock<Vec<SwarmEvent>>>,
    event_counter: &Arc<std::sync::atomic::AtomicU64>,
    swarm_event_tx: &broadcast::Sender<SwarmEvent>,
    mcp_pool: Option<Arc<crate::mcp::SharedMcpPool>>,
) -> Result<Option<String>> {
    if cmd == "create_session" || cmd.starts_with("create_session:") {
        return Ok(Some(
            create_headless_session(
                sessions,
                session_id,
                provider,
                cmd,
                swarm_members,
                swarms_by_id,
                swarm_coordinators,
                swarm_plans,
                None,
                mcp_pool,
            )
            .await?,
        ));
    }

    if cmd.starts_with("destroy_session:") {
        let target_id = cmd.strip_prefix("destroy_session:").unwrap_or("").trim();
        if target_id.is_empty() {
            return Err(anyhow::anyhow!("destroy_session: requires a session_id"));
        }

        let removed_agent = {
            let mut sessions_guard = sessions.write().await;
            sessions_guard.remove(target_id)
        };
        if let Some(ref agent_arc) = removed_agent {
            let agent = agent_arc.lock().await;
            let memory_enabled = agent.memory_enabled();
            let transcript = if memory_enabled {
                Some(agent.build_transcript_for_extraction())
            } else {
                None
            };
            let sid = target_id.to_string();
            drop(agent);
            if let Some(transcript) = transcript {
                crate::memory_agent::trigger_final_extraction(transcript, sid);
            }
        }

        if removed_agent.is_none() {
            return Err(anyhow::anyhow!("Unknown session_id '{}'", target_id));
        }

        let (swarm_id, friendly_name) = {
            let mut members = swarm_members.write().await;
            members
                .remove(target_id)
                .map(|member| (member.swarm_id, member.friendly_name))
                .unwrap_or((None, None))
        };

        if let Some(ref swarm_id) = swarm_id {
            record_swarm_event(
                event_history,
                event_counter,
                swarm_event_tx,
                target_id.to_string(),
                friendly_name.clone(),
                Some(swarm_id.clone()),
                SwarmEventType::StatusChange {
                    old_status: "ready".to_string(),
                    new_status: "stopped".to_string(),
                },
            )
            .await;
            record_swarm_event(
                event_history,
                event_counter,
                swarm_event_tx,
                target_id.to_string(),
                friendly_name,
                Some(swarm_id.clone()),
                SwarmEventType::MemberChange {
                    action: "left".to_string(),
                },
            )
            .await;

            {
                let mut swarms = swarms_by_id.write().await;
                if let Some(swarm) = swarms.get_mut(swarm_id) {
                    swarm.remove(target_id);
                    if swarm.is_empty() {
                        swarms.remove(swarm_id);
                    }
                }
            }

            let was_coordinator = {
                let coordinators = swarm_coordinators.read().await;
                coordinators
                    .get(swarm_id)
                    .map(|coordinator| coordinator == target_id)
                    .unwrap_or(false)
            };
            if was_coordinator {
                let new_coordinator = {
                    let swarms = swarms_by_id.read().await;
                    swarms
                        .get(swarm_id)
                        .and_then(|members| members.iter().min().cloned())
                };
                let mut coordinators = swarm_coordinators.write().await;
                coordinators.remove(swarm_id);
                if let Some(new_id) = new_coordinator {
                    coordinators.insert(swarm_id.clone(), new_id);
                }
            }

            broadcast_swarm_status(swarm_id, swarm_members, swarms_by_id).await;
        }

        return Ok(Some(format!("Session '{}' destroyed", target_id)));
    }

    Ok(None)
}
