use super::{
    SessionInterruptQueues, SwarmEvent, SwarmEventType, SwarmMember, VersionedPlan,
    broadcast_swarm_status, create_headless_session, record_swarm_event,
    remove_session_interrupt_queue,
};
use crate::agent::Agent;
use crate::provider::Provider;
use anyhow::Result;
use std::collections::{HashMap, HashSet};
use std::sync::Arc;
use tokio::sync::{Mutex, RwLock, broadcast};

type SessionAgents = Arc<RwLock<HashMap<String, Arc<Mutex<Agent>>>>>;

fn parse_create_session_command(cmd: &str) -> Option<(Option<String>, bool)> {
    if cmd == "create_session" {
        return Some((None, false));
    }

    if let Some(rest) = cmd.strip_prefix("create_session:selfdev:") {
        let working_dir = rest.trim();
        return Some((
            if working_dir.is_empty() {
                None
            } else {
                Some(working_dir.to_string())
            },
            true,
        ));
    }

    if cmd == "create_session:selfdev" {
        return Some((None, true));
    }

    if let Some(rest) = cmd.strip_prefix("create_session:") {
        let working_dir = rest.trim();
        return Some((
            if working_dir.is_empty() {
                None
            } else {
                Some(working_dir.to_string())
            },
            false,
        ));
    }

    None
}

#[expect(
    clippy::too_many_arguments,
    reason = "session admin debug commands need sessions, swarm state, provider template, queues, and event history"
)]
pub(super) async fn maybe_handle_session_admin_command(
    cmd: &str,
    sessions: &SessionAgents,
    session_id: &Arc<RwLock<String>>,
    provider: &Arc<dyn Provider>,
    swarm_members: &Arc<RwLock<HashMap<String, SwarmMember>>>,
    swarms_by_id: &Arc<RwLock<HashMap<String, HashSet<String>>>>,
    swarm_coordinators: &Arc<RwLock<HashMap<String, String>>>,
    swarm_plans: &Arc<RwLock<HashMap<String, VersionedPlan>>>,
    event_history: &Arc<RwLock<std::collections::VecDeque<SwarmEvent>>>,
    event_counter: &Arc<std::sync::atomic::AtomicU64>,
    swarm_event_tx: &broadcast::Sender<SwarmEvent>,
    soft_interrupt_queues: &SessionInterruptQueues,
    mcp_pool: Option<Arc<crate::mcp::SharedMcpPool>>,
) -> Result<Option<String>> {
    if let Some((working_dir, selfdev_requested)) = parse_create_session_command(cmd) {
        let create_command = match working_dir {
            Some(dir) => format!("create_session:{dir}"),
            None => "create_session".to_string(),
        };
        return Ok(Some(
            create_headless_session(
                sessions,
                session_id,
                provider,
                &create_command,
                swarm_members,
                swarms_by_id,
                swarm_coordinators,
                swarm_plans,
                soft_interrupt_queues,
                selfdev_requested,
                None,
                mcp_pool,
                None,
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
        remove_session_interrupt_queue(soft_interrupt_queues, target_id).await;
        if let Some(ref agent_arc) = removed_agent {
            let agent = agent_arc.lock().await;
            let memory_enabled = agent.memory_enabled();
            let transcript = if memory_enabled {
                Some(agent.build_transcript_for_extraction())
            } else {
                None
            };
            let sid = target_id.to_string();
            let working_dir = agent.working_dir().map(|dir| dir.to_string());
            drop(agent);
            if let Some(transcript) = transcript {
                crate::memory_agent::trigger_final_extraction_with_dir(
                    transcript,
                    sid,
                    working_dir,
                );
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
