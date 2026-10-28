use super::*;

#[test]
fn schema_still_requires_action() {
    let schema = CommunicateTool::new().parameters_schema();
    assert_eq!(schema["required"], json!(["action"]));
}

#[test]
fn schema_hides_model_override_by_default_and_keeps_effort_user_configured() {
    let schema = CommunicateTool::new().parameters_schema();
    let props = schema["properties"]
        .as_object()
        .expect("swarm schema should have properties");

    let mut schema = schema.clone();
    super::super::apply_swarm_model_policy(&mut schema, &crate::config::AgentsConfig::default());
    assert!(schema["properties"].get("model").is_none());
    // Worker effort is user-configured (`agents.swarm_effort`, default low),
    // never chosen by the coordinating agent.
    assert!(!props.contains_key("effort"));
    assert!(
        schema["properties"]["action"]["enum"]
            .as_array()
            .expect("action enum")
            .contains(&json!("list_models"))
    );
}

#[test]
fn schema_only_exposes_exact_allowlisted_worker_models_when_enabled() {
    let mut schema = CommunicateTool::new().parameters_schema();
    let mut agents = crate::config::AgentsConfig {
        swarm_model_allowlist: vec!["openai:gpt-5.5".to_string()],
        ..Default::default()
    };
    super::super::apply_swarm_model_policy(&mut schema, &agents);
    assert!(schema["properties"].get("model").is_none());
    agents.swarm_model_selection = true;
    super::super::apply_swarm_model_policy(&mut schema, &agents);
    assert_eq!(
        schema["properties"]["model"]["enum"],
        json!(["openai:gpt-5.5"])
    );
    agents.swarm_model_allowlist.clear();
    super::super::apply_swarm_model_policy(&mut schema, &agents);
    assert!(schema["properties"].get("model").is_none());
}

#[test]
fn spawning_actions_cannot_bypass_worker_model_policy() {
    let mut agents = crate::config::AgentsConfig {
        swarm_model_allowlist: vec!["claude:claude-opus-4-8".to_string()],
        ..Default::default()
    };
    for action in [
        "spawn",
        "assign_task",
        "assign_next",
        "fill_slots",
        "run_plan",
        "launch",
        "retry",
        "resume",
    ] {
        let input: CommunicateInput = serde_json::from_value(json!({
            "action": action, "model": "claude:claude-opus-4-8"
        }))
        .unwrap();
        assert!(
            agents
                .validate_swarm_model_request(input.model.as_deref())
                .is_err()
        );
        agents.swarm_model_selection = true;
        assert!(
            agents
                .validate_swarm_model_request(input.model.as_deref())
                .is_ok()
        );
        assert!(
            agents
                .validate_swarm_model_request(Some("claude-api:claude-opus-4-8"))
                .is_err()
        );
        agents.swarm_model_selection = false;
    }
}

#[tokio::test]
async fn stale_worker_model_override_is_rejected_before_server_dispatch() {
    let _env = crate::storage::lock_test_env();
    let temp = tempfile::TempDir::new().unwrap();
    let previous = std::env::var_os("JCODE_HOME");
    crate::env::set_var("JCODE_HOME", temp.path());
    let tool = CommunicateTool::new();
    let result = tool
        .execute(
            json!({"action": "spawn", "label": "worker", "model": "claude-api:claude-opus-4-8"}),
            test_ctx("coordinator", temp.path()),
        )
        .await;
    match previous {
        Some(value) => crate::env::set_var("JCODE_HOME", value),
        None => crate::env::remove_var("JCODE_HOME"),
    }
    let error = result.unwrap_err().to_string();
    assert!(
        error.contains("Agent-selected swarm models are disabled"),
        "{error}"
    );
}

#[test]
fn schema_requires_a_nonblank_label_for_spawn() {
    let schema = CommunicateTool::new().parameters_schema();
    assert_eq!(schema["properties"]["label"]["minLength"], json!(1));
    assert!(
        schema["properties"]["label"]["description"]
            .as_str()
            .expect("label description")
            .contains("Required for spawn")
    );
    assert!(
        schema["properties"]["action"]["description"]
            .as_str()
            .expect("action description")
            .contains("spawn requires label")
    );

    let branches = schema["anyOf"]
        .as_array()
        .expect("swarm schema should declare action-specific branches");
    let spawn_branch = branches
        .iter()
        .find(|branch| branch["properties"]["action"]["enum"] == json!(["spawn"]))
        .expect("spawn schema branch");
    assert_eq!(spawn_branch["required"], json!(["action", "label"]));
    let prompt_description = schema["properties"]["prompt"]["description"]
        .as_str()
        .expect("prompt description");
    assert!(prompt_description.contains("Optional extra context"));
    assert!(prompt_description.contains("do not repeat its tasks"));
    assert!(
        schema["properties"]["label"]["description"]
            .as_str()
            .expect("label description")
            .contains("source of truth")
    );

    let non_spawn_branch = branches
        .iter()
        .find(|branch| {
            branch["properties"]["action"]["enum"]
                .as_array()
                .is_some_and(|actions| !actions.contains(&json!("spawn")))
        })
        .expect("non-spawn schema branch");
    assert_eq!(non_spawn_branch["required"], json!(["action"]));
}

