use super::*;

/// Config keys referenced verbatim in policy errors so the message names the
/// exact thing the operator must change.
pub const SWARM_MODEL_ALLOWLIST_KEY: &str = "agents.swarm_model_allowlist";
pub const SWARM_MODEL_DEFAULT_KEY: &str = "agents.swarm_model";
pub const SWARM_PREMIUM_MODELS_KEY: &str = "agents.swarm_premium_models";
/// Per-spawn acknowledgement field for whitelisted premium routes.
pub const SWARM_PREMIUM_OPT_IN_FIELD: &str = "premium_opt_in";
/// UI command that edits the whitelist without hand-editing config.toml.
pub const SWARM_MODEL_POLICY_COMMAND: &str = "/subagent-model";

impl Default for AgentsConfig {
    fn default() -> Self {
        Self {
            swarm_model: None,
            swarm_model_selection: false,
            swarm_model_allowlist: Vec::new(),
            swarm_premium_models: Vec::new(),
            swarm_effort: None,
            swarm_root_effort: None,
            swarm_spawn_mode: SwarmSpawnMode::default(),
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

/// `inherit` / `coordinator` sentinels mean "use the coordinator's model",
/// which is spelled by omitting `model`, not by whitelisting an alias.
fn is_inherit_sentinel(model: &str) -> bool {
    let trimmed = model.trim();
    trimmed.eq_ignore_ascii_case("inherit") || trimmed.eq_ignore_ascii_case("coordinator")
}

impl AgentsConfig {
    /// Exact whitelist membership. A bare model never authorizes another
    /// provider's route, and case/spacing differences are not resolved.
    pub fn swarm_model_allowed(&self, model: &str) -> bool {
        self.swarm_model_allowlist
            .iter()
            .any(|allowed| allowed == model)
    }

    pub fn swarm_model_premium(&self, model: &str) -> bool {
        self.swarm_premium_models
            .iter()
            .any(|premium| premium == model)
    }

    fn allowlist_display(&self) -> String {
        if self.swarm_model_allowlist.is_empty() {
            "(empty: no model override is permitted)".to_string()
        } else {
            self.swarm_model_allowlist.join(", ")
        }
    }

    /// Validate an agent-supplied model override before any provider route is
    /// resolved. The whitelist is authoritative: anything not listed verbatim
    /// is refused, and `premium_opt_in` can only ever narrow, never widen.
    ///
    /// `None` (model omitted) is always allowed and resolved by the caller to
    /// the deterministic default.
    pub fn validate_swarm_model_request(
        &self,
        model: Option<&str>,
        premium_opt_in: bool,
    ) -> Result<(), String> {
        let Some(model) = model else {
            return Ok(());
        };
        self.validate_swarm_model_resolved(model, model, premium_opt_in)
    }

    /// Validate a request whose bare id was resolved to a qualified route.
    ///
    /// Membership is satisfied by either spelling, so a whitelisted bare id
    /// that maps to exactly one route keeps working, and a whitelisted
    /// qualified id accepts the equivalent bare request. `resolved` is the same
    /// model on a route; this never authorizes a different model.
    pub fn validate_swarm_model_resolved(
        &self,
        requested: &str,
        resolved: &str,
        premium_opt_in: bool,
    ) -> Result<(), String> {
        let model = requested;
        if model.trim().is_empty() {
            return Err(format!(
                "Swarm model request is blank. Omit `model` to use the configured default ({SWARM_MODEL_DEFAULT_KEY}), \
                 or pass an exact route such as `/subagent-model allow <route>` and retry."
            ));
        }
        if is_inherit_sentinel(model) {
            return Err(format!(
                "Swarm model `{model}` is not a route. Omit `model` to inherit the coordinator's model, \
                 or list an exact route in {SWARM_MODEL_ALLOWLIST_KEY} (see {SWARM_MODEL_POLICY_COMMAND})."
            ));
        }
        if self.swarm_model_allowlist.is_empty() {
            return Err(format!(
                "Swarm model `{model}` refused: {SWARM_MODEL_ALLOWLIST_KEY} is empty, so no model override is permitted. \
                 Omit `model` to use the configured default, or run `{SWARM_MODEL_POLICY_COMMAND} allow {model}` \
                 (equivalently add \"{model}\" to {SWARM_MODEL_ALLOWLIST_KEY} in ~/.jcode/config.toml)."
            ));
        }
        let allowed_by_whitelist =
            self.swarm_model_allowed(model) || self.swarm_model_allowed(resolved);
        if !allowed_by_whitelist {
            return Err(format!(
                "Swarm model `{model}` refused: not in {SWARM_MODEL_ALLOWLIST_KEY} (allowed: {}). \
                 Run `{SWARM_MODEL_POLICY_COMMAND} allow {model}` or add it to {SWARM_MODEL_ALLOWLIST_KEY} in ~/.jcode/config.toml.",
                self.allowlist_display()
            ));
        }
        let premium = self.swarm_model_premium(model) || self.swarm_model_premium(resolved);
        if premium && !premium_opt_in {
            return Err(format!(
                "Swarm model `{model}` is designated premium in {SWARM_PREMIUM_MODELS_KEY} and requires \
                 `{SWARM_PREMIUM_OPT_IN_FIELD}: true` on the spawn call. Pass the opt-in, or run \
                 `{SWARM_MODEL_POLICY_COMMAND} unpremium {model}` to drop the acknowledgement."
            ));
        }
        Ok(())
    }

    /// Config-load consistency rules for the whitelist policy. Any violation is
    /// a hard load error so a broken policy never silently changes behaviour.
    pub fn validate_swarm_model_policy(&self) -> Result<(), String> {
        for entry in &self.swarm_model_allowlist {
            if entry.is_empty() || entry.trim() != entry {
                return Err(format!(
                    "Invalid entry {entry:?} in {SWARM_MODEL_ALLOWLIST_KEY}: entries must be exact routes \
                     with no surrounding whitespace. Remove the entry or fix it in ~/.jcode/config.toml."
                ));
            }
            if is_inherit_sentinel(entry) {
                return Err(format!(
                    "Invalid entry {entry:?} in {SWARM_MODEL_ALLOWLIST_KEY}: omit `model` to inherit the \
                     coordinator instead of whitelisting an `inherit` alias."
                ));
            }
        }
        if let Some(default) = self.swarm_model.as_deref() {
            let default = default.trim();
            if !default.is_empty() && !is_inherit_sentinel(default) {
                if self.swarm_model_allowlist.is_empty() {
                    return Err(format!(
                        "{SWARM_MODEL_DEFAULT_KEY} = {default:?} is not whitelisted: {SWARM_MODEL_ALLOWLIST_KEY} is empty. \
                         Add \"{default}\" to {SWARM_MODEL_ALLOWLIST_KEY} (or `{SWARM_MODEL_POLICY_COMMAND} allow {default}`), \
                         or clear {SWARM_MODEL_DEFAULT_KEY} to inherit the coordinator's model."
                    ));
                }
                if !self.swarm_model_allowed(default) {
                    return Err(format!(
                        "{SWARM_MODEL_DEFAULT_KEY} = {default:?} is not in {SWARM_MODEL_ALLOWLIST_KEY} (allowed: {}). \
                         Add it with `{SWARM_MODEL_POLICY_COMMAND} allow {default}`, or set the default to a whitelisted route.",
                        self.allowlist_display()
                    ));
                }
            }
        }
        for premium in &self.swarm_premium_models {
            if !self.swarm_model_allowed(premium) {
                return Err(format!(
                    "{SWARM_PREMIUM_MODELS_KEY} entry {premium:?} is not in {SWARM_MODEL_ALLOWLIST_KEY}. \
                     Whitelist it first, or remove it from {SWARM_PREMIUM_MODELS_KEY}."
                ));
            }
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
