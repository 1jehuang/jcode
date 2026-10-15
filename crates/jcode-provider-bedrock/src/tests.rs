use super::*;
use std::ffi::{OsStr, OsString};
use std::sync::{Mutex, MutexGuard, OnceLock};

#[cfg(feature = "aws-sdk")]
#[test]
fn bedrock_tool_schema_removes_top_level_combinators() {
    let schema = json!({
        "oneOf": [
            {"type": "object", "properties": {"action": {"const": "list"}}},
            {"type": "object", "properties": {"query": {"type": "string"}}}
        ],
        "allOf": [
            {"type": "object", "properties": {"category": {"type": "string"}}, "required": ["category"]}
        ]
    });

    let normalized = BedrockProvider::bedrock_input_schema(&schema);
    for keyword in ["oneOf", "anyOf", "allOf"] {
        assert!(normalized.get(keyword).is_none(), "removed {keyword}");
    }
    assert_eq!(normalized["type"], "object");
    assert!(normalized["properties"]["action"].is_object());
    assert!(normalized["properties"]["query"].is_object());
    assert!(normalized["properties"]["category"].is_object());
    assert_eq!(normalized["required"], json!(["category"]));
}

fn lock_test_env() -> MutexGuard<'static, ()> {
    static ENV_LOCK: OnceLock<Mutex<()>> = OnceLock::new();
    ENV_LOCK
        .get_or_init(|| Mutex::new(()))
        .lock()
        .unwrap_or_else(|poisoned| poisoned.into_inner())
}

struct EnvVarGuard {
    key: &'static str,
    previous: Option<OsString>,
}

impl EnvVarGuard {
    fn set(key: &'static str, value: impl AsRef<OsStr>) -> Self {
        let previous = std::env::var_os(key);
        jcode_core::env::set_var(key, value);
        Self { key, previous }
    }

    fn remove(key: &'static str) -> Self {
        let previous = std::env::var_os(key);
        jcode_core::env::remove_var(key);
        Self { key, previous }
    }
}

impl Drop for EnvVarGuard {
    fn drop(&mut self) {
        if let Some(value) = self.previous.as_ref() {
            jcode_core::env::set_var(self.key, value);
        } else {
            jcode_core::env::remove_var(self.key);
        }
    }
}

#[test]
fn detects_env_credentials_requires_region_and_credential_hint() {
    let _guard = lock_test_env();
    let temp = tempfile::tempdir().unwrap();
    let _xdg = EnvVarGuard::set("XDG_CONFIG_HOME", temp.path().as_os_str());
    let _removed = [
        "JCODE_BEDROCK_ENABLE",
        API_KEY_ENV,
        REGION_ENV,
        "AWS_REGION",
        "AWS_DEFAULT_REGION",
        "AWS_PROFILE",
        "JCODE_BEDROCK_PROFILE",
        "AWS_ACCESS_KEY_ID",
        "AWS_SECRET_ACCESS_KEY",
        "AWS_SHARED_CREDENTIALS_FILE",
        "AWS_CONFIG_FILE",
    ]
    .map(EnvVarGuard::remove);
    jcode_core::env::set_var(REGION_ENV, "us-east-1");
    assert!(!BedrockProvider::has_credentials());
    jcode_core::env::set_var("AWS_PROFILE", "test");
    assert!(BedrockProvider::has_credentials());
}

#[test]
fn explicit_enable_marks_configured_for_instance_metadata_credentials() {
    let _guard = lock_test_env();
    jcode_core::env::set_var("JCODE_BEDROCK_ENABLE", "1");
    assert!(BedrockProvider::has_credentials());
    jcode_core::env::remove_var("JCODE_BEDROCK_ENABLE");
}

#[test]
fn detects_bedrock_login_env_file_credentials() {
    let _guard = lock_test_env();
    let temp = tempfile::tempdir().unwrap();
    let _xdg = EnvVarGuard::set("XDG_CONFIG_HOME", temp.path().as_os_str());
    for key in [
        "JCODE_BEDROCK_ENABLE",
        API_KEY_ENV,
        REGION_ENV,
        "AWS_REGION",
        "AWS_DEFAULT_REGION",
        "AWS_PROFILE",
        "JCODE_BEDROCK_PROFILE",
        "AWS_ACCESS_KEY_ID",
    ] {
        jcode_core::env::remove_var(key);
    }

    assert!(!BedrockProvider::has_credentials());
    jcode_provider_env::save_env_value_to_env_file(API_KEY_ENV, ENV_FILE, Some("test-key"))
        .unwrap();
    jcode_core::env::remove_var(API_KEY_ENV);
    assert!(!BedrockProvider::has_credentials());

    jcode_provider_env::save_env_value_to_env_file(REGION_ENV, ENV_FILE, Some("us-east-2"))
        .unwrap();
    jcode_core::env::remove_var(REGION_ENV);

    assert_eq!(
        BedrockProvider::configured_bearer_token().as_deref(),
        Some("test-key")
    );
    assert_eq!(
        BedrockProvider::configured_region().as_deref(),
        Some("us-east-2")
    );
    assert!(BedrockProvider::has_credentials());
}

