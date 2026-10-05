//! Tests for `#[path]`-attributed module `idle_monitor_tests` of `server.rs`.
//!
//! Extracted from the parent file: an inline `#[cfg(test)] mod` counts toward
//! the code-size budget, which only exempts `*_tests.rs` files.

use super::idle_monitor_should_start;

#[test]
fn shared_idle_monitor_preserves_live_headless_worker() {
    assert!(!idle_monitor_should_start(0, true));
}

#[test]
fn temporary_idle_monitor_preserves_live_headless_worker() {
    assert!(!idle_monitor_should_start(0, true));
}

#[test]
fn idle_monitor_starts_only_without_clients_or_headless_workers() {
    assert!(idle_monitor_should_start(0, false));
    assert!(!idle_monitor_should_start(1, false));
}
