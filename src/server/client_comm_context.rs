use super::{
    SharedContext, SwarmEvent, SwarmEventType, SwarmMember, fanout_session_event,
    record_swarm_event,
};
use crate::protocol::{AgentInfo, ContextEntry, NotificationType, ServerEvent};
use std::collections::{HashMap, HashSet};
use std::path::PathBuf;
use std::sync::Arc;
use std::time::Instant;
use tokio::sync::{RwLock, broadcast, mpsc};

async fn swarm_id_for_session(
    session_id: &str,
    swarm_members: &Arc<RwLock<HashMap<String, SwarmMember>>>,
) -> Option<String> {
    let members = swarm_members.read().await;
    members.get(session_id).and_then(|m| m.swarm_id.clone())
}

async fn friendly_name_for_session(
    session_id: &str,
    swarm_members: &Arc<RwLock<HashMap<String, SwarmMember>>>,
) -> Option<String> {
    let members = swarm_members.read().await;
    members
        .get(session_id)
        .and_then(|member| member.friendly_name.clone())
}

#[expect(
    clippy::too_many_arguments,
    reason = "comm share coordinates delivery state, sessions, swarm membership, and event fanout"
)]
pub(super) async fn handle_comm_share(
    id: u64,
    req_session_id: String,
    key: String,
    value: String,
    append: bool,
    client_event_tx: &mpsc::UnboundedSender<ServerEvent>,
    swarm_members: &Arc<RwLock<HashMap<String, SwarmMember>>>,
    swarms_by_id: &Arc<RwLock<HashMap<String, HashSet<String>>>>,
    shared_context: &Arc<RwLock<HashMap<String, HashMap<String, SharedContext>>>>,
    event_history: &Arc<RwLock<std::collections::VecDeque<SwarmEvent>>>,
    event_counter: &Arc<std::sync::atomic::AtomicU64>,
    swarm_event_tx: &broadcast::Sender<SwarmEvent>,
) {
    let swarm_id = swarm_id_for_session(&req_session_id, swarm_members).await;

    if let Some(swarm_id) = swarm_id {
        let friendly_name = friendly_name_for_session(&req_session_id, swarm_members).await;

        {
            let mut ctx = shared_context.write().await;
            let swarm_ctx = ctx.entry(swarm_id.clone()).or_insert_with(HashMap::new);
            let now = Instant::now();
            let created_at = swarm_ctx.get(&key).map(|c| c.created_at).unwrap_or(now);
            let stored_value = if append {
                swarm_ctx
                    .get(&key)
                    .map(|existing| {
                        if existing.value.is_empty() {
                            value.clone()
                        } else {
                            format!("{}\n{}", existing.value, value)
                        }
                    })
                    .unwrap_or_else(|| value.clone())
            } else {
                value.clone()
            };
            swarm_ctx.insert(
                key.clone(),
                SharedContext {
                    key: key.clone(),
                    value: stored_value.clone(),
                    from_session: req_session_id.clone(),
                    from_name: friendly_name.clone(),
                    created_at,
                    updated_at: now,
                },
            );
        }

        let swarm_session_ids: Vec<String> = {
            let swarms = swarms_by_id.read().await;
            swarms
                .get(&swarm_id)
                .map(|sessions| sessions.iter().cloned().collect())
                .unwrap_or_default()
        };

        for sid in &swarm_session_ids {
            if sid != &req_session_id {
                let _ = fanout_session_event(
                    swarm_members,
                    sid,
                    ServerEvent::Notification {
                        from_session: req_session_id.clone(),
                        from_name: friendly_name.clone(),
                        notification_type: NotificationType::SharedContext {
                            key: key.clone(),
                            value: if append {
                                format!("(appended) {}", value)
                            } else {
                                value.clone()
                            },
                        },
                        message: if append {
                            format!("Appended shared context: {} += {}", key, value)
                        } else {
                            format!("Shared context: {} = {}", key, value)
                        },
                    },
                )
                .await;
            }
        }

        record_swarm_event(
            event_history,
            event_counter,
            swarm_event_tx,
            req_session_id.clone(),
            friendly_name.clone(),
            Some(swarm_id.clone()),
            SwarmEventType::ContextUpdate {
                swarm_id: swarm_id.clone(),
                key: key.clone(),
            },
        )
        .await;

        let _ = client_event_tx.send(ServerEvent::Done { id });
    } else {
        let _ = client_event_tx.send(ServerEvent::Error {
            id,
            message: "Not in a swarm. Use a git repository to enable swarm features.".to_string(),
            retry_after_secs: None,
        });
    }
}

