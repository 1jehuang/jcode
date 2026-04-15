use super::{ALL_CLAUDE_MODELS, ALL_OPENAI_MODELS};
use crate::auth;
use crate::provider::cursor;

#[path = "models_catalog.rs"]
mod catalog;

use anyhow::Result;
#[cfg(test)]
pub(crate) use catalog::parse_anthropic_model_catalog;
pub use catalog::{
    AnthropicModelCatalog, OpenAIModelCatalog, fetch_anthropic_model_catalog,
    fetch_anthropic_model_catalog_oauth, fetch_openai_context_limits, fetch_openai_model_catalog,
};
use jcode_provider_core::{ModelRoute, shared_http_client};
use std::collections::{HashMap, HashSet};
use std::sync::RwLock;
use std::time::{Duration, Instant, SystemTime};

pub(crate) fn filtered_display_models(models: impl IntoIterator<Item = String>) -> Vec<String> {
    models
        .into_iter()
        .filter(|model| {
            !crate::subscription_catalog::is_runtime_mode_enabled()
                || crate::subscription_catalog::is_curated_model(model)
        })
        .collect()
}

pub(crate) fn filtered_model_routes(routes: Vec<ModelRoute>) -> Vec<ModelRoute> {
    if !crate::subscription_catalog::is_runtime_mode_enabled() {
        return routes;
    }

    routes
        .into_iter()
        .filter(|route| crate::subscription_catalog::is_curated_model(&route.model))
        .collect()
}

pub(crate) fn ensure_model_allowed_for_subscription(model: &str) -> Result<()> {
    if crate::subscription_catalog::is_runtime_mode_enabled()
        && !crate::subscription_catalog::is_curated_model(model)
    {
        anyhow::bail!(
            "Model '{}' is not included in the current jcode subscription catalog",
            model
        );
    }
    Ok(())
}

/// Default context window size when model-specific data isn't known.
pub const DEFAULT_CONTEXT_LIMIT: usize = 200_000;

/// Dynamic cache of model context window sizes, populated from API at startup.
static CONTEXT_LIMIT_CACHE: std::sync::LazyLock<RwLock<HashMap<String, usize>>> =
    std::sync::LazyLock::new(|| RwLock::new(HashMap::new()));

#[derive(Debug, Clone)]
struct RuntimeModelUnavailability {
    reason: String,
    recorded_at: Instant,
    observed_at: SystemTime,
}

#[derive(Debug, Clone)]
struct RuntimeProviderUnavailability {
    reason: String,
    recorded_at: Instant,
    observed_at: SystemTime,
}

/// Dynamic cache of models actually available for this account (populated from Codex API).
/// When populated, only models in this set should be offered/accepted for the OpenAI provider.
static ACCOUNT_AVAILABLE_MODELS: std::sync::LazyLock<RwLock<HashMap<String, HashSet<String>>>> =
    std::sync::LazyLock::new(|| RwLock::new(HashMap::new()));
static ACCOUNT_AVAILABLE_MODELS_FETCHED_AT: std::sync::LazyLock<RwLock<HashMap<String, Instant>>> =
    std::sync::LazyLock::new(|| RwLock::new(HashMap::new()));
static ACCOUNT_AVAILABLE_MODELS_OBSERVED_AT: std::sync::LazyLock<
    RwLock<HashMap<String, SystemTime>>,
> = std::sync::LazyLock::new(|| RwLock::new(HashMap::new()));
static ACCOUNT_RUNTIME_UNAVAILABLE_MODELS: std::sync::LazyLock<
    RwLock<HashMap<String, RuntimeModelUnavailability>>,
> = std::sync::LazyLock::new(|| RwLock::new(HashMap::new()));
static ACCOUNT_RUNTIME_UNAVAILABLE_PROVIDERS: std::sync::LazyLock<
    RwLock<HashMap<String, RuntimeProviderUnavailability>>,
> = std::sync::LazyLock::new(|| RwLock::new(HashMap::new()));
static ACCOUNT_MODEL_REFRESH_LAST_ATTEMPT: std::sync::LazyLock<RwLock<HashMap<String, Instant>>> =
    std::sync::LazyLock::new(|| RwLock::new(HashMap::new()));
static ACCOUNT_MODEL_REFRESH_IN_FLIGHT: std::sync::LazyLock<RwLock<HashSet<String>>> =
    std::sync::LazyLock::new(|| RwLock::new(HashSet::new()));
static ANTHROPIC_AVAILABLE_MODELS: std::sync::LazyLock<RwLock<HashMap<String, HashSet<String>>>> =
    std::sync::LazyLock::new(|| RwLock::new(HashMap::new()));
