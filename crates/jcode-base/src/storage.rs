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

/// Guard for [`lock_test_env`]. Derefs to the underlying [`MutexGuard`], so
/// existing `let _guard = lock_test_env();` call sites keep working unchanged.
#[cfg(any(test, feature = "test-support"))]
pub struct TestEnvGuard(MutexGuard<'static, ()>);

#[cfg(any(test, feature = "test-support"))]
impl std::ops::Deref for TestEnvGuard {
    type Target = MutexGuard<'static, ()>;
    fn deref(&self) -> &Self::Target {
        &self.0
    }
}

#[cfg(any(test, feature = "test-support"))]
impl std::ops::DerefMut for TestEnvGuard {
    fn deref_mut(&mut self) -> &mut Self::Target {
        &mut self.0
    }
}

#[cfg(any(test, feature = "test-support"))]
pub fn lock_test_env() -> TestEnvGuard {
    let guard = TestEnvGuard(
        test_env_lock()
            .lock()
            .unwrap_or_else(|poisoned| poisoned.into_inner()),
    );
    install_shared_test_home_once();
    guard
}

/// Install the shared test home the first time any test takes the env lock,
/// before that test's body can run. After this point `JCODE_HOME` is always
/// set: scoped tests capture it as their previous value and restore it on
/// drop, so the variable never returns to the unset gap that racing
/// [`ensure_shared_test_jcode_home_if_unset`] calls could observe mid-body of
/// a victim test (the session-picker cache flake, #1784). The install shares
/// the install lock with the unlocked helper, so the two entry points cannot
/// interleave an unset check with a set that lands after a locked test
/// scoped its own home. The `Once` still marks the install done: tests that
/// intentionally remove `JCODE_HOME` inside a locked body keep their unset
/// view for the body's duration.
#[cfg(any(test, feature = "test-support"))]
fn install_shared_test_home_once() {
    static INSTALLED: std::sync::Once = std::sync::Once::new();
    INSTALLED.call_once(|| {
        install_shared_test_home_if_unset();
    });
}

/// Point JCODE_HOME at a per-process temp dir when the environment does not
/// already pin one, so tests never read the developer's real `~/.jcode` state.
///
/// This helper runs without the shared env lock (blocking here deadlocks
/// render-locked tests that build apps, and skipping under contention
/// reintroduces the unset gap this helper exists to close), so its own
/// check-and-set races [`lock_test_env`]'s one-time install: a call parked on
/// the old unset check could resume its set after a locked test scoped its
/// own temporary home, repointing `JCODE_HOME` at the shared scratch home
/// under that test. Both entry points take the install lock and re-check the
/// variable inside it, so every set lands while the variable is still unset,
/// and a test that already took the env lock and scoped a home can never be
/// overwritten (install happens before the env lock is handed out, so by the
/// time any scoped home exists the variable was already set).
#[cfg(any(test, feature = "test-support"))]
pub fn ensure_shared_test_jcode_home_if_unset() {
    install_shared_test_home_if_unset();
}

#[cfg(any(test, feature = "test-support"))]
fn install_shared_test_home_if_unset() {
    static INSTALL_LOCK: OnceLock<Mutex<()>> = OnceLock::new();
    if std::env::var_os("JCODE_HOME").is_some() {
        return;
    }
    let _install = INSTALL_LOCK
        .get_or_init(Default::default)
        .lock()
        .unwrap_or_else(|poisoned| poisoned.into_inner());
    if std::env::var_os("JCODE_HOME").is_none() {
        crate::env::set_var("JCODE_HOME", shared_test_jcode_home());
    }
}

/// The shared per-process test home that [`ensure_shared_test_jcode_home_if_unset`]
/// installs when no test has scoped its own `JCODE_HOME`. Callers use it to tell
/// the shared home apart from a concurrently running test's scoped home before
/// touching files under it (e.g. wiping ambient state between apps).
#[cfg(any(test, feature = "test-support"))]
pub fn shared_test_jcode_home() -> &'static std::path::Path {
    static TEST_HOME: OnceLock<std::path::PathBuf> = OnceLock::new();
    TEST_HOME.get_or_init(|| {
        let path = std::env::temp_dir().join(format!("jcode-test-home-{}", std::process::id()));
        if let Err(err) = std::fs::create_dir_all(&path) {
            crate::logging::debug(&format!(
                "failed to create shared test home {}: {}",
                path.display(),
                err
            ));
        }
        path
    })
}

#[cfg(test)]
mod tests;