#[test]
fn schema_branches_only_require_properties_they_declare() {
    // Gemini rejects the entire request when a `required` entry names a property
    // the same object does not define, which made every tool-enabled Gemini call
    // fail on this tool's spawn branch (issue #655).
    let schema = CommunicateTool::new().parameters_schema();
    for branch in schema["anyOf"].as_array().expect("schema branches") {
        let declared = branch["properties"]
            .as_object()
            .expect("branch properties")
            .keys()
            .cloned()
            .collect::<Vec<_>>();
        for required in branch["required"].as_array().expect("branch required") {
            let name = required.as_str().expect("required name");
            assert!(
                declared.iter().any(|known| known == name),
                "branch requires '{name}' without declaring it: {branch}"
            );
        }
    }
}

#[test]
fn spawn_label_validation_rejects_missing_or_blank_labels() {
    let missing: CommunicateInput =
        serde_json::from_value(json!({"action": "spawn"})).expect("spawn input");
    assert_eq!(
        missing
            .required_spawn_label()
            .expect_err("missing label must fail")
            .to_string(),
        "'label' is required for spawn action"
    );

    let blank: CommunicateInput = serde_json::from_value(json!({
        "action": "spawn",
        "label": "  \n\t "
    }))
    .expect("spawn input");
    assert_eq!(
        blank
            .required_spawn_label()
            .expect_err("blank label must fail")
            .to_string(),
        "'label' must not be blank for spawn action"
    );
}

#[test]
fn spawn_label_validation_trims_valid_labels() {
    let params: CommunicateInput = serde_json::from_value(json!({
        "action": "spawn",
        "label": "  api reviewer  "
    }))
    .expect("spawn input");
    assert_eq!(
        params.required_spawn_label().expect("valid label"),
        "api reviewer"
    );
}

#[tokio::test]
async fn spawn_execute_rejects_missing_label_before_sending_request() {
    let working_dir = tempfile::tempdir().expect("working dir");
    let error = CommunicateTool::new()
        .execute(
            json!({"action": "spawn", "prompt": "review the API"}),
            test_ctx("session-parent", working_dir.path()),
        )
        .await
        .expect_err("missing spawn label must fail locally");

    assert_eq!(error.to_string(), "'label' is required for spawn action");
}

#[test]
fn description_includes_swarm_prompt_guidance() {
    let tool = CommunicateTool::new();
    let description = tool.description();
    assert!(
        description.starts_with("Coordinate agents"),
        "description should lead with the short coordination summary"
    );
    assert!(
        description.contains("Swarm prompt"),
        "description should embed the swarm prompt section"
    );
}

#[test]
fn existing_tool_keeps_prompt_while_new_tool_loads_edit() {
    let project = tempfile::tempdir().unwrap();
    let prompt_dir = project.path().join(".jcode");
    std::fs::create_dir_all(&prompt_dir).unwrap();
    let prompt_path = prompt_dir.join("swarm-prompt.md");
    std::fs::write(&prompt_path, "first routing version").unwrap();

    let existing = CommunicateTool::new_for_working_dir(Some(project.path()));
    std::fs::write(&prompt_path, "second routing version").unwrap();
    let newly_created = CommunicateTool::new_for_working_dir(Some(project.path()));

    assert!(existing.description().contains("first routing version"));
    assert!(!existing.description().contains("second routing version"));
    assert!(
        newly_created
            .description()
            .contains("second routing version")
    );
}

#[test]
fn spawning_action_inputs_preserve_requested_model() {
    for action in [
        "spawn",
        "assign_task",
        "assign_next",
        "fill_slots",
        "run_plan",
    ] {
        for model in [
            "z-ai/glm-5.2:free",
            "openai-api:gpt-5.5",
            "inherit",
            "coordinator",
            "  ",
        ] {
            let input: CommunicateInput = serde_json::from_value(json!({
                "action": action,
                "label": "reviewer",
                "model": model
            }))
            .unwrap();
            assert_eq!(input.model.as_deref(), Some(model));
        }
    }
}

