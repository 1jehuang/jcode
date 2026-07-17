//! OpenRouter / OpenAI-compatible provider runtime (aggregator with
//! provider-routing features, plus direct profile endpoints like DeepSeek and
//! NVIDIA NIM), moved out of `jcode-base` so provider edits compile only this
//! crate plus a binary relink instead of rebuilding the base -> app-core ->
//! tui spine. The binary's composition root registers a parameterized factory
//! with `jcode_base::provider::external::register_openrouter_factory`.

//! OpenRouter API provider
//!
//! Uses OpenRouter's OpenAI-compatible API to access 200+ models from various providers.
//! Models are fetched dynamically from the API and cached to disk.
//!
//! Features:
//! - Provider routing: Ranks providers using OpenRouter's endpoint API data (throughput, uptime, cost, cache support)
//! - Provider pinning: Pins to a provider per-session for cache locality; refreshes pin on cache hits
//! - Cache support: Automatically injects cache breakpoints when provider supports caching
//! - Manual pinning: Set JCODE_OPENROUTER_PROVIDER or use model@Provider syntax

use anyhow::{Context, Result};
use async_trait::async_trait;
use futures::StreamExt;
use jcode_base::provider_catalog::{
    OPENAI_COMPAT_PROFILE, is_safe_env_file_name, is_safe_env_key_name,
    load_api_key_from_env_or_config, load_env_value_from_env_or_config, normalize_api_base,
    openai_compatible_profile_by_id, openai_compatible_profile_id_for_api_base,
    openai_compatible_profile_static_context_limits, openai_compatible_profile_static_models,
    openai_compatible_profiles, resolve_openai_compatible_profile,
};
use jcode_message_types::{CacheControl, ContentBlock, Message, Role, StreamEvent, ToolDefinition};
use jcode_provider_core::{EventStream, Provider};
pub use jcode_provider_openrouter::{
    EndpointInfo, ModelInfo, ModelPricing, ModelTimestampIndex, ProviderRouting,
    all_model_timestamps, load_endpoints_disk_cache_public, load_model_pricing_disk_cache_public,
    load_model_timestamp_index, model_created_timestamp, model_created_timestamp_from_index,
};
use jcode_provider_openrouter::{
    KIMI_FALLBACK_PROVIDERS, ModelCatalogRefreshState, ModelsCache, ParsedProvider, PinSource,
    ProviderPin, current_unix_secs, known_providers, load_disk_cache_entry,
    load_endpoints_disk_cache, parse_model_spec, save_disk_cache_with_source,
    save_disk_cache_with_source_for_namespace, save_endpoints_disk_cache,
};
use reqwest::Client;
use reqwest::header::HeaderName;
use serde::Deserialize;
use serde_json::Value;
use std::collections::{HashMap, HashSet};
use std::sync::{Arc, Mutex, OnceLock};
use tokio::sync::{RwLock, mpsc};
use tokio_stream::wrappers::ReceiverStream;

/// Maximum number of retries for transient errors
const MAX_RETRIES: u32 = 3;

/// Base delay for exponential backoff (in milliseconds)
const RETRY_BASE_DELAY_MS: u64 = 1000;

/// OpenRouter API base URL
const DEFAULT_API_BASE: &str = "https://openrouter.ai/api/v1";
const DEFAULT_API_KEY_NAME: &str = "OPENROUTER_API_KEY";
const DEFAULT_ENV_FILE: &str = "openrouter.env";
const OPENROUTER_TRANSPORT_STATE_ENV: &str = "JCODE_OPENROUTER_TRANSPORT_STATE";
const KIMI_CODING_USER_AGENT: &str = "claude-cli/1.0.0";
const KIMI_CODING_X_APP: &str = "cli";

/// Default model (Claude Sonnet via OpenRouter)
const DEFAULT_MODEL: &str = "anthropic/claude-sonnet-4";

/// Soft refresh TTL for the model catalog.
///
/// We keep the 24h disk cache for resilience/offline startup, but after this
/// shorter interval we refresh in the background so new models appear quickly
/// without blocking the picker UI.
const MODEL_CATALOG_SOFT_REFRESH_SECS: u64 = 15 * 60;
/// Minimum delay between background refresh attempts.
const MODEL_CATALOG_REFRESH_RETRY_SECS: u64 = 60;
/// Standard OpenRouter catalog freshness window for the inactive-slot refresh
/// path. Matches the shared on-disk model-catalog TTL (24h).
const STANDARD_OPENROUTER_CATALOG_TTL_SECS: u64 = 24 * 60 * 60;

/// Endpoints cache TTL (1 hour) - per-model provider endpoint data
const ENDPOINTS_CACHE_TTL_SECS: u64 = 60 * 60;
const MAX_BACKGROUND_ENDPOINT_REFRESHES: usize = 8;

fn explicit_openrouter_runtime_configured() -> bool {
    [
        "JCODE_OPENROUTER_API_BASE",
        "JCODE_OPENROUTER_API_KEY_NAME",
        "JCODE_OPENROUTER_ENV_FILE",
        "JCODE_OPENROUTER_DYNAMIC_BEARER_PROVIDER",
    ]
    .iter()
    .any(|var| std::env::var_os(var).is_some())
}

fn autodetected_openai_compatible_profile()
-> Option<jcode_base::provider_catalog::ResolvedOpenAiCompatibleProfile> {
    if explicit_openrouter_runtime_configured() {
        return None;
    }

    if load_api_key_from_env_or_config(DEFAULT_API_KEY_NAME, DEFAULT_ENV_FILE).is_some() {
        return None;
    }

    let compat = resolve_openai_compatible_profile(OPENAI_COMPAT_PROFILE);
    if load_api_key_from_env_or_config(&compat.api_key_env, &compat.env_file).is_some() {
        return Some(compat);
    }

    let mut matches = openai_compatible_profiles()
        .iter()
        .filter(|profile| profile.id != OPENAI_COMPAT_PROFILE.id)
        .filter_map(|profile| {
            let resolved = resolve_openai_compatible_profile(*profile);
            if jcode_base::provider_catalog::openai_compatible_profile_is_configured(*profile) {
                Some(resolved)
            } else {
                None
            }
        })
        .collect::<Vec<_>>();

    if matches.len() == 1 {
        matches.pop()
    } else {
        None
    }
}

fn configured_api_base() -> String {
    let raw = std::env::var("JCODE_OPENROUTER_API_BASE")
        .ok()
        .map(|v| v.trim().to_string())
        .filter(|v| !v.is_empty())
        .or_else(|| autodetected_openai_compatible_profile().map(|profile| profile.api_base))
        .unwrap_or_else(|| DEFAULT_API_BASE.to_string());
    normalize_api_base(&raw).unwrap_or_else(|| {
        jcode_base::logging::warn(&format!(
            "Ignoring invalid JCODE_OPENROUTER_API_BASE '{}'; using {}",
            raw, DEFAULT_API_BASE
        ));
        DEFAULT_API_BASE.to_string()
    })
}

fn configured_api_key_name() -> String {
    let raw = std::env::var("JCODE_OPENROUTER_API_KEY_NAME")
        .ok()
        .map(|v| v.trim().to_string())
        .filter(|v| !v.is_empty())
        .or_else(|| autodetected_openai_compatible_profile().map(|profile| profile.api_key_env))
        .unwrap_or_else(|| DEFAULT_API_KEY_NAME.to_string());
    if is_safe_env_key_name(&raw) {
        raw
    } else {
        jcode_base::logging::warn(&format!(
            "Ignoring invalid JCODE_OPENROUTER_API_KEY_NAME '{}'; using {}",
            raw, DEFAULT_API_KEY_NAME
        ));
        DEFAULT_API_KEY_NAME.to_string()
    }
}

fn configured_env_file_name() -> String {
    let raw = std::env::var("JCODE_OPENROUTER_ENV_FILE")
        .ok()
        .map(|v| v.trim().to_string())
        .filter(|v| !v.is_empty())
        .or_else(|| autodetected_openai_compatible_profile().map(|profile| profile.env_file))
        .unwrap_or_else(|| DEFAULT_ENV_FILE.to_string());
    if is_safe_env_file_name(&raw) {
        raw
    } else {
        jcode_base::logging::warn(&format!(
            "Ignoring invalid JCODE_OPENROUTER_ENV_FILE '{}'; using {}",
            raw, DEFAULT_ENV_FILE
        ));
        DEFAULT_ENV_FILE.to_string()
    }
}

fn load_named_profile_api_key(
    env_key: &str,
    profile: &jcode_base::config::NamedProviderConfig,
) -> Option<String> {
    if let Some(env_file) = profile
        .env_file
        .as_deref()
        .map(str::trim)
        .filter(|value| !value.is_empty())
    {
        return load_api_key_from_env_or_config(env_key, env_file);
    }

    std::env::var(env_key)
        .ok()
        .map(|key| key.trim().to_string())
        .filter(|key| !key.is_empty())
}

fn parse_env_bool(value: &str) -> Option<bool> {
    match value.trim().to_ascii_lowercase().as_str() {
        "1" | "true" | "yes" | "on" => Some(true),
        "0" | "false" | "no" | "off" => Some(false),
        _ => None,
    }
}

fn provider_features_enabled(api_base: &str) -> bool {
    if let Ok(raw) = std::env::var("JCODE_OPENROUTER_PROVIDER_FEATURES") {
        if let Some(value) = parse_env_bool(&raw) {
            return value;
        }
        jcode_base::logging::warn(&format!(
            "Ignoring invalid JCODE_OPENROUTER_PROVIDER_FEATURES '{}'; expected true/false",
            raw
        ));
    }
    api_base.contains("openrouter.ai")
}

