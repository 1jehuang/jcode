#![cfg_attr(test, allow(clippy::await_holding_lock))]

use super::*;
use crate::message::{Message, ToolDefinition};
use crate::provider::{EventStream, Provider};
use async_trait::async_trait;
use serde_json::Value;

struct MockProvider;

#[async_trait]
impl Provider for MockProvider {
    async fn complete(
        &self,
        _messages: &[Message],
        _tools: &[ToolDefinition],
        _system: &str,
        _resume_session_id: Option<&str>,
    ) -> anyhow::Result<EventStream> {
        Err(anyhow::anyhow!(
            "Mock provider should not be used for streaming completions in tool registry tests"
        ))
    }

    fn name(&self) -> &str {
        "mock"
    }

    fn fork(&self) -> Arc<dyn Provider> {
        Arc::new(MockProvider)
    }
}

#[tokio::test]
async fn test_tool_definitions_are_sorted() {
    // Create registry with mock provider
    let provider: Arc<dyn Provider> = Arc::new(MockProvider);
    let registry = Registry::new(provider).await;

    // Get definitions multiple times and verify they're always in the same order
    let defs1 = registry.definitions(None).await;
    let defs2 = registry.definitions(None).await;

    // Should have the same order
    assert_eq!(defs1.len(), defs2.len());
    for (d1, d2) in defs1.iter().zip(defs2.iter()) {
        assert_eq!(d1.name, d2.name);
    }

    // Verify they're sorted alphabetically
    let names: Vec<&str> = defs1.iter().map(|d| d.name.as_str()).collect();
    let mut sorted_names = names.clone();
    sorted_names.sort();
    assert_eq!(
        names, sorted_names,
        "Tool definitions should be sorted alphabetically"
    );
}

#[test]
fn test_resolve_skill_aliases_to_skill_manage() {
    assert_eq!(Registry::resolve_tool_name("skill"), "skill_manage");
    assert_eq!(Registry::resolve_tool_name("Skill"), "skill_manage");
    assert_eq!(Registry::resolve_tool_name("skill_manage"), "skill_manage");
}

#[tokio::test]
async fn test_discover_tools_not_registered_when_sponsors_disabled() {
    // sponsors.enabled is the legacy config key; when false, integration discovery must not exist.
    let provider: Arc<dyn Provider> = Arc::new(MockProvider);
    let registry = Registry::new(provider).await;
    let names = registry.tool_names().await;
    if crate::config::config().sponsors.enabled {
        assert!(names.iter().any(|n| n == "integration_tools"));
    } else {
        assert!(
            !names.iter().any(|n| n == "integration_tools"),
            "integration_tools must not be registered when sponsors are disabled"
        );
    }
}

#[tokio::test]
async fn subagent_tool_is_not_registered() {
    let provider: Arc<dyn Provider> = Arc::new(MockProvider);
    let registry = Registry::new(provider).await;

    assert!(
        !registry
            .tool_names()
            .await
            .iter()
            .any(|name| name == "subagent"),
        "the deprecated direct subagent tool must not be exposed; use swarm instead"
    );
}

struct BareSchemaTool;

#[async_trait]
impl Tool for BareSchemaTool {
    fn name(&self) -> &str {
        "bare_schema"
    }

    fn description(&self) -> &str {
        "Test tool without an explicit intent property."
    }

    fn parameters_schema(&self) -> Value {
        serde_json::json!({
            "type": "object",
            "required": ["command"],
            "properties": {
                "command": {"type": "string"}
            }
        })
    }

    async fn execute(&self, _input: Value, _ctx: ToolContext) -> Result<ToolOutput> {
        Ok(ToolOutput::new("ok"))
    }
}

/// `to_definition` deliberately injects a required `intent` into every
/// object-shaped tool schema (8505080a6), so a tool that omits `intent` from its
/// own `parameters_schema` still advertises it. This pins that central
/// behaviour: a bare schema gains `intent` as both a property and a requirement.
#[test]
fn tool_definitions_auto_inject_required_intent() {
    let def = BareSchemaTool.to_definition();
    assert_eq!(def.input_schema["properties"]["intent"]["type"], "string");
    let required = def.input_schema["required"]
        .as_array()
        .cloned()
        .unwrap_or_default();
    assert!(
        required.iter().any(|value| value == "intent"),
        "intent must be required after central injection: {required:?}"
    );
    assert!(
        required.iter().any(|value| value == "command"),
        "injection must preserve the tool's own required fields: {required:?}"
    );
}

#[tokio::test]
async fn first_party_tool_definitions_require_intent_with_display_only_docs() {
    let provider: Arc<dyn Provider> = Arc::new(MockProvider);
    let registry = Registry::new(provider).await;
    registry.register_ambient_tools().await;
    registry.register_selfdev_tools().await;

    let defs = registry.definitions(None).await;
    assert!(!defs.is_empty());

    for def in defs {
        let schema = &def.input_schema;
        if schema["type"] != "object" {
            continue;
        }

        assert_eq!(
            schema["properties"]["intent"]["type"], "string",
            "{} should explicitly define optional intent in its schema",
            def.name
        );
        assert!(
            schema["properties"]["intent"]["description"]
                .as_str()
                .unwrap_or_default()
                .contains("shown in the UI"),
            "{} intent description should say it is UI-display-only",
            def.name
        );
        let required = schema["required"].as_array().cloned().unwrap_or_default();
        assert!(
            required.iter().any(|value| value == "intent"),
            "{} must require intent",
            def.name
        );
    }
}

#[test]
fn test_resolve_tool_name_oauth_aliases() {
    assert_eq!(Registry::resolve_tool_name("file_read"), "read");
    assert_eq!(Registry::resolve_tool_name("file_write"), "write");
    assert_eq!(Registry::resolve_tool_name("file_edit"), "edit");
    assert_eq!(Registry::resolve_tool_name("shell_exec"), "bash");
    assert_eq!(Registry::resolve_tool_name("shell"), "bash");
    assert_eq!(Registry::resolve_tool_name("read_file"), "read");
    assert_eq!(Registry::resolve_tool_name("write_file"), "write");
    assert_eq!(Registry::resolve_tool_name("edit_file"), "edit");
    assert_eq!(Registry::resolve_tool_name("task_runner"), "subagent");
    assert_eq!(Registry::resolve_tool_name("task"), "subagent");
    assert_eq!(Registry::resolve_tool_name("launch"), "open");
    assert_eq!(Registry::resolve_tool_name("grep"), "agentgrep");
    assert_eq!(Registry::resolve_tool_name("file_grep"), "agentgrep");
    assert_eq!(Registry::resolve_tool_name("todo_read"), "todo");
    assert_eq!(Registry::resolve_tool_name("todo_write"), "todo");
    assert_eq!(Registry::resolve_tool_name("todoread"), "todo");
    assert_eq!(Registry::resolve_tool_name("todowrite"), "todo");
    assert_eq!(Registry::resolve_tool_name("bash"), "bash");
    assert_eq!(Registry::resolve_tool_name("functions.bash"), "bash");
    assert_eq!(Registry::resolve_tool_name("functions.shell_exec"), "bash");
    assert_eq!(Registry::resolve_tool_name("batch"), "batch");
    assert_eq!(Registry::resolve_tool_name("memory"), "memory");
}

