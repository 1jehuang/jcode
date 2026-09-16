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

use super::entry::ModelPricingEntry;
use crate::config::{ContextTier, CostFields, Tariff};

pub(super) const API_URL: &str = "https://models.dev/api.json";
pub(super) const CACHE_FILE: &str = "models_dev_pricing.json";
pub(super) const CACHE_TTL_SECS: u64 = 24 * 60 * 60;
pub(super) const HTTP_TIMEOUT: Duration = Duration::from_secs(20);

/// Current on-disk cache shape. Version 1 (every binary before this change)
/// had no `schema_version` field and stored bare `ModelCost` objects; version 2
/// stores [ModelPricingEntry] objects that can carry tariffs/schedule/validity.
pub(super) const SCHEMA_VERSION: u32 = 2;

/// A cache file without a `schema_version` was written by a v1 binary.
fn v1_schema_version() -> u32 {
    1
}

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

#[derive(Debug, Clone, Serialize, Deserialize)]
pub(super) struct PricingCache {
    /// Absent in files written before the entry model existed; those are v1.
    #[serde(default = "v1_schema_version")]
    pub(super) schema_version: u32,
    pub(super) cached_at_unix_secs: u64,
    /// provider id -> model id -> entry. Provider ids are models.dev ids
    /// (e.g. `anthropic`, `openai`, `deepseek`, `moonshotai`).
    pub(super) providers: HashMap<String, HashMap<String, ModelPricingEntry>>,
}

impl Default for PricingCache {
    fn default() -> Self {
        Self {
            schema_version: SCHEMA_VERSION,
            cached_at_unix_secs: 0,
            providers: HashMap::new(),
        }
    }
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

    let mut providers: HashMap<String, HashMap<String, ModelPricingEntry>> = HashMap::new();
    for (provider_id, provider) in top {
        let Some(models) = provider.get("models").and_then(|m| m.as_object()) else {
            continue;
        };
        let mut parsed_models = HashMap::new();
        for (model_id, model) in models {
            // models.dev nests rates under a `cost` object; the entry keeps that
            // wrapped shape so tariffs/schedule from custom sources can be
            // attached without another cache migration.
            let Some(cost) = model.get("cost") else {
                continue;
            };
            let (Some(input), Some(output)) = (
                cost.get("input").and_then(|v| v.as_f64()),
                cost.get("output").and_then(|v| v.as_f64()),
            ) else {
                continue;
            };
            let mut entry = ModelPricingEntry::from_model_cost(ModelCost {
                input_usd_per_mtok: input,
                output_usd_per_mtok: output,
                cache_read_usd_per_mtok: cost.get("cache_read").and_then(|v| v.as_f64()),
                cache_write_usd_per_mtok: cost.get("cache_write").and_then(|v| v.as_f64()),
            });
            if let Some(tier) = context_over_200k_tier(cost) {
                entry.context_tiers.push(tier);
            }
            parsed_models.insert(model_id.clone(), entry);
        }
        if !parsed_models.is_empty() {
            providers.insert(provider_id.clone(), parsed_models);
        }
    }

    if providers.is_empty() {
        anyhow::bail!("no priced models in models.dev response");
    }
    Ok(PricingCache {
        schema_version: SCHEMA_VERSION,
        cached_at_unix_secs: now_unix_secs(),
        providers,
    })
}

/// models.dev's native long-context rates, as a tier above 200k input tokens.
///
/// models.dev states these as `cost.context_over_200k = { input, output }` (the
/// odd name is upstream's). They used to be dropped on the floor, which billed
/// a >200k-context call at the base rate. Mapping them into the same
/// [`ContextTier`] shape a hand-written `[[...context_tiers]]` rule uses is what
/// makes the two sources converge internally.
fn context_over_200k_tier(cost: &serde_json::Value) -> Option<ContextTier> {
    const UPSTREAM_THRESHOLD: u64 = 200_000;
    let over = cost.get("context_over_200k")?.as_object()?;
    let fields = CostFields {
        input: over.get("input").and_then(|v| v.as_f64()),
        output: over.get("output").and_then(|v| v.as_f64()),
        cache_read: over.get("cache_read").and_then(|v| v.as_f64()),
        cache_write: over.get("cache_write").and_then(|v| v.as_f64()),
    };
    // An all-empty object states nothing; do not invent a tier for it.
    if fields.input.is_none()
        && fields.output.is_none()
        && fields.cache_read.is_none()
        && fields.cache_write.is_none()
    {
        return None;
    }
    Some(ContextTier {
        min_input_tokens: UPSTREAM_THRESHOLD,
        tariff: Tariff::Absolute(fields),
    })
}

#[cfg(test)]
pub(crate) fn save_test_cache(entries: &[(&str, &str, ModelCost)]) {
    let mut providers: HashMap<String, HashMap<String, ModelPricingEntry>> = HashMap::new();
    for (provider, model, cost) in entries {
        providers
            .entry((*provider).to_string())
            .or_default()
            .insert(
                (*model).to_string(),
                ModelPricingEntry::from_model_cost(*cost),
            );
    }
    save_cache(&PricingCache {
        schema_version: SCHEMA_VERSION,
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
