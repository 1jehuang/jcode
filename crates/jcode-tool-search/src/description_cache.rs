//! Memoized description loading for deferred tool discovery.
//!
//! Tool descriptions are expensive to load (they live in prompt context or
//! must be fetched from MCP servers). This module provides a cache that
//! loads each description at most once and serves repeated lookups from memory.
//!
//! The cache is intentionally synchronous-safe (no `tokio::Mutex` needed)
//! because tool description loading is typically done lazily within a single
//! async context, and the cache only grows monotonically.

use std::collections::HashMap;

/// A tool description that has been loaded (or is pending load).
#[derive(Clone, Debug)]
pub enum DescriptionState {
    /// Description has been loaded and cached.
    Loaded(String),
    /// Description has not been loaded yet; the entry exists but is empty.
    Pending,
}

/// Memoized cache for tool descriptions keyed by raw tool name.
///
/// Callers insert `Pending` entries when they first discover a tool name,
/// then call [`DescriptionCache::load`] (or [`DescriptionCache::get_or_load`])
/// to fill in the actual description text. Subsequent lookups return the
/// cached value without re-fetching.
#[derive(Clone, Debug, Default)]
pub struct DescriptionCache {
    inner: HashMap<String, DescriptionState>,
}

impl DescriptionCache {
    /// Create an empty cache.
    pub fn new() -> Self {
        Self::default()
    }

    /// Register a tool name with a pending (not yet loaded) description.
    pub fn register(&mut self, tool_name: impl Into<String>) {
        let name = tool_name.into();
        self.inner.entry(name).or_insert(DescriptionState::Pending);
    }

    /// Register multiple tool names at once.
    pub fn register_all(&mut self, names: impl IntoIterator<Item = String>) {
        for name in names {
            self.register(name);
        }
    }

    /// Store a loaded description for a tool.
    pub fn store(&mut self, tool_name: impl Into<String>, description: impl Into<String>) {
        self.inner.insert(
            tool_name.into(),
            DescriptionState::Loaded(description.into()),
        );
    }

    /// Get a cached description, if it has been loaded.
    pub fn get(&self, tool_name: &str) -> Option<&str> {
        match self.inner.get(tool_name) {
            Some(DescriptionState::Loaded(desc)) => Some(desc.as_str()),
            _ => None,
        }
    }

    /// Check whether a tool name is registered (loaded or pending).
    pub fn contains(&self, tool_name: &str) -> bool {
        self.inner.contains_key(tool_name)
    }

    /// Check whether a tool's description has been loaded.
    pub fn is_loaded(&self, tool_name: &str) -> bool {
        matches!(self.inner.get(tool_name), Some(DescriptionState::Loaded(_)))
    }

    /// Get-or-load: return cached description, or call the loader and cache.
    ///
    /// The `loader` is called only if the description is `Pending` or absent.
    pub fn get_or_load(
        &mut self,
        tool_name: &str,
        loader: impl FnOnce() -> Option<String>,
    ) -> Option<&str> {
        let state = self
            .inner
            .entry(tool_name.to_string())
            .or_insert(DescriptionState::Pending);
        match state {
            DescriptionState::Loaded(desc) => Some(desc.as_str()),
            DescriptionState::Pending => {
                if let Some(desc) = loader() {
                    *state = DescriptionState::Loaded(desc);
                    match state {
                        DescriptionState::Loaded(d) => Some(d.as_str()),
                        _ => unreachable!(),
                    }
                } else {
                    // Mark as loaded with empty string so we don't retry.
                    *state = DescriptionState::Loaded(String::new());
                    None
                }
            }
        }
    }

    /// Return all tool names registered in the cache.
    pub fn tool_names(&self) -> impl Iterator<Item = &str> {
        self.inner.keys().map(String::as_str)
    }

    /// Return the number of registered tools (loaded + pending).
    pub fn len(&self) -> usize {
        self.inner.len()
    }

    /// Return `true` if the cache is empty.
    pub fn is_empty(&self) -> bool {
        self.inner.is_empty()
    }

    /// Return the number of descriptions that have been loaded.
    pub fn loaded_count(&self) -> usize {
        self.inner
            .values()
            .filter(|s| matches!(s, DescriptionState::Loaded(d) if !d.is_empty()))
            .count()
    }

    /// Evict all pending (unloaded) entries, keeping only loaded ones.
    pub fn evict_pending(&mut self) {
        self.inner
            .retain(|_, state| matches!(state, DescriptionState::Loaded(d) if !d.is_empty()));
    }

    /// Clear the entire cache.
    pub fn clear(&mut self) {
        self.inner.clear();
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn register_and_get_pending() {
        let mut cache = DescriptionCache::new();
        cache.register("ToolA");
        assert!(cache.contains("ToolA"));
        assert!(!cache.is_loaded("ToolA"));
        assert!(cache.get("ToolA").is_none());
    }

    #[test]
    fn store_and_retrieve() {
        let mut cache = DescriptionCache::new();
        cache.store("ToolA", "Does something cool");
        assert!(cache.is_loaded("ToolA"));
        assert_eq!(cache.get("ToolA"), Some("Does something cool"));
    }

    #[test]
    fn get_or_load_caches() {
        let mut cache = DescriptionCache::new();
        let mut call_count = 0;

        let result = cache.get_or_load("ToolA", || {
            call_count += 1;
            Some("Description".into())
        });
        assert_eq!(result, Some("Description"));
        assert_eq!(call_count, 1);

        // Second call should not invoke the loader.
        let result = cache.get_or_load("ToolA", || {
            call_count += 1;
            Some("Updated".into())
        });
        assert_eq!(result, Some("Description"));
        assert_eq!(call_count, 1);
    }

    #[test]
    fn get_or_load_pending_fails() {
        let mut cache = DescriptionCache::new();
        let result = cache.get_or_load("ToolA", || None);
        assert!(result.is_none());
        // Should be marked loaded (with empty string) so loader isn't retried.
        assert!(cache.is_loaded("ToolA"));
        assert_eq!(cache.get("ToolA"), Some(""));
    }

    #[test]
    fn register_all() {
        let mut cache = DescriptionCache::new();
        cache.register_all(vec!["A".into(), "B".into(), "C".into()]);
        assert_eq!(cache.len(), 3);
        assert!(cache.contains("A"));
        assert!(cache.contains("B"));
        assert!(cache.contains("C"));
    }

    #[test]
    fn loaded_count() {
        let mut cache = DescriptionCache::new();
        cache.register("A");
        cache.store("B", "desc B");
        cache.store("C", "desc C");
        assert_eq!(cache.loaded_count(), 2);
    }

    #[test]
    fn evict_pending() {
        let mut cache = DescriptionCache::new();
        cache.register("A"); // pending
        cache.store("B", "desc B"); // loaded
        cache.evict_pending();
        assert!(!cache.contains("A"));
        assert!(cache.contains("B"));
        assert_eq!(cache.len(), 1);
    }

    #[test]
    fn clear() {
        let mut cache = DescriptionCache::new();
        cache.store("A", "desc");
        cache.clear();
        assert!(cache.is_empty());
    }
}