#[tokio::test]
async fn test_batch_resolves_function_namespaced_tools() {
    let provider: Arc<dyn Provider> = Arc::new(MockProvider);
    let registry = Registry::new(provider).await;
    let ctx = ToolContext {
        session_id: "test-batch-function-namespace".to_string(),
        message_id: "test".to_string(),
        tool_call_id: "test".to_string(),
        working_dir: Some(std::env::temp_dir()),
        stdin_request_tx: None,
        graceful_shutdown_signal: None,
        execution_mode: ToolExecutionMode::Direct,
    };

    let result = registry
        .execute(
            "batch",
            serde_json::json!({
                "tool_calls": [
                    {"tool": "functions.bash", "command": "true"},
                    {"tool": "functions.shell_exec", "command": "true"}
                ]
            }),
            ctx,
        )
        .await
        .expect("namespaced batch subcalls should execute");

    assert!(result.output.contains("Completed: 2 succeeded, 0 failed"));
    assert!(!result.output.contains("Unknown tool"));
    assert!(result.output.contains("--- [1] bash ---"));
    assert!(result.output.contains("--- [2] bash ---"));
    assert!(!result.output.contains("functions."));
}

#[tokio::test]
async fn test_batch_rejects_function_namespaced_batch_recursion() {
    let provider: Arc<dyn Provider> = Arc::new(MockProvider);
    let registry = Registry::new(provider).await;
    let ctx = ToolContext {
        session_id: "test-batch-function-namespace-recursion".to_string(),
        message_id: "test".to_string(),
        tool_call_id: "test".to_string(),
        working_dir: Some(std::env::temp_dir()),
        stdin_request_tx: None,
        graceful_shutdown_signal: None,
        execution_mode: ToolExecutionMode::Direct,
    };

    let error = registry
        .execute(
            "batch",
            serde_json::json!({
                "tool_calls": [{"tool": "functions.batch", "tool_calls": []}]
            }),
            ctx,
        )
        .await
        .expect_err("namespaced batch recursion should be rejected");

    assert!(error.to_string().contains("Cannot batch the 'batch' tool"));
}

#[tokio::test]
async fn test_batch_resolves_oauth_names() {
    let provider: Arc<dyn Provider> = Arc::new(MockProvider);
    let registry = Registry::new(provider).await;
    let temp_dir = std::env::temp_dir();

    let ctx = ToolContext {
        session_id: "test".to_string(),
        message_id: "test".to_string(),
        tool_call_id: "test".to_string(),
        working_dir: Some(temp_dir),
        stdin_request_tx: None,
        graceful_shutdown_signal: None,
        execution_mode: ToolExecutionMode::Direct,
    };

    let result = registry
        .execute("shell_exec", serde_json::json!({"command": "true"}), ctx)
        .await;
    assert!(result.is_ok(), "shell_exec should resolve to bash tool");
}

#[tokio::test]
async fn registry_execute_enforces_session_tool_policy_after_alias_resolution() {
    let provider: Arc<dyn Provider> = Arc::new(MockProvider);
    let registry = Registry::new(provider).await;
    let temp_dir = std::env::temp_dir();
    let session_id = "test-policy-deny";
    set_session_tool_policy(session_id, None, HashSet::from(["bash".to_string()]));

    let ctx = ToolContext {
        session_id: session_id.to_string(),
        message_id: "test".to_string(),
        tool_call_id: "test".to_string(),
        working_dir: Some(temp_dir.clone()),
        stdin_request_tx: None,
        graceful_shutdown_signal: None,
        execution_mode: ToolExecutionMode::Direct,
    };

    let result = registry
        .execute("shell_exec", serde_json::json!({"command": "true"}), ctx)
        .await;

    clear_session_tool_policy(session_id);
    assert!(result.is_err(), "deny-list should block aliased bash calls");
    assert!(
        result
            .unwrap_err()
            .to_string()
            .contains("Tool 'bash' is disabled")
    );
}

#[cfg(unix)]
#[tokio::test]
async fn registry_execute_pre_tool_hook_blocks_and_allows() {
    use std::os::unix::fs::PermissionsExt;

    let _guard = crate::storage::lock_test_env();
    let provider: Arc<dyn Provider> = Arc::new(MockProvider);
    let registry = Registry::new(provider).await;
    let temp = tempfile::TempDir::new().expect("temp dir");

    // Policy script: block bash calls whose input mentions "secret".
    let policy = temp.path().join("policy.sh");
    std::fs::write(
        &policy,
        "#!/bin/sh\ninput=$(cat)\ncase \"$input\" in\n  *secret*) echo \"no secrets\" >&2; exit 2 ;;\nesac\nexit 0\n",
    )
    .expect("write policy");
    std::fs::set_permissions(&policy, std::fs::Permissions::from_mode(0o755))
        .expect("chmod policy");

    let prev = std::env::var_os("JCODE_HOOK_PRE_TOOL");
    crate::env::set_var("JCODE_HOOK_PRE_TOOL", policy.to_string_lossy().to_string());
    // jcode-base is compiled without cfg(test) here, so the config cache only
    // re-checks env every 500ms; force a reload so the hook is visible now.
    crate::config::invalidate_config_cache();

    let ctx = || ToolContext {
        session_id: "test-pre-tool-hook".to_string(),
        message_id: "test".to_string(),
        tool_call_id: "test".to_string(),
        working_dir: Some(std::env::temp_dir()),
        stdin_request_tx: None,
        graceful_shutdown_signal: None,
        execution_mode: ToolExecutionMode::Direct,
    };

    let blocked = registry
        .execute(
            "bash",
            serde_json::json!({
                "command": "echo secret"
            }),
            ctx(),
        )
        .await;
    let allowed = registry
        .execute(
            "bash",
            serde_json::json!({
                "command": "true"
            }),
            ctx(),
        )
        .await;

    match prev {
        Some(value) => crate::env::set_var("JCODE_HOOK_PRE_TOOL", value),
        None => crate::env::remove_var("JCODE_HOOK_PRE_TOOL"),
    }
    crate::config::invalidate_config_cache();

    let error = blocked.expect_err("pre_tool hook should block matching input");
    assert!(
        error.to_string().contains("no secrets"),
        "hook stderr should surface in the error: {error}"
    );
    assert!(allowed.is_ok(), "non-matching input should pass the gate");
}

