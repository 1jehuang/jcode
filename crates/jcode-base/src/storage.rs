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
pub fn lock_test_env() -> MutexGuard<'static, ()> {
    // Bounded acquisition, on purpose. Tests take this lock and then hold it
    // across async work, so one slow or blocked holder parks every other test
    // that needs it - and libtest reports the parked ones as "running for over
    // 60 seconds" with no hint about who holds it. Blocking forever turns a
    // diagnosable problem into an invisible one, so give up loudly instead.
    const ACQUIRE_DEADLINE: std::time::Duration = std::time::Duration::from_secs(180);
    const RETRY_INTERVAL: std::time::Duration = std::time::Duration::from_millis(25);

    let deadline = std::time::Instant::now() + ACQUIRE_DEADLINE;
    loop {
        match test_env_lock().try_lock() {
            Ok(guard) => return guard,
            Err(std::sync::TryLockError::Poisoned(poisoned)) => return poisoned.into_inner(),
            Err(std::sync::TryLockError::WouldBlock) => {
                if std::time::Instant::now() >= deadline {
                    panic!(
                        "lock_test_env: another test has held the process env lock for over {}s. \
                         Scope the guard to the env mutation (set_var/remove_var) instead of \
                         holding it across awaits or blocking I/O; see #1636.",
                        ACQUIRE_DEADLINE.as_secs()
                    );
                }
                std::thread::sleep(RETRY_INTERVAL);
            }
        }
    }
}

#[cfg(test)]
mod tests;
