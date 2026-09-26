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
use std::sync::{Mutex, MutexGuard, OnceLock, TryLockError};
#[cfg(any(test, feature = "test-support"))]
use std::time::{Duration, Instant};

/// How long a test may wait for the shared test-env lock before the wait is
/// reported as a failure instead of hanging the suite forever.
#[cfg(any(test, feature = "test-support"))]
const DEFAULT_TEST_ENV_LOCK_TIMEOUT: Duration = Duration::from_secs(300);

#[cfg(any(test, feature = "test-support"))]
pub fn test_env_lock() -> &'static Mutex<()> {
    static ENV_LOCK: OnceLock<Mutex<()>> = OnceLock::new();
    ENV_LOCK.get_or_init(|| Mutex::new(()))
}

/// Name of the thread that last acquired the shared test-env lock.
///
/// A test that re-enters `lock_test_env()` on the thread that already holds it
/// deadlocks the entire suite, and the stuck thread is indistinguishable from a
/// plain waiter (`std::sync::Mutex` is not reentrant). Recording the last
/// successful acquirer on the acquire path is enough to name the culprit when a
/// wait times out: while a holder is stuck, no other thread can acquire, so the
/// record still points at it.
#[cfg(any(test, feature = "test-support"))]
fn env_lock_holder() -> &'static Mutex<Option<EnvLockHolder>> {
    static LAST_HOLDER: OnceLock<Mutex<Option<EnvLockHolder>>> = OnceLock::new();
    LAST_HOLDER.get_or_init(|| Mutex::new(None))
}

#[cfg(any(test, feature = "test-support"))]
#[derive(Clone)]
struct EnvLockHolder {
    thread: std::thread::ThreadId,
    label: String,
    acquired: Instant,
    /// Cleared on guard drop. A released record still names the last acquirer
    /// for the timeout message but never counts as a re-entrant holder.
    held: bool,
}

#[cfg(any(test, feature = "test-support"))]
impl EnvLockHolder {
    fn is_held_by_current_thread(&self) -> bool {
        self.held && self.thread == std::thread::current().id()
    }
}

#[cfg(any(test, feature = "test-support"))]
fn env_lock_thread_label() -> String {
    std::thread::current()
        .name()
        .map(str::to_string)
        .unwrap_or_else(|| format!("{:?}", std::thread::current().id()))
}

/// Bound for the shared test-env lock wait, overridable with
/// `JCODE_TEST_ENV_LOCK_TIMEOUT_SECS` for a shorter diagnosis window.
#[cfg(any(test, feature = "test-support"))]
fn test_env_lock_timeout() -> Duration {
    std::env::var("JCODE_TEST_ENV_LOCK_TIMEOUT_SECS").map_or(
        DEFAULT_TEST_ENV_LOCK_TIMEOUT,
        |value| {
            value
                .trim()
                .parse::<u64>()
                .map_or(DEFAULT_TEST_ENV_LOCK_TIMEOUT, |secs| match secs {
                    0 => DEFAULT_TEST_ENV_LOCK_TIMEOUT,
                    secs => Duration::from_secs(secs),
                })
        },
    )
}

/// Copy of the last acquirer record, recovering a poisoned lock the same way
/// [`test_env_lock`] does.
#[cfg(any(test, feature = "test-support"))]
fn env_lock_last_holder() -> Option<EnvLockHolder> {
    env_lock_holder()
        .lock()
        .unwrap_or_else(|poisoned| poisoned.into_inner())
        .clone()
}

/// Guard for the shared test-env lock. Dropping it clears the holder record
/// so a later contended wait on another thread cannot mistake this thread's
/// stale record for a re-entrant acquisition.
#[cfg(any(test, feature = "test-support"))]
pub struct TestEnvGuard {
    _guard: MutexGuard<'static, ()>,
}

#[cfg(any(test, feature = "test-support"))]
impl Drop for TestEnvGuard {
    fn drop(&mut self) {
        let mut holder = env_lock_holder()
            .lock()
            .unwrap_or_else(|poisoned| poisoned.into_inner());
        if let Some(h) = holder.as_mut()
            && h.thread == std::thread::current().id()
        {
            h.held = false;
        }
    }
}