static ANTHROPIC_AVAILABLE_MODELS_FETCHED_AT: std::sync::LazyLock<
    RwLock<HashMap<String, Instant>>,
> = std::sync::LazyLock::new(|| RwLock::new(HashMap::new()));
static ANTHROPIC_AVAILABLE_MODELS_OBSERVED_AT: std::sync::LazyLock<
    RwLock<HashMap<String, SystemTime>>,
> = std::sync::LazyLock::new(|| RwLock::new(HashMap::new()));
const ACCOUNT_MODEL_CACHE_TTL: Duration = Duration::from_secs(30 * 60);
const RUNTIME_UNAVAILABLE_TTL: Duration = Duration::from_secs(10 * 60);
const PROVIDER_RUNTIME_UNAVAILABLE_TTL: Duration = Duration::from_secs(5 * 60);
const ACCOUNT_MODEL_REFRESH_RETRY_INTERVAL: Duration = Duration::from_secs(60);

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum AccountModelAvailabilityState {
    Available,
    Unavailable,
    Unknown,
}

#[derive(Debug, Clone)]
pub struct AccountModelAvailability {
    pub state: AccountModelAvailabilityState,
    pub reason: Option<String>,
    pub source: &'static str,
    pub observed_at: Option<SystemTime>,
}

fn format_elapsed_duration_short(elapsed: Duration) -> String {
    if elapsed.as_secs() < 60 {
        format!("{}s", elapsed.as_secs())
    } else if elapsed.as_secs() < 3600 {
        format!("{}m", elapsed.as_secs() / 60)
    } else if elapsed.as_secs() < 86_400 {
        format!("{}h", elapsed.as_secs() / 3600)
    } else {
        format!("{}d", elapsed.as_secs() / 86_400)
    }
}

pub fn format_account_model_availability_detail(
    availability: &AccountModelAvailability,
) -> Option<String> {
    let base = match availability.state {
        AccountModelAvailabilityState::Available => return None,
        AccountModelAvailabilityState::Unavailable | AccountModelAvailabilityState::Unknown => {
            availability
                .reason
                .clone()
                .unwrap_or_else(|| "availability unknown".to_string())
        }
    };

    let mut meta_parts = vec![availability.source.to_string()];
    if let Some(observed_at) = availability.observed_at
        && let Ok(elapsed) = SystemTime::now().duration_since(observed_at)
    {
        meta_parts.push(format!("{} ago", format_elapsed_duration_short(elapsed)));
    }

    if meta_parts.is_empty() {
        Some(base)
    } else {
        Some(format!("{} ({})", base, meta_parts.join(", ")))
    }
}

pub(crate) fn normalize_model_id(model: &str) -> String {
    let normalized = model.trim().to_ascii_lowercase();
    normalized
        .strip_suffix("[1m]")
        .unwrap_or(&normalized)
        .to_string()
}

fn normalize_provider_id(provider: &str) -> String {
    provider.trim().to_ascii_lowercase()
}

fn openai_account_scope_from_label(label: Option<String>) -> String {
    label
        .map(|label| label.trim().to_string())
        .filter(|label| !label.is_empty())
        .unwrap_or_else(|| "default".to_string())
}

fn current_openai_account_scope() -> String {
    openai_account_scope_from_label(auth::codex::active_account_label())
}

fn current_claude_account_scope() -> String {
    auth::claude::active_account_label()
        .map(|label| label.trim().to_string())
        .filter(|label| !label.is_empty())
        .unwrap_or_else(|| "default".to_string())
}

fn current_anthropic_catalog_scope() -> String {
    if std::env::var("ANTHROPIC_API_KEY")
        .ok()
        .map(|key| !key.trim().is_empty())
        .unwrap_or(false)
    {
        "api-key".to_string()
    } else {
        format!("oauth::{}", current_claude_account_scope())
    }
}

fn scoped_openai_model_key(scope: &str, model: &str) -> Option<String> {
    let key = normalize_model_id(model);
    if key.is_empty() {
        return None;
    }
    Some(format!("{}::{}", scope, key))
}

fn current_scoped_openai_model_key(model: &str) -> Option<String> {
    scoped_openai_model_key(&current_openai_account_scope(), model)
}

