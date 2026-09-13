//! Integration layer for `jcode-tool-search`.
//!
//! Provides a `ToolSearchIndex` that wraps `DescriptionCache` and exposes
//! keyword search over registered tool names. The existing `DiscoverToolsTool`
//! and the agent's tool-lookup paths can call into this module without touching
//! the raw crate directly.

use jcode_tool_search::{
    DescriptionCache, ToolEntry, parse_tool_name,
    search_tools as keyword_search, SearchResult,
};
use std::sync::{Arc, RwLock};

/// Thread-safe search index backed by a [`DescriptionCache`].
///
/// Register tool names when they are discovered, store their descriptions
/// lazily, and search against the cache at any time.
#[derive(Clone)]
pub struct ToolSearchIndex {
    inner: Arc<RwLock<DescriptionCache>>,
}

impl Default for ToolSearchIndex {
    fn default() -> Self {
        Self::new()
    }
}

impl ToolSearchIndex {
    /// Create an empty search index.
    pub fn new() -> Self {
        Self {
            inner: Arc::new(RwLock::new(DescriptionCache::new())),
        }
    }

    /// Register a batch of tool names (without descriptions yet).
    pub fn register_names(&self, names: impl IntoIterator<Item = String>) {
        if let Ok(mut cache) = self.inner.write() {
            cache.register_all(names);
        }
    }

    /// Register a single tool name.
    pub fn register_name(&self, name: impl Into<String>) {
        if let Ok(mut cache) = self.inner.write() {
            cache.register(name);
        }
    }

    /// Store a loaded description for a tool.
    pub fn store_description(&self, tool_name: impl Into<String>, description: impl Into<String>) {
        if let Ok(mut cache) = self.inner.write() {
            cache.store(tool_name, description);
        }
    }

    /// Get-or-load a description using the provided loader function.
    pub fn get_or_load(
        &self,
        tool_name: &str,
        loader: impl FnOnce() -> Option<String>,
    ) -> Option<String> {
        self.inner
            .write()
            .ok()
            .and_then(|mut cache| cache.get_or_load(tool_name, loader).map(String::from))
    }

    /// Search registered tools against a query string.
    ///
    /// Builds [`ToolEntry`] objects from the cache (using cached descriptions
    /// where available) and delegates to the keyword search scorer.
    pub fn search(&self, query: &str) -> Vec<SearchResult> {
        let cache = match self.inner.read() {
            Ok(c) => c,
            Err(_) => return Vec::new(),
        };

        let tools: Vec<ToolEntry> = cache
            .tool_names()
            .map(|name| {
                let desc = cache.get(name).map(String::from);
                ToolEntry {
                    name: name.to_string(),
                    description: desc,
                }
            })
            .collect();

        keyword_search(query, &tools)
    }

    /// Number of registered tools (loaded + pending).
    pub fn len(&self) -> usize {
        self.inner.read().map(|c| c.len()).unwrap_or(0)
    }

    /// Whether the index is empty.
    pub fn is_empty(&self) -> bool {
        self.len() == 0
    }

    /// Number of descriptions fully loaded.
    pub fn loaded_count(&self) -> usize {
        self.inner.read().map(|c| c.loaded_count()).unwrap_or(0)
    }

    /// Check if a specific tool name is registered.
    pub fn contains(&self, tool_name: &str) -> bool {
        self.inner.read().map(|c| c.contains(tool_name)).unwrap_or(false)
    }

    /// Remove all pending (unloaded) entries, keeping only loaded ones.
    pub fn evict_pending(&self) {
        if let Ok(mut cache) = self.inner.write() {
            cache.evict_pending();
        }
    }
}

/// Convenience: parse a raw tool name into searchable tokens.
pub fn parse_tool_name_tokens(name: &str) -> Option<jcode_tool_search::ParsedToolName> {
    parse_tool_name(name)
}
