//! Background scheduler for OpenAI-compatible provider catalog refreshes.
//!
//! Historically, catalog refreshes were scheduled as a *side effect of
//! rendering routes*: every `model_routes()` build walked every configured
//! OpenAI-compatible profile and, for each stale or missing disk cache,
//! scheduled a background `/models` fetch. Because the route memo has a short
//! TTL, that meant attaching a new session (or opening the model picker) could
//! fan out dozens of HTTP requests.
//!
//! Rendering and refreshing are now separate concerns:
//!
//! - Route building is pure: it reads disk/memory caches and never performs or
//!   schedules I/O.
//! - This scheduler owns refresh cadence. One process-global task sweeps the
//!   configured profiles on a fixed interval and asks the runtime hook to
//!   refresh only the caches that are actually stale or missing.
//!
//! The runtime-side scheduler (`jcode-provider-openrouter-runtime`) still
//! applies its own per-profile single-flight guard and exponential failure
//! backoff, so a sweep is cheap and idempotent.

use std::sync::OnceLock;
use std::time::Duration;

/// How often the sweeper wakes up. Individual profiles refresh far less often:
/// the sweep only acts on caches that are already stale (15 minutes) or
/// missing, and the runtime enforces per-profile backoff on top of that.
const SWEEP_INTERVAL: Duration = Duration::from_secs(60);

/// Delay before the first sweep. Startup already has plenty of contention
/// (auth, session restore, first render), and cached catalogs are served
/// immediately, so there is no reason to add a request burst to it.
const INITIAL_SWEEP_DELAY: Duration = Duration::from_secs(10);

static SCHEDULER_STARTED: OnceLock<()> = OnceLock::new();

/// Invalidate every memoized route catalog in the process. Called when
/// catalogs change out-of-band (refresh completion, auth changes), which is
/// what keeps the route memo TTL a backstop rather than the mechanism.
pub fn bump_catalog_generation() {
    super::CATALOG_GENERATION.fetch_add(1, std::sync::atomic::Ordering::Relaxed);
}

/// Start the background catalog sweeper once per process.
///
/// Safe and cheap to call from hot paths (route building calls it): after the
/// first successful start this is a single atomic load. It is a no-op when no
/// tokio runtime is available, and will start on a later call once one is.
pub(crate) fn ensure_started() {
    if SCHEDULER_STARTED.get().is_some() {
        return;
    }
    let Ok(handle) = tokio::runtime::Handle::try_current() else {
        // No runtime yet (early startup, sync tests, CLI one-shots). Leave the
        // OnceLock unset so a later call from an async context can start it.
        return;
    };
    if SCHEDULER_STARTED.set(()).is_err() {
        return;
    }

    handle.spawn(async move {
        tokio::time::sleep(INITIAL_SWEEP_DELAY).await;
        loop {
            sweep_stale_profile_catalogs();
            tokio::time::sleep(SWEEP_INTERVAL).await;
        }
    });
}

/// Whether a profile's on-disk catalog cache is missing, mismatched, or stale
/// and therefore worth a background refresh.
///
/// This is the pure predicate the sweeper acts on. Route building deliberately
/// does not call it: rendering must never schedule I/O.
pub(crate) fn profile_catalog_cache_needs_refresh(
    profile: crate::provider_catalog::OpenAiCompatibleProfile,
) -> bool {
    let resolved = crate::provider_catalog::resolve_openai_compatible_profile(profile);
    match super::cached_live_models_for_openai_compatible_profile(&resolved) {
        Some((_, cache_is_stale)) => cache_is_stale,
        // Missing or endpoint-mismatched cache: nothing usable to serve.
        None => true,
    }
}

/// Schedule refreshes for every configured profile whose catalog cache is
/// stale or missing. Returns the number of refreshes actually scheduled, which
/// is typically zero on most sweeps.
fn sweep_stale_profile_catalogs() -> usize {
    let mut scheduled = 0usize;

    for profile in crate::provider_catalog::openai_compatible_profiles()
        .iter()
        .copied()
    {
        if !crate::provider_catalog::openai_compatible_profile_is_configured(profile) {
            continue;
        }
        if !profile_catalog_cache_needs_refresh(profile) {
            continue;
        }
        if super::openrouter::maybe_schedule_openai_compatible_profile_catalog_refresh(
            profile,
            "scheduled catalog sweep",
        ) {
            scheduled += 1;
        }
    }

    // The standard OpenRouter catalog lives in its own cache namespace and is
    // not refreshed by the active-provider path when a direct OpenAI-compatible
    // profile occupies the shared runtime slot. The scheduler hook is a no-op
    // when that cache is already fresh.
    if super::openrouter::maybe_schedule_standard_openrouter_catalog_refresh(
        "scheduled catalog sweep",
    ) {
        scheduled += 1;
    }

    if scheduled > 0 {
        crate::logging::info(&format!(
            "Catalog sweep scheduled {} background provider catalog refresh(es)",
            scheduled
        ));
    }

    scheduled
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn ensure_started_is_a_noop_without_a_runtime() {
        // Must not panic when called from a synchronous context.
        ensure_started();
        assert!(SCHEDULER_STARTED.get().is_none());
    }

    #[tokio::test]
    async fn ensure_started_is_idempotent_under_a_runtime() {
        ensure_started();
        // A second call must not spawn another sweeper.
        ensure_started();
        assert!(SCHEDULER_STARTED.get().is_some());
    }

    #[test]
    fn sweep_interval_is_shorter_than_the_cache_staleness_window() {
        // The sweeper must wake often enough to notice a stale cache promptly,
        // but the staleness window is what actually paces refreshes.
        assert!(
            SWEEP_INTERVAL
                < Duration::from_secs(
                    super::super::OPENAI_COMPATIBLE_PROFILE_CATALOG_SOFT_REFRESH_SECS
                )
        );
    }
}