#[test]
fn configured_profile_from_bedrock_env_overrides_stale_bearer_token() {
    let _guard = lock_test_env();
    let temp = tempfile::tempdir().unwrap();
    let _xdg = EnvVarGuard::set("XDG_CONFIG_HOME", temp.path().as_os_str());
    let _removed = [
        "JCODE_BEDROCK_ENABLE",
        API_KEY_ENV,
        REGION_ENV,
        "AWS_REGION",
        "AWS_DEFAULT_REGION",
        "AWS_PROFILE",
        "JCODE_BEDROCK_PROFILE",
        "AWS_ACCESS_KEY_ID",
        "AWS_SECRET_ACCESS_KEY",
        "AWS_SHARED_CREDENTIALS_FILE",
        "AWS_CONFIG_FILE",
    ]
    .map(EnvVarGuard::remove);

    jcode_provider_env::save_env_value_to_env_file(
        API_KEY_ENV,
        ENV_FILE,
        Some("expired-bearer-token"),
    )
    .unwrap();
    jcode_provider_env::save_env_value_to_env_file(REGION_ENV, ENV_FILE, Some("us-east-2"))
        .unwrap();
    jcode_provider_env::save_env_value_to_env_file(
        "JCODE_BEDROCK_PROFILE",
        ENV_FILE,
        Some("jcode-operator"),
    )
    .unwrap();

    assert_eq!(
        BedrockProvider::configured_profile().as_deref(),
        Some("jcode-operator")
    );
    assert_eq!(
        BedrockProvider::configured_bearer_token().as_deref(),
        Some("expired-bearer-token")
    );
    assert_eq!(BedrockProvider::configured_bearer_token_for_runtime(), None);
    assert!(BedrockProvider::has_credentials());
}

#[test]
fn switches_arbitrary_model_ids() {
    let p = BedrockProvider::new();
    p.set_model("us.anthropic.claude-3-5-sonnet-20241022-v2:0")
        .unwrap();
    assert_eq!(p.model(), "us.anthropic.claude-3-5-sonnet-20241022-v2:0");
}

#[test]
fn maps_profile_required_foundation_model_to_inference_profile() {
    let _guard = lock_test_env();
    let temp = tempfile::tempdir().unwrap();
    let _xdg = EnvVarGuard::set("XDG_CONFIG_HOME", temp.path().as_os_str());
    let p = BedrockProvider::new();
    p.profile_required_models
        .write()
        .unwrap()
        .insert("amazon.nova-2-lite-v1:0".to_string());
    p.inference_profile_routes.write().unwrap().insert(
        "amazon.nova-2-lite-v1:0".to_string(),
        "us.amazon.nova-2-lite-v1:0".to_string(),
    );

    p.set_model("amazon.nova-2-lite-v1:0").unwrap();

    assert_eq!(p.model(), "us.amazon.nova-2-lite-v1:0");
}

#[test]
fn maps_foundation_model_from_stale_cached_profile_list() {
    let _guard = lock_test_env();
    let temp = tempfile::tempdir().unwrap();
    let _xdg = EnvVarGuard::set("XDG_CONFIG_HOME", temp.path().as_os_str());
    let p = BedrockProvider::new();
    *p.fetched_inference_profiles.write().unwrap() = vec![
        "global.amazon.nova-2-lite-v1:0".to_string(),
        "us.amazon.nova-2-lite-v1:0".to_string(),
    ];

    p.set_model("amazon.nova-2-lite-v1:0").unwrap();

    assert_eq!(p.model(), "us.amazon.nova-2-lite-v1:0");
}

