use super::{ClientConnectionInfo, SessionAgents};
use std::collections::HashMap;
use std::sync::Arc;
use tokio::sync::RwLock;

type Connections = Arc<RwLock<HashMap<String, ClientConnectionInfo>>>;

/// Apply through cached, non-owning registry metadata even while Agent is busy.
pub(in crate::server) async fn refresh_session(
    sessions: &SessionAgents,
    connections: &Connections,
    session_id: &str,
) {
    if let Ok(connections) = connections.try_read() {
        if let Some((registry, changed)) = sessions.response_capability_registry(session_id) {
            apply(registry, changed, &connections, session_id).await;
        }
        return;
    }
    let sessions = Arc::downgrade(sessions);
    let connections = Arc::clone(connections);
    let session_id = session_id.to_owned();
    crate::runtime_activity::spawn(async move {
        let connections = connections.read().await;
        let capability = sessions
            .upgrade()
            .and_then(|sessions| sessions.response_capability_registry(&session_id));
        if let Some((registry, changed)) = capability {
            apply(registry, changed, &connections, &session_id).await;
        }
    });
}

async fn apply(
    registry: crate::tool::Registry,
    changed: Arc<std::sync::atomic::AtomicBool>,
    connections: &HashMap<String, ClientConnectionInfo>,
    session_id: &str,
) {
    let enabled = connections
        .values()
        .any(|info| info.session_id == session_id && info.supports_user_response);
    let was_enabled = registry
        .tool_names()
        .await
        .iter()
        .any(|name| name == "user_response");
    if was_enabled != enabled {
        registry.set_desktop_ui_enabled(enabled).await;
        changed.store(true, std::sync::atomic::Ordering::Release);
    }
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
    }
    refresh_session(sessions, connections, session_id).await;
}
