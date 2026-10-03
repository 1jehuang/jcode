//! `comm_list_swarms` / `comm_set_swarm_label` request handlers.

use super::{SwarmEvent, SwarmEventType, SwarmMember, record_swarm_event};
use crate::protocol::ServerEvent;
use std::collections::{HashMap, HashSet};
use std::sync::Arc;
use tokio::sync::{RwLock, broadcast, mpsc};

pub(super) async fn handle_comm_list_swarms(
    id: u64,
    req_session_id: String,
    client_event_tx: &mpsc::UnboundedSender<ServerEvent>,
    swarm_members: &Arc<RwLock<HashMap<String, SwarmMember>>>,
    swarms_by_id: &Arc<RwLock<HashMap<String, HashSet<String>>>>,
    swarm_coordinators: &Arc<RwLock<HashMap<String, String>>>,
) {
    let swarms = super::swarm_labels::list_swarms(
        &req_session_id,
        swarm_members,
        swarms_by_id,
        Some(swarm_coordinators),
    )
    .await;
    let _ = client_event_tx.send(ServerEvent::CommSwarms { id, swarms });
}

#[expect(
    clippy::too_many_arguments,
    reason = "label changes are recorded in swarm event history and answered with the directory"
)]
pub(super) async fn handle_comm_set_swarm_label(
    id: u64,
    req_session_id: String,
    label: String,
    client_event_tx: &mpsc::UnboundedSender<ServerEvent>,
    swarm_members: &Arc<RwLock<HashMap<String, SwarmMember>>>,
    swarms_by_id: &Arc<RwLock<HashMap<String, HashSet<String>>>>,
    swarm_coordinators: &Arc<RwLock<HashMap<String, String>>>,
    event_history: &Arc<RwLock<std::collections::VecDeque<SwarmEvent>>>,
    event_counter: &Arc<std::sync::atomic::AtomicU64>,
    swarm_event_tx: &broadcast::Sender<SwarmEvent>,
) {
    let (swarm_id, friendly_name) = {
        let members = swarm_members.read().await;
        match members.get(&req_session_id) {
            Some(member) => (member.swarm_id.clone(), member.friendly_name.clone()),
            None => (None, None),
        }
    };
    let Some(swarm_id) = swarm_id else {
        let _ = client_event_tx.send(ServerEvent::Error {
            id,
            message: "Not in a swarm, so there is no swarm to label.".to_string(),
            retry_after_secs: None,
            server_resumes: false,
        });
        return;
    };
    let known: HashSet<String> = swarms_by_id.read().await.keys().cloned().collect();
    match super::swarm_labels::set_swarm_label(&swarm_id, &label, &known) {
        Ok(applied) => {
            record_swarm_event(
                event_history,
                event_counter,
                swarm_event_tx,
                req_session_id.clone(),
                friendly_name,
                Some(swarm_id.clone()),
                SwarmEventType::Notification {
                    notification_type: "swarm_label".to_string(),
                    message: match applied {
                        Some(label) => format!("Swarm labeled '{label}'"),
                        None => "Swarm label cleared".to_string(),
                    },
                },
            )
            .await;
            handle_comm_list_swarms(
                id,
                req_session_id,
                client_event_tx,
                swarm_members,
                swarms_by_id,
                swarm_coordinators,
            )
            .await;
        }
        Err(err) => {
            let _ = client_event_tx.send(ServerEvent::Error {
                id,
                message: err.to_string(),
                retry_after_secs: None,
                server_resumes: false,
            });
        }
    }
}