#[test]
fn spawning_action_inputs_allow_omitted_or_null_model() {
    for action in [
        "spawn",
        "assign_task",
        "assign_next",
        "fill_slots",
        "run_plan",
    ] {
        let without_model: CommunicateInput =
            serde_json::from_value(json!({"action": action, "label": "reviewer"})).unwrap();
        assert!(without_model.model.is_none());
        let null_model: CommunicateInput = serde_json::from_value(json!({
            "action": action,
            "label": "reviewer",
            "model": null
        }))
        .unwrap();
        assert!(null_model.model.is_none());
    }
}

#[test]
fn format_swarm_model_list_renders_routes_and_default() {
    let routes = vec![
        jcode_provider_core::ModelRoute {
            model: "gpt-5.5".to_string(),
            provider: "OpenAI".to_string(),
            api_method: "openai-api-key".to_string(),
            available: true,
            detail: "API key".to_string(),
            usage: None,
            cheapness: None,
        },
        jcode_provider_core::ModelRoute {
            model: "claude-fable-5".to_string(),
            provider: "Anthropic".to_string(),
            api_method: "anthropic-api-key".to_string(),
            available: false,
            detail: String::new(),
            usage: None,
            cheapness: None,
        },
    ];
    let output =
        format_swarm_model_list(Some("claude-fable-5"), Some("openai-api:gpt-5.5"), &routes);
    assert!(output.contains("Current coordinator model: claude-fable-5"));
    assert!(output.contains("Configured agents.swarm_model default: openai-api:gpt-5.5"));
    assert!(output.contains("gpt-5.5 via OpenAI [openai-api-key] (API key)"));
    assert!(output.contains("claude-fable-5 via Anthropic [anthropic-api-key] [unavailable]"));
    assert!(!output.contains("pass effort"));
}

#[test]
fn format_swarm_model_list_handles_empty_catalog() {
    let output = format_swarm_model_list(None, None, &[]);
    assert!(output.contains("Current coordinator model: unknown"));
    assert!(output.contains("No agents.swarm_model default configured"));
    assert!(!output.contains("unless model is passed"));
    assert!(output.contains("No model routes reported"));
}

#[test]
fn schema_does_not_advertise_explicit_completion_reporting() {
    let tool = CommunicateTool::new();
    let schema = tool.parameters_schema();
    let serialized = schema.to_string();
    assert!(!serialized.contains("report"), "{serialized}");
    assert!(!serialized.contains("validation"), "{serialized}");
    assert!(!serialized.contains("follow_up"), "{serialized}");
}

#[test]
fn legacy_report_input_remains_compatible() {
    let input: super::CommunicateInput = serde_json::from_value(json!({
        "action": "report",
        "message": "Implemented the change.",
        "status": "completed",
        "validation": "Tests passed.",
        "follow_up": "None."
    }))
    .expect("old transcripts and scripts can still deserialize reports");
    assert_eq!(input.action, "report");
    assert_eq!(input.status.as_deref(), Some("completed"));
    assert_eq!(input.validation.as_deref(), Some("Tests passed."));
    assert_eq!(input.follow_up.as_deref(), Some("None."));
}

#[test]
fn schema_advertises_a_lean_lane_first_action_set() {
    let schema = CommunicateTool::new().parameters_schema();
    let props = schema["properties"]
        .as_object()
        .expect("swarm schema should have properties");
    let actions: Vec<&str> = schema["properties"]["action"]["enum"]
        .as_array()
        .expect("action enum")
        .iter()
        .filter_map(|a| a.as_str())
        .collect();
    for kept in [
        "launch",
        "spawn",
        "message",
        "status",
        "plan_status",
        "list",
        "summary",
        "await_members",
        "stop",
        "resume",
        "retry",
        "list_models",
    ] {
        assert!(
            actions.contains(&kept),
            "{kept} should be offered: {actions:?}"
        );
    }
    for hidden in [
        "report",
        "propose_plan",
        "approve_plan",
        "reject_plan",
        "assign_task",
        "assign_next",
        "fill_slots",
        "run_plan",
        "resync_plan",
        "share",
        "read",
        "start_task",
        "salvage",
        "subscribe_channel",
        "task_graph",
    ] {
        assert!(!actions.contains(&hidden), "{hidden} should not be offered");
    }
    assert!(
        actions.len() <= 16,
        "keep the swarm tool small: {actions:?}"
    );
    for kept in [
        "message",
        "to_session",
        "target_session",
        "channel",
        "label",
        "phase",
        "prompt",
        "session_ids",
        "background",
        "concurrency_limit",
        "retain_agents",
        "delivery",
        "wake",
    ] {
        assert!(props.contains_key(kept), "{kept} param should be offered");
    }
    for dropped in [
        "status",
        "validation",
        "follow_up",
        "key",
        "value",
        "proposer_session",
        "reason",
        "role",
        "plan_items",
    ] {
        assert!(
            !props.contains_key(dropped),
            "{dropped} belongs to hidden actions"
        );
    }
}