pub(super) async fn handle_comm_read(
    id: u64,
    req_session_id: String,
    key: Option<String>,
    client_event_tx: &mpsc::UnboundedSender<ServerEvent>,
    swarm_members: &Arc<RwLock<HashMap<String, SwarmMember>>>,
    shared_context: &Arc<RwLock<HashMap<String, HashMap<String, SharedContext>>>>,
) {
    let swarm_id = swarm_id_for_session(&req_session_id, swarm_members).await;

    let entries = if let Some(swarm_id) = swarm_id {
        let ctx = shared_context.read().await;
        if let Some(swarm_ctx) = ctx.get(&swarm_id) {
            if let Some(k) = key {
                swarm_ctx
                    .get(&k)
                    .map(|c| {
                        vec![ContextEntry {
                            key: c.key.clone(),
                            value: c.value.clone(),
                            from_session: c.from_session.clone(),
                            from_name: c.from_name.clone(),
                        }]
                    })
                    .unwrap_or_default()
            } else {
                swarm_ctx
                    .values()
                    .map(|c| ContextEntry {
                        key: c.key.clone(),
                        value: c.value.clone(),
                        from_session: c.from_session.clone(),
                        from_name: c.from_name.clone(),
                    })
                    .collect()
            }
        } else {
            Vec::new()
        }
    } else {
        Vec::new()
    };

    let _ = client_event_tx.send(ServerEvent::CommContext { id, entries });
}

pub(super) async fn handle_comm_list(
    id: u64,
    req_session_id: String,
    client_event_tx: &mpsc::UnboundedSender<ServerEvent>,
    swarm_members: &Arc<RwLock<HashMap<String, SwarmMember>>>,
    swarms_by_id: &Arc<RwLock<HashMap<String, HashSet<String>>>>,
    files_touched_by_session: &Arc<RwLock<HashMap<String, HashSet<PathBuf>>>>,
) {
    let swarm_id = swarm_id_for_session(&req_session_id, swarm_members).await;

    if let Some(swarm_id) = swarm_id {
        let swarm_session_ids: Vec<String> = {
            let swarms = swarms_by_id.read().await;
            swarms
                .get(&swarm_id)
                .map(|sessions| sessions.iter().cloned().collect())
                .unwrap_or_default()
        };

        let members = swarm_members.read().await;
        let touches = files_touched_by_session.read().await;

        let member_list: Vec<AgentInfo> = swarm_session_ids
            .iter()
            .filter_map(|sid| {
                members.get(sid).map(|member| {
                    let mut files: Vec<String> = touches
                        .get(sid)
                        .into_iter()
                        .flat_map(|paths| paths.iter())
                        .map(|path| path.display().to_string())
                        .collect();
                    files.sort();

                    AgentInfo {
                        session_id: sid.clone(),
                        friendly_name: member.friendly_name.clone(),
                        files_touched: files,
                        status: Some(member.status.clone()),
                        detail: member.detail.clone(),
                        role: Some(member.role.clone()),
                        is_headless: Some(member.is_headless),
                        report_back_to_session_id: member.report_back_to_session_id.clone(),
                        latest_completion_report: member.latest_completion_report.clone(),
                        live_attachments: Some(member.event_txs.len()),
                        status_age_secs: Some(member.last_status_change.elapsed().as_secs()),
                    }
                })
            })
            .collect();

        let _ = client_event_tx.send(ServerEvent::CommMembers {
            id,
            members: member_list,
        });
    } else {
        let _ = client_event_tx.send(ServerEvent::Error {
            id,
            message: "Not in a swarm. Use a git repository to enable swarm features.".to_string(),
            retry_after_secs: None,
        });
    }
}