#[cfg(any(test, feature = "test-support"))]
pub fn lock_test_env() -> TestEnvGuard {
    let mutex = test_env_lock();
    // Try first so the uncontended case stays a single syscall, and so a
    // poisoned lock keeps the previous recovery behavior.
    let guard = match mutex.try_lock() {
        Ok(guard) => guard,
        Err(TryLockError::Poisoned(poisoned)) => poisoned.into_inner(),
        Err(TryLockError::WouldBlock) => wait_for_test_env_lock(mutex),
    };
    *env_lock_holder()
        .lock()
        .unwrap_or_else(|poisoned| poisoned.into_inner()) = Some(EnvLockHolder {
        thread: std::thread::current().id(),
        label: env_lock_thread_label(),
        acquired: Instant::now(),
        held: true,
    });
    TestEnvGuard { _guard: guard }
}

/// How long a self-deadlock has to persist before it is reported. The last
/// acquirer is stored a few instructions after the lock is taken, so a thread
/// can briefly read its own stale record while another thread is taking the
/// lock; a genuine re-entrant acquisition never clears.
#[cfg(any(test, feature = "test-support"))]
const SELF_DEADLOCK_GRACE: Duration = Duration::from_millis(200);

#[cfg(any(test, feature = "test-support"))]
fn wait_for_test_env_lock(mutex: &'static Mutex<()>) -> MutexGuard<'static, ()> {
    let me = env_lock_thread_label();
    let started = Instant::now();
    let timeout = test_env_lock_timeout();
    let mut warned = false;
    let mut self_recorded_since: Option<Instant> = None;
    loop {
        match mutex.try_lock() {
            Ok(guard) => return guard,
            Err(TryLockError::Poisoned(poisoned)) => return poisoned.into_inner(),
            Err(TryLockError::WouldBlock) => {
                #[cfg(test)]
                tests::on_env_lock_contention();
            }
        }
        let waited = started.elapsed();
        // The holder record still pointing at this thread while the lock cannot
        // be taken means this thread already holds it: `std::sync::Mutex` is not
        // reentrant, so this wait can never end.
        let recorded_me =
            env_lock_last_holder().is_some_and(|holder| holder.is_held_by_current_thread());
        if recorded_me {
            match self_recorded_since {
                Some(since) if since.elapsed() >= SELF_DEADLOCK_GRACE => panic!(
                    "`{me}` called lock_test_env() while its thread already held the shared test-env \
                     lock, which deadlocks the whole suite: std::sync::Mutex is not reentrant. Take \
                     the lock once per test, or make the helper that re-enters it degrade to try_lock \
                     the way `ensure_test_jcode_home_if_unset` does."
                ),
                Some(_) => {}
                None => self_recorded_since = Some(Instant::now()),
            }
        } else {
            self_recorded_since = None;
        }
        if waited >= timeout {
            let holder = match env_lock_last_holder() {
                Some(holder) => format!(
                    "{} acquired it {:?} ago",
                    holder.label,
                    holder.acquired.elapsed()
                ),
                None => "no acquirer was recorded".to_string(),
            };
            panic!(
                "`{me}` waited {waited:?} for the shared test-env lock and never got it ({holder}). \
                 A test that calls lock_test_env() while its thread already holds the lock \
                 self-deadlocks the suite; the last acquirer named above is the usual culprit. \
                 Hold the lock once per test, degrade a helper that may run under a locked test to \
                 try_lock, or raise JCODE_TEST_ENV_LOCK_TIMEOUT_SECS if the wait is legitimate."
            );
        }
        if !warned && waited >= Duration::from_secs(10) {
            warned = true;
            eprintln!("warning: `{me}` is waiting {waited:?} for the shared test-env lock");
        }
        std::thread::sleep(Duration::from_millis(25));
    }
}

#[cfg(test)]
mod tests;
