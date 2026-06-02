use super::*;
use crate::message::{ContentBlock, Message, Role};
use crate::provider::{EventStream, Provider};
// `tool::Registry` lives in the upper jcode-app-core layer; reached here via the
// dev-dependency on jcode-app-core (legal Cargo dev-dep cycle, lib build unaffected).
use async_trait::async_trait;
use jcode_app_core::tool::Registry;
use std::sync::Arc;

struct EnvVarGuard {
    key: &'static str,
    previous: Option<std::ffi::OsString>,
}

impl EnvVarGuard {
    fn set_path(key: &'static str, value: &std::path::Path) -> Self {
        let previous = std::env::var_os(key);
        crate::env::set_var(key, value);
        Self { key, previous }
    }
}

impl Drop for EnvVarGuard {
    fn drop(&mut self) {
        if let Some(previous) = &self.previous {
            crate::env::set_var(self.key, previous);
        } else {
            crate::env::remove_var(self.key);
        }
    }
}

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
            "Mock provider should not be used for streaming completions in Gemini tests"
        ))
    }

    fn name(&self) -> &str {
        "mock"
    }

    fn fork(&self) -> Arc<dyn Provider> {
        Arc::new(MockProvider)
    }
}

#[test]
fn available_models_include_gemini_defaults() {
    let provider = GeminiProvider::new();
    let models = provider.available_models();
    assert!(models.contains(&"gemini-3-pro-preview"));
    assert!(models.contains(&"gemini-3.1-pro-preview"));
    assert!(models.contains(&"gemini-2.5-pro"));
    assert!(models.contains(&"gemini-2.5-flash"));
}

#[test]
fn set_model_accepts_gemini_models() {
    let provider = GeminiProvider::new();
    provider.set_model("gemini-2.5-flash").unwrap();
    assert_eq!(provider.model(), "gemini-2.5-flash");
}

#[test]
fn detects_model_not_found_errors() {
    let err = anyhow::anyhow!(
        "Gemini request generateContent failed (HTTP 404 Not Found): {{\"error\":{{\"status\":\"NOT_FOUND\",\"message\":\"Requested entity was not found.\"}}}}"
    );
    assert!(is_gemini_model_not_found_error(&err));
}

#[test]
fn fallback_models_skip_current_model() {
    assert_eq!(
        gemini_fallback_models("gemini-2.5-flash"),
        vec![
            "gemini-3.1-pro-preview",
            "gemini-3-pro-preview",
            "gemini-2.5-pro",
            "gemini-3-flash-preview",
            "gemini-2.0-flash",
        ]
    );
}

#[test]
fn extract_gemini_model_ids_discovers_nested_models() {
    let response = json!({
        "routing": {
            "manual": {
                "models": [
                    {"id": "gemini-3-pro-preview"},
                    {"name": "gemini-3.1-pro-preview"}
                ]
            },
            "auto": ["gemini-3-flash-preview", "not-a-model"]
        }
    });

    assert_eq!(
        extract_gemini_model_ids(&response),
        vec![
            "gemini-3.1-pro-preview".to_string(),
            "gemini-3-pro-preview".to_string(),
            "gemini-3-flash-preview".to_string(),
        ]
    );
}

#[test]
fn available_models_display_prefers_discovered_models_and_current_model() {
    let provider = GeminiProvider::new();
    provider.set_model("gemini-4-pro-preview").unwrap();
    *provider.fetched_models.write().unwrap() = vec![
        "gemini-3-flash-preview".to_string(),
        "gemini-3-pro-preview".to_string(),
    ];

    assert_eq!(
        provider.available_models_display(),
        vec![
            "gemini-3-pro-preview".to_string(),
            "gemini-3-flash-preview".to_string(),
            "gemini-4-pro-preview".to_string(),
        ]
    );
}

#[test]
fn available_models_display_without_discovery_uses_current_model_only() {
    let _guard = crate::storage::lock_test_env();
    let temp = tempfile::TempDir::new().expect("tempdir");
    let _home = EnvVarGuard::set_path("JCODE_HOME", temp.path());

    let provider = GeminiProvider::new();
    provider.set_model("gemini-4-pro-preview").unwrap();

    assert_eq!(
        provider.available_models_display(),
        vec!["gemini-4-pro-preview".to_string()]
    );
}

#[test]
fn available_models_display_seeds_from_persisted_catalog() {
    let _guard = crate::storage::lock_test_env();
    let temp = tempfile::TempDir::new().expect("tempdir");
    let _home = EnvVarGuard::set_path("JCODE_HOME", temp.path());

    let path = GeminiProvider::persisted_catalog_path().expect("catalog path");
    crate::storage::write_json(
        &path,
        &PersistedCatalog {
            models: vec!["gemini-3-pro-preview".to_string()],
            fetched_at_rfc3339: chrono::Utc::now().to_rfc3339(),
        },
    )
    .expect("write persisted catalog");

    let provider = GeminiProvider::new();
    assert!(
        provider
            .available_models_display()
            .contains(&"gemini-3-pro-preview".to_string())
    );
}

