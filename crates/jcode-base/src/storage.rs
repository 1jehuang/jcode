#![cfg_attr(test, allow(clippy::items_after_test_module))]

pub use jcode_storage::*;

use anyhow::Result;
use serde::de::DeserializeOwned;
use std::path::Path;

pub fn read_json<T: DeserializeOwned>(path: &Path) -> Result<T> {
    jcode_storage::read_json_with_recovery_handler(path, |event| match event {
        jcode_storage::StorageRecoveryEvent::CorruptPrimary { path, error } => {
            crate::logging::warn(&format!(
                "Corrupt JSON at {}, trying backup: {}",
                path.display(),
                error
            ));
        }
        jcode_storage::StorageRecoveryEvent::RecoveredFromBackup { backup_path } => {
            crate::logging::info(&format!("Recovered from backup: {}", backup_path.display()));
        }
    })
}

#[cfg(any(test, feature = "test-support"))]
use std::sync::{Mutex, MutexGuard, OnceLock};

#[cfg(any(test, feature = "test-support"))]
pub fn test_env_lock() -> &'static Mutex<()> {
    static ENV_LOCK: OnceLock<Mutex<()>> = OnceLock::new();
    ENV_LOCK.get_or_init(|| Mutex::new(()))
}

#[cfg(any(test, feature = "test-support"))]
pub fn lock_test_env() -> TestEnvGuard {
    let guard = test_env_lock()
        .lock()
        .unwrap_or_else(|poisoned| poisoned.into_inner());
    set_test_env_lock_held(true);
    TestEnvGuard { guard }
}

#[cfg(any(test, feature = "test-support"))]
thread_local! {
    static TEST_ENV_LOCK_HELD: std::cell::Cell<bool> = const { std::cell::Cell::new(false) };
}

#[cfg(any(test, feature = "test-support"))]
fn set_test_env_lock_held(held: bool) {
    TEST_ENV_LOCK_HELD.with(|cell| cell.set(held));
}

/// True only when the *current thread* holds `test_env_lock` (via
/// `lock_test_env`). Distinguishes self-held from another-test-held when a
/// `try_lock` fails, so helpers can skip env writes instead of clobbering a
/// concurrently scoped `JCODE_HOME`.
#[cfg(any(test, feature = "test-support"))]
pub fn test_env_lock_held_by_current_thread() -> bool {
    TEST_ENV_LOCK_HELD.with(|cell| cell.get())
}

#[cfg(any(test, feature = "test-support"))]
pub struct TestEnvGuard {
    guard: MutexGuard<'static, ()>,
}

#[cfg(any(test, feature = "test-support"))]
impl Drop for TestEnvGuard {
    fn drop(&mut self) {
        set_test_env_lock_held(false);
    }
}

#[cfg(test)]
mod tests;