fn provider_runtime_scope_key(provider: &str, account_label: Option<&str>) -> String {
    let normalized = normalize_provider_id(provider);
    match normalized.as_str() {
        "openai" => format!(
            "openai::{}",
            openai_account_scope_from_label(account_label.map(|label| label.to_string()))
        ),
        "claude" | "anthropic" => format!(
            "claude::{}",
            account_label
                .map(|label| label.trim().to_string())
                .filter(|label| !label.is_empty())
                .unwrap_or_else(current_claude_account_scope)
        ),
        _ => format!("{}::global", normalized),
    }
}

fn current_provider_runtime_scope_key(provider: &str) -> String {
    let normalized = normalize_provider_id(provider);
    match normalized.as_str() {
        "openai" => provider_runtime_scope_key(provider, Some(&current_openai_account_scope())),
        "claude" | "anthropic" => {
            provider_runtime_scope_key(provider, Some(&current_claude_account_scope()))
        }
        _ => provider_runtime_scope_key(provider, None),
    }
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ModelCapabilities {
    pub provider: Option<String>,
    pub context_window: Option<usize>,
}

fn provider_key_from_hint(provider_hint: Option<&str>) -> Option<&'static str> {
    let normalized = normalize_provider_id(provider_hint?);
    match normalized.as_str() {
        "anthropic" | "claude" => Some("claude"),
        "openai" => Some("openai"),
        "openrouter" => Some("openrouter"),
        "copilot" | "github copilot" => Some("copilot"),
        "gemini" | "google gemini" => Some("gemini"),
        "cursor" => Some("cursor"),
        _ => None,
    }
}

fn model_id_for_capability_lookup(model: &str, provider: Option<&str>) -> (String, bool) {
    let normalized = model.trim().to_ascii_lowercase();
    let (base, is_1m) = if let Some(base) = normalized.strip_suffix("[1m]") {
        (base.to_string(), true)
    } else {
        (normalized, false)
    };

    let lookup = if matches!(provider, Some("openrouter")) || base.contains('/') {
        base.rsplit('/').next().unwrap_or(&base).to_string()
    } else {
        base
    };

    (lookup, is_1m)
}

fn copilot_context_limit_for_model(model: &str) -> usize {
    match model {
        "claude-sonnet-4" | "claude-sonnet-4-6" | "claude-sonnet-4.6" => 128_000,
        "claude-opus-4-6" | "claude-opus-4.6" | "claude-opus-4.6-fast" => 200_000,
        "claude-opus-4.5" | "claude-opus-4-5" => 200_000,
        "claude-sonnet-4.5" | "claude-sonnet-4-5" => 200_000,
        "claude-haiku-4.5" | "claude-haiku-4-5" => 200_000,
        "gpt-4o" | "gpt-4o-mini" => 128_000,
        m if m.starts_with("gpt-4o") => 128_000,
        m if m.starts_with("gpt-4.1") => 128_000,
        m if m.starts_with("gpt-5") => 128_000,
        "o3-mini" | "o4-mini" => 128_000,
        m if m.starts_with("gemini-2.0-flash") => 1_000_000,
        m if m.starts_with("gemini-2.5") => 1_000_000,
        m if m.starts_with("gemini-3") => 1_000_000,
        _ => 128_000,
    }
}

fn fallback_context_limit_for_model(model: &str, provider_hint: Option<&str>) -> Option<usize> {
    let provider = provider_key_from_hint(provider_hint).or_else(|| provider_for_model(model));
    let (model, is_1m) = model_id_for_capability_lookup(model, provider);
    let model = model.as_str();

    if !matches!(provider, Some("claude" | "copilot"))
        && let Some(limit) = get_cached_context_limit(model)
    {
        return Some(limit);
    }

    if matches!(provider, Some("copilot")) {
        return Some(copilot_context_limit_for_model(model));
    }

    // Spark variant has a smaller context window than the full codex model
    if model.starts_with("gpt-5.3-codex-spark") {
        return Some(128_000);
    }

    if model.starts_with("gpt-5.2-chat")
        || model.starts_with("gpt-5.1-chat")
        || model.starts_with("gpt-5-chat")
    {
        return Some(128_000);
    }

    // GPT-5.4-family models should default to the long-context window.
    // The live Codex OAuth catalog can still override this via the dynamic cache above.
    if model.starts_with("gpt-5.4") {
        return Some(1_000_000);
    }

    // Most GPT-5.x codex/reasoning models: 272k per Codex backend API
    if model.starts_with("gpt-5") {
        return Some(272_000);
    }

    if model.starts_with("claude-opus-4-6") || model.starts_with("claude-opus-4.6") {
        let eff_1m = is_1m
            || crate::provider::anthropic::effectively_1m(&format!(
                "claude-opus-4-6{}",
                if is_1m { "[1m]" } else { "" }
            ));
        return Some(if eff_1m { 1_048_576 } else { 200_000 });
    }

    if model.starts_with("claude-sonnet-4-6") || model.starts_with("claude-sonnet-4.6") {
        let eff_1m = is_1m
            || crate::provider::anthropic::effectively_1m(&format!(
                "claude-sonnet-4-6{}",
                if is_1m { "[1m]" } else { "" }
            ));
        return Some(if eff_1m { 1_048_576 } else { 200_000 });
    }

    if model.starts_with("claude-opus-4-5") || model.starts_with("claude-opus-4.5") {
        return Some(200_000);
    }

    if model.starts_with("gemini-2.0-flash")
        || model.starts_with("gemini-2.5")
        || model.starts_with("gemini-3")
    {
        return Some(1_000_000);
    }

    None
}

