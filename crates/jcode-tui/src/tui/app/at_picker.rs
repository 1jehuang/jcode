//! `@<path>` autocomplete picker — wraps `ffs-search` `FilePicker` for use
//! in the TUI input dropdown.
//!
//! ## Design
//!
//! `AtPicker` is a session-scoped singleton that:
//! 1. On `warm_up()`: spawns ffs's background filesystem scan + watcher rooted
//!    at the session working directory. Non-blocking; subsequent `search()`
//!    calls return empty until the scan completes.
//! 2. On `search(query, limit)`: runs a fuzzy match via
//!    [`ffs_search::FilePicker::fuzzy_search_mixed`] which returns both files
//!    and directories ranked by score + frecency. A query ending in `/`
//!    triggers ffs's "directories only" mode automatically — perfect for
//!    Claude-Code-style folder drill-in.
//! 3. On `record_selection(path)`: bumps the LMDB-backed frecency tracker so
//!    repeatedly-mentioned paths float to the top.
//!
//! Frecency state lives at `<cache_root>/<repo_hash>/{frecency,queries}` so
//! multiple jcode sessions on the same repo share rankings.
//!
//! ## Concurrency
//!
//! `AtPicker` is `Send + Sync` and cheap to clone (internals are Arc'd).
//! `search()` uses `try_read_for(5ms)` to avoid blocking the UI thread when
//! the background scan is mutating internal state.

use std::path::{Path, PathBuf};
use std::sync::Arc;
use std::sync::atomic::{AtomicBool, Ordering};
#[cfg(test)]
use std::time::Duration;

use ffs_search::{
    FfsMode, FilePicker, FilePickerOptions, FrecencyTracker, FuzzySearchOptions, MixedItemRef,
    PaginationArgs, QueryParser, QueryTracker, SharedFilePicker, SharedFrecency,
    SharedQueryTracker,
};

/// Maximum suggestions returned by `search()`. Matches Claude Code's
/// `MAX_SUGGESTIONS = 15`.
pub const AT_PICKER_MAX_SUGGESTIONS: usize = 15;

/// One suggestion item produced by `AtPicker::search`.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct AtSuggestion {
    /// Path relative to the picker's base directory, e.g. `src/main.rs`
    /// or `crates/jcode-core` (no trailing `/` even for dirs — caller
    /// decides whether to append it for display).
    pub display_path: String,
    pub is_directory: bool,
}

/// State of the underlying ffs background scan.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum PickerState {
    /// `warm_up()` has not been called yet.
    Uninitialized,
    /// Background scan in progress; queries return empty results.
    Scanning,
    /// Scan complete; queries are served.
    Ready,
}

/// Session-scoped wrapper around ffs `FilePicker`.
#[derive(Clone)]
pub struct AtPicker {
    inner: Arc<AtPickerInner>,
}

struct AtPickerInner {
    base_path: PathBuf,
    cache_dir: PathBuf,
    shared_picker: SharedFilePicker,
    shared_frecency: SharedFrecency,
    shared_query_tracker: SharedQueryTracker,
    /// Set on first successful `warm_up()` to avoid double-spawn.
    warm_up_started: AtomicBool,
}

impl AtPicker {
    /// Create a new picker for a given base path. `cache_root` is typically
    /// `~/.jcode/cache/ffs/`; a per-repo subdirectory will be created inside.
    ///
    /// This call is cheap — no filesystem scan happens until `warm_up()`.
    pub fn new(base_path: impl Into<PathBuf>, cache_root: impl Into<PathBuf>) -> Self {
        let base_path = base_path.into();
        let cache_root = cache_root.into();
        let cache_dir = cache_root.join(repo_cache_key(&base_path));

        Self {
            inner: Arc::new(AtPickerInner {
                base_path,
                cache_dir,
                shared_picker: SharedFilePicker::default(),
                // Frecency on; query-tracker noop (we don't need combo-boost
                // history for one-off mention completions).
                shared_frecency: SharedFrecency::default(),
                shared_query_tracker: SharedQueryTracker::noop(),
                warm_up_started: AtomicBool::new(false),
            }),
        }
    }