fn model_catalog_enabled() -> bool {
    if let Ok(raw) = std::env::var("JCODE_OPENROUTER_MODEL_CATALOG") {
        if let Some(value) = parse_env_bool(&raw) {
            return value;
        }
        jcode_base::logging::warn(&format!(
            "Ignoring invalid JCODE_OPENROUTER_MODEL_CATALOG '{}'; expected true/false",
            raw
        ));
    }
    true
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum AuthHeaderMode {
    AuthorizationBearer,
    ApiKey,
}

fn configured_auth_header_mode() -> AuthHeaderMode {
    let Some(raw) = std::env::var("JCODE_OPENROUTER_AUTH_HEADER")
        .ok()
        .map(|v| v.trim().to_ascii_lowercase())
        .filter(|v| !v.is_empty())
    else {
        return AuthHeaderMode::AuthorizationBearer;
    };

    match raw.as_str() {
        "authorization" | "authorization-bearer" | "bearer" => AuthHeaderMode::AuthorizationBearer,
        "api-key" | "apikey" => AuthHeaderMode::ApiKey,
        other => {
            jcode_base::logging::warn(&format!(
                "Ignoring invalid JCODE_OPENROUTER_AUTH_HEADER '{}'; expected authorization-bearer or api-key",
                other
            ));
            AuthHeaderMode::AuthorizationBearer
        }
    }
}

fn configured_auth_header_name() -> HeaderName {
    let raw = std::env::var("JCODE_OPENROUTER_AUTH_HEADER_NAME")
        .ok()
        .map(|v| v.trim().to_string())
        .filter(|v| !v.is_empty())
        .unwrap_or_else(|| "api-key".to_string());
    HeaderName::from_bytes(raw.as_bytes()).unwrap_or_else(|_| {
        jcode_base::logging::warn(&format!(
            "Ignoring invalid JCODE_OPENROUTER_AUTH_HEADER_NAME '{}'; using api-key",
            raw
        ));
        HeaderName::from_static("api-key")
    })
}

fn configured_dynamic_bearer_provider() -> Option<String> {
    std::env::var("JCODE_OPENROUTER_DYNAMIC_BEARER_PROVIDER")
        .ok()
        .map(|v| v.trim().to_ascii_lowercase())
        .filter(|v| !v.is_empty())
}

fn configured_allow_no_auth() -> bool {
    std::env::var("JCODE_OPENROUTER_ALLOW_NO_AUTH")
        .ok()
        .and_then(|raw| parse_env_bool(&raw))
        .or_else(|| {
            autodetected_openai_compatible_profile().and_then(|profile| {
                if profile.requires_api_key {
                    None
                } else {
                    Some(true)
                }
            })
        })
        .unwrap_or(false)
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum OpenRouterTransportState {
    /// Real OpenRouter BYOK. The provider implementation is both the runtime identity
    /// and the HTTP transport.
    OpenRouterApiKey,
    /// Jcode subscription access currently reuses the OpenRouter HTTP slot, but is
    /// not user BYOK/OpenRouter billing.
    JcodeSubscription,
    /// A direct OpenAI-compatible endpoint that needs a user key, Azure credential,
    /// or provider-profile secret while reusing the OpenRouter-compatible transport.
    DirectApiKey,
    /// A direct local/no-auth OpenAI-compatible endpoint, for example Ollama or LM Studio.
    DirectNoAuth,
}

impl OpenRouterTransportState {
    pub fn from_current_env(runtime_provider: Option<&str>) -> Self {
        if let Some(state) = Self::from_env_marker() {
            return state;
        }

        let runtime_provider = runtime_provider
            .map(|value| value.trim().to_ascii_lowercase())
            .filter(|value| !value.is_empty());

        if matches!(runtime_provider.as_deref(), Some("jcode")) {
            return Self::JcodeSubscription;
        }

        if matches!(runtime_provider.as_deref(), Some("openrouter")) {
            return Self::OpenRouterApiKey;
        }

        if configured_allow_no_auth() {
            return Self::DirectNoAuth;
        }

        if Self::runtime_provider_is_direct_compatible(runtime_provider.as_deref())
            || std::env::var_os("JCODE_NAMED_PROVIDER_PROFILE").is_some()
        {
            return Self::DirectApiKey;
        }

        let api_base = configured_api_base();
        if provider_features_enabled(&api_base) {
            Self::OpenRouterApiKey
        } else {
            Self::DirectApiKey
        }
    }

    fn from_env_marker() -> Option<Self> {
        let raw = std::env::var(OPENROUTER_TRANSPORT_STATE_ENV).ok()?;
        let value = raw.trim().to_ascii_lowercase();
        if value.is_empty() {
            return None;
        }

        match value.as_str() {
            "openrouter" | "openrouter-api-key" | "openrouter_byok" | "openrouter-byok" => {
                Some(Self::OpenRouterApiKey)
            }
            "jcode" | "jcode-subscription" | "subscription" => Some(Self::JcodeSubscription),
            "direct" | "direct-api-key" | "openai-compatible" | "compatible-api-key" => {
                Some(Self::DirectApiKey)
            }
            "direct-no-auth" | "no-auth" | "local" => Some(Self::DirectNoAuth),
            other => {
                jcode_base::logging::warn(&format!(
                    "Ignoring invalid {} '{}'; expected openrouter-api-key, jcode-subscription, direct-api-key, or direct-no-auth",
                    OPENROUTER_TRANSPORT_STATE_ENV, other
                ));
                None
            }
        }
    }

    fn runtime_provider_is_direct_compatible(runtime_provider: Option<&str>) -> bool {
        matches!(runtime_provider, Some("openai-compatible" | "azure-openai"))
            || runtime_provider
                .and_then(jcode_base::provider_catalog::openai_compatible_profile_by_id)
                .is_some()
    }

    pub fn accrues_user_api_key_cost(self) -> bool {
        matches!(self, Self::OpenRouterApiKey | Self::DirectApiKey)
    }

    pub fn is_real_openrouter(self) -> bool {
        matches!(self, Self::OpenRouterApiKey)
    }
}

fn is_kimi_coding_api_base(api_base: &str) -> bool {
    let Ok(url) = reqwest::Url::parse(api_base) else {
        return false;
    };
    matches!(url.host_str(), Some("api.kimi.com"))
        && url.path().trim_end_matches('/').starts_with("/coding")
}

fn is_coding_agent_api_base(api_base: &str) -> bool {
    let Ok(url) = reqwest::Url::parse(api_base) else {
        return false;
    };
    let host = url.host_str().unwrap_or_default();
    let path = url.path().trim_end_matches('/');
    is_kimi_coding_api_base(api_base)
        || host == "coding.dashscope.aliyuncs.com"
        || host == "coding-intl.dashscope.aliyuncs.com"
        || (host == "api.z.ai" && path.starts_with("/api/coding/paas"))
}

fn is_kimi_model_name(model: &str) -> bool {
    model.to_ascii_lowercase().contains("kimi")
}

fn should_send_kimi_coding_agent_headers(api_base: &str, model: Option<&str>) -> bool {
    is_coding_agent_api_base(api_base) || model.map(is_kimi_model_name).unwrap_or(false)
}

fn apply_kimi_coding_agent_headers(
    req: reqwest::RequestBuilder,
    api_base: &str,
    model: Option<&str>,
) -> reqwest::RequestBuilder {
    if should_send_kimi_coding_agent_headers(api_base, model) {
        req.header("User-Agent", KIMI_CODING_USER_AGENT)
            .header("x-app", KIMI_CODING_X_APP)
    } else {
        req
    }
}

#[derive(Debug, Clone)]
enum ProviderAuth {
    AuthorizationBearer {
        token: String,
        label: String,
    },
    HeaderValue {
        header_name: HeaderName,
        value: String,
        label: String,
    },
    AzureEntra {
        label: String,
    },
    None {
        label: String,
    },
}

impl ProviderAuth {
    async fn apply(&self, req: reqwest::RequestBuilder) -> Result<reqwest::RequestBuilder> {
        match self {
            Self::AuthorizationBearer { token, .. } => Ok(req.bearer_auth(token)),
            Self::HeaderValue {
                header_name, value, ..
            } => Ok(req.header(header_name, value)),
            Self::AzureEntra { .. } => {
                let token = jcode_base::auth::azure::get_bearer_token().await?;
                Ok(req.bearer_auth(token))
            }
            Self::None { .. } => Ok(req),
        }
    }

    fn label(&self) -> &str {
        match self {
            Self::AuthorizationBearer { label, .. } => label,
            Self::HeaderValue { label, .. } => label,
            Self::AzureEntra { label } => label,
            Self::None { label } => label,
        }
    }
}

fn add_cache_breakpoint(messages: &mut [Message]) -> bool {
    let mut cache_index = None;
    for (idx, msg) in messages.iter().enumerate().rev() {
        if let Role::User = msg.role
            && msg
                .content
                .iter()
                .any(|b| matches!(b, ContentBlock::Text { .. }))
        {
            cache_index = Some(idx);
            break;
        }
    }

    let Some(idx) = cache_index else {
        return false;
    };

    let msg = &mut messages[idx];
    for block in msg.content.iter_mut().rev() {
        if let ContentBlock::Text { cache_control, .. } = block {
            if cache_control.is_none() {
                *cache_control = Some(CacheControl::ephemeral(None));
            }
            return true;
        }
    }

    false
}

async fn fetch_models_from_api(
    client: Client,
    api_base: String,
    auth: ProviderAuth,
    models_cache: Arc<RwLock<ModelsCache>>,
    cache_namespace: Option<String>,
) -> Result<Vec<ModelInfo>> {
    let url = format!("{}/models", api_base);
    let response =
        apply_kimi_coding_agent_headers(auth.apply(client.get(&url)).await?, &api_base, None)
            .send()
            .await
            .with_context(|| {
                format!(
                    "Failed to send OpenAI-compatible model catalog request\n  endpoint: {}\n  auth: {}\nHint: check network connectivity, DNS/TLS, and that the base URL includes the API version (usually /v1).",
                    url,
                    auth.label()
                )
            })?;

    if !response.status().is_success() {
        let status = response.status();
        let body = jcode_base::util::http_error_body(response, "HTTP error").await;
        anyhow::bail!(
            "OpenAI-compatible model catalog request failed\n  endpoint: {}\n  auth: {}\n  status: {}\n  response: {}\nHint: verify the base URL includes the API version (usually /v1), the key is valid for this endpoint, and the provider supports GET /models.",
            url,
            auth.label(),
            status,
            body
        );
    }

    let raw_body = response
        .text()
        .await
        .with_context(|| format!("Failed to read model catalog response body from {}", url))?;
    let models = parse_openai_compatible_models_response(&raw_body).with_context(|| {
            format!(
                "Failed to parse OpenAI-compatible model catalog response\n  endpoint: {}\n  auth: {}\n  expected: JSON object with a `data` or `models` array, or a top-level array, with model objects containing at least `id` or `name`\n  response: {}",
                url,
                auth.label(),
                jcode_base::util::truncate_str(&raw_body.trim().replace('\n', "\\n"), 1200)
            )
        })?;

    if let Some(namespace) = cache_namespace.as_deref() {
        save_disk_cache_with_source_for_namespace(namespace, &models, Some(&api_base));
    } else {
        save_disk_cache_with_source(&models, Some(&api_base));
    }

    if let Some(now) = current_unix_secs() {
        let mut cache = models_cache.write().await;
        cache.models = models.clone();
        cache.fetched = true;
        cache.cached_at = Some(now);
    } else {
        let mut cache = models_cache.write().await;
        cache.models = models.clone();
        cache.fetched = true;
    }

    Ok(models)
}

fn parse_openai_compatible_models_response(raw_body: &str) -> Result<Vec<ModelInfo>> {
    let value: Value = serde_json::from_str(raw_body)?;
    let items = match &value {
        Value::Array(items) => items,
        Value::Object(object) => object
            .get("data")
            .or_else(|| object.get("models"))
            .and_then(Value::as_array)
            .context("missing model array")?,
        _ => anyhow::bail!("model catalog response must be an object or array"),
    };

    let mut models = Vec::new();
    for item in items {
        if let Some(model) = parse_model_info_value(item) {
            models.push(model);
        }
    }

    if models.is_empty() {
        anyhow::bail!("model catalog response did not contain any valid model objects");
    }

    Ok(models)
}

fn parse_model_info_value(value: &Value) -> Option<ModelInfo> {
    let object = value.as_object()?;
    let id = object
        .get("id")
        .and_then(Value::as_str)
        .or_else(|| object.get("name").and_then(Value::as_str))?
        .to_string();
    let name = object
        .get("name")
        .and_then(Value::as_str)
        .or_else(|| object.get("display_name").and_then(Value::as_str))
        .or_else(|| object.get("displayName").and_then(Value::as_str))
        .unwrap_or("")
        .to_string();

    Some(ModelInfo {
        id,
        name,
        context_length: first_u64_field(
            object,
            &[
                "context_length",
                "contextLength",
                "max_context_length",
                "maxModelLength",
                "max_model_len",
                "trainingContextLength",
            ],
        )
        // llama.cpp's /v1/models reports the serving context only inside
        // `meta` (`n_ctx`, with `n_ctx_train` as the trained maximum). Without
        // this, local llama.cpp models fall back to the generic 200K default
        // and the context gauge overstates the real window (issue #447).
        .or_else(|| {
            object
                .get("meta")
                .and_then(Value::as_object)
                .and_then(|meta| first_u64_field(meta, &["n_ctx", "n_ctx_train"]))
        }),
        pricing: parse_model_pricing(object.get("pricing")),
        created: object.get("created").and_then(value_as_u64),
    })
}

fn first_u64_field(object: &serde_json::Map<String, Value>, keys: &[&str]) -> Option<u64> {
    keys.iter()
        .find_map(|key| object.get(*key).and_then(value_as_u64))
}

fn value_as_u64(value: &Value) -> Option<u64> {
    match value {
        Value::Number(number) => number.as_u64(),
        Value::String(text) => text.parse::<u64>().ok(),
        _ => None,
    }
}

fn value_as_pricing_string(value: &Value) -> Option<String> {
    match value {
        Value::String(text) => Some(text.clone()),
        Value::Number(number) => Some(number.to_string()),
        _ => None,
    }
}

fn parse_model_pricing(value: Option<&Value>) -> ModelPricing {
    let Some(Value::Object(object)) = value else {
        return ModelPricing::default();
    };

    ModelPricing {
        prompt: object
            .get("prompt")
            .or_else(|| object.get("input"))
            .and_then(value_as_pricing_string),
        completion: object
            .get("completion")
            .or_else(|| object.get("output"))
            .and_then(value_as_pricing_string),
        input_cache_read: object
            .get("input_cache_read")
            .or_else(|| object.get("cached_input"))
            .and_then(value_as_pricing_string),
        input_cache_write: object
            .get("input_cache_write")
            .and_then(value_as_pricing_string),
    }
}

fn models_fingerprint(models: &[ModelInfo]) -> String {
    serde_json::to_string(models).unwrap_or_default()
}

fn endpoints_fingerprint(endpoints: &[EndpointInfo]) -> String {
    serde_json::to_string(endpoints).unwrap_or_default()
}

type EndpointsCache = HashMap<String, (u64, Vec<EndpointInfo>)>;

#[derive(Debug, Default)]
struct EndpointRefreshTracker {
    in_flight: HashSet<String>,
    last_attempt_unix: HashMap<String, u64>,
}

static GLOBAL_ENDPOINT_REFRESH: OnceLock<Mutex<EndpointRefreshTracker>> = OnceLock::new();

fn global_endpoint_refresh() -> &'static Mutex<EndpointRefreshTracker> {
    GLOBAL_ENDPOINT_REFRESH.get_or_init(|| Mutex::new(EndpointRefreshTracker::default()))
}

#[derive(Debug, Default)]
struct ProfileCatalogRefreshTracker {
    in_flight: HashSet<String>,
    last_attempt_unix: HashMap<String, u64>,
}

static GLOBAL_PROFILE_CATALOG_REFRESH: OnceLock<Mutex<ProfileCatalogRefreshTracker>> =
    OnceLock::new();

fn global_profile_catalog_refresh() -> &'static Mutex<ProfileCatalogRefreshTracker> {
    GLOBAL_PROFILE_CATALOG_REFRESH
        .get_or_init(|| Mutex::new(ProfileCatalogRefreshTracker::default()))
}