#[tokio::test]
async fn test_definitions_keep_batch_schema_generic() {
    let provider: Arc<dyn Provider> = Arc::new(MockProvider);
    let registry = Registry::new(provider).await;

    let defs = registry.definitions(None).await;
    let batch_def = defs
        .iter()
        .find(|def| def.name == "batch")
        .expect("batch definition should exist");

    assert!(batch_def.input_schema["properties"]["tool_calls"]["items"]["oneOf"].is_null());
    assert!(
        batch_def.input_schema["properties"]["tool_calls"]["items"]["required"]
            .as_array()
            .map(|required| required.iter().any(|value| value == "tool"))
            .unwrap_or(false)
    );
    assert!(
        batch_def.input_schema["properties"]["tool_calls"]["items"]["properties"]["parameters"]
            .is_null()
    );
}

#[test]
fn resolve_tool_name_maps_communicate_to_swarm() {
    assert_eq!(Registry::resolve_tool_name("communicate"), "swarm");
}

#[tokio::test]
#[ignore]
async fn print_tool_definition_token_report() {
    let provider: Arc<dyn Provider> = Arc::new(MockProvider);
    let registry = Registry::new(provider).await;
    let mut defs = registry.definitions(None).await;
    defs.sort_by_key(|def| std::cmp::Reverse(def.prompt_token_estimate()));

    println!("name,total_tokens,description_tokens");
    for def in defs {
        println!(
            "{},{},{}",
            def.name,
            def.prompt_token_estimate(),
            def.description_token_estimate()
        );
    }
}

/// Tool descriptions are always-on prompt cost, so they are capped at ~20
/// estimated tokens. Behavioral guidance belongs in parameter descriptions.
/// Exemptions must be justified inline.
#[tokio::test]
async fn tool_descriptions_stay_under_token_cap() {
    const DESCRIPTION_TOKEN_CAP: usize = 20;
    // integration_tools keeps a deliberate second sentence explaining that catalog
    // entries integrate directly with the agent.
    // swarm appends the user-tunable swarm-prompt.md by design.
    const EXEMPT: &[&str] = &["integration_tools", "swarm"];

    let provider: Arc<dyn Provider> = Arc::new(MockProvider);
    let registry = Registry::new(provider).await;
    let over_cap: Vec<String> = registry
        .definitions(None)
        .await
        .into_iter()
        .filter(|def| !EXEMPT.contains(&def.name.as_str()))
        .filter(|def| def.description_token_estimate() > DESCRIPTION_TOKEN_CAP)
        .map(|def| {
            format!(
                "{} (~{} tokens): {}",
                def.name,
                def.description_token_estimate(),
                def.description
            )
        })
        .collect();
    assert!(
        over_cap.is_empty(),
        "tool descriptions over the {DESCRIPTION_TOKEN_CAP}-token cap:\n{}",
        over_cap.join("\n")
    );
}

fn collect_param_descriptions(schema: &Value, path: &str, out: &mut Vec<(String, String)>) {
    match schema {
        Value::Object(map) => {
            if path != "$"
                && let Some(Value::String(description)) = map.get("description")
            {
                out.push((path.to_string(), description.clone()));
            }
            for (key, value) in map {
                if key == "description" {
                    continue;
                }
                collect_param_descriptions(value, &format!("{path}.{key}"), out);
            }
        }
        Value::Array(items) => {
            for (idx, item) in items.iter().enumerate() {
                collect_param_descriptions(item, &format!("{path}[{idx}]"), out);
            }
        }
        _ => {}
    }
}

/// Parameter descriptions inside tool schemas are also always-on prompt cost,
/// so each is capped. Longer guidance belongs in runtime error messages, docs,
/// or the system prompt (the todo calibration rubrics, for example, live in
/// the gate continuation messages in jcode-base::todo).
#[tokio::test]
async fn tool_parameter_descriptions_stay_under_token_cap() {
    const PARAM_DESCRIPTION_TOKEN_CAP: usize = 25;

    let provider: Arc<dyn Provider> = Arc::new(MockProvider);
    let registry = Registry::new(provider).await;
    let mut over_cap: Vec<String> = Vec::new();
    for def in registry.definitions(None).await {
        let mut descriptions = Vec::new();
        collect_param_descriptions(&def.input_schema, "$", &mut descriptions);
        for (path, description) in descriptions {
            let tokens = crate::util::estimate_tokens(&description);
            if tokens > PARAM_DESCRIPTION_TOKEN_CAP {
                over_cap.push(format!(
                    "{} {} (~{} tokens): {}",
                    def.name, path, tokens, description
                ));
            }
        }
    }
    assert!(
        over_cap.is_empty(),
        "{} parameter descriptions over the {PARAM_DESCRIPTION_TOKEN_CAP}-token cap:\n{}",
        over_cap.len(),
        over_cap.join("\n")
    );
}

fn schema_type_includes(schema: &Value, expected: &str) -> bool {
    match schema.get("type") {
        Some(Value::String(value)) => value == expected,
        Some(Value::Array(values)) => values
            .iter()
            .any(|value| value.as_str().is_some_and(|value| value == expected)),
        _ => false,
    }
}

fn collect_schema_errors(schema: &Value, path: &str, errors: &mut Vec<String>) {
    match schema {
        Value::Object(map) => {
            if schema_type_includes(schema, "array") && !map.contains_key("items") {
                errors.push(format!("{path}: array schema missing items"));
            }

            // Gemini validates `required` against the same object's `properties`
            // and rejects the entire request when a name is missing, which broke
            // every tool-enabled Gemini call (issue #655). Objects without a
            // local `properties` map are exempt: there is nothing to check
            // against, and Gemini accepts those.
            if let (Some(Value::Array(required)), Some(Value::Object(properties))) =
                (map.get("required"), map.get("properties"))
            {
                for name in required {
                    let Some(name) = name.as_str() else {
                        errors.push(format!("{path}.required: entries must be strings"));
                        continue;
                    };
                    if !properties.contains_key(name) {
                        errors.push(format!(
                            "{path}.required: '{name}' is not defined in the same object's properties"
                        ));
                    }
                }
            }

            for keyword in ["anyOf", "oneOf", "allOf"] {
                let Some(branches) = map.get(keyword) else {
                    continue;
                };
                let Some(branches) = branches.as_array() else {
                    errors.push(format!("{path}.{keyword}: must be an array"));
                    continue;
                };
                for (idx, branch) in branches.iter().enumerate() {
                    let branch_path = format!("{path}.{keyword}[{idx}]");
                    match branch {
                        Value::Object(branch_map) => {
                            if !branch_map.contains_key("type") {
                                errors.push(format!("{branch_path}: schema missing type"));
                            }
                        }
                        _ => errors.push(format!("{branch_path}: schema branch must be an object")),
                    }
                }
            }

            for (key, value) in map {
                collect_schema_errors(value, &format!("{path}.{key}"), errors);
            }
        }
        Value::Array(values) => {
            for (idx, value) in values.iter().enumerate() {
                collect_schema_errors(value, &format!("{path}[{idx}]"), errors);
            }
        }
        _ => {}
    }
}

