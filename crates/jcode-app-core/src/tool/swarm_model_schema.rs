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

/// Constrain `agent` to the loaded definition names, with their descriptions,
/// and leave it out entirely when no definitions exist, so the model never sees
/// a dead knob.
/// Resolved from the process working directory and user directories; the spawn
/// handler re-validates against the coordinator's own working directory.
pub(super) fn apply_agent_definitions(schema: &mut Value) {
    let cwd = std::env::current_dir().ok();
    apply_agent_definitions_for(
        schema,
        crate::agent_def::agent_definition_summaries(cwd.as_deref()),
    );
}

pub(super) fn apply_agent_definitions_for(schema: &mut Value, summaries: Vec<(String, String)>) {
    let Some(properties) = schema["properties"].as_object_mut() else {
        return;
    };
    if summaries.is_empty() {
        return;
    }
    let names: Vec<&str> = summaries.iter().map(|(name, _)| name.as_str()).collect();
    let listing = summaries
        .iter()
        .map(|(name, description)| {
            let first_line = description.lines().next().unwrap_or_default();
            let short: String = first_line.chars().take(120).collect();
            format!("{name}: {short}")
        })
        .collect::<Vec<_>>()
        .join("; ");
    properties.insert(
        "agent".to_string(),
        json!({
            "type": "string",
            "enum": names,
            "description": format!("For spawn: agent definition for the worker. {listing}"),
        }),
    );
}

#[cfg(test)]
mod tests {
    use super::*;

    fn base() -> Value {
        json!({"properties": {"label": {"type": "string"}}})
    }

    #[test]
    fn enum_lists_loaded_names_with_descriptions() {
        let mut schema = base();
        apply_agent_definitions_for(
            &mut schema,
            vec![
                ("a".into(), "First one".into()),
                ("b".into(), "Second\nline".into()),
            ],
        );
        assert_eq!(schema["properties"]["agent"]["enum"], json!(["a", "b"]));
        let description = schema["properties"]["agent"]["description"]
            .as_str()
            .unwrap();
        assert!(description.contains("a: First one") && description.contains("b: Second"));
        assert!(!description.contains("line"));
    }

    #[test]
    fn no_definitions_leaves_the_property_out() {
        let mut schema = json!({"properties": {"label": {"type": "string"}}});
        apply_agent_definitions_for(&mut schema, vec![]);
        assert!(schema["properties"].get("agent").is_none());
        assert!(schema["properties"].get("label").is_some());
    }
}
