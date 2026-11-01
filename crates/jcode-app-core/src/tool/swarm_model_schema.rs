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

/// Offer `agent` on `spawn`. The schema is built once per process and cannot
/// know which project a given session works in, so it never lists project
/// definitions: it names only the user-level ones and says project definitions
/// are resolved from the coordinator's working directory. Spawn validates the
/// name there and its error lists the valid choices.
pub(super) fn apply_agent_definitions(schema: &mut Value) {
    apply_agent_definitions_for(schema, crate::agent_def::agent_definition_summaries(None));
}

pub(super) fn apply_agent_definitions_for(schema: &mut Value, user_level: Vec<(String, String)>) {
    let Some(properties) = schema["properties"].as_object_mut() else {
        return;
    };
    let mut description = String::from(
        "For spawn: name of an agent definition for the worker (.jcode/agents or .claude/agents in the coordinator's project, or ~/.jcode/agents).",
    );
    if !user_level.is_empty() {
        let listing = user_level
            .iter()
            .map(|(name, description)| {
                let first_line = description.lines().next().unwrap_or("");
                let short: String = first_line.chars().take(120).collect();
                format!("{name}: {short}")
            })
            .collect::<Vec<_>>()
            .join("; ");
        description.push_str(&format!(" User-level definitions: {listing}."));
    }
    properties.insert(
        "agent".to_string(),
        json!({
            "type": "string",
            "description": description,
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
    fn user_level_names_are_listed_with_descriptions_but_not_enforced() {
        let mut schema = base();
        apply_agent_definitions_for(
            &mut schema,
            vec![
                ("a".into(), "First one".into()),
                ("b".into(), "Second\nline".into()),
            ],
        );
        let agent = &schema["properties"]["agent"];
        assert_eq!(agent["type"], "string");
        assert!(
            agent.get("enum").is_none(),
            "project names are unknown here"
        );
        let description = agent["description"].as_str().unwrap();
        assert!(description.contains("a: First one") && description.contains("b: Second"));
        assert!(!description.contains("line"));
    }

    #[test]
    fn agent_is_offered_even_without_user_level_definitions() {
        let mut schema = base();
        apply_agent_definitions_for(&mut schema, vec![]);
        let description = schema["properties"]["agent"]["description"]
            .as_str()
            .unwrap();
        assert!(description.contains("coordinator's project"));
        assert!(!description.contains("User-level definitions"));
        assert!(schema["properties"].get("label").is_some());
    }
}