#[tokio::test]
async fn test_tool_definitions_do_not_expose_invalid_array_schemas() {
    let provider: Arc<dyn Provider> = Arc::new(MockProvider);
    let registry = Registry::new(provider).await;

    let defs = registry.definitions(None).await;
    let mut errors = Vec::new();
    for def in &defs {
        collect_schema_errors(
            &def.input_schema,
            &format!("tool `{}`", def.name),
            &mut errors,
        );
    }

    assert!(
        errors.is_empty(),
        "tool definitions must not expose invalid schemas:\n{}",
        errors.join("\n")
    );
}

#[test]
fn test_schema_validator_rejects_any_of_branches_without_type() {
    let schema = serde_json::json!({
        "type": "object",
        "properties": {
            "status_filter": {
                "anyOf": [
                    { "enum": ["running", "completed"] },
                    { "type": "array", "items": { "type": "string" } }
                ]
            }
        }
    });

    let mut errors = Vec::new();
    collect_schema_errors(&schema, "tool `test`", &mut errors);

    assert!(
        errors
            .iter()
            .any(|error| error.contains("status_filter.anyOf[0]: schema missing type")),
        "expected missing type error, got: {errors:?}"
    );
}

#[tokio::test]
async fn test_context_guard_small_output_passes_through() {
    let compaction = Arc::new(RwLock::new(CompactionManager::new().with_budget(200_000)));
    let registry = Registry {
        tools: Arc::new(RwLock::new(HashMap::new())),
        skills: Arc::new(RwLock::new(crate::skill::SkillRegistry::default())),
        compaction,
    };

    let output = ToolOutput::new("small output");
    let result = registry.guard_context_overflow("test", output, false).await;
    assert_eq!(result.output, "small output");
}

#[tokio::test]
async fn test_context_guard_withholds_huge_single_output_by_default() {
    let compaction = Arc::new(RwLock::new(CompactionManager::new().with_budget(1000)));
    let registry = Registry {
        tools: Arc::new(RwLock::new(HashMap::new())),
        skills: Arc::new(RwLock::new(crate::skill::SkillRegistry::default())),
        compaction,
    };

    // 30% of 1000 = 300 tokens = 1200 chars max for a single output
    // Create output that's way larger
    let big_output = "x".repeat(8000); // 2000 tokens, well over 30% of 1000
    let output = ToolOutput::new(big_output.clone());
    let result = registry.guard_context_overflow("test", output, false).await;

    // The whole point of the refusal: none of the payload is spent.
    assert!(
        !result.output.contains(&"x".repeat(100)),
        "withheld output must not leak the payload"
    );
    assert!(
        result.output.contains("OUTPUT WITHHELD"),
        "should say the output was withheld, got: {}",
        result.output
    );
    assert!(
        result.output.contains("accept_large_output"),
        "should name the opt-in flag so the caller can retry"
    );
    // A refusal that costs as much as the payload would defeat itself.
    assert!(
        result.output.len() < 1200,
        "refusal should be cheap, was {} chars",
        result.output.len()
    );
}

#[tokio::test]
async fn test_context_guard_returns_truncated_output_when_caller_accepts() {
    let compaction = Arc::new(RwLock::new(CompactionManager::new().with_budget(1000)));
    let registry = Registry {
        tools: Arc::new(RwLock::new(HashMap::new())),
        skills: Arc::new(RwLock::new(crate::skill::SkillRegistry::default())),
        compaction,
    };

    let big_output = "x".repeat(8000);
    let output = ToolOutput::new(big_output.clone());
    let result = registry.guard_context_overflow("test", output, true).await;

    assert!(
        result.output.len() < big_output.len(),
        "opt-in still truncates to what the budget allows"
    );
    assert!(
        result.output.contains("TRUNCATED"),
        "should say the output was truncated, got: {}",
        result.output
    );
    assert!(
        result.output.starts_with(&"x".repeat(200)),
        "opt-in must actually return the payload prefix"
    );
}

#[tokio::test]
async fn test_context_guard_reports_the_real_cost_and_affordable_size() {
    // 200k budget, 40k already used. A 90k-token result is over the 30%
    // single-output ceiling (60k), so it is withheld. The quoted numbers must
    // match the actual arithmetic, since the caller decides based on them.
    let compaction = Arc::new(RwLock::new(CompactionManager::new().with_budget(200_000)));
    {
        let mut mgr = compaction.write().await;
        mgr.update_observed_input_tokens(40_000);
    }
    let registry = Registry {
        tools: Arc::new(RwLock::new(HashMap::new())),
        skills: Arc::new(RwLock::new(crate::skill::SkillRegistry::default())),
        compaction,
    };

    let output = ToolOutput::new("x".repeat(360_000)); // ~90k tokens
    let result = registry.guard_context_overflow("test", output, false).await;

    assert!(result.output.contains("OUTPUT WITHHELD"));
    assert!(
        result.output.contains("90k tokens"),
        "should quote the real output size, got: {}",
        result.output
    );
    assert!(
        result.output.contains("45%"),
        "should quote the share of budget (90k of 200k), got: {}",
        result.output
    );
    assert!(
        result.output.contains("200k context budget"),
        "should quote the budget, got: {}",
        result.output
    );
    assert!(
        result.output.contains("40k is already used"),
        "should quote context already spent, got: {}",
        result.output
    );
    assert!(
        result.output.contains("50k"),
        "should quote the affordable size, now bounded by the absolute \
         single-output ceiling rather than 30% of the budget, got: {}",
        result.output
    );
}

#[tokio::test]
async fn test_context_guard_truncates_when_context_nearly_full() {
    let compaction = Arc::new(RwLock::new(CompactionManager::new().with_budget(10_000)));
    {
        let mut mgr = compaction.write().await;
        mgr.update_observed_input_tokens(9500); // 95% full
    }
    let registry = Registry {
        tools: Arc::new(RwLock::new(HashMap::new())),
        skills: Arc::new(RwLock::new(crate::skill::SkillRegistry::default())),
        compaction,
    };

    // Even a modest output should get truncated when context is 95% full
    let output = ToolOutput::new("x".repeat(4000)); // 1000 tokens
    let result = registry.guard_context_overflow("test", output, false).await;
    assert!(
        result.output.contains("WITHHELD") || result.output.contains("CONTEXT LIMIT"),
        "Should warn about context limits when nearly full"
    );
}

