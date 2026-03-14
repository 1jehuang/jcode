use crate::agent::Agent;
use crate::protocol::ServerEvent;
use crate::provider::Provider;
use std::sync::Arc;
use tokio::sync::{Mutex, mpsc};

async fn model_switching_available(agent: &Arc<Mutex<Agent>>) -> Option<String> {
    let models = {
        let agent_guard = agent.lock().await;
        agent_guard.available_models()
    };
    if models.is_empty() {
        let current = {
            let agent_guard = agent.lock().await;
            agent_guard.provider_model()
        };
        Some(current)
    } else {
        None
    }
}

pub(super) async fn handle_cycle_model(
    id: u64,
    direction: i8,
    agent: &Arc<Mutex<Agent>>,
    client_event_tx: &mpsc::UnboundedSender<ServerEvent>,
) {
    let models = {
        let agent_guard = agent.lock().await;
        agent_guard.available_models()
    };
    if models.is_empty() {
        let model = {
            let agent_guard = agent.lock().await;
            agent_guard.provider_model()
        };
        let _ = client_event_tx.send(ServerEvent::ModelChanged {
            id,
            model,
            provider_name: None,
            error: Some("Model switching is not available for this provider.".to_string()),
        });
        return;
    }

    let current = {
        let agent_guard = agent.lock().await;
        agent_guard.provider_model()
    };
    let current_index = models.iter().position(|m| *m == current).unwrap_or(0);
    let len = models.len();
    let next_index = if direction >= 0 {
        (current_index + 1) % len
    } else {
        (current_index + len - 1) % len
    };
    let next_model = models[next_index];

    let result = {
        let mut agent_guard = agent.lock().await;
        let result = agent_guard.set_model(next_model);
        if result.is_ok() {
            agent_guard.reset_provider_session();
        }
        result.map(|_| (agent_guard.provider_model(), agent_guard.provider_name()))
    };

    match result {
        Ok((updated, pname)) => {
            crate::telemetry::record_model_switch();
            let _ = client_event_tx.send(ServerEvent::ModelChanged {
                id,
                model: updated,
                provider_name: Some(pname),
                error: None,
            });
        }
        Err(e) => {
            let _ = client_event_tx.send(ServerEvent::ModelChanged {
                id,
                model: current,
                provider_name: None,
                error: Some(e.to_string()),
            });
        }
    }
}

pub(super) async fn handle_set_premium_mode(
    id: u64,
    mode: u8,
    agent: &Arc<Mutex<Agent>>,
    client_event_tx: &mpsc::UnboundedSender<ServerEvent>,
) {
    use crate::provider::copilot::PremiumMode;

    let premium_mode = match mode {
        2 => PremiumMode::Zero,
        1 => PremiumMode::OnePerSession,
        _ => PremiumMode::Normal,
    };
    let agent_guard = agent.lock().await;
    agent_guard.set_premium_mode(premium_mode);
    let label = match premium_mode {
        PremiumMode::Zero => "zero premium requests",
        PremiumMode::OnePerSession => "one premium per session",
        PremiumMode::Normal => "normal",
    };
    crate::logging::info(&format!("Server: premium mode set to {} ({})", mode, label));
    let _ = client_event_tx.send(ServerEvent::Ack { id });
}

pub(super) async fn handle_set_model(
    id: u64,
    model: String,
    agent: &Arc<Mutex<Agent>>,
    client_event_tx: &mpsc::UnboundedSender<ServerEvent>,
) {
    if let Some(current) = model_switching_available(agent).await {
        let _ = client_event_tx.send(ServerEvent::ModelChanged {
            id,
            model: current,
            provider_name: None,
            error: Some("Model switching is not available for this provider.".to_string()),
        });
        return;
    }

    let current = {
        let agent_guard = agent.lock().await;
        agent_guard.provider_model()
    };
    let result = {
        let mut agent_guard = agent.lock().await;
        let result = agent_guard.set_model(&model);
        if result.is_ok() {
            agent_guard.reset_provider_session();
        }
        result.map(|_| (agent_guard.provider_model(), agent_guard.provider_name()))
    };

    match result {
        Ok((updated, pname)) => {
            crate::telemetry::record_model_switch();
            let _ = client_event_tx.send(ServerEvent::ModelChanged {
                id,
                model: updated,
                provider_name: Some(pname),
                error: None,
            });
        }
        Err(e) => {
            let _ = client_event_tx.send(ServerEvent::ModelChanged {
                id,
                model: current,
                provider_name: None,
                error: Some(e.to_string()),
            });
        }
    }
}

pub(super) async fn handle_set_reasoning_effort(
    id: u64,
    effort: String,
    agent: &Arc<Mutex<Agent>>,
    client_event_tx: &mpsc::UnboundedSender<ServerEvent>,
) {
    let provider = {
        let agent_guard = agent.lock().await;
        agent_guard.provider_handle()
    };

    match provider.set_reasoning_effort(&effort) {
        Ok(()) => {
            let _ = client_event_tx.send(ServerEvent::ReasoningEffortChanged {
                id,
                effort: provider.reasoning_effort(),
                error: None,
            });
        }
        Err(e) => {
            let _ = client_event_tx.send(ServerEvent::ReasoningEffortChanged {
                id,
                effort: None,
                error: Some(e.to_string()),
            });
        }
    }
}

