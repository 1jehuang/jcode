//! Best-effort catalog persistence with explicit diagnostics for failed I/O.

use super::*;

pub(super) fn best_effort_cache_path() -> Option<std::path::PathBuf> {
    match cache_path() {
        Ok(path) => Some(path),
        Err(error) => {
            crate::logging::warn(&format!(
                "ChatGPT browser catalog cache path unavailable, using memory only: {error:#}"
            ));
            None
        }
    }
}

pub(super) fn read_cached_snapshot(path: &std::path::Path) -> Option<Snapshot> {
    match crate::storage::read_json(path) {
        Ok(snapshot) => Some(snapshot),
        // A missing cache is normal before the first successful discovery.
        Err(error)
            if error
                .downcast_ref::<std::io::Error>()
                .is_some_and(|error| error.kind() == std::io::ErrorKind::NotFound) =>
        {
            None
        }
        Err(error) => {
            crate::logging::warn(&format!(
                "Failed to load ChatGPT browser catalog from {}: {error:#}",
                path.display()
            ));
            None
        }
    }
}

pub(super) fn load_cache() -> Cache {
    let path = best_effort_cache_path();
    let mut cache = CACHE.lock().unwrap_or_else(|e| e.into_inner());
    if cache.path != path {
        cache.snapshot = path
            .as_deref()
            .and_then(read_cached_snapshot)
            .unwrap_or_else(Snapshot::default);
        cache.path = path.clone();
    }
    Cache {
        path,
        snapshot: cache.snapshot.clone(),
    }
}

pub(super) fn store_cache(snapshot: Snapshot) {
    let path = best_effort_cache_path();
    if let Some(path) = &path
        && let Err(error) = crate::storage::write_json(path, &snapshot)
    {
        crate::logging::warn(&format!(
            "Failed to persist ChatGPT browser catalog: {error:#}"
        ));
    }
    *CACHE.lock().unwrap_or_else(|e| e.into_inner()) = Cache { path, snapshot };
}

#[cfg(test)]
#[path = "tests/chatgpt_web_cache.rs"]
mod tests;
