use super::*;

pub(super) fn apply_swarm_model_policy(schema: &mut Value, agents: &crate::config::AgentsConfig) {
    let Some(properties) = schema["properties"].as_object_mut() else {
        crate::logging::error(
            "Cannot apply swarm model policy: schema properties are not an object",
        );
        return;
    };
    if agents.swarm_model_registry {
        // No enum: the catalog is live and per-session. Spawn validates it.
        properties.insert(
            "model".to_string(),
            json!({
                "type": "string",
                "description": "Worker model from `list_models` (available routes only), optionally `model@api_method` to pin the route. Unknown models fail with the live list. Omit for configured default."
            }),
        );
    } else if !agents.swarm_model_selection || agents.swarm_model_allowlist.is_empty() {
        properties.remove("model");
    } else {
        properties.insert(
            "model".to_string(),
            json!({
                "type": "string",
                "enum": agents.swarm_model_allowlist,
                "description": "Exact user-allowlisted route; omit for configured default."
            }),
        );
    }
}