pub(super) async fn handle_set_service_tier(
    id: u64,
    service_tier: String,
    agent: &Arc<Mutex<Agent>>,
    client_event_tx: &mpsc::UnboundedSender<ServerEvent>,
) {
    let provider = {
        let agent_guard = agent.lock().await;
        agent_guard.provider_handle()
    };

    match provider.set_service_tier(&service_tier) {
        Ok(()) => {
            let _ = client_event_tx.send(ServerEvent::ServiceTierChanged {
                id,
                service_tier: provider.service_tier(),
                error: None,
            });
        }
        Err(e) => {
            let _ = client_event_tx.send(ServerEvent::ServiceTierChanged {
                id,
                service_tier: None,
                error: Some(e.to_string()),
            });
        }
    }
}

pub(super) async fn handle_set_transport(
    id: u64,
    transport: String,
    agent: &Arc<Mutex<Agent>>,
    client_event_tx: &mpsc::UnboundedSender<ServerEvent>,
) {
    let provider = {
        let agent_guard = agent.lock().await;
        agent_guard.provider_handle()
    };

    match provider.set_transport(&transport) {
        Ok(()) => {
            let _ = client_event_tx.send(ServerEvent::TransportChanged {
                id,
                transport: provider.transport(),
                error: None,
            });
        }
        Err(e) => {
            let _ = client_event_tx.send(ServerEvent::TransportChanged {
                id,
                transport: None,
                error: Some(e.to_string()),
            });
        }
    }
}

pub(super) async fn handle_set_compaction_mode(
    id: u64,
    mode: crate::config::CompactionMode,
    agent: &Arc<Mutex<Agent>>,
    client_event_tx: &mpsc::UnboundedSender<ServerEvent>,
) {
    let result = {
        let agent_guard = agent.lock().await;
        agent_guard
            .set_compaction_mode(mode.clone())
            .await
            .map(|_| ())
    };

    match result {
        Ok(()) => {
            let updated_mode = {
                let agent_guard = agent.lock().await;
                agent_guard.compaction_mode().await
            };
            let _ = client_event_tx.send(ServerEvent::CompactionModeChanged {
                id,
                mode: updated_mode,
                error: None,
            });
        }
        Err(e) => {
            let fallback_mode = {
                let agent_guard = agent.lock().await;
                agent_guard.compaction_mode().await
            };
            let _ = client_event_tx.send(ServerEvent::CompactionModeChanged {
                id,
                mode: fallback_mode,
                error: Some(e.to_string()),
            });
        }
    }
}

pub(super) async fn handle_notify_auth_changed(
    id: u64,
    provider: &Arc<dyn Provider>,
    agent: &Arc<Mutex<Agent>>,
    client_event_tx: &mpsc::UnboundedSender<ServerEvent>,
) {
    crate::auth::AuthStatus::invalidate_cache();
    let provider_clone = provider.clone();
    let client_event_tx_clone = client_event_tx.clone();
    let agent_clone = agent.clone();
    tokio::spawn(async move {
        provider_clone.on_auth_changed();
        let mut bus_rx = crate::bus::Bus::global().subscribe();
        let deadline = tokio::time::Instant::now() + std::time::Duration::from_secs(10);
        loop {
            let remaining = deadline.saturating_duration_since(tokio::time::Instant::now());
            if remaining.is_zero() {
                break;
            }
            tokio::select! {
                event = bus_rx.recv() => {
                    if matches!(event, Ok(crate::bus::BusEvent::ModelsUpdated)) {
                        break;
                    }
                }
                _ = tokio::time::sleep(remaining) => break,
            }
        }
        let (models, model_routes) = {
            let agent_guard = agent_clone.lock().await;
            (
                agent_guard.available_models_display(),
                agent_guard.model_routes(),
            )
        };
        let _ = client_event_tx_clone.send(ServerEvent::AvailableModelsUpdated {
            available_models: models,
            available_model_routes: model_routes,
        });
    });
    let _ = client_event_tx.send(ServerEvent::Done { id });
}

pub(super) async fn handle_switch_anthropic_account(
    id: u64,
    label: String,
    agent: &Arc<Mutex<Agent>>,
    client_event_tx: &mpsc::UnboundedSender<ServerEvent>,
) {
    match crate::auth::claude::set_active_account(&label) {
        Ok(()) => {
            crate::auth::AuthStatus::invalidate_cache();

            {
                let agent_guard = agent.lock().await;
                let provider = agent_guard.provider_handle();
                drop(agent_guard);
                provider.invalidate_credentials().await;
            }

            crate::provider::clear_all_provider_unavailability_for_account();
            crate::provider::clear_all_model_unavailability_for_account();

            {
                let mut agent_guard = agent.lock().await;
                agent_guard.reset_provider_session();
            }

            tokio::spawn(async {
                let _ = crate::usage::get().await;
            });

            let _ = client_event_tx.send(ServerEvent::Done { id });
        }
        Err(e) => {
            let _ = client_event_tx.send(ServerEvent::Error {
                id,
                message: format!("Failed to switch Anthropic account: {}", e),
                retry_after_secs: None,
            });
        }
    }
}