/// Clear the process-global profile catalog refresh tracker. Tests that
/// assert a refresh fires must not inherit `last_attempt_unix` backoff (or an
/// in-flight marker) recorded by other tests in the same process.
pub fn reset_profile_catalog_refresh_tracker_for_tests() {
    if let Ok(mut state) = global_profile_catalog_refresh().lock() {
        *state = ProfileCatalogRefreshTracker::default();
    }
}

fn begin_profile_catalog_refresh(profile_id: &str) -> bool {
    let Some(now) = current_unix_secs() else {
        return false;
    };
    let Ok(mut state) = global_profile_catalog_refresh().lock() else {
        return false;
    };
    if state.in_flight.contains(profile_id) {
        return false;
    }
    if let Some(last) = state.last_attempt_unix.get(profile_id)
        && now.saturating_sub(*last) < MODEL_CATALOG_REFRESH_RETRY_SECS
    {
        return false;
    }
    state.in_flight.insert(profile_id.to_string());
    state.last_attempt_unix.insert(profile_id.to_string(), now);
    true
}

fn finish_profile_catalog_refresh(profile_id: &str) {
    if let Ok(mut state) = global_profile_catalog_refresh().lock() {
        state.in_flight.remove(profile_id);
    }
}

pub fn maybe_schedule_openai_compatible_profile_catalog_refresh(
    profile: jcode_base::provider_catalog::OpenAiCompatibleProfile,
    context: &'static str,
) -> bool {
    let resolved = jcode_base::provider_catalog::resolve_openai_compatible_profile(profile);
    if !begin_profile_catalog_refresh(&resolved.id) {
        return false;
    }

    let Some(api_base) = normalize_api_base(&resolved.api_base) else {
        finish_profile_catalog_refresh(&resolved.id);
        return false;
    };
    let auth = if let Some(key) =
        load_api_key_from_env_or_config(&resolved.api_key_env, &resolved.env_file)
    {
        ProviderAuth::AuthorizationBearer {
            token: key,
            label: resolved.api_key_env.clone(),
        }
    } else if !resolved.requires_api_key {
        ProviderAuth::None {
            label: "local endpoint (no auth)".to_string(),
        }
    } else {
        finish_profile_catalog_refresh(&resolved.id);
        return false;
    };

    let Ok(handle) = tokio::runtime::Handle::try_current() else {
        finish_profile_catalog_refresh(&resolved.id);
        return false;
    };

    let profile_id = resolved.id.clone();
    let display_name = resolved.display_name.clone();
    let previous_fingerprint =
        jcode_provider_openrouter::load_disk_cache_entry_for_namespace(&profile_id)
            .map(|cache| models_fingerprint(&cache.models))
            .unwrap_or_default();
    handle.spawn(async move {
        let models_cache = Arc::new(RwLock::new(ModelsCache::default()));
        match fetch_models_from_api(
            jcode_provider_core::shared_http_client(),
            api_base,
            auth,
            models_cache,
            Some(profile_id.clone()),
        )
        .await
        {
            Ok(models) => {
                let updated = models_fingerprint(&models) != previous_fingerprint;
                if updated {
                    jcode_base::logging::info(&format!(
                        "Refreshed OpenAI-compatible profile model catalog in background ({}): {} via {} models",
                        context,
                        display_name,
                        models.len()
                    ));
                    jcode_base::bus::Bus::global().publish_models_updated();
                } else {
                    jcode_base::logging::info(&format!(
                        "OpenAI-compatible profile model catalog refresh produced no material change ({}): {} via {} models",
                        context,
                        display_name,
                        models.len()
                    ));
                }
            }
            Err(error) => jcode_base::logging::info(&format!(
                "Failed to refresh OpenAI-compatible profile model catalog in background ({}): {} ({})",
                context, display_name, error
            )),
        }
        finish_profile_catalog_refresh(&profile_id);
    });

    true
}

/// Schedule a background refresh of the *standard* OpenRouter model catalog
/// (the `openrouter` cache namespace), even when standard OpenRouter is not the
/// active provider occupying the shared OpenRouter/OpenAI-compatible runtime
/// slot.
///
/// This matters when a direct OpenAI-compatible profile (e.g. NVIDIA NIM, Groq)
/// is the startup default: that profile owns the single shared slot, so the
/// standard OpenRouter catalog would otherwise never be fetched and its models
/// (e.g. `openrouter/owl-alpha`) would never appear in `/model`. The model
/// picker reads the standard catalog from the `openrouter` disk-cache namespace
/// via `configured_standard_openrouter_profile_routes`; this populates it.
pub fn maybe_schedule_standard_openrouter_catalog_refresh(context: &'static str) -> bool {
    // This always targets canonical openrouter.ai with OPENROUTER_API_KEY and
    // writes to the dedicated `openrouter` cache namespace. It must run even
    // when JCODE_OPENROUTER_* env vars are set by an active named profile
    // (e.g. NVIDIA NIM via `[providers.mynvidia]`, which sets
    // JCODE_OPENROUTER_API_BASE to the NVIDIA endpoint): that profile owns the
    // shared slot and points the live runtime elsewhere, but standard
    // OpenRouter's catalog still needs its own refresh so `/model` can list it
    // (issue #292). Hence we deliberately ignore the shared-slot runtime env.
    let Some(api_key) = load_api_key_from_env_or_config(DEFAULT_API_KEY_NAME, DEFAULT_ENV_FILE)
    else {
        return false;
    };

    let namespace = "openrouter";

    // Only refresh when the cached standard catalog is missing or stale. A
    // present-but-stale cache is the common upgrade case: a user who first ran
    // an older build (before a model like `openrouter/owl-alpha` existed) has a
    // non-empty `openrouter` namespace cache that would otherwise never update
    // while a direct profile owns the shared slot. Reuse the shared 24h catalog
    // TTL so we self-heal on the next picker render after an upgrade.
    let cache_is_fresh = current_unix_secs()
        .zip(jcode_provider_openrouter::load_disk_cache_entry_for_namespace(namespace))
        .map(|(now, cache)| {
            !cache.models.is_empty()
                && now.saturating_sub(cache.cached_at) < STANDARD_OPENROUTER_CATALOG_TTL_SECS
        })
        .unwrap_or(false);
    if cache_is_fresh {
        return false;
    }

    if !begin_profile_catalog_refresh(namespace) {
        return false;
    }

    let Some(api_base) = normalize_api_base(DEFAULT_API_BASE) else {
        finish_profile_catalog_refresh(namespace);
        return false;
    };

    let Ok(handle) = tokio::runtime::Handle::try_current() else {
        finish_profile_catalog_refresh(namespace);
        return false;
    };

    let auth = ProviderAuth::AuthorizationBearer {
        token: api_key,
        label: DEFAULT_API_KEY_NAME.to_string(),
    };
    let previous_fingerprint =
        jcode_provider_openrouter::load_disk_cache_entry_for_namespace(namespace)
            .map(|cache| models_fingerprint(&cache.models))
            .unwrap_or_default();
    handle.spawn(async move {
        let models_cache = Arc::new(RwLock::new(ModelsCache::default()));
        match fetch_models_from_api(
            jcode_provider_core::shared_http_client(),
            api_base,
            auth,
            models_cache,
            Some(namespace.to_string()),
        )
        .await
        {
            Ok(models) => {
                let updated = models_fingerprint(&models) != previous_fingerprint;
                if updated {
                    jcode_base::logging::info(&format!(
                        "Refreshed standard OpenRouter model catalog in background ({}): {} models",
                        context,
                        models.len()
                    ));
                    jcode_base::bus::Bus::global().publish_models_updated();
                } else {
                    jcode_base::logging::info(&format!(
                        "Standard OpenRouter model catalog refresh produced no material change ({}): {} models",
                        context,
                        models.len()
                    ));
                }
            }
            Err(error) => jcode_base::logging::info(&format!(
                "Failed to refresh standard OpenRouter model catalog in background ({}): {}",
                context, error
            )),
        }
        finish_profile_catalog_refresh(namespace);
    });

    true
}

pub struct OpenRouterProvider {
    client: Client,
    model: Arc<RwLock<String>>,
    reasoning_effort: Arc<RwLock<Option<String>>>,
    api_base: String,
    auth: ProviderAuth,
    supports_provider_features: bool,
    supports_model_catalog: bool,
    profile_id: Option<String>,
    /// Explicit `supports_reasoning_effort` override from named-profile config.
    /// `None` means auto-detect (deepseek profile id or DeepSeek-family model).
    reasoning_effort_support: Option<bool>,
    max_tokens: Option<u32>,
    /// Extra top-level JSON object fields merged into every chat/completions
    /// request body (e.g. NVIDIA NIM DeepSeek-V4 `chat_template_kwargs`).
    /// Resolved once at construction from named-profile config or the
    /// `JCODE_OPENAI_EXTRA_BODY` env/env-file value.
    extra_body: Option<serde_json::Map<String, Value>>,
    static_models: Vec<String>,
    static_context_limits: HashMap<String, usize>,
    /// Explicit per-model image-input capability from named-provider `models[].input`.
    /// Missing entries mean unspecified and preserve the provider-level fallback.
    static_image_input_support: HashMap<String, bool>,
    send_openrouter_headers: bool,
    models_cache: Arc<RwLock<ModelsCache>>,
    model_catalog_refresh: Arc<Mutex<ModelCatalogRefreshState>>,
    /// Provider routing preferences
    provider_routing: Arc<RwLock<ProviderRouting>>,
    /// Pinned provider for this session (cache-aware)
    provider_pin: Arc<Mutex<Option<ProviderPin>>>,
    /// In-memory cache of per-model endpoint data
    endpoints_cache: Arc<RwLock<EndpointsCache>>,
    /// Background refresh state for per-model endpoint data
    endpoint_refresh: Arc<Mutex<EndpointRefreshTracker>>,
}

