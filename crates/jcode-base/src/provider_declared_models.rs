//! Model ids a user declared explicitly in config.

//! Extracted from `crate::provider_catalog` so that module can stay flat under
//! `scripts/check_code_size_budget.py`. Provider routing calls this sibling
//! module directly.

use crate::provider_catalog::normalize_api_base;
use std::collections::HashSet;

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
/// `openai-compatible:<id>` api-method suffix, so the lookup is exact. Its
/// normalized endpoint must match the route's resolved endpoint; matching a
/// default model alone cannot authorize importing a foreign provider's models.
pub fn config_declared_model_ids_for_profile(
    profile_id: &str,
    expected_api_base: &str,
) -> Vec<String> {
    let profile_id = profile_id.trim();
    let Some(profile) = crate::config::config().providers.get(profile_id) else {
        return Vec::new();
    };
    if !config_declared_profile_matches_endpoint(expected_api_base, profile) {
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
    expected_api_base: &str,
    profile: &crate::config::NamedProviderConfig,
) -> bool {
    let Some(declared) = normalize_api_base(&profile.base_url) else {
        return false;
    };
    normalize_api_base(expected_api_base).is_some_and(|expected| declared == expected)
}
