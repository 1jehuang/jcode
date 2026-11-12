use super::*;

/// Resolve the reasoning effort for a spawned swarm worker (#1165, #2153).
///
/// Priority: explicit `requested_effort` (SDK) → `agents.swarm_effort` config
/// pin → if the root is in swarm mode, `root_swarm_effort` (the resolved
/// `agents.swarm_root_effort` value) → [`DEFAULT_SWARM_WORKER_EFFORT`].
///
/// The root-inherit step keeps a coordinator that has explicitly opted out of
/// thinking (`swarm_root_effort = "none"`) from silently re-enabling it for its
/// workers by forcing `"low"`.
///
/// `root_swarm_effort` is the already-resolved value from
/// `agents.root_effort_for_swarm()`; passing it as a parameter keeps this
/// function pure and testable without global configuration.
pub(in crate::server) fn resolve_swarm_spawn_effort(
    requested_effort: Option<&str>,
    configured_swarm_effort: Option<&str>,
    root_effort: Option<&str>,
    root_swarm_effort: Option<&str>,
) -> Option<String> {
    let clean = |effort: Option<&str>| {
        effort
            .map(str::trim)
            .filter(|effort| !effort.is_empty())
            .map(str::to_string)
    };
    clean(requested_effort)
        .or_else(|| clean(configured_swarm_effort))
        .or_else(|| {
            root_effort
                .filter(|effort| jcode_base::prompt::is_swarm_effort(effort))
                .and_then(|_| clean(root_swarm_effort))
        })
        .or_else(|| Some(DEFAULT_SWARM_WORKER_EFFORT.to_string()))
}

/// Reasoning effort for spawned swarm workers when neither the request nor
/// `agents.swarm_effort` sets one.
pub(in crate::server) const DEFAULT_SWARM_WORKER_EFFORT: &str = "low";

/// Spawn mode for one swarm spawn. An explicit per-call mode wins, except
/// `auto`: a model passing `auto` expresses no preference, so the user's
/// configured `agents.swarm_spawn_mode` (inline by default) decides. Without
/// this, `auto` tried a visible window first and, from terminals jcode cannot
/// target (VS Code, Cursor), opened a separate terminal app instead of the
/// inline gallery the user configured. Configuring `auto` still works.
pub(in crate::server) fn resolve_swarm_spawn_mode(
    requested: Option<SwarmSpawnMode>,
    configured: SwarmSpawnMode,
) -> SwarmSpawnMode {
    match requested {
        Some(SwarmSpawnMode::Auto) | None => configured,
        Some(mode) => mode,
    }
}

pub(super) fn is_inherit_sentinel(model: &str) -> bool {
    let trimmed = model.trim();
    trimmed.eq_ignore_ascii_case("inherit") || trimmed.eq_ignore_ascii_case("coordinator")
}

/// Selection that inherits the coordinator's model, provider key, and route.
pub(super) fn inherit_coordinator_selection(
    coordinator: &CoordinatorSpawnIdentity,
) -> SwarmSpawnSelection {
    SwarmSpawnSelection {
        model: coordinator.model.clone(),
        provider_key: coordinator
            .provider_key
            .clone()
            .or_else(|| provider_key_for_spawn_model(coordinator.model.as_deref(), None)),
        route_api_method: coordinator.route_api_method.clone(),
    }
}