impl OpenRouterProvider {
    fn profile_supports_reasoning_effort(profile_id: Option<&str>) -> bool {
        matches!(profile_id, Some(id) if id.eq_ignore_ascii_case("deepseek"))
    }

    /// DeepSeek-family models accept the DeepSeek-style top-level
    /// `reasoning_effort` request field regardless of which OpenAI-compatible
    /// gateway serves them (issue #352: profiles like opencode-go serve
    /// DeepSeek V4 but were rejected by the profile-id-only check).
    fn model_is_deepseek_family(model: &str) -> bool {
        model.trim().to_ascii_lowercase().contains("deepseek")
    }

    /// Does this runtime accept the DeepSeek-style `reasoning_effort` field?
    /// Priority: explicit named-profile config override, then the dedicated
    /// deepseek profile, then the active model family for direct compat
    /// endpoints (never for real OpenRouter, which uses unified reasoning).
    pub(crate) fn supports_deepseek_reasoning_effort(&self) -> bool {
        if let Some(explicit) = self.reasoning_effort_support {
            return explicit;
        }
        if Self::profile_supports_reasoning_effort(self.profile_id.as_deref()) {
            return true;
        }
        !Self::profile_supports_unified_reasoning(
            self.profile_id.as_deref(),
            self.send_openrouter_headers,
        ) && Self::model_is_deepseek_family(&self.model_snapshot())
    }

    /// GPT-family reasoning models (gpt-5.x, codex variants, o-series) accept
    /// the standard OpenAI `reasoning_effort` request field on any
    /// OpenAI-compatible gateway that proxies them (e.g. OpenCode Zen serving
    /// `gpt-5.3-codex-spark`). Real OpenRouter uses unified reasoning instead.
    fn model_is_openai_reasoning_family(model: &str) -> bool {
        let model = model.trim().to_ascii_lowercase();
        model.starts_with("gpt-5")
            || model.contains("codex")
            || model.starts_with("o3")
            || model.starts_with("o4")
    }

    /// Does this runtime accept the OpenAI-style `reasoning_effort` field for
    /// the active model? Only for direct compat endpoints serving GPT-family
    /// reasoning models, and only when no explicit config override or
    /// DeepSeek-style support already applies.
    pub(crate) fn supports_openai_reasoning_effort(&self) -> bool {
        if self.reasoning_effort_support == Some(false) {
            return false;
        }
        !Self::profile_supports_unified_reasoning(
            self.profile_id.as_deref(),
            self.send_openrouter_headers,
        ) && Self::model_is_openai_reasoning_family(&self.model_snapshot())
    }

    fn model_snapshot(&self) -> String {
        self.model
            .try_read()
            .map(|model| model.clone())
            .unwrap_or_default()
    }

    pub(crate) fn supports_any_reasoning_effort(&self) -> bool {
        self.supports_deepseek_reasoning_effort()
            || self.supports_openai_reasoning_effort()
            || Self::profile_supports_unified_reasoning(
                self.profile_id.as_deref(),
                self.send_openrouter_headers,
            )
    }

    pub(crate) fn normalize_reasoning_effort_for_self(&self, effort: &str) -> Option<String> {
        if self.supports_deepseek_reasoning_effort() {
            Self::normalize_reasoning_effort(effort)
        } else {
            Self::normalize_unified_reasoning_effort(effort)
        }
    }

    /// Initial reasoning effort at construction. Named/compat profiles that
    /// support effort honor the user's configured `openai_reasoning_effort`
    /// (issue #352: previously hardcoded to None so the config was ignored).
    fn initial_reasoning_effort(
        reasoning_effort_support: Option<bool>,
        profile_id: Option<&str>,
    ) -> Option<String> {
        let supported =
            reasoning_effort_support.unwrap_or(Self::profile_supports_reasoning_effort(profile_id));
        if !supported {
            return None;
        }
        jcode_base::config::config()
            .provider
            .openai_reasoning_effort
            .as_deref()
            .and_then(Self::normalize_reasoning_effort)
    }

    fn profile_rejects_image_input(profile_id: Option<&str>) -> bool {
        matches!(profile_id, Some(id) if id.eq_ignore_ascii_case("deepseek"))
    }

    fn profile_supports_unified_reasoning(
        profile_id: Option<&str>,
        send_openrouter_headers: bool,
    ) -> bool {
        // Real OpenRouter uses unified reasoning. The runtime may carry either
        // no profile id or the "openrouter" doctor-profile id (assigned when
        // the default api base matches the OpenRouter OpenAI-compat profile),
        // so both must qualify (issue: effort rejected on plain OpenRouter).
        send_openrouter_headers
            && profile_id.is_none_or(|id| id.eq_ignore_ascii_case("openrouter"))
    }

    fn normalize_reasoning_effort(raw: &str) -> Option<String> {
        let value = raw.trim().to_ascii_lowercase();
        if value.is_empty() {
            return None;
        }
        match value.as_str() {
            "none" | "low" | "medium" | "high" | "max" | "swarm" | "swarm-deep" => Some(value),
            // Match the existing OpenAI UX: accept unknown non-empty effort values
            // by snapping to the strongest setting instead of rejecting the command.
            other => {
                jcode_base::logging::info(&format!(
                    "Warning: Unsupported DeepSeek reasoning effort '{}'; expected none|low|medium|high|max. Using 'max'.",
                    other
                ));
                Some("max".to_string())
            }
        }
    }

    fn normalize_unified_reasoning_effort(raw: &str) -> Option<String> {
        let value = raw.trim().to_ascii_lowercase();
        if value.is_empty() {
            return None;
        }
        match value.as_str() {
            "none" | "low" | "medium" | "high" | "xhigh" | "swarm" | "swarm-deep" => Some(value),
            "max" => Some("xhigh".to_string()),
            other => {
                jcode_base::logging::info(&format!(
                    "Warning: Unsupported OpenRouter reasoning effort '{}'; expected none|low|medium|high|xhigh|max alias. Using 'xhigh'.",
                    other
                ));
                Some("xhigh".to_string())
            }
        }
    }

    fn configured_max_tokens(profile_id: Option<&str>) -> Option<u32> {
        if let Ok(raw) = std::env::var("JCODE_OPENROUTER_MAX_TOKENS") {
            let trimmed = raw.trim();
            if trimmed.is_empty() || trimmed.eq_ignore_ascii_case("auto") {
                return None;
            }
            match trimmed.parse::<u32>() {
                Ok(0) => return None,
                Ok(value) => return Some(value),
                Err(_) => jcode_base::logging::warn(&format!(
                    "Ignoring invalid JCODE_OPENROUTER_MAX_TOKENS '{}'; expected a positive integer or auto",
                    raw
                )),
            }
        }

        let _ = profile_id;
        None
    }

    /// Resolve extra request-body fields for an OpenAI-compatible/OpenRouter
    /// provider.
    ///
    /// Sources, in precedence order (later overrides earlier):
    /// 1. An optional named-profile `extra_body` config object.
    /// 2. The `JCODE_OPENAI_EXTRA_BODY` env var (or the same key inside the
    ///    profile's `.env` file), parsed as a JSON object string.
    ///
    /// This lets users inject non-standard parameters that some backends
    /// require, e.g. NVIDIA NIM DeepSeek-V4 reasoning models need
    /// `chat_template_kwargs = { "thinking": true, "reasoning_effort": "high" }`
    /// or they silently hang instead of responding (issue #341).
    ///
    /// Returns `None` when nothing is configured. Invalid input is logged and
    /// ignored rather than failing provider construction.
    fn resolve_extra_body(
        config: Option<&serde_json::Value>,
        env_file: &str,
    ) -> Option<serde_json::Map<String, Value>> {
        let mut merged = serde_json::Map::new();

        if let Some(value) = config {
            match value.as_object() {
                Some(object) => {
                    for (key, val) in object {
                        merged.insert(key.clone(), val.clone());
                    }
                }
                None => jcode_base::logging::warn(
                    "Ignoring provider `extra_body`: expected a table/object of top-level request fields",
                ),
            }
        }

        if let Some(raw) = load_env_value_from_env_or_config("JCODE_OPENAI_EXTRA_BODY", env_file) {
            match serde_json::from_str::<Value>(&raw) {
                Ok(Value::Object(object)) => {
                    for (key, val) in object {
                        merged.insert(key, val);
                    }
                }
                Ok(_) => jcode_base::logging::warn(
                    "Ignoring JCODE_OPENAI_EXTRA_BODY: expected a JSON object string, e.g. {\"chat_template_kwargs\":{\"thinking\":true}}",
                ),
                Err(err) => jcode_base::logging::warn(&format!(
                    "Ignoring invalid JCODE_OPENAI_EXTRA_BODY JSON: {err}"
                )),
            }
        }

        if merged.is_empty() {
            None
        } else {
            Some(merged)
        }
    }

    pub fn supports_provider_routing_features(&self) -> bool {
        self.supports_provider_features
    }

    /// Human-facing label for the runtime backing this provider instance.
    ///
    /// Unlike the env-var based [`jcode_base::provider_catalog::runtime_provider_display_name`],
    /// this reads the instance's own `profile_id`/`api_base`, so it stays correct
    /// after a runtime `/model` switch to a different OpenAI-compatible profile
    /// (e.g. NVIDIA NIM) even though `name()` is fixed at `"openrouter"`.
    pub fn runtime_display_name(&self) -> String {
        // Direct OpenAI-compatible profile (NVIDIA NIM, DeepSeek, Z.AI, ...).
        if let Some(profile_id) = self.profile_id.as_deref() {
            if let Some(profile) = openai_compatible_profile_by_id(profile_id) {
                return profile.display_name.to_string();
            }
            return profile_id.to_string();
        }

        // Non-aggregator endpoint without a known profile id: classify by base
        // URL so custom OpenAI-compatible endpoints don't masquerade as the
        // public OpenRouter aggregator.
        if !self.supports_provider_features {
            if let Some(profile_id) =
                jcode_base::provider_catalog::openai_compatible_profile_id_for_api_base(
                    &self.api_base,
                )
                && let Some(profile) = openai_compatible_profile_by_id(profile_id)
            {
                return profile.display_name.to_string();
            }
            if std::env::var("JCODE_RUNTIME_PROVIDER")
                .ok()
                .is_some_and(|value| value.trim().eq_ignore_ascii_case("azure-openai"))
            {
                return "Azure OpenAI".to_string();
            }
            if !self.api_base.contains("openrouter.ai") {
                return "OpenAI-compatible".to_string();
            }
        }

        "OpenRouter".to_string()
    }

    pub fn direct_openai_compatible_route_parts(&self) -> Option<(String, String, String)> {
        if self.supports_provider_features {
            return None;
        }

        let provider_label = self
            .profile_id
            .as_deref()
            .map(|profile_id| {
                openai_compatible_profile_by_id(profile_id)
                    .map(|profile| profile.display_name.to_string())
                    .unwrap_or_else(|| profile_id.to_string())
            })
            .unwrap_or_else(|| "OpenAI-compatible".to_string());
        let api_method = self
            .profile_id
            .as_deref()
            .map(|profile_id| format!("openai-compatible:{}", profile_id))
            .unwrap_or_else(|| "openai-compatible".to_string());

        Some((provider_label, api_method, self.api_base.clone()))
    }