    /// Spawn the background scan + watcher. Idempotent: subsequent calls are
    /// no-ops. Returns `Ok(())` even if frecency init fails (frecency is
    /// optional — search still works without it).
    pub fn warm_up(&self) -> Result<(), AtPickerError> {
        if self.inner.warm_up_started.swap(true, Ordering::AcqRel) {
            return Ok(());
        }

        // Best-effort cache dir creation. Failure here only disables
        // frecency persistence; search still works.
        let frecency_init = std::fs::create_dir_all(&self.inner.cache_dir)
            .map_err(AtPickerError::CacheDir)
            .and_then(|()| {
                FrecencyTracker::open(self.inner.cache_dir.join("frecency"))
                    .map_err(|e| AtPickerError::Ffs(e.to_string()))
            })
            .and_then(|t| {
                self.inner
                    .shared_frecency
                    .init(t)
                    .map_err(|e| AtPickerError::Ffs(e.to_string()))
            });

        if let Err(err) = frecency_init {
            crate::logging::warn(&format!(
                "AtPicker: frecency init failed, continuing without it: {err}"
            ));
        }

        // QueryTracker is noop — init() is also a no-op, but we leave the
        // call here so future enablement is a one-line change.
        let _ = QueryTracker::open(self.inner.cache_dir.join("queries"))
            .ok()
            .map(|t| self.inner.shared_query_tracker.init(t));

        let opts = FilePickerOptions {
            base_path: self.inner.base_path.to_string_lossy().into_owned(),
            mode: FfsMode::Ai,
            enable_mmap_cache: false,
            enable_content_indexing: false,
            cache_budget: None,
            watch: true,
        };

        FilePicker::new_with_shared_state(
            self.inner.shared_picker.clone(),
            self.inner.shared_frecency.clone(),
            opts,
        )
        .map_err(|e| AtPickerError::Ffs(e.to_string()))?;

        Ok(())
    }

    /// Inspect the current picker state without blocking.
    pub fn state(&self) -> PickerState {
        if !self.inner.warm_up_started.load(Ordering::Acquire) {
            return PickerState::Uninitialized;
        }

        // Try a non-blocking read; if we can read it AND a picker is
        // installed AND scan is no longer active, we're Ready.
        let Ok(guard) = self.inner.shared_picker.read() else {
            return PickerState::Scanning;
        };
        match &*guard {
            None => PickerState::Scanning,
            Some(p) => {
                if p.is_scan_active() {
                    PickerState::Scanning
                } else {
                    PickerState::Ready
                }
            }
        }
    }

    /// Synchronously wait for the scan to finish. Used by tests; the TUI
    /// path uses `state()` polling instead.
    #[cfg(test)]
    pub fn wait_until_ready(&self, timeout: Duration) -> bool {
        self.inner.shared_picker.wait_for_scan(timeout)
    }

    /// Fuzzy-search the index. `query` should NOT include the leading `@`.
    /// A trailing `/` activates directory-only mode (drill-in semantics).
    ///
    /// Returns up to `limit` items, capped at `AT_PICKER_MAX_SUGGESTIONS`.
    /// Returns an empty `Vec` if the picker is not yet `Ready`.
    pub fn search(&self, query: &str, limit: usize) -> Vec<AtSuggestion> {
        if !matches!(self.state(), PickerState::Ready) {
            return Vec::new();
        }
        let limit = limit.min(AT_PICKER_MAX_SUGGESTIONS);

        let Ok(guard) = self.inner.shared_picker.read() else {
            return Vec::new();
        };
        let Some(picker) = guard.as_ref() else {
            return Vec::new();
        };

        let parser = QueryParser::default();
        let parsed = parser.parse(query);

        let qt_guard = self.inner.shared_query_tracker.read().ok();
        let qt = qt_guard.as_ref().and_then(|g| g.as_ref());

        let result = picker.fuzzy_search_mixed(
            &parsed,
            qt,
            FuzzySearchOptions {
                max_threads: 0, // auto
                pagination: PaginationArgs { offset: 0, limit },
                ..Default::default()
            },
        );

        let base = picker.base_path();
        result
            .items
            .into_iter()
            .filter_map(|item| match item {
                MixedItemRef::File(f) => {
                    let rel = f.relative_path(picker);
                    Some(AtSuggestion {
                        display_path: rel,
                        is_directory: false,
                    })
                }
                MixedItemRef::Dir(d) => {
                    let abs = d.absolute_path(picker, base);
                    let rel = abs
                        .strip_prefix(base)
                        .ok()
                        .and_then(|p| p.to_str())
                        .map(|s| s.to_string())
                        .unwrap_or_else(|| abs.to_string_lossy().into_owned());
                    if rel.is_empty() {
                        None
                    } else {
                        Some(AtSuggestion {
                            display_path: rel,
                            is_directory: true,
                        })
                    }
                }
            })
            .collect()
    }

