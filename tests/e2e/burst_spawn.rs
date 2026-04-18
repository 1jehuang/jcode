use crate::test_support::*;
use futures::future::join_all;
use serde_json::json;
use std::collections::HashSet;
use std::path::PathBuf;

#[derive(Debug)]
struct BurstAttachClientMetrics {
    target_session_id: String,
    returned_session_id: String,
    attach_ms: u128,
    history_message_count: usize,
    provider_model: Option<String>,
    event_count: usize,
    ack_count: usize,
    history_count: usize,
    done_count: usize,
    other_count: usize,
}

enum BurstAttachOutcome {
    Attached(BurstAttachClientMetrics),
    Rejected(String),
}

async fn burst_attach_resumed_client(
    socket_path: PathBuf,
    target_session_id: String,
) -> Result<(server::Client, BurstAttachClientMetrics)> {
    let mut client = wait_for_server_client(&socket_path).await?;
    let subscribe_start = Instant::now();
    let subscribe_id = client
        .subscribe_with_info(None, None, Some(target_session_id.clone()), false, false)
        .await?;

    let mut event_count = 0usize;
    let mut ack_count = 0usize;
    let mut history_count = 0usize;
    let mut done_count = 0usize;
    let mut other_count = 0usize;
    let mut returned_session_id = None;
    let mut history_message_count = 0usize;
    let mut provider_model = None;

    let deadline = Instant::now() + Duration::from_secs(10);
    while Instant::now() < deadline {
        let event = timeout(Duration::from_secs(1), client.read_event()).await??;
        event_count += 1;
        match event {
            ServerEvent::Ack { .. } => ack_count += 1,
            ServerEvent::History {
                id,
                session_id,
                messages,
                provider_model: event_provider_model,
                ..
            } if id == subscribe_id => {
                history_count += 1;
                returned_session_id = Some(session_id);
                history_message_count = messages.len();
                provider_model = event_provider_model;
            }
            ServerEvent::Done { id } if id == subscribe_id => {
                done_count += 1;
                let metrics = BurstAttachClientMetrics {
                    target_session_id,
                    returned_session_id: returned_session_id
                        .ok_or_else(|| anyhow::anyhow!("missing subscribe history event"))?,
                    attach_ms: subscribe_start.elapsed().as_millis(),
                    history_message_count,
                    provider_model,
                    event_count,
                    ack_count,
                    history_count,
                    done_count,
                    other_count,
                };
                return Ok((client, metrics));
            }
            ServerEvent::Error { id, message, .. } if id == subscribe_id => {
                anyhow::bail!("subscribe failed for {}: {}", target_session_id, message);
            }
            _ => other_count += 1,
        }
    }

    anyhow::bail!(
        "timed out attaching resumed client to {} after {} events",
        target_session_id,
        event_count
    )
}

