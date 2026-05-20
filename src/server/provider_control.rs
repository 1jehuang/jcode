#![cfg_attr(test, allow(clippy::items_after_test_module))]

use crate::agent::Agent;
use crate::auth::lifecycle::{AuthActivationRequest, AuthActivationResult};
use crate::protocol::{AuthChanged, ServerEvent};
use crate::provider::{ModelCatalogRefreshSummary, ModelRoute, Provider};
use std::collections::HashMap;
use std::sync::Arc;
use tokio::sync::{Mutex, RwLock, mpsc};

type SessionAgents = Arc<RwLock<HashMap<String, Arc<Mutex<Agent>>>>>;

struct AuthRefreshTargets {
    providers: Vec<Arc<dyn Provider>>,
    session_providers: Vec<Arc<dyn Provider>>,
    deferred_agents: Vec<Arc<Mutex<Agent>>>,
}

#[derive(Clone)]
struct AvailableModelsSnapshot {
    provider_name: Option<String>,
    provider_model: Option<String>,
    available_models: Vec<String>,
    available_model_routes: Vec<ModelRoute>,
}

impl AvailableModelsSnapshot {
    fn from_agent(agent: &Agent) -> Self {
        Self {
            provider_name: Some(agent.provider_name()),
            provider_model: Some(agent.provider_model()),
            available_models: agent.available_models_display(),
            available_model_routes: agent.model_routes(),
        }
    }

    fn into_event(self) -> ServerEvent {
        ServerEvent::AvailableModelsUpdated {
            provider_name: self.provider_name,
            provider_model: self.provider_model,
            available_models: self.available_models,
            available_model_routes: self.available_model_routes,
        }
    }
}

fn available_models_updated_event_from_agent(agent: &Agent) -> ServerEvent {
    AvailableModelsSnapshot::from_agent(agent).into_event()
}

async fn available_models_snapshot(agent: &Arc<Mutex<Agent>>) -> AvailableModelsSnapshot {
    let agent_guard = agent.lock().await;
    AvailableModelsSnapshot::from_agent(&agent_guard)
}

pub(super) async fn available_models_updated_event(agent: &Arc<Mutex<Agent>>) -> ServerEvent {
    let agent_guard = agent.lock().await;
    available_models_updated_event_from_agent(&agent_guard)
}

pub(super) fn try_available_models_updated_event(agent: &Arc<Mutex<Agent>>) -> Option<ServerEvent> {
    let agent_guard = agent.try_lock().ok()?;
    Some(available_models_updated_event_from_agent(&agent_guard))
}

fn format_model_name_list(models: &[String], limit: usize) -> String {
    let shown = models
        .iter()
        .take(limit)
        .map(|model| format!("`{}`", model))
        .collect::<Vec<_>>()
        .join(", ");
    if models.len() > limit {
        format!("{} … and {} more", shown, models.len() - limit)
    } else {
        shown
    }
}

fn format_auth_catalog_refresh_complete(
    provider_name: Option<&str>,
    provider_model: Option<&str>,
    summary: &ModelCatalogRefreshSummary,
) -> String {
    let provider_label = provider_name.unwrap_or("provider");
    let mut message = format!(
        "**Auth Model Catalog Updated**\n\n{} credentials are active. Catalog diff:\n\nModels: {} → {}  (+{} / -{})\nRoutes: {} → {}  (+{} / -{} / ~{})",
        provider_label,
        summary.model_count_before,
        summary.model_count_after,
        summary.models_added,
        summary.models_removed,
        summary.route_count_before,
        summary.route_count_after,
        summary.routes_added,
        summary.routes_removed,
        summary.routes_changed,
    );
    if !summary.models_added_names.is_empty() {
        message.push_str("\nAdded models: ");
        message.push_str(&format_model_name_list(&summary.models_added_names, 12));
    }
    if !summary.models_removed_names.is_empty() {
        message.push_str("\nRemoved models: ");
        message.push_str(&format_model_name_list(&summary.models_removed_names, 12));
    }
    if let Some(model) = provider_model {
        message.push_str(&format!("\n\nSelected model: `{}`.", model));
    }
    message.push_str("\n\nUse `/model` if you want to choose a different accessible model.");
    message
}

fn auth_model_refresh_quiet_period() -> std::time::Duration {
    if cfg!(test) {
        std::time::Duration::from_millis(20)
    } else {
        std::time::Duration::from_millis(750)
    }
}