    /// Record that the user selected a given absolute path. Bumps frecency
    /// score so the entry ranks higher next time.
    ///
    /// Best-effort: failures are logged and swallowed.
    #[allow(dead_code)] // Public API kept for upcoming TUI wiring.
    pub fn record_selection(&self, path: &Path) {
        let Ok(frecency_guard) = self.inner.shared_frecency.write() else {
            return;
        };
        let Some(tracker) = frecency_guard.as_ref() else {
            return;
        };
        if let Err(err) = tracker.track_access(path) {
            crate::logging::warn(&format!("AtPicker: frecency track_access failed: {err}"));
        }
    }

    /// Base path the picker is rooted at.
    #[allow(dead_code)] // Public API kept for upcoming TUI wiring.
    pub fn base_path(&self) -> &Path {
        &self.inner.base_path
    }
}

impl std::fmt::Debug for AtPicker {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("AtPicker")
            .field("base_path", &self.inner.base_path)
            .field("state", &self.state())
            .finish()
    }
}

#[derive(Debug, thiserror::Error)]
pub enum AtPickerError {
    #[error("could not create cache directory: {0}")]
    CacheDir(std::io::Error),
    #[error("ffs error: {0}")]
    Ffs(String),
}

/// Three-state slot for the lazy-initialized picker stored on `App`.
///
/// Lifecycle:
///   Pending  → first @-keystroke triggers init via [`AtPickerSlot::ensure`]
///   Initialized(p) → subsequent calls return `Some(p.clone())`
///   Failed   → stop retrying (e.g. working_dir missing or invalid)
#[derive(Debug, Default)]
pub enum AtPickerSlot {
    #[default]
    Pending,
    Initialized(AtPicker),
    Failed,
}

impl AtPickerSlot {
    /// Get a handle to the picker, lazy-initializing if necessary.
    /// Returns `None` if init has failed or `working_dir` is missing/invalid.
    pub fn ensure(&mut self, working_dir: Option<&str>) -> Option<AtPicker> {
        match self {
            AtPickerSlot::Initialized(p) => return Some(p.clone()),
            AtPickerSlot::Failed => return None,
            AtPickerSlot::Pending => {}
        }
        let cwd = match working_dir {
            Some(s) => Path::new(s),
            None => {
                *self = AtPickerSlot::Failed;
                return None;
            }
        };
        if !cwd.is_dir() {
            *self = AtPickerSlot::Failed;
            return None;
        }
        let cache_root = jcode_ffs_cache_root();
        let picker = AtPicker::new(cwd, cache_root);
        match picker.warm_up() {
            Ok(()) => {
                *self = AtPickerSlot::Initialized(picker.clone());
                Some(picker)
            }
            Err(err) => {
                crate::logging::warn(&format!(
                    "AtPicker: warm_up failed, disabling @ autocomplete: {err}"
                ));
                *self = AtPickerSlot::Failed;
                None
            }
        }
    }

    /// Returns `Some(AtPicker)` only if already initialized; never starts a
    /// new init. Used when we need a non-blocking peek.
    #[allow(dead_code)] // Public API kept for upcoming TUI wiring.
    pub fn get(&self) -> Option<AtPicker> {
        match self {
            AtPickerSlot::Initialized(p) => Some(p.clone()),
            _ => None,
        }
    }
}

/// Resolve `~/.jcode/cache/ffs/`, creating intermediate dirs as needed.
fn jcode_ffs_cache_root() -> PathBuf {
    let base = crate::storage::jcode_dir().unwrap_or_else(|_| {
        dirs::home_dir()
            .unwrap_or_else(|| PathBuf::from("."))
            .join(".jcode")
    });
    base.join("cache").join("ffs")
}