async fn burst_attach_resumed_client_with_options(
    socket_path: PathBuf,
    target_session_id: String,
    client_has_local_history: bool,
    allow_session_takeover: bool,
) -> Result<BurstAttachOutcome> {
    let mut client = wait_for_server_client(&socket_path).await?;
    let subscribe_start = Instant::now();
    let subscribe_id = client
        .subscribe_with_info(
            None,
            None,
            Some(target_session_id.clone()),
            client_has_local_history,
            allow_session_takeover,
        )
        .await?;

    let mut event_count = 0usize;
    let mut ack_count = 0usize;
    let mut history_count = 0usize;
    let mut done_count = 0usize;
    let mut other_count = 0usize;
    let mut returned_session_id = None;
    let mut history_message_count = 0usize;
    let mut provider_model = None;

    let deadline = Instant::now() + Duration::from_secs(10);
    while Instant::now() < deadline {
        let event = timeout(Duration::from_secs(1), client.read_event()).await??;
        event_count += 1;
        match event {
            ServerEvent::Ack { .. } => ack_count += 1,
            ServerEvent::History {
                id,
                session_id,
                messages,
                provider_model: event_provider_model,
                ..
            } if id == subscribe_id => {
                history_count += 1;
                returned_session_id = Some(session_id);
                history_message_count = messages.len();
                provider_model = event_provider_model;
            }
            ServerEvent::Done { id } if id == subscribe_id => {
                done_count += 1;
                let metrics = BurstAttachClientMetrics {
                    target_session_id,
                    returned_session_id: returned_session_id
                        .ok_or_else(|| anyhow::anyhow!("missing subscribe history event"))?,
                    attach_ms: subscribe_start.elapsed().as_millis(),
                    history_message_count,
                    provider_model,
                    event_count,
                    ack_count,
                    history_count,
                    done_count,
                    other_count,
                };
                drop(client);
                return Ok(BurstAttachOutcome::Attached(metrics));
            }
            ServerEvent::Error { id, message, .. } if id == subscribe_id => {
                return Ok(BurstAttachOutcome::Rejected(message));
            }
            _ => other_count += 1,
        }
    }

    anyhow::bail!(
        "timed out attaching resumed client to {} after {} events",
        target_session_id,
        event_count
    )
}