    pub fn new_named_openai_compatible(
        profile_name: &str,
        profile: &jcode_base::config::NamedProviderConfig,
    ) -> Result<Self> {
        // The OpenRouter/OpenAI-compatible catalog cache helpers are currently
        // process-env scoped. Named provider profiles are constructed directly
        // in several CLI/TUI paths, so make sure their cache namespace is active
        // before any model-cache reads/writes happen. Without this, a custom
        // endpoint can accidentally display the default OpenRouter catalog.
        jcode_base::env::set_var("JCODE_OPENROUTER_CACHE_NAMESPACE", profile_name);
        let api_base = normalize_api_base(&profile.base_url).ok_or_else(|| {
            anyhow::anyhow!("Provider profile '{}' has invalid base_url", profile_name)
        })?;
        let key_env = profile
            .api_key_env
            .as_deref()
            .map(str::trim)
            .filter(|v| !v.is_empty());
        let key_label = key_env.unwrap_or("inline api_key").to_string();
        let key = key_env
            .and_then(|name| load_named_profile_api_key(name, profile))
            .or_else(|| profile.api_key.clone());
        let auth = match profile.auth {
            jcode_base::config::NamedProviderAuth::None => ProviderAuth::None {
                label: "local endpoint (no auth)".to_string(),
            },
            jcode_base::config::NamedProviderAuth::Bearer => ProviderAuth::AuthorizationBearer {
                token: key
                    .ok_or_else(|| anyhow::anyhow!("{} not found in environment", key_label))?,
                label: key_label,
            },
            jcode_base::config::NamedProviderAuth::Header => ProviderAuth::HeaderValue {
                header_name: HeaderName::from_bytes(
                    profile
                        .auth_header
                        .as_deref()
                        .unwrap_or("api-key")
                        .as_bytes(),
                )?,
                value: key
                    .ok_or_else(|| anyhow::anyhow!("{} not found in environment", key_label))?,
                label: key_label,
            },
        };
        let model = profile
            .default_model
            .clone()
            .unwrap_or_else(|| DEFAULT_MODEL.to_string());
        let static_models = profile
            .models
            .iter()
            .map(|m| m.id.trim())
            .filter(|id| !id.is_empty())
            .map(ToString::to_string)
            .collect::<Vec<_>>();
        let static_context_limits = profile
            .models
            .iter()
            .filter_map(|model| {
                let id = model.id.trim();
                if id.is_empty() {
                    return None;
                }
                model
                    .context_window
                    .map(|limit| (id.to_ascii_lowercase(), limit))
            })
            .collect::<HashMap<_, _>>();
        let static_image_input_support = profile
            .models
            .iter()
            .filter_map(|model| {
                let id = model.id.trim();
                if id.is_empty() || model.input.is_empty() {
                    return None;
                }
                let supports_images = model
                    .input
                    .iter()
                    .any(|input| input.eq_ignore_ascii_case("image"));
                Some((id.to_ascii_lowercase(), supports_images))
            })
            .collect::<HashMap<_, _>>();
        Ok(Self {
            client: jcode_provider_core::shared_http_client(),
            model: Arc::new(RwLock::new(model)),
            reasoning_effort: Arc::new(RwLock::new(Self::initial_reasoning_effort(
                profile.supports_reasoning_effort,
                Some(profile_name),
            ))),
            api_base,
            auth,
            supports_provider_features: matches!(
                profile.provider_type,
                jcode_base::config::NamedProviderType::OpenRouter
            ) || profile.provider_routing
                || profile.allow_provider_pinning,
            supports_model_catalog: profile.model_catalog
                || matches!(
                    profile.provider_type,
                    jcode_base::config::NamedProviderType::OpenRouter
                ),
            profile_id: Some(profile_name.to_string()),
            reasoning_effort_support: profile.supports_reasoning_effort,
            max_tokens: Self::configured_max_tokens(Some(profile_name)),
            extra_body: Self::resolve_extra_body(
                profile.extra_body.as_ref(),
                profile
                    .env_file
                    .as_deref()
                    .filter(|name| is_safe_env_file_name(name))
                    .unwrap_or(DEFAULT_ENV_FILE),
            ),
            static_models,
            static_context_limits,
            static_image_input_support,
            send_openrouter_headers: false,
            models_cache: Arc::new(RwLock::new(ModelsCache::default())),
            model_catalog_refresh: Arc::new(Mutex::new(ModelCatalogRefreshState::default())),
            provider_routing: Arc::new(RwLock::new(ProviderRouting::default())),
            provider_pin: Arc::new(Mutex::new(None)),
            endpoints_cache: Arc::new(RwLock::new(HashMap::new())),
            endpoint_refresh: Arc::new(Mutex::new(EndpointRefreshTracker::default())),
        })
    }

    /// Return true if this model is a Kimi K2/K2.5 variant (Moonshot).
    fn is_kimi_model(model: &str) -> bool {
        jcode_provider_openrouter::is_kimi_model(model)
    }

