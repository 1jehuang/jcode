use super::*;

pub(super) fn provider_key_for_spawn_model(
    model: Option<&str>,
    provider_key_override: Option<&str>,
) -> Option<String> {
    if let Some(provider_key) = provider_key_override
        .map(str::trim)
        .filter(|provider_key| !provider_key.is_empty())
    {
        return Some(provider_key.to_string());
    }

    let model = model?.trim();
    if model.is_empty() {
        return None;
    }

    if let Some((prefix, _rest)) = model.split_once(':') {
        let prefix = prefix.trim();
        if crate::provider::provider_from_model_key(prefix).is_some()
            || crate::provider_catalog::resolve_openai_compatible_profile_selection(prefix)
                .is_some()
            || crate::config::config().providers.contains_key(prefix)
        {
            return Some(prefix.to_string());
        }
    }

    crate::provider::provider_for_model(model).map(str::to_string)
}

/// The model/auth identity a spawned swarm agent should inherit from its
/// coordinator. Resolved with a persisted-session fallback so it stays correct
/// even when the coordinator agent is mid-turn (its mutex held), which is the
/// common case because spawns are issued from inside the coordinator's turn.
#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub(super) struct CoordinatorSpawnIdentity {
    pub model: Option<String>,
    pub provider_key: Option<String>,
    pub route_api_method: Option<String>,
    pub is_canary: bool,
}

/// The resolved model + auth route a spawned swarm agent should be created
/// with, after reconciling `agents.swarm_model` config against the
/// coordinator's identity.
#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub(super) struct SwarmSpawnSelection {
    pub model: Option<String>,
    pub provider_key: Option<String>,
    pub route_api_method: Option<String>,
}

/// Resolve the coordinator's model/auth identity without blocking on its agent
/// mutex. During an active coordinator turn the agent lock is held for the
/// whole turn, so `try_lock` fails exactly when a spawn is issued. We fall back
/// to the persisted session snapshot so spawned agents still inherit the
/// coordinator's model, provider key, and auth route instead of silently
/// dropping to the config default (e.g. Claude OAuth instead of the API route).
pub(super) async fn resolve_coordinator_spawn_identity(
    req_session_id: &str,
    sessions: &SessionAgents,
) -> CoordinatorSpawnIdentity {
    if let Some(agent) = {
        let agent_sessions = sessions.read().await;
        agent_sessions.get(req_session_id).cloned()
    } && let Ok(agent_guard) = agent.try_lock()
    {
        return CoordinatorSpawnIdentity {
            model: Some(agent_guard.provider_model()),
            provider_key: agent_guard.session_provider_key(),
            route_api_method: agent_guard.session_route_api_method(),
            is_canary: agent_guard.is_canary(),
        };
    }

    // Agent busy (mid-turn) or not resident: read the authoritative persisted
    // session snapshot instead of falling back to config defaults.
    match Session::load_startup_stub(req_session_id) {
        Ok(session) => {
            let identity = CoordinatorSpawnIdentity {
                model: session.model.clone(),
                provider_key: session.provider_key.clone(),
                route_api_method: session.route_api_method.clone(),
                is_canary: session.is_canary,
            };
            crate::logging::info(&format!(
                "Swarm spawn: coordinator {} agent busy/unavailable, inheriting identity from persisted session (model={:?} provider_key={:?} route={:?} canary={})",
                req_session_id,
                identity.model,
                identity.provider_key,
                identity.route_api_method,
                identity.is_canary,
            ));
            identity
        }
        Err(error) => {
            crate::logging::warn(&format!(
                "Swarm spawn: failed to load persisted coordinator session {} for model inheritance: {} (spawned agent will use server defaults)",
                req_session_id, error
            ));
            CoordinatorSpawnIdentity::default()
        }
    }
}

