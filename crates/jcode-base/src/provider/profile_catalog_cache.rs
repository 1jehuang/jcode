/// Cached `/models` ids for `namespace`, accepted only when the cache records
/// the same `api_base` the requesting profile actually talks to.
///
/// The cache namespace is process-global (`KCODE_OPENROUTER_CACHE_NAMESPACE`,
/// defaulting to `openrouter`), so a namespace file can be left holding another
/// endpoint's catalog: a stale profile cache, or a direct compatible profile
/// that never set the namespace var. Trusting such a file makes every picker row
/// carry the wrong provider's models (issue #1824). A mismatch is reported as
/// "no usable cache" so callers fall back to their static list and the
/// background catalog sweeper schedules a refetch.
pub(super) fn cached_live_models_for_namespace(
    namespace: &str,
    expected_api_base: &str,
) -> Option<(Vec<String>, bool)> {
    let cache = jcode_provider_openrouter::load_disk_cache_entry_for_namespace(namespace)?;
    let cache_is_stale = jcode_provider_openrouter::current_unix_secs()
        .map(|now| super::openai_compatible_profile_catalog_cache_is_stale(cache.cached_at, now))
        .unwrap_or(false);
    let source_api_base = cache
        .source_api_base
        .as_deref()
        .and_then(crate::provider_catalog::normalize_api_base)?;
    let expected_api_base = crate::provider_catalog::normalize_api_base(expected_api_base)?;
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
