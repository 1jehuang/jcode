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
    is_safe_env_file_name, is_safe_env_key_name, load_api_key_from_env_or_config,
    normalize_api_base,
};
use anyhow::{Context, Result};
use async_trait::async_trait;
use bytes::Bytes;
use futures::{Stream, StreamExt};
use reqwest::Client;
use reqwest::header::HeaderName;
use serde::{Deserialize, Serialize};
use serde_json::Value;
use std::collections::{HashMap, HashSet, VecDeque};
use std::path::PathBuf;
use std::pin::Pin;
use std::sync::{Arc, Mutex};
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
const DEFAULT_CACHE_NAMESPACE: &str = "openrouter";

/// Default model (Claude Sonnet via OpenRouter)
const DEFAULT_MODEL: &str = "anthropic/claude-sonnet-4";
/// Default provider order for Kimi models when no local stats exist yet.
/// Ordered for practical coding use: speed first, then cache quality, then cost.
const KIMI_FALLBACK_PROVIDERS: &[&str] = &["Fireworks", "Moonshot AI", "Together", "DeepInfra"];
/// Known provider names for autocomplete when OpenRouter doesn't supply a list.
const KNOWN_PROVIDERS: &[&str] = &[
    "Moonshot AI",
    "OpenAI",
    "Anthropic",
    "Fireworks",
    "Together",
    "DeepInfra",
];
/// Short aliases to normalize provider input.
const PROVIDER_ALIASES: &[(&str, &str)] = &[
    ("moonshot", "Moonshot AI"),
    ("moonshotai", "Moonshot AI"),
    ("openai", "OpenAI"),
    ("anthropic", "Anthropic"),
    ("fireworks", "Fireworks"),
    ("together", "Together"),
    ("deepinfra", "DeepInfra"),
];

/// Known OpenRouter provider names for autocomplete/fallback suggestions.
pub fn known_providers() -> Vec<String> {
    KNOWN_PROVIDERS.iter().map(|p| (*p).to_string()).collect()
}

/// Cache TTL in seconds (24 hours)
const CACHE_TTL_SECS: u64 = 24 * 60 * 60;
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

fn configured_api_base() -> String {
    let raw = std::env::var("JCODE_OPENROUTER_API_BASE")
        .ok()
        .map(|v| v.trim().to_string())
        .filter(|v| !v.is_empty())
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

fn configured_cache_namespace() -> String {
    let raw = std::env::var("JCODE_OPENROUTER_CACHE_NAMESPACE")
        .ok()
        .map(|v| v.trim().to_string())
        .filter(|v| !v.is_empty())
        .unwrap_or_else(|| DEFAULT_CACHE_NAMESPACE.to_string());
    let sanitized: String = raw
        .chars()
        .filter(|c| c.is_ascii_alphanumeric() || *c == '-' || *c == '_')
        .collect();
    if sanitized.is_empty() {
        DEFAULT_CACHE_NAMESPACE.to_string()
    } else {
        sanitized
    }
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

fn configured_dynamic_bearer_provider() -> Option<String> {
    std::env::var("JCODE_OPENROUTER_DYNAMIC_BEARER_PROVIDER")
        .ok()
        .map(|v| v.trim().to_ascii_lowercase())
        .filter(|v| !v.is_empty())
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
        }
    }

    fn label(&self) -> &str {
        match self {
            Self::AuthorizationBearer { label, .. } => label,
            Self::HeaderValue { label, .. } => label,
            Self::AzureEntra { label } => label,
        }
    }
}

/// Model info from OpenRouter API
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct ModelInfo {
    pub id: String,
    pub name: String,
    pub context_length: Option<u64>,
    #[serde(default)]
    pub pricing: ModelPricing,
    /// Unix timestamp when the model was created/added
    #[serde(default)]
    pub created: Option<u64>,
}

#[derive(Debug, Clone, Default, Serialize, Deserialize)]
pub struct ModelPricing {
    pub prompt: Option<String>,
    pub completion: Option<String>,
    #[serde(default, rename = "input_cache_read")]
    pub input_cache_read: Option<String>,
    #[serde(default, rename = "input_cache_write")]
    pub input_cache_write: Option<String>,
}

/// Per-provider endpoint info from OpenRouter /endpoints API
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct EndpointInfo {
    pub provider_name: String,
    #[serde(default)]
    pub tag: Option<String>,
    #[serde(default)]
    pub pricing: ModelPricing,
    #[serde(default)]
    pub context_length: Option<u64>,
    #[serde(default)]
    pub max_completion_tokens: Option<u64>,
    #[serde(default)]
    pub quantization: Option<String>,
    #[serde(default)]
    pub uptime_last_30m: Option<f64>,
    #[serde(default)]
    pub latency_last_30m: Option<serde_json::Value>,
    #[serde(default)]
    pub throughput_last_30m: Option<serde_json::Value>,
    #[serde(default)]
    pub supports_implicit_caching: Option<bool>,
    #[serde(default)]
    pub status: Option<i32>,
}

impl EndpointInfo {
    /// Extract p50 value from a percentile object or plain number
    fn extract_p50(value: &serde_json::Value) -> Option<f64> {
        match value {
            serde_json::Value::Number(n) => n.as_f64(),
            serde_json::Value::Object(map) => map.get("p50").and_then(|v| v.as_f64()),
            _ => None,
        }
    }

    /// Format a short detail string for picker display: "$0.45/M, 99% up, 14 tps"
    pub fn detail_string(&self) -> String {
        let mut parts = Vec::new();
        if let Some(ref prompt) = self.pricing.prompt {
            if let Ok(p) = prompt.parse::<f64>() {
                parts.push(format!("${:.2}/M", p * 1e6));
            }
        }
        if let Some(uptime) = self.uptime_last_30m {
            parts.push(format!("{:.0}%", uptime));
        }
        if let Some(ref tps) = self.throughput_last_30m {
            if let Some(t) = Self::extract_p50(tps) {
                if t > 0.0 {
                    parts.push(format!("{:.0}tps", t));
                }
            }
        }
        if let Some(ref cache_read) = self.pricing.input_cache_read {
            if let Ok(cr) = cache_read.parse::<f64>() {
                if cr > 0.0 {
                    parts.push("cache".to_string());
                }
            }
        }
        if let Some(ref q) = self.quantization {
            if q != "unknown" {
                parts.push(q.clone());
            }
        }
        parts.join(", ")
    }
}