#[test]
fn hides_profile_required_foundation_model_when_profile_route_exists() {
    let _guard = lock_test_env();
    let temp = tempfile::tempdir().unwrap();
    let _xdg = EnvVarGuard::set("XDG_CONFIG_HOME", temp.path().as_os_str());
    let p = BedrockProvider::new();
    *p.fetched_models.write().unwrap() = vec!["amazon.nova-2-lite-v1:0".to_string()];
    *p.fetched_inference_profiles.write().unwrap() = vec!["us.amazon.nova-2-lite-v1:0".to_string()];
    p.profile_required_models
        .write()
        .unwrap()
        .insert("amazon.nova-2-lite-v1:0".to_string());
    p.inference_profile_routes.write().unwrap().insert(
        "amazon.nova-2-lite-v1:0".to_string(),
        "us.amazon.nova-2-lite-v1:0".to_string(),
    );

    let display = p.all_display_models();

    assert!(
        !display
            .iter()
            .any(|model| model == "amazon.nova-2-lite-v1:0")
    );
    assert!(
        display
            .iter()
            .any(|model| model == "us.amazon.nova-2-lite-v1:0")
    );
}

#[test]
fn hides_foundation_model_when_profile_route_exists() {
    let _guard = lock_test_env();
    let temp = tempfile::tempdir().unwrap();
    let _xdg = EnvVarGuard::set("XDG_CONFIG_HOME", temp.path().as_os_str());
    let p = BedrockProvider::new();
    *p.fetched_models.write().unwrap() = vec!["amazon.nova-2-lite-v1:0".to_string()];
    *p.fetched_inference_profiles.write().unwrap() = vec!["us.amazon.nova-2-lite-v1:0".to_string()];
    p.inference_profile_routes.write().unwrap().insert(
        "amazon.nova-2-lite-v1:0".to_string(),
        "us.amazon.nova-2-lite-v1:0".to_string(),
    );

    let display = p.all_display_models();

    assert!(
        !display
            .iter()
            .any(|model| model == "amazon.nova-2-lite-v1:0")
    );
    assert!(
        display
            .iter()
            .any(|model| model == "us.amazon.nova-2-lite-v1:0")
    );
}

#[test]
fn profile_required_foundation_model_without_profile_route_is_disabled() {
    let _guard = lock_test_env();
    let temp = tempfile::tempdir().unwrap();
    let _xdg = EnvVarGuard::set("XDG_CONFIG_HOME", temp.path().as_os_str());
    let p = BedrockProvider::new();
    *p.fetched_models.write().unwrap() = vec!["amazon.nova-2-lite-v1:0".to_string()];
    p.profile_required_models
        .write()
        .unwrap()
        .insert("amazon.nova-2-lite-v1:0".to_string());

    let route = p
        .model_routes()
        .into_iter()
        .find(|route| route.model == "amazon.nova-2-lite-v1:0")
        .expect("profile-required foundation model should be listed with a reason");

    assert!(!route.available);
    assert!(route.detail.contains("requires an inference profile"));
}

#[test]
fn global_inference_profiles_use_foundation_capabilities_and_detail() {
    let p = BedrockProvider::new();
    *p.fetched_inference_profiles.write().unwrap() =
        vec!["global.amazon.nova-2-lite-v1:0".to_string()];

    let route = p
        .model_routes()
        .into_iter()
        .find(|route| route.model == "global.amazon.nova-2-lite-v1:0")
        .expect("global inference profile should be listed");

    assert!(route.available);
    assert!(
        route
            .detail
            .contains("inference profile for amazon.nova-2-lite-v1:0")
    );
    assert!(route.detail.contains("tools"));
    assert!(!route.detail.contains("no tools"));
}

#[test]
fn ignores_persisted_bedrock_catalog_from_different_region() {
    let _guard = lock_test_env();
    let temp = tempfile::tempdir().unwrap();
    let _xdg = EnvVarGuard::set("XDG_CONFIG_HOME", temp.path().as_os_str());
    {
        let _region = EnvVarGuard::set(REGION_ENV, "us-east-1");
        BedrockProvider::persist_catalog(
            &["openai.gpt-oss-120b-1:0".to_string()],
            &[],
            &HashSet::new(),
            &HashMap::new(),
            &HashSet::new(),
        );
    }
    let _region = EnvVarGuard::set(REGION_ENV, "us-east-2");

    let p = BedrockProvider::new();

    assert!(p.fetched_models.read().unwrap().is_empty());
}

#[test]
fn prefers_region_inference_profile_over_global_profile() {
    let _guard = lock_test_env();
    let _region = EnvVarGuard::set(REGION_ENV, "us-east-2");
    let mut routes = HashMap::new();

    BedrockProvider::insert_preferred_profile_route(
        &mut routes,
        "amazon.nova-2-lite-v1:0",
        "global.amazon.nova-2-lite-v1:0",
    );
    BedrockProvider::insert_preferred_profile_route(
        &mut routes,
        "amazon.nova-2-lite-v1:0",
        "us.amazon.nova-2-lite-v1:0",
    );

    assert_eq!(
        routes.get("amazon.nova-2-lite-v1:0").map(String::as_str),
        Some("us.amazon.nova-2-lite-v1:0")
    );
}

