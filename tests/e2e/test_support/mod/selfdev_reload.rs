use super::*;

pub(crate) async fn wait_for_selfdev_reload_cycle(
    debug_socket_path: &std::path::Path,
    expected_session_id: &str,
    previous_server_id: &str,
    timeout: Duration,
) -> Result<String> {
    let deadline = Instant::now() + timeout;
    let mut last_observation = "no server/client observation yet".to_string();
    let mut stable_since: Option<Instant> = None;

    while Instant::now() < deadline {
        let marker_active = jcode::server::reload_marker_active(Duration::from_secs(30));
        let server_info = match tokio::time::timeout(
            Duration::from_millis(750),
            debug_run_command(debug_socket_path.to_path_buf(), "server:info", None),
        )
        .await
        {
            Ok(Ok(output)) => output,
            Ok(Err(err)) => {
                last_observation =
                    format!("server:info failed while marker_active={marker_active}: {err}");
                tokio::time::sleep(Duration::from_millis(50)).await;
                continue;
            }
            Err(_) => {
                last_observation =
                    format!("server:info timed out while marker_active={marker_active}");
                tokio::time::sleep(Duration::from_millis(50)).await;
                continue;
            }
        };

        let server_info_json: serde_json::Value = serde_json::from_str(&server_info)?;
        let Some(server_id) = server_info_json.get("id").and_then(|v| v.as_str()) else {
            last_observation = format!("server:info missing id: {}", server_info);
            tokio::time::sleep(Duration::from_millis(50)).await;
            continue;
        };

        if server_id == previous_server_id {
            last_observation = format!(
                "server id still {} while marker_active={marker_active}",
                previous_server_id
            );
            tokio::time::sleep(Duration::from_millis(50)).await;
            continue;
        }

        let clients_map = match tokio::time::timeout(
            Duration::from_millis(750),
            debug_run_command(debug_socket_path.to_path_buf(), "clients:map", None),
        )
        .await
        {
            Ok(Ok(output)) => output,
            Ok(Err(err)) => {
                last_observation = format!(
                    "clients:map failed on replacement server {}: {}",
                    server_id, err
                );
                tokio::time::sleep(Duration::from_millis(50)).await;
                continue;
            }
            Err(_) => {
                last_observation =
                    format!("clients:map timed out on replacement server {}", server_id);
                tokio::time::sleep(Duration::from_millis(50)).await;
                continue;
            }
        };

        let clients_json: serde_json::Value = serde_json::from_str(&clients_map)?;
        let clients = clients_json
            .get("clients")
            .and_then(|v| v.as_array())
            .cloned()
            .unwrap_or_default();
        let session_connected = clients.iter().any(|client| {
            client.get("session_id").and_then(|v| v.as_str()) == Some(expected_session_id)
        });

        if !session_connected || clients.len() != 1 {
            last_observation = format!(
                "replacement server {} not yet stable for session {} (client_count={}): {}",
                server_id,
                expected_session_id,
                clients.len(),
                clients_map
            );
            stable_since = None;
            tokio::time::sleep(Duration::from_millis(50)).await;
            continue;
        }

        match stable_since {
            Some(since) if since.elapsed() >= Duration::from_millis(150) => {
                return Ok(server_id.to_string());
            }
            Some(_) => {}
            None => {
                stable_since = Some(Instant::now());
            }
        }

        tokio::time::sleep(Duration::from_millis(50)).await;
    }

    anyhow::bail!(
        "Self-dev reload did not reconnect within {}s: {}",
        timeout.as_secs_f32(),
        last_observation
    )
}

#[cfg(unix)]
pub(crate) async fn wait_for_selfdev_client_reload_cycle(
    debug_socket_path: &std::path::Path,
    expected_session_id: &str,
    previous_client_id: &str,
    expected_server_id: &str,
    timeout: Duration,
) -> Result<String> {
    let deadline = Instant::now() + timeout;
    let mut last_observation = "no client reload observation yet".to_string();
    let mut stable_since: Option<Instant> = None;

    while Instant::now() < deadline {
        let server_info = match tokio::time::timeout(
            Duration::from_millis(750),
            debug_run_command(debug_socket_path.to_path_buf(), "server:info", None),
        )
        .await
        {
            Ok(Ok(output)) => output,
            Ok(Err(err)) => {
                last_observation = format!("server:info failed during client reload: {err}");
                tokio::time::sleep(Duration::from_millis(50)).await;
                continue;
            }
            Err(_) => {
                last_observation = "server:info timed out during client reload".to_string();
                tokio::time::sleep(Duration::from_millis(50)).await;
                continue;
            }
        };

        let server_info_json: serde_json::Value = serde_json::from_str(&server_info)?;
        let Some(server_id) = server_info_json.get("id").and_then(|v| v.as_str()) else {
            last_observation = format!("server:info missing id: {}", server_info);
            tokio::time::sleep(Duration::from_millis(50)).await;
            continue;
        };

        if server_id != expected_server_id {
            last_observation = format!(
                "client reload unexpectedly changed server {} -> {}",
                expected_server_id, server_id
            );
            tokio::time::sleep(Duration::from_millis(50)).await;
            continue;
        }

        let clients_map = match tokio::time::timeout(
            Duration::from_millis(750),
            debug_run_command(debug_socket_path.to_path_buf(), "clients:map", None),
        )
        .await
        {
            Ok(Ok(output)) => output,
            Ok(Err(err)) => {
                last_observation = format!("clients:map failed during client reload: {err}");
                tokio::time::sleep(Duration::from_millis(50)).await;
                continue;
            }
            Err(_) => {
                last_observation = "clients:map timed out during client reload".to_string();
                tokio::time::sleep(Duration::from_millis(50)).await;
                continue;
            }
        };

        let clients_json: serde_json::Value = serde_json::from_str(&clients_map)?;
        let clients = clients_json
            .get("clients")
            .and_then(|v| v.as_array())
            .cloned()
            .unwrap_or_default();
        let new_client_id = clients.iter().find_map(|client| {
            let session_id = client.get("session_id").and_then(|v| v.as_str())?;
            if session_id != expected_session_id {
                return None;
            }
            client
                .get("client_id")
                .and_then(|v| v.as_str())
                .map(str::to_string)
        });

        let Some(new_client_id) = new_client_id else {
            last_observation = format!(
                "clients:map missing session {}: {}",
                expected_session_id, clients_map
            );
            stable_since = None;
            tokio::time::sleep(Duration::from_millis(50)).await;
            continue;
        };

        if new_client_id == previous_client_id {
            last_observation = format!(
                "client id still {} for session {}",
                previous_client_id, expected_session_id
            );
            stable_since = None;
            tokio::time::sleep(Duration::from_millis(50)).await;
            continue;
        }

        if clients.len() != 1 {
            last_observation = format!(
                "client reload not yet stable for session {} (client_count={}): {}",
                expected_session_id,
                clients.len(),
                clients_map
            );
            stable_since = None;
            tokio::time::sleep(Duration::from_millis(50)).await;
            continue;
        }

        match stable_since {
            Some(since) if since.elapsed() >= Duration::from_millis(150) => {
                return Ok(new_client_id);
            }
            Some(_) => {}
            None => {
                stable_since = Some(Instant::now());
            }
        }

        tokio::time::sleep(Duration::from_millis(50)).await;
    }

    anyhow::bail!(
        "Self-dev client reload did not reconnect within {}s: {}",
        timeout.as_secs_f32(),
        last_observation
    )
}
