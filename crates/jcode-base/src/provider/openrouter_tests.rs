use super::*;
use bytes::Bytes;
use futures::StreamExt;
use jcode_provider_openrouter::stream::OpenRouterStream;
use std::ffi::OsString;
use std::io::{Read, Write};
use std::net::TcpListener;
use std::sync::Mutex;
use std::sync::mpsc;
use std::time::Duration;
use tempfile::TempDir;

struct SharedEnvLock;

static ENV_LOCK: SharedEnvLock = SharedEnvLock;

impl SharedEnvLock {
    /// Acquire the process-global test env lock.
    ///
    /// This recovers from a poisoned mutex (`into_inner`) instead of
    /// propagating the `PoisonError`. The env guard only protects shared
    /// process env state, so a panic in one test must not cascade into a
    /// flood of unrelated `PoisonError` failures across every other test
    /// that takes this lock.
    fn lock(&self) -> std::sync::MutexGuard<'static, ()> {
        crate::storage::lock_test_env()
    }
}

struct EnvVarGuard {
    key: &'static str,
    previous: Option<OsString>,
}

impl EnvVarGuard {
    fn set(key: &'static str, value: impl AsRef<std::ffi::OsStr>) -> Self {
        let previous = std::env::var_os(key);
        crate::env::set_var(key, value);
        Self { key, previous }
    }

    fn remove(key: &'static str) -> Self {
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

fn test_config_dir(temp: &TempDir) -> std::path::PathBuf {
    #[cfg(target_os = "macos")]
    {
        temp.path().join("Library").join("Application Support")
    }
    #[cfg(target_os = "windows")]
    {
        temp.path().join("AppData").join("Roaming")
    }
    #[cfg(all(not(target_os = "macos"), not(target_os = "windows")))]
    {
        temp.path().to_path_buf()
    }
}

fn write_test_api_key(temp: &TempDir, env_file: &str, env_key: &str, value: &str) {
    let config_dir = test_config_dir(temp).join("jcode");
    std::fs::create_dir_all(&config_dir).expect("create test config dir");
    std::fs::write(config_dir.join(env_file), format!("{env_key}={value}\n"))
        .expect("write test api key");
}

fn isolate_openrouter_autodetect_env() -> Vec<EnvVarGuard> {
    let mut guards = vec![
        EnvVarGuard::remove("JCODE_OPENROUTER_API_BASE"),
        EnvVarGuard::remove("JCODE_OPENROUTER_API_KEY_NAME"),
        EnvVarGuard::remove("JCODE_OPENROUTER_ENV_FILE"),
        EnvVarGuard::remove("JCODE_OPENROUTER_DYNAMIC_BEARER_PROVIDER"),
        EnvVarGuard::remove("JCODE_OPENROUTER_MODEL"),
        EnvVarGuard::remove("JCODE_OPENROUTER_CACHE_NAMESPACE"),
        EnvVarGuard::remove("JCODE_OPENROUTER_ALLOW_NO_AUTH"),
        EnvVarGuard::remove("JCODE_OPENROUTER_TRANSPORT_STATE"),
        EnvVarGuard::remove("JCODE_OPENROUTER_PROVIDER_FEATURES"),
        EnvVarGuard::remove("JCODE_OPENROUTER_MODEL_CATALOG"),
        EnvVarGuard::remove("JCODE_OPENROUTER_AUTH_HEADER"),
        EnvVarGuard::remove("JCODE_OPENROUTER_AUTH_HEADER_NAME"),
        EnvVarGuard::remove("JCODE_OPENROUTER_STATIC_MODELS"),
        EnvVarGuard::remove("JCODE_ACTIVE_PROVIDER"),
        EnvVarGuard::remove("JCODE_RUNTIME_PROVIDER"),
        EnvVarGuard::remove("JCODE_NAMED_PROVIDER_PROFILE"),
        EnvVarGuard::remove("JCODE_PROVIDER_PROFILE_NAME"),
        EnvVarGuard::remove("JCODE_PROVIDER_PROFILE_ACTIVE"),
        EnvVarGuard::remove("JCODE_OPENAI_COMPAT_API_BASE"),
        EnvVarGuard::remove("JCODE_OPENAI_COMPAT_API_KEY_NAME"),
        EnvVarGuard::remove("JCODE_OPENAI_COMPAT_ENV_FILE"),
        EnvVarGuard::remove("JCODE_OPENAI_COMPAT_SETUP_URL"),
        EnvVarGuard::remove("JCODE_OPENAI_COMPAT_DEFAULT_MODEL"),
        EnvVarGuard::remove("JCODE_OPENAI_COMPAT_LOCAL_ENABLED"),
    ];
    guards.extend(
        crate::provider_catalog::openai_compatible_profiles()
            .iter()
            .map(|profile| EnvVarGuard::remove(profile.api_key_env)),
    );
    guards
}

#[test]
fn test_has_credentials() {
    let _has_creds = OpenRouterProvider::has_credentials();
}

#[test]
fn openai_compatible_models_endpoint_allows_minimal_model_objects() {
    let parsed = parse_openai_compatible_models_response(
        r#"{
            "object": "list",
            "data": [
                {"id": "glm-51-nvfp4", "object": "model", "created": null, "owned_by": null},
                {"id": "gte-qwen2-7b", "object": "model"}
            ]
        }"#,
    )
    .expect("minimal OpenAI-compatible /models response should parse");

    assert_eq!(parsed.len(), 2);
    assert_eq!(parsed[0].id, "glm-51-nvfp4");
    assert_eq!(parsed[0].name, "");
}

#[test]
fn openai_compatible_models_endpoint_allows_chutes_numeric_pricing() {
    let parsed = parse_openai_compatible_models_response(
        r#"{
            "object": "list",
            "data": [{
                "id": "Qwen/Qwen3-32B-TEE",
                "root": "Qwen/Qwen3-32B-FP8",
                "price": {
                    "input": {"tao": 0.0002439746644509701, "usd": 0.08},
                    "output": {"tao": 0.0007319239933529102, "usd": 0.24}
                },
                "object": "model",
                "parent": null,
                "created": 1778439139,
                "pricing": {
                    "prompt": 0.08,
                    "completion": 0.24,
                    "input_cache_read": 0.04
                },
                "owned_by": "sglang",
                "context_length": 40960,
                "supported_features": ["json_mode", "tools"]
            }]
        }"#,
    )
    .expect("Chutes /models response with numeric pricing should parse");

    assert_eq!(parsed.len(), 1);
    assert_eq!(parsed[0].id, "Qwen/Qwen3-32B-TEE");
    assert_eq!(parsed[0].pricing.prompt.as_deref(), Some("0.08"));
    assert_eq!(parsed[0].pricing.completion.as_deref(), Some("0.24"));
    assert_eq!(parsed[0].pricing.input_cache_read.as_deref(), Some("0.04"));
}

#[test]
fn openai_compatible_models_endpoint_allows_together_top_level_array() {
    let parsed = parse_openai_compatible_models_response(
        r#"[
            {
                "id": "Austism/chronos-hermes-13b",
                "object": "model",
                "created": 1692896905,
                "type": "chat",
                "display_name": "Chronos Hermes (13B)",
                "context_length": 2048,
                "pricing": {
                    "input": 0.3,
                    "output": 0.3,
                    "cached_input": 0.2
                }
            }
        ]"#,
    )
    .expect("Together /models top-level array should parse");

    assert_eq!(parsed.len(), 1);
    assert_eq!(parsed[0].id, "Austism/chronos-hermes-13b");
    assert_eq!(parsed[0].name, "Chronos Hermes (13B)");
    assert_eq!(parsed[0].context_length, Some(2048));
    assert_eq!(parsed[0].pricing.prompt.as_deref(), Some("0.3"));
    assert_eq!(parsed[0].pricing.completion.as_deref(), Some("0.3"));
    assert_eq!(parsed[0].pricing.input_cache_read.as_deref(), Some("0.2"));
}

#[test]
fn openai_compatible_models_endpoint_allows_models_array_with_name_ids() {
    let parsed = parse_openai_compatible_models_response(
        r#"{
            "models": [{
                "name": "accounts/fireworks/models/example",
                "displayName": "Example Fireworks Model",
                "contextLength": 8192
            }]
        }"#,
    )
    .expect("models array with name-based identifiers should parse");

    assert_eq!(parsed.len(), 1);
    assert_eq!(parsed[0].id, "accounts/fireworks/models/example");
    assert_eq!(parsed[0].name, "accounts/fireworks/models/example");
    assert_eq!(parsed[0].context_length, Some(8192));
}

#[test]
fn named_openai_compatible_provider_sets_catalog_cache_namespace() {
    let _lock = ENV_LOCK.lock();
    let _namespace = EnvVarGuard::remove("JCODE_OPENROUTER_CACHE_NAMESPACE");
    let _key = EnvVarGuard::set("TEST_NAMED_COMPAT_KEY", "test-key");

    let profile = crate::config::NamedProviderConfig {
        base_url: "https://llm.example.com/v1".to_string(),
        api_key_env: Some("TEST_NAMED_COMPAT_KEY".to_string()),
        model_catalog: true,
        default_model: Some("example-model".to_string()),
        ..Default::default()
    };

    let _provider = OpenRouterProvider::new_named_openai_compatible("example-compat", &profile)
        .expect("named profile should initialize");

    assert_eq!(
        std::env::var("JCODE_OPENROUTER_CACHE_NAMESPACE").as_deref(),
        Ok("example-compat")
    );
}

#[test]
fn named_openai_compatible_provider_exposes_static_models_as_routes() {
    let _lock = ENV_LOCK.lock();
    let _namespace = EnvVarGuard::remove("JCODE_OPENROUTER_CACHE_NAMESPACE");
    let _key = EnvVarGuard::set("TEST_NAMED_COMPAT_KEY", "test-key");

    let profile = crate::config::NamedProviderConfig {
        base_url: "https://llm.example.com/v1".to_string(),
        api_key_env: Some("TEST_NAMED_COMPAT_KEY".to_string()),
        model_catalog: true,
        default_model: Some("glm-51-nvfp4".to_string()),
        models: vec![crate::config::NamedProviderModelConfig {
            id: "glm-51-nvfp4".to_string(),
            ..Default::default()
        }],
        ..Default::default()
    };

    let provider = OpenRouterProvider::new_named_openai_compatible("comtegra-test", &profile)
        .expect("named profile should initialize");
    let routes = provider.model_routes();

    assert!(routes.iter().any(|route| {
        route.model == "glm-51-nvfp4"
            && route.api_method == "openai-compatible:comtegra-test"
            && route.available
    }));
}

