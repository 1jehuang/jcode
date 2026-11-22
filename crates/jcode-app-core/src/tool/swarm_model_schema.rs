use super::*;

pub(super) fn apply_swarm_model_policy(schema: &mut Value, agents: &crate::config::AgentsConfig) {
    let Some(properties) = schema["properties"].as_object_mut() else {
        crate::logging::error(
            "Cannot apply swarm model policy: schema properties are not an object",
        );
        return;
    };
    // The whitelist is authoritative: with an empty list no model override is
    // offered at all, so the model cannot be even requested.
    if agents.swarm_model_allowlist.is_empty() {
        properties.remove("model");
        properties.remove("premium_opt_in");
        return;
    }
    properties.insert(
        "model".to_string(),
        json!({
            "type": "string",
            "enum": agents.swarm_model_allowlist,
            "description": "Exact user-whitelisted route (agents.swarm_model_allowlist); omit for the configured deterministic default."
        }),
    );
    if agents.swarm_premium_models.is_empty() {
        properties.remove("premium_opt_in");
    } else {
        properties.insert(
            "premium_opt_in".to_string(),
            json!({
                "type": "boolean",
                "description": format!(
                    "Required for models designated premium in agents.swarm_premium_models ({}). Never widens the whitelist.",
                    agents.swarm_premium_models.join(", ")
                )
            }),
        );
    }
}