#[tokio::test]
async fn test_context_guard_still_refuses_when_context_is_exhausted() {
    // With almost no room left there is nothing to spend, so accepting the cost
    // cannot buy anything. The opt-in must not become a way to blow past the
    // window entirely.
    let compaction = Arc::new(RwLock::new(CompactionManager::new().with_budget(10_000)));
    {
        let mut mgr = compaction.write().await;
        mgr.update_observed_input_tokens(9_990);
    }
    let registry = Registry {
        tools: Arc::new(RwLock::new(HashMap::new())),
        skills: Arc::new(RwLock::new(crate::skill::SkillRegistry::default())),
        compaction,
    };

    let payload = "x".repeat(400_000);
    let result = registry
        .guard_context_overflow("test", ToolOutput::new(payload.clone()), true)
        .await;
    assert!(
        result.output.len() < 2_000,
        "exhausted context must not return the payload, got {} chars",
        result.output.len()
    );
    assert!(
        result.output.contains("CONTEXT LIMIT REACHED"),
        "should report the hard limit, got: {}",
        result.output
    );
}

#[tokio::test]
async fn test_context_guard_zero_budget_passes_through() {
    let compaction = Arc::new(RwLock::new(CompactionManager::new().with_budget(0)));
    let registry = Registry {
        tools: Arc::new(RwLock::new(HashMap::new())),
        skills: Arc::new(RwLock::new(crate::skill::SkillRegistry::default())),
        compaction,
    };

    let output = ToolOutput::new("x".repeat(100_000));
    let result = registry.guard_context_overflow("test", output, false).await;
    assert_eq!(
        result.output.len(),
        100_000,
        "Zero budget should pass through"
    );
}

#[test]
fn test_accepts_large_output_requires_an_unambiguous_yes() {
    use super::accepts_large_output;

    assert!(accepts_large_output(
        &serde_json::json!({ "accept_large_output": true })
    ));
    // Models routinely stringify booleans, so accept the string spelling too.
    assert!(accepts_large_output(
        &serde_json::json!({ "accept_large_output": "true" })
    ));
    assert!(accepts_large_output(
        &serde_json::json!({ "accept_large_output": "TRUE" })
    ));

    // Everything else means no. Spending the rest of the window should never
    // happen because of a truthy-looking value.
    for input in [
        serde_json::json!({}),
        serde_json::json!({ "accept_large_output": false }),
        serde_json::json!({ "accept_large_output": "false" }),
        serde_json::json!({ "accept_large_output": 1 }),
        serde_json::json!({ "accept_large_output": "yes" }),
        serde_json::json!({ "accept_large_output": serde_json::Value::Null }),
        serde_json::json!({ "query": "accept_large_output" }),
    ] {
        assert!(
            !accepts_large_output(&input),
            "should not opt in for {input}"
        );
    }
}

#[tokio::test]
async fn test_request_permission_is_ambient_only() {
    let provider: Arc<dyn Provider> = Arc::new(MockProvider);
    let registry = Registry::new(provider).await;

    let defs = registry.definitions(None).await;
    assert!(
        !defs.iter().any(|d| d.name == "request_permission"),
        "request_permission should not be available in normal sessions"
    );

    registry.register_ambient_tools().await;
    let defs_after = registry.definitions(None).await;
    assert!(
        defs_after.iter().any(|d| d.name == "request_permission"),
        "request_permission should be available after ambient tool registration"
    );
}

#[test]
fn closest_tool_names_suggests_near_misses() {
    let available = ["todo", "end_ambient_cycle", "bash", "read", "write", "edit"];
    // Exact-ish prefix/typo cases the ambient agent hit (#104).
    let s = Registry::closest_tool_names("todos", &available);
    assert_eq!(s.first().map(String::as_str), Some("todo"));

    let s = Registry::closest_tool_names("end_ambient_cyle", &available);
    assert!(s.iter().any(|n| n == "end_ambient_cycle"), "got {s:?}");

    // Case-insensitive containment.
    let s = Registry::closest_tool_names("Bash", &available);
    assert_eq!(s.first().map(String::as_str), Some("bash"));

    // A wildly unrelated name should yield no confident suggestion.
    let s = Registry::closest_tool_names("xyzzy_quux", &available);
    assert!(s.is_empty(), "got {s:?}");
}

#[tokio::test]
async fn unknown_tool_error_lists_available_tools_and_suggestions() {
    let provider: Arc<dyn Provider> = Arc::new(MockProvider);
    let registry = Registry::new(provider).await;
    registry.register_ambient_tools().await;

    let ctx = ToolContext {
        session_id: "test-unknown-tool".to_string(),
        message_id: "test".to_string(),
        tool_call_id: "test".to_string(),
        working_dir: None,
        stdin_request_tx: None,
        graceful_shutdown_signal: None,
        execution_mode: ToolExecutionMode::Direct,
    };
    let err = registry
        .execute("ToolSearch", serde_json::json!({}), ctx)
        .await
        .expect_err("ToolSearch is not a real tool");
    let msg = err.to_string();
    assert!(msg.contains("Unknown tool: ToolSearch"), "got: {msg}");
    assert!(
        msg.contains("Available tools:"),
        "error must list available tools so the model can recover (#104): {msg}"
    );
    assert!(
        msg.contains("end_ambient_cycle"),
        "available list should include registered ambient tools: {msg}"
    );
}

#[tokio::test]
async fn gemini_build_tools_from_registry_definitions_omits_const_keywords() {
    // Moved from jcode-base/src/provider/gemini_tests.rs: this is the one test
    // that needs the upper-layer tool::Registry, so it lives here instead of
    // forcing a base -> app-core dev-dependency cycle.
    fn schema_contains_key(schema: &serde_json::Value, key: &str) -> bool {
        match schema {
            serde_json::Value::Object(map) => {
                map.contains_key(key) || map.values().any(|value| schema_contains_key(value, key))
            }
            serde_json::Value::Array(items) => {
                items.iter().any(|value| schema_contains_key(value, key))
            }
            _ => false,
        }
    }

    let provider: Arc<dyn Provider> = Arc::new(MockProvider);
    let registry = Registry::new(provider).await;
    let defs = registry.definitions(None).await;

    let built = crate::provider::gemini::build_tools(&defs).expect("gemini tools");
    let parameters = &built[0].function_declarations;

    assert!(!schema_contains_key(
        &serde_json::json!(parameters),
        "const"
    ));

    // Gemini rejects the whole generateContent request when any `required` entry
    // names a property the same object does not declare, which made every
    // tool-enabled Gemini call fail (issue #655). Assert on the *converted*
    // declarations: the pre-conversion sweep in
    // `test_tool_definitions_do_not_expose_invalid_array_schemas` cannot prove
    // the adapter output is clean, and the adapter is what Gemini actually sees.
    let mut dangling = Vec::new();
    for declaration in parameters {
        collect_dangling_required(
            &declaration.parameters,
            &format!("tool `{}`", declaration.name),
            &mut dangling,
        );
    }
    assert!(
        dangling.is_empty(),
        "converted Gemini function declarations still require undeclared properties:\n{}",
        dangling.join("\n")
    );
}