async fn auth_refresh_targets(
    provider_template: &Arc<dyn Provider>,
    current_provider: &Arc<dyn Provider>,
    sessions: &SessionAgents,
) -> AuthRefreshTargets {
    fn push_unique(handles: &mut Vec<Arc<dyn Provider>>, provider: Arc<dyn Provider>) {
        if !handles
            .iter()
            .any(|existing| Arc::ptr_eq(existing, &provider))
        {
            handles.push(provider);
        }
    }

    let mut handles = Vec::new();
    let mut session_handles = Vec::new();
    let mut deferred_agents = Vec::new();
    push_unique(&mut handles, Arc::clone(provider_template));
    push_unique(&mut handles, Arc::clone(current_provider));

    let agents: Vec<Arc<Mutex<Agent>>> = {
        let sessions_guard = sessions.read().await;
        sessions_guard.values().cloned().collect()
    };

    for agent in agents {
        let Ok(agent_guard) = agent.try_lock() else {
            crate::logging::info(
                "Deferring busy session provider auth-change refresh until the session is idle",
            );
            deferred_agents.push(agent);
            continue;
        };
        let provider = agent_guard.provider_handle();
        if handles
            .iter()
            .any(|existing| Arc::ptr_eq(existing, &provider))
        {
            continue;
        }
        push_unique(&mut session_handles, provider);
    }

    AuthRefreshTargets {
        providers: handles,
        session_providers: session_handles,
        deferred_agents,
    }
}

fn spawn_deferred_auth_refreshes(agents: Vec<Arc<Mutex<Agent>>>) {
    for agent in agents {
        tokio::spawn(async move {
            let provider = {
                let agent_guard = agent.lock().await;
                agent_guard.provider_handle()
            };
            provider.on_auth_changed_preserve_current_provider();
            crate::bus::Bus::global().publish_models_updated();
        });
    }
}