async fn run_burst_resume_attach_stress(burst_size: usize) -> Result<()> {
    let _env = setup_test_env()?;

    let runtime_dir = std::env::temp_dir().join(format!(
        "jcode-burst-spawn-test-{}",
        std::time::SystemTime::now()
            .duration_since(std::time::UNIX_EPOCH)
            .unwrap()
            .as_nanos()
    ));
    std::fs::create_dir_all(&runtime_dir)?;
    let unique_suffix = runtime_dir
        .file_name()
        .and_then(|value| value.to_str())
        .unwrap_or("burst");
    let socket_path = runtime_dir.join("jcode.sock");
    let debug_socket_path = runtime_dir.join("jcode-debug.sock");

    let mut expected_session_ids = Vec::with_capacity(burst_size);
    for idx in 0..burst_size {
        let mut session = Session::create_with_id(
            format!("session_burst_attach_{idx}_{unique_suffix}"),
            None,
            Some(format!("Burst Attach {idx}")),
        );
        session.model = Some("burst-model".to_string());
        session.add_message(
            Role::User,
            vec![ContentBlock::Text {
                text: format!("resume me {idx}"),
                cache_control: None,
            }],
        );
        session.add_message(
            Role::Assistant,
            vec![ContentBlock::Text {
                text: format!("attached reply {idx}"),
                cache_control: None,
            }],
        );
        session.save()?;
        expected_session_ids.push(session.id);
    }

    let provider = Arc::new(MockProvider::with_models(vec!["burst-model"]));
    let provider_dyn: Arc<dyn jcode::provider::Provider> = provider;
    let server_instance = server::Server::new_with_paths(
        provider_dyn,
        socket_path.clone(),
        debug_socket_path.clone(),
    );
    let server_handle = tokio::spawn(async move { server_instance.run().await });

    let cpu_start = current_process_cpu_time()?;
    let wall_start = Instant::now();

    let burst_results = join_all(expected_session_ids.iter().map(|session_id| {
        let socket_path = socket_path.clone();
        async move { burst_attach_resumed_client(socket_path, session_id.to_string()).await }
    }))
    .await;

    let mut connected_clients = Vec::with_capacity(burst_size);
    let mut metrics = Vec::with_capacity(burst_size);
    for result in burst_results {
        let (client, client_metrics) = result?;
        assert_eq!(
            client_metrics.returned_session_id,
            client_metrics.target_session_id
        );
        assert_eq!(client_metrics.history_count, 1);
        assert_eq!(client_metrics.done_count, 1);
        assert!(
            client_metrics.history_message_count >= 2,
            "expected resumed history for {} to include persisted messages",
            client_metrics.target_session_id
        );
        assert_eq!(
            client_metrics.provider_model.as_deref(),
            Some("burst-model")
        );
        connected_clients.push(client);
        metrics.push(client_metrics);
    }

    let wall_elapsed = wall_start.elapsed();
    let cpu_elapsed = current_process_cpu_time()?.saturating_sub(cpu_start);

    let client_map = debug_run_command_json(debug_socket_path.clone(), "clients:map", None).await?;
    let info = debug_run_command_json(debug_socket_path.clone(), "server:info", None).await?;

    let clients = client_map
        .get("clients")
        .and_then(|value| value.as_array())
        .context("clients:map missing clients array")?;
    assert_eq!(
        client_map.get("count").and_then(|value| value.as_u64()),
        Some(burst_size as u64)
    );
    assert_eq!(clients.len(), burst_size);

    let mapped_session_ids: HashSet<String> = clients
        .iter()
        .filter_map(|client| {
            client
                .get("session_id")
                .and_then(|value| value.as_str())
                .map(|value| value.to_string())
        })
        .collect();
    let expected_session_ids_set: HashSet<String> = expected_session_ids.iter().cloned().collect();
    assert_eq!(mapped_session_ids, expected_session_ids_set);

    let ready_count = clients
        .iter()
        .filter(|client| client.get("status").and_then(|value| value.as_str()) == Some("ready"))
        .count();
    assert_eq!(
        ready_count, burst_size,
        "all resumed clients should settle to ready"
    );

    assert_eq!(
        info.get("session_count").and_then(|value| value.as_u64()),
        Some(burst_size as u64)
    );
    assert_eq!(
        info.get("swarm_member_count")
            .and_then(|value| value.as_u64()),
        Some(burst_size as u64),
        "burst attach should not leak temporary swarm members"
    );

    let mut latencies_ms: Vec<u128> = metrics.iter().map(|metric| metric.attach_ms).collect();
    latencies_ms.sort_unstable();
    let total_events: usize = metrics.iter().map(|metric| metric.event_count).sum();
    let total_acks: usize = metrics.iter().map(|metric| metric.ack_count).sum();
    let total_histories: usize = metrics.iter().map(|metric| metric.history_count).sum();
    let total_dones: usize = metrics.iter().map(|metric| metric.done_count).sum();
    let total_other_events: usize = metrics.iter().map(|metric| metric.other_count).sum();
    let total_history_messages: usize = metrics
        .iter()
        .map(|metric| metric.history_message_count)
        .sum();
    let cpu_utilization = if wall_elapsed.is_zero() {
        0.0
    } else {
        cpu_elapsed.as_secs_f64() / wall_elapsed.as_secs_f64()
    };

    eprintln!(
        "burst_spawn_metrics={} ",
        serde_json::to_string_pretty(&json!({
            "burst_size": burst_size,
            "wall_ms": wall_elapsed.as_millis(),
            "cpu_ms": cpu_elapsed.as_millis(),
            "cpu_utilization_ratio": cpu_utilization,
            "cpu_ms_per_attach": cpu_elapsed.as_secs_f64() * 1000.0 / burst_size as f64,
            "latency_ms": {
                "min": latencies_ms.first().copied().unwrap_or(0),
                "p50": percentile_ms(&latencies_ms, 50),
                "p90": percentile_ms(&latencies_ms, 90),
                "p99": percentile_ms(&latencies_ms, 99),
                "max": latencies_ms.last().copied().unwrap_or(0),
                "spread": latencies_ms.last().copied().unwrap_or(0)
                    .saturating_sub(latencies_ms.first().copied().unwrap_or(0)),
            },
            "events": {
                "total": total_events,
                "acks": total_acks,
                "histories": total_histories,
                "dones": total_dones,
                "other": total_other_events,
            },
            "history_messages_total": total_history_messages,
            "connected_clients": clients.len(),
            "unique_session_mappings": mapped_session_ids.len(),
            "ready_count": ready_count,
            "server_info": info,
        }))?
    );

    drop(connected_clients);
    abort_server_and_cleanup(&server_handle, &socket_path, &debug_socket_path);

    Ok(())
}

