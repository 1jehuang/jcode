//! OpenRouter API provider
//!
//! Uses OpenRouter's OpenAI-compatible API to access 200+ models from various providers.
//! Models are fetched dynamically from the API and cached to disk.
//!
//! Features:
//! - Provider pinning: Set JCODE_OPENROUTER_PROVIDER to pin to a specific provider (e.g., "Fireworks")
//! - Cache token parsing: Parses cached_tokens from OpenRouter responses for cache hit detection

use super::{EventStream, Provider};
use crate::message::{
    CacheControl, ContentBlock, Message, Role, StreamEvent, ToolDefinition,
    TOOL_OUTPUT_MISSING_TEXT,
};
use anyhow::{Context, Result};
use async_trait::async_trait;
use bytes::Bytes;
use futures::{Stream, StreamExt};
use reqwest::Client;
use serde::{Deserialize, Serialize};
use serde_json::Value;
use std::collections::{HashMap, HashSet, VecDeque};
use std::path::PathBuf;
use std::pin::Pin;
use std::sync::{Arc, Mutex};
use std::task::{Context as TaskContext, Poll};
use std::time::Instant;
use tokio::sync::{mpsc, RwLock};
use tokio_stream::wrappers::ReceiverStream;

/// Maximum number of retries for transient errors
const MAX_RETRIES: u32 = 3;

/// Base delay for exponential backoff (in milliseconds)
const RETRY_BASE_DELAY_MS: u64 = 1000;

/// OpenRouter API base URL
const API_BASE: &str = "https://openrouter.ai/api/v1";

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
/// Provider stats TTL (14 days)
const PROVIDER_STATS_TTL_SECS: u64 = 14 * 24 * 60 * 60;
/// Pin provider to preserve cache for this long after a cache hit
const CACHE_PIN_TTL_SECS: u64 = 60 * 60;
/// If throughput values are within this fraction, rebalance weights toward cost
const THROUGHPUT_SIMILARITY_THRESHOLD: f64 = 0.10;
/// EWMA alpha for provider stats
const PROVIDER_STATS_EWMA_ALPHA: f64 = 0.2;
/// Primary routing weights when throughput differences are meaningful.
/// Priority order: speed > cache > cost.
const WEIGHT_SPEED_PRIMARY: f64 = 0.55;
const WEIGHT_CACHE_PRIMARY: f64 = 0.30;
const WEIGHT_COST_PRIMARY: f64 = 0.15;
/// Rebalanced weights when throughput is effectively similar.
/// Still keeps speed as the top signal while giving cost more influence.
const WEIGHT_SPEED_BALANCED: f64 = 0.45;
const WEIGHT_CACHE_BALANCED: f64 = 0.35;
const WEIGHT_COST_BALANCED: f64 = 0.20;

/// Endpoints cache TTL (1 hour) - per-model provider endpoint data
const ENDPOINTS_CACHE_TTL_SECS: u64 = 60 * 60;

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

#[derive(Debug, Clone, Serialize, Deserialize, Default)]
struct ProviderStatsStore {
    models: HashMap<String, HashMap<String, ProviderStats>>,
}