/// Split a configured swarm model that carries an explicit auth-route prefix
/// (`openai-api:`, `openai-oauth:`, `claude-api:`, `claude-oauth:`) into a
/// structured selection so spawned sessions pin the exact provider + auth
/// method instead of guessing from the bare model name.
///
/// Example: `agents.swarm_model = "openai-api:gpt-5.5"` resolves to
/// `model = gpt-5.5`, `provider_key = openai-api-key`,
/// `route_api_method = openai-api-key`, which makes every spawned agent use
/// GPT-5.5 on the OpenAI API key route regardless of the coordinator's model.
///
/// Returns `None` for models without such a prefix, or for prefixes that carry
/// no API-vs-OAuth decision (bare provider aliases, OpenRouter, Copilot, ...).
/// Those keep their prefixed model and route correctly via the existing
/// session-restore path.
pub(super) fn explicit_route_for_configured_model(model: &str) -> Option<SwarmSpawnSelection> {
    let (_, prefix, bare) = crate::provider::explicit_model_provider_prefix(model)?;
    let bare = bare.trim();
    if bare.is_empty() {
        return None;
    }
    // Only the dual-auth (Anthropic/OpenAI OAuth-vs-API) prefixes carry an
    // explicit credential decision worth pinning. The canonical parser maps the
    // prefix to its stable route id, which `ModelRouteApiMethod::parse` round-
    // trips back to the exact auth method when the spawned session is restored.
    let route_id = jcode_provider_core::AuthRoute::parse_explicit_credential_prefix(prefix)?
        .route_api_method();
    Some(SwarmSpawnSelection {
        model: Some(bare.to_string()),
        provider_key: Some(route_id.to_string()),
        route_api_method: Some(route_id.to_string()),
    })
}

/// True when a model string is one of the "inherit the coordinator" sentinels.
/// Selection for a concrete model string (optionally route-prefixed like
/// `openai-api:gpt-5.5`), reconciled against the coordinator's identity.
pub(super) fn selection_for_concrete_model(
    model: String,
    coordinator: &CoordinatorSpawnIdentity,
) -> SwarmSpawnSelection {
    // A model may pin an explicit provider + auth route via a prefix
    // (e.g. "openai-api:gpt-5.5"). Honor it directly so spawned agents do
    // NOT inherit the coordinator's model/auth and instead use the
    // requested model on the requested API route.
    if let Some(selection) = explicit_route_for_configured_model(&model) {
        return selection;
    }

    // A concrete model only inherits the coordinator's provider_key/route
    // when it targets the same model; otherwise the route would point at
    // the wrong provider/auth mode.
    if coordinator.model.as_deref() == Some(model.as_str()) {
        SwarmSpawnSelection {
            model: Some(model.clone()),
            provider_key: coordinator
                .provider_key
                .clone()
                .or_else(|| provider_key_for_spawn_model(Some(&model), None)),
            route_api_method: coordinator.route_api_method.clone(),
        }
    } else {
        SwarmSpawnSelection {
            provider_key: provider_key_for_spawn_model(Some(&model), None),
            model: Some(model),
            route_api_method: None,
        }
    }
}

pub(super) async fn session_swarm_model(
    req_session_id: &str,
    sessions: &SessionAgents,
    global: Option<String>,
) -> Option<String> {
    let agent = sessions.read().await.get(req_session_id).cloned();
    if let Some(agent) = agent
        && let Ok(guard) = agent.try_lock()
    {
        return guard
            .session_for_split()
            .effective_agent_model("swarm", global);
    }
    Session::load_startup_stub(req_session_id)
        .map(|session| session.effective_agent_model("swarm", global.clone()))
        .unwrap_or(global)
}

pub(super) fn resolve_swarm_spawn_selection(
    requested_model: Option<String>,
    configured_swarm_model: Option<String>,
    coordinator: &CoordinatorSpawnIdentity,
) -> SwarmSpawnSelection {
    // An explicit per-worker choice overrides the configured default. The
    // inheritance sentinels bypass even a concrete configured model.
    if let Some(model) = requested_model
        .map(|model| model.trim().to_string())
        .filter(|model| !model.is_empty())
    {
        return if is_inherit_sentinel(&model) {
            inherit_coordinator_selection(coordinator)
        } else {
            selection_for_concrete_model(model, coordinator)
        };
    }
    // Treat empty strings and the explicit "inherit"/"coordinator" sentinels as
    // "no override": spawned swarm agents should inherit the coordinator's model
    // unless `agents.swarm_model` is deliberately set to a concrete model. This
    // avoids the surprising case where a stale `swarm_model` config pins every
    // spawned agent to an unrelated model/provider.
    let configured_swarm_model = configured_swarm_model
        .map(|model| model.trim().to_string())
        .filter(|model| !model.trim().is_empty() && !is_inherit_sentinel(model));

    match configured_swarm_model {
        Some(model) => selection_for_concrete_model(model, coordinator),
        None => inherit_coordinator_selection(coordinator),
    }
}

