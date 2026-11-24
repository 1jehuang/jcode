//! Correlate native client control replies independently of caller-local IDs.

use super::{ClientConnectionInfo, ClientDebugState, resolve_client_debug_sender};
use anyhow::Result;
use std::collections::HashMap;
use std::sync::{Arc, atomic::AtomicU64};
use tokio::sync::{RwLock, broadcast};

static NEXT_REQUEST_ID: AtomicU64 = AtomicU64::new(1);

pub(super) async fn forward(
    requested_session: Option<&str>,
    cmd: &str,
    client_connections: &Arc<RwLock<HashMap<String, ClientConnectionInfo>>>,
    client_debug_state: &Arc<RwLock<ClientDebugState>>,
    client_debug_response_tx: &broadcast::Sender<(u64, String)>,
) -> Result<String> {
    // shortcut: refuse ambiguous views; add an explicit native client identity if they need independent catalog reads.
    if cmd == "model-options"
        && let Some(session) = requested_session
    {
        let registered: std::collections::HashSet<_> = client_debug_state
            .read()
            .await
            .clients
            .keys()
            .cloned()
            .collect();
        let clients = client_connections.read().await;
        let count = clients
            .values()
            .filter(|client| {
                client.session_id == session
                    && client
                        .debug_client_id
                        .as_ref()
                        .is_some_and(|id| registered.contains(id))
            })
            .count();
        if count > 1 {
            anyhow::bail!(
                "Several TUI clients share this conversation. Close the other views before loading its model choices."
            );
        }
    }
    let request_id = NEXT_REQUEST_ID.fetch_add(1, std::sync::atomic::Ordering::Relaxed);
    let mut response_rx = client_debug_response_tx.subscribe();
    let mut attempts = 0usize;

    loop {
        let (client_id, tx) = match resolve_client_debug_sender(
            requested_session,
            client_connections,
            client_debug_state,
        )
        .await
        {
            Ok(target) => target,
            Err(err) => break Err(err),
        };

        if tx.send((request_id, cmd.to_string())).is_ok() {
            // Wait for response with timeout
            let timeout = tokio::time::Duration::from_secs(30);
            match tokio::time::timeout(timeout, async {
                loop {
                    match response_rx.recv().await {
                        Ok((resp_id, output)) if resp_id == request_id => {
                            if cmd == "model-options" {
                                let mut catalog: serde_json::Value = serde_json::from_str(&output)
                                    .map_err(|_| anyhow::anyhow!("Update jcode and reload the TUI client to export native model choices"))?;
                                let session = catalog.get("session_id").and_then(|value| value.as_str())
                                    .ok_or_else(|| anyhow::anyhow!("The jcode client is reconnecting. Retry once its conversation is connected."))?;
                                if requested_session.is_some_and(|expected| expected != session) {
                                    anyhow::bail!("The jcode client changed conversation. Reload its native choices.");
                                }
                                let connections = client_connections.read().await;
                                if !connections.values().any(|connection| connection.session_id == session && connection.debug_client_id.as_deref() == Some(client_id.as_str())) {
                                    anyhow::bail!("The jcode client reconnected or changed conversation. Reload its native choices.");
                                }
                                catalog["client_id"] = serde_json::json!(client_id);
                                return Ok(catalog.to_string());
                            }
                            return Ok(output);
                        }
                        Err(broadcast::error::RecvError::Closed) => {
                            return Err(anyhow::anyhow!("The client response channel closed"));
                        }
                        _ => {}
                    }
                }
            })
            .await
            {
                Ok(result) => break result,
                Err(_) => {
                    break Err(anyhow::anyhow!("Timeout waiting for client response"));
                }
            }
        } else {
            let mut debug_state = client_debug_state.write().await;
            debug_state.unregister(&client_id);
            attempts += 1;
            if requested_session.is_some() || debug_state.clients.is_empty() || attempts > 8 {
                break Err(anyhow::anyhow!("No TUI client connected"));
            }
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use tokio::sync::mpsc;

    fn client(id: &str, session: &str) -> ClientConnectionInfo {
        ClientConnectionInfo {
            client_id: id.into(),
            session_id: session.into(),
            client_instance_id: None,
            debug_client_id: Some(id.into()),
            connected_at: std::time::Instant::now(),
            last_seen: std::time::Instant::now(),
            is_processing: false,
            current_tool_name: None,
            terminal_env: Vec::new(),
            supports_user_response: false,
            ask_user_tx: None,
            disconnect_tx: mpsc::unbounded_channel().0,
        }
    }

    #[tokio::test]
    async fn concurrent_catalogs_do_not_share_caller_local_ids_or_choose_ambiguous_views() {
        let connections = Arc::new(RwLock::new(HashMap::from([
            ("a".into(), client("a", "sa")),
            ("b".into(), client("b", "sb")),
        ])));
        let mut state = ClientDebugState::default();
        let (a_tx, mut a_rx) = mpsc::unbounded_channel();
        let (b_tx, mut b_rx) = mpsc::unbounded_channel();
        state.register("a".into(), a_tx);
        state.register("b".into(), b_tx);
        let state = Arc::new(RwLock::new(state));
        let (responses, _) = broadcast::channel(16);
        let drive = async {
            let (a, _) = a_rx.recv().await.unwrap();
            let (b, _) = b_rx.recv().await.unwrap();
            assert_ne!(
                a, b,
                "forwarded IDs must be unique even when every CLI caller uses id 1"
            );
            responses
                .send((
                    b,
                    serde_json::json!({ "choices": ["b"], "client_pid": 20, "session_id": "sb" })
                        .to_string(),
                ))
                .unwrap();
            responses
                .send((
                    a,
                    serde_json::json!({ "choices": ["a"], "client_pid": 10, "session_id": "sa" })
                        .to_string(),
                ))
                .unwrap();
        };
        let (a, b, ()) = tokio::time::timeout(std::time::Duration::from_secs(2), async {
            tokio::join!(
                forward(
                    Some("sa"),
                    "model-options",
                    &connections,
                    &state,
                    &responses
                ),
                forward(
                    Some("sb"),
                    "model-options",
                    &connections,
                    &state,
                    &responses
                ),
                drive
            )
        })
        .await
        .unwrap();
        let a: serde_json::Value = serde_json::from_str(&a.unwrap()).unwrap();
        assert_eq!(a["choices"][0], "a");
        assert_eq!(a["client_id"], "a");
        let b: serde_json::Value = serde_json::from_str(&b.unwrap()).unwrap();
        assert_eq!(b["choices"][0], "b");
        assert_eq!(b["client_id"], "b");
        for reconnect in [false, true] {
            let drive = async {
                let (id, _) = a_rx.recv().await.unwrap();
                let mut clients = connections.write().await;
                let client = clients.get_mut("a").unwrap();
                if reconnect {
                    client.debug_client_id = Some("new-connection".into());
                } else {
                    client.session_id = "another-conversation".into();
                }
                drop(clients);
                responses.send((id, serde_json::json!({ "choices": ["stale"], "client_pid": 10, "session_id": "sa" }).to_string())).unwrap();
            };
            let (result, ()) = tokio::join!(
                forward(
                    Some("sa"),
                    "model-options",
                    &connections,
                    &state,
                    &responses
                ),
                drive
            );
            assert!(
                result
                    .unwrap_err()
                    .to_string()
                    .contains("reconnected or changed conversation")
            );
            let mut clients = connections.write().await;
            *clients.get_mut("a").unwrap() = client("a", "sa");
        }
        connections.write().await.get_mut("b").unwrap().session_id = "sa".into();
        assert!(
            forward(
                Some("sa"),
                "model-options",
                &connections,
                &state,
                &responses
            )
            .await
            .unwrap_err()
            .to_string()
            .contains("Several TUI clients")
        );
    }
}
