use crate::auth;
use crate::provider::cursor;

#[path = "models_catalog.rs"]
mod catalog;
#[path = "model_catalog_refresh.rs"]
mod catalog_refresh;
#[path = "model_catalog_service.rs"]
mod catalog_service;
use catalog_refresh::note_openai_model_catalog_refresh_attempt_for_scope;
pub use catalog_refresh::{
    begin_anthropic_model_catalog_refresh, begin_anthropic_model_catalog_refresh_for_scope,
    begin_forced_anthropic_model_catalog_refresh_for_scope, begin_openai_model_catalog_refresh,
    begin_openai_model_catalog_refresh_for_scope, finish_anthropic_model_catalog_refresh_for_scope,
    finish_openai_model_catalog_refresh, finish_openai_model_catalog_refresh_for_scope,
    note_openai_model_catalog_refresh_attempt, should_refresh_anthropic_model_catalog,
    should_refresh_anthropic_model_catalog_for_scope, should_refresh_openai_model_catalog,
    should_refresh_openai_model_catalog_for_scope,
};

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
    ALL_CLAUDE_MODELS, ALL_OPENAI_MODELS, CHATGPT_WEB_MODEL, ModelCapabilities,
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
    #[serde(default)]
    service_tiers: HashMap<String, Vec<String>>,
    observed_at_unix_secs: u64,
}

// Keep field order identical to PersistedModelCatalogScope. Only the refreshed
// payload is borrowed, avoiding deep copies made solely for serialization.
#[derive(Serialize)]
struct BorrowedScope<'a> {
    models: &'a [String],
    context_limits: &'a HashMap<String, usize>,
    reasoning_efforts: &'a HashMap<String, Vec<String>>,
    service_tiers: &'a HashMap<String, Vec<String>>,
    observed_at_unix_secs: u64,
}

struct ScopeOverlay<'a> {
    scopes: &'a HashMap<String, PersistedModelCatalogScope>,
    scope: &'a str,
    replacement: BorrowedScope<'a>,
}

impl Serialize for ScopeOverlay<'_> {
    fn serialize<S: serde::Serializer>(&self, serializer: S) -> Result<S::Ok, S::Error> {
        use serde::ser::SerializeMap;
        let mut map = serializer.serialize_map(Some(self.scopes.len()))?;
        for (scope, snapshot) in self.scopes {
            if scope == self.scope {
                map.serialize_entry(scope, &self.replacement)?;
            } else {
                map.serialize_entry(scope, snapshot)?;
            }
        }
        map.end()
    }
}

#[derive(Serialize)]
struct StoreOverlay<'a> {
    scopes: ScopeOverlay<'a>,
}

fn borrowed_catalog_store<'a>(
    store: &'a mut PersistedModelCatalogStore,
    scope: &'a str,
    replacement: BorrowedScope<'a>,
) -> StoreOverlay<'a> {
    // Insert exactly as the owned path did, preserving HashMap bucket order
    // even for a new scope. The discarded old payload is no longer needed.
    // Empty placeholders allocate no model/context/capability collections.
    store.scopes.insert(
        scope.to_string(),
        PersistedModelCatalogScope {
            models: Vec::new(),
            context_limits: HashMap::new(),
            reasoning_efforts: HashMap::new(),
            service_tiers: HashMap::new(),
            observed_at_unix_secs: replacement.observed_at_unix_secs,
        },
    );
    StoreOverlay {
        scopes: ScopeOverlay {
            scopes: &store.scopes,
            scope,
            replacement,
        },
    }
}

