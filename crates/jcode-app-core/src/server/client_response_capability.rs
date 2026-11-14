use super::{Agent, ClientConnectionInfo, SessionAgents};
use std::collections::HashMap;
use std::sync::Arc;
use tokio::sync::{Mutex, RwLock};

type Connections = Arc<RwLock<HashMap<String, ClientConnectionInfo>>>;

/// Re-read attachment capabilities only after acquiring the Agent. A queued
/// update must never replay a snapshot taken before a migration or disconnect.
pub(in crate::server) async fn refresh_session(
    sessions: &SessionAgents,
    connections: &Connections,
    session_id: &str,
) {
    let Some(agent) = sessions.read().await.get(session_id).cloned() else {
        return;
    };
    if try_apply(&agent, connections, session_id).await {
        return;
    }
    let connections = Arc::clone(connections);
    let session_id = session_id.to_owned();
    tokio::spawn(async move {
        loop {
            // Wait without holding the connection registry, then retry in the
            // registry -> Agent order used by attachment cleanup. Never hold
            // the Agent while waiting for the registry's write-side cleanup.
            drop(agent.lock().await);
            if try_apply(&agent, &connections, &session_id).await {
                break;
            }
        }
    });
}

async fn try_apply(agent: &Arc<Mutex<Agent>>, connections: &Connections, session_id: &str) -> bool {
    let connections = connections.read().await;
    let Ok(mut agent) = agent.try_lock() else {
        return false;
    };
    let attached = || {
        connections
            .values()
            .filter(|info| info.session_id == session_id)
    };
    let enabled = attached().any(|info| info.supports_user_response);
    agent.set_desktop_response_capability(enabled).await;
    let ask_user_tx = attached().find_map(|info| info.ask_user_tx.clone());
    agent.set_ask_user_client(ask_user_tx).await;
    true
}

/// EOF means this transport cannot answer, even while its session is retained
/// for reconnect or a background turn. Do not wait for the cleanup grace timer.
pub(in crate::server) async fn disconnect(
    sessions: &SessionAgents,
    connections: &Connections,
    connection_id: &str,
    session_id: &str,
) {
    if let Some(info) = connections.write().await.get_mut(connection_id) {
        info.supports_user_response = false;
        info.ask_user_tx = None;
    }
    refresh_session(sessions, connections, session_id).await;
}