/// Collect `required` entries that name a property absent from the same
/// object's `properties` map. Objects without a local `properties` map are
/// exempt, matching what Gemini validates.
fn collect_dangling_required(schema: &Value, path: &str, errors: &mut Vec<String>) {
    match schema {
        Value::Object(map) => {
            if let (Some(Value::Array(required)), Some(Value::Object(properties))) =
                (map.get("required"), map.get("properties"))
            {
                for name in required {
                    if let Some(name) = name.as_str()
                        && !properties.contains_key(name)
                    {
                        errors.push(format!("{path}.required: '{name}' is not declared here"));
                    }
                }
            }
            for (key, value) in map {
                collect_dangling_required(value, &format!("{path}.{key}"), errors);
            }
        }
        Value::Array(values) => {
            for (idx, value) in values.iter().enumerate() {
                collect_dangling_required(value, &format!("{path}[{idx}]"), errors);
            }
        }
        _ => {}
    }
}

#[tokio::test]
async fn test_context_guard_never_spends_more_than_it_reports() {
    // State-space sweep over budget, fill level, and payload size. Two
    // invariants must hold in every combination, because the whole point of the
    // guard is that a caller can trust the accounting:
    //   1. Without the opt-in, the returned text is small. Refusing has to be
    //      cheap or it reproduces the bug it prevents.
    //   2. The returned text never exceeds the remaining safety headroom, with
    //      or without the opt-in. Otherwise "accept the cost" would silently
    //      overrun the window.
    for budget in [10_000usize, 50_000, 200_000] {
        for fill_percent in [0usize, 25, 50, 80, 89, 95] {
            for payload_tokens in [1usize, 500, 5_000, 100_000] {
                for accept in [false, true] {
                    let compaction =
                        Arc::new(RwLock::new(CompactionManager::new().with_budget(budget)));
                    let used = budget * fill_percent / 100;
                    if used > 0 {
                        let mut mgr = compaction.write().await;
                        mgr.update_observed_input_tokens(used as u64);
                    }
                    let registry = Registry {
                        tools: Arc::new(RwLock::new(HashMap::new())),
                        skills: Arc::new(RwLock::new(crate::skill::SkillRegistry::default())),
                        compaction,
                    };

                    let payload = "x".repeat(payload_tokens * 4);
                    let result = registry
                        .guard_context_overflow("test", ToolOutput::new(payload.clone()), accept)
                        .await;
                    let returned_tokens = result.output.len() / 4;

                    let threshold = (budget as f32 * 0.90) as usize;
                    let headroom = threshold.saturating_sub(used);
                    let passed_through = result.output == payload;

                    if !accept && !passed_through {
                        assert!(
                            result.output.len() < 1_500,
                            "refusal must stay cheap: budget={budget} fill={fill_percent} \
                             payload={payload_tokens} returned {} chars",
                            result.output.len()
                        );
                    }

                    // Allow a small slack for the notice text appended after the slice.
                    assert!(
                        returned_tokens <= headroom.max(1_000) + 500,
                        "returned ~{returned_tokens}k tokens with only {headroom} headroom: \
                         budget={budget} fill={fill_percent} payload={payload_tokens} \
                         accept={accept}"
                    );
                }
            }
        }
    }
}

#[tokio::test]
async fn test_context_guard_refusal_reads_clearly_for_todays_regression() {
    // The exact shape that motivated this change: a 233k-token agentgrep result
    // against a 200k budget with 18k already used. Printed so the wording stays
    // reviewable, and asserted so it keeps naming the cost and the escape hatch.
    let compaction = Arc::new(RwLock::new(CompactionManager::new().with_budget(200_000)));
    {
        let mut mgr = compaction.write().await;
        mgr.update_observed_input_tokens(18_000);
    }
    let registry = Registry {
        tools: Arc::new(RwLock::new(HashMap::new())),
        skills: Arc::new(RwLock::new(crate::skill::SkillRegistry::default())),
        compaction,
    };

    let result = registry
        .guard_context_overflow("agentgrep", ToolOutput::new("x".repeat(932_000)), false)
        .await;
    println!("---\n{}\n---", result.output);

    assert!(result.output.contains("233k tokens"));
    assert!(result.output.contains("116%"), "got: {}", result.output);
    assert!(result.output.contains("18k is already used"));
    assert!(result.output.contains("accept_large_output"));
    assert!(result.output.contains("paths_only"));
}

/// Tool that returns a fixed-size payload, for exercising the guard through the
/// real `execute()` path rather than by calling the guard directly.
struct BigOutputTool {
    chars: usize,
}

#[async_trait]
impl Tool for BigOutputTool {
    fn name(&self) -> &str {
        "big_output"
    }

    fn description(&self) -> &str {
        "Returns a large fixed payload for context guard tests."
    }

    fn parameters_schema(&self) -> Value {
        serde_json::json!({ "type": "object", "properties": {} })
    }

    async fn execute(&self, _input: Value, _ctx: ToolContext) -> Result<ToolOutput> {
        Ok(ToolOutput::new("x".repeat(self.chars)))
    }
}

async fn execute_big_output(input: Value) -> String {
    let provider: Arc<dyn Provider> = Arc::new(MockProvider);
    let registry = Registry::new(provider).await;
    {
        let mut mgr = registry.compaction.write().await;
        *mgr = CompactionManager::new().with_budget(10_000);
    }
    registry
        .register(
            "big_output".to_string(),
            Arc::new(BigOutputTool { chars: 400_000 }),
        )
        .await;

    let ctx = ToolContext {
        session_id: "test-context-guard-execute".to_string(),
        message_id: "test".to_string(),
        tool_call_id: "test".to_string(),
        working_dir: Some(std::env::temp_dir()),
        stdin_request_tx: None,
        graceful_shutdown_signal: None,
        execution_mode: ToolExecutionMode::Direct,
    };

    registry
        .execute("big_output", input, ctx)
        .await
        .expect("tool should succeed")
        .output
}

#[tokio::test]
async fn test_execute_withholds_oversized_output_by_default() {
    // The guard is only useful if it runs on the real call path. Every other
    // test calls guard_context_overflow directly, which would still pass if the
    // flag were never plumbed through execute().
    let output = execute_big_output(serde_json::json!({ "intent": "test" })).await;
    assert!(
        output.contains("OUTPUT WITHHELD"),
        "execute() must apply the guard, got: {output}"
    );
    assert!(
        output.len() < 1_500,
        "withheld output should be cheap, got {} chars",
        output.len()
    );
}

#[tokio::test]
async fn test_execute_honors_accept_large_output_from_raw_input() {
    // Proves the flag survives the trip through execute(): the tool itself never
    // declares or reads `accept_large_output`, so this only works because the
    // registry reads it off the raw input.
    let output =
        execute_big_output(serde_json::json!({ "intent": "test", "accept_large_output": true }))
            .await;
    assert!(
        output.contains("OUTPUT TRUNCATED"),
        "opt-in should return truncated payload, got: {}",
        &output[..output.len().min(200)]
    );
    assert!(
        output.starts_with(&"x".repeat(200)),
        "opt-in must actually return payload"
    );
}