fn openai_static_model_ids() -> Vec<String> {
    let mut models: Vec<String> = ALL_OPENAI_MODELS.iter().map(|m| (*m).to_string()).collect();

    // Only advertise the explicit [1m] alias when the live catalog we fetched
    // says this backend exposes a >=1M context window for GPT-5.4.
    if get_cached_context_limit("gpt-5.4").unwrap_or_default() >= 1_000_000 {
        if let Some(index) = models.iter().position(|model| model == "gpt-5.4") {
            models.insert(index + 1, "gpt-5.4[1m]".to_string());
        } else {
            models.push("gpt-5.4[1m]".to_string());
        }
    }

    models
}

fn anthropic_static_model_ids() -> Vec<String> {
    ALL_CLAUDE_MODELS.iter().map(|m| (*m).to_string()).collect()
}

/// Look up a cached context limit for a model.
fn get_cached_context_limit(model: &str) -> Option<usize> {
    let cache = CONTEXT_LIMIT_CACHE.read().ok()?;
    cache.get(model).copied()
}

/// Populate the context limit cache from API-provided model data.
/// Called once at startup when OpenAI OAuth credentials are available.
pub fn populate_context_limits(models: HashMap<String, usize>) {
    if let Ok(mut cache) = CONTEXT_LIMIT_CACHE.write() {
        for (model, limit) in &models {
            crate::logging::info(&format!(
                "Context limit cache: {} = {}k",
                model,
                limit / 1000
            ));
            cache.insert(model.clone(), *limit);
        }
    }
}

/// Populate the account-available model list (called once at startup from the Codex API).
pub fn populate_account_models(slugs: Vec<String>) {
    populate_account_models_for_scope(&current_openai_account_scope(), slugs);
}

pub fn populate_anthropic_models(slugs: Vec<String>) {
    populate_anthropic_models_for_scope(&current_anthropic_catalog_scope(), slugs);
}

fn populate_account_models_for_scope(scope: &str, slugs: Vec<String>) {
    if !slugs.is_empty() {
        let mut normalized = HashSet::new();
        for slug in slugs {
            let slug = normalize_model_id(&slug);
            if !slug.is_empty() {
                normalized.insert(slug);
            }
        }
        if normalized.is_empty() {
            return;
        }

        if let Ok(mut available) = ACCOUNT_AVAILABLE_MODELS.write() {
            let mut sorted: Vec<String> = normalized.iter().cloned().collect();
            sorted.sort();
            crate::logging::info(&format!(
                "Account available models [{}]: {}",
                scope,
                sorted.join(", ")
            ));
            available.insert(scope.to_string(), normalized.clone());
        }
        if let Ok(mut fetched_at) = ACCOUNT_AVAILABLE_MODELS_FETCHED_AT.write() {
            fetched_at.insert(scope.to_string(), Instant::now());
        }
        if let Ok(mut observed_at) = ACCOUNT_AVAILABLE_MODELS_OBSERVED_AT.write() {
            observed_at.insert(scope.to_string(), SystemTime::now());
        }
        if let Ok(mut last_attempt) = ACCOUNT_MODEL_REFRESH_LAST_ATTEMPT.write() {
            last_attempt.insert(scope.to_string(), Instant::now());
        }
        if let Ok(mut unavailable) = ACCOUNT_RUNTIME_UNAVAILABLE_MODELS.write() {
            unavailable.retain(|key, _| {
                let Some((entry_scope, model)) = key.split_once("::") else {
                    return true;
                };
                entry_scope != scope || !normalized.contains(model)
            });
        }
        crate::bus::Bus::global().publish(crate::bus::BusEvent::ModelsUpdated);
    }
}