#[test]
fn build_contents_preserves_tool_calls_and_results() {
    let messages = vec![
        Message {
            role: Role::Assistant,
            content: vec![ContentBlock::ToolUse {
                id: "call_1".to_string(),
                name: "read".to_string(),
                input: json!({"path":"README.md"}),
            }],
            timestamp: None,
            tool_duration_ms: None,
        },
        Message {
            role: Role::User,
            content: vec![ContentBlock::ToolResult {
                tool_use_id: "call_1".to_string(),
                content: "ok".to_string(),
                is_error: None,
            }],
            timestamp: None,
            tool_duration_ms: None,
        },
    ];

    let contents = build_contents(&messages);
    assert_eq!(contents.len(), 2);
    assert_eq!(contents[0].role, "model");
    assert_eq!(contents[1].role, "user");
    assert_eq!(
        contents[0].parts[0].function_call.as_ref().unwrap().name,
        "read"
    );
    assert_eq!(
        contents[1].parts[0]
            .function_response
            .as_ref()
            .unwrap()
            .name,
        "read"
    );
}

#[test]
fn build_contents_normalizes_non_object_tool_call_args_for_gemini_struct() {
    let messages = vec![Message {
        role: Role::Assistant,
        content: vec![ContentBlock::ToolUse {
            id: "call_primitive".to_string(),
            name: "read".to_string(),
            input: json!(20),
        }],
        timestamp: None,
        tool_duration_ms: None,
    }];

    let contents = build_contents(&messages);
    assert_eq!(
        contents[0].parts[0].function_call.as_ref().unwrap().args,
        json!({})
    );
}

#[test]
fn build_tools_uses_function_declarations() {
    let defs = vec![ToolDefinition {
        name: "read".to_string(),
        description: "Read a file".to_string(),
        input_schema: json!({"type":"object","properties":{"path":{"type":"string"}}}),
    }];

    let built = build_tools(&defs).unwrap();
    assert_eq!(built.len(), 1);
    assert_eq!(built[0].function_declarations[0].name, "read");
}

fn schema_contains_key(schema: &Value, key: &str) -> bool {
    match schema {
        Value::Object(map) => {
            map.contains_key(key) || map.values().any(|value| schema_contains_key(value, key))
        }
        Value::Array(items) => items.iter().any(|value| schema_contains_key(value, key)),
        _ => false,
    }
}

#[test]
fn build_tools_rewrites_const_for_gemini_schema_compatibility() {
    let defs = vec![ToolDefinition {
        name: "batch".to_string(),
        description: "Batch tools".to_string(),
        input_schema: json!({
            "type": "object",
            "properties": {
                "tool_calls": {
                    "type": "array",
                    "items": {
                        "oneOf": [
                            {
                                "type": "object",
                                "properties": {
                                    "tool": { "type": "string", "const": "read" },
                                    "file_path": { "type": "string" }
                                },
                                "required": ["tool", "file_path"]
                            }
                        ]
                    }
                }
            }
        }),
    }];

    let built = build_tools(&defs).expect("gemini tools");
    let parameters = &built[0].function_declarations[0].parameters;

    assert!(!schema_contains_key(parameters, "const"));
    assert_eq!(
        parameters["properties"]["tool_calls"]["items"]["oneOf"][0]["properties"]["tool"]["enum"],
        json!(["read"])
    );
}

#[tokio::test]
async fn build_tools_from_registry_definitions_omits_const_keywords() {
    let provider: Arc<dyn Provider> = Arc::new(MockProvider);
    let registry = Registry::new(provider).await;
    let defs = registry.definitions(None).await;

    let built = build_tools(&defs).expect("gemini tools");
    let parameters = &built[0].function_declarations;

    assert!(!schema_contains_key(&json!(parameters), "const"));
}

#[test]
fn parses_prompt_feedback_block_reason() {
    let response: VertexGenerateContentResponse = serde_json::from_value(json!({
        "promptFeedback": {
            "blockReason": "PROHIBITED_CONTENT",
            "blockReasonMessage": "Prompt violated policy"
        }
    }))
    .expect("parse prompt feedback");

    let feedback = response.prompt_feedback.expect("missing prompt feedback");
    assert_eq!(feedback.block_reason.as_deref(), Some("PROHIBITED_CONTENT"));
    assert_eq!(
        feedback.block_reason_message.as_deref(),
        Some("Prompt violated policy")
    );
}

