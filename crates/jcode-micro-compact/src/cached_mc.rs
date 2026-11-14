use serde::{Deserialize, Serialize};

/// Cached micro-compact path that records which content blocks were compacted
/// in a previous turn, enabling incremental MC without re-analyzing the
/// entire message history.
///
/// The cache key is derived from the message structure (number of messages,
/// last message ID/hash) so it invalidates naturally when the conversation
/// changes.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct CachedMcPath {
    /// Opaque cache key derived from conversation state.
    pub cache_key: String,
    /// Indices of content blocks that were compacted in the cached run.
    pub compacted_indices: Vec<usize>,
    /// Estimated tokens saved by the cached compaction.
    pub tokens_saved: usize,
    /// Number of messages in the conversation when the cache was built.
    pub message_count: usize,
}

impl CachedMcPath {
    /// Build a cache key from conversation metadata.
    ///
    /// The key encodes message count and a hash of the last message so the
    /// cache naturally invalidates when new messages arrive.
    pub fn build_cache_key(message_count: usize, last_message_hash: &str) -> String {
        format!("mc:{}:{}", message_count, last_message_hash)
    }

    /// Check whether this cache entry is still valid for the given state.
    pub fn is_valid(&self, message_count: usize, last_message_hash: &str) -> bool {
        let expected = Self::build_cache_key(message_count, last_message_hash);
        self.cache_key == expected
    }
}

/// Configuration for the cached MC path behavior.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct CachedMcConfig {
    /// Maximum number of cache entries to retain.
    pub max_entries: usize,
    /// Whether caching is enabled.
    pub enabled: bool,
}

impl Default for CachedMcConfig {
    fn default() -> Self {
        Self {
            max_entries: 16,
            enabled: true,
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_cache_key_determinism() {
        let k1 = CachedMcPath::build_cache_key(5, "abc123");
        let k2 = CachedMcPath::build_cache_key(5, "abc123");
        assert_eq!(k1, k2);
    }

    #[test]
    fn test_cache_key_differs_on_count() {
        let k1 = CachedMcPath::build_cache_key(5, "abc123");
        let k2 = CachedMcPath::build_cache_key(6, "abc123");
        assert_ne!(k1, k2);
    }

    #[test]
    fn test_cache_key_differs_on_hash() {
        let k1 = CachedMcPath::build_cache_key(5, "abc123");
        let k2 = CachedMcPath::build_cache_key(5, "def456");
        assert_ne!(k1, k2);
    }

    #[test]
    fn test_validity_check() {
        let cache = CachedMcPath {
            cache_key: CachedMcPath::build_cache_key(10, "hash"),
            compacted_indices: vec![0, 3],
            tokens_saved: 500,
            message_count: 10,
        };
        assert!(cache.is_valid(10, "hash"));
        assert!(!cache.is_valid(11, "hash"));
        assert!(!cache.is_valid(10, "other"));
    }

    #[test]
    fn test_default_config() {
        let config = CachedMcConfig::default();
        assert!(config.enabled);
        assert_eq!(config.max_entries, 16);
    }
}