fn populate_anthropic_models_for_scope(scope: &str, slugs: Vec<String>) {
    if slugs.is_empty() {
        return;
    }

    let mut normalized = HashSet::new();
    for slug in slugs {
        let slug = normalize_model_id(&slug);
        if !slug.is_empty() {
            normalized.insert(slug);
        }
    }
    if normalized.is_empty() {
        return;
    }

    if let Ok(mut available) = ANTHROPIC_AVAILABLE_MODELS.write() {
        let mut sorted: Vec<String> = normalized.iter().cloned().collect();
        sorted.sort();
        crate::logging::info(&format!(
            "Anthropic available models [{}]: {}",
            scope,
            sorted.join(", ")
        ));
        available.insert(scope.to_string(), normalized);
    }
    if let Ok(mut fetched_at) = ANTHROPIC_AVAILABLE_MODELS_FETCHED_AT.write() {
        fetched_at.insert(scope.to_string(), Instant::now());
    }
    if let Ok(mut observed_at) = ANTHROPIC_AVAILABLE_MODELS_OBSERVED_AT.write() {
        observed_at.insert(scope.to_string(), SystemTime::now());
    }
    crate::bus::Bus::global().publish(crate::bus::BusEvent::ModelsUpdated);
}

pub(crate) fn merge_openai_model_ids(dynamic_models: Vec<String>) -> Vec<String> {
    let mut models = openai_static_model_ids();
    let mut seen: HashSet<String> = models
        .iter()
        .map(|model| normalize_model_id(model))
        .collect();
    let mut extras = Vec::new();

    for model in dynamic_models {
        let trimmed = model.trim();
        if trimmed.is_empty() {
            continue;
        }

        let normalized = normalize_model_id(trimmed);
        if normalized.is_empty() || !seen.insert(normalized) {
            continue;
        }

        extras.push(trimmed.to_string());
    }

    extras.sort();
    models.extend(extras);
    models
}

pub(crate) fn merge_anthropic_model_ids(dynamic_models: Vec<String>) -> Vec<String> {
    let mut models = anthropic_static_model_ids();
    let mut seen: HashSet<String> = models
        .iter()
        .map(|model| normalize_model_id(model))
        .collect();
    let mut extras = Vec::new();

    for model in dynamic_models {
        let trimmed = model.trim();
        if trimmed.is_empty() {
            continue;
        }

        let normalized = normalize_model_id(trimmed);
        if normalized.is_empty() || !seen.insert(normalized) {
            continue;
        }

        extras.push(trimmed.to_string());
    }

    extras.sort();
    models.extend(extras);
    models
}

pub fn known_anthropic_model_ids() -> Vec<String> {
    let scope = current_anthropic_catalog_scope();
    let dynamic_models = ANTHROPIC_AVAILABLE_MODELS
        .read()
        .ok()
        .and_then(|cache| {
            cache
                .get(&scope)
                .map(|models| models.iter().cloned().collect())
        })
        .unwrap_or_default();
    merge_anthropic_model_ids(dynamic_models)
}

pub fn known_openai_model_ids() -> Vec<String> {
    let scope = current_openai_account_scope();
    let dynamic_models = ACCOUNT_AVAILABLE_MODELS
        .read()
        .ok()
        .and_then(|cache| {
            cache
                .get(&scope)
                .map(|models| models.iter().cloned().collect())
        })
        .unwrap_or_default();
    merge_openai_model_ids(dynamic_models)
}

pub fn note_openai_model_catalog_refresh_attempt() {
    if let Ok(mut last_attempt) = ACCOUNT_MODEL_REFRESH_LAST_ATTEMPT.write() {
        last_attempt.insert(current_openai_account_scope(), Instant::now());
    }
}

fn note_openai_model_catalog_refresh_attempt_for_scope(scope: &str) {
    if let Ok(mut last_attempt) = ACCOUNT_MODEL_REFRESH_LAST_ATTEMPT.write() {
        last_attempt.insert(scope.to_string(), Instant::now());
    }
}

fn openai_model_catalog_refresh_throttled() -> bool {
    let scope = current_openai_account_scope();
    let Ok(last_attempt) = ACCOUNT_MODEL_REFRESH_LAST_ATTEMPT.read() else {
        return false;
    };

    last_attempt
        .get(&scope)
        .map(|at| at.elapsed() < ACCOUNT_MODEL_REFRESH_RETRY_INTERVAL)
        .unwrap_or(false)
}

