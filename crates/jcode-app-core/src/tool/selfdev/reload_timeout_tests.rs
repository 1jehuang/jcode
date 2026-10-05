//! Tests for the JCODE_SELFDEV_RELOAD_TIMEOUT_SECS override.

use super::*;

#[test]
fn reload_timeout_secs_defaults_to_15() {
    let _storage_guard = crate::storage::lock_test_env();
    let _guard = EnvVarGuard::remove("JCODE_SELFDEV_RELOAD_TIMEOUT_SECS");
    assert_eq!(SelfDevTool::reload_timeout_secs(), 15);
}

#[test]
fn reload_timeout_secs_honors_valid_env_override() {
    let _storage_guard = crate::storage::lock_test_env();
    let _guard = EnvVarGuard::set("JCODE_SELFDEV_RELOAD_TIMEOUT_SECS", "27");
    assert_eq!(SelfDevTool::reload_timeout_secs(), 27);
}

#[test]
fn reload_timeout_secs_ignores_empty_invalid_and_zero_values() {
    let _storage_guard = crate::storage::lock_test_env();
    let _guard = EnvVarGuard::set("JCODE_SELFDEV_RELOAD_TIMEOUT_SECS", "   ");
    assert_eq!(SelfDevTool::reload_timeout_secs(), 15);
    drop(_guard);

    let _guard = EnvVarGuard::set("JCODE_SELFDEV_RELOAD_TIMEOUT_SECS", "abc");
    assert_eq!(SelfDevTool::reload_timeout_secs(), 15);
    drop(_guard);

    let _guard = EnvVarGuard::set("JCODE_SELFDEV_RELOAD_TIMEOUT_SECS", "0");
    assert_eq!(SelfDevTool::reload_timeout_secs(), 15);
}
