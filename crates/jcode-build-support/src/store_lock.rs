//! Cross-process serialization of immutable-store publication and deletion.
//!
//! The lock file is permanent. Never unlink it: doing so permits two processes
//! to lock different inodes at the same pathname. The OS releases the lock on
//! process death, so recovery does not depend on PID reuse or stale timestamps.

use anyhow::{Context, Result};
use std::cell::RefCell;
use std::collections::HashMap;
use std::fs::{File, OpenOptions, TryLockError};
use std::path::{Path, PathBuf};
use std::rc::{Rc, Weak};
use std::time::{Duration, Instant};

const LOCK_TIMEOUT: Duration = Duration::from_secs(30);
const RETRY_INTERVAL: Duration = Duration::from_millis(25);
const LOCK_FILE: &str = ".store.lock";

thread_local! {
    // Reentrant only on the owning thread, for composed synchronous publishers.
    // A guard cannot move to another thread or survive an async Send boundary.
    static HELD: RefCell<HashMap<PathBuf, Weak<File>>> = RefCell::new(HashMap::new());
}

/// A publication transaction. Hold from before creating a version directory
/// until all channel links, markers, launcher and manifest references are saved.
/// Dropping the last guard releases the kernel lock, including during unwinding.
#[must_use]
pub struct BuildStoreLock {
    _file: Rc<File>,
}

/// Acquire the shared publisher/pruner lock, failing closed after 30 seconds.
/// Nested acquisition on the same thread and store is supported.
pub fn acquire_build_store_lock() -> Result<BuildStoreLock> {
    acquire_in(&crate::builds_dir()?, LOCK_TIMEOUT)
}

/// Opportunistic cleanup must not stall an executor behind a publisher.
pub(crate) fn acquire_build_store_lock_nonblocking() -> Result<BuildStoreLock> {
    acquire_in(&crate::builds_dir()?, Duration::ZERO)
}

pub(crate) fn acquire_in(builds: &Path, timeout: Duration) -> Result<BuildStoreLock> {
    std::fs::create_dir_all(builds)?;
    let path = std::fs::canonicalize(builds)?.join(LOCK_FILE);
    if let Some(file) = HELD.with(|held| held.borrow().get(&path).and_then(Weak::upgrade)) {
        return Ok(BuildStoreLock { _file: file });
    }
    let file = OpenOptions::new()
        .read(true)
        .write(true)
        .create(true)
        .truncate(false)
        .open(&path)
        .with_context(|| format!("Opening build store lock {}", path.display()))?;
    let started = Instant::now();
    #[cfg(all(test, unix))]
    let mut reported_contention = false;
    loop {
        match file.try_lock() {
            Ok(()) => break,
            Err(TryLockError::WouldBlock) if started.elapsed() < timeout => {
                #[cfg(all(test, unix))]
                if !reported_contention {
                    report_test_contention()?;
                    reported_contention = true;
                }
                std::thread::sleep(RETRY_INTERVAL.min(timeout.saturating_sub(started.elapsed())));
            }
            Err(TryLockError::WouldBlock) => {
                anyhow::bail!("Timed out waiting for build store lock {}", path.display());
            }
            Err(TryLockError::Error(error)) => {
                return Err(error)
                    .with_context(|| format!("Locking build store {}", path.display()));
            }
        }
    }
    let file = Rc::new(file);
    HELD.with(|held| {
        let mut held = held.borrow_mut();
        held.retain(|_, file| file.strong_count() > 0);
        held.insert(path, Rc::downgrade(&file));
    });
    Ok(BuildStoreLock { _file: file })
}

#[cfg(all(test, unix))]
fn report_test_contention() -> Result<()> {
    use std::io::Write;
    if let Some(socket) = std::env::var_os("JCODE_TEST_LOCK_ATTEMPT_SOCKET") {
        std::os::unix::net::UnixStream::connect(socket)?.write_all(b"blocked")?;
    }
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn nested_guards_hold_lock_until_last_guard_is_dropped() {
        let store = tempfile::tempdir().unwrap();
        let first = acquire_in(store.path(), Duration::ZERO).unwrap();
        let second = acquire_in(store.path(), Duration::ZERO).unwrap();
        drop(first);
        let contender = File::options()
            .read(true)
            .write(true)
            .open(store.path().join(LOCK_FILE))
            .unwrap();
        assert!(matches!(
            contender.try_lock(),
            Err(TryLockError::WouldBlock)
        ));
        drop(second);
        contender.try_lock().unwrap();
    }

    #[test]
    fn contention_times_out_without_unlinking_lock_file() {
        let store = tempfile::tempdir().unwrap();
        let guard = acquire_in(store.path(), Duration::ZERO).unwrap();
        let path = store.path().to_path_buf();
        let error = std::thread::spawn(move || {
            acquire_in(&path, Duration::ZERO).err().unwrap().to_string()
        })
        .join()
        .unwrap();
        assert!(error.contains("Timed out waiting for build store lock"));
        drop(guard);
        assert!(store.path().join(LOCK_FILE).is_file());
        acquire_in(store.path(), Duration::ZERO).unwrap();
    }

    #[test]
    fn invalid_lock_path_fails_closed() {
        let store = tempfile::tempdir().unwrap();
        std::fs::create_dir(store.path().join(LOCK_FILE)).unwrap();
        assert!(acquire_in(store.path(), Duration::ZERO).is_err());
    }
}
