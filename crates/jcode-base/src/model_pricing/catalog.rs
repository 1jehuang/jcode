//! models.dev catalog fetch, disk/memory cache, and response parsing.
//!
//! Split out of the parent module so the fetch/cache machinery and the
//! pricing rules that consume it can evolve separately. Everything here is
//! private to `model_pricing`; the parent re-exports the public surface.

use serde::{Deserialize, Serialize};
use std::collections::HashMap;
use std::path::PathBuf;
use std::sync::atomic::AtomicBool;
use std::sync::{Arc, Mutex};
use std::time::{Duration, SystemTime, UNIX_EPOCH};

pub(super) const API_URL: &str = "https://models.dev/api.json";
pub(super) const CACHE_FILE: &str = "models_dev_pricing.json";
pub(super) const CACHE_TTL_SECS: u64 = 24 * 60 * 60;
pub(super) const HTTP_TIMEOUT: Duration = Duration::from_secs(20);

/// Per-model USD prices per million tokens.
#[derive(Debug, Clone, Copy, PartialEq, Serialize, Deserialize)]
pub struct ModelCost {
    pub input_usd_per_mtok: f64,
    pub output_usd_per_mtok: f64,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub cache_read_usd_per_mtok: Option<f64>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub cache_write_usd_per_mtok: Option<f64>,
}

#[derive(Debug, Clone, Default, Serialize, Deserialize)]
pub(super) struct PricingCache {
    pub(super) cached_at_unix_secs: u64,
    /// provider id -> model id -> cost. Provider ids are models.dev ids
    /// (e.g. `anthropic`, `openai`, `deepseek`, `moonshotai`).
    pub(super) providers: HashMap<String, HashMap<String, ModelCost>>,
}

/// In-memory pricing cache keyed by the on-disk cache path it was loaded
/// from, so changing `JCODE_HOME` (tests, multi-home setups) never serves
/// pricing that belongs to a different home directory.
///
/// Held behind an `Arc` so per-route pricing lookups share one parsed catalog
/// instead of deep-cloning the multi-thousand-model map on every call (that
/// clone dominated server CPU during client connect bursts).
pub(super) static MEMORY_CACHE: Mutex<Option<(PathBuf, Arc<PricingCache>)>> = Mutex::new(None);
pub(super) static REFRESH_IN_FLIGHT: AtomicBool = AtomicBool::new(false);

pub(super) fn cache_path() -> PathBuf {
    crate::storage::jcode_dir()
        .unwrap_or_else(|_| PathBuf::from(".").join(".jcode"))
        .join("cache")
        .join(CACHE_FILE)
}

pub(super) fn now_unix_secs() -> u64 {
    SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .map(|d| d.as_secs())
        .unwrap_or(0)
}

pub(super) fn load_cache() -> Option<Arc<PricingCache>> {
    let path = cache_path();
    {
        let memory = MEMORY_CACHE.lock().ok()?;
        if let Some((cached_path, cache)) = memory.as_ref()
            && cached_path == &path
        {
            return Some(Arc::clone(cache));
        }
    }
    let cache: PricingCache = crate::storage::read_json(&path).ok()?;
    let cache = Arc::new(cache);
    if let Ok(mut memory) = MEMORY_CACHE.lock() {
        *memory = Some((path, Arc::clone(&cache)));
    }
    Some(cache)
}

pub(super) fn save_cache(cache: &PricingCache) {
    let path = cache_path();
    if let Ok(mut memory) = MEMORY_CACHE.lock() {
        *memory = Some((path.clone(), Arc::new(cache.clone())));
    }
    let _ = crate::storage::write_json(&path, cache);
}

/// Fetch the catalog and persist the parsed cache.
pub(super) async fn refresh_now() -> anyhow::Result<()> {
    let client = crate::provider::shared_http_client();
    let response = client
        .get(API_URL)
        .header("Accept", "application/json")
        .timeout(HTTP_TIMEOUT)
        .send()
        .await?;
    if !response.status().is_success() {
        anyhow::bail!("HTTP {}", response.status());
    }
    let body = response.text().await?;
    let cache = parse_api_response(&body)?;
    save_cache(&cache);
    crate::logging::info(&format!(
        "models.dev pricing refreshed: {} providers, {} priced models",
        cache.providers.len(),
        cache.providers.values().map(HashMap::len).sum::<usize>()
    ));
    Ok(())
}

pub(super) fn parse_api_response(body: &str) -> anyhow::Result<PricingCache> {
    let json: serde_json::Value = serde_json::from_str(body)?;
    let top = json
        .as_object()
        .ok_or_else(|| anyhow::anyhow!("expected top-level provider object"))?;

    let mut providers: HashMap<String, HashMap<String, ModelCost>> = HashMap::new();
    for (provider_id, provider) in top {
        let Some(models) = provider.get("models").and_then(|m| m.as_object()) else {
            continue;
        };
        let mut parsed_models = HashMap::new();
        for (model_id, model) in models {
            let Some(cost) = model.get("cost") else {
                continue;
            };
            let (Some(input), Some(output)) = (
                cost.get("input").and_then(|v| v.as_f64()),
                cost.get("output").and_then(|v| v.as_f64()),
            ) else {
                continue;
            };
            parsed_models.insert(
                model_id.clone(),
                ModelCost {
                    input_usd_per_mtok: input,
                    output_usd_per_mtok: output,
                    cache_read_usd_per_mtok: cost.get("cache_read").and_then(|v| v.as_f64()),
                    cache_write_usd_per_mtok: cost.get("cache_write").and_then(|v| v.as_f64()),
                },
            );
        }
        if !parsed_models.is_empty() {
            providers.insert(provider_id.clone(), parsed_models);
        }
    }

    if providers.is_empty() {
        anyhow::bail!("no priced models in models.dev response");
    }
    Ok(PricingCache {
        cached_at_unix_secs: now_unix_secs(),
        providers,
    })
}

#[cfg(test)]
pub(crate) fn save_test_cache(entries: &[(&str, &str, ModelCost)]) {
    let mut providers: HashMap<String, HashMap<String, ModelCost>> = HashMap::new();
    for (provider, model, cost) in entries {
        providers
            .entry((*provider).to_string())
            .or_default()
            .insert((*model).to_string(), *cost);
    }
    save_cache(&PricingCache {
        cached_at_unix_secs: now_unix_secs(),
        providers,
    });
}

#[cfg(test)]
pub(crate) fn clear_memory_cache_for_tests() {
    if let Ok(mut memory) = MEMORY_CACHE.lock() {
        *memory = None;
    }
}
