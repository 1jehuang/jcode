use super::*;

/// Resolve and validate a requested spawn model against the live route catalog.
///
/// The whitelist policy runs first (so an unlisted model is refused before any
/// route work). A bare id is then resolved to its qualified route only when
/// exactly one configured route serves it. Ambiguous ids are refused with the
/// candidate routes, and unknown ids are refused with the whitelist and config
/// key. Errors never name a model the caller did not ask for.
pub(in crate::server) fn resolve_spawn_requested_model(
    agents: &crate::config::AgentsConfig,
    requested: Option<&str>,
    routes: &[jcode_provider_core::ModelRoute],
    premium_opt_in: bool,
) -> anyhow::Result<Option<String>> {
    // A whitelist of ambiguous bare ids is under-specified: refuse rather than
    // let a later spawn resolve it by guessing. This applies to every spawn,
    // including ones that omit `model`.
    if !routes.is_empty()
        && let Some((entry, candidates)) = jcode_provider_core::ambiguous_bare_whitelist_entries(
            routes,
            &agents.swarm_model_allowlist,
        )
        .into_iter()
        .next()
    {
        anyhow::bail!(
            "agents.swarm_model_allowlist entry `{entry}` matches {} configured routes: {}. \
             Qualify it with a route prefix (for example `{}`) and whitelist that exact route, \
             or remove the entry.",
            candidates.len(),
            candidates.join(", "),
            candidates.first().cloned().unwrap_or_default()
        );
    }

    let Some(requested) = requested else {
        return Ok(None);
    };
    let requested = requested.trim().to_string();

    // Route-qualified requests already chose a route; only the whitelist applies.
    if requested.contains(':') || routes.is_empty() {
        agents
            .validate_swarm_model_request(Some(&requested), premium_opt_in)
            .map_err(anyhow::Error::msg)?;
        return Ok(Some(requested));
    }

    match jcode_provider_core::spawn_model_candidates(routes, &requested) {
        jcode_provider_core::SpawnModelCandidates::Qualifiable(resolved) => {
            agents
                .validate_swarm_model_resolved(&requested, &resolved, premium_opt_in)
                .map_err(anyhow::Error::msg)?;
            Ok(Some(resolved))
        }
        jcode_provider_core::SpawnModelCandidates::Ambiguous(candidates) => anyhow::bail!(
            "Swarm model `{requested}` matches {} configured routes: {}. \
             Add a route prefix to choose one (for example `{}`) and whitelist that exact route \
             in agents.swarm_model_allowlist (`/subagent-model allow {prefix}`). \
             Refusing to guess, because a bare id is ambiguous here.",
            candidates.len(),
            candidates.join(", "),
            candidates.first().cloned().unwrap_or_default(),
            prefix = candidates
                .first()
                .cloned()
                .unwrap_or_else(|| requested.clone())
        ),
        jcode_provider_core::SpawnModelCandidates::Unknown => anyhow::bail!(
            "Swarm model `{requested}` is not a configured route. \
             Whitelist (agents.swarm_model_allowlist): {}. \
             Add a configured route with `/subagent-model allow <route>` (or set it in \
             agents.swarm_model_allowlist in ~/.jcode/config.toml), or omit `model` to use the \
             configured default (agents.swarm_model).",
            if agents.swarm_model_allowlist.is_empty() {
                "empty".to_string()
            } else {
                agents.swarm_model_allowlist.join(", ")
            }
        ),
    }
}

