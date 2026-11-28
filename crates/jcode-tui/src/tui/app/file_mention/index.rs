//! Background file-index manager with RCU-style atomic snapshot swaps.

use std::path::{Path, PathBuf};
use std::sync::atomic::{AtomicBool, AtomicUsize, Ordering};
use std::sync::{Arc, Mutex, RwLock};
use std::time::Duration;

use super::cache::ensure_ignored_dir_scanned;
use super::search::build_path_index;
use super::types::PathIndex;

/// Manages background index builds with lock-free reads.
///
/// - Reads (`snapshot`) use `std::sync::RwLock::read` — the read lock is
///   never contended because the write lock is held for nanoseconds (an
///   `Arc` pointer swap).
/// - Writes happen in `tokio::spawn` tasks; `std::sync::RwLock::write`
///   blocks the worker thread briefly, which is acceptable for a sub-µs
///   critical section.
///
/// We deliberately use `std::sync::RwLock`, **not** `tokio::sync::RwLock`.
/// `tokio::sync::RwLock::blocking_read()` panics when called from inside a
/// tokio runtime, and `snapshot()` is called from the sync hot path which
/// runs on the tokio main thread.
pub(super) struct FileIndexManager {
    current: Arc<RwLock<Arc<PathIndex>>>,
    cwd: PathBuf,
    refreshing: Arc<AtomicBool>,
    /// Live count of files scanned by the in-flight build. Reset when a build
    /// starts, updated as the walker collects files, read by the popover to
    /// show indexing progress instead of a static hint.
    progress: Arc<AtomicUsize>,
    /// Flag set by the background file watcher (notify) when FS changes occur.
    /// Checked in `check_refresh` to trigger re-index without TTL polling.
    pub(super) dirty: Arc<AtomicBool>,
    /// Holds the notify watcher thread alive. Dropping this stops watching.
    #[allow(dead_code)]
    _watcher_handle: Arc<Mutex<Option<notify::RecommendedWatcher>>>,
}

struct RefreshingFlagGuard(Arc<AtomicBool>);

impl Drop for RefreshingFlagGuard {
    fn drop(&mut self) {
        self.0.store(false, Ordering::Release);
    }
}

impl FileIndexManager {
    pub fn new(cwd: PathBuf) -> Self {
        let dirty = Arc::new(AtomicBool::new(false));
        let watcher = Self::start_watcher(cwd.clone(), dirty.clone());

        Self {
            current: Arc::new(RwLock::new(Arc::new(PathIndex::empty(cwd.clone())))),
            cwd,
            refreshing: Arc::new(AtomicBool::new(false)),
            progress: Arc::new(AtomicUsize::new(0)),
            dirty,
            _watcher_handle: Arc::new(Mutex::new(watcher)),
        }
    }

    /// Test-only manager without a filesystem watcher and with a prebuilt
    /// index. The watcher is what makes parallel tests flaky here: a shared
    /// TMPDIR plus notify event churn can flip `dirty` mid-test and route
    /// `check_refresh` into `refresh_async`, whose `tokio::spawn` panics
    /// outside a runtime. Tests drive this manager synchronously instead.
    #[cfg(test)]
    pub(super) fn new_headless_with_index(cwd: PathBuf, index: PathIndex) -> Self {
        Self {
            current: Arc::new(RwLock::new(Arc::new(index))),
            cwd,
            refreshing: Arc::new(AtomicBool::new(false)),
            progress: Arc::new(AtomicUsize::new(0)),
            dirty: Arc::new(AtomicBool::new(false)),
            _watcher_handle: Arc::new(Mutex::new(None)),
        }
    }

