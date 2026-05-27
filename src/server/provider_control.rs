#![cfg_attr(test, allow(clippy::items_after_test_module))]

use crate::agent::Agent;
use crate::auth::lifecycle::{AuthActivationRequest, AuthActivationResult};
use crate::protocol::{AuthChanged, NotificationType, ServerEvent};
use crate::provider::{ModelCatalogRefreshSummary, ModelRoute, Provider};
use std::collections::HashMap;
use std::sync::Arc;
use std::time::Instant;
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

fn available_models_snapshot_from_provider(
    provider: &Arc<dyn Provider>,
) -> AvailableModelsSnapshot {
    AvailableModelsSnapshot {
        provider_name: Some(provider.name().to_string()),
        provider_model: Some(provider.model()),
        available_models: provider.available_models_display(),
        available_model_routes: provider.model_routes(),
    }
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

fn log_provider_control_deferred(operation: &'static str, id: u64) -> Instant {
    let queued_at = Instant::now();
    crate::logging::event_warn(
        "SERVER_PROVIDER_CONTROL_DEFERRED",
        vec![
            ("phase", "queued".to_string()),
            ("operation", operation.to_string()),
            ("request_id", id.to_string()),
            ("reason", "agent_busy".to_string()),
        ],
    );
    queued_at
}

fn log_provider_control_lock_acquired(operation: &'static str, id: u64, queued_at: Instant) {
    crate::logging::event_info(
        "SERVER_PROVIDER_CONTROL_DEFERRED",
        vec![
            ("phase", "lock_acquired".to_string()),
            ("operation", operation.to_string()),
            ("request_id", id.to_string()),
            ("wait_ms", queued_at.elapsed().as_millis().to_string()),
        ],
    );
}

fn log_provider_control_completed(operation: &'static str, id: u64, queued_at: Instant) {
    crate::logging::event_info(
        "SERVER_PROVIDER_CONTROL_DEFERRED",
        vec![
            ("phase", "completed".to_string()),
            ("operation", operation.to_string()),
            ("request_id", id.to_string()),
            ("total_ms", queued_at.elapsed().as_millis().to_string()),
        ],
    );
}

fn spawn_deferred_agent_mutation<F>(
    operation: &'static str,
    id: u64,
    agent: Arc<Mutex<Agent>>,
    client_event_tx: mpsc::UnboundedSender<ServerEvent>,
    apply: F,
) where
    F: FnOnce(&mut Agent, &mpsc::UnboundedSender<ServerEvent>) + Send + 'static,
{
    let queued_at = log_provider_control_deferred(operation, id);
    tokio::spawn(async move {
        let mut agent_guard = agent.lock().await;
        log_provider_control_lock_acquired(operation, id, queued_at);
        apply(&mut agent_guard, &client_event_tx);
        log_provider_control_completed(operation, id, queued_at);
    });
}

fn spawn_deferred_provider_operation<F>(
    operation: &'static str,
    id: u64,
    agent: Arc<Mutex<Agent>>,
    client_event_tx: mpsc::UnboundedSender<ServerEvent>,
    apply: F,
) where
    F: FnOnce(Arc<dyn Provider>, &mpsc::UnboundedSender<ServerEvent>) + Send + 'static,
{
    let queued_at = log_provider_control_deferred(operation, id);
    tokio::spawn(async move {
        let provider = {
            let agent_guard = agent.lock().await;
            log_provider_control_lock_acquired(operation, id, queued_at);
            agent_guard.provider_handle()
        };
        apply(provider, &client_event_tx);
        log_provider_control_completed(operation, id, queued_at);
    });
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

fn model_switching_unavailable_current(agent: &Agent) -> Option<String> {
    if agent.available_models_for_switching().is_empty() {
        Some(agent.provider_model())
    } else {
        None
    }
}

fn send_model_changed_result(
    id: u64,
    result: anyhow::Result<(String, String)>,
    fallback_model: String,
    client_event_tx: &mpsc::UnboundedSender<ServerEvent>,
) {
    match result {
        Ok((updated, provider_name)) => {
            crate::telemetry::record_model_switch();
            let _ = client_event_tx.send(ServerEvent::ModelChanged {
                id,
                model: updated,
                provider_name: Some(provider_name),
                error: None,
            });
        }
        Err(error) => {
            let _ = client_event_tx.send(ServerEvent::ModelChanged {
                id,
                model: fallback_model,
                provider_name: None,
                error: Some(error.to_string()),
            });
        }
    }
}

fn apply_cycle_model(
    id: u64,
    direction: i8,
    agent: &mut Agent,
    client_event_tx: &mpsc::UnboundedSender<ServerEvent>,
) {
    let models = agent.available_models_for_switching();
    if models.is_empty() {
        let _ = client_event_tx.send(ServerEvent::ModelChanged {
            id,
            model: agent.provider_model(),
            provider_name: None,
            error: Some("Model switching is not available for this provider.".to_string()),
        });
        return;
    }

    let current = agent.provider_model();
    let current_index = models.iter().position(|m| *m == current).unwrap_or(0);
    let len = models.len();
    let next_index = if direction >= 0 {
        (current_index + 1) % len
    } else {
        (current_index + len - 1) % len
    };
    let next_model = models[next_index].clone();
    let result = {
        let result = agent.set_model(&next_model);
        if result.is_ok() {
            agent.reset_provider_session();
        }
        result.map(|_| (agent.provider_model(), agent.provider_name()))
    };
    send_model_changed_result(id, result, current, client_event_tx);
}

pub(super) async fn handle_cycle_model(
    id: u64,
    direction: i8,
    agent: &Arc<Mutex<Agent>>,
    client_event_tx: &mpsc::UnboundedSender<ServerEvent>,
) {
    if let Ok(mut agent_guard) = agent.try_lock() {
        apply_cycle_model(id, direction, &mut agent_guard, client_event_tx);
    } else {
        spawn_deferred_agent_mutation(
            "cycle_model",
            id,
            Arc::clone(agent),
            client_event_tx.clone(),
            move |agent_guard, client_event_tx| {
                apply_cycle_model(id, direction, agent_guard, client_event_tx);
            },
        );
    }
}

fn premium_mode_label(mode: crate::provider::copilot::PremiumMode) -> &'static str {
    use crate::provider::copilot::PremiumMode;
    match mode {
        PremiumMode::Zero => "zero premium requests",
        PremiumMode::OnePerSession => "one premium per session",
        PremiumMode::Normal => "normal",
    }
}

fn apply_set_premium_mode(
    id: u64,
    mode: u8,
    premium_mode: crate::provider::copilot::PremiumMode,
    agent: &Agent,
    client_event_tx: &mpsc::UnboundedSender<ServerEvent>,
) {
    agent.set_premium_mode(premium_mode);
    crate::logging::info(&format!(
        "Server: premium mode set to {} ({})",
        mode,
        premium_mode_label(premium_mode)
    ));
    let _ = client_event_tx.send(ServerEvent::Ack { id });
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
    if let Ok(agent_guard) = agent.try_lock() {
        apply_set_premium_mode(id, mode, premium_mode, &agent_guard, client_event_tx);
    } else {
        spawn_deferred_agent_mutation(
            "set_premium_mode",
            id,
            Arc::clone(agent),
            client_event_tx.clone(),
            move |agent_guard, client_event_tx| {
                apply_set_premium_mode(id, mode, premium_mode, agent_guard, client_event_tx);
            },
        );
    }
}

fn apply_set_model(
    id: u64,
    model: String,
    agent: &mut Agent,
    client_event_tx: &mpsc::UnboundedSender<ServerEvent>,
) {
    if let Some(current) = model_switching_unavailable_current(agent) {
        let _ = client_event_tx.send(ServerEvent::ModelChanged {
            id,
            model: current,
            provider_name: None,
            error: Some("Model switching is not available for this provider.".to_string()),
        });
        return;
    }

    let current = agent.provider_model();
    let result = {
        let result = agent.set_model(&model);
        if result.is_ok() {
            agent.reset_provider_session();
        }
        result.map(|_| (agent.provider_model(), agent.provider_name()))
    };
    send_model_changed_result(id, result, current, client_event_tx);
}

pub(super) async fn handle_set_model(
    id: u64,
    model: String,
    agent: &Arc<Mutex<Agent>>,
    client_event_tx: &mpsc::UnboundedSender<ServerEvent>,
) {
    if let Ok(mut agent_guard) = agent.try_lock() {
        apply_set_model(id, model, &mut agent_guard, client_event_tx);
    } else {
        spawn_deferred_agent_mutation(
            "set_model",
            id,
            Arc::clone(agent),
            client_event_tx.clone(),
            move |agent_guard, client_event_tx| {
                apply_set_model(id, model, agent_guard, client_event_tx);
            },
        );
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
        send_catalog_activity(
            &client_event_tx_clone,
            &crate::message::format_model_refresh_progress_markdown(
                "Starting provider model catalog refresh",
                Some(5),
            ),
        );

        let refresh_started = Instant::now();
        let refresh_future = provider_clone.refresh_model_catalog();
        tokio::pin!(refresh_future);
        let mut heartbeat = tokio::time::interval(std::time::Duration::from_secs(2));
        heartbeat.set_missed_tick_behavior(tokio::time::MissedTickBehavior::Skip);

        let result = loop {
            tokio::select! {
                result = &mut refresh_future => break result,
                _ = heartbeat.tick() => {
                    let elapsed_secs = refresh_started.elapsed().as_secs();
                    if elapsed_secs > 0 {
                        send_catalog_activity(
                            &client_event_tx_clone,
                            &crate::message::format_model_refresh_progress_markdown(
                                &format!("Waiting on provider APIs ({elapsed_secs}s elapsed)"),
                                None,
                            ),
                        );
                    }
                }
            }
        };
        match result {
            Ok(_) => {
                send_catalog_activity(
                    &client_event_tx_clone,
                    &crate::message::format_model_refresh_progress_markdown(
                        "Updating model picker",
                        Some(95),
                    ),
                );
                crate::bus::Bus::global().publish_models_updated();
                let event = available_models_updated_event(&agent_clone).await;
                let _ = client_event_tx_clone.send(event);
                send_catalog_activity(
                    &client_event_tx_clone,
                    &crate::message::format_model_refresh_progress_markdown(
                        "Model list refresh complete",
                        Some(100),
                    ),
                );
            }
            Err(err) => {
                send_catalog_activity(
                    &client_event_tx_clone,
                    &crate::message::format_model_refresh_progress_markdown(
                        "Model list refresh failed",
                        None,
                    ),
                );
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

fn send_catalog_activity(client_event_tx: &mpsc::UnboundedSender<ServerEvent>, message: &str) {
    let _ = client_event_tx.send(ServerEvent::Notification {
        from_session: "jcode".to_string(),
        from_name: Some("Jcode".to_string()),
        notification_type: NotificationType::Message {
            scope: Some("catalog_activity".to_string()),
            channel: None,
        },
        message: message.to_string(),
    });
}

pub(super) async fn handle_set_reasoning_effort(
    id: u64,
    effort: String,
    agent: &Arc<Mutex<Agent>>,
    client_event_tx: &mpsc::UnboundedSender<ServerEvent>,
) {
    let result = if let Ok(mut agent_guard) = agent.try_lock() {
        agent_guard.set_reasoning_effort(&effort)
    } else {
        spawn_deferred_reasoning_effort_change(
            id,
            effort,
            Arc::clone(agent),
            client_event_tx.clone(),
        );
        return;
    };

    send_reasoning_effort_result(id, result, client_event_tx);
}

fn send_reasoning_effort_result(
    id: u64,
    result: anyhow::Result<Option<String>>,
    client_event_tx: &mpsc::UnboundedSender<ServerEvent>,
) {
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

fn spawn_deferred_reasoning_effort_change(
    id: u64,
    effort: String,
    agent: Arc<Mutex<Agent>>,
    client_event_tx: mpsc::UnboundedSender<ServerEvent>,
) {
    let queued_at = log_provider_control_deferred("set_reasoning_effort", id);
    tokio::spawn(async move {
        let mut agent_guard = agent.lock().await;
        log_provider_control_lock_acquired("set_reasoning_effort", id, queued_at);
        let result = agent_guard.set_reasoning_effort(&effort);
        crate::logging::info(&format!(
            "Deferred reasoning effort change completed request_id={} requested={} success={}",
            id,
            effort,
            result.is_ok()
        ));
        send_reasoning_effort_result(id, result, &client_event_tx);
        log_provider_control_completed("set_reasoning_effort", id, queued_at);
    });
}

pub(super) async fn handle_set_service_tier(
    id: u64,
    service_tier: String,
    agent: &Arc<Mutex<Agent>>,
    client_event_tx: &mpsc::UnboundedSender<ServerEvent>,
) {
    let apply = move |provider: Arc<dyn Provider>,
                      client_event_tx: &mpsc::UnboundedSender<ServerEvent>| {
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
    };

    if let Ok(agent_guard) = agent.try_lock() {
        apply(agent_guard.provider_handle(), client_event_tx);
    } else {
        spawn_deferred_provider_operation(
            "set_service_tier",
            id,
            Arc::clone(agent),
            client_event_tx.clone(),
            apply,
        );
    }
}

pub(super) async fn handle_set_transport(
    id: u64,
    transport: String,
    agent: &Arc<Mutex<Agent>>,
    client_event_tx: &mpsc::UnboundedSender<ServerEvent>,
) {
    let apply = move |provider: Arc<dyn Provider>,
                      client_event_tx: &mpsc::UnboundedSender<ServerEvent>| {
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
    };

    if let Ok(agent_guard) = agent.try_lock() {
        apply(agent_guard.provider_handle(), client_event_tx);
    } else {
        spawn_deferred_provider_operation(
            "set_transport",
            id,
            Arc::clone(agent),
            client_event_tx.clone(),
            apply,
        );
    }
}

pub(super) async fn handle_set_compaction_mode(
    id: u64,
    mode: crate::config::CompactionMode,
    agent: &Arc<Mutex<Agent>>,
    client_event_tx: &mpsc::UnboundedSender<ServerEvent>,
) {
    if let Ok(agent_guard) = agent.try_lock() {
        let registry = agent_guard.registry();
        drop(agent_guard);
        apply_set_compaction_mode(id, mode, registry, client_event_tx).await;
    } else {
        spawn_deferred_set_compaction_mode(id, mode, Arc::clone(agent), client_event_tx.clone());
    }
}

async fn apply_set_compaction_mode(
    id: u64,
    mode: crate::config::CompactionMode,
    registry: crate::tool::Registry,
    client_event_tx: &mpsc::UnboundedSender<ServerEvent>,
) {
    let result = {
        let compaction = registry.compaction();
        let mut manager = compaction.write().await;
        manager.set_mode(mode);
        Ok::<(), anyhow::Error>(())
    };

    match result {
        Ok(()) => {
            let updated_mode = registry.compaction().read().await.mode();
            let _ = client_event_tx.send(ServerEvent::CompactionModeChanged {
                id,
                mode: updated_mode,
                error: None,
            });
        }
        Err(e) => {
            let fallback_mode = registry.compaction().read().await.mode();
            let _ = client_event_tx.send(ServerEvent::CompactionModeChanged {
                id,
                mode: fallback_mode,
                error: Some(e.to_string()),
            });
        }
    }
}

fn spawn_deferred_set_compaction_mode(
    id: u64,
    mode: crate::config::CompactionMode,
    agent: Arc<Mutex<Agent>>,
    client_event_tx: mpsc::UnboundedSender<ServerEvent>,
) {
    let queued_at = log_provider_control_deferred("set_compaction_mode", id);
    tokio::spawn(async move {
        let registry = {
            let agent_guard = agent.lock().await;
            log_provider_control_lock_acquired("set_compaction_mode", id, queued_at);
            agent_guard.registry()
        };
        apply_set_compaction_mode(id, mode, registry, &client_event_tx).await;
        log_provider_control_completed("set_compaction_mode", id, queued_at);
    });
}

#[allow(clippy::too_many_arguments)]
pub(super) async fn handle_notify_auth_changed(
    id: u64,
    provider_hint: Option<String>,
    auth: Option<AuthChanged>,
    provider: &Arc<dyn Provider>,
    provider_template: &Arc<dyn Provider>,
    sessions: &SessionAgents,
    client_session_id: &str,
    agent: &Arc<Mutex<Agent>>,
    client_event_tx: &mpsc::UnboundedSender<ServerEvent>,
) {
    crate::auth::AuthStatus::invalidate_cache();
    let (session_id, before_snapshot) = if let Ok(agent_guard) = agent.try_lock() {
        (
            agent_guard.session_id().to_string(),
            AvailableModelsSnapshot::from_agent(&agent_guard),
        )
    } else {
        crate::logging::event_warn(
            "SERVER_PROVIDER_CONTROL_DEFERRED",
            vec![
                ("phase", "fallback_snapshot".to_string()),
                ("operation", "notify_auth_changed".to_string()),
                ("request_id", id.to_string()),
                ("session_id", client_session_id.to_string()),
                ("reason", "agent_busy".to_string()),
            ],
        );
        (
            client_session_id.to_string(),
            available_models_snapshot_from_provider(provider),
        )
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
            spawn_account_switch_refresh(
                id,
                "anthropic",
                Arc::clone(agent),
                client_event_tx.clone(),
            );
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
            spawn_account_switch_refresh(id, "openai", Arc::clone(agent), client_event_tx.clone());
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

fn spawn_account_switch_refresh(
    id: u64,
    provider_kind: &'static str,
    agent: Arc<Mutex<Agent>>,
    client_event_tx: mpsc::UnboundedSender<ServerEvent>,
) {
    tokio::spawn(async move {
        let started = Instant::now();
        crate::logging::event_info(
            "SERVER_PROVIDER_CONTROL_ACCOUNT_SWITCH",
            vec![
                ("phase", "refresh_start".to_string()),
                ("provider", provider_kind.to_string()),
                ("request_id", id.to_string()),
            ],
        );
        let provider = if let Ok(mut agent_guard) = agent.try_lock() {
            let provider = agent_guard.provider_handle();
            agent_guard.reset_provider_session();
            provider
        } else {
            let queued_at = log_provider_control_deferred("account_switch_refresh", id);
            let mut agent_guard = agent.lock().await;
            log_provider_control_lock_acquired("account_switch_refresh", id, queued_at);
            let provider = agent_guard.provider_handle();
            agent_guard.reset_provider_session();
            log_provider_control_completed("account_switch_refresh", id, queued_at);
            provider
        };
        provider.invalidate_credentials().await;

        crate::provider::clear_all_provider_unavailability_for_account();
        crate::provider::clear_all_model_unavailability_for_account();

        match provider_kind {
            "anthropic" => {
                tokio::spawn(async {
                    let _ = crate::usage::get().await;
                });
            }
            "openai" => {
                tokio::spawn(async {
                    let _ = crate::usage::get_openai_usage().await;
                });
            }
            _ => {}
        }

        crate::bus::Bus::global().publish_models_updated();
        let event = available_models_updated_event(&agent).await;
        let _ = client_event_tx.send(event);
        let _ = client_event_tx.send(ServerEvent::Done { id });
        crate::logging::event_info(
            "SERVER_PROVIDER_CONTROL_ACCOUNT_SWITCH",
            vec![
                ("phase", "refresh_done".to_string()),
                ("provider", provider_kind.to_string()),
                ("request_id", id.to_string()),
                ("elapsed_ms", started.elapsed().as_millis().to_string()),
            ],
        );
    });
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::message::{Message, ToolDefinition};
    use crate::provider::EventStream;
    use async_trait::async_trait;
    use std::sync::Mutex as StdMutex;
    use tokio::time::{Duration, timeout};

    struct IsolatedRuntimeDir {
        _prev_runtime: Option<std::ffi::OsString>,
        _temp: tempfile::TempDir,
    }

    impl IsolatedRuntimeDir {
        fn new() -> Self {
            let temp = tempfile::TempDir::new().expect("runtime dir");
            let prev_runtime = std::env::var_os("JCODE_RUNTIME_DIR");
            crate::env::set_var("JCODE_RUNTIME_DIR", temp.path());
            Self {
                _prev_runtime: prev_runtime,
                _temp: temp,
            }
        }
    }

    impl Drop for IsolatedRuntimeDir {
        fn drop(&mut self) {
            if let Some(prev_runtime) = self._prev_runtime.take() {
                crate::env::set_var("JCODE_RUNTIME_DIR", prev_runtime);
            } else {
                crate::env::remove_var("JCODE_RUNTIME_DIR");
            }
        }
    }

    #[derive(Default)]
    struct TestEffortProvider {
        model: StdMutex<Option<String>>,
        effort: StdMutex<Option<String>>,
        service_tier: StdMutex<Option<String>>,
        transport: StdMutex<Option<String>>,
    }

    #[async_trait]
    impl Provider for TestEffortProvider {
        async fn complete(
            &self,
            _messages: &[Message],
            _tools: &[ToolDefinition],
            _system: &str,
            _resume_session_id: Option<&str>,
        ) -> anyhow::Result<EventStream> {
            panic!("complete should not run in provider control test")
        }

        fn name(&self) -> &str {
            "test-effort"
        }

        fn model(&self) -> String {
            self.model
                .lock()
                .expect("model lock")
                .clone()
                .unwrap_or_else(|| "test-model-a".to_string())
        }

        fn set_model(&self, model: &str) -> anyhow::Result<()> {
            *self.model.lock().expect("model lock") = Some(model.to_string());
            Ok(())
        }

        fn available_models_for_switching(&self) -> Vec<String> {
            vec!["test-model-a".to_string(), "test-model-b".to_string()]
        }

        fn reasoning_effort(&self) -> Option<String> {
            self.effort.lock().expect("effort lock").clone()
        }

        fn set_reasoning_effort(&self, effort: &str) -> anyhow::Result<()> {
            *self.effort.lock().expect("effort lock") = Some(effort.to_string());
            Ok(())
        }

        fn service_tier(&self) -> Option<String> {
            self.service_tier.lock().expect("service lock").clone()
        }

        fn set_service_tier(&self, service_tier: &str) -> anyhow::Result<()> {
            *self.service_tier.lock().expect("service lock") = Some(service_tier.to_string());
            Ok(())
        }

        fn transport(&self) -> Option<String> {
            self.transport.lock().expect("transport lock").clone()
        }

        fn set_transport(&self, transport: &str) -> anyhow::Result<()> {
            *self.transport.lock().expect("transport lock") = Some(transport.to_string());
            Ok(())
        }

        fn fork(&self) -> Arc<dyn Provider> {
            Arc::new(Self {
                model: StdMutex::new(Some(self.model())),
                effort: StdMutex::new(self.reasoning_effort()),
                service_tier: StdMutex::new(self.service_tier()),
                transport: StdMutex::new(self.transport()),
            })
        }
    }

    async fn test_agent(
        session_id: &str,
    ) -> (
        Arc<TestEffortProvider>,
        Arc<Mutex<Agent>>,
        mpsc::UnboundedSender<ServerEvent>,
        mpsc::UnboundedReceiver<ServerEvent>,
    ) {
        let provider = Arc::new(TestEffortProvider::default());
        let provider_dyn: Arc<dyn Provider> = provider.clone();
        let registry = crate::tool::Registry::new(Arc::clone(&provider_dyn)).await;
        let mut session =
            crate::session::Session::create_with_id(session_id.to_string(), None, None);
        session.model = Some(provider.model());
        let agent = Arc::new(Mutex::new(Agent::new_with_session(
            Arc::clone(&provider_dyn),
            registry,
            session,
            None,
        )));
        let (client_event_tx, client_event_rx) = mpsc::unbounded_channel();
        (provider, agent, client_event_tx, client_event_rx)
    }

    #[tokio::test]
    async fn set_reasoning_effort_does_not_wait_for_busy_agent_lock() {
        let _guard = crate::storage::lock_test_env();
        let _runtime = IsolatedRuntimeDir::new();

        let (provider, agent, client_event_tx, mut client_event_rx) =
            test_agent("session_busy_reasoning_effort").await;
        let busy_agent_lock = agent.lock().await;

        timeout(
            Duration::from_millis(100),
            handle_set_reasoning_effort(7, "low".to_string(), &agent, &client_event_tx),
        )
        .await
        .expect("reasoning effort changes must not wait for a busy agent mutex");

        assert!(client_event_rx.try_recv().is_err());

        drop(busy_agent_lock);

        let event = timeout(Duration::from_secs(1), client_event_rx.recv())
            .await
            .expect("deferred reasoning effort change should finish after agent is idle");
        assert_eq!(provider.reasoning_effort().as_deref(), Some("low"));
        assert!(matches!(
            event,
            Some(ServerEvent::ReasoningEffortChanged {
                id: 7,
                effort: Some(effort),
                error: None,
            }) if effort == "low"
        ));
    }

    #[tokio::test]
    async fn set_model_does_not_wait_for_busy_agent_lock() {
        let _guard = crate::storage::lock_test_env();
        let _runtime = IsolatedRuntimeDir::new();

        let (provider, agent, client_event_tx, mut client_event_rx) =
            test_agent("session_busy_set_model").await;
        let busy_agent_lock = agent.lock().await;

        timeout(
            Duration::from_millis(100),
            handle_set_model(8, "test-model-b".to_string(), &agent, &client_event_tx),
        )
        .await
        .expect("model changes must not wait for a busy agent mutex");

        assert!(client_event_rx.try_recv().is_err());

        drop(busy_agent_lock);

        let event = timeout(Duration::from_secs(1), client_event_rx.recv())
            .await
            .expect("deferred model change should finish after agent is idle");
        assert_eq!(provider.model(), "test-model-b");
        assert!(matches!(
            event,
            Some(ServerEvent::ModelChanged {
                id: 8,
                model,
                provider_name: Some(provider_name),
                error: None,
            }) if model == "test-model-b" && provider_name == "test-effort"
        ));
    }

    #[tokio::test]
    async fn set_service_tier_does_not_wait_for_busy_agent_lock() {
        let _guard = crate::storage::lock_test_env();
        let _runtime = IsolatedRuntimeDir::new();

        let (provider, agent, client_event_tx, mut client_event_rx) =
            test_agent("session_busy_set_service_tier").await;
        let busy_agent_lock = agent.lock().await;

        timeout(
            Duration::from_millis(100),
            handle_set_service_tier(9, "priority".to_string(), &agent, &client_event_tx),
        )
        .await
        .expect("service tier changes must not wait for a busy agent mutex");

        assert!(client_event_rx.try_recv().is_err());

        drop(busy_agent_lock);

        let event = timeout(Duration::from_secs(1), client_event_rx.recv())
            .await
            .expect("deferred service tier change should finish after agent is idle");
        assert_eq!(provider.service_tier().as_deref(), Some("priority"));
        assert!(matches!(
            event,
            Some(ServerEvent::ServiceTierChanged {
                id: 9,
                service_tier: Some(service_tier),
                error: None,
            }) if service_tier == "priority"
        ));
    }
}
