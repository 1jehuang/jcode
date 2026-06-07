//! Service handle that owns the server's file-touch tracking state.
//!
//! Historically the [`Server`](super::Server) struct held two raw
//! `Arc<RwLock<..>>` maps for file-touch tracking and every call site reached
//! directly into them. This service consolidates that state behind
//! intention-revealing methods so the rest of the server no longer needs to
//! know the internal map shapes or locking order.
//!
//! The two indexes are kept in sync:
//! * forward: `path -> chronological accesses`
//! * reverse: `session_id -> set of touched paths`

use super::FileAccess;
use std::collections::{HashMap, HashSet};
use std::path::{Path, PathBuf};
use std::sync::Arc;
use std::time::{Duration, Instant};
use tokio::sync::RwLock;

/// Shared ownership of the server's file-touch tracking indexes.
///
/// Cloning is cheap: every clone shares the same underlying `Arc`-backed maps,
/// matching the previous behavior where the raw `Arc<RwLock<..>>` fields were
/// cloned and passed around.
#[derive(Clone)]
pub(crate) struct FileTouchService {
    /// Forward index: path -> list of accesses (chronological order).
    touches: Arc<RwLock<HashMap<PathBuf, Vec<FileAccess>>>>,
    /// Reverse index: session_id -> set of paths the session has touched.
    by_session: Arc<RwLock<HashMap<String, HashSet<PathBuf>>>>,
}

impl FileTouchService {
    /// Create an empty file-touch tracker.
    pub(crate) fn new() -> Self {
        Self {
            touches: Arc::new(RwLock::new(HashMap::new())),
            by_session: Arc::new(RwLock::new(HashMap::new())),
        }
    }

    /// Record a single file access, updating both the forward and reverse
    /// indexes. The forward index is updated first (and its lock released)
    /// before the reverse index, preserving the original locking order.
    pub(crate) async fn record_touch(&self, path: PathBuf, access: FileAccess) {
        let session_id = access.session_id.clone();
        {
            let mut touches = self.touches.write().await;
            touches
                .entry(path.clone())
                .or_insert_with(Vec::new)
                .push(access);
        }
        {
            let mut by_session = self.by_session.write().await;
            by_session.entry(session_id).or_default().insert(path);
        }
    }

    /// Cloned snapshot of all accesses recorded for `path`, or `None` if the
    /// path has not been touched. Callers rely on the `Some`/`None` distinction
    /// (e.g. for logging "no touches yet" vs computing peer touches).
    pub(crate) async fn accesses_for_path(&self, path: &Path) -> Option<Vec<FileAccess>> {
        self.touches.read().await.get(path).cloned()
    }

    /// Sorted, display-formatted list of the distinct files a session has
    /// touched (empty if the session has touched nothing).
    pub(crate) async fn sorted_file_strings_for_session(&self, session_id: &str) -> Vec<String> {
        let by_session = self.by_session.read().await;
        let mut files: Vec<String> = by_session
            .get(session_id)
            .into_iter()
            .flat_map(|paths| paths.iter())
            .map(|path| path.display().to_string())
            .collect();
        files.sort();
        files
    }

    /// Cloned snapshot of the entire forward (`path -> accesses`) index.
    ///
    /// Used by read-only reporting paths (debug commands, memory accounting)
    /// that need to iterate the whole map.
    pub(crate) async fn snapshot(&self) -> HashMap<PathBuf, Vec<FileAccess>> {
        self.touches.read().await.clone()
    }

    /// Cloned snapshot of the reverse (`session_id -> paths`) index.
    pub(crate) async fn reverse_snapshot(&self) -> HashMap<String, HashSet<PathBuf>> {
        self.by_session.read().await.clone()
    }

    /// Remove every touch recorded for a session from both indexes.
    ///
    /// Uses the reverse index to bound the forward-index work to only the
    /// paths the session actually touched, falling back to a full scan if the
    /// reverse entry is missing.
    pub(crate) async fn clear_session(&self, session_id: &str) {
        let touched_paths = {
            let mut reverse = self.by_session.write().await;
            reverse.remove(session_id)
        };

        let mut touches = self.touches.write().await;
        if let Some(paths) = touched_paths {
            for path in paths {
                let mut remove_path = false;
                if let Some(accesses) = touches.get_mut(&path) {
                    accesses.retain(|access| access.session_id != session_id);
                    remove_path = accesses.is_empty();
                }
                if remove_path {
                    touches.remove(&path);
                }
            }
            return;
        }

        touches.retain(|_, accesses| {
            accesses.retain(|access| access.session_id != session_id);
            !accesses.is_empty()
        });
    }

    /// Drop accesses older than `max_age` and rebuild the reverse index from the
    /// surviving forward entries.
    pub(crate) async fn expire_older_than(&self, max_age: Duration) {
        let mut touches = self.touches.write().await;
        let now = Instant::now();
        touches.retain(|_, accesses| {
            accesses.retain(|access| now.duration_since(access.timestamp) < max_age);
            !accesses.is_empty()
        });
        let mut rebuilt_reverse_index: HashMap<String, HashSet<PathBuf>> = HashMap::new();
        for (path, accesses) in touches.iter() {
            for access in accesses {
                rebuilt_reverse_index
                    .entry(access.session_id.clone())
                    .or_default()
                    .insert(path.clone());
            }
        }
        drop(touches);
        *self.by_session.write().await = rebuilt_reverse_index;
    }
}
