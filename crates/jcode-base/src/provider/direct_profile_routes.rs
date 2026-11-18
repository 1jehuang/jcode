use super::*;

// Keep inactive direct profiles on the same 15-minute soft-refresh cadence as
// the active OpenRouter/OpenAI-compatible runtime. We continue serving the
// cached routes immediately while a background refresh updates the catalog.
pub(crate) const OPENAI_COMPATIBLE_PROFILE_CATALOG_SOFT_REFRESH_SECS: u64 = 15 * 60;

pub(super) fn openai_compatible_profile_catalog_cache_is_stale(cached_at: u64, now: u64) -> bool {
    now.saturating_sub(cached_at) >= OPENAI_COMPATIBLE_PROFILE_CATALOG_SOFT_REFRESH_SECS
}

pub(crate) fn cached_live_models_for_openai_compatible_profile(
    resolved: &crate::provider_catalog::ResolvedOpenAiCompatibleProfile,
) -> Option<(Vec<String>, bool)> {
    let cache = jcode_provider_openrouter::load_disk_cache_entry_for_namespace(&resolved.id)?;
    let cache_is_stale = jcode_provider_openrouter::current_unix_secs()
        .map(|now| openai_compatible_profile_catalog_cache_is_stale(cache.cached_at, now))
        .unwrap_or(false);
    let source_api_base = cache
        .source_api_base
        .as_deref()
        .and_then(crate::provider_catalog::normalize_api_base)?;
    let expected_api_base = crate::provider_catalog::normalize_api_base(&resolved.api_base)?;
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

pub(super) fn direct_openai_compatible_profile_routes(
    profile: crate::provider_catalog::OpenAiCompatibleProfile,
) -> Vec<ModelRoute> {
    let resolved = crate::provider_catalog::resolve_openai_compatible_profile(profile);
    let static_models = crate::provider_catalog::openai_compatible_profile_static_models(profile);
    // Pure read: the catalog scheduler owns refresh cadence, so rendering
    // routes cannot fan out HTTP requests.
    let (mut models, from_live_catalog) = if let Some((models, _cache_is_stale)) =
        cached_live_models_for_openai_compatible_profile(&resolved)
    {
        (models, true)
    } else {
        let mut models = static_models;
        if models.is_empty()
            && let Some(default_model) = resolved.default_model.as_ref()
            && !default_model.trim().is_empty()
        {
            models.push(default_model.trim().to_string());
        }
        (models, false)
    };

    let provider = resolved.display_name.clone();
    let api_method = format!("openai-compatible:{}", resolved.id);
    let detail = if from_live_catalog {
        resolved.api_base.clone()
    } else if resolved.api_base.trim().is_empty() {
        "fallback: static provider model list".to_string()
    } else {
        format!(
            "{}; fallback: static provider model list",
            resolved.api_base
        )
    };

    let mut routes = Vec::new();
    for model in models.drain(..) {
        if !is_listable_model_name(&model)
            || !crate::provider_catalog::openai_compatible_profile_model_supports_chat(
                &resolved.id,
                &model,
            )
            || routes.iter().any(|route: &ModelRoute| route.model == model)
        {
            continue;
        }

        routes.push(ModelRoute {
            model,
            provider: provider.clone(),
            api_method: api_method.clone(),
            available: true,
            detail: detail.clone(),
            usage: None,
            cheapness: None,
        });
    }

    routes
}

pub(super) fn standard_openrouter_profile_configured() -> bool {
    crate::provider_catalog::load_env_value_from_env_or_config(
        "OPENROUTER_API_KEY",
        "openrouter.env",
    )
    .is_some()
}

pub(super) fn configured_standard_openrouter_profile_routes() -> Vec<ModelRoute> {
    let Some(cache) = jcode_provider_openrouter::load_disk_cache_entry_for_namespace("openrouter")
    else {
        return Vec::new();
    };

    let source_matches_openrouter = cache
        .source_api_base
        .as_deref()
        .and_then(crate::provider_catalog::normalize_api_base)
        .map(|base| base.contains("openrouter.ai"))
        .unwrap_or(false);
    if !source_matches_openrouter {
        return Vec::new();
    }

    let available = standard_openrouter_profile_configured();
    cache
        .models
        .into_iter()
        .map(|model| model.id.trim().to_string())
        .filter(|model| is_listable_model_name(model))
        .map(|model| build_openrouter_auto_route(&model, available, String::new()))
        .collect()
}
