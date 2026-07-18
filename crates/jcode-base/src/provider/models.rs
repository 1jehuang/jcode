use crate::auth;
use crate::provider::cursor;

#[path = "models_catalog.rs"]
mod catalog;
#[path = "model_catalog_service.rs"]
mod catalog_service;

use anyhow::Result;
#[cfg(test)]
pub(crate) use catalog::parse_anthropic_model_catalog;
pub use catalog::{
    AnthropicModelCatalog, ModelCatalogHttpStatus, OpenAIModelCatalog,
    fetch_anthropic_model_catalog, fetch_anthropic_model_catalog_oauth,
    fetch_openai_api_key_model_catalog, fetch_openai_context_limits, fetch_openai_model_catalog,
};
use catalog_service::{ModelCatalogService, RuntimeModelUnavailability};
use jcode_provider_core::{
    ALL_CLAUDE_MODELS, ALL_OPENAI_MODELS, CHATGPT_WEB_MODEL, ModelCapabilities, ModelRoute,
    OPENAI_API_ONLY_PRO_MODELS, context_limit_for_model_with_provider_and_cache,
    core_provider_for_model_with_hint, is_openai_api_only_pro_model, provider_key_from_hint,
    shared_http_client,
};
use serde::{Deserialize, Serialize};
use std::collections::{HashMap, HashSet};
use std::path::PathBuf;
use std::sync::RwLock;
use std::time::{Duration, Instant, SystemTime, UNIX_EPOCH};

const OPENAI_MODEL_CATALOG_CACHE_FILE: &str = "openai_model_catalog_cache.json";
const ANTHROPIC_MODEL_CATALOG_CACHE_FILE: &str = "anthropic_model_catalog_cache.json";