async fn apply_auth_runtime_model_to_agent(
    activation: &AuthActivationResult,
    model: Option<&str>,
    agent: &Arc<Mutex<Agent>>,
) {
    let Some(model) = model.map(str::trim).filter(|model| !model.is_empty()) else {
        return;
    };

    let provider = activation.provider_id.as_deref().unwrap_or("auth");
    let result = {
        let mut agent_guard = agent.lock().await;
        let provider_name = agent_guard.provider_handle().name().to_string();
        let model_request = activation.model_switch_request(&provider_name, model);
        let result = agent_guard.set_model_from_auth(&model_request);
        if result.is_ok() {
            agent_guard.reset_provider_session();
        }
        result.map(|_| agent_guard.provider_model())
    };

    match result {
        Ok(_) => crate::logging::auth_event(
            "auth_changed_runtime_model_applied",
            provider,
            &[("provider_session", "reset")],
        ),
        Err(error) => {
            let message = error.to_string();
            crate::logging::auth_event(
                "auth_changed_runtime_model_failed",
                provider,
                &[("reason", message.as_str())],
            );
        }
    }
}

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
        let result = provider_clone.refresh_model_catalog().await;
        match result {
            Ok(_) => {
                crate::bus::Bus::global().publish_models_updated();
                let event = available_models_updated_event(&agent_clone).await;
                let _ = client_event_tx_clone.send(event);
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
    let result = {
        let mut agent_guard = agent.lock().await;
        agent_guard.set_reasoning_effort(&effort)
    };

    match result {
        Ok(effort) => {
            let _ = client_event_tx.send(ServerEvent::ReasoningEffortChanged {
                id,
                effort,
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

#[allow(clippy::too_many_arguments)]
pub(super) async fn handle_notify_auth_changed(
    id: u64,
    provider_hint: Option<String>,
    auth: Option<AuthChanged>,
    provider: &Arc<dyn Provider>,
    provider_template: &Arc<dyn Provider>,
    sessions: &SessionAgents,
    agent: &Arc<Mutex<Agent>>,
    client_event_tx: &mpsc::UnboundedSender<ServerEvent>,
) {
    crate::auth::AuthStatus::invalidate_cache();
    let session_id = {
        let agent_guard = agent.lock().await;
        agent_guard.session_id().to_string()
    };
    let activation_request = AuthActivationRequest::new(provider_hint, auth);
    crate::bus::Bus::global().publish(crate::bus::BusEvent::UiActivity(
        crate::bus::UiActivity::auth(
            Some(session_id.clone()),
            "**Auth Change Received**\n\nThe server is reloading provider credentials and refreshing model route availability for this session.",
            Some("Auth: refreshing providers..."),
        ),
    ));
    let targets = auth_refresh_targets(provider_template, provider, sessions).await;
    let client_event_tx_clone = client_event_tx.clone();
    let agent_clone = agent.clone();
    let before_snapshot = available_models_snapshot(agent).await;
    tokio::spawn(async move {
        let activation = crate::auth::lifecycle::activate_auth_change(&activation_request);
        let mut bus_rx = crate::bus::Bus::global().subscribe();
        for provider in targets.providers {
            provider.on_auth_changed();
        }
        for provider in targets.session_providers {
            provider.on_auth_changed_preserve_current_provider();
        }

        // Auth refresh is global so every live session learns about newly
        // configured credentials, but the automatic post-login model switch is
        // session-local. A user logging Groq/Cerebras into one workspace should
        // not silently move unrelated sessions off their chosen provider/model.
        apply_auth_runtime_model_to_agent(
            &activation,
            activation.activated_model.as_deref(),
            &agent_clone,
        )
        .await;
        let auth_selection_generation = {
            let agent_guard = agent_clone.lock().await;
            agent_guard.provider_model_selection_generation()
        };

        crate::bus::Bus::global().publish_models_updated();
        crate::bus::Bus::global().publish(crate::bus::BusEvent::UiActivity(
            crate::bus::UiActivity::catalog(
                Some(session_id.clone()),
                "**Auth Model Routes Updating**\n\nCredentials are reloaded. Jcode is pushing an updated model catalog snapshot to connected clients.",
                Some("Auth: model routes updating..."),
            ),
        ));

        spawn_deferred_auth_refreshes(targets.deferred_agents);

        // Hot-initializing providers is synchronous, while dynamic catalogs may
        // continue refreshing in the background. Push an immediate snapshot so
        // the model picker/header stop looking stale right after login, then
        // push another snapshot when the background refresh announces itself.
        let mut latest_snapshot = available_models_snapshot(&agent_clone).await;
        let _ = client_event_tx_clone.send(latest_snapshot.clone().into_event());

        let max_deadline = tokio::time::Instant::now() + std::time::Duration::from_secs(10);
        let quiet_period = auth_model_refresh_quiet_period();
        let mut quiet_deadline: Option<tokio::time::Instant> = None;
        loop {
            let now = tokio::time::Instant::now();
            let deadline = quiet_deadline
                .map(|quiet| std::cmp::min(max_deadline, quiet))
                .unwrap_or(max_deadline);
            let remaining = deadline.saturating_duration_since(now);
            if remaining.is_zero() {
                break;
            }
            tokio::select! {
                event = bus_rx.recv() => {
                    if matches!(event, Ok(crate::bus::BusEvent::ModelsUpdated)) {
                        latest_snapshot = available_models_snapshot(&agent_clone).await;
                        let _ = client_event_tx_clone.send(latest_snapshot.clone().into_event());
                        quiet_deadline = Some(tokio::time::Instant::now() + quiet_period);
                    }
                }
                _ = tokio::time::sleep(remaining) => break,
            }
        }

        let manual_model_selected_during_auth_refresh = {
            let agent_guard = agent_clone.lock().await;
            agent_guard.user_selected_provider_model_after(auth_selection_generation)
        };
        if manual_model_selected_during_auth_refresh {
            crate::logging::auth_event(
                "auth_changed_auto_model_skipped_after_manual_switch",
                activation.provider_id.as_deref().unwrap_or("auth"),
                &[("reason", "user_selected_provider_model_during_refresh")],
            );
            latest_snapshot = available_models_snapshot(&agent_clone).await;
            let _ = client_event_tx_clone.send(latest_snapshot.clone().into_event());
        } else if let Some(model_to_select) =
            crate::auth::lifecycle::provider_model_to_select_after_auth(
                &activation,
                latest_snapshot.provider_model.as_deref(),
                &latest_snapshot.available_model_routes,
            )
        {
            apply_auth_runtime_model_to_agent(&activation, Some(&model_to_select), &agent_clone)
                .await;
            latest_snapshot = available_models_snapshot(&agent_clone).await;
            let _ = client_event_tx_clone.send(latest_snapshot.clone().into_event());
        }

        let summary = crate::provider::summarize_model_catalog_refresh(
            before_snapshot.available_models,
            latest_snapshot.available_models.clone(),
            before_snapshot.available_model_routes,
            latest_snapshot.available_model_routes.clone(),
        );
        let catalog_invariants = crate::auth::lifecycle::validate_catalog_invariants(
            &activation,
            latest_snapshot.provider_model.as_deref(),
            &latest_snapshot.available_model_routes,
        );
        let mut catalog_message = format_auth_catalog_refresh_complete(
            activation
                .provider_label
                .as_deref()
                .or(latest_snapshot.provider_name.as_deref()),
            latest_snapshot.provider_model.as_deref(),
            &summary,
        );
        if let Some(warning) = catalog_invariants.warning_message() {
            catalog_message.push_str(&warning);
        }
        crate::bus::Bus::global().publish(crate::bus::BusEvent::UiActivity(
            crate::bus::UiActivity::catalog(
                Some(session_id),
                catalog_message,
                Some("Auth: model catalog updated"),
            ),
        ));
    });
    let _ = client_event_tx.send(ServerEvent::Done { id });
}

#[cfg(test)]
#[path = "provider_control_tests.rs"]
mod provider_control_tests;

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

            {
                let agent_clone = Arc::clone(agent);
                let client_event_tx_clone = client_event_tx.clone();
                tokio::spawn(async move {
                    crate::bus::Bus::global().publish_models_updated();
                    let event = available_models_updated_event(&agent_clone).await;
                    let _ = client_event_tx_clone.send(event);
                });
            }

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

            {
                let agent_clone = Arc::clone(agent);
                let client_event_tx_clone = client_event_tx.clone();
                tokio::spawn(async move {
                    crate::bus::Bus::global().publish_models_updated();
                    let event = available_models_updated_event(&agent_clone).await;
                    let _ = client_event_tx_clone.send(event);
                });
            }

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
