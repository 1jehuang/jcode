//! Cross-layer invalidation signal for the cached session list.
//!
//! The session list cache itself is a TUI session-picker implementation detail,
//! but the *events* that should invalidate it (e.g. a session rename handled by
//! the server) originate in lower layers. To avoid those layers depending on
//! `tui`, the cache owner registers an invalidator here at startup and
//! producers call [`invalidate`].

use std::sync::{LazyLock, RwLock};

type Invalidator = fn();

static INVALIDATORS: LazyLock<RwLock<Vec<Invalidator>>> = LazyLock::new(|| RwLock::new(Vec::new()));

/// Register a callback that clears a session-list cache.
///
/// Inverts the historical `server -> tui` dependency: the TUI session picker
/// registers its cache-clearing function here (via `cli::startup`) so the
/// server can request invalidation without referencing `tui`.
pub fn register_invalidator(invalidator: Invalidator) {
    INVALIDATORS
        .write()
        .unwrap_or_else(|poisoned| poisoned.into_inner())
        .push(invalidator);
}

/// Invalidate every registered session-list cache. No-op if none are
/// registered (e.g. in a process with no TUI session picker).
pub fn invalidate() {
    let invalidators = INVALIDATORS
        .read()
        .unwrap_or_else(|poisoned| poisoned.into_inner());
    for invalidator in invalidators.iter() {
        invalidator();
    }
}