/// Registry mode: resolve a concrete requested model against the requesting
/// session's live catalog (same as `swarm list_models`), else the template's.
pub(super) async fn registry_requested_model(
    requested: Option<String>,
    registry_mode: bool,
    req_session_id: &str,
    sessions: &SessionAgents,
    provider_template: &Arc<dyn Provider>,
) -> anyhow::Result<Option<String>> {
    let model = match requested {
        Some(m) if registry_mode && !is_inherit_sentinel(m.trim()) => m,
        // No explicit worker model: the declarative routing policy picks one.
        None => return Ok(best_fit_swarm_model(req_session_id, sessions, provider_template).await),
        other => return Ok(other),
    };
    let routes = live_routes(req_session_id, sessions, provider_template).await;
    resolve_registry_model(&model, &routes)
        .map(Some)
        .map_err(anyhow::Error::msg)
}

/// Resolve an agent-requested worker model against the live route catalog
/// (`Provider::model_routes`). Accepts `model` or `model@<api_method>` (the
/// `[api_method]` shown by `swarm list_models`). Returns the spawn model string
/// to pass on (route-pinned when the route carries an explicit credential), or
/// an error listing every available route so the caller can retry.
pub(super) fn resolve_registry_model(
    requested: &str,
    routes: &[jcode_provider_core::ModelRoute],
) -> Result<String, String> {
    let requested = requested.trim();
    // Bare model first; else `model@api_method` (api_method may itself contain '@').
    let (model, method) = match requested.split_once('@') {
        Some((m, api)) if !routes.iter().any(|r| r.model == requested) => (m, Some(api)),
        _ => (requested, None),
    };
    let mut candidates = routes.iter().filter(|r| {
        r.available && r.model == model && method.is_none_or(|api| r.api_method == api)
    });
    match candidates.next() {
        Some(route) => {
            let pinned = format!("{}:{}", route.api_method, route.model);
            // Only pin when the api_method is a prefix the spawn path understands;
            // otherwise the bare model lets the normal provider resolution run.
            Ok(
                if explicit_route_for_configured_model(&pinned).is_some()
                    || jcode_provider_core::oauth_account_label(&route.api_method).is_some()
                {
                    pinned
                } else {
                    route.model.clone()
                },
            )
        }
        None => {
            let mut avail: Vec<String> = routes
                .iter()
                .filter(|r| r.available)
                .map(|r| format!("{}@{}", r.model, r.api_method))
                .collect();
            avail.sort();
            avail.dedup();
            Err(format!(
                "Model '{requested}' is not an available configured route. Available ({}):\n{}",
                avail.len(),
                avail.join("\n")
            ))
        }
    }
}

#[cfg(test)]
#[path = "comm_session_registry_tests.rs"]
mod registry_tests;

/// Best-fit worker pick from `routing-policy.json` (`parallel_shard` tier),
/// sticky per coordinator session. `None` (no policy, nothing eligible) keeps
/// the existing configured/inherited default. Logs one telemetry row.
pub(super) async fn best_fit_swarm_model(
    req_session_id: &str,
    sessions: &SessionAgents,
    provider_template: &Arc<dyn Provider>,
) -> Option<String> {
    use super::super::turn_failover::best_fit_router as bf;
    let policy = bf::Policy::load()?;
    let routes = live_routes(req_session_id, sessions, provider_template).await;
    let started = std::time::Instant::now();
    let picked = tokio::task::spawn_blocking({
        let sid = req_session_id.to_string();
        move || bf::pick(&policy, &sid, bf::SWARM_TIER, &routes, &bf::local_probe)
    })
    .await
    .unwrap_or_else(|e| {
        crate::logging::warn(&format!("best-fit pick task failed: {e}"));
        None
    });
    bf::telemetry(serde_json::json!({
        "event": "swarm_dispatch",
        "session_id": req_session_id,
        "tier": bf::SWARM_TIER,
        "route": picked.as_ref().map(|(r, _, _)| bf::route_id(r)),
        "billing": picked.as_ref().map(|(_, b, _)| b.as_str()),
        "sticky": picked.as_ref().map(|(_, _, s)| *s),
        "ms": started.elapsed().as_millis() as u64,
    }));
    let (route, _, _) = picked?;
    match resolve_registry_model(&bf::route_id(&route), std::slice::from_ref(&route)) {
        Ok(model) => Some(model),
        Err(e) => {
            crate::logging::warn(&format!("best-fit pick not spawnable: {e}"));
            None
        }
    }
}

/// The requester's live route catalog when its lock is free, else the template's.
async fn live_routes(
    req_session_id: &str,
    sessions: &SessionAgents,
    provider_template: &Arc<dyn Provider>,
) -> Vec<jcode_provider_core::ModelRoute> {
    let agent = sessions.read().await.get(req_session_id).cloned();
    match agent.as_ref().and_then(|a| a.try_lock().ok()) {
        Some(guard) => guard.model_routes(),
        None => provider_template.model_routes(),
    }
}