#[tokio::test]
async fn test_execute_ignores_a_non_boolean_accept_flag() {
    // A truthy-looking value must not spend the window.
    let output =
        execute_big_output(serde_json::json!({ "intent": "test", "accept_large_output": 1 })).await;
    assert!(
        output.contains("OUTPUT WITHHELD"),
        "numeric 1 must not opt in, got: {}",
        &output[..output.len().min(200)]
    );
}

#[tokio::test]
async fn test_every_tool_advertises_the_large_output_escape_hatch() {
    // The guard applies to every tool, so every tool must document the way out.
    // Asserted over the real definition list rather than per tool, because the
    // failure mode is a new tool nobody remembered to annotate.
    let provider: Arc<dyn Provider> = Arc::new(MockProvider);
    let registry = Registry::new(provider).await;
    registry.register_ambient_tools().await;

    let defs = registry.definitions(None).await;
    assert!(
        defs.len() > 20,
        "expected the full tool set, got {}",
        defs.len()
    );

    let mut missing = Vec::new();
    for def in &defs {
        let flag = &def.input_schema["properties"][jcode_tool_core::ACCEPT_LARGE_OUTPUT_KEY];
        if flag.get("type").and_then(Value::as_str) != Some("boolean") {
            missing.push(def.name.clone());
        }
        // Advertising it as required would force the model to answer a question
        // about token budgets on every single call.
        if let Some(required) = def.input_schema["required"].as_array() {
            assert!(
                !required
                    .iter()
                    .any(|v| v.as_str() == Some(jcode_tool_core::ACCEPT_LARGE_OUTPUT_KEY)),
                "{} must not require accept_large_output",
                def.name
            );
        }
    }
    assert!(
        missing.is_empty(),
        "tools missing the accept_large_output escape hatch: {missing:?}"
    );
}

#[tokio::test]
async fn test_large_output_flag_costs_little_across_the_whole_tool_set() {
    // Adding a property to every schema is paid on every request, forever. Keep
    // the total honest: ~20 tokens per tool is acceptable, a paragraph is not.
    let provider: Arc<dyn Provider> = Arc::new(MockProvider);
    let registry = Registry::new(provider).await;
    registry.register_ambient_tools().await;
    let defs = registry.definitions(None).await;

    let property =
        serde_json::to_string(&crate::tool::accept_large_output_schema_property_for_test())
            .expect("serializable");
    let per_tool = crate::util::estimate_tokens(&property);
    let total = per_tool * defs.len();

    assert!(
        per_tool <= 25,
        "per-tool cost {per_tool} tokens is too high: {property}"
    );
    assert!(
        total < 1_500,
        "{} tools x {per_tool} tokens = {total} tokens of permanent prompt overhead",
        defs.len()
    );
}

#[tokio::test]
async fn test_batch_guards_both_its_subcalls_and_its_own_aggregate() {
    // Batch is how oversized results actually arrive in practice: several
    // searches fan out at once. Two separate guard applications matter here, and
    // the aggregate one is the load-bearing case: batch concatenates every
    // sub-result, so even if each sub-call were individually acceptable the
    // combined output can blow the window. That aggregate is what withheld
    // today's regression.
    let provider: Arc<dyn Provider> = Arc::new(MockProvider);
    let registry = Registry::new(provider).await;
    {
        let mut mgr = registry.compaction.write().await;
        *mgr = CompactionManager::new().with_budget(10_000);
    }
    registry
        .register(
            "big_output".to_string(),
            Arc::new(BigOutputTool { chars: 400_000 }),
        )
        .await;

    let ctx = |name: &str| ToolContext {
        session_id: format!("test-batch-context-guard-{name}"),
        message_id: "test".to_string(),
        tool_call_id: "test".to_string(),
        working_dir: Some(std::env::temp_dir()),
        stdin_request_tx: None,
        graceful_shutdown_signal: None,
        execution_mode: ToolExecutionMode::Direct,
    };
    let calls = serde_json::json!([
        { "tool": "big_output", "intent": "one" },
        { "tool": "big_output", "intent": "two" },
    ]);

    // Without an opt-in anywhere, nothing large escapes: no payload reaches the
    // transcript, only the refusal.
    let withheld = registry
        .execute(
            "batch",
            serde_json::json!({ "intent": "test", "tool_calls": calls }),
            ctx("withheld"),
        )
        .await
        .expect("batch should succeed")
        .output;
    assert!(
        withheld.contains("OUTPUT WITHHELD"),
        "batch output must be guarded, got: {}",
        &withheld[..withheld.len().min(300)]
    );
    assert!(
        !withheld.contains(&"x".repeat(100)),
        "no payload should survive when nothing opted in"
    );

    // Opting in at the batch level returns the aggregate, which is the level a
    // caller reads. The sub-calls' own refusals are inside it, since each was
    // guarded separately and neither sub-call opted in.
    let accepted = registry
        .execute(
            "batch",
            serde_json::json!({
                "intent": "test",
                "accept_large_output": true,
                "tool_calls": calls,
            }),
            ctx("accepted"),
        )
        .await
        .expect("batch should succeed")
        .output;
    // The aggregate is now returned rather than withheld: it carries the
    // per-subcall section headers, which the withheld version never reaches.
    assert!(
        !accepted.starts_with("⚠️ OUTPUT WITHHELD"),
        "batch-level opt-in should return the aggregate, got: {}",
        &accepted[..accepted.len().min(200)]
    );
    assert!(
        accepted.contains("--- [1] big_output ---"),
        "aggregate should contain per-subcall sections, got: {}",
        &accepted[..accepted.len().min(300)]
    );
    assert!(
        accepted.matches("OUTPUT WITHHELD").count() >= 1,
        "each sub-call is guarded on its own; neither opted in"
    );
}

#[tokio::test]
async fn test_guard_withholds_large_output_on_a_million_token_window() {
    // The regression that made every other test in this file misleading. They
    // all pinned budgets of 1k to 200k, where 30% of the budget is a small
    // number. Production reported a 1M-token window, so 30% permitted a 300k
    // single result and a repo-wide grep costing 233k tokens sailed straight
    // through a guard that had unit tests passing.
    let compaction = Arc::new(RwLock::new(CompactionManager::new().with_budget(1_000_000)));
    {
        let mut mgr = compaction.write().await;
        mgr.update_observed_input_tokens(21_000);
    }
    let registry = Registry {
        tools: Arc::new(RwLock::new(HashMap::new())),
        skills: Arc::new(RwLock::new(crate::skill::SkillRegistry::default())),
        compaction,
    };

    // ~233k tokens: the real size of the agentgrep result that started this.
    let output = ToolOutput::new("x".repeat(932_000));
    let result = registry
        .guard_context_overflow("agentgrep", output, false)
        .await;

    assert!(
        result.output.contains("OUTPUT WITHHELD"),
        "a 233k-token result must be withheld even on a 1M window, got: {}",
        &result.output[..result.output.len().min(200)]
    );
    assert!(
        result.output.len() < 1_500,
        "refusal should cost ~120 tokens, not {} chars",
        result.output.len()
    );
}

