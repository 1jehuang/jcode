//! Per-path locks for file-editing tools.
//!
//! `batch` runs subcalls concurrently, so two edits to the same file could
//! both read the original and then race their writes, losing one edit and
//! leaving a torn tail. Every read-modify-write tool holds the lock for each
//! path it touches for the whole read-modify-write cycle.

use std::collections::HashMap;
use std::path::{Path, PathBuf};
use std::sync::{Arc, Mutex, OnceLock};
use tokio::sync::OwnedMutexGuard;

type LockMap = Mutex<HashMap<PathBuf, Arc<tokio::sync::Mutex<()>>>>;

fn locks() -> &'static LockMap {
    static LOCKS: OnceLock<LockMap> = OnceLock::new();
    LOCKS.get_or_init(Default::default)
}

fn key(path: &Path) -> PathBuf {
    std::fs::canonicalize(path).unwrap_or_else(|_| path.to_path_buf())
}

/// Lock one path for a read-modify-write cycle.
pub(crate) async fn lock(path: &Path) -> OwnedMutexGuard<()> {
    let mutex = {
        let mut map = locks().lock().unwrap_or_else(|error| error.into_inner());
        // Drop entries nobody holds so the map cannot grow without bound.
        map.retain(|_, mutex| Arc::strong_count(mutex) > 1);
        map.entry(key(path)).or_default().clone()
    };
    mutex.lock_owned().await
}

/// Lock several paths in a stable order, so overlapping multi-file edits
/// cannot deadlock each other.
pub(crate) async fn lock_all(paths: impl IntoIterator<Item = PathBuf>) -> Vec<OwnedMutexGuard<()>> {
    let mut keys: Vec<PathBuf> = paths.into_iter().map(|path| key(&path)).collect();
    keys.sort();
    keys.dedup();
    let mut guards = Vec::with_capacity(keys.len());
    for path in keys {
        guards.push(lock(&path).await);
    }
    guards
}

#[cfg(test)]
mod tests {
    use super::*;

    #[tokio::test]
    async fn concurrent_edits_to_one_file_do_not_lose_updates() {
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("f.txt");
        std::fs::write(&path, "").unwrap();
        let tasks = (0..32).map(|index| {
            let path = path.clone();
            tokio::spawn(async move {
                let _guard = lock(&path).await;
                let mut content = tokio::fs::read_to_string(&path).await.unwrap();
                tokio::task::yield_now().await;
                content.push_str(&format!("{index}\n"));
                tokio::fs::write(&path, content).await.unwrap();
            })
        });
        for task in tasks.collect::<Vec<_>>() {
            task.await.unwrap();
        }
        assert_eq!(std::fs::read_to_string(&path).unwrap().lines().count(), 32);
    }
}