#[test]
fn direct_openai_compatible_provider_advertises_image_input_support() {
    let _lock = ENV_LOCK.lock();
    let _namespace = EnvVarGuard::remove("JCODE_OPENROUTER_CACHE_NAMESPACE");

    let profile = crate::config::NamedProviderConfig {
        base_url: "http://localhost:1234/v1".to_string(),
        auth: crate::config::NamedProviderAuth::None,
        default_model: Some("local-vision-model".to_string()),
        ..Default::default()
    };

    let provider = OpenRouterProvider::new_named_openai_compatible("local-compat", &profile)
        .expect("local named profile should initialize without auth");

    assert!(provider.supports_image_input());
}

#[test]
fn direct_deepseek_profile_does_not_advertise_image_input_support() {
    let provider = OpenRouterProvider {
        profile_id: Some("deepseek".to_string()),
        supports_provider_features: false,
        ..make_custom_compatible_provider()
    };

    assert!(!provider.supports_image_input());
}

#[test]
fn direct_deepseek_profile_omits_image_url_parts() {
    let _lock = ENV_LOCK.lock();
    let (api_base, request_rx) = spawn_single_response_chat_server();
    let provider = OpenRouterProvider {
        api_base,
        profile_id: Some("deepseek".to_string()),
        supports_provider_features: false,
        supports_model_catalog: false,
        ..make_custom_compatible_provider()
    };
    let messages = vec![Message {
        role: Role::User,
        content: vec![
            ContentBlock::Text {
                text: "describe this".to_string(),
                cache_control: None,
            },
            ContentBlock::Image {
                media_type: "image/png".to_string(),
                data: "aW1hZ2U=".to_string(),
            },
        ],
        timestamp: None,
        tool_duration_ms: None,
    }];

    let rt = tokio::runtime::Builder::new_current_thread()
        .enable_all()
        .build()
        .expect("runtime");
    rt.block_on(async {
        let mut stream = provider
            .complete(&messages, &[], "", None)
            .await
            .expect("fake chat request should start");
        while let Some(event) = stream.next().await {
            if event.is_err() {
                break;
            }
        }
    });

    let request = request_rx
        .recv_timeout(Duration::from_secs(2))
        .expect("capture fake provider request");
    assert!(
        !request.contains(r#""type":"image_url""#),
        "DeepSeek request must not contain unsupported image_url content parts: {request}"
    );
    assert!(
        request.contains("Image omitted"),
        "DeepSeek request should preserve a textual placeholder for omitted images: {request}"
    );
}

/// Extract the JSON request body from a captured raw HTTP request.
fn parse_captured_request_body(request: &str) -> serde_json::Value {
    let body = request
        .split_once("\r\n\r\n")
        .map(|(_, body)| body)
        .unwrap_or(request);
    serde_json::from_str(body)
        .unwrap_or_else(|err| panic!("captured request body should be JSON ({err}): {body}"))
}

/// Regression for issue #321: when an assistant turn is interrupted mid-thinking
/// on a direct OpenAI-compatible provider that does not support reasoning replay
/// (e.g. DeepSeek), the persisted assistant message contains only a `Reasoning`
/// block. The request builder must not emit an assistant message that has
/// neither `content` nor `tool_calls`, otherwise the provider rejects the whole
/// request with 400 "Invalid assistant message: content or tool_calls must be
/// set" and the session can never recover.
#[test]
fn interrupted_reasoning_only_assistant_message_is_not_sent_empty() {
    let _lock = ENV_LOCK.lock();
    let (api_base, request_rx) = spawn_single_response_chat_server();
    let provider = OpenRouterProvider {
        api_base,
        profile_id: Some("deepseek".to_string()),
        supports_provider_features: false,
        supports_model_catalog: false,
        ..make_custom_compatible_provider()
    };

    let messages = vec![
        Message {
            role: Role::User,
            content: vec![ContentBlock::Text {
                text: "do a thing".to_string(),
                cache_control: None,
            }],
            timestamp: None,
            tool_duration_ms: None,
        },
        // Assistant turn that was interrupted while only reasoning had streamed,
        // so it carries a Reasoning block but no text or tool calls.
        Message {
            role: Role::Assistant,
            content: vec![ContentBlock::Reasoning {
                text: "thinking about the request".to_string(),
            }],
            timestamp: None,
            tool_duration_ms: None,
        },
        Message {
            role: Role::User,
            content: vec![ContentBlock::Text {
                text: "actually do this instead".to_string(),
                cache_control: None,
            }],
            timestamp: None,
            tool_duration_ms: None,
        },
    ];

    let rt = tokio::runtime::Builder::new_current_thread()
        .enable_all()
        .build()
        .expect("runtime");
    rt.block_on(async {
        let mut stream = provider
            .complete(&messages, &[], "", None)
            .await
            .expect("fake chat request should start");
        while let Some(event) = stream.next().await {
            if event.is_err() {
                break;
            }
        }
    });

    let request = request_rx
        .recv_timeout(Duration::from_secs(2))
        .expect("capture fake provider request");
    let body = parse_captured_request_body(&request);
    let api_messages = body
        .get("messages")
        .and_then(|m| m.as_array())
        .expect("request should contain messages array");

    for msg in api_messages {
        if msg.get("role").and_then(|v| v.as_str()) != Some("assistant") {
            continue;
        }
        let has_content = msg
            .get("content")
            .map(|v| !v.is_null() && v.as_str().map(|s| !s.is_empty()).unwrap_or(true))
            .unwrap_or(false);
        let has_tool_calls = msg
            .get("tool_calls")
            .and_then(|v| v.as_array())
            .map(|calls| !calls.is_empty())
            .unwrap_or(false);
        assert!(
            has_content || has_tool_calls,
            "assistant message must carry content or tool_calls (issue #321); got: {msg}"
        );
    }
}

/// Companion to issue #321: when the provider *does* support reasoning replay
/// (e.g. a generic OpenRouter-style endpoint with provider features enabled and
/// thinking on), an interrupted reasoning-only assistant turn should be sent
/// with both a `reasoning_content` field and a valid (empty) `content`, so the
/// turn is preserved without violating the "content or tool_calls" requirement.
#[test]
fn interrupted_reasoning_only_assistant_message_keeps_reasoning_with_content() {
    let _lock = ENV_LOCK.lock();
    let (api_base, request_rx) = spawn_single_response_chat_server();
    let provider = OpenRouterProvider {
        api_base,
        profile_id: None,
        supports_provider_features: true,
        supports_model_catalog: false,
        ..make_custom_compatible_provider()
    };

    let messages = vec![
        Message {
            role: Role::User,
            content: vec![ContentBlock::Text {
                text: "do a thing".to_string(),
                cache_control: None,
            }],
            timestamp: None,
            tool_duration_ms: None,
        },
        Message {
            role: Role::Assistant,
            content: vec![ContentBlock::Reasoning {
                text: "thinking about the request".to_string(),
            }],
            timestamp: None,
            tool_duration_ms: None,
        },
        Message {
            role: Role::User,
            content: vec![ContentBlock::Text {
                text: "actually do this instead".to_string(),
                cache_control: None,
            }],
            timestamp: None,
            tool_duration_ms: None,
        },
    ];

    let rt = tokio::runtime::Builder::new_current_thread()
        .enable_all()
        .build()
        .expect("runtime");
    rt.block_on(async {
        let mut stream = provider
            .complete(&messages, &[], "", None)
            .await
            .expect("fake chat request should start");
        while let Some(event) = stream.next().await {
            if event.is_err() {
                break;
            }
        }
    });

    let request = request_rx
        .recv_timeout(Duration::from_secs(2))
        .expect("capture fake provider request");
    let body = parse_captured_request_body(&request);
    let api_messages = body
        .get("messages")
        .and_then(|m| m.as_array())
        .expect("request should contain messages array");

    let assistant = api_messages
        .iter()
        .find(|msg| msg.get("role").and_then(|v| v.as_str()) == Some("assistant"))
        .expect("request should retain the interrupted assistant turn");

    assert!(
        assistant.get("reasoning_content").is_some(),
        "reasoning-capable provider should keep reasoning_content; got: {assistant}"
    );
    assert!(
        assistant.get("content").is_some(),
        "interrupted reasoning-only assistant turn must still carry content (issue #321); got: {assistant}"
    );
}

/// Regression for issue #322: the dedicated Kimi coding endpoint
/// (`https://api.kimi.com/coding/v1`, model `kimi-for-coding`) enables thinking
/// server-side and rejects any assistant tool-call message that lacks
/// `reasoning_content` with 400 "thinking is enabled but reasoning_content is
/// missing in assistant tool call message". When an assistant turn produced a
/// tool call without an accompanying reasoning block (the common case once the
/// thinking stream is not persisted), the request builder must still attach a
/// `reasoning_content` field to that assistant message so the endpoint accepts
/// the request.
#[test]
fn kimi_for_coding_tool_call_message_includes_reasoning_content() {
    let _lock = ENV_LOCK.lock();
    let _thinking = EnvVarGuard::remove("JCODE_OPENROUTER_THINKING");
    let (api_base, request_rx) = spawn_single_response_chat_server();
    let provider = OpenRouterProvider {
        api_base,
        // The dedicated Kimi coding endpoint is a direct OpenAI-compatible
        // profile (no OpenRouter provider routing features).
        profile_id: Some("kimi".to_string()),
        supports_provider_features: false,
        supports_model_catalog: false,
        model: Arc::new(RwLock::new("kimi-for-coding".to_string())),
        ..make_custom_compatible_provider()
    };

    let messages = vec![
        Message {
            role: Role::User,
            content: vec![ContentBlock::Text {
                text: "list the files".to_string(),
                cache_control: None,
            }],
            timestamp: None,
            tool_duration_ms: None,
        },
        // Assistant turn that emitted a tool call but whose hidden reasoning was
        // not persisted (so there is no Reasoning block to replay).
        Message {
            role: Role::Assistant,
            content: vec![ContentBlock::ToolUse {
                id: "call_1".to_string(),
                name: "bash".to_string(),
                input: serde_json::json!({"command": "ls"}),
                thought_signature: None,
            }],
            timestamp: None,
            tool_duration_ms: None,
        },
        Message {
            role: Role::User,
            content: vec![ContentBlock::ToolResult {
                tool_use_id: "call_1".to_string(),
                content: "a.txt\nb.txt".to_string(),
                is_error: None,
            }],
            timestamp: None,
            tool_duration_ms: None,
        },
    ];

    let rt = tokio::runtime::Builder::new_current_thread()
        .enable_all()
        .build()
        .expect("runtime");
    rt.block_on(async {
        let mut stream = provider
            .complete(&messages, &[], "", None)
            .await
            .expect("fake chat request should start");
        while let Some(event) = stream.next().await {
            if event.is_err() {
                break;
            }
        }
    });

    let request = request_rx
        .recv_timeout(Duration::from_secs(2))
        .expect("capture fake provider request");
    let body = parse_captured_request_body(&request);
    let api_messages = body
        .get("messages")
        .and_then(|m| m.as_array())
        .expect("request should contain messages array");

    let assistant = api_messages
        .iter()
        .find(|msg| {
            msg.get("role").and_then(|v| v.as_str()) == Some("assistant")
                && msg.get("tool_calls").is_some()
        })
        .expect("request should retain the assistant tool-call turn");

    let reasoning = assistant.get("reasoning_content");
    assert!(
        reasoning.is_some_and(|value| value.as_str().is_some_and(|s| !s.is_empty())),
        "Kimi coding endpoint requires reasoning_content on assistant tool-call messages (issue #322); got: {assistant}"
    );
}

#[test]
fn default_named_openai_compatible_provider_uses_direct_compatible_request_path() {
    let _lock = ENV_LOCK.lock();
    let temp = TempDir::new().expect("create temp home");
    let jcode_home = temp.path().join("jcode-home");
    let _jcode_home = EnvVarGuard::set("JCODE_HOME", &jcode_home);
    let _home = EnvVarGuard::set("HOME", temp.path());
    let _appdata = EnvVarGuard::set("APPDATA", temp.path().join("AppData").join("Roaming"));
    let _env = isolate_openrouter_autodetect_env();
    let _key = EnvVarGuard::set("TEST_DEFAULT_COMPAT_KEY", "test-key");
    let (api_base, request_rx) = spawn_single_response_chat_server();

    std::fs::create_dir_all(&jcode_home).expect("create test config dir");
    std::fs::write(
        jcode_home.join("config.toml"),
        format!(
            r#"
[provider]
default_provider = "my-gateway"

[providers.my-gateway]
type = "openai-compatible"
base_url = "{api_base}"
api_key_env = "TEST_DEFAULT_COMPAT_KEY"
default_model = "opaque/model@id"
model_catalog = false
"#
        ),
    )
    .expect("write test config");
    crate::config::invalidate_config_cache();

    let provider =
        crate::provider::MultiProvider::new_with_auth_status(crate::auth::AuthStatus::default());
    assert_eq!(
        provider.active_provider(),
        crate::provider::ActiveProvider::OpenRouter
    );
    let openrouter = provider
        .openrouter_provider()
        .expect("openrouter execution slot");
    assert!(
        !openrouter.supports_provider_routing_features(),
        "named openai-compatible defaults must not use OpenRouter provider routing features"
    );
    assert_eq!(
        openrouter
            .direct_openai_compatible_route_parts()
            .as_ref()
            .map(|parts| parts.1.as_str()),
        Some("openai-compatible:my-gateway")
    );

    let messages = vec![Message {
        role: Role::User,
        content: vec![ContentBlock::Text {
            text: "hello".to_string(),
            cache_control: None,
        }],
        timestamp: None,
        tool_duration_ms: None,
    }];

    let rt = tokio::runtime::Builder::new_current_thread()
        .enable_all()
        .build()
        .expect("runtime");
    rt.block_on(async {
        let mut stream = openrouter
            .complete(&messages, &[], "", None)
            .await
            .expect("fake chat request should start");
        while let Some(event) = stream.next().await {
            event.expect("stream event should parse");
        }
    });

    let request = request_rx
        .recv_timeout(Duration::from_secs(2))
        .expect("capture fake provider request");
    assert!(
        request.starts_with("POST /v1/chat/completions "),
        "unexpected chat request: {request}"
    );
    assert!(
        request.contains(r#""model":"opaque/model@id""#),
        "request should use named profile default model: {request}"
    );
    assert!(
        !request.contains(r#""provider":"#),
        "direct OpenAI-compatible request must not include OpenRouter provider routing object: {request}"
    );
    assert!(
        !request.contains("HTTP-Referer") && !request.contains("X-Title"),
        "direct OpenAI-compatible request must not include OpenRouter-only headers: {request}"
    );
}

/// Regression for issue #304: a `default_provider` pointing at an
/// `openai-compatible` profile must build requests with the direct
/// OpenAI-compatible request shape, NOT the OpenRouter request builder, even
/// when `model_catalog` is left enabled (the default). Using the OpenRouter
/// builder leaks the `provider` routing object / OpenRouter-only headers and
/// causes strict third-party gateways to reject the request with
/// 400 "Unrecognized chat message".
#[test]
fn default_named_openai_compatible_with_catalog_uses_direct_compatible_request_path() {
    let _lock = ENV_LOCK.lock();
    let temp = TempDir::new().expect("create temp home");
    let jcode_home = temp.path().join("jcode-home");
    let _jcode_home = EnvVarGuard::set("JCODE_HOME", &jcode_home);
    let _home = EnvVarGuard::set("HOME", temp.path());
    let _appdata = EnvVarGuard::set("APPDATA", temp.path().join("AppData").join("Roaming"));
    let _env = isolate_openrouter_autodetect_env();
    let _key = EnvVarGuard::set("TEST_DEFAULT_COMPAT_KEY", "test-key");
    let (api_base, request_rx) = spawn_single_response_chat_server();

    std::fs::create_dir_all(&jcode_home).expect("create test config dir");
    std::fs::write(
        jcode_home.join("config.toml"),
        format!(
            r#"
[provider]
default_provider = "my-gateway"

[providers.my-gateway]
type = "openai-compatible"
base_url = "{api_base}"
api_key_env = "TEST_DEFAULT_COMPAT_KEY"
default_model = "opaque/model@id"
"#
        ),
    )
    .expect("write test config");
    crate::config::invalidate_config_cache();

    let provider =
        crate::provider::MultiProvider::new_with_auth_status(crate::auth::AuthStatus::default());
    assert_eq!(
        provider.active_provider(),
        crate::provider::ActiveProvider::OpenRouter
    );
    let openrouter = provider
        .openrouter_provider()
        .expect("openrouter execution slot");
    assert!(
        !openrouter.supports_provider_routing_features(),
        "named openai-compatible defaults must not use OpenRouter provider routing features even with catalog enabled"
    );

    let messages = vec![Message {
        role: Role::User,
        content: vec![ContentBlock::Text {
            text: "hello".to_string(),
            cache_control: None,
        }],
        timestamp: None,
        tool_duration_ms: None,
    }];

    let rt = tokio::runtime::Builder::new_current_thread()
        .enable_all()
        .build()
        .expect("runtime");
    rt.block_on(async {
        let mut stream = openrouter
            .complete(&messages, &[], "", None)
            .await
            .expect("fake chat request should start");
        while let Some(event) = stream.next().await {
            event.expect("stream event should parse");
        }
    });

    let request = request_rx
        .recv_timeout(Duration::from_secs(2))
        .expect("capture fake provider request");
    assert!(
        request.starts_with("POST /v1/chat/completions "),
        "unexpected chat request: {request}"
    );
    assert!(
        !request.contains(r#""provider":"#),
        "direct OpenAI-compatible request must not include OpenRouter provider routing object: {request}"
    );
    assert!(
        !request.contains("HTTP-Referer") && !request.contains("X-Title"),
        "direct OpenAI-compatible request must not include OpenRouter-only headers: {request}"
    );
}

#[test]
fn minimax_profile_exposes_static_models_before_catalog_refresh() {
    let models = crate::provider_catalog::openai_compatible_profile_static_models(
        jcode_provider_metadata::MINIMAX_PROFILE,
    );
    assert!(models.iter().any(|model| model == "MiniMax-M2.7"));
    assert!(models.iter().any(|model| model == "MiniMax-M2.7-highspeed"));
    assert!(models.iter().any(|model| model == "MiniMax-M2"));
}

#[test]
fn cerebras_profile_exposes_live_chat_models_before_catalog_refresh() {
    assert_eq!(
        jcode_provider_metadata::CEREBRAS_PROFILE.default_model,
        Some("gpt-oss-120b")
    );

    let models = crate::provider_catalog::openai_compatible_profile_static_models(
        jcode_provider_metadata::CEREBRAS_PROFILE,
    );

    assert!(
        !models.iter().any(|model| model == "qwen-3-coder-480b"),
        "old Cerebras default is no longer returned by the live /models catalog"
    );
    assert!(models.iter().any(|model| model == "gpt-oss-120b"));
    assert!(models.iter().any(|model| model == "zai-glm-4.7"));
    assert!(
        !models
            .iter()
            .any(|model| model == "qwen-3-235b-a22b-instruct-2507")
    );
    assert!(!models.iter().any(|model| model == "llama3.1-8b"));
}

#[test]
fn openai_compatible_profiles_with_unverified_live_catalogs_have_static_fallbacks() {
    let cases = [
        (jcode_provider_metadata::OPENCODE_PROFILE, "minimax-m2.7"),
        (jcode_provider_metadata::OPENCODE_GO_PROFILE, "kimi-k2.5"),
        (jcode_provider_metadata::ZAI_PROFILE, "glm-4.7"),
        (
            jcode_provider_metadata::AI302_PROFILE,
            "qwen3-235b-a22b-instruct-2507",
        ),
        (jcode_provider_metadata::BASETEN_PROFILE, "zai-org/GLM-4.7"),
        (jcode_provider_metadata::CORTECS_PROFILE, "kimi-k2.5"),
        (jcode_provider_metadata::KIMI_PROFILE, "kimi-for-coding"),
        (jcode_provider_metadata::FIRMWARE_PROFILE, "kimi-k2.5"),
        (
            jcode_provider_metadata::HUGGING_FACE_PROFILE,
            "Qwen/Qwen3-Coder-480B-A35B-Instruct",
        ),
        (jcode_provider_metadata::MOONSHOT_PROFILE, "kimi-k2.5"),
        (
            jcode_provider_metadata::NEBIUS_PROFILE,
            "openai/gpt-oss-120b",
        ),
        (
            jcode_provider_metadata::SCALEWAY_PROFILE,
            "qwen3-coder-30b-a3b-instruct",
        ),
        (
            jcode_provider_metadata::STACKIT_PROFILE,
            "openai/gpt-oss-120b",
        ),
        (jcode_provider_metadata::PERPLEXITY_PROFILE, "sonar"),
        (
            jcode_provider_metadata::DEEPINFRA_PROFILE,
            "moonshotai/Kimi-K2-Instruct",
        ),
        (
            jcode_provider_metadata::FIREWORKS_PROFILE,
            "accounts/fireworks/routers/kimi-k2p5-turbo",
        ),
        (jcode_provider_metadata::XIAOMI_MIMO_PROFILE, "mimo-v2.5"),
        (
            jcode_provider_metadata::ALIBABA_CODING_PLAN_PROFILE,
            "qwen3-coder-plus",
        ),
    ];

    for (profile, expected_model) in cases {
        let models = crate::provider_catalog::openai_compatible_profile_static_models(profile);
        assert!(
            models.iter().any(|model| model == expected_model),
            "{} should expose static fallback model {expected_model}; got {models:?}",
            profile.id
        );
    }
}

#[test]
fn comtegra_profile_uses_endpoint_default_max_tokens() {
    let _lock = ENV_LOCK.lock();
    let _override = EnvVarGuard::remove("JCODE_OPENROUTER_MAX_TOKENS");

    assert_eq!(
        OpenRouterProvider::configured_max_tokens(Some("comtegra")),
        None
    );
    assert_eq!(
        OpenRouterProvider::configured_max_tokens(Some("deepseek")),
        None
    );
}

#[test]
fn max_tokens_env_overrides_profile_default() {
    let _lock = ENV_LOCK.lock();
    let _override = EnvVarGuard::set("JCODE_OPENROUTER_MAX_TOKENS", "4096");

    assert_eq!(
        OpenRouterProvider::configured_max_tokens(Some("comtegra")),
        Some(4096)
    );
}

#[test]
fn test_configured_api_base_accepts_https() {
    let _lock = ENV_LOCK.lock();
    let prev = std::env::var("JCODE_OPENROUTER_API_BASE").ok();
    crate::env::set_var(
        "JCODE_OPENROUTER_API_BASE",
        "https://api.groq.com/openai/v1/",
    );
    assert_eq!(configured_api_base(), "https://api.groq.com/openai/v1");
    if let Some(value) = prev {
        crate::env::set_var("JCODE_OPENROUTER_API_BASE", value);
    } else {
        crate::env::remove_var("JCODE_OPENROUTER_API_BASE");
    }
}

#[test]
fn test_configured_api_base_rejects_insecure_http_remote() {
    let _lock = ENV_LOCK.lock();
    let prev = std::env::var("JCODE_OPENROUTER_API_BASE").ok();
    crate::env::set_var("JCODE_OPENROUTER_API_BASE", "http://example.com/v1");
    assert_eq!(configured_api_base(), DEFAULT_API_BASE);
    if let Some(value) = prev {
        crate::env::set_var("JCODE_OPENROUTER_API_BASE", value);
    } else {
        crate::env::remove_var("JCODE_OPENROUTER_API_BASE");
    }
}

#[test]
fn autodetects_single_saved_openai_compatible_profile() {
    let _lock = ENV_LOCK.lock();
    let temp = TempDir::new().expect("create temp dir");
    let _xdg = EnvVarGuard::set("XDG_CONFIG_HOME", temp.path());
    let _home = EnvVarGuard::set("HOME", temp.path());
    let _appdata = EnvVarGuard::set("APPDATA", temp.path().join("AppData").join("Roaming"));
    let _env = isolate_openrouter_autodetect_env();

    let opencode = crate::provider_catalog::resolve_openai_compatible_profile(
        crate::provider_catalog::OPENCODE_PROFILE,
    );
    write_test_api_key(
        &temp,
        &opencode.env_file,
        &opencode.api_key_env,
        "test-opencode-key",
    );

    assert_eq!(configured_api_base(), opencode.api_base);
    assert_eq!(configured_api_key_name(), opencode.api_key_env);
    assert_eq!(configured_env_file_name(), opencode.env_file);
    assert!(OpenRouterProvider::has_credentials());
}

#[test]
fn autodetects_single_saved_local_openai_compatible_profile() {
    let _lock = ENV_LOCK.lock();
    let temp = TempDir::new().expect("create temp dir");
    let _xdg = EnvVarGuard::set("XDG_CONFIG_HOME", temp.path());
    let _home = EnvVarGuard::set("HOME", temp.path());
    let _appdata = EnvVarGuard::set("APPDATA", temp.path().join("AppData").join("Roaming"));
    let _env = isolate_openrouter_autodetect_env();

    let lmstudio = crate::provider_catalog::resolve_openai_compatible_profile(
        crate::provider_catalog::LMSTUDIO_PROFILE,
    );
    let config_dir = test_config_dir(&temp).join("jcode");
    std::fs::create_dir_all(&config_dir).expect("create test config dir");
    std::fs::write(
        config_dir.join(&lmstudio.env_file),
        format!(
            "{}=1\n",
            crate::provider_catalog::OPENAI_COMPAT_LOCAL_ENABLED_ENV
        ),
    )
    .expect("write local config");

    assert_eq!(configured_api_base(), lmstudio.api_base);
    assert_eq!(configured_api_key_name(), lmstudio.api_key_env);
    assert_eq!(configured_env_file_name(), lmstudio.env_file);
    assert!(configured_allow_no_auth());
    assert!(OpenRouterProvider::has_credentials());
}

#[test]
fn openrouter_transport_state_distinguishes_runtime_identities() {
    let _lock = ENV_LOCK.lock();
    // Isolate the on-disk config/credential lookup the same way the sibling
    // autodetect tests do, so this test does not read whatever provider
    // profile happens to be configured on the host machine.
    let temp = TempDir::new().expect("create temp dir");
    let _xdg = EnvVarGuard::set("XDG_CONFIG_HOME", temp.path());
    let _home = EnvVarGuard::set("HOME", temp.path());
    let _appdata = EnvVarGuard::set("APPDATA", temp.path().join("AppData").join("Roaming"));
    let _env = isolate_openrouter_autodetect_env();

    assert_eq!(
        OpenRouterTransportState::from_current_env(None),
        OpenRouterTransportState::OpenRouterApiKey
    );
    assert!(OpenRouterTransportState::from_current_env(None).accrues_user_api_key_cost());
    assert!(OpenRouterTransportState::from_current_env(None).is_real_openrouter());

    crate::env::set_var("JCODE_OPENROUTER_TRANSPORT_STATE", "direct-api-key");
    assert_eq!(
        OpenRouterTransportState::from_current_env(None),
        OpenRouterTransportState::DirectApiKey
    );
    crate::env::remove_var("JCODE_OPENROUTER_TRANSPORT_STATE");

    crate::env::set_var("JCODE_RUNTIME_PROVIDER", "openrouter");
    assert_eq!(
        OpenRouterTransportState::from_current_env(Some("openrouter")),
        OpenRouterTransportState::OpenRouterApiKey
    );
    assert!(OpenRouterTransportState::from_current_env(Some("openrouter")).is_real_openrouter());
    crate::env::remove_var("JCODE_RUNTIME_PROVIDER");

    crate::env::set_var("JCODE_RUNTIME_PROVIDER", "jcode");
    assert_eq!(
        OpenRouterTransportState::from_current_env(Some("jcode")),
        OpenRouterTransportState::JcodeSubscription
    );
    assert!(!OpenRouterTransportState::from_current_env(Some("jcode")).accrues_user_api_key_cost());

    crate::env::set_var("JCODE_RUNTIME_PROVIDER", "openai-compatible");
    assert_eq!(
        OpenRouterTransportState::from_current_env(Some("openai-compatible")),
        OpenRouterTransportState::DirectApiKey
    );

    crate::env::set_var("JCODE_OPENROUTER_ALLOW_NO_AUTH", "1");
    assert_eq!(
        OpenRouterTransportState::from_current_env(Some("openai-compatible")),
        OpenRouterTransportState::DirectNoAuth
    );
    assert!(
        !OpenRouterTransportState::from_current_env(Some("openai-compatible"))
            .accrues_user_api_key_cost()
    );

    crate::env::remove_var("JCODE_OPENROUTER_ALLOW_NO_AUTH");
    crate::env::remove_var("JCODE_RUNTIME_PROVIDER");
    crate::env::set_var("JCODE_NAMED_PROVIDER_PROFILE", "my-gateway");
    assert_eq!(
        OpenRouterTransportState::from_current_env(None),
        OpenRouterTransportState::DirectApiKey
    );
}

#[test]
fn does_not_guess_when_multiple_saved_openai_compatible_profiles_exist() {
    let _lock = ENV_LOCK.lock();
    let temp = TempDir::new().expect("create temp dir");
    let _xdg = EnvVarGuard::set("XDG_CONFIG_HOME", temp.path());
    let _home = EnvVarGuard::set("HOME", temp.path());
    let _appdata = EnvVarGuard::set("APPDATA", temp.path().join("AppData").join("Roaming"));
    let _env = isolate_openrouter_autodetect_env();

    let opencode = crate::provider_catalog::resolve_openai_compatible_profile(
        crate::provider_catalog::OPENCODE_PROFILE,
    );
    let chutes = crate::provider_catalog::resolve_openai_compatible_profile(
        crate::provider_catalog::CHUTES_PROFILE,
    );
    write_test_api_key(
        &temp,
        &opencode.env_file,
        &opencode.api_key_env,
        "test-opencode-key",
    );
    write_test_api_key(
        &temp,
        &chutes.env_file,
        &chutes.api_key_env,
        "test-chutes-key",
    );

    assert_eq!(configured_api_base(), DEFAULT_API_BASE);
    assert_eq!(configured_api_key_name(), DEFAULT_API_KEY_NAME);
    assert_eq!(configured_env_file_name(), DEFAULT_ENV_FILE);
    assert!(!OpenRouterProvider::has_credentials());
}

#[test]
fn autodetected_profile_seeds_default_model_and_cache_namespace() {
    let _lock = ENV_LOCK.lock();
    let temp = TempDir::new().expect("create temp dir");
    let _xdg = EnvVarGuard::set("XDG_CONFIG_HOME", temp.path());
    let _home = EnvVarGuard::set("HOME", temp.path());
    let _appdata = EnvVarGuard::set("APPDATA", temp.path().join("AppData").join("Roaming"));
    let _env = isolate_openrouter_autodetect_env();

    let zai = crate::provider_catalog::resolve_openai_compatible_profile(
        crate::provider_catalog::ZAI_PROFILE,
    );
    write_test_api_key(&temp, &zai.env_file, &zai.api_key_env, "test-zai-key");

    let provider = OpenRouterProvider::new().expect("provider");
    assert_eq!(provider.model.blocking_read().clone(), "glm-4.5");
    assert_eq!(
        std::env::var("JCODE_OPENROUTER_CACHE_NAMESPACE")
            .ok()
            .as_deref(),
        Some("zai")
    );
}

#[test]
fn test_parse_model_spec() {
    let (model, provider) = parse_model_spec("anthropic/claude-sonnet-4@Fireworks");
    assert_eq!(model, "anthropic/claude-sonnet-4");
    let provider = provider.expect("provider");
    assert_eq!(provider.name, "Fireworks");
    assert!(provider.allow_fallbacks);

    let (model, provider) = parse_model_spec("anthropic/claude-sonnet-4@Fireworks!");
    assert_eq!(model, "anthropic/claude-sonnet-4");
    let provider = provider.expect("provider");
    assert_eq!(provider.name, "Fireworks");
    assert!(!provider.allow_fallbacks);

    let (model, provider) = parse_model_spec("moonshotai/kimi-k2.5@moonshot");
    assert_eq!(model, "moonshotai/kimi-k2.5");
    let provider = provider.expect("provider");
    assert_eq!(provider.name, "Moonshot AI");

    let (model, provider) = parse_model_spec("anthropic/claude-sonnet-4@auto");
    assert_eq!(model, "anthropic/claude-sonnet-4");
    assert!(provider.is_none());
}

fn make_endpoint(name: &str, throughput: f64, uptime: f64, cache: bool, cost: f64) -> EndpointInfo {
    EndpointInfo {
        provider_name: name.to_string(),
        tag: None,
        pricing: ModelPricing {
            prompt: Some(format!("{:.10}", cost)),
            completion: None,
            input_cache_read: if cache {
                Some("0.00000007".to_string())
            } else {
                None
            },
            input_cache_write: None,
        },
        context_length: None,
        max_completion_tokens: None,
        quantization: None,
        uptime_last_30m: Some(uptime),
        latency_last_30m: None,
        throughput_last_30m: Some(serde_json::json!({"p50": throughput})),
        supports_implicit_caching: Some(cache),
        status: Some(0),
    }
}

fn make_provider() -> OpenRouterProvider {
    OpenRouterProvider {
        client: crate::provider::shared_http_client(),
        model: Arc::new(RwLock::new(DEFAULT_MODEL.to_string())),
        reasoning_effort: Arc::new(RwLock::new(None)),
        api_base: DEFAULT_API_BASE.to_string(),
        auth: ProviderAuth::AuthorizationBearer {
            token: "test".to_string(),
            label: DEFAULT_API_KEY_NAME.to_string(),
        },
        supports_provider_features: true,
        supports_model_catalog: true,
        profile_id: None,
        max_tokens: None,
        static_models: Vec::new(),
        static_context_limits: HashMap::new(),
        send_openrouter_headers: true,
        models_cache: Arc::new(RwLock::new(ModelsCache::default())),
        model_catalog_refresh: Arc::new(Mutex::new(ModelCatalogRefreshState::default())),
        endpoint_refresh: Arc::new(Mutex::new(EndpointRefreshTracker::default())),
        provider_routing: Arc::new(RwLock::new(ProviderRouting::default())),
        provider_pin: Arc::new(Mutex::new(None)),
        endpoints_cache: Arc::new(RwLock::new(HashMap::new())),
    }
}

fn make_custom_compatible_provider() -> OpenRouterProvider {
    OpenRouterProvider {
        client: crate::provider::shared_http_client(),
        model: Arc::new(RwLock::new(DEFAULT_MODEL.to_string())),
        reasoning_effort: Arc::new(RwLock::new(None)),
        api_base: "https://compat.example.test/v1".to_string(),
        auth: ProviderAuth::AuthorizationBearer {
            token: "test".to_string(),
            label: "OPENAI_COMPAT_API_KEY".to_string(),
        },
        supports_provider_features: false,
        supports_model_catalog: true,
        profile_id: None,
        max_tokens: None,
        static_models: Vec::new(),
        static_context_limits: HashMap::new(),
        send_openrouter_headers: false,
        models_cache: Arc::new(RwLock::new(ModelsCache::default())),
        model_catalog_refresh: Arc::new(Mutex::new(ModelCatalogRefreshState::default())),
        endpoint_refresh: Arc::new(Mutex::new(EndpointRefreshTracker::default())),
        provider_routing: Arc::new(RwLock::new(ProviderRouting::default())),
        provider_pin: Arc::new(Mutex::new(None)),
        endpoints_cache: Arc::new(RwLock::new(HashMap::new())),
    }
}

fn spawn_single_response_models_server(body: &'static str) -> (String, mpsc::Receiver<String>) {
    let listener = TcpListener::bind("127.0.0.1:0").expect("bind fake provider server");
    let addr = listener.local_addr().expect("fake provider addr");
    let (request_tx, request_rx) = mpsc::channel();

    std::thread::spawn(move || {
        let (mut stream, _) = listener.accept().expect("accept fake provider request");
        stream
            .set_read_timeout(Some(Duration::from_secs(2)))
            .expect("set read timeout");
        let mut request = vec![0u8; 8192];
        let n = stream.read(&mut request).unwrap_or(0);
        let request = String::from_utf8_lossy(&request[..n]).into_owned();
        let _ = request_tx.send(request);

        let response = format!(
            "HTTP/1.1 200 OK\r\nContent-Type: application/json\r\nContent-Length: {}\r\nConnection: close\r\n\r\n{}",
            body.len(),
            body
        );
        stream
            .write_all(response.as_bytes())
            .expect("write fake provider response");
    });

    (format!("http://{addr}/v1"), request_rx)
}

fn spawn_single_response_chat_server() -> (String, mpsc::Receiver<String>) {
    let listener = TcpListener::bind("127.0.0.1:0").expect("bind fake provider server");
    let addr = listener.local_addr().expect("fake provider addr");
    let (request_tx, request_rx) = mpsc::channel();

    std::thread::spawn(move || {
        let (mut stream, _) = listener.accept().expect("accept fake provider request");
        stream
            .set_read_timeout(Some(Duration::from_secs(2)))
            .expect("set read timeout");
        let mut request = vec![0u8; 16384];
        let n = stream.read(&mut request).unwrap_or(0);
        let request = String::from_utf8_lossy(&request[..n]).into_owned();
        let _ = request_tx.send(request);

        let body = "data: [DONE]\n\n";
        let response = format!(
            "HTTP/1.1 200 OK\r\nContent-Type: text/event-stream\r\nContent-Length: {}\r\nConnection: close\r\n\r\n{}",
            body.len(),
            body
        );
        stream
            .write_all(response.as_bytes())
            .expect("write fake provider response");
    });

    (format!("http://{addr}/v1"), request_rx)
}

#[test]
fn direct_deepseek_profile_exposes_max_reasoning_effort() {
    let provider = OpenRouterProvider {
        profile_id: Some("deepseek".to_string()),
        supports_provider_features: false,
        ..make_custom_compatible_provider()
    };

    assert_eq!(
        provider.available_efforts(),
        vec!["none", "low", "medium", "high", "max"]
    );
    provider
        .set_reasoning_effort("max")
        .expect("DeepSeek direct profile should accept max effort");
    assert_eq!(provider.reasoning_effort().as_deref(), Some("max"));
}

#[test]
fn openrouter_profile_exposes_unified_reasoning_effort() {
    let provider = make_provider();

    assert_eq!(
        provider.available_efforts(),
        vec!["none", "low", "medium", "high", "xhigh"]
    );
    provider
        .set_reasoning_effort("max")
        .expect("OpenRouter max alias should be accepted");
    assert_eq!(provider.reasoning_effort().as_deref(), Some("xhigh"));
}

#[test]
fn non_deepseek_compatible_profile_does_not_expose_reasoning_effort() {
    let provider = make_custom_compatible_provider();

    assert!(provider.available_efforts().is_empty());
    let error = provider
        .set_reasoning_effort("max")
        .expect_err("generic compatible profile should not expose DeepSeek effort UX");
    assert!(
        error.to_string().contains("OpenRouter and DeepSeek"),
        "unexpected error: {error:?}"
    );
}

#[test]
fn openrouter_chat_request_sends_unified_reasoning_effort() {
    let (api_base, request_rx) = spawn_single_response_chat_server();
    let provider = OpenRouterProvider {
        api_base,
        model: Arc::new(RwLock::new("anthropic/claude-sonnet-4.6".to_string())),
        supports_model_catalog: false,
        ..make_provider()
    };
    provider
        .set_reasoning_effort("high")
        .expect("OpenRouter unified reasoning should accept high effort");

    let messages = vec![Message {
        role: Role::User,
        content: vec![ContentBlock::Text {
            text: "hello".to_string(),
            cache_control: None,
        }],
        timestamp: None,
        tool_duration_ms: None,
    }];

    let rt = tokio::runtime::Builder::new_current_thread()
        .enable_all()
        .build()
        .expect("runtime");
    rt.block_on(async {
        let mut stream = provider
            .complete(&messages, &[], "", None)
            .await
            .expect("fake chat request should start");
        while let Some(event) = stream.next().await {
            event.expect("stream event should parse");
        }
    });

    let request = request_rx
        .recv_timeout(Duration::from_secs(2))
        .expect("capture fake provider request");
    assert!(
        request.contains(r#""reasoning":{"effort":"high"}"#),
        "OpenRouter request should include unified reasoning effort: {request}"
    );
    assert!(
        !request.contains(r#""thinking":{"type":"enabled"}"#),
        "unified reasoning should supersede legacy thinking override: {request}"
    );
}

fn live_openrouter_models() -> Vec<String> {
    std::env::var("JCODE_LIVE_OPENROUTER_MODELS")
        .or_else(|_| std::env::var("JCODE_OPENROUTER_MODEL"))
        .unwrap_or_else(|_| "anthropic/claude-sonnet-4.6".to_string())
        .split([',', '\n'])
        .map(str::trim)
        .filter(|model| !model.is_empty())
        .map(ToString::to_string)
        .collect()
}

async fn collect_openrouter_live_smoke_stream(
    mut stream: EventStream,
    timeout: Duration,
) -> Result<(usize, usize, bool)> {
    tokio::time::timeout(timeout, async move {
        let mut text_bytes = 0usize;
        let mut thinking_bytes = 0usize;
        let mut saw_message_end = false;
        while let Some(event) = stream.next().await {
            match event? {
                StreamEvent::TextDelta(text) => {
                    text_bytes += text.len();
                }
                StreamEvent::ThinkingDelta(text) => {
                    thinking_bytes += text.len();
                }
                StreamEvent::MessageEnd { .. } => {
                    saw_message_end = true;
                    break;
                }
                StreamEvent::Error { message, .. } => anyhow::bail!(message),
                _ => {}
            }
        }
        Ok((text_bytes, thinking_bytes, saw_message_end))
    })
    .await
    .context("live OpenRouter smoke timed out")?
}

#[tokio::test]
#[ignore = "live smoke: requires OPENROUTER_API_KEY or configured OpenRouter credentials"]
async fn live_openrouter_unified_reasoning_smoke() -> Result<()> {
    let _env_lock = ENV_LOCK.lock();
    let Some(token) = OpenRouterProvider::get_api_key() else {
        eprintln!(
            "skipping live OpenRouter smoke: OPENROUTER_API_KEY or configured OpenRouter credentials not found"
        );
        return Ok(());
    };

    let models = live_openrouter_models();
    let effort = std::env::var("JCODE_LIVE_OPENROUTER_REASONING_EFFORT")
        .unwrap_or_else(|_| "low".to_string());
    let max_tokens = std::env::var("JCODE_LIVE_OPENROUTER_MAX_TOKENS")
        .ok()
        .and_then(|value| value.trim().parse::<u32>().ok())
        .unwrap_or(1024);

    for model in models {
        let provider = OpenRouterProvider {
            auth: ProviderAuth::AuthorizationBearer {
                token: token.clone(),
                label: configured_api_key_name(),
            },
            model: Arc::new(RwLock::new(model.clone())),
            max_tokens: Some(max_tokens),
            ..make_provider()
        };
        provider.set_reasoning_effort(&effort)?;

        let messages = vec![Message {
            role: Role::User,
            content: vec![ContentBlock::Text {
                text: "Live smoke test: answer exactly OK.".to_string(),
                cache_control: None,
            }],
            timestamp: None,
            tool_duration_ms: None,
        }];

        let stream = provider
            .complete(
                &messages,
                &[],
                "You are a live provider smoke test. Keep the answer tiny.",
                None,
            )
            .await
            .with_context(|| format!("starting live OpenRouter stream for {model}"))?;
        let (text_bytes, thinking_bytes, saw_message_end) =
            collect_openrouter_live_smoke_stream(stream, Duration::from_secs(90))
                .await
                .with_context(|| format!("collecting live OpenRouter stream for {model}"))?;

        eprintln!(
            "live OpenRouter reasoning smoke passed: model={model}, effort={effort}, text_bytes={text_bytes}, thinking_bytes={thinking_bytes}, message_end={saw_message_end}"
        );
        assert!(
            text_bytes > 0 || thinking_bytes > 0,
            "live OpenRouter response for {model} contained neither text nor thinking deltas"
        );
    }

    Ok(())
}

#[test]
fn direct_deepseek_chat_request_sends_reasoning_effort() {
    let (api_base, request_rx) = spawn_single_response_chat_server();
    let provider = OpenRouterProvider {
        api_base,
        model: Arc::new(RwLock::new("deepseek-v4-pro".to_string())),
        profile_id: Some("deepseek".to_string()),
        supports_provider_features: false,
        supports_model_catalog: false,
        send_openrouter_headers: false,
        ..make_custom_compatible_provider()
    };
    provider
        .set_reasoning_effort("max")
        .expect("DeepSeek direct profile should accept max effort");

    let messages = vec![Message {
        role: Role::User,
        content: vec![ContentBlock::Text {
            text: "hello".to_string(),
            cache_control: None,
        }],
        timestamp: None,
        tool_duration_ms: None,
    }];

    let rt = tokio::runtime::Builder::new_current_thread()
        .enable_all()
        .build()
        .expect("runtime");
    rt.block_on(async {
        let mut stream = provider
            .complete(&messages, &[], "", None)
            .await
            .expect("fake chat request should start");
        while let Some(event) = stream.next().await {
            event.expect("stream event should parse");
        }
    });

    let request = request_rx
        .recv_timeout(Duration::from_secs(2))
        .expect("capture fake provider request");
    assert!(
        request.starts_with("POST /v1/chat/completions "),
        "unexpected chat request: {request}"
    );
    assert!(
        request.contains(r#""model":"deepseek-v4-pro""#),
        "request should contain model: {request}"
    );
    assert!(
        request.contains(r#""reasoning_effort":"max""#),
        "DeepSeek request should include max reasoning effort: {request}"
    );
}

#[test]
fn openai_compatible_model_catalog_refresh_calls_models_endpoint_and_updates_display() {
    let _lock = ENV_LOCK.lock();
    let temp = TempDir::new().expect("create temp home");
    let _home = EnvVarGuard::set("HOME", temp.path());
    let _appdata = EnvVarGuard::set("APPDATA", temp.path().join("AppData").join("Roaming"));
    let _namespace = EnvVarGuard::set(
        "JCODE_OPENROUTER_CACHE_NAMESPACE",
        "test-openai-compatible-flow",
    );
    let (api_base, request_rx) = spawn_single_response_models_server(
        r#"{
            "object": "list",
            "data": [
                {"id": "live-login-flow-model", "object": "model", "context_length": 131072}
            ]
        }"#,
    );
    let provider = OpenRouterProvider {
        api_base,
        model: Arc::new(RwLock::new("live-login-flow-model".to_string())),
        auth: ProviderAuth::AuthorizationBearer {
            token: "sk-live-catalog".to_string(),
            label: "OPENAI_COMPAT_API_KEY".to_string(),
        },
        supports_provider_features: false,
        supports_model_catalog: true,
        profile_id: None,
        static_models: vec!["static-login-flow-fallback".to_string()],
        send_openrouter_headers: false,
        ..make_custom_compatible_provider()
    };

    let rt = tokio::runtime::Builder::new_current_thread()
        .enable_all()
        .build()
        .expect("runtime");
    let fetched = rt
        .block_on(provider.refresh_models())
        .expect("refresh fake model catalog");
    assert_eq!(fetched[0].id, "live-login-flow-model");
    assert_eq!(provider.context_window(), 131_072);

    let request = request_rx
        .recv_timeout(Duration::from_secs(2))
        .expect("capture fake provider request");
    assert!(
        request.starts_with("GET /v1/models "),
        "unexpected catalog request: {request}"
    );
    assert!(
        request
            .to_ascii_lowercase()
            .contains("authorization: bearer sk-live-catalog"),
        "catalog request should include saved API key auth header: {request}"
    );
    assert!(
        request.to_ascii_lowercase().contains("user-agent: jcode/"),
        "catalog requests must include a User-Agent because providers like Cerebras reject bare HTTP clients: {request}"
    );

    let display = provider.available_models_display();
    assert!(display.iter().any(|model| model == "live-login-flow-model"));
    assert!(
        display
            .iter()
            .any(|model| model == "static-login-flow-fallback"),
        "static fallback/default models should remain visible alongside live catalog models: {display:?}"
    );

    let fresh_provider = OpenRouterProvider {
        api_base: provider.api_base.clone(),
        model: Arc::new(RwLock::new("live-login-flow-model".to_string())),
        auth: provider.auth.clone(),
        supports_provider_features: false,
        supports_model_catalog: true,
        profile_id: None,
        send_openrouter_headers: false,
        ..make_custom_compatible_provider()
    };
    assert_eq!(fresh_provider.context_window(), 131_072);
}

#[test]
fn built_in_openai_compatible_static_models_drop_out_after_live_catalog() {
    let _lock = ENV_LOCK.lock();
    let temp = TempDir::new().expect("create temp home");
    let _home = EnvVarGuard::set("HOME", temp.path());
    let _appdata = EnvVarGuard::set("APPDATA", temp.path().join("AppData").join("Roaming"));
    let _namespace = EnvVarGuard::set(
        "JCODE_OPENROUTER_CACHE_NAMESPACE",
        "test-cerebras-live-catalog-filters-static-fallback",
    );
    let (api_base, _request_rx) = spawn_single_response_models_server(
        r#"{
            "object": "list",
            "data": [
                {"id": "qwen-3-235b-a22b-instruct-2507", "object": "model"},
                {"id": "zai-glm-4.7", "object": "model"},
                {"id": "gpt-oss-120b", "object": "model"}
            ]
        }"#,
    );
    let provider = OpenRouterProvider {
        api_base,
        auth: ProviderAuth::AuthorizationBearer {
            token: "sk-live-catalog".to_string(),
            label: "CEREBRAS_API_KEY".to_string(),
        },
        supports_provider_features: false,
        supports_model_catalog: true,
        profile_id: Some("cerebras".to_string()),
        static_models: vec!["gpt-oss-120b".to_string(), "zai-glm-4.7".to_string()],
        send_openrouter_headers: false,
        ..make_custom_compatible_provider()
    };

    let rt = tokio::runtime::Builder::new_current_thread()
        .enable_all()
        .build()
        .expect("runtime");
    rt.block_on(provider.refresh_models())
        .expect("refresh fake model catalog");

    let display = provider.available_models_display();
    assert!(display.iter().any(|model| model == "gpt-oss-120b"));
    assert!(display.iter().any(|model| model == "zai-glm-4.7"));
    assert!(
        display
            .iter()
            .any(|model| model == "qwen-3-235b-a22b-instruct-2507"),
        "live catalog chat-capable models should remain visible: {display:?}"
    );
}

#[test]
fn direct_openai_compatible_static_models_are_marked_as_fallback_before_live_catalog() {
    let provider = OpenRouterProvider {
        supports_provider_features: false,
        supports_model_catalog: true,
        profile_id: Some("opencode".to_string()),
        static_models: vec!["minimax-m2.7".to_string()],
        send_openrouter_headers: false,
        ..make_custom_compatible_provider()
    };

    let routes = provider.model_routes();
    let route = routes
        .iter()
        .find(|route| route.model == "minimax-m2.7")
        .expect("static fallback route should be present before live catalog fetch");

    assert!(
        route
            .detail
            .contains("fallback: static provider model list"),
        "fallback routes should be clearly labeled in the model picker: {route:?}"
    );
}

#[test]
fn cerebras_live_catalog_models_are_selectable_on_explicit_switch() {
    let provider = OpenRouterProvider {
        supports_provider_features: false,
        supports_model_catalog: true,
        profile_id: Some("cerebras".to_string()),
        static_models: vec!["gpt-oss-120b".to_string()],
        send_openrouter_headers: false,
        ..make_custom_compatible_provider()
    };

    provider
        .set_model("zai-glm-4.7")
        .expect("live Cerebras model should be selectable");
    assert_eq!(provider.model(), "zai-glm-4.7");
    provider
        .set_model("gpt-oss-120b")
        .expect("default Cerebras model should remain selectable");
    assert_eq!(provider.model(), "gpt-oss-120b");
}

#[test]
fn direct_deepseek_profile_uses_static_1m_context_when_catalog_is_absent() {
    let _lock = ENV_LOCK.lock();
    let _base = EnvVarGuard::set("JCODE_OPENROUTER_API_BASE", "https://api.deepseek.com");
    let _key_name = EnvVarGuard::set("JCODE_OPENROUTER_API_KEY_NAME", "DEEPSEEK_API_KEY");
    let _api_key = EnvVarGuard::set("DEEPSEEK_API_KEY", "test");
    let _namespace = EnvVarGuard::set("JCODE_OPENROUTER_CACHE_NAMESPACE", "deepseek");
    let _model = EnvVarGuard::set("JCODE_OPENROUTER_MODEL", "deepseek-v4-flash");
    let _catalog = EnvVarGuard::set("JCODE_OPENROUTER_MODEL_CATALOG", "0");

    let provider = OpenRouterProvider::new().expect("provider");

    assert_eq!(provider.context_window(), 1_000_000);
}

#[test]
fn named_openai_compatible_model_context_window_overrides_default() {
    let _lock = ENV_LOCK.lock();
    let _namespace = EnvVarGuard::remove("JCODE_OPENROUTER_CACHE_NAMESPACE");
    let mut config = crate::config::NamedProviderConfig {
        base_url: "https://compat.example.test/v1".to_string(),
        api_key: Some("test".to_string()),
        default_model: Some("custom-long-context".to_string()),
        models: vec![crate::config::NamedProviderModelConfig {
            id: "custom-long-context".to_string(),
            context_window: Some(512_000),
            input: Vec::new(),
        }],
        ..Default::default()
    };
    config.model_catalog = false;

    let provider =
        OpenRouterProvider::new_named_openai_compatible("custom", &config).expect("provider");

    assert_eq!(provider.context_window(), 512_000);
}

#[test]
fn named_openai_compatible_loads_api_key_from_env_file() {
    let _lock = ENV_LOCK.lock();
    let temp = TempDir::new().expect("create temp dir");
    let _xdg = EnvVarGuard::set("XDG_CONFIG_HOME", temp.path());
    let _home = EnvVarGuard::set("HOME", temp.path());
    let _appdata = EnvVarGuard::set("APPDATA", temp.path().join("AppData").join("Roaming"));
    let _namespace = EnvVarGuard::remove("JCODE_OPENROUTER_CACHE_NAMESPACE");
    let _api_key = EnvVarGuard::remove("CUSTOM_API_KEY");
    write_test_api_key(&temp, "custom.env", "CUSTOM_API_KEY", "from-env-file");

    let config = crate::config::NamedProviderConfig {
        base_url: "https://compat.example.test/v1".to_string(),
        api_key_env: Some("CUSTOM_API_KEY".to_string()),
        env_file: Some("custom.env".to_string()),
        default_model: Some("custom-model".to_string()),
        ..Default::default()
    };

    OpenRouterProvider::new_named_openai_compatible("custom", &config)
        .expect("provider should load key from env file");
}

#[test]
fn custom_compatible_provider_preserves_claude_like_model_ids() {
    let provider = make_custom_compatible_provider();

    provider.set_model("claude-opus4.6-thinking").unwrap();

    assert_eq!(provider.model(), "claude-opus4.6-thinking");
}

#[test]
fn custom_compatible_provider_preserves_at_sign_model_ids() {
    let provider = make_custom_compatible_provider();

    provider.set_model("gpt-5.4@OpenAI").unwrap();

    assert_eq!(provider.model(), "gpt-5.4@OpenAI");
}

#[test]
fn openrouter_provider_normalizes_bare_pinned_model_ids() {
    let provider = make_provider();

    provider.set_model("gpt-5.4@OpenAI").unwrap();

    assert_eq!(provider.model(), "openai/gpt-5.4");
}

#[test]
fn test_rank_providers_cache_priority() {
    let endpoints = vec![
        make_endpoint("FastCache", 50.0, 99.0, true, 0.0000002),
        make_endpoint("FasterNoCache", 60.0, 99.0, false, 0.0000001),
    ];

    let ranked = OpenRouterProvider::rank_providers_from_endpoints(&endpoints);
    assert_eq!(ranked.first().map(|s| s.as_str()), Some("FastCache"));
}

#[test]
fn test_rank_providers_speed_priority_among_cache_capable() {
    let endpoints = vec![
        make_endpoint("Fireworks", 120.0, 99.0, true, 0.0000013),
        make_endpoint("Moonshot AI", 80.0, 99.0, true, 0.0000010),
    ];

    let ranked = OpenRouterProvider::rank_providers_from_endpoints(&endpoints);
    assert_eq!(ranked.first().map(|s| s.as_str()), Some("Fireworks"));
}

#[test]
fn test_rank_providers_filters_down_providers() {
    let mut down_ep = make_endpoint("DownProvider", 200.0, 100.0, true, 0.0000001);
    down_ep.status = Some(1); // down
    let endpoints = vec![
        down_ep,
        make_endpoint("UpProvider", 50.0, 99.0, true, 0.0000002),
    ];

    let ranked = OpenRouterProvider::rank_providers_from_endpoints(&endpoints);
    assert_eq!(ranked.len(), 1);
    assert_eq!(ranked[0], "UpProvider");
}

#[test]
fn test_background_refresh_waits_for_soft_ttl() {
    let provider = make_provider();

    assert!(!provider.should_background_refresh_model_catalog(
        MODEL_CATALOG_SOFT_REFRESH_SECS.saturating_sub(1)
    ));
    assert!(provider.should_background_refresh_model_catalog(MODEL_CATALOG_SOFT_REFRESH_SECS));
}

#[test]
fn test_background_refresh_is_throttled_between_attempts() {
    let provider = make_provider();
    assert!(provider.begin_background_model_catalog_refresh());
    assert!(!provider.should_background_refresh_model_catalog(MODEL_CATALOG_SOFT_REFRESH_SECS));

    OpenRouterProvider::finish_background_model_catalog_refresh(&provider.model_catalog_refresh);

    assert!(!provider.should_background_refresh_model_catalog(MODEL_CATALOG_SOFT_REFRESH_SECS));
}

#[test]
fn test_kimi_routing_uses_endpoints_or_fallback() {
    let provider = OpenRouterProvider {
        model: Arc::new(RwLock::new("moonshotai/kimi-k2.5".to_string())),
        ..make_provider()
    };

    let rt = tokio::runtime::Builder::new_current_thread()
        .enable_all()
        .build()
        .expect("runtime");
    let routing = rt.block_on(provider.effective_routing("moonshotai/kimi-k2.5"));
    let order = routing.order.expect("provider order should be set");
    // Should have providers - either from endpoint API or Kimi fallback
    assert!(
        !order.is_empty(),
        "Kimi routing should always produce a provider order"
    );
}

#[test]
fn observed_session_provider_pin_sticks_without_fallbacks() {
    // Simulates the KV-cache stickiness contract: after OpenRouter serves a
    // request for this model from a concrete provider (recorded as an
    // observed pin), every subsequent request must route to that exact same
    // provider with fallbacks disabled so the upstream prompt cache stays warm.
    let model = "anthropic/claude-sonnet-4.6";
    let provider = OpenRouterProvider {
        model: Arc::new(RwLock::new(model.to_string())),
        provider_pin: Arc::new(Mutex::new(Some(ProviderPin {
            model: model.to_string(),
            provider: "anthropic".to_string(),
            source: PinSource::Observed,
            allow_fallbacks: true,
            last_cache_read: None,
        }))),
        ..make_provider()
    };

    let rt = tokio::runtime::Builder::new_current_thread()
        .enable_all()
        .build()
        .expect("runtime");
    let routing = rt.block_on(provider.effective_routing(model));

    assert_eq!(
        routing.order.as_deref(),
        Some(["anthropic".to_string()].as_slice()),
        "observed session provider should be pinned exactly"
    );
    assert!(
        !routing.allow_fallbacks,
        "observed session pin must disable fallbacks to preserve the KV cache"
    );
}

#[test]
fn observed_pin_yields_to_explicit_user_routing_order() {
    // If the user explicitly narrowed routing themselves (base order set),
    // their configured order wins over the auto-observed session pin.
    let model = "anthropic/claude-sonnet-4.6";
    let mut base = ProviderRouting::default();
    base.order = Some(vec!["fireworks".to_string()]);
    let provider = OpenRouterProvider {
        model: Arc::new(RwLock::new(model.to_string())),
        provider_routing: Arc::new(RwLock::new(base)),
        provider_pin: Arc::new(Mutex::new(Some(ProviderPin {
            model: model.to_string(),
            provider: "anthropic".to_string(),
            source: PinSource::Observed,
            allow_fallbacks: true,
            last_cache_read: None,
        }))),
        ..make_provider()
    };

    let rt = tokio::runtime::Builder::new_current_thread()
        .enable_all()
        .build()
        .expect("runtime");
    let routing = rt.block_on(provider.effective_routing(model));

    assert_eq!(
        routing.order.as_deref(),
        Some(["fireworks".to_string()].as_slice()),
        "explicit user routing order should win over an observed session pin"
    );
}

#[test]
fn test_kimi_coding_header_detection_matches_endpoint_and_model() {
    assert!(should_send_kimi_coding_agent_headers(
        "https://api.kimi.com/coding/v1",
        None,
    ));
    assert!(should_send_kimi_coding_agent_headers(
        "https://coding.dashscope.aliyuncs.com/v1",
        None,
    ));
    assert!(should_send_kimi_coding_agent_headers(
        "https://coding-intl.dashscope.aliyuncs.com/v1",
        None,
    ));
    assert!(should_send_kimi_coding_agent_headers(
        "https://api.z.ai/api/coding/paas/v4",
        None,
    ));
    assert!(should_send_kimi_coding_agent_headers(
        "https://example.com/v1",
        Some("kimi-for-coding"),
    ));
    assert!(should_send_kimi_coding_agent_headers(
        "https://openrouter.ai/api/v1",
        Some("moonshotai/kimi-k2.5"),
    ));
    assert!(!should_send_kimi_coding_agent_headers(
        "https://api.openrouter.ai/api/v1",
        Some("anthropic/claude-sonnet-4"),
    ));
}

#[test]
fn test_openrouter_kimi_chat_request_includes_compat_user_agent() {
    let request = apply_kimi_coding_agent_headers(
        Client::new().post("https://openrouter.ai/api/v1/chat/completions"),
        "https://openrouter.ai/api/v1",
        Some("moonshotai/kimi-k2.5"),
    )
    .build()
    .expect("build request");
    assert!(
        request
            .headers()
            .get("User-Agent")
            .and_then(|value| value.to_str().ok())
            == Some(KIMI_CODING_USER_AGENT),
        "Kimi OpenRouter chat request should include compatibility User-Agent"
    );
}

#[test]
fn test_parse_next_event_accepts_compact_sse_data_and_reasoning_content() {
    let bytes = Bytes::from_static(
        b"data:{\"choices\":[{\"delta\":{\"reasoning_content\":\"thinking\"}}]}\n\n",
    );
    let mut stream = OpenRouterStream::new(
        futures::stream::once(async move { Ok::<Bytes, reqwest::Error>(bytes) }),
        "kimi-for-coding".to_string(),
        Arc::new(Mutex::new(None)),
    );

    match futures::executor::block_on(stream.next()) {
        Some(Ok(StreamEvent::ThinkingDelta(text))) => assert_eq!(text, "thinking"),
        other => panic!("expected ThinkingDelta, got {:?}", other),
    }
}

#[test]
fn test_parse_next_event_emits_only_incremental_reasoning_content() {
    let chunks = vec![
        Ok::<Bytes, reqwest::Error>(Bytes::from_static(
            b"data:{\"choices\":[{\"delta\":{\"reasoning_content\":\"Thinking\"}}]}\n\n",
        )),
        Ok::<Bytes, reqwest::Error>(Bytes::from_static(
            b"data:{\"choices\":[{\"delta\":{\"reasoning_content\":\"Thinking more\"}}]}\n\n",
        )),
    ];
    let mut stream = OpenRouterStream::new(
        futures::stream::iter(chunks),
        "moonshotai/kimi-k2.5".to_string(),
        Arc::new(Mutex::new(None)),
    );

    match futures::executor::block_on(stream.next()) {
        Some(Ok(StreamEvent::ThinkingDelta(text))) => assert_eq!(text, "Thinking"),
        other => panic!("expected first ThinkingDelta, got {:?}", other),
    }
    match futures::executor::block_on(stream.next()) {
        Some(Ok(StreamEvent::ThinkingDelta(text))) => assert_eq!(text, " more"),
        other => panic!("expected incremental ThinkingDelta, got {:?}", other),
    }
}

#[test]
fn test_endpoint_detail_string() {
    let ep = EndpointInfo {
        provider_name: "TestProvider".to_string(),
        tag: None,
        pricing: ModelPricing {
            prompt: Some("0.00000045".to_string()),
            completion: Some("0.00000225".to_string()),
            input_cache_read: Some("0.00000007".to_string()),
            input_cache_write: Some("0.00000012".to_string()),
        },
        context_length: Some(131072),
        max_completion_tokens: Some(8192),
        quantization: Some("fp8".to_string()),
        uptime_last_30m: Some(99.5),
        latency_last_30m: Some(serde_json::json!({"p50": 500, "p75": 800})),
        throughput_last_30m: Some(serde_json::json!({"p50": 42, "p75": 55})),
        supports_implicit_caching: Some(true),
        status: Some(0),
    };
    let detail = ep.detail_string();
    assert!(
        detail.contains("$0.45/M"),
        "should contain price: {}",
        detail
    );
    assert!(detail.contains("100%"), "should contain uptime: {}", detail);
    assert!(
        detail.contains("out $2.25/M"),
        "should contain output price: {}",
        detail
    );
    assert!(
        detail.contains("cache write $0.12/M"),
        "should contain cache write price: {}",
        detail
    );
    assert!(
        detail.contains("cache read $0.07/M"),
        "should contain cache read price: {}",
        detail
    );
    assert!(
        detail.contains("500ms p50"),
        "should contain latency: {}",
        detail
    );
    assert!(
        detail.contains("42tps"),
        "should contain throughput: {}",
        detail
    );
    assert!(
        detail.contains("cache on"),
        "should contain cache: {}",
        detail
    );
    assert!(
        detail.contains("fp8"),
        "should contain quantization: {}",
        detail
    );
}

#[test]
fn strict_openai_schema_endpoint_detects_mistral_profile() {
    // Mistral direct profile rejects non-standard reasoning_content/thinking
    // fields with a 422 (issue #261), so it must be flagged strict.
    assert!(OpenRouterProvider::strict_openai_schema_endpoint(
        Some("mistral"),
        "https://api.mistral.ai/v1"
    ));
    assert!(OpenRouterProvider::strict_openai_schema_endpoint(
        Some("MISTRAL"),
        "https://example.com/v1"
    ));
}

#[test]
fn strict_openai_schema_endpoint_detects_mistral_api_base() {
    assert!(OpenRouterProvider::strict_openai_schema_endpoint(
        None,
        "https://api.mistral.ai/v1"
    ));
    assert!(OpenRouterProvider::strict_openai_schema_endpoint(
        Some("custom"),
        "https://API.MISTRAL.AI/v1"
    ));
}

#[test]
fn strict_openai_schema_endpoint_allows_other_providers() {
    assert!(!OpenRouterProvider::strict_openai_schema_endpoint(
        Some("deepseek"),
        "https://api.deepseek.com"
    ));
    assert!(!OpenRouterProvider::strict_openai_schema_endpoint(
        None,
        "https://openrouter.ai/api/v1"
    ));
    assert!(!OpenRouterProvider::strict_openai_schema_endpoint(
        Some("openai"),
        "https://api.openai.com/v1"
    ));
}

#[test]
fn runtime_display_name_tracks_active_openai_compatible_profile() {
    // Regression for issue #329: switching to a direct OpenAI-compatible
    // profile (NVIDIA NIM) at runtime must surface that profile's display
    // name, not the fixed "OpenRouter" aggregator label. The machine-facing
    // `name()` stays "openrouter" because billing/routing logic keys off it.
    let _lock = ENV_LOCK.lock();
    let temp = TempDir::new().expect("create temp home");
    let jcode_home = temp.path().join("jcode-home");
    let _jcode_home = EnvVarGuard::set("JCODE_HOME", &jcode_home);
    let _home = EnvVarGuard::set("HOME", temp.path());
    let _appdata = EnvVarGuard::set("APPDATA", temp.path().join("AppData").join("Roaming"));
    let _env = isolate_openrouter_autodetect_env();

    // Configure both the OpenRouter aggregator and NVIDIA NIM credentials so
    // the slot can host either runtime. Set after the isolate guard, which
    // clears every profile api-key env var.
    let _or_key = EnvVarGuard::set("OPENROUTER_API_KEY", "or-test-key");
    let _nim_key = EnvVarGuard::set("NVIDIA_API_KEY", "nim-test-key");
    crate::config::invalidate_config_cache();

    let provider =
        crate::provider::MultiProvider::new_with_auth_status(crate::auth::AuthStatus::default());

    // Switch to a NVIDIA NIM model via the profile-prefixed model request.
    provider
        .set_model("nvidia-nim:nvidia/llama-3.1-nemotron-ultra-253b-v1")
        .expect("switch to nvidia-nim profile");

    assert_eq!(
        Provider::name(&provider),
        "OpenRouter",
        "machine-facing name must stay stable for billing/routing"
    );
    assert_eq!(
        Provider::display_name(&provider),
        "NVIDIA NIM",
        "header/UI display name must reflect the active runtime profile"
    );

    // Switching back to the plain OpenRouter aggregator restores the label.
    provider
        .set_model("anthropic/claude-sonnet-4")
        .expect("switch back to openrouter aggregator");
    assert_eq!(Provider::display_name(&provider), "OpenRouter");
}

#[test]
fn runtime_display_name_for_profile_runtime_instance() {
    // Direct unit coverage of the per-instance resolver used by
    // `Provider::display_name`.
    let _lock = ENV_LOCK.lock();
    let temp = TempDir::new().expect("create temp home");
    let jcode_home = temp.path().join("jcode-home");
    let _jcode_home = EnvVarGuard::set("JCODE_HOME", &jcode_home);
    let _home = EnvVarGuard::set("HOME", temp.path());
    let _appdata = EnvVarGuard::set("APPDATA", temp.path().join("AppData").join("Roaming"));
    let _env = isolate_openrouter_autodetect_env();
    let _key = EnvVarGuard::set("NVIDIA_API_KEY", "nim-test-key");

    let nim = OpenRouterProvider::new_openai_compatible_profile_runtime(
        crate::provider_catalog::NVIDIA_NIM_PROFILE,
    )
    .expect("build nvidia-nim runtime");
    assert_eq!(nim.runtime_display_name(), "NVIDIA NIM");
    assert_eq!(Provider::name(&nim), "openrouter");
}
