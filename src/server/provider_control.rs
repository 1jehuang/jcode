use crate::agent::Agent;
use crate::protocol::ServerEvent;
use crate::provider::Provider;
use std::sync::Arc;
use tokio::sync::{Mutex, mpsc};

async fn model_switching_available(agent: &Arc<Mutex<Agent>>) -> Option<String> {
    let models = {
        let agent_guard = agent.lock().await;
        agent_guard.available_models_for_switching()
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
        agent_guard.available_models_for_switching()
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
    let next_model = models[next_index].clone();

    let result = {
        let mut agent_guard = agent.lock().await;
        let result = agent_guard.set_model(&next_model);
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

pub(super) async fn handle_refresh_models(
    id: u64,
    provider: &Arc<dyn Provider>,
    agent: &Arc<Mutex<Agent>>,
    client_event_tx: &mpsc::UnboundedSender<ServerEvent>,
) {
    let provider_clone = provider.clone();
    let agent_clone = agent.clone();
    let client_event_tx_clone = client_event_tx.clone();
    tokio::spawn(async move {
        let result = provider_clone.prefetch_models().await;
        match result {
            Ok(()) => {
                crate::bus::Bus::global().publish(crate::bus::BusEvent::ModelsUpdated);
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
            }
            Err(err) => {
                let _ = client_event_tx_clone.send(ServerEvent::Error {
                    id,
                    message: format!("Failed to refresh models: {}", err),
                    retry_after_secs: None,
                });
            }
        }
    });
    let _ = client_event_tx.send(ServerEvent::Done { id });
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
        let mut bus_rx = crate::bus::Bus::global().subscribe();
        provider_clone.on_auth_changed();
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

#[cfg(test)]
mod tests {
    use super::*;
    use crate::message::{Message, StreamEvent, ToolDefinition};
    use crate::provider::{EventStream, ModelRoute, Provider};
    use crate::tool::Registry;
    use async_trait::async_trait;
    use std::pin::Pin;
    use std::sync::RwLock as StdRwLock;

    #[derive(Default)]
    struct AuthChangeMockState {
        logged_in: StdRwLock<bool>,
    }

    struct AuthChangeMockProvider {
        state: Arc<AuthChangeMockState>,
    }

    impl AuthChangeMockProvider {
        fn new() -> Self {
            Self {
                state: Arc::new(AuthChangeMockState::default()),
            }
        }
    }

    #[async_trait]
    impl Provider for AuthChangeMockProvider {
        async fn complete(
            &self,
            _messages: &[Message],
            _tools: &[ToolDefinition],
            _system: &str,
            _resume_session_id: Option<&str>,
        ) -> anyhow::Result<EventStream> {
            let stream = futures::stream::empty::<anyhow::Result<StreamEvent>>();
            Ok(Box::pin(stream) as Pin<Box<dyn futures::Stream<Item = _> + Send>>)
        }

        fn name(&self) -> &str {
            "mock-auth"
        }

        fn model(&self) -> String {
            if *self.state.logged_in.read().unwrap() {
                "logged-in-model".to_string()
            } else {
                "logged-out-model".to_string()
            }
        }

        fn available_models_display(&self) -> Vec<String> {
            if *self.state.logged_in.read().unwrap() {
                vec!["logged-in-model".to_string(), "second-model".to_string()]
            } else {
                vec!["logged-out-model".to_string()]
            }
        }

        fn model_routes(&self) -> Vec<ModelRoute> {
            self.available_models_display()
                .into_iter()
                .map(|model| ModelRoute {
                    model,
                    provider: "MockAuth".to_string(),
                    api_method: "mock-auth".to_string(),
                    available: true,
                    detail: String::new(),
                    cheapness: None,
                })
                .collect()
        }

        fn on_auth_changed(&self) {
            *self.state.logged_in.write().unwrap() = true;
            crate::bus::Bus::global().publish(crate::bus::BusEvent::ModelsUpdated);
        }

        fn fork(&self) -> Arc<dyn Provider> {
            Arc::new(Self {
                state: Arc::clone(&self.state),
            })
        }
    }

    #[tokio::test]
    async fn notify_auth_changed_emits_available_models_updated_after_provider_update() {
        let provider: Arc<dyn Provider> = Arc::new(AuthChangeMockProvider::new());
        let registry = Registry::empty();
        let agent = Arc::new(Mutex::new(Agent::new(provider.clone(), registry)));
        let (client_event_tx, mut client_event_rx) = mpsc::unbounded_channel();

        handle_notify_auth_changed(42, &provider, &agent, &client_event_tx).await;

        let mut saw_done = false;
        let mut saw_models = None;
        let deadline = tokio::time::Instant::now() + std::time::Duration::from_secs(2);
        while tokio::time::Instant::now() < deadline {
            let remaining = deadline.saturating_duration_since(tokio::time::Instant::now());
            let event = tokio::time::timeout(remaining, client_event_rx.recv())
                .await
                .expect("receive server event before timeout");
            match event.expect("channel open") {
                ServerEvent::Done { id } => {
                    assert_eq!(id, 42);
                    saw_done = true;
                }
                ServerEvent::AvailableModelsUpdated {
                    available_models,
                    available_model_routes,
                } => {
                    saw_models = Some((available_models, available_model_routes));
                    break;
                }
                _ => {}
            }
        }

        assert!(saw_done, "expected immediate Done ack");
        let (available_models, available_model_routes) =
            saw_models.expect("expected AvailableModelsUpdated event");
        assert_eq!(
            available_models,
            vec!["logged-in-model".to_string(), "second-model".to_string()]
        );
        assert!(available_model_routes.iter().any(|route| {
            route.model == "logged-in-model"
                && route.provider == "MockAuth"
                && route.api_method == "mock-auth"
        }));
    }

    #[tokio::test]
    async fn refresh_models_emits_available_models_updated_after_prefetch() {
        let provider: Arc<dyn Provider> = Arc::new(AuthChangeMockProvider::new());
        let registry = Registry::empty();
        let agent = Arc::new(Mutex::new(Agent::new(provider.clone(), registry)));
        let (client_event_tx, mut client_event_rx) = mpsc::unbounded_channel();

        handle_refresh_models(7, &provider, &agent, &client_event_tx).await;

        let mut saw_done = false;
        let mut saw_models = None;
        let deadline = tokio::time::Instant::now() + std::time::Duration::from_secs(2);
        while tokio::time::Instant::now() < deadline {
            let remaining = deadline.saturating_duration_since(tokio::time::Instant::now());
            let event = tokio::time::timeout(remaining, client_event_rx.recv())
                .await
                .expect("receive server event before timeout");
            match event.expect("channel open") {
                ServerEvent::Done { id } => {
                    assert_eq!(id, 7);
                    saw_done = true;
                }
                ServerEvent::AvailableModelsUpdated {
                    available_models,
                    available_model_routes,
                } => {
                    saw_models = Some((available_models, available_model_routes));
                    break;
                }
                _ => {}
            }
        }

        assert!(saw_done, "expected immediate Done ack");
        let (available_models, available_model_routes) =
            saw_models.expect("expected AvailableModelsUpdated event");
        assert_eq!(available_models, vec!["logged-out-model".to_string()]);
        assert!(available_model_routes.iter().any(|route| {
            route.model == "logged-out-model"
                && route.provider == "MockAuth"
                && route.api_method == "mock-auth"
        }));
    }
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

pub(super) async fn handle_switch_openai_account(
    id: u64,
    label: String,
    agent: &Arc<Mutex<Agent>>,
    client_event_tx: &mpsc::UnboundedSender<ServerEvent>,
) {
    match crate::auth::codex::set_active_account(&label) {
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
                let _ = crate::usage::get_openai_usage().await;
            });

            let _ = client_event_tx.send(ServerEvent::Done { id });
        }
        Err(e) => {
            let _ = client_event_tx.send(ServerEvent::Error {
                id,
                message: format!("Failed to switch OpenAI account: {}", e),
                retry_after_secs: None,
            });
        }
    }
}