#[derive(Debug, Clone, Default, Serialize, Deserialize)]
struct PersistedModelCatalogStore {
    scopes: HashMap<String, PersistedModelCatalogScope>,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
struct PersistedModelCatalogScope {
    models: Vec<String>,
    #[serde(default)]
    context_limits: HashMap<String, usize>,
    #[serde(default)]
    reasoning_efforts: HashMap<String, Vec<String>>,
    observed_at_unix_secs: u64,
}

pub(crate) fn filtered_display_models(models: impl IntoIterator<Item = String>) -> Vec<String> {
    models
        .into_iter()
        .filter(|model| {
            !crate::subscription_catalog::is_runtime_mode_enabled()
                || crate::subscription_catalog::is_model_allowed_for_current_tier(model)
        })
        .collect()
}

pub(crate) fn filtered_model_routes(routes: Vec<ModelRoute>) -> Vec<ModelRoute> {
    if !crate::subscription_catalog::is_runtime_mode_enabled() {
        return routes;
    }

    routes
        .into_iter()
        .filter(|route| {
            crate::subscription_catalog::is_model_allowed_for_current_tier(&route.model)
        })
        .collect()
}

pub(crate) fn ensure_model_allowed_for_subscription(model: &str) -> Result<()> {
    if !crate::subscription_catalog::is_runtime_mode_enabled() {
        return Ok(());
    }
    match crate::subscription_catalog::find_curated_model(model) {
        None => {
            anyhow::bail!(
                "Model '{}' is not included in the current jcode subscription catalog",
                model
            );
        }
        Some(curated) => {
            let tier = crate::subscription_catalog::effective_tier();
            if !tier.allows(curated.min_tier) {
                anyhow::bail!(
                    "Model '{}' requires the {} tier (current tier: {}). Upgrade your jcode subscription to use it.",
                    curated.display_name,
                    curated.min_tier.display_name(),
                    tier.display_name()
                );
            }
        }
    }
    Ok(())
}

/// Dynamic cache of model context window sizes, populated from API at startup.
static CONTEXT_LIMIT_CACHE: std::sync::LazyLock<RwLock<HashMap<String, usize>>> =
    std::sync::LazyLock::new(|| RwLock::new(HashMap::new()));

#[derive(Debug, Clone)]
struct RuntimeProviderUnavailability {
    reason: String,
    recorded_at: Instant,
    observed_at: SystemTime,
}

/// Dynamic cache of models actually available for this account (populated from provider APIs).
/// When populated, only models in this set should be offered/accepted for that account/provider.
static OPENAI_MODEL_CATALOG_SERVICE: std::sync::LazyLock<ModelCatalogService> =
    std::sync::LazyLock::new(|| {
        ModelCatalogService::new(
            ACCOUNT_MODEL_CACHE_TTL,
            ACCOUNT_MODEL_REFRESH_RETRY_INTERVAL,
            RUNTIME_UNAVAILABLE_TTL,
        )
    });
static ANTHROPIC_MODEL_CATALOG_SERVICE: std::sync::LazyLock<ModelCatalogService> =
    std::sync::LazyLock::new(|| {
        ModelCatalogService::new(
            ACCOUNT_MODEL_CACHE_TTL,
            ACCOUNT_MODEL_REFRESH_RETRY_INTERVAL,
            RUNTIME_UNAVAILABLE_TTL,
        )
    });
static ACCOUNT_RUNTIME_UNAVAILABLE_PROVIDERS: std::sync::LazyLock<
    RwLock<HashMap<String, RuntimeProviderUnavailability>>,
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
    jcode_provider_core::model_id::canonical(model)
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
    // Match the credential-resolution order used by the Anthropic provider:
    // the API key can come from the process env *or* the persisted
    // anthropic.env file. Checking only the env var made env-file-keyed
    // sessions read/write the OAuth scope while requests actually used the
    // API key, so the `api-key` catalog scope went permanently stale.
    if crate::provider_catalog::load_api_key_from_env_or_config(
        "ANTHROPIC_API_KEY",
        "anthropic.env",
    )
    .is_some()
    {
        "api-key".to_string()
    } else {
        format!("oauth::{}", current_claude_account_scope())
    }
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

fn model_ids_with_context_aliases(models: Vec<String>) -> Vec<String> {
    let mut deduped = Vec::new();
    let mut seen = HashSet::new();

    for model in models {
        let normalized = normalize_model_id(&model);
        if normalized.is_empty() {
            continue;
        }
        if seen.insert(model.clone()) {
            deduped.push(model.clone());
        }
        if model_exposes_1m_alias(&normalized) {
            let alias = format!("{}[1m]", normalized);
            if seen.insert(alias.clone()) {
                deduped.push(alias);
            }
        }
    }

    deduped
}

/// Whether a `<model>[1m]` long-context picker alias should be surfaced.
///
/// For *known* Claude models this is authoritative: only opt-in 1M models (Opus
/// 4.6, Sonnet 4.6) get an alias. Native-1M models (Opus 4.8, 4.7) already use
/// 1M by default, so a `[1m]` alias would be a redundant duplicate, and
/// 200K-only models (Sonnet 4.5, which the live catalog wrongly advertises as
/// 1M) get no alias. Unknown/future Claude ids and all non-Claude models keep
/// the prior behavior: alias when the cached catalog limit is >= 1M.
fn model_exposes_1m_alias(normalized_model: &str) -> bool {
    if normalized_model.starts_with("claude-") {
        let mode = jcode_provider_core::anthropic_context_mode(normalized_model);
        // Only trust the classifier for models it actually recognizes; for
        // anything it maps to `Standard` we can't tell a genuine 200K model from
        // an unrecognized future one, so fall back to the catalog heuristic.
        if mode != jcode_provider_core::AnthropicContextMode::Standard {
            return mode.exposes_1m_alias();
        }
    }
    get_cached_context_limit(normalized_model).unwrap_or_default() >= 1_000_000
}

fn live_catalog_model_ids(service: &ModelCatalogService, scope: &str) -> Option<Vec<String>> {
    service.model_ids(scope).map(model_ids_with_context_aliases)
}

fn load_openai_catalog_from_disk(scope: &str) -> Option<Vec<String>> {
    hydrate_catalog_cache_from_disk(
        OPENAI_MODEL_CATALOG_CACHE_FILE,
        scope,
        &OPENAI_MODEL_CATALOG_SERVICE,
    )
}

fn load_anthropic_catalog_from_disk(scope: &str) -> Option<Vec<String>> {
    hydrate_catalog_cache_from_disk(
        ANTHROPIC_MODEL_CATALOG_CACHE_FILE,
        scope,
        &ANTHROPIC_MODEL_CATALOG_SERVICE,
    )
}

fn observed_at_unix_secs(observed_at: SystemTime) -> u64 {
    observed_at
        .duration_since(UNIX_EPOCH)
        .map(|duration| duration.as_secs())
        .unwrap_or(0)
}

fn system_time_from_unix_secs(secs: u64) -> SystemTime {
    UNIX_EPOCH + Duration::from_secs(secs)
}

fn model_catalog_cache_path(file_name: &str) -> Result<PathBuf> {
    Ok(crate::storage::app_config_dir()?.join(file_name))
}

fn load_persisted_model_catalog_store(file_name: &str) -> Option<PersistedModelCatalogStore> {
    let path = model_catalog_cache_path(file_name).ok()?;
    crate::storage::read_json(&path).ok()
}

fn save_persisted_model_catalog_store(file_name: &str, store: &PersistedModelCatalogStore) {
    let Ok(path) = model_catalog_cache_path(file_name) else {
        return;
    };
    if let Err(err) = crate::storage::write_json(&path, store) {
        crate::logging::warn(&format!(
            "Failed to persist model catalog cache {}: {}",
            path.display(),
            err
        ));
    }
}

fn persist_scoped_model_catalog(
    file_name: &str,
    scope: &str,
    models: &[String],
    context_limits: &HashMap<String, usize>,
    reasoning_efforts: &HashMap<String, Vec<String>>,
    observed_at: SystemTime,
) {
    if models.is_empty() {
        return;
    }

    let mut store = load_persisted_model_catalog_store(file_name).unwrap_or_default();
    store.scopes.insert(
        scope.to_string(),
        PersistedModelCatalogScope {
            models: models.to_vec(),
            context_limits: context_limits.clone(),
            reasoning_efforts: reasoning_efforts.clone(),
            observed_at_unix_secs: observed_at_unix_secs(observed_at),
        },
    );
    save_persisted_model_catalog_store(file_name, &store);
}

fn hydrate_catalog_cache_from_disk(
    file_name: &str,
    scope: &str,
    service: &ModelCatalogService,
) -> Option<Vec<String>> {
    let store = load_persisted_model_catalog_store(file_name)?;
    let persisted = store.scopes.get(scope)?.clone();
    if persisted.models.is_empty() {
        return None;
    }

    let mut normalized = HashSet::new();
    for model in &persisted.models {
        let normalized_model = normalize_model_id(model);
        if !normalized_model.is_empty() {
            normalized.insert(normalized_model);
        }
    }
    if normalized.is_empty() {
        return None;
    }

    let observed_at = system_time_from_unix_secs(persisted.observed_at_unix_secs);
    service.hydrate_scope_models_from_snapshot(scope, normalized, observed_at);
    if !persisted.context_limits.is_empty() {
        populate_context_limits(persisted.context_limits.clone());
    }

    Some(model_ids_with_context_aliases(persisted.models))
}

pub fn cached_anthropic_model_ids() -> Option<Vec<String>> {
    let scope = current_anthropic_catalog_scope();
    live_catalog_model_ids(&ANTHROPIC_MODEL_CATALOG_SERVICE, &scope)
        .or_else(|| load_anthropic_catalog_from_disk(&scope))
}

pub fn cached_openai_model_ids() -> Option<Vec<String>> {
    let scope = current_openai_account_scope();
    live_catalog_model_ids(&OPENAI_MODEL_CATALOG_SERVICE, &scope)
        .or_else(|| load_openai_catalog_from_disk(&scope))
}

/// Return model-specific OpenAI reasoning capabilities for the active account
/// from its scoped disk snapshot. Live refreshes update provider-local state
/// directly; this keeps startup exact before the first refresh completes.
pub fn cached_openai_reasoning_efforts() -> Option<HashMap<String, Vec<String>>> {
    let scope = current_openai_account_scope();
    let store = load_persisted_model_catalog_store(OPENAI_MODEL_CATALOG_CACHE_FILE)?;
    let efforts = store.scopes.get(&scope)?.reasoning_efforts.clone();
    (!efforts.is_empty()).then_some(efforts)
}

/// Test-only: clear the process-global in-memory model catalogs. The catalog
/// services are statics shared by every test in the process; a test that
/// hydrates a scope (directly or via `persist_*` + `cached_*`) otherwise leaks
/// fixture models into later tests' `known_*_model_ids()` validation.
#[cfg(any(test, feature = "test-support"))]
pub fn reset_model_catalog_services_for_tests() {
    OPENAI_MODEL_CATALOG_SERVICE.reset_for_tests();
    ANTHROPIC_MODEL_CATALOG_SERVICE.reset_for_tests();
}

pub fn persist_openai_model_catalog(catalog: &OpenAIModelCatalog) {
    persist_scoped_model_catalog(
        OPENAI_MODEL_CATALOG_CACHE_FILE,
        &current_openai_account_scope(),
        &catalog.available_models,
        &catalog.context_limits,
        &catalog.reasoning_efforts,
        SystemTime::now(),
    );
}

pub fn persist_anthropic_model_catalog(catalog: &AnthropicModelCatalog) {
    persist_scoped_model_catalog(
        ANTHROPIC_MODEL_CATALOG_CACHE_FILE,
        &current_anthropic_catalog_scope(),
        &catalog.available_models,
        &catalog.context_limits,
        &HashMap::new(),
        SystemTime::now(),
    );
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

/// Populate the context limit cache from named provider model configs in the
/// user's config file.
///
/// Custom OpenAI-compatible providers that lack a usable `/v1/models` endpoint
/// rely on per-model `context_window` config. That value is honored by the
/// provider instance's own `context_window()` method, but every other
/// resolution path (TUI info widget, compaction budget, model switching) goes
/// through the global [`CONTEXT_LIMIT_CACHE`] via
/// [`context_limit_for_model_with_provider`]. Seed that cache here so the
/// configured limit is respected globally instead of falling back to
/// [`DEFAULT_CONTEXT_LIMIT`].
pub fn populate_context_limits_from_config() {
    populate_context_limits_from_config_value(crate::config::config());
}

/// Seed the global context-limit cache from an explicit config reference.
///
/// Runtime model specs reach the lookup in several shapes, so each configured
/// model is seeded under every key the lookup can normalize to (issue #421):
/// - the bare lowercased id (`qwen3.6-35b-a2000-128k`);
/// - the slash base (`x.gguf` for `/opt/models/x.gguf`), because
///   `model_id_for_capability_lookup` reduces slash-containing ids to their
///   final segment;
/// - the profile-qualified spec (`cachyai-a2000:qwen3.6-35b-a2000-128k`),
///   because session-restored models keep the `<profile>:` routing prefix and
///   non-slash qualified specs are looked up verbatim.
pub fn populate_context_limits_from_config_value(cfg: &crate::config::Config) {
    let mut limits = HashMap::new();
    for (profile_id, provider_cfg) in cfg.providers.iter() {
        for model in &provider_cfg.models {
            let Some(limit) = model.context_window else {
                continue;
            };
            for key in config_context_limit_cache_keys(profile_id, &model.id) {
                limits.insert(key, limit);
            }
        }
    }
    if !limits.is_empty() {
        populate_context_limits(limits);
    }
}

/// Cache keys under which a configured per-model `context_window` must be
/// discoverable so every runtime lookup shape resolves to it. See
/// [`populate_context_limits_from_config_value`].
pub(crate) fn config_context_limit_cache_keys(profile_id: &str, model_id: &str) -> Vec<String> {
    let id = model_id.trim().to_ascii_lowercase();
    if id.is_empty() {
        return Vec::new();
    }
    let mut keys = vec![id.clone()];
    let slash_base = jcode_provider_core::model_id::slash_base(&id).to_string();
    if slash_base != id && !slash_base.is_empty() {
        keys.push(slash_base);
    }
    let profile = profile_id.trim().to_ascii_lowercase();
    if !profile.is_empty() {
        keys.push(format!("{profile}:{id}"));
    }
    keys
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

        let mut sorted: Vec<String> = normalized.iter().cloned().collect();
        sorted.sort();
        crate::logging::info(&format!(
            "Account available models [{}]: {}",
            scope,
            sorted.join(", ")
        ));
        OPENAI_MODEL_CATALOG_SERVICE.replace_scope_models(
            scope,
            normalized.clone(),
            SystemTime::now(),
        );
        OPENAI_MODEL_CATALOG_SERVICE.note_attempt(scope);
        for model in &normalized {
            OPENAI_MODEL_CATALOG_SERVICE.clear_runtime_model_unavailable(scope, model);
        }
        crate::bus::Bus::global().publish_models_updated();
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

    let mut sorted: Vec<String> = normalized.iter().cloned().collect();
    sorted.sort();
    crate::logging::info(&format!(
        "Anthropic available models [{}]: {}",
        scope,
        sorted.join(", ")
    ));
    ANTHROPIC_MODEL_CATALOG_SERVICE.replace_scope_models(scope, normalized, SystemTime::now());
    crate::bus::Bus::global().publish_models_updated();
}

#[cfg(test)]
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

#[cfg(test)]
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
    cached_anthropic_model_ids().unwrap_or_else(anthropic_static_model_ids)
}

/// True when an OpenAI platform API key is configured (env or openai.env).
/// Deliberately a direct credential probe: routing/list decisions must not
/// populate the process-global cached `AuthStatus` snapshot as a side effect
/// (callers like `known_openai_model_ids` run before auth fixtures/state are
/// finalized, and a poisoned cache misreports every subsequent route).
pub fn openai_platform_api_key_configured() -> bool {
    crate::provider_catalog::load_api_key_from_env_or_config("OPENAI_API_KEY", "openai.env")
        .map(|value| !value.trim().is_empty())
        .unwrap_or(false)
}

pub fn known_openai_model_ids() -> Vec<String> {
    let mut models = cached_openai_model_ids().unwrap_or_else(openai_static_model_ids);
    if !models.iter().any(|model| model == CHATGPT_WEB_MODEL) {
        models.push(CHATGPT_WEB_MODEL.to_string());
    }
    // GPT Pro models never appear in the ChatGPT/Codex OAuth catalog (they are
    // platform-API-only), so a live OAuth catalog must not hide them when the
    // user has an OPENAI_API_KEY that can actually reach them.
    if openai_platform_api_key_configured() {
        for pro in OPENAI_API_ONLY_PRO_MODELS {
            if !models.iter().any(|model| model == pro) {
                models.push((*pro).to_string());
            }
        }
    }
    models
}

pub fn note_openai_model_catalog_refresh_attempt() {
    OPENAI_MODEL_CATALOG_SERVICE.note_attempt(&current_openai_account_scope());
}

fn note_openai_model_catalog_refresh_attempt_for_scope(scope: &str) {
    OPENAI_MODEL_CATALOG_SERVICE.note_attempt(scope);
}

fn openai_model_catalog_refresh_throttled() -> bool {
    let scope = current_openai_account_scope();
    OPENAI_MODEL_CATALOG_SERVICE.refresh_throttled(&scope)
}

fn anthropic_model_catalog_refresh_throttled(scope: &str) -> bool {
    ANTHROPIC_MODEL_CATALOG_SERVICE.refresh_throttled(scope)
}

pub fn should_refresh_openai_model_catalog() -> bool {
    if account_model_cache_is_fresh() {
        return false;
    }
    if openai_model_catalog_refresh_throttled() {
        return false;
    }
    OPENAI_MODEL_CATALOG_SERVICE.should_refresh(&current_openai_account_scope())
}

pub fn should_refresh_anthropic_model_catalog() -> bool {
    let scope = current_anthropic_catalog_scope();
    if anthropic_model_cache_is_fresh(&scope) {
        return false;
    }
    if anthropic_model_catalog_refresh_throttled(&scope) {
        return false;
    }
    ANTHROPIC_MODEL_CATALOG_SERVICE.should_refresh(&scope)
}

pub fn begin_openai_model_catalog_refresh() -> bool {
    let scope = current_openai_account_scope();
    OPENAI_MODEL_CATALOG_SERVICE.begin_refresh(&scope)
}

pub fn begin_anthropic_model_catalog_refresh() -> Option<String> {
    let scope = current_anthropic_catalog_scope();
    ANTHROPIC_MODEL_CATALOG_SERVICE
        .begin_refresh(&scope)
        .then_some(scope)
}

pub fn finish_openai_model_catalog_refresh() {
    OPENAI_MODEL_CATALOG_SERVICE.finish_refresh(&current_openai_account_scope());
}

fn finish_openai_model_catalog_refresh_for_scope(scope: &str) {
    OPENAI_MODEL_CATALOG_SERVICE.finish_refresh(scope);
}

pub fn finish_anthropic_model_catalog_refresh_for_scope(scope: &str) {
    ANTHROPIC_MODEL_CATALOG_SERVICE.finish_refresh(scope);
}

fn account_model_cache_is_fresh() -> bool {
    let scope = current_openai_account_scope();
    OPENAI_MODEL_CATALOG_SERVICE.is_fresh(&scope)
}

fn anthropic_model_cache_is_fresh(scope: &str) -> bool {
    ANTHROPIC_MODEL_CATALOG_SERVICE.is_fresh(scope)
}

fn runtime_model_unavailability(model: &str) -> Option<RuntimeModelUnavailability> {
    let scope = current_openai_account_scope();
    let model = normalize_model_id(model);
    if model.is_empty() {
        return None;
    }
    OPENAI_MODEL_CATALOG_SERVICE.runtime_model_unavailability(&scope, &model)
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
    OPENAI_MODEL_CATALOG_SERVICE.contains_model(&scope, &key)
}

fn account_models_observed_at() -> Option<SystemTime> {
    let scope = current_openai_account_scope();
    OPENAI_MODEL_CATALOG_SERVICE.observed_at(&scope)
}

/// Refresh the OpenAI model catalog in the background.
///
/// `is_chatgpt_mode` is the authoritative discriminator for which endpoint to
/// hit and must come from the loaded credential's shape
/// (`OpenAIProvider::is_chatgpt_mode`), never from sniffing the token string or
/// the requested credential *intent*. ChatGPT/Codex OAuth sessions use the
/// `backend-api/codex/models` endpoint; platform API keys (`sk-*`) use
/// `api.openai.com/v1/models`, which rejects Codex tokens (and vice versa) with
/// a 401.
pub fn refresh_openai_model_catalog_in_background(
    access_token: String,
    is_chatgpt_mode: bool,
    context: &'static str,
) {
    let scope = current_openai_account_scope();
    if access_token.trim().is_empty() {
        finish_openai_model_catalog_refresh_for_scope(&scope);
        return;
    }

    let use_platform_api = !is_chatgpt_mode;

    tokio::spawn(async move {
        let refresh_result = if use_platform_api {
            fetch_openai_api_key_model_catalog(&access_token).await
        } else {
            fetch_openai_model_catalog(&access_token).await
        };
        match refresh_result {
            Ok(catalog)
                if !catalog.available_models.is_empty() || !catalog.context_limits.is_empty() =>
            {
                crate::logging::info(&format!(
                    "Refreshed OpenAI model catalog ({}{}): {} available, {} with context limits",
                    context,
                    if use_platform_api {
                        ", platform-api"
                    } else {
                        ", codex-api"
                    },
                    catalog.available_models.len(),
                    catalog.context_limits.len()
                ));
                persist_openai_model_catalog(&catalog);
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
                    "Failed to refresh OpenAI model catalog from {} ({}): {}",
                    if use_platform_api {
                        "platform API"
                    } else {
                        "Codex API"
                    },
                    context,
                    e
                ));
            }
        }
        note_openai_model_catalog_refresh_attempt_for_scope(&scope);
        finish_openai_model_catalog_refresh_for_scope(&scope);
    });
}