    /// Strip a session-routing `<profile>:` prefix from a model spec.
    ///
    /// Session restore persists models as `<provider-key>:<model>` (see
    /// `MultiProvider::model_switch_request_for_session_*`). For a standalone
    /// `OpenRouterProvider` bound to a named OpenAI-compatible profile, that
    /// prefix is a routing token, not part of the model id, and must not reach
    /// the wire. We strip it when:
    /// - the spec has a `:` separator, and
    /// - the prefix is NOT a built-in routing prefix
    ///   (`explicit_model_provider_prefix`), and
    /// - the prefix matches either this provider's own `profile_id` or a known
    ///   built-in OpenAI-compatible profile id.
    ///
    /// Built-in routing prefixes (`claude:`, `openai:`, `copilot:`, ...) are
    /// left intact so switching the active provider from a saved session still
    /// round-trips verbatim.
    fn strip_session_profile_prefix<'a>(&self, model: &'a str) -> &'a str {
        let Some((prefix, rest)) = model.split_once(':') else {
            return model;
        };
        if jcode_provider_core::explicit_model_provider_prefix(model).is_some() {
            return model;
        }
        let rest = rest.trim();
        if rest.is_empty() {
            return model;
        }
        let prefix = prefix.trim();
        let matches_known_profile = self
            .profile_id
            .as_deref()
            .is_some_and(|id| id.eq_ignore_ascii_case(prefix))
            || openai_compatible_profile_by_id(prefix).is_some()
            // A user-defined named provider profile (`[providers.<name>]` in
            // config.toml) is also a valid session-routing prefix. The shared
            // server may boot via the deferred-auth path without binding this
            // provider's `profile_id`, so without this check a `<name>:` prefix
            // (e.g. `cline:`) would leak verbatim to the upstream API and be
            // rejected with a 404 model_not_found.
            || jcode_base::config::config()
                .providers
                .keys()
                .any(|id| id.eq_ignore_ascii_case(prefix));
        if matches_known_profile { rest } else { model }
    }

    /// Return true when this request targets Moonshot's dedicated Kimi coding
    /// endpoint (`https://api.kimi.com/coding/v1`, default model
    /// `kimi-for-coding`). That endpoint enables thinking server-side and
    /// rejects any assistant tool-call message that lacks `reasoning_content`
    /// (issue #322). The endpoint's own model id (`kimi-for-coding`) is not
    /// caught by `is_kimi_model`, so detect it by profile/api-base/model.
    fn is_kimi_coding_endpoint(&self, model: &str) -> bool {
        self.profile_id
            .as_deref()
            .is_some_and(|id| id.eq_ignore_ascii_case("kimi"))
            || is_kimi_coding_api_base(&self.api_base)
            || is_kimi_model_name(model)
    }

    /// Parse thinking override from env. Values: "enabled"/"disabled"/"auto".
    /// Returns Some(true)=force enable, Some(false)=force disable, None=auto.
    fn thinking_override() -> Option<bool> {
        let raw = std::env::var("JCODE_OPENROUTER_THINKING").ok()?;
        let value = raw.trim().to_lowercase();
        match value.as_str() {
            "enabled" | "enable" | "on" | "true" | "1" => Some(true),
            "disabled" | "disable" | "off" | "false" | "0" => Some(false),
            "auto" | "" => None,
            other => {
                jcode_base::logging::info(&format!(
                    "Warning: Unsupported JCODE_OPENROUTER_THINKING '{}'; expected enabled/disabled/auto",
                    other
                ));
                None
            }
        }
    }

    /// Detect providers that strictly enforce the OpenAI-compatible schema and
    /// reject the non-standard `reasoning_content` message field and top-level
    /// `thinking` request field. Mistral's API returns 422 "Extra inputs are
    /// not permitted" when either is present (issue #261).
    fn strict_openai_schema_endpoint(profile_id: Option<&str>, api_base: &str) -> bool {
        if profile_id
            .map(|id| id.eq_ignore_ascii_case("mistral"))
            .unwrap_or(false)
        {
            return true;
        }
        api_base.to_ascii_lowercase().contains("mistral.ai")
    }

    pub fn new() -> Result<Self> {
        let autodetected_profile = autodetected_openai_compatible_profile();
        let api_base = configured_api_base();
        let supports_provider_features = provider_features_enabled(&api_base);
        let supports_model_catalog = model_catalog_enabled();
        let send_openrouter_headers = supports_provider_features;
        let auth = Self::resolve_auth()?;
        let profile_id = std::env::var("JCODE_OPENROUTER_CACHE_NAMESPACE")
            .ok()
            .map(|value| value.trim().to_ascii_lowercase())
            .filter(|value| !value.is_empty())
            .and_then(|id| openai_compatible_profile_by_id(&id).map(|_| id))
            .or_else(|| {
                autodetected_profile
                    .as_ref()
                    .map(|profile| profile.id.clone())
            })
            .or_else(|| {
                openai_compatible_profile_id_for_api_base(&api_base).map(ToString::to_string)
            });
        let static_context_limits = profile_id
            .as_deref()
            .and_then(openai_compatible_profile_by_id)
            .map(openai_compatible_profile_static_context_limits)
            .unwrap_or_default();
        let static_models = std::env::var("JCODE_OPENROUTER_STATIC_MODELS")
            .ok()
            .map(|raw| {
                raw.lines()
                    .map(str::trim)
                    .filter(|line| !line.is_empty())
                    .map(ToString::to_string)
                    .collect::<Vec<_>>()
            })
            .unwrap_or_else(|| {
                autodetected_profile
                    .as_ref()
                    .and_then(|profile| openai_compatible_profile_by_id(&profile.id))
                    .map(openai_compatible_profile_static_models)
                    .unwrap_or_default()
            });

        if std::env::var_os("JCODE_OPENROUTER_CACHE_NAMESPACE").is_none()
            && let Some(profile) = autodetected_profile.as_ref()
        {
            jcode_base::env::set_var("JCODE_OPENROUTER_CACHE_NAMESPACE", &profile.id);
        }

        let model = std::env::var("JCODE_OPENROUTER_MODEL")
            .ok()
            .map(|value| value.trim().to_string())
            .filter(|value| !value.is_empty())
            .or_else(|| {
                autodetected_profile
                    .as_ref()
                    .and_then(|profile| profile.default_model.clone())
            })
            .unwrap_or_else(|| DEFAULT_MODEL.to_string());

        // Parse provider routing from environment
        let provider_routing = if supports_provider_features {
            Self::parse_provider_routing()
        } else {
            ProviderRouting::default()
        };
        let max_tokens = Self::configured_max_tokens(profile_id.as_deref());
        let extra_body = Self::resolve_extra_body(None, &configured_env_file_name());

        Ok(Self {
            client: jcode_provider_core::shared_http_client(),
            model: Arc::new(RwLock::new(model)),
            reasoning_effort: Arc::new(RwLock::new(Self::initial_reasoning_effort(
                None,
                profile_id.as_deref(),
            ))),
            api_base,
            auth,
            supports_provider_features,
            supports_model_catalog,
            profile_id,
            reasoning_effort_support: None,
            max_tokens,
            extra_body,
            static_models,
            static_context_limits,
            static_image_input_support: HashMap::new(),
            send_openrouter_headers,
            models_cache: Arc::new(RwLock::new(ModelsCache::default())),
            model_catalog_refresh: Arc::new(Mutex::new(ModelCatalogRefreshState::default())),
            provider_routing: Arc::new(RwLock::new(provider_routing)),
            provider_pin: Arc::new(Mutex::new(None)),
            endpoints_cache: Arc::new(RwLock::new(HashMap::new())),
            endpoint_refresh: Arc::new(Mutex::new(EndpointRefreshTracker::default())),
        })
    }

    pub fn new_openrouter_api_key_runtime() -> Result<Self> {
        let api_key = load_api_key_from_env_or_config(DEFAULT_API_KEY_NAME, DEFAULT_ENV_FILE)
            .ok_or_else(|| {
                let path = jcode_base::storage::app_config_dir()
                    .map(|dir| dir.join(DEFAULT_ENV_FILE).display().to_string())
                    .unwrap_or_else(|_| DEFAULT_ENV_FILE.to_string());
                anyhow::anyhow!(
                    "{} not found in environment or {}",
                    DEFAULT_API_KEY_NAME,
                    path
                )
            })?;

        Ok(Self {
            client: jcode_provider_core::shared_http_client(),
            model: Arc::new(RwLock::new(DEFAULT_MODEL.to_string())),
            reasoning_effort: Arc::new(RwLock::new(None)),
            api_base: DEFAULT_API_BASE.to_string(),
            auth: ProviderAuth::AuthorizationBearer {
                token: api_key,
                label: DEFAULT_API_KEY_NAME.to_string(),
            },
            supports_provider_features: true,
            supports_model_catalog: true,
            profile_id: None,
            reasoning_effort_support: None,
            max_tokens: Self::configured_max_tokens(None),
            extra_body: Self::resolve_extra_body(None, DEFAULT_ENV_FILE),
            static_models: Vec::new(),
            static_context_limits: HashMap::new(),
            static_image_input_support: HashMap::new(),
            send_openrouter_headers: true,
            models_cache: Arc::new(RwLock::new(ModelsCache::default())),
            model_catalog_refresh: Arc::new(Mutex::new(ModelCatalogRefreshState::default())),
            provider_routing: Arc::new(RwLock::new(Self::parse_provider_routing())),
            provider_pin: Arc::new(Mutex::new(None)),
            endpoints_cache: Arc::new(RwLock::new(HashMap::new())),
            endpoint_refresh: Arc::new(Mutex::new(EndpointRefreshTracker::default())),
        })
    }

    pub fn new_openai_compatible_profile_runtime(
        profile: jcode_base::provider_catalog::OpenAiCompatibleProfile,
    ) -> Result<Self> {
        let resolved = resolve_openai_compatible_profile(profile);
        let api_base = normalize_api_base(&resolved.api_base).ok_or_else(|| {
            anyhow::anyhow!(
                "OpenAI-compatible profile '{}' has invalid API base '{}'",
                resolved.id,
                resolved.api_base
            )
        })?;
        let auth = match load_api_key_from_env_or_config(&resolved.api_key_env, &resolved.env_file)
        {
            Some(token) => ProviderAuth::AuthorizationBearer {
                token,
                label: resolved.api_key_env.clone(),
            },
            None if !resolved.requires_api_key => ProviderAuth::None {
                label: "local endpoint (no auth)".to_string(),
            },
            None => {
                let path = jcode_base::storage::app_config_dir()
                    .map(|dir| dir.join(&resolved.env_file).display().to_string())
                    .unwrap_or_else(|_| resolved.env_file.clone());
                anyhow::bail!(
                    "{} credentials not available. {} not found in environment or {}. Run `jcode login --provider {}` first.",
                    resolved.display_name,
                    resolved.api_key_env,
                    path,
                    resolved.id,
                );
            }
        };

        let static_context_limits = openai_compatible_profile_static_context_limits(profile);
        let static_models = openai_compatible_profile_static_models(profile);
        let model = resolved
            .default_model
            .clone()
            .unwrap_or_else(|| DEFAULT_MODEL.to_string());

        Ok(Self {
            client: jcode_provider_core::shared_http_client(),
            model: Arc::new(RwLock::new(model)),
            reasoning_effort: Arc::new(RwLock::new(Self::initial_reasoning_effort(
                None,
                Some(&resolved.id),
            ))),
            api_base,
            auth,
            supports_provider_features: false,
            supports_model_catalog: true,
            profile_id: Some(resolved.id.clone()),
            reasoning_effort_support: None,
            max_tokens: Self::configured_max_tokens(Some(&resolved.id)),
            extra_body: Self::resolve_extra_body(None, &resolved.env_file),
            static_models,
            static_context_limits,
            static_image_input_support: HashMap::new(),
            send_openrouter_headers: false,
            models_cache: Arc::new(RwLock::new(ModelsCache::default())),
            model_catalog_refresh: Arc::new(Mutex::new(ModelCatalogRefreshState::default())),
            provider_routing: Arc::new(RwLock::new(ProviderRouting::default())),
            provider_pin: Arc::new(Mutex::new(None)),
            endpoints_cache: Arc::new(RwLock::new(HashMap::new())),
            endpoint_refresh: Arc::new(Mutex::new(EndpointRefreshTracker::default())),
        })
    }

    fn should_background_refresh_model_catalog(&self, cache_age_secs: u64) -> bool {
        if cache_age_secs < MODEL_CATALOG_SOFT_REFRESH_SECS {
            return false;
        }

        let Some(now) = current_unix_secs() else {
            return false;
        };

        let Ok(state) = self.model_catalog_refresh.lock() else {
            return false;
        };

        if state.in_flight {
            return false;
        }

        state
            .last_attempt_unix
            .map(|last| now.saturating_sub(last) >= MODEL_CATALOG_REFRESH_RETRY_SECS)
            .unwrap_or(true)
    }

    pub(crate) fn should_merge_static_models_with_live_catalog(&self) -> bool {
        // Built-in OpenAI-compatible provider profiles use `static_models` as a
        // startup/pre-catalog fallback so `/model` is useful immediately after
        // login. Once a live `/models` catalog has been fetched, the live catalog
        // is more authoritative for access control. Keeping built-in fallback
        // entries after a successful fetch can advertise preview/stale models that
        // the provider rejects at chat time, which is especially confusing for
        // direct providers such as Cerebras.
        //
        // Preserve static models for OpenRouter itself and for custom/named
        // profiles, where the user supplied the list explicitly and there may be
        // no provider-side catalog contract.
        self.supports_provider_features || self.profile_id.is_none()
    }

    pub(crate) fn filter_profile_chat_supported_models(&self, models: Vec<String>) -> Vec<String> {
        let Some(profile_id) = self.profile_id.as_deref() else {
            return models;
        };

        models
            .into_iter()
            .filter(|model| {
                jcode_base::provider_catalog::openai_compatible_profile_model_supports_chat(
                    profile_id, model,
                )
            })
            .collect()
    }

    fn model_disk_cache_source_matches(
        &self,
        cache_entry: &jcode_provider_openrouter::DiskCache,
    ) -> bool {
        let Some(source_api_base) = cache_entry
            .source_api_base
            .as_deref()
            .and_then(normalize_api_base)
        else {
            // Legacy cache files did not record which endpoint produced the
            // catalog. They are acceptable for real OpenRouter catalogs, but
            // not for direct OpenAI-compatible profiles: a process-wide cache
            // namespace can leave an OpenRouter catalog under a profile such as
            // `chutes`, which then makes every picker row look like that direct
            // provider.
            return self.supports_provider_features;
        };

        source_api_base == self.api_base
    }

    pub(crate) fn load_usable_model_disk_cache_entry(
        &self,
    ) -> Option<jcode_provider_openrouter::DiskCache> {
        load_disk_cache_entry().filter(|entry| self.model_disk_cache_source_matches(entry))
    }

    fn begin_background_model_catalog_refresh(&self) -> bool {
        let Some(now) = current_unix_secs() else {
            return false;
        };

        let Ok(mut state) = self.model_catalog_refresh.lock() else {
            return false;
        };

        if state.in_flight {
            return false;
        }

        if let Some(last) = state.last_attempt_unix
            && now.saturating_sub(last) < MODEL_CATALOG_REFRESH_RETRY_SECS
        {
            return false;
        }

        state.in_flight = true;
        state.last_attempt_unix = Some(now);
        true
    }

    fn finish_background_model_catalog_refresh(
        refresh_state: &Arc<Mutex<ModelCatalogRefreshState>>,
    ) {
        if let Ok(mut state) = refresh_state.lock() {
            state.in_flight = false;
        }
    }

    fn begin_background_endpoint_refresh(&self, model: &str) -> bool {
        let Some(now) = current_unix_secs() else {
            return false;
        };

        let Ok(mut state) = self.endpoint_refresh.lock() else {
            return false;
        };
        let Ok(mut global_state) = global_endpoint_refresh().lock() else {
            return false;
        };

        if state.in_flight.contains(model) {
            return false;
        }
        if global_state.in_flight.len() >= MAX_BACKGROUND_ENDPOINT_REFRESHES {
            return false;
        }
        if global_state.in_flight.contains(model) {
            return false;
        }

        if let Some(last) = state.last_attempt_unix.get(model)
            && now.saturating_sub(*last) < MODEL_CATALOG_REFRESH_RETRY_SECS
        {
            return false;
        }
        if let Some(last) = global_state.last_attempt_unix.get(model)
            && now.saturating_sub(*last) < MODEL_CATALOG_REFRESH_RETRY_SECS
        {
            return false;
        }

        state.in_flight.insert(model.to_string());
        state.last_attempt_unix.insert(model.to_string(), now);
        global_state.in_flight.insert(model.to_string());
        global_state
            .last_attempt_unix
            .insert(model.to_string(), now);
        true
    }

    fn finish_background_endpoint_refresh(
        refresh_state: &Arc<Mutex<EndpointRefreshTracker>>,
        model: &str,
    ) {
        if let Ok(mut state) = refresh_state.lock() {
            state.in_flight.remove(model);
        }
        if let Ok(mut global_state) = global_endpoint_refresh().lock() {
            global_state.in_flight.remove(model);
        }
    }

    fn maybe_schedule_endpoint_refresh(
        &self,
        model: &str,
        cache_age_secs: Option<u64>,
        context: &'static str,
        notify_models_updated: bool,
    ) -> bool {
        if !self.supports_provider_features || !self.supports_model_catalog {
            return false;
        }

        if matches!(cache_age_secs, Some(age) if age < ENDPOINTS_CACHE_TTL_SECS) {
            return false;
        }

        if !self.begin_background_endpoint_refresh(model) {
            return false;
        }

        let Ok(handle) = tokio::runtime::Handle::try_current() else {
            Self::finish_background_endpoint_refresh(&self.endpoint_refresh, model);
            return false;
        };

        let client = self.client.clone();
        let api_base = self.api_base.clone();
        let auth = self.auth.clone();
        let model_name = model.to_string();
        let refresh_state = Arc::clone(&self.endpoint_refresh);
        let endpoints_cache = Arc::clone(&self.endpoints_cache);
        let previous_fingerprint = self.cached_endpoints_fingerprint(model);

        handle.spawn(async move {
            let provider = OpenRouterProvider {
                client,
                model: Arc::new(RwLock::new(model_name.clone())),
                reasoning_effort: Arc::new(RwLock::new(None)),
                api_base,
                auth,
                supports_provider_features: true,
                supports_model_catalog: true,
                profile_id: None,
                reasoning_effort_support: None,
                max_tokens: None,
                extra_body: None,
                static_models: Vec::new(),
                static_context_limits: HashMap::new(),
                static_image_input_support: HashMap::new(),
                send_openrouter_headers: true,
                models_cache: Arc::new(RwLock::new(ModelsCache::default())),
                model_catalog_refresh: Arc::new(Mutex::new(ModelCatalogRefreshState::default())),
                provider_routing: Arc::new(RwLock::new(ProviderRouting::default())),
                provider_pin: Arc::new(Mutex::new(None)),
                endpoints_cache,
                endpoint_refresh: Arc::clone(&refresh_state),
            };

            match provider.fetch_endpoints(&model_name).await {
                Ok(endpoints) => {
                    let updated = endpoints_fingerprint(&endpoints) != previous_fingerprint;
                    if notify_models_updated && updated {
                        jcode_base::logging::info(&format!(
                            "Refreshed OpenRouter endpoint providers in background ({}): {} via {} providers",
                            context,
                            model_name,
                            endpoints.len()
                        ));
                        jcode_base::bus::Bus::global().publish_models_updated();
                    } else if updated {
                        jcode_base::logging::info(&format!(
                            "Refreshed OpenRouter endpoint providers in background without broadcast ({}): {} via {} providers",
                            context,
                            model_name,
                            endpoints.len()
                        ));
                    } else {
                        jcode_base::logging::info(&format!(
                            "OpenRouter endpoint refresh produced no material change ({}): {}",
                            context, model_name
                        ));
                    }
                }
                Err(error) => jcode_base::logging::info(&format!(
                    "Failed to refresh OpenRouter endpoint providers in background ({}): {} ({})",
                    context, model_name, error
                )),
            }

            OpenRouterProvider::finish_background_endpoint_refresh(&refresh_state, &model_name);
        });

        true
    }

    fn maybe_schedule_model_catalog_refresh(&self, cache_age_secs: u64, context: &'static str) {
        if !self.should_background_refresh_model_catalog(cache_age_secs)
            || !self.begin_background_model_catalog_refresh()
        {
            return;
        }

        let Ok(handle) = tokio::runtime::Handle::try_current() else {
            Self::finish_background_model_catalog_refresh(&self.model_catalog_refresh);
            return;
        };

        let client = self.client.clone();
        let api_base = self.api_base.clone();
        let auth = self.auth.clone();
        let models_cache = Arc::clone(&self.models_cache);
        let refresh_state = Arc::clone(&self.model_catalog_refresh);
        let previous_fingerprint = self.cached_model_catalog_fingerprint();

        handle.spawn(async move {
            match fetch_models_from_api(client, api_base, auth, models_cache, None).await {
                Ok(models) => {
                    let updated = models_fingerprint(&models) != previous_fingerprint;
                    if updated {
                        jcode_base::logging::info(&format!(
                            "Refreshed OpenRouter model catalog in background ({}): {} models",
                            context,
                            models.len()
                        ));
                        jcode_base::bus::Bus::global().publish_models_updated();
                    } else {
                        jcode_base::logging::info(&format!(
                            "OpenRouter model catalog refresh produced no material change ({}): {} models",
                            context,
                            models.len()
                        ));
                    }
                }
                Err(e) => jcode_base::logging::info(&format!(
                    "Failed to refresh OpenRouter model catalog in background ({}): {}",
                    context, e
                )),
            }
            OpenRouterProvider::finish_background_model_catalog_refresh(&refresh_state);
        });
    }

    /// Parse provider routing configuration from environment variables
    fn parse_provider_routing() -> ProviderRouting {
        jcode_provider_openrouter::parse_provider_routing_from_env()
    }

    fn set_explicit_pin(&self, model: &str, provider: ParsedProvider) {
        let mut pin = self
            .provider_pin
            .lock()
            .unwrap_or_else(|poisoned| poisoned.into_inner());
        *pin = Some(ProviderPin {
            model: model.to_string(),
            provider: provider.name,
            source: PinSource::Explicit,
            allow_fallbacks: provider.allow_fallbacks,
            last_cache_read: None,
        });
    }

    fn clear_pin_if_model_changed(&self, model: &str, clear_explicit: bool) {
        let mut pin = self
            .provider_pin
            .lock()
            .unwrap_or_else(|poisoned| poisoned.into_inner());
        if let Some(existing) = pin.as_ref() {
            let should_clear = existing.model != model
                || (clear_explicit && existing.source == PinSource::Explicit);
            if should_clear {
                *pin = None;
            }
        }
    }

    pub fn explicit_provider_pin_for_current_model(&self) -> Option<String> {
        if !self.supports_provider_features {
            return None;
        }

        let model = self.model.try_read().ok()?.clone();
        self.provider_pin
            .lock()
            .unwrap_or_else(|poisoned| poisoned.into_inner())
            .as_ref()
            .filter(|pin| pin.model == model && pin.source == PinSource::Explicit)
            .map(|pin| pin.provider.clone())
    }

    fn rank_providers_from_endpoints(endpoints: &[EndpointInfo]) -> Vec<String> {
        jcode_provider_openrouter::rank_providers_from_endpoints(endpoints)
    }

    async fn effective_routing(&self, model: &str) -> ProviderRouting {
        if !self.supports_provider_features {
            return ProviderRouting::default();
        }

        let base = self.provider_routing.read().await.clone();
        let pin = self
            .provider_pin
            .lock()
            .unwrap_or_else(|poisoned| poisoned.into_inner())
            .clone();

        if let Some(pin) = pin
            && pin.model == model
        {
            // Once OpenRouter has actually served a request for this model from a
            // concrete provider, stick to that provider for the rest of the
            // session. Re-selecting a provider per request would route to a
            // backend with a cold KV cache, so we pin the observed provider and
            // disable fallbacks to keep prompt-prefix caching warm.
            let use_pin = match pin.source {
                PinSource::Explicit => true,
                // Honor an explicit user-configured order only when the user
                // actively narrowed routing themselves; otherwise the observed
                // session provider wins so the cache stays warm.
                PinSource::Observed => base.order.is_none(),
            };

            if use_pin {
                let mut routing = base.clone();
                routing.order = Some(vec![pin.provider.clone()]);
                // Pin hard: an explicit pin honors its own fallback preference,
                // an observed (session) pin always disables fallbacks so every
                // turn reuses the same upstream provider and its KV cache.
                match pin.source {
                    PinSource::Explicit if pin.allow_fallbacks => {}
                    _ => routing.allow_fallbacks = false,
                }
                return routing;
            }
        }

        if base.order.is_some() {
            return base;
        }

        let ranked = {
            let mut endpoints = load_endpoints_disk_cache(model).or_else(|| {
                let cache = self.endpoints_cache.try_read().ok()?;
                cache.get(model).map(|(_, eps)| eps.clone())
            });

            // Fetch endpoints from API if no cache available
            if endpoints.is_none()
                && let Ok(fetched) = self.fetch_endpoints(model).await
                && !fetched.is_empty()
            {
                endpoints = Some(fetched);
            }

            Self::rank_providers_from_endpoints(&endpoints.unwrap_or_default())
        };
        if !ranked.is_empty() {
            let mut routing = base.clone();
            routing.order = Some(ranked);
            return routing;
        }

        if Self::is_kimi_model(model) {
            let mut routing = base.clone();
            routing.order = Some(
                KIMI_FALLBACK_PROVIDERS
                    .iter()
                    .map(|p| (*p).to_string())
                    .collect(),
            );
            routing.allow_fallbacks = false;
            return routing;
        }

        let mut routing = base.clone();
        if routing.sort.is_none() {
            routing.sort = Some("throughput".to_string());
        }
        routing
    }

    /// Set provider routing at runtime
    pub async fn set_provider_routing(&self, routing: ProviderRouting) {
        if !self.supports_provider_features {
            return;
        }
        let mut current = self.provider_routing.write().await;
        *current = routing;
    }

    /// Get current provider routing
    pub async fn get_provider_routing(&self) -> ProviderRouting {
        self.provider_routing.read().await.clone()
    }

    /// Return the currently preferred provider for display.
    /// Returns the pinned provider if set, otherwise the top-ranked provider from endpoint data.
    pub fn preferred_provider(&self) -> Option<String> {
        if !self.supports_provider_features {
            return None;
        }

        let model = self.model.try_read().ok()?.clone();

        // Check pin first
        let pin = self
            .provider_pin
            .lock()
            .unwrap_or_else(|poisoned| poisoned.into_inner());
        if let Some(ref pin) = *pin
            && pin.model == model
        {
            return Some(pin.provider.clone());
        }

        // Check explicit routing
        if let Ok(routing) = self.provider_routing.try_read()
            && let Some(ref order) = routing.order
            && let Some(first) = order.first()
        {
            return Some(first.clone());
        }

        // Fall back to ranked endpoint data
        let endpoints = load_endpoints_disk_cache(&model).or_else(|| {
            self.endpoints_cache
                .try_read()
                .ok()?
                .get(&model)
                .map(|(_, eps)| eps.clone())
        });

        if let Some(ref eps) = endpoints {
            let ranked = Self::rank_providers_from_endpoints(eps);
            if let Some(first) = ranked.into_iter().next() {
                return Some(first);
            }
        }

        // For Kimi models, use the hardcoded fallback order
        if Self::is_kimi_model(&model) {
            return KIMI_FALLBACK_PROVIDERS.first().map(|s| s.to_string());
        }

        None
    }

    /// Return a list of known/observed providers for a model (for autocomplete).
    pub fn available_providers_for_model(&self, model: &str) -> Vec<String> {
        if !self.supports_provider_features {
            return Vec::new();
        }

        let mut providers: Vec<String> = Vec::new();

        if let Some(endpoints) = load_endpoints_disk_cache(model) {
            providers.extend(endpoints.into_iter().map(|e| e.provider_name));
        } else if let Ok(cache) = self.endpoints_cache.try_read()
            && let Some((_, endpoints)) = cache.get(model)
        {
            providers.extend(endpoints.iter().map(|e| e.provider_name.clone()));
        }

        if providers.is_empty() {
            self.maybe_schedule_endpoint_refresh(
                model,
                None,
                "provider autocomplete cache miss",
                false,
            );
            providers = known_providers();
        } else if let Some((_, age)) = load_endpoints_disk_cache_public(model) {
            self.maybe_schedule_endpoint_refresh(
                model,
                Some(age),
                "provider autocomplete stale cache",
                false,
            );
        }

        providers.sort();
        providers.dedup();
        providers
    }

    /// Return provider details from cached endpoints data (sync, no network).
    pub fn provider_details_for_model(&self, model: &str) -> Vec<(String, String)> {
        if !self.supports_provider_features {
            return Vec::new();
        }

        // Try endpoints disk cache first (has pricing, uptime, cache info)
        if let Some(endpoints) = load_endpoints_disk_cache(model) {
            if let Some((_, age)) = load_endpoints_disk_cache_public(model) {
                self.maybe_schedule_endpoint_refresh(
                    model,
                    Some(age),
                    "provider details stale cache",
                    false,
                );
            }
            return endpoints
                .iter()
                .map(|e| (e.provider_name.clone(), e.detail_string()))
                .collect();
        }

        // Try in-memory endpoints cache
        if let Ok(cache) = self.endpoints_cache.try_read()
            && let Some((_, endpoints)) = cache.get(model)
        {
            return endpoints
                .iter()
                .map(|e| (e.provider_name.clone(), e.detail_string()))
                .collect();
        }

        self.maybe_schedule_endpoint_refresh(model, None, "provider details cache miss", false);

        Vec::new()
    }

    pub fn maybe_schedule_endpoint_refresh_for_display(
        &self,
        model: &str,
        cache_age_secs: Option<u64>,
        context: &'static str,
    ) -> bool {
        self.maybe_schedule_endpoint_refresh(model, cache_age_secs, context, false)
    }

    fn cached_model_catalog_fingerprint(&self) -> String {
        if let Ok(cache) = self.models_cache.try_read()
            && cache.fetched
        {
            return models_fingerprint(&cache.models);
        }
        if let Some(cache_entry) = self.load_usable_model_disk_cache_entry() {
            return models_fingerprint(&cache_entry.models);
        }
        String::new()
    }

    pub(crate) fn cached_live_model_ids_for_display(&self) -> Option<HashSet<String>> {
        if let Ok(cache) = self.models_cache.try_read()
            && cache.fetched
            && !cache.models.is_empty()
        {
            return Some(cache.models.iter().map(|model| model.id.clone()).collect());
        }

        self.load_usable_model_disk_cache_entry().and_then(|entry| {
            if entry.models.is_empty() {
                None
            } else {
                Some(entry.models.into_iter().map(|model| model.id).collect())
            }
        })
    }

    fn cached_endpoints_fingerprint(&self, model: &str) -> String {
        if let Some(endpoints) = load_endpoints_disk_cache(model) {
            return endpoints_fingerprint(&endpoints);
        }
        if let Ok(cache) = self.endpoints_cache.try_read()
            && let Some((_, endpoints)) = cache.get(model)
        {
            return endpoints_fingerprint(endpoints);
        }
        String::new()
    }

    /// Check if OPENROUTER_API_KEY is available (env var or config file)
    pub fn has_credentials() -> bool {
        if matches!(
            configured_dynamic_bearer_provider().as_deref(),
            Some("azure")
        ) {
            return jcode_base::auth::azure::has_configuration();
        }
        if configured_allow_no_auth() {
            return true;
        }
        Self::get_api_key().is_some()
    }

    fn resolve_auth() -> Result<ProviderAuth> {
        if let Some(provider) = configured_dynamic_bearer_provider() {
            return match provider.as_str() {
                "azure" => {
                    if jcode_base::auth::azure::has_configuration() {
                        Ok(ProviderAuth::AzureEntra {
                            label: "Azure OpenAI Entra ID".to_string(),
                        })
                    } else {
                        anyhow::bail!(
                            "Azure OpenAI is configured for Entra ID, but Azure settings are incomplete. Run `jcode login --provider azure`."
                        )
                    }
                }
                other => anyhow::bail!(
                    "Unsupported JCODE_OPENROUTER_DYNAMIC_BEARER_PROVIDER '{}'.",
                    other
                ),
            };
        }

        if configured_allow_no_auth() {
            if let Some(api_key) = Self::get_api_key() {
                let key_name = configured_api_key_name();
                return Ok(match configured_auth_header_mode() {
                    AuthHeaderMode::AuthorizationBearer => ProviderAuth::AuthorizationBearer {
                        token: api_key,
                        label: key_name,
                    },
                    AuthHeaderMode::ApiKey => ProviderAuth::HeaderValue {
                        header_name: configured_auth_header_name(),
                        value: api_key,
                        label: key_name,
                    },
                });
            }
            return Ok(ProviderAuth::None {
                label: "local endpoint (no auth)".to_string(),
            });
        }

        let key_name = configured_api_key_name();
        let api_key = Self::get_api_key().ok_or_else(|| {
            let env_file = configured_env_file_name();
            let path = jcode_base::storage::app_config_dir()
                .map(|dir| dir.join(&env_file).display().to_string())
                .unwrap_or_else(|_| env_file.clone());
            anyhow::anyhow!("{} not found in environment or {}", key_name, path)
        })?;

        Ok(match configured_auth_header_mode() {
            AuthHeaderMode::AuthorizationBearer => ProviderAuth::AuthorizationBearer {
                token: api_key,
                label: key_name,
            },
            AuthHeaderMode::ApiKey => ProviderAuth::HeaderValue {
                header_name: configured_auth_header_name(),
                value: api_key,
                label: key_name,
            },
        })
    }

    /// Get API key from environment or config file
    fn get_api_key() -> Option<String> {
        let key_name = configured_api_key_name();
        let env_file = configured_env_file_name();
        load_api_key_from_env_or_config(&key_name, &env_file)
    }

    /// Fetch available models from OpenRouter API (with disk caching)
    pub async fn fetch_models(&self) -> Result<Vec<ModelInfo>> {
        if !self.supports_model_catalog {
            return Ok(Vec::new());
        }

        // Check in-memory cache first
        {
            let cache = self.models_cache.read().await;
            if cache.fetched {
                if let Some(cached_at) = cache
                    .cached_at
                    .and_then(|t| current_unix_secs().map(|now| now.saturating_sub(t)))
                {
                    self.maybe_schedule_model_catalog_refresh(cached_at, "memory cache");
                }
                return Ok(cache.models.clone());
            }
        }

        // Check disk cache
        if let Some(cache_entry) = self.load_usable_model_disk_cache_entry() {
            let cache_age = current_unix_secs()
                .map(|now| now.saturating_sub(cache_entry.cached_at))
                .unwrap_or(0);
            let mut cache = self.models_cache.write().await;
            cache.models = cache_entry.models.clone();
            cache.fetched = true;
            cache.cached_at = Some(cache_entry.cached_at);
            drop(cache);
            self.maybe_schedule_model_catalog_refresh(cache_age, "disk cache");
            return Ok(cache_entry.models);
        }

        fetch_models_from_api(
            self.client.clone(),
            self.api_base.clone(),
            self.auth.clone(),
            Arc::clone(&self.models_cache),
            None,
        )
        .await
    }

    /// Force refresh the models cache from API
    pub async fn refresh_models(&self) -> Result<Vec<ModelInfo>> {
        fetch_models_from_api(
            self.client.clone(),
            self.api_base.clone(),
            self.auth.clone(),
            Arc::clone(&self.models_cache),
            None,
        )
        .await
    }

    /// Fetch per-provider endpoint data for a model from OpenRouter API.
    /// Returns cached data if available and fresh (1-hour TTL).
    pub async fn fetch_endpoints(&self, model: &str) -> Result<Vec<EndpointInfo>> {
        if !self.supports_provider_features || !self.supports_model_catalog {
            return Ok(Vec::new());
        }

        let now = std::time::SystemTime::now()
            .duration_since(std::time::UNIX_EPOCH)
            .map(|d| d.as_secs())
            .unwrap_or(0);

        // Check in-memory cache
        {
            let cache = self.endpoints_cache.read().await;
            if let Some((cached_at, endpoints)) = cache.get(model)
                && now - cached_at < ENDPOINTS_CACHE_TTL_SECS
            {
                return Ok(endpoints.clone());
            }
        }

        // Check disk cache
        if let Some(endpoints) = load_endpoints_disk_cache(model) {
            let mut cache = self.endpoints_cache.write().await;
            cache.insert(model.to_string(), (now, endpoints.clone()));
            return Ok(endpoints);
        }

        // Fetch from API
        let url = format!("{}/models/{}/endpoints", self.api_base, model);
        let response = self
            .auth
            .apply(self.client.get(&url))
            .await?
            .send()
            .await
            .context("Failed to fetch endpoint data")?;

        if !response.status().is_success() {
            let status = response.status();
            let body = jcode_base::util::http_error_body(response, "HTTP error").await;
            anyhow::bail!("Endpoints API error ({}): {}", status, body);
        }

        #[derive(Deserialize)]
        struct EndpointsWrapper {
            endpoints: Vec<EndpointInfo>,
        }

        #[derive(Deserialize)]
        struct EndpointsResponse {
            data: EndpointsWrapper,
        }

        let resp: EndpointsResponse = response
            .json()
            .await
            .context("Failed to parse endpoints response")?;

        let endpoints = resp.data.endpoints;

        // Save to disk cache
        save_endpoints_disk_cache(model, &endpoints);

        // Update in-memory cache
        {
            let mut cache = self.endpoints_cache.write().await;
            cache.insert(model.to_string(), (now, endpoints.clone()));
        }

        Ok(endpoints)
    }

    /// Force refresh per-provider endpoint data for a model from the API.
    pub async fn refresh_endpoints(&self, model: &str) -> Result<Vec<EndpointInfo>> {
        if !self.supports_provider_features || !self.supports_model_catalog {
            return Ok(Vec::new());
        }

        let now = std::time::SystemTime::now()
            .duration_since(std::time::UNIX_EPOCH)
            .map(|d| d.as_secs())
            .unwrap_or(0);

        let url = format!("{}/models/{}/endpoints", self.api_base, model);
        let response = self
            .auth
            .apply(self.client.get(&url))
            .await?
            .send()
            .await
            .context("Failed to refresh endpoint data")?;

        if !response.status().is_success() {
            let status = response.status();
            let body = jcode_base::util::http_error_body(response, "HTTP error").await;
            anyhow::bail!("Endpoints API error ({}): {}", status, body);
        }

        #[derive(Deserialize)]
        struct EndpointsWrapper {
            endpoints: Vec<EndpointInfo>,
        }

        #[derive(Deserialize)]
        struct EndpointsResponse {
            data: EndpointsWrapper,
        }

        let resp: EndpointsResponse = response
            .json()
            .await
            .context("Failed to parse endpoints response")?;

        let endpoints = resp.data.endpoints;
        save_endpoints_disk_cache(model, &endpoints);

        let mut cache = self.endpoints_cache.write().await;
        cache.insert(model.to_string(), (now, endpoints.clone()));

        Ok(endpoints)
    }

    /// Get context length for a model
    pub async fn context_length_for_model(&self, model_id: &str) -> Option<u64> {
        if let Ok(models) = self.fetch_models().await {
            models
                .iter()
                .find(|m| m.id == model_id)
                .and_then(|m| m.context_length)
        } else {
            None
        }
    }

    async fn model_pricing(&self, model_id: &str) -> Option<ModelPricing> {
        let cache = self.models_cache.read().await;
        if cache.fetched
            && let Some(model) = cache.models.iter().find(|m| m.id == model_id)
        {
            return Some(model.pricing.clone());
        }

        if let Some(cache_entry) = self.load_usable_model_disk_cache_entry() {
            let models = cache_entry.models;
            let pricing = models
                .iter()
                .find(|m| m.id == model_id)
                .map(|m| m.pricing.clone());
            if pricing.is_some() {
                if let Ok(mut cache) = self.models_cache.try_write() {
                    cache.models = models;
                    cache.fetched = true;
                }
                return pricing;
            }
        }

        if let Ok(models) = self.fetch_models().await
            && let Some(model) = models.iter().find(|m| m.id == model_id)
        {
            return Some(model.pricing.clone());
        }

        None
    }

    async fn model_supports_cache(&self, model_id: &str) -> bool {
        // Check model-level pricing first
        if let Some(pricing) = self.model_pricing(model_id).await {
            let has_cache_read = pricing
                .input_cache_read
                .as_deref()
                .and_then(|v| v.parse::<f64>().ok())
                .unwrap_or(0.0)
                > 0.0;
            let has_cache_write = pricing
                .input_cache_write
                .as_deref()
                .and_then(|v| v.parse::<f64>().ok())
                .unwrap_or(0.0)
                > 0.0;
            if has_cache_read || has_cache_write {
                return true;
            }
        }

        // Check per-provider endpoint data (any provider supporting cache is enough)
        let endpoints = load_endpoints_disk_cache(model_id).or_else(|| {
            self.endpoints_cache
                .try_read()
                .ok()?
                .get(model_id)
                .map(|(_, eps)| eps.clone())
        });
        if let Some(endpoints) = endpoints {
            return endpoints.iter().any(|e| {
                e.supports_implicit_caching == Some(true)
                    || e.pricing
                        .input_cache_read
                        .as_deref()
                        .and_then(|v| v.parse::<f64>().ok())
                        .unwrap_or(0.0)
                        > 0.0
            });
        }

        false
    }
}

#[path = "openrouter_provider_impl.rs"]
mod openrouter_provider_impl;
#[path = "openrouter_sse_stream.rs"]
mod openrouter_sse_stream;

#[cfg(test)]
#[allow(clippy::await_holding_lock)]
#[path = "openrouter_tests.rs"]
mod tests;