#[derive(Debug, Clone, Serialize, Deserialize)]
struct EndpointsDiskCache {
    cached_at: u64,
    endpoints: Vec<EndpointInfo>,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum PinSource {
    Explicit,
    Observed,
}

#[derive(Debug, Clone)]
struct ProviderPin {
    model: String,
    provider: String,
    source: PinSource,
    allow_fallbacks: bool,
    last_cache_read: Option<Instant>,
}

#[derive(Debug, Clone)]
struct ParsedProvider {
    name: String,
    allow_fallbacks: bool,
}

fn normalize_provider_name(raw: &str) -> String {
    let trimmed = raw.trim();
    if trimmed.is_empty() {
        return String::new();
    }

    let lower = trimmed.to_lowercase();
    for (alias, canonical) in PROVIDER_ALIASES {
        if lower == *alias {
            return (*canonical).to_string();
        }
    }

    for known in KNOWN_PROVIDERS {
        if known.eq_ignore_ascii_case(trimmed) {
            return (*known).to_string();
        }
    }

    let simplified: String = lower
        .chars()
        .filter(|c| c.is_ascii_alphanumeric())
        .collect();
    for known in KNOWN_PROVIDERS {
        let known_simple: String = known
            .to_lowercase()
            .chars()
            .filter(|c| c.is_ascii_alphanumeric())
            .collect();
        if known_simple == simplified {
            return (*known).to_string();
        }
    }

    trimmed.to_string()
}

fn parse_model_spec(raw: &str) -> (String, Option<ParsedProvider>) {
    let trimmed = raw.trim();
    if let Some((model, provider)) = trimmed.rsplit_once('@') {
        let model = model.trim();
        let mut provider = provider.trim();
        if model.is_empty() {
            return (trimmed.to_string(), None);
        }
        if provider.is_empty() {
            return (model.to_string(), None);
        }
        let mut allow_fallbacks = true;
        if provider.ends_with('!') {
            provider = provider.trim_end_matches('!').trim();
            allow_fallbacks = false;
        }
        if provider.is_empty() {
            return (model.to_string(), None);
        }
        if provider.eq_ignore_ascii_case("auto") {
            return (model.to_string(), None);
        }
        let provider = normalize_provider_name(provider);
        return (
            model.to_string(),
            Some(ParsedProvider {
                name: provider,
                allow_fallbacks,
            }),
        );
    }

    (trimmed.to_string(), None)
}

fn add_cache_breakpoint(messages: &mut [Message]) -> bool {
    let mut cache_index = None;
    for (idx, msg) in messages.iter().enumerate().rev() {
        if let Role::User = msg.role {
            if msg
                .content
                .iter()
                .any(|b| matches!(b, ContentBlock::Text { .. }))
            {
                cache_index = Some(idx);
                break;
            }
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

/// Disk cache structure
#[derive(Debug, Serialize, Deserialize)]
struct DiskCache {
    /// Unix timestamp when cache was written
    cached_at: u64,
    /// Cached models
    models: Vec<ModelInfo>,
}

/// In-memory cache
#[derive(Debug, Default)]
struct ModelsCache {
    models: Vec<ModelInfo>,
    fetched: bool,
    cached_at: Option<u64>,
}

#[derive(Debug, Default)]
struct ModelCatalogRefreshState {
    in_flight: bool,
    last_attempt_unix: Option<u64>,
}

fn current_unix_secs() -> Option<u64> {
    std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .ok()
        .map(|d| d.as_secs())
}

fn load_disk_cache_entry() -> Option<DiskCache> {
    let path = cache_path();
    let content = std::fs::read_to_string(&path).ok()?;
    let cache: DiskCache = serde_json::from_str(&content).ok()?;
    let now = current_unix_secs()?;

    if now.saturating_sub(cache.cached_at) < CACHE_TTL_SECS {
        Some(cache)
    } else {
        None
    }
}

/// Get the cache file path
fn cache_path() -> PathBuf {
    let namespace = configured_cache_namespace();
    dirs::home_dir()
        .unwrap_or_else(|| PathBuf::from("."))
        .join(".jcode")
        .join("cache")
        .join(format!("{}_models.json", namespace))
}

/// Load models from disk cache if valid
fn load_disk_cache() -> Option<Vec<ModelInfo>> {
    load_disk_cache_entry().map(|cache| cache.models)
}

pub fn load_model_pricing_disk_cache_public(model_id: &str) -> Option<ModelPricing> {
    load_disk_cache()?
        .into_iter()
        .find(|model| model.id == model_id)
        .map(|model| model.pricing)
}

/// Look up the `created` timestamp for a model from the disk cache.
/// Tries exact match first, then common provider-prefixed variants
/// (e.g. "claude-opus-4-6" → "anthropic/claude-opus-4.6").
pub fn model_created_timestamp(model_id: &str) -> Option<u64> {
    let path = cache_path();
    let content = std::fs::read_to_string(&path).ok()?;
    let cache: DiskCache = serde_json::from_str(&content).ok()?;

    // Exact match
    if let Some(ts) = cache
        .models
        .iter()
        .find(|m| m.id == model_id)
        .and_then(|m| m.created)
    {
        return Some(ts);
    }

    // Try OpenRouter-style ID variants for direct provider models
    let candidates = openrouter_id_candidates(model_id);
    for candidate in &candidates {
        if let Some(ts) = cache
            .models
            .iter()
            .find(|m| m.id == *candidate)
            .and_then(|m| m.created)
        {
            return Some(ts);
        }
    }

    None
}

/// Generate OpenRouter ID candidates for a direct provider model name.
/// e.g. "claude-opus-4-6" → ["anthropic/claude-opus-4.6", "anthropic/claude-opus-4-6"]
///      "gpt-5.3-codex"   → ["openai/gpt-5.3-codex"]
fn openrouter_id_candidates(model: &str) -> Vec<String> {
    let mut candidates = Vec::new();
    if model.starts_with("claude-") || model.starts_with("claude_") {
        candidates.push(format!("anthropic/{}", model));
        // Try version with dot instead of dash (claude-opus-4-6 → claude-opus-4.6)
        if let Some(pos) = model.rfind('-') {
            let mut dotted = model.to_string();
            dotted.replace_range(pos..pos + 1, ".");
            candidates.push(format!("anthropic/{}", dotted));
        }
    } else if model.starts_with("gpt-")
        || model.starts_with("codex-")
        || model.starts_with("o1")
        || model.starts_with("o3")
        || model.starts_with("o4")
    {
        candidates.push(format!("openai/{}", model));
    }
    candidates
}

/// Return all cached model timestamps as (id, created) pairs.
pub fn all_model_timestamps() -> Vec<(String, u64)> {
    let path = cache_path();
    let content = match std::fs::read_to_string(&path) {
        Ok(c) => c,
        Err(_) => return Vec::new(),
    };
    let cache: DiskCache = match serde_json::from_str(&content) {
        Ok(c) => c,
        Err(_) => return Vec::new(),
    };
    cache
        .models
        .into_iter()
        .filter_map(|m| m.created.map(|t| (m.id, t)))
        .collect()
}

/// Save models to disk cache
fn save_disk_cache(models: &[ModelInfo]) {
    let path = cache_path();

    // Create cache directory if needed
    if let Some(parent) = path.parent() {
        let _ = std::fs::create_dir_all(parent);
    }

    let now = std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .map(|d| d.as_secs())
        .unwrap_or(0);

    let cache = DiskCache {
        cached_at: now,
        models: models.to_vec(),
    };

    if let Ok(content) = serde_json::to_string(&cache) {
        let _ = std::fs::write(&path, content);
    }
}

async fn fetch_models_from_api(
    client: Client,
    api_base: String,
    auth: ProviderAuth,
    models_cache: Arc<RwLock<ModelsCache>>,
) -> Result<Vec<ModelInfo>> {
    let url = format!("{}/models", api_base);
    let response = auth
        .apply(client.get(&url))
        .await?
        .send()
        .await
        .with_context(|| format!("Failed to fetch models from {}", api_base))?;

    if !response.status().is_success() {
        let status = response.status();
        let body = response.text().await.unwrap_or_default();
        anyhow::bail!("Model catalog API error ({}): {}", status, body);
    }

    #[derive(Deserialize)]
    struct ModelsResponse {
        data: Vec<ModelInfo>,
    }

    let models_response: ModelsResponse = response
        .json()
        .await
        .context("Failed to parse models response")?;

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

fn endpoints_cache_path(model: &str) -> PathBuf {
    // Use a safe filename from the model ID
    let safe_name = model.replace('/', "__");
    let namespace = configured_cache_namespace();
    dirs::home_dir()
        .unwrap_or_else(|| PathBuf::from("."))
        .join(".jcode")
        .join("cache")
        .join(format!("{}_endpoints_{}.json", namespace, safe_name))
}

/// Public access to endpoints disk cache for picker (ignores TTL — stale data is fine for display).
/// Returns (endpoints, age_secs) so caller can show staleness.
pub fn load_endpoints_disk_cache_public(model: &str) -> Option<(Vec<EndpointInfo>, u64)> {
    let path = endpoints_cache_path(model);
    let content = std::fs::read_to_string(&path).ok()?;
    let cache: EndpointsDiskCache = serde_json::from_str(&content).ok()?;
    if cache.endpoints.is_empty() {
        return None;
    }
    let now = std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .ok()?
        .as_secs();
    let age = now.saturating_sub(cache.cached_at);
    Some((cache.endpoints, age))
}

fn load_endpoints_disk_cache(model: &str) -> Option<Vec<EndpointInfo>> {
    let path = endpoints_cache_path(model);
    let content = std::fs::read_to_string(&path).ok()?;
    let cache: EndpointsDiskCache = serde_json::from_str(&content).ok()?;
    let now = std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .ok()?
        .as_secs();
    if now - cache.cached_at < ENDPOINTS_CACHE_TTL_SECS {
        Some(cache.endpoints)
    } else {
        None
    }
}

fn save_endpoints_disk_cache(model: &str, endpoints: &[EndpointInfo]) {
    let path = endpoints_cache_path(model);
    if let Some(parent) = path.parent() {
        let _ = std::fs::create_dir_all(parent);
    }
    let now = std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .map(|d| d.as_secs())
        .unwrap_or(0);
    let cache = EndpointsDiskCache {
        cached_at: now,
        endpoints: endpoints.to_vec(),
    };
    if let Ok(content) = serde_json::to_string(&cache) {
        let _ = std::fs::write(&path, content);
    }
}

/// Provider routing configuration
#[derive(Debug, Clone)]
pub struct ProviderRouting {
    /// List of provider slugs to try in order (e.g., ["Fireworks", "Together"])
    pub order: Option<Vec<String>>,
    /// Whether to allow fallbacks to other providers (default: true)
    pub allow_fallbacks: bool,
    /// Sort providers by OpenRouter's routing metric (e.g., "throughput", "price", "latency")
    pub sort: Option<String>,
    /// Prefer providers with at least this throughput (tokens/sec)
    pub preferred_min_throughput: Option<u32>,
    /// Prefer providers with latency below this value (ms)
    pub preferred_max_latency: Option<u32>,
    /// Max price per 1M tokens (USD) for providers
    pub max_price: Option<f64>,
    /// Require providers to support all request parameters
    pub require_parameters: Option<bool>,
}

impl Default for ProviderRouting {
    fn default() -> Self {
        Self {
            order: None,
            allow_fallbacks: true,
            sort: None,
            preferred_min_throughput: None,
            preferred_max_latency: None,
            max_price: None,
            require_parameters: None,
        }
    }
}

impl ProviderRouting {
    fn is_empty(&self) -> bool {
        self.order.is_none()
            && self.sort.is_none()
            && self.preferred_min_throughput.is_none()
            && self.preferred_max_latency.is_none()
            && self.max_price.is_none()
            && self.require_parameters.is_none()
            && self.allow_fallbacks
    }
}

pub struct OpenRouterProvider {
    client: Client,
    model: Arc<RwLock<String>>,
    api_base: String,
    auth: ProviderAuth,
    supports_provider_features: bool,
    supports_model_catalog: bool,
    send_openrouter_headers: bool,
    models_cache: Arc<RwLock<ModelsCache>>,
    model_catalog_refresh: Arc<Mutex<ModelCatalogRefreshState>>,
    /// Provider routing preferences
    provider_routing: Arc<RwLock<ProviderRouting>>,
    /// Pinned provider for this session (cache-aware)
    provider_pin: Arc<Mutex<Option<ProviderPin>>>,
    /// In-memory cache of per-model endpoint data
    endpoints_cache: Arc<RwLock<HashMap<String, (u64, Vec<EndpointInfo>)>>>,
}

impl OpenRouterProvider {
    /// Return true if this model is a Kimi K2/K2.5 variant (Moonshot).
    fn is_kimi_model(model: &str) -> bool {
        let lower = model.to_lowercase();
        lower.contains("moonshotai/") || lower.contains("kimi-k2") || lower.contains("kimi-k2.5")
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
        let api_base = configured_api_base();
        let supports_provider_features = provider_features_enabled(&api_base);
        let supports_model_catalog = model_catalog_enabled();
        let send_openrouter_headers = supports_provider_features;
        let auth = Self::resolve_auth()?;

        let model =
            std::env::var("JCODE_OPENROUTER_MODEL").unwrap_or_else(|_| DEFAULT_MODEL.to_string());

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
            send_openrouter_headers,
            models_cache: Arc::new(RwLock::new(ModelsCache::default())),
            model_catalog_refresh: Arc::new(Mutex::new(ModelCatalogRefreshState::default())),
            provider_routing: Arc::new(RwLock::new(provider_routing)),
            provider_pin: Arc::new(Mutex::new(None)),
            endpoints_cache: Arc::new(RwLock::new(HashMap::new())),
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

        if let Some(last) = state.last_attempt_unix {
            if now.saturating_sub(last) < MODEL_CATALOG_REFRESH_RETRY_SECS {
                return false;
            }
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

        handle.spawn(async move {
            match fetch_models_from_api(client, api_base, auth, models_cache).await {
                Ok(models) => crate::logging::info(&format!(
                    "Refreshed OpenRouter model catalog in background ({}): {} models",
                    context,
                    models.len()
                )),
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
        let mut routing = ProviderRouting::default();

        // JCODE_OPENROUTER_PROVIDER: comma-separated list of providers to prefer
        // e.g., "Fireworks" or "Fireworks,Together"
        if let Ok(providers) = std::env::var("JCODE_OPENROUTER_PROVIDER") {
            let order: Vec<String> = providers
                .split(',')
                .map(|s| s.trim().to_string())
                .filter(|s| !s.is_empty())
                .collect();
            if !order.is_empty() {
                routing.order = Some(order);
            }
        }

        // JCODE_OPENROUTER_NO_FALLBACK: disable fallbacks to other providers
        if std::env::var("JCODE_OPENROUTER_NO_FALLBACK").is_ok() {
            routing.allow_fallbacks = false;
        }

        routing
    }

    fn set_explicit_pin(&self, model: &str, provider: ParsedProvider) {
        let mut pin = self.provider_pin.lock().unwrap();
        *pin = Some(ProviderPin {
            model: model.to_string(),
            provider: provider.name,
            source: PinSource::Explicit,
            allow_fallbacks: provider.allow_fallbacks,
            last_cache_read: None,
        });
    }

    fn clear_pin_if_model_changed(&self, model: &str, clear_explicit: bool) {
        let mut pin = self.provider_pin.lock().unwrap();
        if let Some(existing) = pin.as_ref() {
            let should_clear = existing.model != model
                || (clear_explicit
                    && existing.model == model
                    && existing.source == PinSource::Explicit);
            if should_clear {
                *pin = None;
            }
        }
    }

    fn rank_providers_from_endpoints(endpoints: &[EndpointInfo]) -> Vec<String> {
        if endpoints.is_empty() {
            return Vec::new();
        }

        let cache_available = endpoints.iter().any(|e| {
            e.supports_implicit_caching == Some(true)
                || e.pricing
                    .input_cache_read
                    .as_deref()
                    .and_then(|v| v.parse::<f64>().ok())
                    .unwrap_or(0.0)
                    > 0.0
        });

        let mut candidates: Vec<&EndpointInfo> = endpoints
            .iter()
            .filter(|e| e.status != Some(1)) // filter out down providers
            .collect();

        if cache_available {
            let cache_candidates: Vec<&EndpointInfo> = candidates
                .iter()
                .filter(|e| {
                    e.supports_implicit_caching == Some(true)
                        || e.pricing
                            .input_cache_read
                            .as_deref()
                            .and_then(|v| v.parse::<f64>().ok())
                            .unwrap_or(0.0)
                            > 0.0
                })
                .copied()
                .collect();
            if !cache_candidates.is_empty() {
                candidates = cache_candidates;
            }
        }

        if candidates.is_empty() {
            return Vec::new();
        }

        let mut scored: Vec<(f64, &str)> = candidates
            .iter()
            .map(|e| {
                let throughput = e
                    .throughput_last_30m
                    .as_ref()
                    .and_then(|v| EndpointInfo::extract_p50(v))
                    .unwrap_or(0.0);
                let uptime = e.uptime_last_30m.unwrap_or(0.0) / 100.0;
                let cost = e
                    .pricing
                    .prompt
                    .as_deref()
                    .and_then(|v| v.parse::<f64>().ok())
                    .unwrap_or(0.0);
                let cost_score = if cost > 0.0 {
                    1.0 / (1.0 + cost * 1e6)
                } else {
                    0.5
                };

                let score =
                    0.50 * throughput.min(200.0) / 200.0 + 0.30 * uptime + 0.20 * cost_score;

                (score, e.provider_name.as_str())
            })
            .collect();

        scored.sort_by(|a, b| b.0.partial_cmp(&a.0).unwrap_or(std::cmp::Ordering::Equal));
        scored
            .into_iter()
            .map(|(_, name)| name.to_string())
            .collect()
    }

    async fn effective_routing(&self, model: &str) -> ProviderRouting {
        if !self.supports_provider_features {
            return ProviderRouting::default();
        }

        let base = self.provider_routing.read().await.clone();
        let pin = self.provider_pin.lock().unwrap().clone();

        if let Some(pin) = pin {
            if pin.model == model {
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
            if endpoints.is_none() {
                if let Ok(fetched) = self.fetch_endpoints(model).await {
                    if !fetched.is_empty() {
                        endpoints = Some(fetched);
                    }
                }
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
        if let Ok(pin) = self.provider_pin.lock() {
            if let Some(ref pin) = *pin {
                if pin.model == model {
                    return Some(pin.provider.clone());
                }
            }
        }

        // Check explicit routing
        if let Ok(routing) = self.provider_routing.try_read() {
            if let Some(ref order) = routing.order {
                if let Some(first) = order.first() {
                    return Some(first.clone());
                }
            }
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
        } else if let Ok(cache) = self.endpoints_cache.try_read() {
            if let Some((_, endpoints)) = cache.get(model) {
                providers.extend(endpoints.iter().map(|e| e.provider_name.clone()));
            }
        }

        if providers.is_empty() {
            providers = known_providers();
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
            return endpoints
                .iter()
                .map(|e| (e.provider_name.clone(), e.detail_string()))
                .collect();
        }

        // Try in-memory endpoints cache
        if let Ok(cache) = self.endpoints_cache.try_read() {
            if let Some((_, endpoints)) = cache.get(model) {
                return endpoints
                    .iter()
                    .map(|e| (e.provider_name.clone(), e.detail_string()))
                    .collect();
            }
        }

        Vec::new()
    }

    /// Check if OPENROUTER_API_KEY is available (env var or config file)
    pub fn has_credentials() -> bool {
        if matches!(
            configured_dynamic_bearer_provider().as_deref(),
            Some("azure")
        ) {
            return crate::auth::azure::has_configuration();
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

        let key_name = configured_api_key_name();
        let api_key = Self::get_api_key().ok_or_else(|| {
            anyhow::anyhow!(
                "{} not found in environment or ~/.config/jcode/{}",
                key_name,
                configured_env_file_name()
            )
        })?;

        Ok(match configured_auth_header_mode() {
            AuthHeaderMode::AuthorizationBearer => ProviderAuth::AuthorizationBearer {
                token: api_key,
                label: key_name,
            },
            AuthHeaderMode::ApiKey => ProviderAuth::HeaderValue {
                header_name: HeaderName::from_static("api-key"),
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
            if let Some((cached_at, endpoints)) = cache.get(model) {
                if now - cached_at < ENDPOINTS_CACHE_TTL_SECS {
                    return Ok(endpoints.clone());
                }
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
            let body = response.text().await.unwrap_or_default();
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
        if cache.fetched {
            if let Some(model) = cache.models.iter().find(|m| m.id == model_id) {
                return Some(model.pricing.clone());
            }
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

        if let Ok(models) = self.fetch_models().await {
            if let Some(model) = models.iter().find(|m| m.id == model_id) {
                return Some(model.pricing.clone());
            }
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

#[async_trait]
impl Provider for OpenRouterProvider {
    async fn complete(
        &self,
        messages: &[Message],
        tools: &[ToolDefinition],
        system: &str,
        _resume_session_id: Option<&str>,
    ) -> Result<EventStream> {
        let model = self.model.read().await.clone();
        let thinking_override = Self::thinking_override();
        let thinking_enabled = thinking_override.or_else(|| {
            if Self::is_kimi_model(&model) {
                Some(true)
            } else {
                None
            }
        });
        let allow_reasoning = thinking_enabled != Some(false);
        let include_reasoning_content =
            thinking_enabled == Some(true) || (allow_reasoning && Self::is_kimi_model(&model));

        let mut effective_messages: Vec<Message> = messages.to_vec();
        let cache_supported = self.model_supports_cache(&model).await;
        let cache_control_added = if cache_supported {
            add_cache_breakpoint(&mut effective_messages)
        } else {
            false
        };

        // Build messages in OpenAI format
        let mut api_messages = Vec::new();

        // Add system message if provided
        if !system.is_empty() {
            api_messages.push(serde_json::json!({
                "role": "system",
                "content": system
            }));
        }

        let content_from_parts = |parts: Vec<Value>| -> Option<Value> {
            if parts.is_empty() {
                return None;
            }
            if parts.len() == 1 {
                let part = &parts[0];
                let has_cache = part.get("cache_control").is_some();
                if !has_cache {
                    if let Some(text) = part.get("text").and_then(|v| v.as_str()) {
                        return Some(serde_json::json!(text));
                    }
                }
            }
            Some(Value::Array(parts))
        };

        let mut tool_result_last_pos: HashMap<String, usize> = HashMap::new();
        for (idx, msg) in effective_messages.iter().enumerate() {
            if let Role::User = msg.role {
                for block in &msg.content {
                    if let ContentBlock::ToolResult { tool_use_id, .. } = block {
                        tool_result_last_pos.insert(tool_use_id.clone(), idx);
                    }
                }
            }
        }

        let missing_output = format!("[Error] {}", TOOL_OUTPUT_MISSING_TEXT);
        let mut injected_missing = 0usize;
        let mut delayed_results = 0usize;
        let mut skipped_results = 0usize;
        let mut tool_calls_seen: HashSet<String> = HashSet::new();
        let mut pending_tool_results: HashMap<String, String> = HashMap::new();
        let mut used_tool_results: HashSet<String> = HashSet::new();

        // Convert messages
        for (idx, msg) in effective_messages.iter().enumerate() {
            match msg.role {
                Role::User => {
                    let mut pending_user_parts: Vec<Value> = Vec::new();
                    for block in &msg.content {
                        match block {
                            ContentBlock::Text {
                                text,
                                cache_control,
                            } => {
                                let mut part = serde_json::json!({
                                    "type": "text",
                                    "text": text
                                });
                                if let Some(cache_control) = cache_control {
                                    part["cache_control"] =
                                        serde_json::to_value(cache_control).unwrap_or(Value::Null);
                                }
                                pending_user_parts.push(part);
                            }
                            ContentBlock::Image { media_type, data } => {
                                pending_user_parts.push(serde_json::json!({
                                    "type": "image_url",
                                    "image_url": {
                                        "url": format!("data:{};base64,{}", media_type, data)
                                    }
                                }));
                            }
                            ContentBlock::ToolResult {
                                tool_use_id,
                                content,
                                is_error,
                            } => {
                                if let Some(content) =
                                    content_from_parts(std::mem::take(&mut pending_user_parts))
                                {
                                    api_messages.push(serde_json::json!({
                                        "role": "user",
                                        "content": content
                                    }));
                                }

                                if used_tool_results.contains(tool_use_id) {
                                    skipped_results += 1;
                                    continue;
                                }
                                let output = if is_error == &Some(true) {
                                    format!("[Error] {}", content)
                                } else {
                                    content.clone()
                                };
                                if tool_calls_seen.contains(tool_use_id) {
                                    api_messages.push(serde_json::json!({
                                        "role": "tool",
                                        "tool_call_id": crate::message::sanitize_tool_id(tool_use_id),
                                        "content": output
                                    }));
                                    used_tool_results.insert(tool_use_id.clone());
                                } else if pending_tool_results.contains_key(tool_use_id) {
                                    skipped_results += 1;
                                } else {
                                    pending_tool_results.insert(tool_use_id.clone(), output);
                                    delayed_results += 1;
                                }
                            }
                            _ => {}
                        }
                    }

                    if let Some(content) =
                        content_from_parts(std::mem::take(&mut pending_user_parts))
                    {
                        api_messages.push(serde_json::json!({
                            "role": "user",
                            "content": content
                        }));
                    }
                }
                Role::Assistant => {
                    let mut text_content = String::new();
                    let reasoning_content = String::new();
                    let mut tool_calls = Vec::new();
                    let mut post_tool_outputs: Vec<(String, String)> = Vec::new();
                    let mut missing_tool_outputs: Vec<String> = Vec::new();

                    for block in &msg.content {
                        match block {
                            ContentBlock::Text { text, .. } => {
                                text_content.push_str(text);
                            }
                            ContentBlock::ToolUse { id, name, input } => {
                                let args = if input.is_object() {
                                    serde_json::to_string(input).unwrap_or_default()
                                } else {
                                    "{}".to_string()
                                };
                                tool_calls.push(serde_json::json!({
                                    "id": crate::message::sanitize_tool_id(id),
                                    "type": "function",
                                    "function": {
                                        "name": name,
                                        "arguments": args
                                    }
                                }));
                                tool_calls_seen.insert(id.clone());
                                if let Some(output) = pending_tool_results.remove(id) {
                                    post_tool_outputs.push((id.clone(), output));
                                    used_tool_results.insert(id.clone());
                                } else {
                                    let has_future_output = tool_result_last_pos
                                        .get(id)
                                        .map(|pos| *pos > idx)
                                        .unwrap_or(false);
                                    if !has_future_output {
                                        missing_tool_outputs.push(id.clone());
                                        used_tool_results.insert(id.clone());
                                    }
                                }
                            }
                            _ => {}
                        }
                    }

                    let mut assistant_msg = serde_json::json!({
                        "role": "assistant",
                    });

                    if !text_content.is_empty() {
                        assistant_msg["content"] = serde_json::json!(text_content);
                    }

                    if !tool_calls.is_empty() {
                        assistant_msg["tool_calls"] = serde_json::json!(tool_calls);
                    }

                    if allow_reasoning
                        && (include_reasoning_content || !reasoning_content.is_empty())
                        && (!reasoning_content.is_empty() || !tool_calls.is_empty())
                    {
                        let reasoning_payload =
                            if reasoning_content.is_empty() && !tool_calls.is_empty() {
                                " ".to_string()
                            } else {
                                reasoning_content
                            };
                        assistant_msg["reasoning_content"] = serde_json::json!(reasoning_payload);
                    }

                    if !text_content.is_empty() || !tool_calls.is_empty() {
                        api_messages.push(assistant_msg);

                        for (tool_call_id, output) in post_tool_outputs {
                            api_messages.push(serde_json::json!({
                                "role": "tool",
                                "tool_call_id": crate::message::sanitize_tool_id(&tool_call_id),
                                "content": output
                            }));
                        }

                        if !missing_tool_outputs.is_empty() {
                            injected_missing += missing_tool_outputs.len();
                            for missing_id in missing_tool_outputs {
                                api_messages.push(serde_json::json!({
                                    "role": "tool",
                                    "tool_call_id": crate::message::sanitize_tool_id(&missing_id),
                                    "content": missing_output.clone()
                                }));
                            }
                        }
                    }
                }
            }
        }

        if delayed_results > 0 {
            crate::logging::info(&format!(
                "[openrouter] Delayed {} tool output(s) to preserve call ordering",
                delayed_results
            ));
        }

        if !pending_tool_results.is_empty() {
            skipped_results += pending_tool_results.len();
        }

        if injected_missing > 0 {
            crate::logging::info(&format!(
                "[openrouter] Injected {} synthetic tool output(s) to prevent API error",
                injected_missing
            ));
        }
        if skipped_results > 0 {
            crate::logging::info(&format!(
                "[openrouter] Filtered {} orphaned tool result(s) to prevent API error",
                skipped_results
            ));
        }

        // Safety pass: ensure tool-call messages include reasoning_content (when allowed)
        // and that every tool call has a matching tool output after it.
        let mut outputs_after: HashSet<String> = HashSet::new();
        let mut missing_by_index: Vec<Vec<String>> = vec![Vec::new(); api_messages.len()];

        for (idx, msg) in api_messages.iter().enumerate().rev() {
            let role = msg.get("role").and_then(|v| v.as_str()).unwrap_or("");
            if role == "tool" {
                if let Some(id) = msg.get("tool_call_id").and_then(|v| v.as_str()) {
                    outputs_after.insert(id.to_string());
                }
                continue;
            }

            if role == "assistant" {
                if let Some(tool_calls) = msg.get("tool_calls").and_then(|v| v.as_array()) {
                    for call in tool_calls {
                        if let Some(id) = call.get("id").and_then(|v| v.as_str()) {
                            if !outputs_after.contains(id) {
                                outputs_after.insert(id.to_string());
                                missing_by_index[idx].push(id.to_string());
                            }
                        }
                    }
                }
            }
        }

        let mut normalized = Vec::with_capacity(api_messages.len());
        let mut extra_outputs = 0usize;
        let mut missing_reasoning = 0usize;

        for (idx, mut msg) in api_messages.into_iter().enumerate() {
            let role = msg.get("role").and_then(|v| v.as_str()).unwrap_or("");
            if role == "assistant" && allow_reasoning {
                if msg.get("tool_calls").and_then(|v| v.as_array()).is_some() {
                    let needs_reasoning = match msg.get("reasoning_content") {
                        Some(value) => value.as_str().map(|s| s.trim().is_empty()).unwrap_or(true),
                        None => true,
                    };
                    if needs_reasoning {
                        msg["reasoning_content"] = serde_json::json!(" ");
                        missing_reasoning += 1;
                    }
                }
            }

            normalized.push(msg);

            if let Some(missing) = missing_by_index.get(idx) {
                for id in missing {
                    extra_outputs += 1;
                    normalized.push(serde_json::json!({
                        "role": "tool",
                        "tool_call_id": id,
                        "content": missing_output.clone()
                    }));
                }
            }
        }

        api_messages = normalized;

        if missing_reasoning > 0 {
            crate::logging::info(&format!(
                "[openrouter] Filled reasoning_content on {} tool-call message(s)",
                missing_reasoning
            ));
        }
        if extra_outputs > 0 {
            crate::logging::info(&format!(
                "[openrouter] Safety-injected {} missing tool output(s) at request build",
                extra_outputs
            ));
        }

        // Final safety pass: ensure every tool_call_id has at least one tool response after it.
        let mut tool_output_positions: HashMap<String, usize> = HashMap::new();
        for (idx, msg) in api_messages.iter().enumerate() {
            if msg.get("role").and_then(|v| v.as_str()) == Some("tool") {
                if let Some(id) = msg.get("tool_call_id").and_then(|v| v.as_str()) {
                    tool_output_positions.entry(id.to_string()).or_insert(idx);
                }
            }
        }

        let mut missing_after: HashSet<String> = HashSet::new();
        for (idx, msg) in api_messages.iter().enumerate() {
            if msg.get("role").and_then(|v| v.as_str()) != Some("assistant") {
                continue;
            }
            if let Some(tool_calls) = msg.get("tool_calls").and_then(|v| v.as_array()) {
                for call in tool_calls {
                    if let Some(id) = call.get("id").and_then(|v| v.as_str()) {
                        let has_after = tool_output_positions
                            .get(id)
                            .map(|pos| *pos > idx)
                            .unwrap_or(false);
                        if !has_after {
                            missing_after.insert(id.to_string());
                        }
                    }
                }
            }
        }

        if !missing_after.is_empty() {
            for id in missing_after.iter() {
                api_messages.push(serde_json::json!({
                    "role": "tool",
                    "tool_call_id": id,
                    "content": missing_output.clone()
                }));
            }
            crate::logging::info(&format!(
                "[openrouter] Appended {} tool output(s) to satisfy call ordering",
                missing_after.len()
            ));
        }

        // Final pass: ensure tool outputs immediately follow assistant tool calls.
        let mut tool_output_map: HashMap<String, Value> = HashMap::new();
        for msg in &api_messages {
            if msg.get("role").and_then(|v| v.as_str()) == Some("tool") {
                if let Some(id) = msg.get("tool_call_id").and_then(|v| v.as_str()) {
                    let is_missing = msg
                        .get("content")
                        .and_then(|v| v.as_str())
                        .map(|v| v == missing_output)
                        .unwrap_or(false);
                    match tool_output_map.get(id) {
                        Some(existing) => {
                            let existing_missing = existing
                                .get("content")
                                .and_then(|v| v.as_str())
                                .map(|v| v == missing_output)
                                .unwrap_or(false);
                            if existing_missing && !is_missing {
                                tool_output_map.insert(id.to_string(), msg.clone());
                            }
                        }
                        None => {
                            tool_output_map.insert(id.to_string(), msg.clone());
                        }
                    }
                }
            }
        }

        let mut reordered: Vec<Value> = Vec::with_capacity(api_messages.len());
        let mut used_outputs: HashSet<String> = HashSet::new();
        let mut injected_ordered = 0usize;
        let mut dropped_orphans = 0usize;

        for msg in api_messages.into_iter() {
            let role = msg.get("role").and_then(|v| v.as_str()).unwrap_or("");
            if role == "assistant" {
                let tool_calls = msg.get("tool_calls").and_then(|v| v.as_array()).cloned();
                if let Some(tool_calls) = tool_calls {
                    if tool_calls.is_empty() {
                        reordered.push(msg);
                        continue;
                    }
                    reordered.push(msg);
                    for call in tool_calls {
                        if let Some(id) = call.get("id").and_then(|v| v.as_str()) {
                            if let Some(tool_msg) = tool_output_map.get(id) {
                                reordered.push(tool_msg.clone());
                                used_outputs.insert(id.to_string());
                            } else {
                                injected_ordered += 1;
                                reordered.push(serde_json::json!({
                                    "role": "tool",
                                    "tool_call_id": id,
                                    "content": missing_output.clone()
                                }));
                                used_outputs.insert(id.to_string());
                            }
                        }
                    }
                    continue;
                }
            }

            if role == "tool" {
                if let Some(id) = msg.get("tool_call_id").and_then(|v| v.as_str()) {
                    if used_outputs.contains(id) {
                        dropped_orphans += 1;
                        continue;
                    }
                }
                dropped_orphans += 1;
                continue;
            }

            reordered.push(msg);
        }

        api_messages = reordered;

        if injected_ordered > 0 {
            crate::logging::info(&format!(
                "[openrouter] Inserted {} tool output(s) to enforce call ordering",
                injected_ordered
            ));
        }
        if dropped_orphans > 0 {
            crate::logging::info(&format!(
                "[openrouter] Dropped {} orphaned tool output(s) during re-ordering",
                dropped_orphans
            ));
        }

        // Build tools in OpenAI format
        let api_tools: Vec<Value> = tools
            .iter()
            .map(|t| {
                serde_json::json!({
                    "type": "function",
                    "function": {
                        "name": t.name,
                        "description": t.description,
                        "parameters": t.input_schema,
                    }
                })
            })
            .collect();

        // Build request
        let mut request = serde_json::json!({
            "model": model,
            "messages": api_messages,
            "stream": true,
        });

        if !api_tools.is_empty() {
            request["tools"] = serde_json::json!(api_tools);
            request["tool_choice"] = serde_json::json!("auto");
        }

        // Optional thinking override for OpenRouter (provider-specific).
        if let Some(enable) = thinking_enabled {
            request["thinking"] = serde_json::json!({
                "type": if enable { "enabled" } else { "disabled" }
            });
        }

        // Add provider routing if configured and supported by backend.
        let mut provider_obj = None;
        if self.supports_provider_features {
            let routing = self.effective_routing(&model).await;
            if !routing.is_empty() {
                let mut obj = serde_json::json!({});
                if let Some(ref order) = routing.order {
                    obj["order"] = serde_json::json!(order);
                }
                if !routing.allow_fallbacks {
                    obj["allow_fallbacks"] = serde_json::json!(false);
                }
                if let Some(ref sort) = routing.sort {
                    obj["sort"] = serde_json::json!(sort);
                }
                if let Some(min_tp) = routing.preferred_min_throughput {
                    obj["preferred_min_throughput"] = serde_json::json!(min_tp);
                }
                if let Some(max_latency) = routing.preferred_max_latency {
                    obj["preferred_max_latency"] = serde_json::json!(max_latency);
                }
                if let Some(max_price) = routing.max_price {
                    obj["max_price"] = serde_json::json!(max_price);
                }
                if let Some(require_parameters) = routing.require_parameters {
                    obj["require_parameters"] = serde_json::json!(require_parameters);
                }
                provider_obj = Some(obj);
            }
        }

        if cache_control_added && self.supports_provider_features {
            let mut obj = provider_obj.unwrap_or_else(|| serde_json::json!({}));
            obj["require_parameters"] = serde_json::json!(true);
            provider_obj = Some(obj);
        }

        if let Some(obj) = provider_obj {
            request["provider"] = obj;
        }

        // OpenRouter uses HTTPS/SSE transport only
        crate::logging::info("OpenRouter transport: HTTPS (SSE)");

        let (tx, rx) = mpsc::channel::<Result<StreamEvent>>(100);
        let client = self.client.clone();
        let api_base = self.api_base.clone();
        let auth = self.auth.clone();
        let send_openrouter_headers = self.send_openrouter_headers;
        let request_for_retries = request;
        let model_for_stream = model.clone();
        let provider_pin = Arc::clone(&self.provider_pin);

        tokio::spawn(async move {
            if tx
                .send(Ok(StreamEvent::ConnectionType {
                    connection: "https/sse".to_string(),
                }))
                .await
                .is_err()
            {
                return;
            }
            run_stream_with_retries(
                client,
                api_base,
                auth,
                send_openrouter_headers,
                request_for_retries,
                tx,
                provider_pin,
                model_for_stream,
            )
            .await;
        });

        Ok(Box::pin(ReceiverStream::new(rx)))
    }

    fn name(&self) -> &str {
        "openrouter"
    }

    fn model(&self) -> String {
        self.model
            .try_read()
            .map(|m| m.clone())
            .unwrap_or_else(|_| DEFAULT_MODEL.to_string())
    }

    fn set_model(&self, model: &str) -> Result<()> {
        // OpenRouter accepts any model ID - validation happens at API call time
        // This allows using any model without needing to pre-fetch the list
        let (model_id, provider) = parse_model_spec(model);
        if let Ok(mut current) = self.model.try_write() {
            *current = model_id.clone();
        } else {
            return Err(anyhow::anyhow!(
                "Cannot change model while a request is in progress"
            ));
        }

        if self.supports_provider_features {
            if let Some(provider) = provider {
                self.set_explicit_pin(&model_id, provider);
            } else {
                self.clear_pin_if_model_changed(&model_id, true);
            }
        } else {
            self.clear_pin_if_model_changed(&model_id, true);
        }

        Ok(())
    }

    fn available_models(&self) -> Vec<&'static str> {
        // OpenRouter models are fetched dynamically from the API.
        // Static list is empty; use available_models_display for cached list.
        vec![]
    }

    fn available_models_display(&self) -> Vec<String> {
        if !self.supports_model_catalog {
            let model = self.model();
            if model.trim().is_empty() {
                return Vec::new();
            }
            return vec![model];
        }

        if let Ok(cache) = self.models_cache.try_read() {
            if cache.fetched && !cache.models.is_empty() {
                if let Some(cache_age) = cache.cached_at.and_then(|cached_at| {
                    current_unix_secs().map(|now| now.saturating_sub(cached_at))
                }) {
                    self.maybe_schedule_model_catalog_refresh(cache_age, "display memory cache");
                }
                return cache.models.iter().map(|m| m.id.clone()).collect();
            }
        }

        if let Some(cache_entry) = load_disk_cache_entry() {
            let cache_age = current_unix_secs()
                .map(|now| now.saturating_sub(cache_entry.cached_at))
                .unwrap_or(0);
            if let Ok(mut cache) = self.models_cache.try_write() {
                cache.models = cache_entry.models.clone();
                cache.fetched = true;
                cache.cached_at = Some(cache_entry.cached_at);
            }
            self.maybe_schedule_model_catalog_refresh(cache_age, "display disk cache");
            return cache_entry.models.into_iter().map(|m| m.id).collect();
        }

        let model = self.model();
        if model.trim().is_empty() {
            Vec::new()
        } else {
            vec![model]
        }
    }

    async fn prefetch_models(&self) -> Result<()> {
        if !self.supports_model_catalog {
            return Ok(());
        }

        let _ = self.fetch_models().await?;
        if self.supports_provider_features {
            // Also prefetch endpoints for the current model so preferred_provider() works immediately.
            let model = self.model();
            if load_endpoints_disk_cache(&model).is_none() {
                let _ = self.fetch_endpoints(&model).await;
            }
        }
        Ok(())
    }

    fn supports_compaction(&self) -> bool {
        true
    }

    fn preferred_provider(&self) -> Option<String> {
        self.preferred_provider()
    }

    fn context_window(&self) -> usize {
        let model_id = self.model();
        // Try cached model data from OpenRouter API
        let cache = self.models_cache.try_read();
        if let Ok(cache) = cache {
            if let Some(model) = cache.models.iter().find(|m| m.id == model_id) {
                if let Some(ctx) = model.context_length {
                    return ctx as usize;
                }
            }
        }
        crate::provider::context_limit_for_model_with_provider(&model_id, Some(self.name()))
            .unwrap_or(crate::provider::DEFAULT_CONTEXT_LIMIT)
    }

    fn fork(&self) -> Arc<dyn Provider> {
        Arc::new(Self {
            client: self.client.clone(),
            model: Arc::new(RwLock::new(
                self.model.try_read().map(|m| m.clone()).unwrap_or_default(),
            )),
            api_base: self.api_base.clone(),
            auth: self.auth.clone(),
            supports_provider_features: self.supports_provider_features,
            supports_model_catalog: self.supports_model_catalog,
            send_openrouter_headers: self.send_openrouter_headers,
            models_cache: Arc::clone(&self.models_cache),
            model_catalog_refresh: Arc::clone(&self.model_catalog_refresh),
            provider_routing: Arc::new(RwLock::new(
                self.provider_routing
                    .try_read()
                    .map(|r| r.clone())
                    .unwrap_or_default(),
            )),
            provider_pin: Arc::new(Mutex::new(None)),
            endpoints_cache: Arc::clone(&self.endpoints_cache),
        })
    }
}

// ============================================================================
// SSE Stream Parser
// ============================================================================

async fn run_stream_with_retries(
    client: Client,
    api_base: String,
    auth: ProviderAuth,
    send_openrouter_headers: bool,
    request: Value,
    tx: mpsc::Sender<Result<StreamEvent>>,
    provider_pin: Arc<Mutex<Option<ProviderPin>>>,
    model: String,
) {
    let mut last_error = None;

    for attempt in 0..MAX_RETRIES {
        if attempt > 0 {
            let delay = RETRY_BASE_DELAY_MS * (1 << (attempt - 1));
            tokio::time::sleep(std::time::Duration::from_millis(delay)).await;
            crate::logging::info(&format!(
                "Retrying API request using {} (attempt {}/{})",
                auth.label(),
                attempt + 1,
                MAX_RETRIES
            ));
        }

        crate::logging::info(&format!(
            "API stream attempt {}/{} over HTTPS transport (model: {}, auth: {})",
            attempt + 1,
            MAX_RETRIES,
            model,
            auth.label()
        ));

        match stream_response(
            client.clone(),
            api_base.clone(),
            auth.clone(),
            send_openrouter_headers,
            request.clone(),
            tx.clone(),
            Arc::clone(&provider_pin),
            model.clone(),
        )
        .await
        {
            Ok(()) => return,
            Err(e) => {
                let error_str = e.to_string().to_lowercase();
                if is_retryable_error(&error_str) && attempt + 1 < MAX_RETRIES {
                    crate::logging::info(&format!("Transient API error, will retry: {}", e));
                    last_error = Some(e);
                    continue;
                }

                let _ = tx.send(Err(e)).await;
                return;
            }
        }
    }

    if let Some(e) = last_error {
        let _ = tx
            .send(Err(anyhow::anyhow!(
                "Failed after {} retries: {}",
                MAX_RETRIES,
                e
            )))
            .await;
    }
}

async fn stream_response(
    client: Client,
    api_base: String,
    auth: ProviderAuth,
    send_openrouter_headers: bool,
    request: Value,
    tx: mpsc::Sender<Result<StreamEvent>>,
    provider_pin: Arc<Mutex<Option<ProviderPin>>>,
    model: String,
) -> Result<()> {
    use crate::message::ConnectionPhase;
    let _ = tx
        .send(Ok(StreamEvent::ConnectionPhase {
            phase: ConnectionPhase::Connecting,
        }))
        .await;
    let connect_start = std::time::Instant::now();

    let url = format!("{}/chat/completions", api_base);
    let mut req = auth
        .apply(
            client
                .post(&url)
                .header("Content-Type", "application/json")
                .header("Accept-Encoding", "identity"),
        )
        .await?;

    if send_openrouter_headers {
        req = req
            .header("HTTP-Referer", "https://github.com/jcode")
            .header("X-Title", "jcode");
    }

    let response = req
        .json(&request)
        .send()
        .await
        .with_context(|| format!("Failed to send request using {}", auth.label()))?;

    let connect_ms = connect_start.elapsed().as_millis();
    crate::logging::info(&format!(
        "HTTP connection established in {}ms (status={})",
        connect_ms,
        response.status()
    ));

    if !response.status().is_success() {
        let status = response.status();
        let body = response.text().await.unwrap_or_default();
        anyhow::bail!("API error ({}): {}", status, body);
    }

    let _ = tx
        .send(Ok(StreamEvent::ConnectionPhase {
            phase: ConnectionPhase::WaitingForResponse,
        }))
        .await;

    let mut stream = OpenRouterStream::new(response.bytes_stream(), model.clone(), provider_pin);

    const SSE_CHUNK_TIMEOUT: std::time::Duration = std::time::Duration::from_secs(180);

    loop {
        let event = match tokio::time::timeout(SSE_CHUNK_TIMEOUT, stream.next()).await {
            Ok(Some(event)) => event,
            Ok(None) => break, // stream ended normally
            Err(_) => {
                crate::logging::warn("OpenRouter SSE stream timed out (no data for 180s)");
                anyhow::bail!("Stream read timeout: no data received for 180 seconds");
            }
        };
        if tx.send(event).await.is_err() {
            return Ok(());
        }
    }

    Ok(())
}

fn is_retryable_error(error_str: &str) -> bool {
    error_str.contains("connection reset")
        || error_str.contains("connection reset by peer")
        || error_str.contains("connection refused")
        || error_str.contains("broken pipe")
        || error_str.contains("timed out")
        || error_str.contains("timeout")
        || error_str.contains("error decoding")
        || error_str.contains("stream error")
        || error_str.contains("error reading")
        || error_str.contains("unexpected eof")
        || error_str.contains("eof")
        || error_str.contains("5")
            && (error_str.contains("50")
                || error_str.contains("502")
                || error_str.contains("503")
                || error_str.contains("504")
                || error_str.contains("internal server error"))
        || error_str.contains("overloaded")
}

struct OpenRouterStream {
    inner: Pin<Box<dyn Stream<Item = Result<Bytes, reqwest::Error>> + Send>>,
    buffer: String,
    pending: VecDeque<StreamEvent>,
    current_tool_call: Option<ToolCallAccumulator>,
    /// Track if we've emitted the provider info (only emit once)
    provider_emitted: bool,
    model: String,
    provider_pin: Arc<Mutex<Option<ProviderPin>>>,
}

#[derive(Default)]
struct ToolCallAccumulator {
    id: String,
    name: String,
    arguments: String,
}

impl OpenRouterStream {
    fn new(
        stream: impl Stream<Item = Result<Bytes, reqwest::Error>> + Send + 'static,
        model: String,
        provider_pin: Arc<Mutex<Option<ProviderPin>>>,
    ) -> Self {
        Self {
            inner: Box::pin(stream),
            buffer: String::new(),
            pending: VecDeque::new(),
            current_tool_call: None,
            provider_emitted: false,
            model,
            provider_pin,
        }
    }

    fn observe_provider(&mut self, provider: &str) {
        let mut pin = self.provider_pin.lock().unwrap();
        if let Some(existing) = pin.as_ref() {
            if existing.source == PinSource::Explicit && existing.model == self.model {
                return;
            }
            if existing.source == PinSource::Observed
                && existing.model == self.model
                && existing.provider == provider
            {
                return;
            }
        }

        *pin = Some(ProviderPin {
            model: self.model.clone(),
            provider: provider.to_string(),
            source: PinSource::Observed,
            allow_fallbacks: true,
            last_cache_read: None,
        });
    }

    fn refresh_cache_pin(&mut self, provider: &str) {
        let mut pin = self.provider_pin.lock().unwrap();
        if let Some(existing) = pin.as_mut() {
            if existing.model == self.model && existing.provider == provider {
                existing.last_cache_read = Some(Instant::now());
            }
        }
    }

    fn parse_next_event(&mut self) -> Option<StreamEvent> {
        if let Some(event) = self.pending.pop_front() {
            return Some(event);
        }

        while let Some(pos) = self.buffer.find("\n\n") {
            let event_str = self.buffer[..pos].to_string();
            self.buffer = self.buffer[pos + 2..].to_string();

            // Parse SSE event
            let mut data = None;
            for line in event_str.lines() {
                if let Some(d) = line.strip_prefix("data: ") {
                    data = Some(d);
                }
            }

            let data = match data {
                Some(d) => d,
                None => continue,
            };

            if data == "[DONE]" {
                return Some(StreamEvent::MessageEnd { stop_reason: None });
            }

            let parsed: Value = match serde_json::from_str(data) {
                Ok(v) => v,
                Err(_) => continue,
            };

            // Extract upstream provider info (only emit once)
            // OpenRouter returns "provider" field indicating which provider handled the request
            if !self.provider_emitted {
                if let Some(provider) = parsed.get("provider").and_then(|p| p.as_str()) {
                    self.provider_emitted = true;
                    self.observe_provider(provider);
                    self.pending.push_back(StreamEvent::UpstreamProvider {
                        provider: provider.to_string(),
                    });
                }
            }

            // Check for error
            if let Some(error) = parsed.get("error") {
                let message = error
                    .get("message")
                    .and_then(|v| v.as_str())
                    .unwrap_or("OpenRouter error")
                    .to_string();
                return Some(StreamEvent::Error {
                    message,
                    retry_after_secs: None,
                });
            }

            // Parse choices
            if let Some(choices) = parsed.get("choices").and_then(|c| c.as_array()) {
                for choice in choices {
                    let delta = match choice.get("delta") {
                        Some(d) => d,
                        None => continue,
                    };

                    // Text content
                    if let Some(content) = delta.get("content").and_then(|c| c.as_str()) {
                        if !content.is_empty() {
                            return Some(StreamEvent::TextDelta(content.to_string()));
                        }
                    }

                    // Tool calls
                    if let Some(tool_calls) = delta.get("tool_calls").and_then(|t| t.as_array()) {
                        for tc in tool_calls {
                            let _index = tc.get("index").and_then(|i| i.as_u64()).unwrap_or(0);

                            // Check if this is a new tool call
                            if let Some(id) = tc.get("id").and_then(|i| i.as_str()) {
                                // Emit previous tool call if any
                                if let Some(prev) = self.current_tool_call.take() {
                                    if !prev.id.is_empty() {
                                        self.pending.push_back(StreamEvent::ToolUseStart {
                                            id: prev.id,
                                            name: prev.name,
                                        });
                                        self.pending
                                            .push_back(StreamEvent::ToolInputDelta(prev.arguments));
                                        self.pending.push_back(StreamEvent::ToolUseEnd);
                                    }
                                }

                                let name = tc
                                    .get("function")
                                    .and_then(|f| f.get("name"))
                                    .and_then(|n| n.as_str())
                                    .unwrap_or("")
                                    .to_string();

                                self.current_tool_call = Some(ToolCallAccumulator {
                                    id: id.to_string(),
                                    name,
                                    arguments: String::new(),
                                });
                            }

                            // Accumulate arguments
                            if let Some(args) = tc
                                .get("function")
                                .and_then(|f| f.get("arguments"))
                                .and_then(|a| a.as_str())
                            {
                                if let Some(ref mut tc) = self.current_tool_call {
                                    tc.arguments.push_str(args);
                                }
                            }
                        }
                    }

                    // Check for finish reason
                    if let Some(_finish_reason) =
                        choice.get("finish_reason").and_then(|f| f.as_str())
                    {
                        // Emit any pending tool call
                        if let Some(tc) = self.current_tool_call.take() {
                            if !tc.id.is_empty() {
                                self.pending.push_back(StreamEvent::ToolUseStart {
                                    id: tc.id,
                                    name: tc.name,
                                });
                                self.pending
                                    .push_back(StreamEvent::ToolInputDelta(tc.arguments));
                                self.pending.push_back(StreamEvent::ToolUseEnd);
                            }
                        }

                        // Don't emit MessageEnd here - wait for [DONE]
                    }
                }
            }

            // Extract usage if present
            if let Some(usage) = parsed.get("usage") {
                let input_tokens = usage.get("prompt_tokens").and_then(|t| t.as_u64());
                let output_tokens = usage.get("completion_tokens").and_then(|t| t.as_u64());

                // OpenRouter returns cached tokens in various formats depending on provider:
                // - "cached_tokens" (OpenRouter's unified field)
                // - "prompt_tokens_details.cached_tokens" (OpenAI-style)
                // - "cache_read_input_tokens" (Anthropic-style, passed through)
                let cache_read_input_tokens = usage
                    .get("cached_tokens")
                    .and_then(|t| t.as_u64())
                    .or_else(|| {
                        usage
                            .get("prompt_tokens_details")
                            .and_then(|d| d.get("cached_tokens"))
                            .and_then(|t| t.as_u64())
                    })
                    .or_else(|| {
                        usage
                            .get("cache_read_input_tokens")
                            .and_then(|t| t.as_u64())
                    });

                // Cache creation tokens (Anthropic-style, passed through for some providers)
                let cache_creation_input_tokens = usage
                    .get("cache_creation_input_tokens")
                    .and_then(|t| t.as_u64());

                // Refresh cache pin when we see cache activity
                if cache_read_input_tokens.is_some() || cache_creation_input_tokens.is_some() {
                    if let Some(provider) = parsed.get("provider").and_then(|p| p.as_str()) {
                        self.refresh_cache_pin(provider);
                    }
                }

                if input_tokens.is_some()
                    || output_tokens.is_some()
                    || cache_read_input_tokens.is_some()
                {
                    self.pending.push_back(StreamEvent::TokenUsage {
                        input_tokens,
                        output_tokens,
                        cache_read_input_tokens,
                        cache_creation_input_tokens,
                    });
                }
            }

            if let Some(event) = self.pending.pop_front() {
                return Some(event);
            }
        }

        None
    }
}

impl Stream for OpenRouterStream {
    type Item = Result<StreamEvent>;

    fn poll_next(mut self: Pin<&mut Self>, cx: &mut TaskContext<'_>) -> Poll<Option<Self::Item>> {
        loop {
            if let Some(event) = self.parse_next_event() {
                return Poll::Ready(Some(Ok(event)));
            }

            match self.inner.as_mut().poll_next(cx) {
                Poll::Ready(Some(Ok(bytes))) => {
                    if let Ok(text) = std::str::from_utf8(&bytes) {
                        self.buffer.push_str(text);
                    }
                }
                Poll::Ready(Some(Err(e))) => {
                    return Poll::Ready(Some(Err(anyhow::anyhow!("Stream error: {}", e))));
                }
                Poll::Ready(None) => {
                    // Stream ended - emit any pending tool call
                    if let Some(tc) = self.current_tool_call.take() {
                        if !tc.id.is_empty() {
                            self.pending.push_back(StreamEvent::ToolUseStart {
                                id: tc.id,
                                name: tc.name,
                            });
                            self.pending
                                .push_back(StreamEvent::ToolInputDelta(tc.arguments));
                            self.pending.push_back(StreamEvent::ToolUseEnd);
                        }
                    }
                    if let Some(event) = self.pending.pop_front() {
                        return Poll::Ready(Some(Ok(event)));
                    }
                    return Poll::Ready(None);
                }
                Poll::Pending => {
                    return Poll::Pending;
                }
            }
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::sync::Mutex;

    static ENV_LOCK: Mutex<()> = Mutex::new(());

    #[test]
    fn test_has_credentials() {
        let _has_creds = OpenRouterProvider::has_credentials();
    }

    #[test]
    fn test_configured_api_base_accepts_https() {
        let _lock = ENV_LOCK.lock().unwrap();
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
        let _lock = ENV_LOCK.lock().unwrap();
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

    fn make_endpoint(
        name: &str,
        throughput: f64,
        uptime: f64,
        cache: bool,
        cost: f64,
    ) -> EndpointInfo {
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
            api_base: DEFAULT_API_BASE.to_string(),
            auth: ProviderAuth::AuthorizationBearer {
                token: "test".to_string(),
                label: DEFAULT_API_KEY_NAME.to_string(),
            },
            supports_provider_features: true,
            supports_model_catalog: true,
            send_openrouter_headers: true,
            models_cache: Arc::new(RwLock::new(ModelsCache::default())),
            model_catalog_refresh: Arc::new(Mutex::new(ModelCatalogRefreshState::default())),
            provider_routing: Arc::new(RwLock::new(ProviderRouting::default())),
            provider_pin: Arc::new(Mutex::new(None)),
            endpoints_cache: Arc::new(RwLock::new(HashMap::new())),
        }
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

        OpenRouterProvider::finish_background_model_catalog_refresh(
            &provider.model_catalog_refresh,
        );

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
    fn test_endpoint_detail_string() {
        let ep = EndpointInfo {
            provider_name: "TestProvider".to_string(),
            tag: None,
            pricing: ModelPricing {
                prompt: Some("0.00000045".to_string()),
                completion: Some("0.00000225".to_string()),
                input_cache_read: Some("0.00000007".to_string()),
                input_cache_write: None,
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
            detail.contains("42tps"),
            "should contain throughput: {}",
            detail
        );
        assert!(detail.contains("cache"), "should contain cache: {}", detail);
        assert!(
            detail.contains("fp8"),
            "should contain quantization: {}",
            detail
        );
    }
}
