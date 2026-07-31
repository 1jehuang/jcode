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
    // sponsors.enabled defaults to false; the discovery tool must not exist.
    let provider: Arc<dyn Provider> = Arc::new(MockProvider);
    let registry = Registry::new(provider).await;
    let names = registry.tool_names().await;
    if crate::config::config().sponsors.enabled {
        assert!(names.iter().any(|n| n == "discover_tools"));
    } else {
        assert!(
            !names.iter().any(|n| n == "discover_tools"),
            "discover_tools must not be registered when sponsors are disabled"
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
    // discover_tools keeps a deliberate second sentence disclosing that catalog
    // entries are vetted/partnered integrations.
    // swarm appends the user-tunable swarm-prompt.md by design.
    const EXEMPT: &[&str] = &["discover_tools", "swarm"];

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
    let result = registry.guard_context_overflow("test", output).await;
    assert_eq!(result.output, "small output");
}

#[tokio::test]
async fn test_context_guard_truncates_huge_single_output() {
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
    let result = registry.guard_context_overflow("test", output).await;
    assert!(
        result.output.len() < big_output.len(),
        "Output should be truncated"
    );
    assert!(
        result.output.contains("TRUNCATED"),
        "Should contain truncation warning"
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
    let result = registry.guard_context_overflow("test", output).await;
    assert!(
        result.output.contains("TRUNCATED") || result.output.contains("CONTEXT LIMIT"),
        "Should warn about context limits when nearly full"
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
    let result = registry.guard_context_overflow("test", output).await;
    assert_eq!(
        result.output.len(),
        100_000,
        "Zero budget should pass through"
    );
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