#[derive(Debug, Clone, Serialize, Deserialize, Default)]
struct ProviderStats {
    samples: u64,
    avg_cache_hit: Option<f64>,
    avg_throughput: Option<f64>,
    avg_cost_per_mtok: Option<f64>,
    last_seen: u64,
    #[serde(default)]
    cache_read_supported: bool,
    #[serde(default)]
    cache_write_supported: bool,
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
struct ProviderSample {
    cache_hit: Option<f64>,
    throughput: Option<f64>,
    cost_per_mtok: Option<f64>,
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

fn update_ewma(prev: Option<f64>, value: f64) -> f64 {
    let value = value.max(0.0);
    match prev {
        Some(p) => p + PROVIDER_STATS_EWMA_ALPHA * (value - p),
        None => value,
    }
}

fn min_max(values: &[f64]) -> (Option<f64>, Option<f64>) {
    if values.is_empty() {
        return (None, None);
    }
    let mut min_val = values[0];
    let mut max_val = values[0];
    for v in values.iter().skip(1) {
        if *v < min_val {
            min_val = *v;
        }
        if *v > max_val {
            max_val = *v;
        }
    }
    (Some(min_val), Some(max_val))
}

fn normalize(value: f64, min: Option<f64>, max: Option<f64>, default: f64) -> f64 {
    match (min, max) {
        (Some(min), Some(max)) => {
            if (max - min).abs() < f64::EPSILON {
                1.0
            } else {
                ((value - min) / (max - min)).clamp(0.0, 1.0)
            }
        }
        _ => default,
    }
}

fn normalize_inverse(value: f64, min: Option<f64>, max: Option<f64>, default: f64) -> f64 {
    match (min, max) {
        (Some(min), Some(max)) => {
            if (max - min).abs() < f64::EPSILON {
                1.0
            } else {
                ((max - value) / (max - min)).clamp(0.0, 1.0)
            }
        }
        _ => default,
    }
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
}

/// Get the cache file path
fn cache_path() -> PathBuf {
    dirs::home_dir()
        .unwrap_or_else(|| PathBuf::from("."))
        .join(".jcode")
        .join("cache")
        .join("openrouter_models.json")
}

/// Get provider stats cache file path
fn provider_stats_path() -> PathBuf {
    dirs::home_dir()
        .unwrap_or_else(|| PathBuf::from("."))
        .join(".jcode")
        .join("cache")
        .join("openrouter_provider_stats.json")
}

fn now_epoch_secs() -> u64 {
    std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .map(|d| d.as_secs())
        .unwrap_or(0)
}

/// Load models from disk cache if valid
fn load_disk_cache() -> Option<Vec<ModelInfo>> {
    let path = cache_path();
    let content = std::fs::read_to_string(&path).ok()?;
    let cache: DiskCache = serde_json::from_str(&content).ok()?;

    // Check if cache is still valid
    let now = std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .ok()?
        .as_secs();

    if now - cache.cached_at < CACHE_TTL_SECS {
        Some(cache.models)
    } else {
        None
    }
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

fn endpoints_cache_path(model: &str) -> PathBuf {
    // Use a safe filename from the model ID
    let safe_name = model.replace('/', "__");
    dirs::home_dir()
        .unwrap_or_else(|| PathBuf::from("."))
        .join(".jcode")
        .join("cache")
        .join(format!("openrouter_endpoints_{}.json", safe_name))
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

fn load_provider_stats() -> ProviderStatsStore {
    let path = provider_stats_path();
    let content = match std::fs::read_to_string(&path) {
        Ok(c) => c,
        Err(_) => return ProviderStatsStore::default(),
    };

    serde_json::from_str(&content).unwrap_or_default()
}

fn save_provider_stats(stats: &ProviderStatsStore) {
    let path = provider_stats_path();
    if let Some(parent) = path.parent() {
        let _ = std::fs::create_dir_all(parent);
    }

    if let Ok(content) = serde_json::to_string(stats) {
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
    api_key: String,
    models_cache: Arc<RwLock<ModelsCache>>,
    /// Provider routing preferences
    provider_routing: Arc<RwLock<ProviderRouting>>,
    /// Observed provider stats (shared across forks)
    provider_stats: Arc<Mutex<ProviderStatsStore>>,
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
        let api_key = Self::get_api_key().ok_or_else(|| {
            anyhow::anyhow!(
                "OPENROUTER_API_KEY not found in environment or ~/.config/jcode/openrouter.env"
            )
        })?;

        let model =
            std::env::var("JCODE_OPENROUTER_MODEL").unwrap_or_else(|_| DEFAULT_MODEL.to_string());

        // Parse provider routing from environment
        let provider_routing = Self::parse_provider_routing();

        Ok(Self {
            client: Client::new(),
            model: Arc::new(RwLock::new(model)),
            api_key,
            models_cache: Arc::new(RwLock::new(ModelsCache::default())),
            provider_routing: Arc::new(RwLock::new(provider_routing)),
            provider_stats: Arc::new(Mutex::new(load_provider_stats())),
            provider_pin: Arc::new(Mutex::new(None)),
            endpoints_cache: Arc::new(RwLock::new(HashMap::new())),
        })
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

    fn rank_providers(&self, model: &str) -> Vec<String> {
        let stats = self.provider_stats.lock().unwrap();
        let model_stats = match stats.models.get(model) {
            Some(m) => m,
            None => return Vec::new(),
        };
        let now = now_epoch_secs();
        let mut entries: Vec<(String, ProviderStats)> = model_stats
            .iter()
            .filter_map(|(provider, stat)| {
                if now.saturating_sub(stat.last_seen) > PROVIDER_STATS_TTL_SECS {
                    None
                } else {
                    Some((provider.clone(), stat.clone()))
                }
            })
            .collect();
        drop(stats);

        if entries.is_empty() {
            return Vec::new();
        }

        let cache_supported = entries
            .iter()
            .any(|(_, stat)| stat.cache_read_supported || stat.cache_write_supported);

        if cache_supported {
            // If this model has cache-capable upstreams, ignore non-cache providers.
            // This prevents selecting a fast-but-no-cache backend for long coding sessions.
            entries.retain(|(_, stat)| stat.cache_read_supported || stat.cache_write_supported);
        }

        if entries.is_empty() {
            return Vec::new();
        }

        let cache_vals: Vec<f64> = entries
            .iter()
            .filter_map(|(_, stat)| stat.avg_cache_hit)
            .collect();
        let throughput_vals: Vec<f64> = entries
            .iter()
            .filter_map(|(_, stat)| stat.avg_throughput)
            .collect();
        let cost_vals: Vec<f64> = entries
            .iter()
            .filter_map(|(_, stat)| stat.avg_cost_per_mtok)
            .collect();

        let (min_cache, max_cache) = min_max(&cache_vals);
        let (min_tp, max_tp) = min_max(&throughput_vals);
        let (min_cost, max_cost) = min_max(&cost_vals);

        let throughput_range = match (min_tp, max_tp) {
            (Some(min), Some(max)) if max > 0.0 => (max - min) / max,
            _ => 0.0,
        };

        let (w_cache, w_tp, w_cost) = if throughput_range < THROUGHPUT_SIMILARITY_THRESHOLD {
            (
                WEIGHT_CACHE_BALANCED,
                WEIGHT_SPEED_BALANCED,
                WEIGHT_COST_BALANCED,
            )
        } else {
            (
                WEIGHT_CACHE_PRIMARY,
                WEIGHT_SPEED_PRIMARY,
                WEIGHT_COST_PRIMARY,
            )
        };

        let mut scored: Vec<(f64, String)> = entries
            .drain(..)
            .map(|(provider, stat)| {
                let cache_impl_score = match (stat.cache_read_supported, stat.cache_write_supported)
                {
                    (true, true) => 1.0,
                    (true, false) | (false, true) => 0.75,
                    (false, false) => 0.0,
                };
                let cache_score = if cache_supported {
                    // Base cache score on implementation support, then refine with observed hit-rate.
                    // Hit-rate can vary by prompt shape; support is the stronger base signal.
                    stat.avg_cache_hit
                        .map(|v| normalize(v, min_cache, max_cache, cache_impl_score * 0.6))
                        .unwrap_or(cache_impl_score * 0.6)
                } else {
                    0.0
                };
                let tp_score = stat
                    .avg_throughput
                    .map(|v| normalize(v, min_tp, max_tp, 0.5))
                    .unwrap_or(0.5);
                let cost_score = stat
                    .avg_cost_per_mtok
                    .map(|v| normalize_inverse(v, min_cost, max_cost, 0.5))
                    .unwrap_or(0.5);
                // Downweight noisy one-off samples, but keep all providers eligible.
                let confidence = (stat.samples as f64 / 8.0).clamp(0.35, 1.0);
                let raw_score = w_cache * cache_score + w_tp * tp_score + w_cost * cost_score;
                let score = confidence * raw_score + (1.0 - confidence) * 0.5;
                (score, provider)
            })
            .collect();

        scored.sort_by(|a, b| b.0.partial_cmp(&a.0).unwrap_or(std::cmp::Ordering::Equal));
        scored.into_iter().map(|(_, p)| p).collect()
    }

    async fn effective_routing(&self, model: &str) -> ProviderRouting {
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

        let ranked = self.rank_providers(model);
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
        let mut current = self.provider_routing.write().await;
        *current = routing;
    }

    /// Get current provider routing
    pub async fn get_provider_routing(&self) -> ProviderRouting {
        self.provider_routing.read().await.clone()
    }

    /// Return a list of known/observed providers for a model (for autocomplete).
    pub fn available_providers_for_model(&self, model: &str) -> Vec<String> {
        let mut providers: Vec<String> = Vec::new();
        if let Ok(stats) = self.provider_stats.lock() {
            if let Some(model_stats) = stats.models.get(model) {
                providers.extend(model_stats.keys().cloned());
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
    /// Falls back to local stats if no endpoints cache available.
    pub fn provider_details_for_model(&self, model: &str) -> Vec<(String, String)> {
        // Try endpoints disk cache first (has pricing, uptime, cache info)
        if let Some(endpoints) = load_endpoints_disk_cache(model) {
            return endpoints
                .iter()
                .map(|e| (e.provider_name.clone(), e.detail_string()))
                .collect();
        }

        // Fall back to local observed stats
        if let Ok(stats) = self.provider_stats.lock() {
            if let Some(model_stats) = stats.models.get(model) {
                let mut details: Vec<(String, String)> = model_stats
                    .iter()
                    .map(|(name, s)| {
                        let mut parts = Vec::new();
                        if let Some(tps) = s.avg_throughput {
                            parts.push(format!("{:.0} tps", tps));
                        }
                        if let Some(cache) = s.avg_cache_hit {
                            if cache > 0.0 {
                                parts.push(format!("{:.0}% cache", cache * 100.0));
                            }
                        }
                        (name.clone(), parts.join(", "))
                    })
                    .collect();
                details.sort_by(|a, b| a.0.cmp(&b.0));
                return details;
            }
        }

        Vec::new()
    }

    /// Check if OPENROUTER_API_KEY is available (env var or config file)
    pub fn has_credentials() -> bool {
        Self::get_api_key().is_some()
    }

    /// Get API key from environment or config file
    fn get_api_key() -> Option<String> {
        // First check environment variable
        if let Ok(key) = std::env::var("OPENROUTER_API_KEY") {
            return Some(key);
        }

        // Fall back to config file
        let config_path = dirs::config_dir()?.join("jcode").join("openrouter.env");
        let content = std::fs::read_to_string(config_path).ok()?;

        for line in content.lines() {
            if let Some(key) = line.strip_prefix("OPENROUTER_API_KEY=") {
                let key = key.trim().trim_matches('"').trim_matches('\'');
                if !key.is_empty() {
                    return Some(key.to_string());
                }
            }
        }

        None
    }

    /// Fetch available models from OpenRouter API (with disk caching)
    pub async fn fetch_models(&self) -> Result<Vec<ModelInfo>> {
        // Check in-memory cache first
        {
            let cache = self.models_cache.read().await;
            if cache.fetched {
                return Ok(cache.models.clone());
            }
        }

        // Check disk cache
        if let Some(models) = load_disk_cache() {
            let mut cache = self.models_cache.write().await;
            cache.models = models.clone();
            cache.fetched = true;
            return Ok(models);
        }

        // Fetch from API
        let url = format!("{}/models", API_BASE);
        let response = self
            .client
            .get(&url)
            .header("Authorization", format!("Bearer {}", self.api_key))
            .send()
            .await
            .context("Failed to fetch models from OpenRouter")?;

        if !response.status().is_success() {
            let status = response.status();
            let body = response.text().await.unwrap_or_default();
            anyhow::bail!("OpenRouter API error ({}): {}", status, body);
        }

        #[derive(Deserialize)]
        struct ModelsResponse {
            data: Vec<ModelInfo>,
        }

        let models_response: ModelsResponse = response
            .json()
            .await
            .context("Failed to parse models response")?;

        // Save to disk cache
        save_disk_cache(&models_response.data);

        // Update in-memory cache
        {
            let mut cache = self.models_cache.write().await;
            cache.models = models_response.data.clone();
            cache.fetched = true;
        }

        Ok(models_response.data)
    }

    /// Force refresh the models cache from API
    pub async fn refresh_models(&self) -> Result<Vec<ModelInfo>> {
        // Clear in-memory cache
        {
            let mut cache = self.models_cache.write().await;
            cache.fetched = false;
            cache.models.clear();
        }

        // Delete disk cache
        let _ = std::fs::remove_file(cache_path());

        // Fetch fresh
        self.fetch_models().await
    }

    /// Fetch per-provider endpoint data for a model from OpenRouter API.
    /// Returns cached data if available and fresh (1-hour TTL).
    pub async fn fetch_endpoints(&self, model: &str) -> Result<Vec<EndpointInfo>> {
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
        let url = format!("{}/models/{}/endpoints", API_BASE, model);
        let response = self
            .client
            .get(&url)
            .header("Authorization", format!("Bearer {}", self.api_key))
            .send()
            .await
            .context("Failed to fetch endpoints from OpenRouter")?;

        if !response.status().is_success() {
            let status = response.status();
            let body = response.text().await.unwrap_or_default();
            anyhow::bail!("OpenRouter endpoints API error ({}): {}", status, body);
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
        let Some(pricing) = self.model_pricing(model_id).await else {
            return false;
        };

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

        has_cache_read || has_cache_write
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
        let cache_supported = if self.model_supports_cache(&model).await {
            true
        } else {
            let stats = self.provider_stats.lock().unwrap();
            stats
                .models
                .get(&model)
                .map(|m| {
                    m.values()
                        .any(|s| s.cache_read_supported || s.cache_write_supported)
                })
                .unwrap_or(false)
        };
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

        let build_content_parts = |blocks: &[ContentBlock]| -> Vec<Value> {
            let mut parts = Vec::new();
            for block in blocks {
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
                        parts.push(part);
                    }
                    ContentBlock::Image { media_type, data } => {
                        parts.push(serde_json::json!({
                            "type": "image_url",
                            "image_url": {
                                "url": format!("data:{};base64,{}", media_type, data)
                            }
                        }));
                    }
                    _ => {}
                }
            }
            parts
        };

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
                    let parts = build_content_parts(&msg.content);
                    if let Some(content) = content_from_parts(parts) {
                        api_messages.push(serde_json::json!({
                            "role": "user",
                            "content": content
                        }));
                    }

                    for block in &msg.content {
                        if let ContentBlock::ToolResult {
                            tool_use_id,
                            content,
                            is_error,
                        } = block
                        {
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
                                    "tool_call_id": tool_use_id,
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
                    }
                }
                Role::Assistant => {
                    let mut text_content = String::new();
                    let mut reasoning_content = String::new();
                    let mut tool_calls = Vec::new();
                    let mut post_tool_outputs: Vec<(String, String)> = Vec::new();
                    let mut missing_tool_outputs: Vec<String> = Vec::new();

                    for block in &msg.content {
                        match block {
                            ContentBlock::Text { text, .. } => {
                                text_content.push_str(text);
                            }
                            ContentBlock::ToolUse { id, name, input } => {
                                tool_calls.push(serde_json::json!({
                                    "id": id,
                                    "type": "function",
                                    "function": {
                                        "name": name,
                                        "arguments": serde_json::to_string(input).unwrap_or_default()
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
                    {
                        if !reasoning_content.is_empty() || !tool_calls.is_empty() {
                            let reasoning_payload =
                                if reasoning_content.is_empty() && !tool_calls.is_empty() {
                                    " ".to_string()
                                } else {
                                    reasoning_content
                                };
                            assistant_msg["reasoning_content"] =
                                serde_json::json!(reasoning_payload);
                        }
                    }

                    if !text_content.is_empty() || !tool_calls.is_empty() {
                        api_messages.push(assistant_msg);

                        for (tool_call_id, output) in post_tool_outputs {
                            api_messages.push(serde_json::json!({
                                "role": "tool",
                                "tool_call_id": tool_call_id,
                                "content": output
                            }));
                        }

                        if !missing_tool_outputs.is_empty() {
                            injected_missing += missing_tool_outputs.len();
                            for missing_id in missing_tool_outputs {
                                api_messages.push(serde_json::json!({
                                    "role": "tool",
                                    "tool_call_id": missing_id,
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

        // Add provider routing if configured
        let routing = self.effective_routing(&model).await;
        let mut provider_obj = None;
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

        if cache_control_added {
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
        let api_key = self.api_key.clone();
        let request_for_retries = request;
        let model_for_stream = model.clone();
        let provider_stats = Arc::clone(&self.provider_stats);
        let provider_pin = Arc::clone(&self.provider_pin);

        tokio::spawn(async move {
            run_stream_with_retries(
                client,
                api_key,
                request_for_retries,
                tx,
                provider_stats,
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

        if let Some(provider) = provider {
            self.set_explicit_pin(&model_id, provider);
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
        if let Ok(cache) = self.models_cache.try_read() {
            if cache.fetched && !cache.models.is_empty() {
                return cache.models.iter().map(|m| m.id.clone()).collect();
            }
        }

        if let Some(models) = load_disk_cache() {
            if let Ok(mut cache) = self.models_cache.try_write() {
                cache.models = models.clone();
                cache.fetched = true;
            }
            return models.into_iter().map(|m| m.id).collect();
        }

        Vec::new()
    }

    async fn prefetch_models(&self) -> Result<()> {
        let _ = self.fetch_models().await?;
        Ok(())
    }

    fn supports_compaction(&self) -> bool {
        true
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
        crate::provider::context_limit_for_model(&model_id)
            .unwrap_or(crate::provider::DEFAULT_CONTEXT_LIMIT)
    }

    fn fork(&self) -> Arc<dyn Provider> {
        Arc::new(Self {
            client: self.client.clone(),
            model: Arc::new(RwLock::new(
                self.model.try_read().map(|m| m.clone()).unwrap_or_default(),
            )),
            api_key: self.api_key.clone(),
            models_cache: Arc::clone(&self.models_cache),
            provider_routing: Arc::new(RwLock::new(
                self.provider_routing
                    .try_read()
                    .map(|r| r.clone())
                    .unwrap_or_default(),
            )),
            provider_stats: Arc::clone(&self.provider_stats),
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
    api_key: String,
    request: Value,
    tx: mpsc::Sender<Result<StreamEvent>>,
    provider_stats: Arc<Mutex<ProviderStatsStore>>,
    provider_pin: Arc<Mutex<Option<ProviderPin>>>,
    model: String,
) {
    let mut last_error = None;

    for attempt in 0..MAX_RETRIES {
        if attempt > 0 {
            let delay = RETRY_BASE_DELAY_MS * (1 << (attempt - 1));
            tokio::time::sleep(std::time::Duration::from_millis(delay)).await;
            crate::logging::info(&format!(
                "Retrying OpenRouter API request (attempt {}/{})",
                attempt + 1,
                MAX_RETRIES
            ));
        }

        crate::logging::info(&format!(
            "OpenRouter stream attempt {}/{} over HTTPS transport (model: {})",
            attempt + 1,
            MAX_RETRIES,
            model
        ));

        match stream_response(
            client.clone(),
            api_key.clone(),
            request.clone(),
            tx.clone(),
            Arc::clone(&provider_stats),
            Arc::clone(&provider_pin),
            model.clone(),
        )
        .await
        {
            Ok(()) => return,
            Err(e) => {
                let error_str = e.to_string().to_lowercase();
                if is_retryable_error(&error_str) && attempt + 1 < MAX_RETRIES {
                    crate::logging::info(&format!("OpenRouter transient error, will retry: {}", e));
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
    api_key: String,
    request: Value,
    tx: mpsc::Sender<Result<StreamEvent>>,
    provider_stats: Arc<Mutex<ProviderStatsStore>>,
    provider_pin: Arc<Mutex<Option<ProviderPin>>>,
    model: String,
) -> Result<()> {
    let url = format!("{}/chat/completions", API_BASE);
    let response = client
        .post(&url)
        .header("Authorization", format!("Bearer {}", api_key))
        .header("Content-Type", "application/json")
        .header("Accept-Encoding", "identity")
        .header("HTTP-Referer", "https://github.com/jcode")
        .header("X-Title", "jcode")
        .json(&request)
        .send()
        .await
        .context("Failed to send request to OpenRouter")?;

    if !response.status().is_success() {
        let status = response.status();
        let body = response.text().await.unwrap_or_default();
        anyhow::bail!("OpenRouter API error ({}): {}", status, body);
    }

    let mut stream = OpenRouterStream::new(
        response.bytes_stream(),
        model.clone(),
        provider_stats,
        provider_pin,
    );

    while let Some(event) = stream.next().await {
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
    provider_stats: Arc<Mutex<ProviderStatsStore>>,
    provider_pin: Arc<Mutex<Option<ProviderPin>>>,
    provider_name: Option<String>,
    last_usage: Option<UsageSnapshot>,
    started_at: Instant,
    stats_recorded: bool,
}

#[derive(Default)]
struct ToolCallAccumulator {
    id: String,
    name: String,
    arguments: String,
}

#[derive(Debug, Clone)]
struct UsageSnapshot {
    input_tokens: Option<u64>,
    output_tokens: Option<u64>,
    cache_read_input_tokens: Option<u64>,
    cache_creation_input_tokens: Option<u64>,
    cost: Option<f64>,
}

fn parse_f64(value: &Value) -> Option<f64> {
    value
        .as_f64()
        .or_else(|| value.as_str().and_then(|s| s.parse::<f64>().ok()))
}

impl OpenRouterStream {
    fn new(
        stream: impl Stream<Item = Result<Bytes, reqwest::Error>> + Send + 'static,
        model: String,
        provider_stats: Arc<Mutex<ProviderStatsStore>>,
        provider_pin: Arc<Mutex<Option<ProviderPin>>>,
    ) -> Self {
        Self {
            inner: Box::pin(stream),
            buffer: String::new(),
            pending: VecDeque::new(),
            current_tool_call: None,
            provider_emitted: false,
            model,
            provider_stats,
            provider_pin,
            provider_name: None,
            last_usage: None,
            started_at: Instant::now(),
            stats_recorded: false,
        }
    }

    fn observe_provider(&mut self, provider: &str) {
        self.provider_name = Some(provider.to_string());

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

    fn record_stats(&mut self) {
        if self.stats_recorded {
            return;
        }
        self.stats_recorded = true;

        let provider = match self.provider_name.clone() {
            Some(p) => p,
            None => return,
        };
        let usage = match self.last_usage.clone() {
            Some(u) => u,
            None => return,
        };

        let duration_secs = self.started_at.elapsed().as_secs_f64().max(0.001);
        let throughput = usage
            .output_tokens
            .map(|tokens| tokens as f64 / duration_secs);

        let cache_hit = match (usage.cache_read_input_tokens, usage.input_tokens) {
            (Some(cached), Some(total)) if total > 0 => Some(cached as f64 / total as f64),
            _ => None,
        };

        let total_tokens = usage.input_tokens.unwrap_or(0) + usage.output_tokens.unwrap_or(0);
        let cost_per_mtok = usage.cost.and_then(|cost| {
            if total_tokens > 0 {
                Some(cost / total_tokens as f64 * 1_000_000.0)
            } else {
                None
            }
        });

        let sample = ProviderSample {
            cache_hit,
            throughput,
            cost_per_mtok,
        };

        let mut stats = self.provider_stats.lock().unwrap();
        let model_entry = stats
            .models
            .entry(self.model.clone())
            .or_insert_with(HashMap::new);
        let entry = model_entry
            .entry(provider.clone())
            .or_insert_with(ProviderStats::default);

        entry.samples = entry.samples.saturating_add(1);
        entry.last_seen = now_epoch_secs();
        if usage.cache_read_input_tokens.is_some() {
            entry.cache_read_supported = true;
        }
        if usage.cache_creation_input_tokens.is_some() {
            entry.cache_write_supported = true;
        }

        if let Some(cache_hit) = sample.cache_hit {
            entry.avg_cache_hit = Some(update_ewma(entry.avg_cache_hit, cache_hit));
        }
        if let Some(throughput) = sample.throughput {
            entry.avg_throughput = Some(update_ewma(entry.avg_throughput, throughput));
        }
        if let Some(cost_per_mtok) = sample.cost_per_mtok {
            entry.avg_cost_per_mtok = Some(update_ewma(entry.avg_cost_per_mtok, cost_per_mtok));
        }

        let snapshot = stats.clone();
        drop(stats);
        save_provider_stats(&snapshot);

        if usage.cache_read_input_tokens.is_some() || usage.cache_creation_input_tokens.is_some() {
            let mut pin = self.provider_pin.lock().unwrap();
            if let Some(existing) = pin.as_mut() {
                if existing.model == self.model && existing.provider == provider {
                    existing.last_cache_read = Some(Instant::now());
                }
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
                self.record_stats();
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
                            let index = tc.get("index").and_then(|i| i.as_u64()).unwrap_or(0);

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
                    if let Some(finish_reason) =
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

                let cost = usage
                    .get("total_cost")
                    .and_then(parse_f64)
                    .or_else(|| usage.get("cost").and_then(parse_f64))
                    .or_else(|| {
                        let prompt_cost = usage.get("prompt_cost").and_then(parse_f64);
                        let completion_cost = usage.get("completion_cost").and_then(parse_f64);
                        match (prompt_cost, completion_cost) {
                            (Some(p), Some(c)) => Some(p + c),
                            (Some(p), None) => Some(p),
                            (None, Some(c)) => Some(c),
                            _ => None,
                        }
                    });

                self.last_usage = Some(UsageSnapshot {
                    input_tokens,
                    output_tokens,
                    cache_read_input_tokens,
                    cache_creation_input_tokens,
                    cost,
                });

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

impl Drop for OpenRouterStream {
    fn drop(&mut self) {
        self.record_stats();
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::collections::HashMap;

    #[test]
    fn test_has_credentials() {
        // has_credentials() checks both env var AND config file
        // So we just verify it returns a boolean without panicking
        let _has_creds = OpenRouterProvider::has_credentials();
        // If we got here, the function works
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

    #[test]
    fn test_rank_providers_cache_priority() {
        let now = now_epoch_secs();
        let mut stats = ProviderStatsStore::default();
        let mut model_stats = HashMap::new();
        model_stats.insert(
            "FastCache".to_string(),
            ProviderStats {
                samples: 5,
                avg_cache_hit: Some(0.5),
                avg_throughput: Some(50.0),
                avg_cost_per_mtok: Some(2.0),
                last_seen: now,
                cache_read_supported: true,
                cache_write_supported: false,
            },
        );
        model_stats.insert(
            "FasterNoCache".to_string(),
            ProviderStats {
                samples: 5,
                avg_cache_hit: Some(0.1),
                avg_throughput: Some(60.0),
                avg_cost_per_mtok: Some(1.0),
                last_seen: now,
                cache_read_supported: false,
                cache_write_supported: false,
            },
        );
        stats.models.insert("test/model".to_string(), model_stats);

        let provider = OpenRouterProvider {
            client: Client::new(),
            model: Arc::new(RwLock::new("test/model".to_string())),
            api_key: "test".to_string(),
            models_cache: Arc::new(RwLock::new(ModelsCache::default())),
            provider_routing: Arc::new(RwLock::new(ProviderRouting::default())),
            provider_stats: Arc::new(Mutex::new(stats)),
            provider_pin: Arc::new(Mutex::new(None)),
            endpoints_cache: Arc::new(RwLock::new(HashMap::new())),
        };

        let ranked = provider.rank_providers("test/model");
        assert_eq!(ranked.first().map(|s| s.as_str()), Some("FastCache"));
    }

    #[test]
    fn test_rank_providers_speed_priority_among_cache_capable() {
        let now = now_epoch_secs();
        let mut stats = ProviderStatsStore::default();
        let mut model_stats = HashMap::new();
        model_stats.insert(
            "Fireworks".to_string(),
            ProviderStats {
                samples: 12,
                avg_cache_hit: Some(0.30),
                avg_throughput: Some(120.0),
                avg_cost_per_mtok: Some(1.3),
                last_seen: now,
                cache_read_supported: true,
                cache_write_supported: true,
            },
        );
        model_stats.insert(
            "Moonshot AI".to_string(),
            ProviderStats {
                samples: 12,
                avg_cache_hit: Some(0.70),
                avg_throughput: Some(80.0),
                avg_cost_per_mtok: Some(1.0),
                last_seen: now,
                cache_read_supported: true,
                cache_write_supported: true,
            },
        );
        stats
            .models
            .insert("moonshotai/kimi-k2.5".to_string(), model_stats);

        let provider = OpenRouterProvider {
            client: Client::new(),
            model: Arc::new(RwLock::new("moonshotai/kimi-k2.5".to_string())),
            api_key: "test".to_string(),
            models_cache: Arc::new(RwLock::new(ModelsCache::default())),
            provider_routing: Arc::new(RwLock::new(ProviderRouting::default())),
            provider_stats: Arc::new(Mutex::new(stats)),
            provider_pin: Arc::new(Mutex::new(None)),
            endpoints_cache: Arc::new(RwLock::new(HashMap::new())),
        };

        let ranked = provider.rank_providers("moonshotai/kimi-k2.5");
        assert_eq!(ranked.first().map(|s| s.as_str()), Some("Fireworks"));
    }

    #[test]
    fn test_kimi_fallback_prefers_fireworks_without_stats() {
        let provider = OpenRouterProvider {
            client: Client::new(),
            model: Arc::new(RwLock::new("moonshotai/kimi-k2.5".to_string())),
            api_key: "test".to_string(),
            models_cache: Arc::new(RwLock::new(ModelsCache::default())),
            provider_routing: Arc::new(RwLock::new(ProviderRouting::default())),
            provider_stats: Arc::new(Mutex::new(ProviderStatsStore::default())),
            provider_pin: Arc::new(Mutex::new(None)),
            endpoints_cache: Arc::new(RwLock::new(HashMap::new())),
        };

        let rt = tokio::runtime::Builder::new_current_thread()
            .enable_all()
            .build()
            .expect("runtime");
        let routing = rt.block_on(provider.effective_routing("moonshotai/kimi-k2.5"));
        let order = routing.order.expect("provider order");
        assert_eq!(order.first().map(|s| s.as_str()), Some("Fireworks"));
        assert!(
            !routing.allow_fallbacks,
            "Kimi should disable fallbacks to force provider"
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