pub(in crate::server) fn resolve_swarm_spawn_effort(
    requested_effort: Option<&str>,
    configured_swarm_effort: Option<&str>,
) -> Option<String> {
    let clean = |effort: Option<&str>| {
        effort
            .map(str::trim)
            .filter(|effort| !effort.is_empty())
            .map(str::to_string)
    };
    clean(requested_effort)
        .or_else(|| clean(configured_swarm_effort))
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

pub(in crate::server) fn is_inherit_sentinel(model: &str) -> bool {
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

#[cfg(test)]
mod spawn_model_resolution_tests {
    use super::resolve_spawn_requested_model;
    use jcode_provider_core::{ModelRoute, SpawnModelCandidates};

    fn route(provider: &str, api_method: &str, model: &str) -> ModelRoute {
        ModelRoute {
            model: model.to_string(),
            provider: provider.to_string(),
            api_method: api_method.to_string(),
            available: true,
            detail: String::new(),
            cheapness: None,
            usage: None,
        }
    }

    fn agents(allowlist: &[&str]) -> crate::config::AgentsConfig {
        crate::config::AgentsConfig {
            swarm_model_allowlist: allowlist.iter().map(|m| m.to_string()).collect(),
            ..Default::default()
        }
    }

    #[test]
    fn provider_core_candidates_unique_ambiguous_and_unknown() {
        let routes = vec![route("Pool", "openai-compatible:ocgo-pool", "hy4-flash-free")];
        assert_eq!(
            jcode_provider_core::spawn_model_candidates(&routes, "hy4-flash-free"),
            SpawnModelCandidates::Qualifiable("ocgo-pool:hy4-flash-free".to_string())
        );

        let routes = vec![
            route("Pool", "openai-compatible:ocgo-pool", "hy4-flash-free"),
            route("OpenCode Go", "openai-compatible:opencode-go", "hy4-flash-free"),
        ];
        assert_eq!(
            jcode_provider_core::spawn_model_candidates(&routes, "hy4-flash-free"),
            SpawnModelCandidates::Ambiguous(vec![
                "ocgo-pool:hy4-flash-free".to_string(),
                "opencode-go:hy4-flash-free".to_string(),
            ])
        );
        assert_eq!(
            jcode_provider_core::spawn_model_candidates(&routes, "nope"),
            SpawnModelCandidates::Unknown
        );
    }

    #[test]
    fn bare_unique_id_resolves_and_spawns() {
        let routes = vec![route("MiniMax", "openai-compatible:minimax", "MiniMax-M3.1-Flash")];
        let agents = agents(&["minimax:MiniMax-M3.1-Flash"]);
        let resolved = resolve_spawn_requested_model(
            &agents,
            Some("MiniMax-M3.1-Flash"),
            &routes,
            false,
        )
        .expect("unique bare id resolves");
        assert_eq!(resolved.as_deref(), Some("minimax:MiniMax-M3.1-Flash"));
    }

    #[test]
    fn ambiguous_bare_id_lists_candidates_and_refuses() {
        let routes = vec![
            route("Pool", "openai-compatible:ocgo-pool", "hy4-flash-free"),
            route("OpenCode Go", "openai-compatible:opencode-go", "hy4-flash-free"),
        ];
        let agents = agents(&["opencode-go:hy4-flash-free"]);
        let error = resolve_spawn_requested_model(&agents, Some("hy4-flash-free"), &routes, false)
            .expect_err("ambiguous id must be refused");
        let error = error.to_string();
        assert!(error.contains("hy4-flash-free"), "{error}");
        assert!(error.contains("ocgo-pool:hy4-flash-free"), "{error}");
        assert!(error.contains("opencode-go:hy4-flash-free"), "{error}");
        assert!(error.contains("route prefix"), "{error}");
    }

    #[test]
    fn unknown_bare_id_names_the_whitelist_and_config_key() {
        let routes = vec![route("MiniMax", "openai-compatible:minimax", "MiniMax-M3.1-Flash")];
        let agents = agents(&["minimax:MiniMax-M3.1-Flash"]);
        let error = resolve_spawn_requested_model(&agents, Some("gpt-6-astra"), &routes, false)
            .expect_err("unknown id must be refused");
        let error = error.to_string();
        assert!(error.contains("agents.swarm_model_allowlist"), "{error}");
        assert!(error.contains("/subagent-model allow"), "{error}");
        assert!(error.contains("minimax:MiniMax-M3.1-Flash"), "{error}");
        // Never mention a model the caller did not ask for.
        assert!(!error.contains("gpt-6-luna"), "{error}");
    }

    #[test]
    fn ambiguous_bare_whitelist_entry_is_rejected_listing_routes() {
        let routes = vec![
            route("Pool", "openai-compatible:ocgo-pool", "hy4-flash-free"),
            route("OpenCode Go", "openai-compatible:opencode-go", "hy4-flash-free"),
        ];
        let agents = agents(&["hy4-flash-free"]);
        let error = resolve_spawn_requested_model(&agents, None, &routes, false)
            .expect_err("ambiguous whitelist entry must be rejected");
        let error = error.to_string();
        assert!(error.contains("agents.swarm_model_allowlist"), "{error}");
        assert!(error.contains("ocgo-pool:hy4-flash-free"), "{error}");
        assert!(error.contains("opencode-go:hy4-flash-free"), "{error}");
    }

    #[test]
    fn route_qualified_id_spawns_without_resolution() {
        let routes = vec![
            route("Pool", "openai-compatible:ocgo-pool", "hy4-flash-free"),
            route("OpenCode Go", "openai-compatible:opencode-go", "hy4-flash-free"),
        ];
        let agents = agents(&["opencode-go:hy4-flash-free"]);
        let resolved =
            resolve_spawn_requested_model(&agents, Some("opencode-go:hy4-flash-free"), &routes, false)
                .expect("qualified id is allowed");
        assert_eq!(resolved.as_deref(), Some("opencode-go:hy4-flash-free"));
    }
}
