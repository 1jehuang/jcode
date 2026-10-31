use super::*;

pub(super) fn apply_swarm_model_policy(schema: &mut Value, agents: &crate::config::AgentsConfig) {
    let Some(properties) = schema["properties"].as_object_mut() else {
        crate::logging::error(
            "Cannot apply swarm model policy: schema properties are not an object",
        );
        return;
    };
    if !agents.swarm_model_selection || agents.swarm_model_allowlist.is_empty() {
        properties.remove("model");
    } else {
        properties.insert("model".to_string(), json!({
            "type": "string",
            "enum": agents.swarm_model_allowlist,
            "description": "Optional worker model. Only exact user-allowlisted routes are permitted. Omit to use the configured default."
        }));
    }
}