pub fn record_model_unavailable_for_account(model: &str, reason: &str) {
    let scope = current_openai_account_scope();
    let model = normalize_model_id(model);
    if model.is_empty() {
        return;
    }
    OPENAI_MODEL_CATALOG_SERVICE.record_runtime_model_unavailable(&scope, &model, reason);
}

pub fn clear_model_unavailable_for_account(model: &str) {
    let scope = current_openai_account_scope();
    let model = normalize_model_id(model);
    if model.is_empty() {
        return;
    }
    OPENAI_MODEL_CATALOG_SERVICE.clear_runtime_model_unavailable(&scope, &model);
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
    OPENAI_MODEL_CATALOG_SERVICE.clear_runtime_model_unavailable_scope(&scope);
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
    if model.trim() == CHATGPT_WEB_MODEL {
        return AccountModelAvailability {
            state: AccountModelAvailabilityState::Unknown,
            reason: Some("requires a logged-in ChatGPT web session".to_string()),
            source: "browser-session",
            observed_at: None,
        };
    }

    if let Some(runtime) = runtime_model_unavailability(model) {
        return AccountModelAvailability {
            state: AccountModelAvailabilityState::Unavailable,
            reason: Some(runtime.reason),
            source: "runtime-error",
            observed_at: Some(runtime.observed_at),
        };
    }

    // GPT Pro models are platform-API-only: the ChatGPT/Codex OAuth account
    // snapshot never contains them, so judging them against it would report
    // "not available for your account" to every OAuth user who also has a
    // perfectly working OPENAI_API_KEY. Key presence is the real signal.
    if is_openai_api_only_pro_model(model) {
        return if openai_platform_api_key_configured() {
            AccountModelAvailability {
                state: AccountModelAvailabilityState::Available,
                reason: None,
                source: "api-key",
                observed_at: None,
            }
        } else {
            AccountModelAvailability {
                state: AccountModelAvailabilityState::Unavailable,
                reason: Some(
                    "requires an OpenAI platform API key (OPENAI_API_KEY); \
                     not available via ChatGPT/Codex OAuth"
                        .to_string(),
                ),
                source: "api-key",
                observed_at: None,
            }
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
    "gpt-5.5",
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
    let models = OPENAI_MODEL_CATALOG_SERVICE.model_ids(&scope)?;

    for preferred in OPENAI_MODEL_PREFERENCE {
        if models.iter().any(|model| model == *preferred)
            && runtime_model_unavailability(preferred).is_none()
        {
            return Some(preferred.to_string());
        }
    }

    models
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
    context_limit_for_model_with_provider_and_cache(model, provider_hint, get_cached_context_limit)
}

pub fn resolve_model_capabilities(model: &str, provider_hint: Option<&str>) -> ModelCapabilities {
    let provider = provider_for_model_with_hint(model, provider_hint).map(str::to_string);
    let context_window = context_limit_for_model_with_provider(model, provider_hint);
    ModelCapabilities {
        provider,
        context_window,
    }
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
    } else if jcode_provider_core::model_id::matches_known_model(model, ALL_CLAUDE_MODELS) {
        Some("claude")
    } else if jcode_provider_core::model_id::matches_known_model(model, ALL_OPENAI_MODELS) {
        Some("openai")
    } else if crate::provider::bedrock::BedrockProvider::is_bedrock_model_id(model) {
        Some("bedrock")
    } else if model.contains('/') {
        Some("openrouter")
    } else if model.starts_with("claude-") {
        Some("claude")
    } else if model.starts_with("gpt-") {
        Some("openai")
    } else if model.starts_with("gemini-") {
        Some("gemini")
    } else if let Some(provider) = core_provider_for_model_with_hint(model, None) {
        Some(provider)
    } else if crate::provider::antigravity::is_known_model(model) {
        Some("antigravity")
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