#[test]
fn parses_candidate_finish_message() {
    let response: VertexGenerateContentResponse = serde_json::from_value(json!({
        "candidates": [
            {
                "finishReason": "SAFETY",
                "finishMessage": "Response blocked by safety filters"
            }
        ]
    }))
    .expect("parse candidate");

    let candidate = response
        .candidates
        .expect("missing candidates")
        .into_iter()
        .next()
        .expect("missing first candidate");
    assert_eq!(candidate.finish_reason.as_deref(), Some("SAFETY"));
    assert_eq!(
        candidate.finish_message.as_deref(),
        Some("Response blocked by safety filters")
    );
}

// ---------------------------------------------------------------------------
// Regression tests for issue #126 / upstream PR #162: Gemini's generateContent
// rejects standard JSON-Schema metadata. MCP servers (notion, supabase, …)
// emit schemas with $defs/$ref/$schema, which would otherwise crash the call.
// `gemini_compatible_schema` must inline $ref targets and strip the metadata.
// ---------------------------------------------------------------------------

#[test]
fn gemini_schema_inlines_refs_and_strips_metadata() {
    let schema = serde_json::json!({
        "$schema": "https://json-schema.org/draft-07",
        "$id": "https://example.com/foo.json",
        "title": "Foo",
        "type": "object",
        "$defs": {
            "Inner": {
                "type": "object",
                "properties": {
                    "kind": { "const": "leaf" }
                }
            }
        },
        "properties": {
            "inner": { "$ref": "#/$defs/Inner" }
        }
    });

    let out = gemini_compatible_schema(&schema);
    let obj = out.as_object().expect("object root");

    // Metadata keywords are stripped.
    assert!(!obj.contains_key("$schema"), "$schema should be stripped");
    assert!(!obj.contains_key("$id"), "$id should be stripped");
    assert!(!obj.contains_key("title"), "title should be stripped");
    assert!(
        !obj.contains_key("$defs"),
        "$defs should be stripped from root"
    );

    // $ref is inlined — `inner` should now look like the Inner def.
    let inner = obj
        .get("properties")
        .and_then(|p| p.get("inner"))
        .and_then(|v| v.as_object())
        .expect("inner object");
    assert_eq!(inner.get("type").and_then(|v| v.as_str()), Some("object"));
    // const → enum conversion still happens after inlining.
    let kind = inner
        .get("properties")
        .and_then(|p| p.get("kind"))
        .and_then(|v| v.as_object())
        .expect("kind object");
    let enum_arr = kind.get("enum").and_then(|v| v.as_array()).expect("enum");
    assert_eq!(enum_arr.len(), 1);
    assert_eq!(enum_arr[0].as_str(), Some("leaf"));
}

#[test]
fn gemini_schema_resolves_definitions_alias_too() {
    // Older drafts use `definitions` instead of `$defs`. Both must work.
    let schema = serde_json::json!({
        "type": "object",
        "definitions": {
            "Bar": { "type": "string" }
        },
        "properties": {
            "bar": { "$ref": "#/definitions/Bar" }
        }
    });

    let out = gemini_compatible_schema(&schema);
    let obj = out.as_object().expect("object root");
    assert!(!obj.contains_key("definitions"));
    let bar = obj
        .get("properties")
        .and_then(|p| p.get("bar"))
        .and_then(|v| v.as_object())
        .expect("bar object");
    assert_eq!(bar.get("type").and_then(|v| v.as_str()), Some("string"));
}

#[test]
fn gemini_schema_falls_back_to_empty_object_on_unresolved_ref() {
    // Unresolvable $ref must produce a permissive empty object, not a crash
    // and not a forwarded $ref Gemini would reject.
    let schema = serde_json::json!({
        "type": "object",
        "properties": {
            "ghost": { "$ref": "#/$defs/DoesNotExist" }
        }
    });
    let out = gemini_compatible_schema(&schema);
    let ghost = out
        .get("properties")
        .and_then(|p| p.get("ghost"))
        .expect("ghost");
    let ghost_obj = ghost.as_object().expect("ghost object");
    assert!(
        ghost_obj.is_empty(),
        "unresolved $ref should become an empty object, got {ghost:?}"
    );
}

#[test]
fn gemini_schema_does_not_recurse_forever_on_cycles() {
    // A self-referential schema must terminate (returns an empty object once
    // depth limit is exceeded) instead of overflowing the stack.
    let schema = serde_json::json!({
        "type": "object",
        "$defs": {
            "Loop": {
                "type": "object",
                "properties": {
                    "next": { "$ref": "#/$defs/Loop" }
                }
            }
        },
        "properties": {
            "head": { "$ref": "#/$defs/Loop" }
        }
    });
    // Should not panic / overflow.
    let _ = gemini_compatible_schema(&schema);
}