    /// Spawn a background thread that watches `cwd` for filesystem changes
    /// and sets the `dirty` flag. Returns the watcher handle.
    fn start_watcher(cwd: PathBuf, dirty: Arc<AtomicBool>) -> Option<notify::RecommendedWatcher> {
        use notify::{Event, EventKind, RecursiveMode, Watcher};
        let (tx, rx) = std::sync::mpsc::channel();
        let mut watcher = notify::RecommendedWatcher::new(
            move |res: Result<Event, notify::Error>| {
                if let Ok(event) = res {
                    // Ignore access-only events; only care about creates, modifies,
                    // deletes, and renames.
                    match event.kind {
                        EventKind::Create(_)
                        | EventKind::Modify(_)
                        | EventKind::Remove(_)
                        | EventKind::Any => {}
                        _ => return,
                    }
                    let _ = tx.send(());
                }
            },
            notify::Config::default(),
        )
        .ok()?;

        let _ = watcher.watch(&cwd, RecursiveMode::NonRecursive);
        // Drain events; set dirty on any relevant filesystem change.
        // Debounce: only set dirty at most once per second.
        std::thread::spawn(move || {
            let mut last_set = std::time::Instant::now()
                .checked_sub(Duration::from_secs(2))
                .unwrap_or_else(|| std::time::Instant::now() - Duration::from_secs(2));
            for _ in rx {
                let now = std::time::Instant::now();
                if now.duration_since(last_set) >= Duration::from_secs(1) {
                    dirty.store(true, Ordering::Release);
                    last_set = now;
                }
            }
        });

        Some(watcher)
    }

    /// Obtain a lightweight snapshot of the current index.
    ///
    /// Uses `std::sync::RwLock::read` — safe on the sync hot path because
    /// the write lock is only held for an `Arc` pointer swap (~ns).
    pub fn snapshot(&self) -> Arc<PathIndex> {
        self.current
            .read()
            .unwrap_or_else(|e| e.into_inner())
            .clone()
    }

    /// Borrow the current index without cloning the `Arc<PathIndex>`.
    pub(super) fn with_index<R>(&self, f: impl FnOnce(&PathIndex) -> R) -> R {
        let guard = self.current.read().unwrap_or_else(|e| e.into_inner());
        f(&guard)
    }

    /// Lazily scan ignored directories by mutating only the lazy layer of the
    /// current index. The caller must not hold a snapshot `Arc`, so `Arc::make_mut`
    /// can usually borrow the current index mutably without cloning the full file
    /// list. If another reader briefly holds a snapshot, this still falls back to
    /// clone-on-write for correctness instead of blocking the UI path.
    pub(super) fn ensure_ignored_dir_scanned(&self, query: &str) -> bool {
        let mut guard = self.current.write().unwrap_or_else(|e| e.into_inner());
        let index = Arc::make_mut(&mut guard);
        let before_lazy = index.lazy_entries.len();
        let before_scanned = index.scanned_ignored_dirs.len();
        ensure_ignored_dir_scanned(query, index);
        index.lazy_entries.len() != before_lazy
            || index.scanned_ignored_dirs.len() != before_scanned
    }

    /// Kick off an async background refresh.
    ///
    /// Takes `&self` (not `&mut self`) because state is shared through `Arc`.
    pub fn refresh_async(&self) {
        if self.refreshing.swap(true, Ordering::Acquire) {
            return; // already refreshing
        }

        let cwd = self.cwd.clone();
        let current = self.current.clone();
        let refreshing = self.refreshing.clone();
        let progress = self.progress.clone();
        progress.store(0, Ordering::Relaxed);

        tokio::spawn(async move {
            let _refreshing_guard = RefreshingFlagGuard(refreshing);
            let new_index = build_path_index(&cwd, &progress).await;

            // Write lock held for sub-µs: just an Arc pointer swap.
            {
                let mut guard = current.write().unwrap_or_else(|e| e.into_inner());
                *guard = Arc::new(new_index);
            }
        });
    }

    pub fn cwd(&self) -> &Path {
        &self.cwd
    }

    pub fn is_refreshing(&self) -> bool {
        self.refreshing.load(Ordering::Acquire)
    }

    /// Files scanned so far by the in-flight build (0 when idle).
    pub fn scanned_files(&self) -> usize {
        self.progress.load(Ordering::Relaxed)
    }
}