pub fn should_refresh_openai_model_catalog() -> bool {
    if account_model_cache_is_fresh() {
        return false;
    }
    if openai_model_catalog_refresh_throttled() {
        return false;
    }
    ACCOUNT_MODEL_REFRESH_IN_FLIGHT
        .read()
        .map(|in_flight| !in_flight.contains(&current_openai_account_scope()))
        .unwrap_or(true)
}

pub fn begin_openai_model_catalog_refresh() -> bool {
    if !should_refresh_openai_model_catalog() {
        return false;
    }
    let scope = current_openai_account_scope();
    let Ok(mut in_flight) = ACCOUNT_MODEL_REFRESH_IN_FLIGHT.write() else {
        return false;
    };
    if !in_flight.insert(scope.clone()) {
        return false;
    }

    if let Ok(mut last_attempt) = ACCOUNT_MODEL_REFRESH_LAST_ATTEMPT.write() {
        last_attempt.insert(scope, Instant::now());
    }
    true
}

pub fn finish_openai_model_catalog_refresh() {
    if let Ok(mut in_flight) = ACCOUNT_MODEL_REFRESH_IN_FLIGHT.write() {
        in_flight.remove(&current_openai_account_scope());
    }
}

fn finish_openai_model_catalog_refresh_for_scope(scope: &str) {
    if let Ok(mut in_flight) = ACCOUNT_MODEL_REFRESH_IN_FLIGHT.write() {
        in_flight.remove(scope);
    }
}

fn account_model_cache_is_fresh() -> bool {
    let scope = current_openai_account_scope();
    let Ok(guard) = ACCOUNT_AVAILABLE_MODELS_FETCHED_AT.read() else {
        return false;
    };
    guard
        .get(&scope)
        .map(|fetched_at| fetched_at.elapsed() <= ACCOUNT_MODEL_CACHE_TTL)
        .unwrap_or(false)
}

fn runtime_model_unavailability(model: &str) -> Option<RuntimeModelUnavailability> {
    let key = current_scoped_openai_model_key(model)?;

    let mut unavailable = ACCOUNT_RUNTIME_UNAVAILABLE_MODELS.write().ok()?;
    if let Some(entry) = unavailable.get(&key) {
        if entry.recorded_at.elapsed() <= RUNTIME_UNAVAILABLE_TTL {
            return Some(entry.clone());
        }
        unavailable.remove(&key);
    }
    None
}

fn account_snapshot_model_available(model: &str) -> Option<bool> {
    if !account_model_cache_is_fresh() {
        return None;
    }
    let key = normalize_model_id(model);
    if key.is_empty() {
        return None;
    }

    let scope = current_openai_account_scope();
    let cache = ACCOUNT_AVAILABLE_MODELS.read().ok()?;
    let models = cache.get(&scope)?;
    Some(models.contains(&key))
}

fn account_models_observed_at() -> Option<SystemTime> {
    let scope = current_openai_account_scope();
    ACCOUNT_AVAILABLE_MODELS_OBSERVED_AT
        .read()
        .ok()
        .and_then(|map| map.get(&scope).copied())
}

pub fn refresh_openai_model_catalog_in_background(access_token: String, context: &'static str) {
    let scope = current_openai_account_scope();
    if access_token.trim().is_empty() {
        finish_openai_model_catalog_refresh_for_scope(&scope);
        return;
    }

    tokio::spawn(async move {
        let refresh_result = fetch_openai_model_catalog(&access_token).await;
        match refresh_result {
            Ok(catalog)
                if !catalog.available_models.is_empty() || !catalog.context_limits.is_empty() =>
            {
                crate::logging::info(&format!(
                    "Refreshed OpenAI model catalog ({}): {} available, {} with context limits",
                    context,
                    catalog.available_models.len(),
                    catalog.context_limits.len()
                ));
                if !catalog.context_limits.is_empty() {
                    populate_context_limits(catalog.context_limits.clone());
                }
                if !catalog.available_models.is_empty() {
                    populate_account_models_for_scope(&scope, catalog.available_models.clone());
                }
            }
            Ok(_) => {
                crate::logging::info(&format!(
                    "Codex models API refresh returned no model catalog data ({})",
                    context
                ));
            }
            Err(e) => {
                crate::logging::info(&format!(
                    "Failed to refresh OpenAI model catalog from Codex API ({}): {}",
                    context, e
                ));
            }
        }
        note_openai_model_catalog_refresh_attempt_for_scope(&scope);
        finish_openai_model_catalog_refresh_for_scope(&scope);
    });
}