/// Stable per-repo key for the cache subdirectory. Just the basename plus
/// a short hash of the absolute path so two repos with the same name don't
/// collide.
fn repo_cache_key(path: &Path) -> String {
    use std::hash::{Hash, Hasher};
    let basename = path
        .file_name()
        .and_then(|s| s.to_str())
        .unwrap_or("repo")
        .replace(
            |c: char| !c.is_ascii_alphanumeric() && c != '-' && c != '_',
            "_",
        );
    let mut hasher = std::collections::hash_map::DefaultHasher::new();
    path.hash(&mut hasher);
    format!("{}-{:016x}", basename, hasher.finish())
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::fs;
    use tempfile::TempDir;

    fn setup_repo() -> (TempDir, TempDir) {
        let repo = TempDir::new().unwrap();
        let cache = TempDir::new().unwrap();
        fs::write(repo.path().join("main.rs"), "fn main() {}").unwrap();
        fs::write(repo.path().join("Cargo.toml"), "[package]").unwrap();
        fs::create_dir_all(repo.path().join("src")).unwrap();
        fs::write(repo.path().join("src/lib.rs"), "// lib").unwrap();
        fs::create_dir_all(repo.path().join("docs")).unwrap();
        fs::write(repo.path().join("docs/README.md"), "# docs").unwrap();
        (repo, cache)
    }

    #[test]
    fn state_uninitialized_before_warm_up() {
        let (repo, cache) = setup_repo();
        let picker = AtPicker::new(repo.path(), cache.path());
        assert_eq!(picker.state(), PickerState::Uninitialized);
        assert!(picker.search("main", 10).is_empty());
    }

    #[test]
    fn warm_up_eventually_becomes_ready() {
        let (repo, cache) = setup_repo();
        let picker = AtPicker::new(repo.path(), cache.path());
        picker.warm_up().expect("warm up");
        assert!(
            picker.wait_until_ready(Duration::from_secs(5)),
            "scan did not complete within 5s"
        );
        assert_eq!(picker.state(), PickerState::Ready);
    }

    #[test]
    fn search_returns_files_and_dirs() {
        let (repo, cache) = setup_repo();
        let picker = AtPicker::new(repo.path(), cache.path());
        picker.warm_up().expect("warm up");
        assert!(picker.wait_until_ready(Duration::from_secs(5)));

        let results = picker.search("", 15);
        assert!(!results.is_empty(), "expected non-empty initial listing");

        // We expect at least our planted files + dirs to surface.
        let has_main = results
            .iter()
            .any(|s| !s.is_directory && s.display_path.contains("main.rs"));
        let has_dir = results.iter().any(|s| s.is_directory);
        assert!(has_main, "expected main.rs in results: {:?}", results);
        assert!(has_dir, "expected at least one directory: {:?}", results);
    }

    #[test]
    fn search_filters_by_query() {
        let (repo, cache) = setup_repo();
        let picker = AtPicker::new(repo.path(), cache.path());
        picker.warm_up().expect("warm up");
        assert!(picker.wait_until_ready(Duration::from_secs(5)));

        let results = picker.search("Cargo", 15);
        assert!(
            results
                .iter()
                .any(|s| s.display_path.contains("Cargo.toml")),
            "expected Cargo.toml: {:?}",
            results
        );
    }

    #[test]
    fn search_with_trailing_slash_returns_dirs_only() {
        let (repo, cache) = setup_repo();
        let picker = AtPicker::new(repo.path(), cache.path());
        picker.warm_up().expect("warm up");
        assert!(picker.wait_until_ready(Duration::from_secs(5)));

        let results = picker.search("/", 15);
        // ffs `dirs_only` mode: every result must be a directory.
        for s in &results {
            assert!(
                s.is_directory,
                "expected dir-only mode for trailing-slash query, got file: {:?}",
                s
            );
        }
    }

    #[test]
    fn warm_up_idempotent() {
        let (repo, cache) = setup_repo();
        let picker = AtPicker::new(repo.path(), cache.path());
        picker.warm_up().expect("first warm up");
        picker.warm_up().expect("second warm up no-op");
    }

    #[test]
    fn multi_picker_same_dir_no_deadlock() {
        // Risk: two AtPickers on the same cwd shouldn't deadlock on LMDB.
        // We give them DIFFERENT cache subdirs (per-PID-style isolation)
        // because LMDB only supports one writer per environment.
        let (repo, _cache) = setup_repo();
        let cache1 = TempDir::new().unwrap();
        let cache2 = TempDir::new().unwrap();

        let p1 = AtPicker::new(repo.path(), cache1.path());
        let p2 = AtPicker::new(repo.path(), cache2.path());
        p1.warm_up().expect("p1 warm");
        p2.warm_up().expect("p2 warm");

        assert!(p1.wait_until_ready(Duration::from_secs(5)));
        assert!(p2.wait_until_ready(Duration::from_secs(5)));

        let r1 = p1.search("main", 10);
        let r2 = p2.search("main", 10);
        assert!(!r1.is_empty());
        assert!(!r2.is_empty());
    }

    #[test]
    fn repo_cache_key_is_stable_and_unique() {
        let a = repo_cache_key(Path::new("/tmp/foo"));
        let b = repo_cache_key(Path::new("/tmp/foo"));
        let c = repo_cache_key(Path::new("/tmp/bar"));
        assert_eq!(a, b, "same path → same key");
        assert_ne!(a, c, "different path → different key");
        assert!(a.starts_with("foo-"));
        assert!(c.starts_with("bar-"));
    }
}