#[tokio::test(flavor = "multi_thread", worker_threads = 8)]
async fn burst_retry_takeover_without_local_history_keeps_existing_live_clients_connected()
-> Result<()> {
    let _env = setup_test_env()?;

    let runtime_dir = std::env::temp_dir().join(format!(
        "jcode-burst-spawn-live-clients-{}",
        std::time::SystemTime::now()
            .duration_since(std::time::UNIX_EPOCH)
            .unwrap()
            .as_nanos()
    ));
    std::fs::create_dir_all(&runtime_dir)?;
    let unique_suffix = runtime_dir
        .file_name()
        .and_then(|value| value.to_str())
        .unwrap_or("burst-live");
    let socket_path = runtime_dir.join("jcode.sock");
    let debug_socket_path = runtime_dir.join("jcode-debug.sock");

    let live_session_count = 10usize;
    let mut live_session_ids = Vec::with_capacity(live_session_count);
    for idx in 0..live_session_count {
        let mut session = Session::create_with_id(
            format!("session_live_attach_{idx}_{unique_suffix}"),
            None,
            Some(format!("Live Attach {idx}")),
        );
        session.model = Some("burst-model".to_string());
        session.add_message(
            Role::User,
            vec![ContentBlock::Text {
                text: format!("resume me live {idx}"),
                cache_control: None,
            }],
        );
        session.add_message(
            Role::Assistant,
            vec![ContentBlock::Text {
                text: format!("live reply {idx}"),
                cache_control: None,
            }],
        );
        session.save()?;
        live_session_ids.push(session.id);
    }

    let provider = Arc::new(MockProvider::with_models(vec!["burst-model"]));
    let provider_dyn: Arc<dyn jcode::provider::Provider> = provider;
    let server_instance = server::Server::new_with_paths(
        provider_dyn,
        socket_path.clone(),
        debug_socket_path.clone(),
    );
    let server_handle = tokio::spawn(async move { server_instance.run().await });

    let mut live_clients = Vec::with_capacity(live_session_count);
    for session_id in live_session_ids.iter().cloned() {
        let (client, metrics) =
            burst_attach_resumed_client(socket_path.clone(), session_id.clone()).await?;
        assert_eq!(metrics.returned_session_id, session_id);
        live_clients.push((session_id, client));
    }

    let initial_client_map =
        debug_run_command_json(debug_socket_path.clone(), "clients:map", None).await?;
    let initial_session_to_client = client_id_map(&initial_client_map)?;

    let retry_results = join_all(live_session_ids.iter().map(|session_id| {
        let socket_path = socket_path.clone();
        async move {
            burst_attach_resumed_client_with_options(
                socket_path,
                session_id.to_string(),
                false,
                true,
            )
            .await
        }
    }))
    .await;

    for result in retry_results {
        match result? {
            BurstAttachOutcome::Attached(metrics) => {
                assert_eq!(metrics.returned_session_id, metrics.target_session_id);
                assert_eq!(metrics.history_count, 1);
                assert_eq!(metrics.done_count, 1);
            }
            BurstAttachOutcome::Rejected(message) => {
                anyhow::bail!("retry attach should succeed for live shared session: {message}");
            }
        }
    }

    let final_client_map =
        debug_run_command_json(debug_socket_path.clone(), "clients:map", None).await?;
    let final_session_to_client = client_id_map(&final_client_map)?;
    assert_eq!(
        final_session_to_client, initial_session_to_client,
        "existing live client connections should not be replaced during burst retries"
    );

    drop(live_clients);
    abort_server_and_cleanup(&server_handle, &socket_path, &debug_socket_path);

    Ok(())
}