pub fn record_model_unavailable_for_account(model: &str, reason: &str) {
    let Some(key) = current_scoped_openai_model_key(model) else {
        return;
    };

    if let Ok(mut unavailable) = ACCOUNT_RUNTIME_UNAVAILABLE_MODELS.write() {
        unavailable.insert(
            key,
            RuntimeModelUnavailability {
                reason: reason.trim().to_string(),
                recorded_at: Instant::now(),
                observed_at: SystemTime::now(),
            },
        );
    }
}

pub fn clear_model_unavailable_for_account(model: &str) {
    let Some(key) = current_scoped_openai_model_key(model) else {
        return;
    };

    if let Ok(mut unavailable) = ACCOUNT_RUNTIME_UNAVAILABLE_MODELS.write() {
        unavailable.remove(&key);
    }
}

fn runtime_provider_unavailability(provider: &str) -> Option<RuntimeProviderUnavailability> {
    let key = current_provider_runtime_scope_key(provider);

    let mut unavailable = ACCOUNT_RUNTIME_UNAVAILABLE_PROVIDERS.write().ok()?;
    if let Some(entry) = unavailable.get(&key) {
        if entry.recorded_at.elapsed() <= PROVIDER_RUNTIME_UNAVAILABLE_TTL {
            return Some(entry.clone());
        }
        unavailable.remove(&key);
    }
    None
}

pub fn record_provider_unavailable_for_account(provider: &str, reason: &str) {
    let key = current_provider_runtime_scope_key(provider);
    if key.trim().is_empty() {
        return;
    }

    if let Ok(mut unavailable) = ACCOUNT_RUNTIME_UNAVAILABLE_PROVIDERS.write() {
        unavailable.insert(
            key,
            RuntimeProviderUnavailability {
                reason: reason.trim().to_string(),
                recorded_at: Instant::now(),
                observed_at: SystemTime::now(),
            },
        );
    }
}

pub fn clear_provider_unavailable_for_account(provider: &str) {
    let key = current_provider_runtime_scope_key(provider);
    if key.trim().is_empty() {
        return;
    }

    if let Ok(mut unavailable) = ACCOUNT_RUNTIME_UNAVAILABLE_PROVIDERS.write() {
        unavailable.remove(&key);
    }
}

/// Clear all runtime model unavailability markers.
pub fn clear_all_model_unavailability_for_account() {
    let scope = current_openai_account_scope();
    if let Ok(mut unavailable) = ACCOUNT_RUNTIME_UNAVAILABLE_MODELS.write() {
        unavailable.retain(|key, _| !key.starts_with(&format!("{}::", scope)));
    }
}

/// Clear all runtime provider unavailability markers.
pub fn clear_all_provider_unavailability_for_account() {
    let scope = current_openai_account_scope();
    if let Ok(mut unavailable) = ACCOUNT_RUNTIME_UNAVAILABLE_PROVIDERS.write() {
        unavailable.retain(|key, _| !key.starts_with(&format!("openai::{}", scope)));
    }
}

pub fn provider_unavailability_detail_for_account(provider: &str) -> Option<String> {
    let entry = runtime_provider_unavailability(provider)?;
    let mut detail = entry.reason;
    if let Ok(elapsed) = SystemTime::now().duration_since(entry.observed_at) {
        detail.push_str(&format!(
            " (runtime-error, {} ago)",
            format_elapsed_duration_short(elapsed)
        ));
    }

    Some(detail)
}

pub fn model_unavailability_detail_for_account(model: &str) -> Option<String> {
    let availability = model_availability_for_account(model);
    format_account_model_availability_detail(&availability)
}

/// Check if a model is available for the current account.
/// Returns None when availability is currently unknown (e.g. stale/missing snapshot).
/// Returns Some(true) when available and Some(false) when unavailable.
pub fn is_model_available_for_account(model: &str) -> Option<bool> {
    match model_availability_for_account(model).state {
        AccountModelAvailabilityState::Available => Some(true),
        AccountModelAvailabilityState::Unavailable => Some(false),
        AccountModelAvailabilityState::Unknown => None,
    }
}