#[cfg(test)]
pub(crate) fn filtered_display_models(models: impl IntoIterator<Item = String>) -> Vec<String> {
    models
        .into_iter()
        .filter(|model| {
            !crate::subscription_catalog::is_runtime_mode_enabled()
                || crate::subscription_catalog::is_model_allowed_for_current_tier(model)
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

#[path = "models/model_catalog_state.rs"]
mod model_catalog_state;
pub(crate) use model_catalog_state::normalize_model_id;
pub(crate) use model_catalog_state::openai_route_context_limit_resolver;
pub use model_catalog_state::{
    anthropic_catalog_scope_for_route, cached_anthropic_model_ids,
    cached_anthropic_model_ids_for_scope, cached_context_limit_for_model, cached_openai_model_ids,
    cached_openai_model_ids_for_scope, cached_openai_reasoning_efforts,
    cached_openai_reasoning_efforts_for_scope, cached_openai_service_tiers_for_scope,
    clear_model_unavailable_for_account, clear_model_unavailable_for_scope,
    openai_catalog_scope_for_credential, persist_anthropic_model_catalog,
    persist_anthropic_model_catalog_for_scope, persist_openai_model_catalog,
    persist_openai_model_catalog_for_scope, populate_account_models,
    populate_account_models_for_scope, populate_anthropic_models,
    populate_anthropic_models_for_scope, populate_context_limits,
    populate_context_limits_from_config, populate_context_limits_from_config_value,
    record_model_unavailable_for_account, record_model_unavailable_for_scope,
    refresh_openai_model_catalog_in_background,
    refresh_openai_model_catalog_in_background_for_scope,
};
use model_catalog_state::{
    anthropic_static_model_ids, configured_openai_api_key_catalog_scope,
    current_anthropic_catalog_scope, current_openai_account_scope,
    current_provider_runtime_scope_key, openai_static_model_ids, provider_runtime_scope_key,
};
#[cfg(test)]
use model_catalog_state::{
    hydrate_catalog_cache_from_disk, load_persisted_model_catalog_store, model_catalog_cache_path,
    model_ids_with_context_aliases, observed_at_unix_secs, persist_scoped_model_catalog,
    save_persisted_model_catalog_store, system_time_from_unix_secs,
};
/// Test-only: clear the process-global in-memory model catalogs. The catalog
/// services are statics shared by every test in the process; a test that
/// hydrates a scope (directly or via `persist_*` + `cached_*`) otherwise leaks
/// fixture models into later tests' `known_*_model_ids()` validation.
#[cfg(any(test, feature = "test-support"))]
pub fn reset_model_catalog_services_for_tests() {
    OPENAI_MODEL_CATALOG_SERVICE.reset_for_tests();
    ANTHROPIC_MODEL_CATALOG_SERVICE.reset_for_tests();
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
    let api = cached_anthropic_model_ids_for_scope(&anthropic_catalog_scope_for_route(false))
        .or_else(|| {
            super::anthropic::load_anthropic_api_key()
                .is_ok()
                .then(anthropic_static_model_ids)
        });
    let oauth = cached_anthropic_model_ids_for_scope(&anthropic_catalog_scope_for_route(true))
        .or_else(|| {
            auth::claude::load_credentials()
                .is_ok()
                .then(anthropic_static_model_ids)
        });
    if api.is_none() && oauth.is_none() {
        return anthropic_static_model_ids();
    }
    let mut models = api.unwrap_or_default();
    models.extend(oauth.unwrap_or_default());
    models.sort();
    models.dedup();
    models
}

pub fn known_anthropic_model_ids_for_scope(scope: &str) -> Vec<String> {
    cached_anthropic_model_ids_for_scope(scope).unwrap_or_else(anthropic_static_model_ids)
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

/// Every OpenAI model reachable through any configured credential: the union of
/// the ChatGPT/Codex OAuth catalog and the platform API-key catalog. Used where
/// no specific credential is selected yet (route listing, runtime-less
/// validation). A runtime validating its own credential should use
/// [`known_openai_model_ids_for_scope`].
pub fn known_openai_model_ids() -> Vec<String> {
    let oauth = cached_openai_model_ids();
    let api = configured_openai_api_key_catalog_scope()
        .and_then(|scope| cached_openai_model_ids_for_scope(&scope));
    let models = match (oauth, api) {
        (None, None) => openai_static_model_ids(),
        (oauth, api) => {
            let mut models = oauth.unwrap_or_default();
            for model in api.unwrap_or_default() {
                if !models.contains(&model) {
                    models.push(model);
                }
            }
            models
        }
    };
    finish_known_openai_model_ids(models)
}

/// OpenAI models valid for one credential's catalog scope (see
/// [`openai_catalog_scope_for_credential`]).
pub fn known_openai_model_ids_for_scope(scope: &str) -> Vec<String> {
    finish_known_openai_model_ids(
        cached_openai_model_ids_for_scope(scope).unwrap_or_else(openai_static_model_ids),
    )
}

fn finish_known_openai_model_ids(mut models: Vec<String>) -> Vec<String> {
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

fn runtime_model_unavailability(scope: &str, model: &str) -> Option<RuntimeModelUnavailability> {
    let model = normalize_model_id(model);
    if model.is_empty() {
        return None;
    }
    OPENAI_MODEL_CATALOG_SERVICE.runtime_model_unavailability(scope, &model)
}

fn account_snapshot_model_available(scope: &str, model: &str) -> Option<bool> {
    if !OPENAI_MODEL_CATALOG_SERVICE.is_fresh(scope) {
        return None;
    }
    let key = normalize_model_id(model);
    if key.is_empty() {
        return None;
    }
    OPENAI_MODEL_CATALOG_SERVICE.contains_model(scope, &key)
}

fn account_models_observed_at(scope: &str) -> Option<SystemTime> {
    OPENAI_MODEL_CATALOG_SERVICE.observed_at(scope)
}

#[path = "model_provider_availability.rs"]
mod provider_availability;
pub use provider_availability::*;
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
    model_availability_for_scope(&current_openai_account_scope(), model)
}

/// Availability of `model` within one credential's catalog scope (see
/// [`openai_catalog_scope_for_credential`]).
pub fn model_availability_for_scope(scope: &str, model: &str) -> AccountModelAvailability {
    if model.trim() == CHATGPT_WEB_MODEL {
        return AccountModelAvailability {
            state: AccountModelAvailabilityState::Unknown,
            reason: Some("requires a logged-in ChatGPT web session".to_string()),
            source: "browser-session",
            observed_at: None,
        };
    }

    if let Some(runtime) = runtime_model_unavailability(scope, model) {
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

    if !OPENAI_MODEL_CATALOG_SERVICE.is_fresh(scope) {
        return AccountModelAvailability {
            state: AccountModelAvailabilityState::Unknown,
            reason: Some("availability snapshot is stale".to_string()),
            source: "account-snapshot",
            observed_at: account_models_observed_at(scope),
        };
    }

    match account_snapshot_model_available(scope, model) {
        Some(true) => AccountModelAvailability {
            state: AccountModelAvailabilityState::Available,
            reason: None,
            source: "account-snapshot",
            observed_at: account_models_observed_at(scope),
        },
        Some(false) => AccountModelAvailability {
            state: AccountModelAvailabilityState::Unavailable,
            reason: Some("not available for your account".to_string()),
            source: "account-snapshot",
            observed_at: account_models_observed_at(scope),
        },
        None => AccountModelAvailability {
            state: AccountModelAvailabilityState::Unknown,
            reason: Some("no availability snapshot yet".to_string()),
            source: "account-snapshot",
            observed_at: account_models_observed_at(scope),
        },
    }
}

/// Preferred model order for fallback selection.
/// If the desired model isn't available, we try these in order.
const OPENAI_MODEL_PREFERENCE: &[&str] = &[
    "gpt-6-astra",
    "gpt-5.6-sol",
    "gpt-5.6-pro",
    "gpt-5.6",
    "gpt-5.6-terra",
    "gpt-5.6-luna",
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
    get_best_available_openai_model_for_scope(&current_openai_account_scope())
}

pub fn get_best_available_openai_model_for_scope(scope: &str) -> Option<String> {
    if !OPENAI_MODEL_CATALOG_SERVICE.is_fresh(scope) {
        return None;
    }
    let models = OPENAI_MODEL_CATALOG_SERVICE.model_ids(scope)?;

    for preferred in OPENAI_MODEL_PREFERENCE {
        if models.iter().any(|model| model == *preferred)
            && runtime_model_unavailability(scope, preferred).is_none()
        {
            return Some(preferred.to_string());
        }
    }

    models
        .into_iter()
        .find(|model| runtime_model_unavailability(scope, model).is_none())
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
    context_limit_for_model_with_provider_and_cache(
        model,
        provider_hint,
        cached_context_limit_for_model,
    )
}

pub fn resolve_model_capabilities(model: &str, provider_hint: Option<&str>) -> ModelCapabilities {
    let provider = provider_for_model_with_hint(model, provider_hint).map(str::to_string);
    let context_window = context_limit_for_model_with_provider(model, provider_hint);
    ModelCapabilities {
        provider,
        context_window,
    }
}

/// Automatic agent failover is narrower than manual model selection. A model
/// appearing in an account's catalog is not evidence that it can run an agent
/// turn: platform catalogs also advertise video, audio and embedding models.
/// Accept curated/frontier agent models, plus versioned GPT variants so an
/// explicitly configured new release need not wait for the bundled order.
/// Unrecognized catalogs remain available for manual selection, not failover.
pub(super) fn is_agent_model_for_failover(model: &str, provider: &str) -> bool {
    let normalized = normalize_model_id(model);
    if !super::is_listable_model_name(&normalized) || normalized.contains("[web]") {
        return false;
    }
    if provider == "bedrock" {
        return super::bedrock::BedrockProvider::model_supports_tools(model);
    }
    let base = jcode_provider_core::model_id::slash_base(&normalized);
    // These GPT specializations do not implement the normal agent tool API.
    if base
        .split('-')
        .any(|token| matches!(token, "instruct" | "search" | "research"))
    {
        return false;
    }
    if crate::auth::lifecycle::ranked_flagship_for_provider(provider, &[model]).is_some() {
        return true;
    }
    matches!(provider, "openai" | "copilot" | "cursor" | "openrouter")
        && base
            .strip_prefix("gpt-")
            .and_then(|version| version.chars().next())
            .is_some_and(|first| first.is_ascii_digit())
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

#[cfg(test)]
mod catalog_ownership_tests {
    use super::*;

    #[test]
    fn openai_route_capacity_ignores_unscoped_context_cache() {
        let _env = crate::storage::lock_test_env();
        let model = "gpt-6-regression-credential-scoped-context";
        populate_context_limits(HashMap::from([(model.to_string(), 123_000)]));
        let resolver = openai_route_context_limit_resolver();
        let legacy = context_limit_for_model_with_provider(model, Some("openai"));
        let api = resolver(model, "openai-api");
        let account = resolver(model, "openai-oauth@regression-new-account");
        CONTEXT_LIMIT_CACHE.write().unwrap().remove(model);
        assert_eq!(legacy, Some(123_000), "reproduce the poisoned global cache");
        assert_eq!(api, Some(1_000_000));
        assert_eq!(account, Some(1_000_000));
    }

    #[test]
    fn model_picker_transfers_first_string_and_preserves_order() {
        let first = "ownership-picker-future-model-a".to_string();
        let second = "ownership-picker-future-model-b".to_string();
        let first_backing = first.as_ptr();
        let second_backing = second.as_ptr();
        let duplicate = first.clone();
        let result = model_ids_with_context_aliases(vec![
            String::new(),
            first,
            duplicate,
            second,
            String::new(),
        ]);
        assert_eq!(
            result,
            [
                "ownership-picker-future-model-a",
                "ownership-picker-future-model-b",
            ]
        );
        assert_eq!(result[0].as_ptr(), first_backing);
        assert_eq!(result[1].as_ptr(), second_backing);
    }

    fn overlay<'a>(
        store: &'a mut PersistedModelCatalogStore,
        scope: &'a str,
        replacement: &'a PersistedModelCatalogScope,
    ) -> StoreOverlay<'a> {
        borrowed_catalog_store(
            store,
            scope,
            BorrowedScope {
                models: &replacement.models,
                context_limits: &replacement.context_limits,
                reasoning_efforts: &replacement.reasoning_efforts,
                service_tiers: &replacement.service_tiers,
                observed_at_unix_secs: replacement.observed_at_unix_secs,
            },
        )
    }

    fn benchmark_snapshot(count: usize) -> PersistedModelCatalogScope {
        let mut result = snapshot(Vec::new(), HashMap::new());
        result.observed_at_unix_secs = 1_795_000_123;
        for index in 0..count {
            let model = format!("future-model-{index:05}-long-model-name");
            result.models.push(model.clone());
            result.context_limits.insert(model.clone(), 128_000 + index);
            result
                .reasoning_efforts
                .insert(model.clone(), vec!["low".into(), "high".into()]);
            result.service_tiers.insert(
                model,
                if index % 2 == 0 {
                    vec!["priority".into()]
                } else {
                    Vec::new()
                },
            );
        }
        result
    }

    #[test]
    fn borrowed_overlay_matches_legacy_compact_and_pretty_bytes() {
        for count in [0, 1, 32, 2048] {
            let replacement = benchmark_snapshot(count);
            for scope_count in [0, 1, 8] {
                let store = PersistedModelCatalogStore {
                    scopes: (0..scope_count)
                        .map(|index| (format!("account::{index}"), benchmark_snapshot(3)))
                        .collect(),
                };
                for scope in ["account::0", "api-key::new-endpoint"] {
                    let mut legacy = store.clone();
                    legacy.scopes.insert(scope.to_string(), replacement.clone());
                    let mut candidate = store.clone();
                    let view = overlay(&mut candidate, scope, &replacement);
                    assert_eq!(
                        serde_json::to_vec(&view).unwrap(),
                        serde_json::to_vec(&legacy).unwrap()
                    );
                    assert_eq!(
                        serde_json::to_vec_pretty(&view).unwrap(),
                        serde_json::to_vec_pretty(&legacy).unwrap()
                    );
                }
            }
        }
    }

    #[cfg(target_os = "linux")]
    fn thread_cpu_ns() -> u64 {
        let mut time = libc::timespec {
            tv_sec: 0,
            tv_nsec: 0,
        };
        // SAFETY: clock_gettime writes only the valid timespec pointed to here.
        assert_eq!(
            unsafe { libc::clock_gettime(libc::CLOCK_THREAD_CPUTIME_ID, &mut time) },
            0
        );
        time.tv_sec as u64 * 1_000_000_000 + time.tv_nsec as u64
    }

    #[test]
    #[ignore = "release-only CPU experiment, run explicitly with --release --ignored --nocapture"]
    #[cfg(target_os = "linux")]
    fn scoped_writer_release_cpu_comparison() {
        assert!(!cfg!(debug_assertions), "run with --release");
        use std::hint::black_box;
        for (name, models, scopes, batch) in [
            ("tiny", 1, 1, 4096),
            ("small", 32, 4, 512),
            ("large", 2048, 8, 8),
        ] {
            let replacement = benchmark_snapshot(models);
            let fixture = PersistedModelCatalogStore {
                scopes: (0..scopes)
                    .map(|index| (format!("account::{index}"), benchmark_snapshot(models)))
                    .collect(),
            };
            for scope in ["account::0", "api-key::new-endpoint"] {
                let mut legacy_times = Vec::new();
                let mut overlay_times = Vec::new();
                for round in 0..9 {
                    for candidate in if round % 2 == 0 {
                        [false, true]
                    } else {
                        [true, false]
                    } {
                        // Parsing is identical for both paths. Prepare equally owned stores
                        // outside timing to isolate update + serialization + destruction CPU.
                        let stores: Vec<_> = (0..batch).map(|_| fixture.clone()).collect();
                        let start = thread_cpu_ns();
                        for mut store in stores {
                            if candidate {
                                let view = overlay(&mut store, scope, &replacement);
                                black_box(serde_json::to_vec(&view).unwrap());
                            } else {
                                store.scopes.insert(scope.to_string(), replacement.clone());
                                black_box(serde_json::to_vec(&store).unwrap());
                            }
                            black_box(store);
                        }
                        let elapsed = (thread_cpu_ns() - start) / batch;
                        if candidate {
                            overlay_times.push(elapsed);
                        } else {
                            legacy_times.push(elapsed);
                        }
                    }
                }
                legacy_times.sort_unstable();
                overlay_times.sort_unstable();
                println!(
                    "{name} scope={scope} legacy_ns={} overlay_ns={} ratio={:.4} legacy_samples={legacy_times:?} overlay_samples={overlay_times:?}",
                    legacy_times[4],
                    overlay_times[4],
                    overlay_times[4] as f64 / legacy_times[4] as f64
                );
            }
        }
    }

    struct CatalogTestHome {
        previous: Option<std::ffi::OsString>,
        _temp: tempfile::TempDir,
        _lock: std::sync::MutexGuard<'static, ()>,
    }

    impl CatalogTestHome {
        fn new() -> Self {
            let lock = crate::storage::lock_test_env();
            let temp = tempfile::tempdir().unwrap();
            let previous = std::env::var_os("JCODE_HOME");
            crate::env::set_var("JCODE_HOME", temp.path());
            Self {
                previous,
                _temp: temp,
                _lock: lock,
            }
        }
    }

    impl Drop for CatalogTestHome {
        fn drop(&mut self) {
            if let Some(previous) = &self.previous {
                crate::env::set_var("JCODE_HOME", previous);
            } else {
                crate::env::remove_var("JCODE_HOME");
            }
        }
    }

    #[test]
    fn borrowed_persistence_preserves_empty_timestamp_and_concurrent_scopes() {
        let _home = CatalogTestHome::new();
        let file_name = "borrowed-catalog-persistence.json";
        let path = model_catalog_cache_path(file_name).unwrap();
        let replacement = benchmark_snapshot(3);
        let persist = |scope: &str, models: &[String]| {
            persist_scoped_model_catalog(
                file_name,
                scope,
                models,
                &replacement.context_limits,
                &replacement.reasoning_efforts,
                &replacement.service_tiers,
                system_time_from_unix_secs(replacement.observed_at_unix_secs),
            );
        };
        persist("empty", &[]);
        assert!(!path.exists(), "empty input must not create a cache");
        persist("oauth::account", &replacement.models);
        persist("api-key::endpoint", &replacement.models);
        let before = std::fs::read(&path).unwrap();
        persist("oauth::account", &[]);
        assert_eq!(std::fs::read(&path).unwrap(), before);

        std::thread::scope(|threads| {
            for index in 0..8 {
                let persist = &persist;
                let models = &replacement.models;
                threads.spawn(move || persist(&format!("concurrent::{index}"), models));
            }
        });
        let store = load_persisted_model_catalog_store(file_name).unwrap();
        assert_eq!(
            store.scopes.len(),
            10,
            "serialized read-modify-write must retain every scope"
        );
        let expected = serde_json::to_value(&replacement).unwrap();
        for snapshot in store.scopes.values() {
            assert_eq!(serde_json::to_value(snapshot).unwrap(), expected);
        }

        let updated = vec!["updated-model".to_string()];
        persist("oauth::account", &updated);
        let store = load_persisted_model_catalog_store(file_name).unwrap();
        assert_eq!(store.scopes["oauth::account"].models, updated);
        assert_eq!(
            serde_json::to_value(&store.scopes["api-key::endpoint"]).unwrap(),
            expected
        );
        assert_eq!(
            store.scopes["oauth::account"].observed_at_unix_secs,
            replacement.observed_at_unix_secs
        );
    }

    #[test]
    fn owned_capability_reads_preserve_missing_empty_and_account_scopes() {
        let _home = CatalogTestHome::new();
        assert_eq!(cached_openai_reasoning_efforts_for_scope("missing"), None);
        assert_eq!(cached_openai_service_tiers_for_scope("missing"), None);
        let efforts = HashMap::from([
            (
                "future-model".to_string(),
                vec!["high".to_string(), "low".to_string()],
            ),
            ("explicit-empty".to_string(), Vec::new()),
        ]);
        let tiers = HashMap::from([
            ("future-model".to_string(), vec!["priority".to_string()]),
            ("denied".to_string(), Vec::new()),
        ]);
        let mut selected = snapshot(vec!["future-model".to_string()], HashMap::new());
        selected.reasoning_efforts = efforts.clone();
        selected.service_tiers = tiers.clone();
        let store = PersistedModelCatalogStore {
            scopes: HashMap::from([
                ("oauth::account".to_string(), selected),
                (
                    "api-key::endpoint".to_string(),
                    snapshot(vec!["other".to_string()], HashMap::new()),
                ),
            ]),
        };
        save_persisted_model_catalog_store(OPENAI_MODEL_CATALOG_CACHE_FILE, &store);
        let path = model_catalog_cache_path(OPENAI_MODEL_CATALOG_CACHE_FILE).unwrap();
        let before = std::fs::read(&path).unwrap();
        for _ in 0..2 {
            assert_eq!(
                cached_openai_reasoning_efforts_for_scope("oauth::account"),
                Some(efforts.clone())
            );
            assert_eq!(
                cached_openai_service_tiers_for_scope("oauth::account"),
                Some(tiers.clone())
            );
            assert_eq!(
                cached_openai_reasoning_efforts_for_scope("api-key::endpoint"),
                None
            );
            assert_eq!(
                cached_openai_service_tiers_for_scope("api-key::endpoint"),
                Some(HashMap::new())
            );
            assert_eq!(cached_openai_reasoning_efforts_for_scope("missing"), None);
            assert_eq!(cached_openai_service_tiers_for_scope("missing"), None);
        }
        assert_eq!(std::fs::read(&path).unwrap(), before);
    }

    fn service() -> ModelCatalogService {
        ModelCatalogService::new(
            ACCOUNT_MODEL_CACHE_TTL,
            ACCOUNT_MODEL_REFRESH_RETRY_INTERVAL,
            RUNTIME_UNAVAILABLE_TTL,
        )
    }

    fn snapshot(
        models: Vec<String>,
        context_limits: HashMap<String, usize>,
    ) -> PersistedModelCatalogScope {
        PersistedModelCatalogScope {
            models,
            context_limits,
            reasoning_efforts: HashMap::new(),
            service_tiers: HashMap::new(),
            observed_at_unix_secs: observed_at_unix_secs(SystemTime::now()) - 5,
        }
    }

    #[test]
    fn populate_context_limits_moves_keys_and_preserves_unrelated_entries() {
        let model = "catalog-ownership-moved-key".to_string();
        let allocation = model.as_ptr() as usize;
        let unrelated = "catalog-ownership-unrelated-key".to_string();
        populate_context_limits(HashMap::from([(unrelated.clone(), 42), (model.clone(), 1)]));
        populate_context_limits(HashMap::from([(model, 987_654)]));
        populate_context_limits(HashMap::new());
        let mut cache = CONTEXT_LIMIT_CACHE.write().unwrap();
        let (key, value) = cache.get_key_value("catalog-ownership-moved-key").unwrap();
        // HashMap retains an existing equal key when replacing its value.
        assert_eq!(*value, 987_654);
        assert_eq!(cache.get(&unrelated), Some(&42));
        assert_ne!(key.as_ptr() as usize, allocation);
        cache.remove("catalog-ownership-moved-key");
        cache.remove(&unrelated);

        let fresh = "catalog-ownership-fresh-key".to_string();
        let allocation = fresh.as_ptr() as usize;
        drop(cache);
        populate_context_limits(HashMap::from([(fresh, 123_456)]));
        let mut cache = CONTEXT_LIMIT_CACHE.write().unwrap();
        let (key, value) = cache.get_key_value("catalog-ownership-fresh-key").unwrap();
        assert_eq!(
            key.as_ptr() as usize,
            allocation,
            "new keys must be moved, not cloned"
        );
        assert_eq!(*value, 123_456);
        cache.remove("catalog-ownership-fresh-key");
    }

    #[test]
    fn hydration_moves_only_selected_scope_without_mutating_disk() {
        let _env = crate::storage::lock_test_env();
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("catalog.json");
        let file_name = path.to_str().unwrap();
        let selected = "catalog-ownership-selected".to_string();
        let other = "catalog-ownership-other".to_string();
        let selected_snapshot = snapshot(
            vec![selected.clone(), selected.clone()],
            HashMap::from([(selected.clone(), 1_000_000)]),
        );
        let observed_at = system_time_from_unix_secs(selected_snapshot.observed_at_unix_secs);
        let store = PersistedModelCatalogStore {
            scopes: HashMap::from([
                ("account::selected".to_string(), selected_snapshot),
                (
                    "endpoint::other".to_string(),
                    snapshot(vec![other.clone()], HashMap::from([(other.clone(), 9)])),
                ),
                (
                    "empty".to_string(),
                    snapshot(
                        Vec::new(),
                        HashMap::from([("catalog-ownership-empty".to_string(), 9)]),
                    ),
                ),
                (
                    "invalid".to_string(),
                    snapshot(
                        vec![String::new()],
                        HashMap::from([("catalog-ownership-invalid".to_string(), 9)]),
                    ),
                ),
            ]),
        };
        crate::storage::write_json(&path, &store).unwrap();
        let before = std::fs::read(&path).unwrap();
        let service = service();
        assert_eq!(
            hydrate_catalog_cache_from_disk(file_name, "account::selected", &service),
            Some(vec![selected.clone(), format!("{selected}[1m]")])
        );
        assert_eq!(
            service.model_ids("account::selected"),
            Some(vec![selected.clone()])
        );
        assert_eq!(service.observed_at("account::selected"), Some(observed_at));
        assert_eq!(service.model_ids("endpoint::other"), None);
        assert_eq!(cached_context_limit_for_model(&selected), Some(1_000_000));
        assert_eq!(cached_context_limit_for_model(&other), None);
        for scope in ["missing", "empty", "invalid"] {
            assert_eq!(
                hydrate_catalog_cache_from_disk(file_name, scope, &service),
                None
            );
            assert_eq!(service.model_ids(scope), None);
        }
        assert_eq!(
            cached_context_limit_for_model("catalog-ownership-empty"),
            None
        );
        assert_eq!(
            cached_context_limit_for_model("catalog-ownership-invalid"),
            None
        );
        assert_eq!(std::fs::read(&path).unwrap(), before);
        assert_eq!(
            load_persisted_model_catalog_store(file_name)
                .unwrap()
                .scopes
                .len(),
            4
        );
        CONTEXT_LIMIT_CACHE.write().unwrap().remove(&selected);
    }
}

#[cfg(test)]
mod scoped_refresh_tests {
    use super::*;

    #[test]
    fn saved_account_background_refresh_uses_captured_scope() {
        let scope = format!(
            "captured-refresh-test-{}",
            chrono::Utc::now().timestamp_nanos_opt().unwrap()
        );
        assert!(!OPENAI_MODEL_CATALOG_SERVICE.refresh_throttled(&scope));
        assert!(begin_openai_model_catalog_refresh_for_scope(&scope));
        refresh_openai_model_catalog_in_background_for_scope(
            String::new(),
            true,
            "scope-test",
            scope.clone(),
        );
        assert!(
            OPENAI_MODEL_CATALOG_SERVICE.refresh_throttled(&scope),
            "completion must apply retry throttling to the captured scope, not the active login"
        );
    }
}