#[test]
fn known_context_and_vision_capabilities() {
    let p = BedrockProvider::new();
    p.set_model("anthropic.claude-3-5-sonnet-20241022-v2:0")
        .unwrap();
    assert!(p.supports_image_input());
    assert_eq!(p.context_window(), 200_000);
    p.set_model("amazon.nova-micro-v1:0").unwrap();
    assert!(!p.supports_image_input());
    assert_eq!(p.context_window(), 128_000);
}

#[test]
fn known_no_tool_models_do_not_advertise_tools() {
    assert!(!BedrockProvider::model_info("us.deepseek.r1-v1:0").supports_tools);
    assert!(!BedrockProvider::model_info("deepseek.v3.2").supports_tools);
    assert!(!BedrockProvider::model_info("mistral.mistral-large-3-675b-instruct").supports_tools);
    assert!(!BedrockProvider::model_info("openai.gpt-oss-120b-1:0").supports_tools);
    assert!(BedrockProvider::model_info("us.amazon.nova-2-lite-v1:0").supports_tools);
    assert!(BedrockProvider::model_info("us.anthropic.claude-sonnet-4-6").supports_tools);
}

#[test]
fn error_classification_mentions_model_access() {
    let message = BedrockProvider::classify_error_message(
        "ValidationException: The provided model identifier is invalid",
    );
    assert!(message.contains("model"));
    assert!(message.contains("region"));
}

#[test]
fn error_classification_mentions_legacy_models() {
    let message = BedrockProvider::classify_error_message(
        "Access denied. This Model is marked by provider as Legacy and you have not been actively using the model in the last 30 days",
    );
    assert!(message.contains("legacy"));
    assert!(message.contains("active"));
    assert!(!message.starts_with("AWS IAM denied"));
}

#[test]
fn tool_use_streaming_error_is_not_classified_as_legacy_sdk_type_name() {
    let message = BedrockProvider::classify_error_message(
        "ValidationException: This model doesn't support tool use in streaming mode. extensions_1x: {hyper_util::client::legacy::connect::http::HttpInfo}",
    );
    assert!(message.contains("does not support tool use"));
    assert!(!message.starts_with("This Bedrock model is marked as legacy"));
}

#[test]
fn expired_sso_error_is_concise_and_actionable() {
    let message = BedrockProvider::classify_error_message(
        "ServiceError(ServiceError { source: AccessDeniedException(AccessDeniedException { message: Some(\"Bearer Token has expired\") }) })",
    );
    assert_eq!(
        message,
        "AWS SSO/session credentials look expired. Run `aws sso login --profile <profile>` and retry."
    );
}

#[test]
fn missing_credentials_error_omits_sdk_blob() {
    let message = BedrockProvider::classify_error_message(
        "CredentialsNotLoaded: could not load credentials from any provider; extensions_1x: noisy sdk internals",
    );
    assert!(message.contains("AWS credentials were not found"));
    assert!(!message.contains("extensions_1x"));
}

#[test]
fn legacy_model_route_is_unavailable_with_reason() {
    let _guard = lock_test_env();
    let temp = tempfile::tempdir().unwrap();
    let _xdg = EnvVarGuard::set("XDG_CONFIG_HOME", temp.path().as_os_str());
    let p = BedrockProvider::new();
    *p.fetched_models.write().unwrap() = vec!["anthropic.claude-3-haiku-20240307-v1:0".to_string()];
    p.legacy_models
        .write()
        .unwrap()
        .insert("anthropic.claude-3-haiku-20240307-v1:0".to_string());

    let route = p
        .model_routes()
        .into_iter()
        .find(|route| route.model == "anthropic.claude-3-haiku-20240307-v1:0")
        .expect("legacy route should be listed");

    assert!(!route.available);
    assert!(route.detail.contains("legacy"));
}

#[tokio::test]
#[ignore = "requires AWS credentials and enabled Bedrock model access"]
async fn bedrock_live_smoke_test() {
    if std::env::var("JCODE_BEDROCK_LIVE_TEST").ok().as_deref() != Some("1") {
        return;
    }
    let provider = BedrockProvider::new();
    let output = provider
        .complete_simple("say bedrock ok and nothing else", "")
        .await
        .expect("live Bedrock completion");
    assert!(output.to_ascii_lowercase().contains("bedrock ok"));
}
