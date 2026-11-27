use super::*;

pub(super) fn log_provider_control_deferred(operation: &'static str, id: u64) -> Instant {
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

pub(super) fn log_provider_control_lock_acquired(
    operation: &'static str,
    id: u64,
    queued_at: Instant,
) {
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

pub(super) fn log_provider_control_completed(operation: &'static str, id: u64, queued_at: Instant) {
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

pub(super) fn spawn_deferred_agent_mutation<F>(
    operation: &'static str,
    id: u64,
    agent: Arc<Mutex<Agent>>,
    client_event_tx: mpsc::UnboundedSender<ServerEvent>,
    apply: F,
) where
    F: FnOnce(&mut Agent, &mpsc::UnboundedSender<ServerEvent>) + Send + 'static,
{
    let queued_at = log_provider_control_deferred(operation, id);
    crate::runtime_activity::spawn(async move {
        let mut agent_guard = agent.lock().await;
        log_provider_control_lock_acquired(operation, id, queued_at);
        apply(&mut agent_guard, &client_event_tx);
        log_provider_control_completed(operation, id, queued_at);
    });
}

pub(super) fn spawn_deferred_provider_operation<F>(
    operation: &'static str,
    id: u64,
    agent: Arc<Mutex<Agent>>,
    client_event_tx: mpsc::UnboundedSender<ServerEvent>,
    apply: F,
) where
    F: FnOnce(Arc<dyn Provider>, &mpsc::UnboundedSender<ServerEvent>) + Send + 'static,
{
    let queued_at = log_provider_control_deferred(operation, id);
    crate::runtime_activity::spawn(async move {
        let provider = {
            let agent_guard = agent.lock().await;
            log_provider_control_lock_acquired(operation, id, queued_at);
            agent_guard.provider_handle()
        };
        apply(provider, &client_event_tx);
        log_provider_control_completed(operation, id, queued_at);
    });
}

pub(super) async fn auth_refresh_targets(
    provider_template: &Arc<dyn Provider>,
    current_provider: &Arc<dyn Provider>,
    current_agent: &Arc<Mutex<Agent>>,
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
        // The requesting session's provider is already included explicitly,
        // even when that agent is busy and its lock cannot be inspected here.
        if Arc::ptr_eq(&agent, current_agent) {
            continue;
        }
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

pub(super) fn spawn_deferred_auth_refreshes(agents: Vec<Arc<Mutex<Agent>>>) {
    for agent in agents {
        crate::runtime_activity::spawn(async move {
            let provider = {
                let agent_guard = agent.lock().await;
                agent_guard.provider_handle()
            };
            provider.on_auth_changed_preserve_current_provider();
            crate::bus::Bus::global().publish_models_updated();
        });
    }
}

pub(super) async fn apply_auth_runtime_model_to_agent(
    activation: &AuthActivationResult,
    model: Option<&str>,
    agent: &Arc<Mutex<Agent>>,
    unless_user_selected_after: Option<u64>,
) {
    let Some(model) = model.map(str::trim).filter(|model| !model.is_empty()) else {
        return;
    };

    let provider = activation.provider_id.as_deref().unwrap_or("auth");
    let result = {
        let mut agent_guard = agent.lock().await;
        if unless_user_selected_after
            .is_some_and(|generation| agent_guard.user_selected_provider_model_after(generation))
        {
            crate::logging::auth_event(
                "auth_changed_auto_model_skipped_after_manual_switch",
                provider,
                &[("reason", "user_selected_provider_model_during_refresh")],
            );
            return;
        }
        let provider_name = agent_guard.provider_handle().name().to_string();
        let model_request = activation.model_switch_request(&provider_name, model);
        let result = agent_guard.set_model_from_auth(&model_request);
        if result.is_ok() {
            agent_guard.reset_provider_session();
        }
        result.map(|_| agent_guard.provider_model())
    };

    match result {
        Ok(resolved_model) => crate::logging::auth_event(
            "auth_changed_runtime_model_applied",
            provider,
            &[
                ("requested_model", model),
                ("resolved_model", resolved_model.as_str()),
                ("provider_session", "reset"),
            ],
        ),
        Err(error) => {
            let message = error.to_string();
            crate::logging::auth_event(
                "auth_changed_runtime_model_failed",
                provider,
                &[("requested_model", model), ("reason", message.as_str())],
            );
        }
    }
}

pub(super) async fn apply_auth_route_to_agent(
    route: &ModelRoute,
    agent: &Arc<Mutex<Agent>>,
    unless_user_selected_after: Option<u64>,
) {
    let selection = RouteSelection::from_model_route(route);
    let requested_model = selection.routed_model_spec();
    let result = {
        let mut agent_guard = agent.lock().await;
        if unless_user_selected_after
            .is_some_and(|generation| agent_guard.user_selected_provider_model_after(generation))
        {
            crate::logging::auth_event(
                "auth_changed_auto_model_skipped_after_manual_switch",
                &route.provider,
                &[("reason", "user_selected_provider_model_during_refresh")],
            );
            return;
        }
        let result = agent_guard.set_route_selection_from_auth(&selection);
        if result.is_ok() {
            agent_guard.reset_provider_session();
        }
        result.map(|_| agent_guard.provider_model())
    };

    match result {
        Ok(resolved_model) => crate::logging::auth_event(
            "auth_changed_global_route_applied",
            &route.provider,
            &[
                ("requested_model", requested_model.as_str()),
                ("resolved_model", resolved_model.as_str()),
                ("api_method", route.api_method.as_str()),
                ("provider_session", "reset"),
            ],
        ),
        Err(error) => {
            let message = error.to_string();
            crate::logging::auth_event(
                "auth_changed_global_route_failed",
                &route.provider,
                &[
                    ("requested_model", requested_model.as_str()),
                    ("api_method", route.api_method.as_str()),
                    ("reason", message.as_str()),
                ],
            );
        }
    }
}
