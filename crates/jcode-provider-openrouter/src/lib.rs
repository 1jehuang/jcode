use serde::{Deserialize, Serialize};
use std::collections::HashMap;
use std::path::PathBuf;
use std::sync::{LazyLock, Mutex};
use std::time::{Instant, SystemTime};

const CACHE_TTL_SECS: u64 = 24 * 60 * 60;
const ENDPOINTS_CACHE_TTL_SECS: u64 = 60 * 60;
const DEFAULT_CACHE_NAMESPACE: &str = "openrouter";

/// Default provider order for Kimi models when no local stats exist yet.
/// Ordered for practical coding use: speed first, then cache quality, then cost.
pub const KIMI_FALLBACK_PROVIDERS: &[&str] = &["Fireworks", "Moonshot AI", "Together", "DeepInfra"];

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

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct ModelInfo {
    pub id: String,
    #[serde(default)]
    pub name: String,
    pub context_length: Option<u64>,
    #[serde(default)]
    pub pricing: ModelPricing,
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
    fn extract_p50(value: &serde_json::Value) -> Option<f64> {
        match value {
            serde_json::Value::Number(n) => n.as_f64(),
            serde_json::Value::Object(map) => map.get("p50").and_then(|v| v.as_f64()),
            _ => None,
        }
    }

    pub fn detail_string(&self) -> String {
        let mut parts = Vec::new();
        if let Some(ref prompt) = self.pricing.prompt
            && let Ok(p) = prompt.parse::<f64>()
        {
            parts.push(format!("${:.2}/M", p * 1e6));
        }
        if let Some(uptime) = self.uptime_last_30m {
            parts.push(format!("{:.0}%", uptime));
        }
        if let Some(ref tps) = self.throughput_last_30m
            && let Some(t) = Self::extract_p50(tps)
            && t > 0.0
        {
            parts.push(format!("{:.0}tps", t));
        }
        if let Some(ref cache_read) = self.pricing.input_cache_read
            && let Ok(cr) = cache_read.parse::<f64>()
            && cr > 0.0
        {
            parts.push("cache".to_string());
        }
        if let Some(ref q) = self.quantization
            && q != "unknown"
        {
            parts.push(q.clone());
        }
        parts.join(", ")
    }
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct DiskCache {
    pub cached_at: u64,
    pub models: Vec<ModelInfo>,
}

#[derive(Debug, Clone)]
struct DiskCacheMemoEntry {
    modified_at: Option<SystemTime>,
    cache: Option<DiskCache>,
}

static DISK_CACHE_MEMO: LazyLock<Mutex<HashMap<PathBuf, DiskCacheMemoEntry>>> =
    LazyLock::new(|| Mutex::new(HashMap::new()));

#[derive(Debug, Clone, Serialize, Deserialize)]
struct EndpointsDiskCache {
    cached_at: u64,
    endpoints: Vec<EndpointInfo>,
}

#[derive(Debug, Clone)]
struct EndpointsDiskCacheMemoEntry {
    modified_at: Option<SystemTime>,
    cache: Option<EndpointsDiskCache>,
}

static ENDPOINTS_DISK_CACHE_MEMO: LazyLock<Mutex<HashMap<PathBuf, EndpointsDiskCacheMemoEntry>>> =
    LazyLock::new(|| Mutex::new(HashMap::new()));

#[derive(Debug, Default, Clone)]
pub struct ModelsCache {
    pub models: Vec<ModelInfo>,
    pub fetched: bool,
    pub cached_at: Option<u64>,
}

#[derive(Debug, Default, Clone)]
pub struct ModelCatalogRefreshState {
    pub in_flight: bool,
    pub last_attempt_unix: Option<u64>,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum PinSource {
    Explicit,
    Observed,
}

#[derive(Debug, Clone)]
pub struct ProviderPin {
    pub model: String,
    pub provider: String,
    pub source: PinSource,
    pub allow_fallbacks: bool,
    pub last_cache_read: Option<Instant>,
}

#[derive(Debug, Clone)]
pub struct ParsedProvider {
    pub name: String,
    pub allow_fallbacks: bool,
}

pub fn normalize_provider_name(raw: &str) -> String {
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

pub fn parse_model_spec(raw: &str) -> (String, Option<ParsedProvider>) {
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

pub fn current_unix_secs() -> Option<u64> {
    std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .ok()
        .map(|d| d.as_secs())
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

fn cache_path() -> PathBuf {
    let namespace = configured_cache_namespace();
    dirs::home_dir()
        .unwrap_or_else(|| PathBuf::from("."))
        .join(".jcode")
        .join("cache")
        .join(format!("{}_models.json", namespace))
}

fn disk_cache_modified_at(path: &PathBuf) -> Option<SystemTime> {
    std::fs::metadata(path).ok()?.modified().ok()
}

fn fresh_disk_cache(cache: Option<DiskCache>) -> Option<DiskCache> {
    let now = current_unix_secs()?;
    let cache = cache?;
    if now.saturating_sub(cache.cached_at) < CACHE_TTL_SECS {
        Some(cache)
    } else {
        None
    }
}

pub fn load_disk_cache_entry() -> Option<DiskCache> {
    let path = cache_path();
    let modified_at = disk_cache_modified_at(&path);

    if let Ok(memo) = DISK_CACHE_MEMO.lock()
        && let Some(entry) = memo.get(&path)
        && entry.modified_at == modified_at
    {
        return fresh_disk_cache(entry.cache.clone());
    }

    let loaded = std::fs::read_to_string(&path)
        .ok()
        .and_then(|content| serde_json::from_str::<DiskCache>(&content).ok());

    if let Ok(mut memo) = DISK_CACHE_MEMO.lock() {
        memo.insert(
            path,
            DiskCacheMemoEntry {
                modified_at,
                cache: loaded.clone(),
            },
        );
    }

    fresh_disk_cache(loaded)
}

pub fn load_disk_cache() -> Option<Vec<ModelInfo>> {
    load_disk_cache_entry().map(|cache| cache.models)
}

pub fn load_model_pricing_disk_cache_public(model_id: &str) -> Option<ModelPricing> {
    load_disk_cache()?
        .into_iter()
        .find(|model| model.id == model_id)
        .map(|model| model.pricing)
}

pub type ModelTimestampIndex = HashMap<String, u64>;

pub fn model_created_timestamp(model_id: &str) -> Option<u64> {
    let timestamps = load_model_timestamp_index();
    model_created_timestamp_from_index(model_id, &timestamps)
}

pub fn model_created_timestamp_from_index(
    model_id: &str,
    timestamps: &ModelTimestampIndex,
) -> Option<u64> {
    if let Some(ts) = timestamps.get(model_id).copied() {
        return Some(ts);
    }

    let candidates = openrouter_id_candidates(model_id);
    for candidate in &candidates {
        if let Some(ts) = timestamps.get(candidate).copied() {
            return Some(ts);
        }
    }

    None
}

fn openrouter_id_candidates(model: &str) -> Vec<String> {
    let mut candidates = Vec::new();
    if model.starts_with("claude-") || model.starts_with("claude_") {
        candidates.push(format!("anthropic/{}", model));
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

pub fn load_model_timestamp_index() -> ModelTimestampIndex {
    all_model_timestamps().into_iter().collect()
}

pub fn all_model_timestamps() -> Vec<(String, u64)> {
    load_disk_cache_entry()
        .into_iter()
        .flat_map(|cache| cache.models)
        .filter_map(|m| m.created.map(|t| (m.id, t)))
        .collect()
}

pub fn save_disk_cache(models: &[ModelInfo]) {
    let path = cache_path();
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

    if let Ok(mut memo) = DISK_CACHE_MEMO.lock() {
        memo.insert(
            path.clone(),
            DiskCacheMemoEntry {
                modified_at: disk_cache_modified_at(&path),
                cache: Some(cache),
            },
        );
    }
}

fn endpoints_cache_path(model: &str) -> PathBuf {
    let safe_name = model.replace('/', "__");
    let namespace = configured_cache_namespace();
    dirs::home_dir()
        .unwrap_or_else(|| PathBuf::from("."))
        .join(".jcode")
        .join("cache")
        .join(format!("{}_endpoints_{}.json", namespace, safe_name))
}

pub fn load_endpoints_disk_cache_public(model: &str) -> Option<(Vec<EndpointInfo>, u64)> {
    let path = endpoints_cache_path(model);
    let modified_at = disk_cache_modified_at(&path);
    let cache = if let Ok(memo) = ENDPOINTS_DISK_CACHE_MEMO.lock()
        && let Some(entry) = memo.get(&path)
        && entry.modified_at == modified_at
    {
        entry.cache.clone()?
    } else {
        let loaded = std::fs::read_to_string(&path)
            .ok()
            .and_then(|content| serde_json::from_str::<EndpointsDiskCache>(&content).ok());
        if let Ok(mut memo) = ENDPOINTS_DISK_CACHE_MEMO.lock() {
            memo.insert(
                path.clone(),
                EndpointsDiskCacheMemoEntry {
                    modified_at,
                    cache: loaded.clone(),
                },
            );
        }
        loaded?
    };
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

pub fn load_endpoints_disk_cache(model: &str) -> Option<Vec<EndpointInfo>> {
    let path = endpoints_cache_path(model);
    let modified_at = disk_cache_modified_at(&path);
    let cache = if let Ok(memo) = ENDPOINTS_DISK_CACHE_MEMO.lock()
        && let Some(entry) = memo.get(&path)
        && entry.modified_at == modified_at
    {
        entry.cache.clone()?
    } else {
        let loaded = std::fs::read_to_string(&path)
            .ok()
            .and_then(|content| serde_json::from_str::<EndpointsDiskCache>(&content).ok());
        if let Ok(mut memo) = ENDPOINTS_DISK_CACHE_MEMO.lock() {
            memo.insert(
                path.clone(),
                EndpointsDiskCacheMemoEntry {
                    modified_at,
                    cache: loaded.clone(),
                },
            );
        }
        loaded?
    };
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

pub fn save_endpoints_disk_cache(model: &str, endpoints: &[EndpointInfo]) {
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

    if let Ok(mut memo) = ENDPOINTS_DISK_CACHE_MEMO.lock() {
        memo.insert(
            path.clone(),
            EndpointsDiskCacheMemoEntry {
                modified_at: disk_cache_modified_at(&path),
                cache: Some(cache),
            },
        );
    }
}

#[derive(Debug, Clone)]
pub struct ProviderRouting {
    pub order: Option<Vec<String>>,
    pub allow_fallbacks: bool,
    pub sort: Option<String>,
    pub preferred_min_throughput: Option<u32>,
    pub preferred_max_latency: Option<u32>,
    pub max_price: Option<f64>,
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
    pub fn is_empty(&self) -> bool {
        self.order.is_none()
            && self.sort.is_none()
            && self.preferred_min_throughput.is_none()
            && self.preferred_max_latency.is_none()
            && self.max_price.is_none()
            && self.require_parameters.is_none()
            && self.allow_fallbacks
    }
}

pub fn parse_provider_routing_from_env() -> ProviderRouting {
    let mut routing = ProviderRouting::default();

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

    if std::env::var("JCODE_OPENROUTER_NO_FALLBACK").is_ok() {
        routing.allow_fallbacks = false;
    }

    routing
}

pub fn is_kimi_model(model: &str) -> bool {
    let lower = model.to_lowercase();
    lower.contains("moonshotai/") || lower.contains("kimi-k2") || lower.contains("kimi-k2.5")
}

pub fn rank_providers_from_endpoints(endpoints: &[EndpointInfo]) -> Vec<String> {
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

    let mut candidates: Vec<&EndpointInfo> =
        endpoints.iter().filter(|e| e.status != Some(1)).collect();

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
                .and_then(EndpointInfo::extract_p50)
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

            let score = 0.50 * throughput.min(200.0) / 200.0 + 0.30 * uptime + 0.20 * cost_score;

            (score, e.provider_name.as_str())
        })
        .collect();

    scored.sort_by(|a, b| b.0.partial_cmp(&a.0).unwrap_or(std::cmp::Ordering::Equal));
    scored
        .into_iter()
        .map(|(_, name)| name.to_string())
        .collect()
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn parse_model_spec_handles_provider_aliases_and_auto() {
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
    fn model_created_timestamp_from_index_handles_provider_aliases() {
        let timestamps = ModelTimestampIndex::from([
            ("anthropic/claude-opus-4.7".to_string(), 100),
            ("openai/gpt-5.4".to_string(), 200),
            ("moonshotai/kimi-k2.6".to_string(), 300),
        ]);

        assert_eq!(
            model_created_timestamp_from_index("claude-opus-4-7", &timestamps),
            Some(100)
        );
        assert_eq!(
            model_created_timestamp_from_index("gpt-5.4", &timestamps),
            Some(200)
        );
        assert_eq!(
            model_created_timestamp_from_index("moonshotai/kimi-k2.6", &timestamps),
            Some(300)
        );
        assert_eq!(
            model_created_timestamp_from_index("unknown-model", &timestamps),
            None
        );
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

    #[test]
    fn rank_providers_prioritizes_cache_then_speed() {
        let endpoints = vec![
            make_endpoint("FastCache", 50.0, 99.0, true, 0.0000002),
            make_endpoint("FasterNoCache", 60.0, 99.0, false, 0.0000001),
        ];

        let ranked = rank_providers_from_endpoints(&endpoints);
        assert_eq!(ranked.first().map(|s| s.as_str()), Some("FastCache"));
    }

    #[test]
    fn endpoint_detail_string_formats_common_fields() {
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
            max_completion_tokens: Some(16384),
            quantization: Some("fp8".to_string()),
            uptime_last_30m: Some(99.2),
            latency_last_30m: None,
            throughput_last_30m: Some(serde_json::json!({"p50": 14.2})),
            supports_implicit_caching: Some(true),
            status: Some(0),
        };

        let detail = ep.detail_string();
        assert!(detail.contains("$0.45/M"));
        assert!(detail.contains("99%"));
        assert!(detail.contains("14tps"));
        assert!(detail.contains("cache"));
        assert!(detail.contains("fp8"));
    }
}