#[tokio::test]
async fn test_single_output_ceiling_is_absolute_not_only_proportional() {
    // Guards the invariant directly: however large the window, one tool result
    // may never exceed the absolute ceiling. Without this, raising a model's
    // advertised context window silently raises the per-call blast radius.
    for budget in [200_000usize, 1_000_000, 2_000_000, 10_000_000] {
        let compaction = Arc::new(RwLock::new(CompactionManager::new().with_budget(budget)));
        let registry = Registry {
            tools: Arc::new(RwLock::new(HashMap::new())),
            skills: Arc::new(RwLock::new(crate::skill::SkillRegistry::default())),
            compaction,
        };

        // Just over the absolute ceiling, but a trivial fraction of a huge window.
        let over_ceiling_tokens = Registry::SINGLE_OUTPUT_MAX_TOKENS + 10_000;
        let result = registry
            .guard_context_overflow(
                "test",
                ToolOutput::new("x".repeat(over_ceiling_tokens * 4)),
                false,
            )
            .await;
        assert!(
            result.output.contains("OUTPUT WITHHELD"),
            "budget={budget}: {over_ceiling_tokens} tokens must exceed the absolute ceiling"
        );
    }
}

/// Every built-in tool, normalized for every provider dialect, must be
/// sendable.
///
/// This is the guard the recurring schema-outage class never had. #446, #495,
/// #543, #655, #687, #713 and #754 were each discovered by a user whose
/// provider had gone down, then fixed by appending one keyword to one
/// provider's deny-list. Nothing checked the *other* providers for the same
/// construct, which is exactly how #754 hit Gemini through Antigravity months
/// after the same class was fixed for OpenAI.
///
/// Running the real registry through every registered dialect turns "some
/// provider is about to break" into a failing test on the commit that
/// introduces it.
#[tokio::test]
async fn tool_schemas_are_sendable_to_every_provider_dialect() {
    let provider: Arc<dyn Provider> = Arc::new(MockProvider);
    let registry = Registry::new(provider).await;
    let defs = registry.definitions(None).await;
    assert!(!defs.is_empty(), "the sweep must not pass vacuously");

    let mut failures = Vec::new();
    // Not per-dialect: no provider *rejects* a property that declares no type,
    // but OpenAI refuses `strict` for the whole catalog over one (#713), so a
    // built-in tool acquiring one would silently cost every OpenAI-route agent
    // its structured-output guarantees.
    for def in &defs {
        for error in jcode_schema_dialect::untyped_properties(&def.input_schema) {
            failures.push(format!("tool `{}` {error}", def.name));
        }
    }
    for spec in jcode_schema_dialect::registry::ALL {
        for def in &defs {
            let normalized = jcode_schema_dialect::dialect::apply(&def.input_schema, spec);
            for error in
                jcode_schema_dialect::must_not_contain_unsupported_constructs(&normalized, spec)
            {
                failures.push(format!("[{}] tool `{}` {error}", spec.id, def.name));
            }
            // Over-stripping is the hazard an allow-list introduces: a dialect
            // that forgot to list `description` would produce requests that
            // succeed while silently deleting every tool's prompt text.
            for error in jcode_schema_dialect::must_preserve_meaning(&def.input_schema, &normalized)
            {
                failures.push(format!(
                    "[{}] tool `{}` lost meaning: {error}",
                    spec.id, def.name
                ));
            }
        }
    }

    assert!(
        failures.is_empty(),
        "tool schemas are not sendable to every provider:\n{}",
        failures.join("\n")
    );
}

/// The sweep above must fail when a tool really does carry a construct a
/// provider rejects, otherwise it is decorative. Feeds the exact
/// `@playwright/mcp` schema from #754 through the same checker to prove the
/// detection works end to end.
#[test]
fn the_dialect_sweep_catches_the_issue_754_schema() {
    let hostile = serde_json::json!({
        "type": "object",
        "properties": {
            "data": {
                "type": "object",
                "additionalProperties": { "type": "string" },
                "propertyNames": { "type": "string" }
            }
        }
    });

    let unnormalized = jcode_schema_dialect::must_not_contain_unsupported_constructs(
        &hostile,
        &jcode_schema_dialect::registry::GEMINI,
    );
    assert!(
        unnormalized
            .iter()
            .any(|e| e.message.contains("propertyNames")),
        "the checker must flag the raw schema, got {unnormalized:?}"
    );

    let normalized =
        jcode_schema_dialect::dialect::apply(&hostile, &jcode_schema_dialect::registry::GEMINI);
    assert!(
        jcode_schema_dialect::must_not_contain_unsupported_constructs(
            &normalized,
            &jcode_schema_dialect::registry::GEMINI,
        )
        .is_empty(),
        "and must pass once normalized"
    );
}

/// Failing strict eligibility closed for #711/#713 must not quietly cost jcode's
/// own tools their strict mode, since that would drop the structured-output
/// guarantees on every OpenAI-route tool call with nothing to notice.
///
/// The four tools listed below were already non-strict before that change, for
/// reasons unrelated to it (`batch` declares `additionalProperties: true` so its
/// sub-call payloads stay open-world; the others carry open maps or untyped
/// action payloads). Pinning the exact set is what makes this a regression
/// detector: a fifth name appearing means a stricter rule went too far, and a
/// name disappearing means a tool became strict-eligible and the list is stale.
#[tokio::test]
async fn only_the_known_open_world_tools_are_ineligible_for_openai_strict_mode() {
    /// Built-ins that legitimately cannot be strict. Verified against master
    /// before the #711/#713 eligibility changes, so this is pre-existing.
    const KNOWN_OPEN_WORLD_TOOLS: &[&str] = &["batch", "browser", "initiative", "swarm"];

    let provider: Arc<dyn Provider> = Arc::new(MockProvider);
    let registry = Registry::new(provider).await;
    let defs = registry.definitions(None).await;
    assert!(!defs.is_empty(), "the sweep must not pass vacuously");

    let mut ineligible: Vec<String> = Vec::new();
    for def in &defs {
        let compatible =
            jcode_provider_core::openai_schema::openai_compatible_schema(&def.input_schema);
        if !jcode_provider_core::openai_schema::schema_supports_strict(&compatible) {
            ineligible.push(def.name.clone());
        }
    }
    ineligible.sort();

    let expected: Vec<String> = KNOWN_OPEN_WORLD_TOOLS
        .iter()
        .map(ToString::to_string)
        .collect();
    assert_eq!(
        ineligible, expected,
        "the set of strict-ineligible built-in tools changed; a new name means an \
         eligibility rule is too aggressive, a missing name means this list is stale"
    );
}
