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

    fn set_value(key: &'static str, value: &str) -> Self {
        let previous = std::env::var_os(key);
        crate::env::set_var(key, value);
        Self { key, previous }
    }

    fn unset(key: &'static str) -> Self {
        let previous = std::env::var_os(key);
        crate::env::remove_var(key);
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
fn build_contents_replays_thought_signature_on_function_call() {
    // Gemini 3 (Antigravity Cloud Code backend) rejects function calls that
    // omit the original thoughtSignature on later turns. Verify the signature
    // captured on the ToolUse block is replayed verbatim on the functionCall
    // part, and that an absent/empty signature stays absent.
    let messages = vec![
        Message {
            role: Role::Assistant,
            content: vec![ContentBlock::ToolUse {
                id: "call_sig".to_string(),
                name: "read".to_string(),
                input: json!({"path":"README.md"}),
                thought_signature: Some("SIGNATURE_ABC".to_string()),
            }],
            timestamp: None,
            tool_duration_ms: None,
        },
        Message {
            role: Role::Assistant,
            content: vec![ContentBlock::ToolUse {
                id: "call_nosig".to_string(),
                name: "bash".to_string(),
                input: json!({"command":"ls"}),
                thought_signature: None,
            }],
            timestamp: None,
            tool_duration_ms: None,
        },
    ];

    let contents = build_contents(&messages);
    assert_eq!(
        contents[0].parts[0].thought_signature.as_deref(),
        Some("SIGNATURE_ABC"),
        "signature must be replayed on the matching function call part"
    );
    assert_eq!(
        contents[1].parts[0].thought_signature, None,
        "missing signature must not be fabricated"
    );
}

#[test]
fn build_contents_replays_every_signature_across_multi_tool_history() {
    // Regression guard for the Antigravity/Cloud Code 400
    // ("Function call is missing a thought_signature ... position 5"): the
    // backend validates *every* functionCall in the replayed history, not just
    // the latest one. A multi-turn transcript where an earlier tool_use drops
    // its signature is exactly what triggers the field failure, so assert that
    // each captured signature survives serialization onto its matching part.
    let signatures = ["SIG_A", "SIG_B", "SIG_C"];
    let mut messages = Vec::new();
    for (idx, sig) in signatures.iter().enumerate() {
        messages.push(Message {
            role: Role::Assistant,
            content: vec![ContentBlock::ToolUse {
                id: format!("call_{idx}"),
                name: "bash".to_string(),
                input: json!({ "command": format!("echo {idx}") }),
                thought_signature: Some(sig.to_string()),
            }],
            timestamp: None,
            tool_duration_ms: None,
        });
        messages.push(Message {
            role: Role::User,
            content: vec![ContentBlock::ToolResult {
                tool_use_id: format!("call_{idx}"),
                content: format!("out {idx}"),
                is_error: Some(false),
            }],
            timestamp: None,
            tool_duration_ms: None,
        });
    }

    let contents = build_contents(&messages);
    let replayed: Vec<Option<&str>> = contents
        .iter()
        .flat_map(|content| content.parts.iter())
        .filter(|part| part.function_call.is_some())
        .map(|part| part.thought_signature.as_deref())
        .collect();
    assert_eq!(
        replayed,
        vec![Some("SIG_A"), Some("SIG_B"), Some("SIG_C")],
        "every functionCall in the history must carry its captured thought_signature, \
         not just the most recent one"
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
                thought_signature: None,
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
            thought_signature: None,
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

#[test]
fn build_tools_strips_additional_properties_for_gemini_schema_compatibility() {
    // The Gemini Code Assist generateContent endpoint rejects `additionalProperties`
    // (and other draft-JSON-Schema keywords) with HTTP 400, so build_tools must
    // strip them recursively while preserving the rest of the schema.
    let defs = vec![ToolDefinition {
        name: "read".to_string(),
        description: "Reads a file".to_string(),
        input_schema: json!({
            "type": "object",
            "$schema": "http://json-schema.org/draft-07/schema#",
            "properties": {
                "file_path": { "type": "string" },
                "opts": {
                    "type": "object",
                    "properties": { "limit": { "type": "integer" } },
                    "additionalProperties": false
                }
            },
            "required": ["file_path"],
            "additionalProperties": false
        }),
    }];

    let built = build_tools(&defs).expect("gemini tools");
    let parameters = &built[0].function_declarations[0].parameters;

    assert!(!schema_contains_key(parameters, "additionalProperties"));
    assert!(!schema_contains_key(parameters, "$schema"));
    // Real schema content is preserved.
    assert_eq!(
        parameters["properties"]["file_path"]["type"],
        json!("string")
    );
    assert_eq!(
        parameters["properties"]["opts"]["properties"]["limit"]["type"],
        json!("integer")
    );
    assert_eq!(parameters["required"], json!(["file_path"]));
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

#[test]
fn auth_mode_prefers_api_key_when_present() {
    let _guard = crate::storage::lock_test_env();
    let temp = tempfile::TempDir::new().expect("tempdir");
    let _home = EnvVarGuard::set_path("JCODE_HOME", temp.path());
    let _google = EnvVarGuard::unset("GOOGLE_API_KEY");
    let _force = EnvVarGuard::unset("JCODE_GEMINI_FORCE_OAUTH");
    let _key = EnvVarGuard::set_value("GEMINI_API_KEY", "test-developer-key");

    match GeminiProvider::auth_mode() {
        GeminiAuthMode::ApiKey(key) => assert_eq!(key, "test-developer-key"),
        GeminiAuthMode::Oauth => panic!("expected API-key auth mode when GEMINI_API_KEY is set"),
    }
}

#[test]
fn auth_mode_force_oauth_overrides_api_key() {
    let _guard = crate::storage::lock_test_env();
    let temp = tempfile::TempDir::new().expect("tempdir");
    let _home = EnvVarGuard::set_path("JCODE_HOME", temp.path());
    let _google = EnvVarGuard::unset("GOOGLE_API_KEY");
    let _key = EnvVarGuard::set_value("GEMINI_API_KEY", "test-developer-key");
    let _force = EnvVarGuard::set_value("JCODE_GEMINI_FORCE_OAUTH", "1");

    assert!(matches!(GeminiProvider::auth_mode(), GeminiAuthMode::Oauth));
}

#[test]
fn auth_mode_defaults_to_oauth_without_api_key() {
    let _guard = crate::storage::lock_test_env();
    let temp = tempfile::TempDir::new().expect("tempdir");
    let _home = EnvVarGuard::set_path("JCODE_HOME", temp.path());
    let _key = EnvVarGuard::unset("GEMINI_API_KEY");
    let _google = EnvVarGuard::unset("GOOGLE_API_KEY");
    let _force = EnvVarGuard::unset("JCODE_GEMINI_FORCE_OAUTH");

    assert!(matches!(GeminiProvider::auth_mode(), GeminiAuthMode::Oauth));
}

#[test]
fn developer_api_base_url_defaults_to_generativelanguage() {
    let _guard = crate::storage::lock_test_env();
    let _endpoint = EnvVarGuard::unset("GEMINI_API_ENDPOINT");
    let _version = EnvVarGuard::unset("GEMINI_API_VERSION");

    assert_eq!(
        GeminiProvider::developer_api_base_url(),
        "https://generativelanguage.googleapis.com/v1beta"
    );
}

#[test]
fn developer_api_base_url_honors_env_overrides() {
    let _guard = crate::storage::lock_test_env();
    let _endpoint = EnvVarGuard::set_value("GEMINI_API_ENDPOINT", "https://example.test/");
    let _version = EnvVarGuard::set_value("GEMINI_API_VERSION", "/v9/");

    assert_eq!(
        GeminiProvider::developer_api_base_url(),
        "https://example.test/v9"
    );
}

#[test]
fn developer_api_response_parses_without_code_assist_envelope() {
    // The Developer API returns the bare generateContent body; ensure it maps
    // onto the same response type the Code Assist envelope yields.
    let response: VertexGenerateContentResponse = serde_json::from_value(json!({
        "candidates": [
            {
                "content": {
                    "role": "model",
                    "parts": [{ "text": "hello from developer api" }]
                },
                "finishReason": "STOP"
            }
        ],
        "usageMetadata": {
            "promptTokenCount": 3,
            "candidatesTokenCount": 5
        }
    }))
    .expect("parse developer api response");

    let candidate = response
        .candidates
        .expect("missing candidates")
        .into_iter()
        .next()
        .expect("missing first candidate");
    assert_eq!(candidate.finish_reason.as_deref(), Some("STOP"));
    let text = candidate
        .content
        .expect("missing content")
        .parts
        .into_iter()
        .next()
        .and_then(|part| part.text)
        .expect("missing text");
    assert_eq!(text, "hello from developer api");
}

#[test]
fn system_instruction_tool_guard_only_applies_with_tools() {
    // Without tools, the system instruction is passed through unchanged.
    let plain = super::build_system_instruction_with_tool_guard("You are helpful.", false)
        .expect("system instruction present");
    let plain_text = plain.parts[0].text.clone().unwrap();
    assert_eq!(plain_text, "You are helpful.");
    assert!(!plain_text.contains("Function calling"));

    // With tools, the MALFORMED_FUNCTION_CALL prevention guidance is appended.
    let guarded = super::build_system_instruction_with_tool_guard("You are helpful.", true)
        .expect("system instruction present");
    let guarded_text = guarded.parts[0].text.clone().unwrap();
    assert!(guarded_text.starts_with("You are helpful."));
    assert!(guarded_text.contains("Function calling"));
    assert!(guarded_text.contains("native function call, not code"));
    assert!(guarded_text.contains("default_api."));
}

#[test]
fn system_instruction_tool_guard_with_empty_system_still_emits_guidance() {
    // An empty base system prompt plus tools must still carry the guard so the
    // model is steered away from pseudo-code tool calls.
    let guarded = super::build_system_instruction_with_tool_guard("", true)
        .expect("guard-only instruction present");
    let text = guarded.parts[0].text.clone().unwrap();
    assert!(text.contains("Function calling"));

    // Empty system and no tools yields no instruction at all.
    assert!(super::build_system_instruction_with_tool_guard("", false).is_none());
}
