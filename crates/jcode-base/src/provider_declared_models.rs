//! Model ids a user declared explicitly in config.
//!
//! Extracted from `crate::provider_catalog` so that module can stay flat under
//! `scripts/check_code_size_budget.py`. Callers name this module directly
//! (`crate::provider_declared_models::...`).
//!
//! It also owns the single validated reader for a profile's on-disk `/models`
//! cache, [`cached_live_models_for_namespace`], shared by the direct profile
//! routes in `crate::provider` and the OpenRouter namespace lookup.

use crate::provider::openai_compatible_profile_catalog_cache_is_stale;
use crate::provider_catalog::{normalize_api_base, openai_compatible_profile_by_id};
use std::collections::HashSet;

/// Cached `/models` ids for `namespace`, accepted only when the cache records
/// the same `api_base` the requesting profile actually talks to.
///
/// The cache namespace is process-global (`JCODE_OPENROUTER_CACHE_NAMESPACE`,
/// defaulting to `openrouter`), so a namespace file can be left holding another
/// endpoint's catalog: a stale profile cache, or a direct compatible profile
/// that never set the namespace var. Trusting such a file makes every picker row
/// carry the wrong provider's models (issue #1824). A mismatch is reported as
/// "no usable cache" so callers fall back to their static list and the
/// background catalog sweeper schedules a refetch.
pub(crate) fn cached_live_models_for_namespace(
    namespace: &str,
    expected_api_base: &str,
) -> Option<(Vec<String>, bool)> {
    let cache = jcode_provider_openrouter::load_disk_cache_entry_for_namespace(namespace)?;
    let cache_is_stale = jcode_provider_openrouter::current_unix_secs()
        .map(|now| openai_compatible_profile_catalog_cache_is_stale(cache.cached_at, now))
        .unwrap_or(false);
    let source_api_base = cache
        .source_api_base
        .as_deref()
        .and_then(normalize_api_base)?;
    let expected_api_base = normalize_api_base(expected_api_base)?;
    if source_api_base != expected_api_base {
        return None;
    }

    let models = cache
        .models
        .into_iter()
        .map(|model| model.id.trim().to_string())
        .filter(|model| !model.is_empty())
        .collect::<Vec<_>>();
    if models.is_empty() {
        None
    } else {
        Some((models, cache_is_stale))
    }
}

/// Model ids a user explicitly declared for `profile_name` in config.toml via
/// `[[providers.<name>.models]]`.
///
/// A live `GET /models` catalog is not authoritative for OpenAI-compatible
/// endpoints: gateways routinely serve a model on chat completions while
/// omitting it from the list endpoint (issue #1824, MiniMax
/// `MiniMax-M3.1-Flash-Preview`). Callers union these ids into the picker list so
/// an endpoint gap can be repaired locally, and they pass through the same
/// filters (`is_listable_model_name`, chat-support) as fetched models.
///
/// A user-defined profile is addressed by its config key (`minimax` for
/// `[providers.minimax]`), which is also the cache namespace and the
/// `openai-compatible:<id>` api-method suffix, so the lookup is exact. A
/// `[providers.openai-compatible]` block keyed with the builtin profile id is
/// matched by `default_model`/`base_url` instead, since keying it with a
/// profile id would shadow the builtin catalog.
pub fn config_declared_model_ids_for_profile(profile_id: &str) -> Vec<String> {
    let profile_id = profile_id.trim();
    let Some(profile) = crate::config::config().providers.get(profile_id) else {
        return Vec::new();
    };
    if !config_declared_profile_matches_endpoint(profile_id, profile) {
        return Vec::new();
    }

    let mut models: Vec<String> = profile
        .models
        .iter()
        .filter(|model| {
            // `input` empty means unspecified (assume text-capable), matching
            // `named_provider_profile_routes`.
            model.input.is_empty() || model.input.iter().any(|input| input == "text")
        })
        .map(|model| model.id.trim().to_string())
        .filter(|id| !id.is_empty())
        .collect();
    if models.is_empty()
        && let Some(default_model) = profile
            .default_model
            .as_deref()
            .map(str::trim)
            .filter(|model| !model.is_empty())
    {
        models.push(default_model.to_string());
    }

    let mut seen = HashSet::new();
    models
        .into_iter()
        .filter(|model| seen.insert(model.clone()))
        .collect()
}

/// Whether a `[providers.<profile_id>]` block really targets the built-in
/// OpenAI-compatible profile with this id, rather than shadowing the name with
/// an unrelated endpoint.
fn config_declared_profile_matches_endpoint(
    profile_id: &str,
    profile: &crate::config::NamedProviderConfig,
) -> bool {
    let Some(builtin) = openai_compatible_profile_by_id(profile_id) else {
        return true;
    };
    if normalize_api_base(&profile.base_url).is_none() {
        return false;
    }
    if normalize_api_base(&profile.base_url) == normalize_api_base(builtin.api_base) {
        return true;
    }

    // A differing `base_url` is tolerated only when the block keeps the built-in
    // endpoint's default model, i.e. it adds metadata (a per-model
    // `context_window`) without redirecting requests.
    match (
        profile
            .default_model
            .as_deref()
            .map(str::trim)
            .filter(|model| !model.is_empty()),
        builtin.default_model,
    ) {
        (Some(declared), Some(builtin_default)) => declared == builtin_default,
        _ => false,
    }
}
