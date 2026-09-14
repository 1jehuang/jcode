//! A counter that changes exactly when the loaded `[pricing]` config changes.
//!
//! Two memos outside this module cache prices and neither key can see a
//! hand-edited `config.toml`:
//!
//! * the TUI's per-model price memo (`CostState::cached_price_model`, keyed on
//!   `{model}|{tier}`) keeps serving what it resolved before the edit, and
//! * the route catalog's memo (`provider::RoutesMemoEntry`, fresh for
//!   `auth_generation + catalog_generation + 60s`) keeps feeding the model
//!   picker the prices it read at build time.
//!
//! Both now fold this counter in, so an edit to `[pricing]` takes effect on the
//! next read rather than at the next restart (or after the 60s TTL at best).
//!
//! The counter is driven by the *identity* of the loaded config, which
//! `sources::pricing_config` already compares to decide whether its validated
//! view is current: each reload leaks a fresh `Config`, so the address is an
//! exact answer to "which config is in force" and no second scheme for
//! fingerprinting config contents is needed here.

use std::sync::atomic::{AtomicU64, Ordering};

/// Bumped when a config identity the memoized `[pricing]` view was not built
/// from comes into force.
static PRICING_GENERATION: AtomicU64 = AtomicU64::new(0);

/// Record that the `[pricing]` view changed because the config did.
///
/// Called by [`super::sources::pricing_config`] when it replaces its memo, so
/// the generation is in step with the config that is actually in force.
pub(super) fn bump_pricing_generation() {
    PRICING_GENERATION.fetch_add(1, Ordering::Relaxed);
}

/// The current pricing generation.
///
/// Reading it also re-checks the loaded config against the memoized `[pricing]`
/// view, so a caller that compares generations notices a config edit as soon as
/// it looks instead of only once something else happens to price a call. That
/// matters for the route-catalog memo, whose freshness predicate runs *before*
/// it rebuilds (and so before anything would otherwise re-read the config). The
/// check is the one `sources::pricing_config` performs anyway: a mutex lock and
/// a pointer comparison on the steady-state path.
pub fn pricing_generation() -> u64 {
    // The view itself is not what this function returns; the identity check
    // that produces it is, and that check bumps the counter on a config change.
    let _ = super::sources::pricing_config();
    PRICING_GENERATION.load(Ordering::Relaxed)
}
