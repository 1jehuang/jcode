//! OpenRouter API provider
//!
//! Uses OpenRouter's OpenAI-compatible API to access 200+ models from various providers.
//! Models are fetched dynamically from the API and cached to disk.
//!
//! Features:
//! - Provider pinning: Set JCODE_OPENROUTER_PROVIDER to pin to a specific provider (e.g., "Fireworks")
//! - Cache token parsing: Parses cached_tokens from OpenRouter responses for cache hit detection

use super::{EventStream, Provider};
use crate::message::{ContentBlock, Message, Role, StreamEvent, ToolDefinition};
use anyhow::{Context, Result};
use async_trait::async_trait;
use bytes::Bytes;
use futures::Stream;
use reqwest::Client;
use serde::{Deserialize, Serialize};
use serde_json::Value;
use std::collections::{HashMap, VecDeque};
use std::path::PathBuf;
use std::pin::Pin;
use std::sync::{Arc, Mutex};
use std::task::{Context as TaskContext, Poll};
use std::time::Instant;
use tokio::sync::RwLock;

/// OpenRouter API base URL
const API_BASE: &str = "https://openrouter.ai/api/v1";

/// Default model (Claude Sonnet via OpenRouter)
const DEFAULT_MODEL: &str = "anthropic/claude-sonnet-4";

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

/// Model info from OpenRouter API
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct ModelInfo {
    pub id: String,
    pub name: String,
    pub context_length: Option<u64>,
    #[serde(default)]
    pub pricing: ModelPricing,
}

#[derive(Debug, Clone, Default, Serialize, Deserialize)]
pub struct ModelPricing {
    pub prompt: Option<String>,
    pub completion: Option<String>,
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
        return (
            model.to_string(),
            Some(ParsedProvider {
                name: provider.to_string(),
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
}

impl OpenRouterProvider {
    /// Return true if this model is a Kimi K2/K2.5 variant (Moonshot).
    fn is_kimi_model(model: &str) -> bool {
        let lower = model.to_lowercase();
        lower.contains("moonshotai/")
            || lower.contains("kimi-k2")
            || lower.contains("kimi-k2.5")
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
        let api_key = Self::get_api_key()
            .ok_or_else(|| anyhow::anyhow!("OPENROUTER_API_KEY not found in environment or ~/.config/jcode/openrouter.env"))?;

        let model = std::env::var("JCODE_OPENROUTER_MODEL")
            .unwrap_or_else(|_| DEFAULT_MODEL.to_string());

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
                || (clear_explicit && existing.model == model && existing.source == PinSource::Explicit);
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
            (0.6, 0.2, 0.2)
        } else {
            (0.6, 0.3, 0.1)
        };

        let mut scored: Vec<(f64, String)> = entries
            .drain(..)
            .map(|(provider, stat)| {
                let cache_score = stat
                    .avg_cache_hit
                    .map(|v| normalize(v, min_cache, max_cache, 0.0))
                    .unwrap_or(0.0);
                let tp_score = stat
                    .avg_throughput
                    .map(|v| normalize(v, min_tp, max_tp, 0.5))
                    .unwrap_or(0.5);
                let cost_score = stat
                    .avg_cost_per_mtok
                    .map(|v| normalize_inverse(v, min_cost, max_cost, 0.5))
                    .unwrap_or(0.5);
                let score = w_cache * cache_score + w_tp * tp_score + w_cost * cost_score;
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

        // Build messages in OpenAI format
        let mut api_messages = Vec::new();

        // Add system message if provided
        if !system.is_empty() {
            api_messages.push(serde_json::json!({
                "role": "system",
                "content": system
            }));
        }

        // Convert messages
        for msg in messages {
            match msg.role {
                Role::User => {
                    for block in &msg.content {
                        match block {
                            ContentBlock::Text { text, .. } => {
                                api_messages.push(serde_json::json!({
                                    "role": "user",
                                    "content": text
                                }));
                            }
                            ContentBlock::ToolResult {
                                tool_use_id,
                                content,
                                is_error,
                            } => {
                                let output = if is_error == &Some(true) {
                                    format!("[Error] {}", content)
                                } else {
                                    content.clone()
                                };
                                api_messages.push(serde_json::json!({
                                    "role": "tool",
                                    "tool_call_id": tool_use_id,
                                    "content": output
                                }));
                            }
                            _ => {}
                        }
                    }
                }
                Role::Assistant => {
                    let mut text_content = String::new();
                    let mut tool_calls = Vec::new();

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
                        // Moonshot/Kimi requires reasoning_content on tool-call messages when thinking is enabled.
                        if Self::is_kimi_model(&model) {
                            assistant_msg["reasoning_content"] = serde_json::json!("");
                        }
                    }

                    if !text_content.is_empty() || !tool_calls.is_empty() {
                        api_messages.push(assistant_msg);
                    }
                }
            }
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
        if let Some(enable) = Self::thinking_override() {
            request["thinking"] = serde_json::json!({
                "type": if enable { "enabled" } else { "disabled" }
            });
        }

        // Add provider routing if configured
        let routing = self.effective_routing(&model).await;
        if !routing.is_empty() {
            let mut provider_obj = serde_json::json!({});
            if let Some(ref order) = routing.order {
                provider_obj["order"] = serde_json::json!(order);
            }
            if !routing.allow_fallbacks {
                provider_obj["allow_fallbacks"] = serde_json::json!(false);
            }
            if let Some(ref sort) = routing.sort {
                provider_obj["sort"] = serde_json::json!(sort);
            }
            if let Some(min_tp) = routing.preferred_min_throughput {
                provider_obj["preferred_min_throughput"] = serde_json::json!(min_tp);
            }
            if let Some(max_latency) = routing.preferred_max_latency {
                provider_obj["preferred_max_latency"] = serde_json::json!(max_latency);
            }
            if let Some(max_price) = routing.max_price {
                provider_obj["max_price"] = serde_json::json!(max_price);
            }
            if let Some(require_parameters) = routing.require_parameters {
                provider_obj["require_parameters"] = serde_json::json!(require_parameters);
            }
            request["provider"] = provider_obj;
        }

        // Send request
        let url = format!("{}/chat/completions", API_BASE);
        let response = self
            .client
            .post(&url)
            .header("Authorization", format!("Bearer {}", self.api_key))
            .header("Content-Type", "application/json")
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

        let stream = OpenRouterStream::new(
            response.bytes_stream(),
            model.clone(),
            Arc::clone(&self.provider_stats),
            Arc::clone(&self.provider_pin),
        );
        Ok(Box::pin(stream))
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
        })
    }
}

// ============================================================================
// SSE Stream Parser
// ============================================================================

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

        if usage.cache_read_input_tokens.unwrap_or(0) > 0 {
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
                    if let Some(finish_reason) = choice.get("finish_reason").and_then(|f| f.as_str())
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
                let input_tokens = usage
                    .get("prompt_tokens")
                    .and_then(|t| t.as_u64());
                let output_tokens = usage
                    .get("completion_tokens")
                    .and_then(|t| t.as_u64());

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
        };

        let ranked = provider.rank_providers("test/model");
        assert_eq!(ranked.first().map(|s| s.as_str()), Some("FastCache"));
    }
}