/// Stress the burst attach path used when many spawned windows resume pre-created sessions.
/// This targets the race-prone phase directly and records useful metrics for regressions.
#[tokio::test(flavor = "multi_thread", worker_threads = 8)]
async fn burst_spawn_resume_attach_keeps_unique_live_mappings_and_reports_metrics() -> Result<()> {
    run_burst_resume_attach_stress(20).await
}


#[tokio::test(flavor = "multi_thread", worker_threads = 8)]
async fn burst_attach_detach_reattach_restores_live_clients_cleanly() -> Result<()> {
    let _env = setup_test_env()?;

    let runtime_dir = std::env::temp_dir().join(format!(
        "jcode-burst-spawn-reattach-{}",
        std::time::SystemTime::now()
            .duration_since(std::time::UNIX_EPOCH)
            .unwrap()
            .as_nanos()
    ));
    std::fs::create_dir_all(&runtime_dir)?;
    let unique_suffix = runtime_dir
        .file_name()
        .and_then(|value| value.to_str())
        .unwrap_or("burst-reattach");
    let socket_path = runtime_dir.join("jcode.sock");
    let debug_socket_path = runtime_dir.join("jcode-debug.sock");

    let burst_size = 6usize;
    let mut session_ids = Vec::with_capacity(burst_size);
    for idx in 0..burst_size {
        let mut session = Session::create_with_id(
            format!("session_burst_reattach_{idx}_{unique_suffix}"),
            None,
            Some(format!("Burst Reattach {idx}")),
        );
        session.model = Some("burst-model".to_string());
        session.add_message(
            Role::User,
            vec![ContentBlock::Text {
                text: format!("resume me reattach {idx}"),
                cache_control: None,
            }],
        );
        session.add_message(
            Role::Assistant,
            vec![ContentBlock::Text {
                text: format!("reattach reply {idx}"),
                cache_control: None,
            }],
        );
        session.save()?;
        session_ids.push(session.id);
    }

    let provider = Arc::new(MockProvider::with_models(vec!["burst-model"]));
    let provider_dyn: Arc<dyn jcode::provider::Provider> = provider;
    let server_instance = server::Server::new_with_paths(
        provider_dyn,
        socket_path.clone(),
        debug_socket_path.clone(),
    );
    let server_handle = tokio::spawn(async move { server_instance.run().await });

    let initial_results = join_all(session_ids.iter().map(|session_id| {
        let socket_path = socket_path.clone();
        async move { burst_attach_resumed_client(socket_path, session_id.to_string()).await }
    }))
    .await;

    let mut initial_clients = Vec::with_capacity(burst_size);
    for result in initial_results {
        let (client, metrics) = result?;
        assert_eq!(metrics.returned_session_id, metrics.target_session_id);
        initial_clients.push(client);
    }
    wait_for_debug_client_count(&debug_socket_path, burst_size, Duration::from_secs(5)).await?;

    drop(initial_clients);
    wait_for_debug_client_count(&debug_socket_path, 0, Duration::from_secs(5)).await?;

    let reattach_results = join_all(session_ids.iter().map(|session_id| {
        let socket_path = socket_path.clone();
        async move { burst_attach_resumed_client(socket_path, session_id.to_string()).await }
    }))
    .await;

    let mut reattached_clients = Vec::with_capacity(burst_size);
    for result in reattach_results {
        let (client, metrics) = result?;
        assert_eq!(metrics.returned_session_id, metrics.target_session_id);
        assert_eq!(metrics.history_count, 1);
        assert_eq!(metrics.done_count, 1);
        reattached_clients.push(client);
    }
    wait_for_debug_client_count(&debug_socket_path, burst_size, Duration::from_secs(5)).await?;

    drop(reattached_clients);
    abort_server_and_cleanup(&server_handle, &socket_path, &debug_socket_path);

    Ok(())
}

#[tokio::test(flavor = "multi_thread", worker_threads = 8)]
#[ignore = "resource-heavy scale validation"]
async fn burst_spawn_resume_attach_scales_to_100_clients() -> Result<()> {
    run_burst_resume_attach_stress(100).await
}