pub fn model_availability_for_account(model: &str) -> AccountModelAvailability {
    if let Some(runtime) = runtime_model_unavailability(model) {
        return AccountModelAvailability {
            state: AccountModelAvailabilityState::Unavailable,
            reason: Some(runtime.reason),
            source: "runtime-error",
            observed_at: Some(runtime.observed_at),
        };
    }

    if !account_model_cache_is_fresh() {
        return AccountModelAvailability {
            state: AccountModelAvailabilityState::Unknown,
            reason: Some("availability snapshot is stale".to_string()),
            source: "account-snapshot",
            observed_at: account_models_observed_at(),
        };
    }

    match account_snapshot_model_available(model) {
        Some(true) => AccountModelAvailability {
            state: AccountModelAvailabilityState::Available,
            reason: None,
            source: "account-snapshot",
            observed_at: account_models_observed_at(),
        },
        Some(false) => AccountModelAvailability {
            state: AccountModelAvailabilityState::Unavailable,
            reason: Some("not available for your account".to_string()),
            source: "account-snapshot",
            observed_at: account_models_observed_at(),
        },
        None => AccountModelAvailability {
            state: AccountModelAvailabilityState::Unknown,
            reason: Some("no availability snapshot yet".to_string()),
            source: "account-snapshot",
            observed_at: account_models_observed_at(),
        },
    }
}

/// Preferred model order for fallback selection.
/// If the desired model isn't available, we try these in order.
const OPENAI_MODEL_PREFERENCE: &[&str] = &[
    "gpt-5.4",
    "gpt-5.3-codex-spark",
    "gpt-5.3-codex",
    "gpt-5.2-codex",
    "gpt-5.1-codex-max",
    "gpt-5.1-codex",
];

/// Get the best available OpenAI model, falling back through the preference list.
/// Returns None if the dynamic model list hasn't been fetched yet.
pub fn get_best_available_openai_model() -> Option<String> {
    if !account_model_cache_is_fresh() {
        return None;
    }
    let scope = current_openai_account_scope();
    let cache = ACCOUNT_AVAILABLE_MODELS.read().ok()?;
    let models = cache.get(&scope)?;

    for preferred in OPENAI_MODEL_PREFERENCE {
        if models.contains(*preferred) && runtime_model_unavailability(preferred).is_none() {
            return Some(preferred.to_string());
        }
    }

    let mut sorted_models: Vec<String> = models.iter().cloned().collect();
    sorted_models.sort();
    sorted_models
        .into_iter()
        .find(|model| runtime_model_unavailability(model).is_none())
}

/// Return the context window size in tokens for a given model, if known.
///
/// First checks the dynamic cache (populated from the Codex backend API at startup),
/// then falls back to hardcoded defaults.
pub fn context_limit_for_model(model: &str) -> Option<usize> {
    context_limit_for_model_with_provider(model, None)
}

pub fn context_limit_for_model_with_provider(
    model: &str,
    provider_hint: Option<&str>,
) -> Option<usize> {
    fallback_context_limit_for_model(model, provider_hint)
}

pub fn resolve_model_capabilities(model: &str, provider_hint: Option<&str>) -> ModelCapabilities {
    let provider = provider_for_model_with_hint(model, provider_hint).map(str::to_string);
    let context_window = context_limit_for_model_with_provider(model, provider_hint);
    ModelCapabilities {
        provider,
        context_window,
    }
}

/// Normalize a Copilot-style model name to the canonical form used by our
/// provider model lists. Copilot uses dots in version numbers (e.g.
/// `claude-opus-4.6`) while our canonical lists use hyphens (`claude-opus-4-6`).
/// Returns None if no normalization is needed (model already canonical or unknown).
pub(crate) fn normalize_copilot_model_name(model: &str) -> Option<&'static str> {
    for canonical in ALL_CLAUDE_MODELS.iter().chain(ALL_OPENAI_MODELS.iter()) {
        if *canonical == model {
            return None;
        }
    }
    let normalized = model.replace('.', "-");
    ALL_CLAUDE_MODELS
        .iter()
        .chain(ALL_OPENAI_MODELS.iter())
        .find(|canonical| **canonical == normalized)
        .copied()
}

/// Detect which provider a model belongs to
pub fn provider_for_model_with_hint(
    model: &str,
    provider_hint: Option<&str>,
) -> Option<&'static str> {
    if let Some(provider) = provider_key_from_hint(provider_hint) {
        return Some(provider);
    }

    let model = model.trim();
    if model.contains('@') {
        Some("openrouter")
    } else if ALL_CLAUDE_MODELS.contains(&model) {
        Some("claude")
    } else if ALL_OPENAI_MODELS.contains(&model) {
        Some("openai")
    } else if model.contains('/') {
        Some("openrouter")
    } else if model.starts_with("claude-") {
        Some("claude")
    } else if model.starts_with("gpt-") {
        Some("openai")
    } else if model.starts_with("gemini-") {
        Some("gemini")
    } else if cursor::is_known_model(model) {
        Some("cursor")
    } else {
        None
    }
}

/// Detect which provider a model belongs to
pub fn provider_for_model(model: &str) -> Option<&'static str> {
    provider_for_model_with_hint(model, None)
}
