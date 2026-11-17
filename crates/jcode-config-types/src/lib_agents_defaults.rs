use super::*;

impl Default for AgentsConfig {
    fn default() -> Self {
        Self {
            swarm_model: None,
            swarm_model_selection: false,
            swarm_model_allowlist: Vec::new(),
            swarm_effort: None,
            swarm_root_effort: None,
            swarm_spawn_mode: SwarmSpawnMode::default(),
            swarm_gallery_max_pct: None,
            swarm_strip_layout: SwarmStripLayout::default(),
            memory_jev_provider: default_memory_jev_provider(),
            memory_jev_model: None,
            browser_jev_model: None,
            voice_jev_model: None,
            memory_jev_threshold: default_memory_jev_threshold(),
            memory_model: None,
            memory_sidecar_enabled: default_memory_sidecar_enabled(),
            memory_rerank_cadence: default_memory_rerank_cadence(),
            memory_rerank_votes: default_memory_rerank_votes(),
            memory_rerank_min_agree: default_memory_rerank_min_agree(),
            memory_embedding_backend: default_memory_embedding_backend(),
            memory_embedding_model: None,
            memory_embedding_base_url: None,
            memory_embedding_dim: None,
            swarm_max_concurrent_agents: default_swarm_max_concurrent_agents(),
            swarm_stall_after_secs: default_swarm_stall_after_secs(),
            swarm_stall_nudge: false,
        }
    }
}

impl AgentsConfig {
    /// Validate an agent-supplied override before resolving any provider route.
    /// Matching is exact: a bare model never authorizes another provider's route.
    pub fn validate_swarm_model_request(&self, model: Option<&str>) -> Result<(), &'static str> {
        let Some(model) = model else {
            return Ok(());
        };
        if !self.swarm_model_selection {
            return Err(
                "Agent-selected swarm models are disabled. Omit model to use the user-configured default.",
            );
        }
        if model.trim().is_empty()
            || !self
                .swarm_model_allowlist
                .iter()
                .any(|allowed| allowed == model)
        {
            return Err(
                "Requested swarm model is not in agents.swarm_model_allowlist. Omit model to use the user-configured default.",
            );
        }
        Ok(())
    }

    /// Resolve swarm mode's root effort without allowing the orchestration
    /// sentinel to recurse into itself. Unknown values use the low-effort
    /// default without invalidating other settings.
    pub fn root_effort_for_swarm(&self) -> &'static str {
        let value = self.swarm_root_effort.as_deref().unwrap_or("low").trim();
        ["none", "minimal", "low", "medium", "high", "xhigh", "max"]
            .into_iter()
            .find(|level| level.eq_ignore_ascii_case(value))
            .unwrap_or("low")
    }
}
