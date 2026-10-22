//! Connected user sessions, not daemon-owned agents or persisted lifecycle state.
use super::{ClientConnectionInfo, SwarmMember};
use crate::protocol::ServerEvent;
use std::collections::{BTreeSet, HashMap};
use std::sync::Arc;
use tokio::sync::RwLock;

fn connected_user_ids(
    connected: impl IntoIterator<Item = String>,
    members: &HashMap<String, SwarmMember>,
) -> Vec<String> {
    connected
        .into_iter()
        .filter(|id| !id.is_empty() && members.get(id).is_none_or(|member| !member.is_headless))
        .collect::<BTreeSet<_>>()
        .into_iter()
        .collect()
}

/// Snapshot connections without taking any agent lock, including during a turn.
/// Bare loaded agents and headless swarm workers do not imply an open user client.
pub(super) async fn connection_presence_pong(
    id: u64,
    connections: &Arc<RwLock<HashMap<String, ClientConnectionInfo>>>,
    members: &Arc<RwLock<HashMap<String, SwarmMember>>>,
) -> ServerEvent {
    // Resume and disconnect also use these registries. Never hold both locks.
    let connected = connections
        .read()
        .await
        .values()
        .map(|info| info.session_id.clone())
        .collect::<Vec<_>>();
    let user_connected_session_ids = connected_user_ids(connected, &*members.read().await);
    ServerEvent::Pong {
        id,
        native_ssh_protocol: Some(1),
        capabilities: vec![
            "session_tools".into(),
            "soft_interrupt_ids".into(),
            "runtime_commands".into(),
            "session_connections".into(),
            "rewind_checkpoints".into(),
        ],
        user_connected_session_ids: Some(user_connected_session_ids),
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn member(id: &str, headless: bool) -> SwarmMember {
        let now = std::time::Instant::now();
        SwarmMember {
            session_id: id.into(),
            event_tx: tokio::sync::mpsc::unbounded_channel().0,
            event_txs: HashMap::new(),
            working_dir: None,
            swarm_id: None,
            swarm_enabled: false,
            status: "ready".into(),
            detail: None,
            friendly_name: None,
            report_back_to_session_id: None,
            latest_completion_report: None,
            role: "agent".into(),
            joined_at: now,
            last_status_change: now,
            is_headless: headless,
            output_tail: None,
            todo_progress: None,
            todo_items: Vec::new(),
            runtime: crate::protocol::SwarmMemberRuntime::default(),
            task_label: None,
        }
    }

    #[test]
    fn headless_and_loaded_agents_are_not_user_connections() {
        let members = HashMap::from([
            ("headless".into(), member("headless", true)),
            ("loaded-history".into(), member("loaded-history", false)),
            ("idle-user".into(), member("idle-user", false)),
        ]);
        assert_eq!(
            connected_user_ids(["headless", "idle-user"].map(str::to_string), &members),
            vec!["idle-user"]
        );
        assert!(connected_user_ids(Vec::new(), &members).is_empty());
    }

    #[test]
    fn connections_are_deduplicated_and_empty_connections_are_not_users() {
        assert_eq!(
            connected_user_ids(
                ["ram", "", "ram", "idle"].map(str::to_string),
                &HashMap::new()
            ),
            vec!["idle", "ram"]
        );
        assert!(connected_user_ids(Vec::new(), &HashMap::new()).is_empty());
    }
}
