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

use super::{EventStream, Provider};
use crate::message::{
    CacheControl, ContentBlock, Message, Role, StreamEvent, TOOL_OUTPUT_MISSING_TEXT,
    ToolDefinition,
};
use crate::provider_catalog::{
    OPENAI_COMPAT_PROFILE, is_safe_env_file_name, is_safe_env_key_name,
    load_api_key_from_env_or_config, normalize_api_base, openai_compatible_profile_by_id,
    openai_compatible_profile_id_for_api_base, openai_compatible_profile_static_context_limits,
    openai_compatible_profile_static_models, openai_compatible_profiles,
    resolve_openai_compatible_profile,
};
use anyhow::{Context, Result};
use async_trait::async_trait;
use bytes::Bytes;
use futures::{Stream, StreamExt};
pub use jcode_provider_openrouter::{
    EndpointInfo, ModelInfo, ModelPricing, ModelTimestampIndex, ProviderRouting,
    all_model_timestamps, load_endpoints_disk_cache_public, load_model_pricing_disk_cache_public,
    load_model_timestamp_index, model_created_timestamp, model_created_timestamp_from_index,
};
use jcode_provider_openrouter::{
    KIMI_FALLBACK_PROVIDERS, ModelCatalogRefreshState, ModelsCache, ParsedProvider, PinSource,
    ProviderPin, current_unix_secs, known_providers, load_disk_cache, load_disk_cache_entry,
    load_endpoints_disk_cache, parse_model_spec, save_disk_cache, save_endpoints_disk_cache,
};
use reqwest::Client;
use reqwest::header::HeaderName;
use serde::Deserialize;
use serde_json::Value;
use std::collections::{HashMap, HashSet, VecDeque};
use std::pin::Pin;
use std::sync::{Arc, Mutex, OnceLock};
use std::task::{Context as TaskContext, Poll};
use std::time::Instant;
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
/// Pin provider to preserve cache for this long after a cache hit
const CACHE_PIN_TTL_SECS: u64 = 60 * 60;

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
-> Option<crate::provider_catalog::ResolvedOpenAiCompatibleProfile> {
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
            if crate::provider_catalog::openai_compatible_profile_is_configured(*profile) {
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
        crate::logging::warn(&format!(
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
        crate::logging::warn(&format!(
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
        crate::logging::warn(&format!(
            "Ignoring invalid JCODE_OPENROUTER_ENV_FILE '{}'; using {}",
            raw, DEFAULT_ENV_FILE
        ));
        DEFAULT_ENV_FILE.to_string()
    }
}

fn load_named_profile_api_key(
    env_key: &str,
    profile: &crate::config::NamedProviderConfig,
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
        crate::logging::warn(&format!(
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
        crate::logging::warn(&format!(
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
            crate::logging::warn(&format!(
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
        crate::logging::warn(&format!(
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

fn is_kimi_for_coding_model(model: &str) -> bool {
    model.trim().eq_ignore_ascii_case("kimi-for-coding")
}

fn should_send_kimi_coding_agent_headers(api_base: &str, model: Option<&str>) -> bool {
    is_coding_agent_api_base(api_base) || model.map(is_kimi_for_coding_model).unwrap_or(false)
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

fn is_reserved_header_name(name: &HeaderName) -> bool {
    matches!(
        name.as_str(),
        "host"
            | "content-length"
            | "transfer-encoding"
            | "connection"
            | "keep-alive"
            | "te"
            | "trailer"
            | "upgrade"
            | "proxy-authenticate"
            | "proxy-authorization"
    )
}

pub(crate) fn parse_named_provider_extra_headers(
    profile_name: &str,
    profile: &crate::config::NamedProviderConfig,
) -> Vec<(HeaderName, String)> {
    let mut out = Vec::with_capacity(profile.headers.len());
    for (raw_name, raw_value) in &profile.headers {
        let trimmed_name = raw_name.trim();
        if trimmed_name.is_empty() {
            continue;
        }
        let header_name = match HeaderName::from_bytes(trimmed_name.as_bytes()) {
            Ok(name) => name,
            Err(_) => {
                crate::logging::warn(&format!(
                    "Provider profile '{}' has invalid custom header name '{}'; skipping",
                    profile_name, trimmed_name
                ));
                continue;
            }
        };
        if is_reserved_header_name(&header_name) {
            crate::logging::warn(&format!(
                "Provider profile '{}' tried to override reserved header '{}'; skipping",
                profile_name, trimmed_name
            ));
            continue;
        }
        if raw_value.contains('\r') || raw_value.contains('\n') {
            crate::logging::warn(&format!(
                "Provider profile '{}' header '{}' contains CR/LF; skipping",
                profile_name, trimmed_name
            ));
            continue;
        }
        out.push((header_name, raw_value.clone()));
    }
    out
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
                let token = crate::auth::azure::get_bearer_token().await?;
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
        let body = crate::util::http_error_body(response, "HTTP error").await;
        anyhow::bail!(
            "OpenAI-compatible model catalog request failed\n  endpoint: {}\n  auth: {}\n  status: {}\n  response: {}\nHint: verify the base URL includes the API version (usually /v1), the key is valid for this endpoint, and the provider supports GET /models.",
            url,
            auth.label(),
            status,
            body
        );
    }

    #[derive(Deserialize)]
    struct ModelsResponse {
        data: Vec<ModelInfo>,
    }

    let raw_body = response
        .text()
        .await
        .with_context(|| format!("Failed to read model catalog response body from {}", url))?;
    let models_response: ModelsResponse = serde_json::from_str(&raw_body).with_context(|| {
        format!(
            "Failed to parse OpenAI-compatible model catalog response\n  endpoint: {}\n  auth: {}\n  expected: JSON object with a `data` array of model objects containing at least `id`\n  response: {}",
            url,
            auth.label(),
            crate::util::truncate_str(&raw_body.trim().replace('\n', "\\n"), 1200)
        )
    })?;

    save_disk_cache(&models_response.data);

    if let Some(now) = current_unix_secs() {
        let mut cache = models_cache.write().await;
        cache.models = models_response.data.clone();
        cache.fetched = true;
        cache.cached_at = Some(now);
    } else {
        let mut cache = models_cache.write().await;
        cache.models = models_response.data.clone();
        cache.fetched = true;
    }

    Ok(models_response.data)
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

pub struct OpenRouterProvider {
    client: Client,
    model: Arc<RwLock<String>>,
    api_base: String,
    auth: ProviderAuth,
    supports_provider_features: bool,
    supports_model_catalog: bool,
    profile_id: Option<String>,
    static_models: Vec<String>,
    static_context_limits: HashMap<String, usize>,
    send_openrouter_headers: bool,
    extra_headers: Vec<(HeaderName, String)>,
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
    pub(crate) fn supports_provider_routing_features(&self) -> bool {
        self.supports_provider_features
    }

    pub fn new_named_openai_compatible(
        profile_name: &str,
        profile: &crate::config::NamedProviderConfig,
    ) -> Result<Self> {
        // The OpenRouter/OpenAI-compatible catalog cache helpers are currently
        // process-env scoped. Named provider profiles are constructed directly
        // in several CLI/TUI paths, so make sure their cache namespace is active
        // before any model-cache reads/writes happen. Without this, a custom
        // endpoint can accidentally display the default OpenRouter catalog.
        crate::env::set_var("JCODE_OPENROUTER_CACHE_NAMESPACE", profile_name);
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
            crate::config::NamedProviderAuth::None => ProviderAuth::None {
                label: "local endpoint (no auth)".to_string(),
            },
            crate::config::NamedProviderAuth::Bearer => ProviderAuth::AuthorizationBearer {
                token: key
                    .ok_or_else(|| anyhow::anyhow!("{} not found in environment", key_label))?,
                label: key_label,
            },
            crate::config::NamedProviderAuth::Header => ProviderAuth::HeaderValue {
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
        let extra_headers = parse_named_provider_extra_headers(profile_name, profile);
        Ok(Self {
            client: crate::provider::shared_http_client(),
            model: Arc::new(RwLock::new(model)),
            api_base,
            auth,
            supports_provider_features: matches!(
                profile.provider_type,
                crate::config::NamedProviderType::OpenRouter
            ) || profile.provider_routing
                || profile.allow_provider_pinning,
            supports_model_catalog: profile.model_catalog
                || matches!(
                    profile.provider_type,
                    crate::config::NamedProviderType::OpenRouter
                ),
            profile_id: Some(profile_name.to_string()),
            static_models,
            static_context_limits,
            send_openrouter_headers: false,
            extra_headers,
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
                crate::logging::info(&format!(
                    "Warning: Unsupported JCODE_OPENROUTER_THINKING '{}'; expected enabled/disabled/auto",
                    other
                ));
                None
            }
        }
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
            crate::env::set_var("JCODE_OPENROUTER_CACHE_NAMESPACE", &profile.id);
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

        Ok(Self {
            client: crate::provider::shared_http_client(),
            model: Arc::new(RwLock::new(model)),
            api_base,
            auth,
            supports_provider_features,
            supports_model_catalog,
            profile_id,
            static_models,
            static_context_limits,
            send_openrouter_headers,
            extra_headers: Vec::new(),
            models_cache: Arc::new(RwLock::new(ModelsCache::default())),
            model_catalog_refresh: Arc::new(Mutex::new(ModelCatalogRefreshState::default())),
            provider_routing: Arc::new(RwLock::new(provider_routing)),
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
                api_base,
                auth,
                supports_provider_features: true,
                supports_model_catalog: true,
                profile_id: None,
                static_models: Vec::new(),
                static_context_limits: HashMap::new(),
                send_openrouter_headers: true,
                extra_headers: Vec::new(),
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
                        crate::logging::info(&format!(
                            "Refreshed OpenRouter endpoint providers in background ({}): {} via {} providers",
                            context,
                            model_name,
                            endpoints.len()
                        ));
                        crate::bus::Bus::global().publish_models_updated();
                    } else if updated {
                        crate::logging::info(&format!(
                            "Refreshed OpenRouter endpoint providers in background without broadcast ({}): {} via {} providers",
                            context,
                            model_name,
                            endpoints.len()
                        ));
                    } else {
                        crate::logging::info(&format!(
                            "OpenRouter endpoint refresh produced no material change ({}): {}",
                            context, model_name
                        ));
                    }
                }
                Err(error) => crate::logging::info(&format!(
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
            match fetch_models_from_api(client, api_base, auth, models_cache).await {
                Ok(models) => {
                    let updated = models_fingerprint(&models) != previous_fingerprint;
                    if updated {
                        crate::logging::info(&format!(
                            "Refreshed OpenRouter model catalog in background ({}): {} models",
                            context,
                            models.len()
                        ));
                        crate::bus::Bus::global().publish_models_updated();
                    } else {
                        crate::logging::info(&format!(
                            "OpenRouter model catalog refresh produced no material change ({}): {} models",
                            context,
                            models.len()
                        ));
                    }
                }
                Err(e) => crate::logging::info(&format!(
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
            let cache_recent = pin
                .last_cache_read
                .map(|t| t.elapsed().as_secs() <= CACHE_PIN_TTL_SECS)
                .unwrap_or(false);
            let use_pin = match pin.source {
                PinSource::Explicit => true,
                PinSource::Observed => cache_recent || base.order.is_none(),
            };

            if use_pin {
                let mut routing = base.clone();
                routing.order = Some(vec![pin.provider.clone()]);
                if !pin.allow_fallbacks {
                    routing.allow_fallbacks = false;
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
        if let Some(cache_entry) = load_disk_cache_entry() {
            return models_fingerprint(&cache_entry.models);
        }
        String::new()
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
            return crate::auth::azure::has_configuration();
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
                    if crate::auth::azure::has_configuration() {
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
            let path = crate::storage::app_config_dir()
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
        if let Some(cache_entry) = load_disk_cache_entry() {
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
            let body = crate::util::http_error_body(response, "HTTP error").await;
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
            let body = crate::util::http_error_body(response, "HTTP error").await;
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

        if let Some(models) = load_disk_cache() {
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
#[path = "openrouter_tests.rs"]
mod tests;
