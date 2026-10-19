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
thread_local! {
    /// Whether this thread holds the env lock, so nested `lock_test_env`
    /// calls on the same thread no-op instead of self-deadlocking.
    static TEST_ENV_LOCK_HELD: std::cell::Cell<bool> = const { std::cell::Cell::new(false) };
}

/// Reentrant guard for the shared test env lock.
///
/// Same-thread nesting returns a no-op guard (`std::sync::Mutex` is not
/// reentrant); cross-thread callers still serialize on the same mutex.
#[cfg(any(test, feature = "test-support"))]
pub struct TestEnvGuard(Option<MutexGuard<'static, ()>>);

#[cfg(any(test, feature = "test-support"))]
impl Drop for TestEnvGuard {
    fn drop(&mut self) {
        if self.0.take().is_some() {
            TEST_ENV_LOCK_HELD.with(|held| held.set(false));
        }
    }
}

#[cfg(any(test, feature = "test-support"))]
pub fn lock_test_env() -> TestEnvGuard {
    if TEST_ENV_LOCK_HELD.with(|held| held.get()) {
        return TestEnvGuard(None);
    }
    let guard = test_env_lock()
        .lock()
        .unwrap_or_else(|poisoned| poisoned.into_inner());
    TEST_ENV_LOCK_HELD.with(|held| held.set(true));
    TestEnvGuard(Some(guard))
}

#[cfg(test)]
mod tests;
