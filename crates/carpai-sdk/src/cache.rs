//! Cache management for CarpAI SDK

use crate::error::{CarpAiError, Result};
use crate::types::{CompletionRequest, CompletionResponse, RequestId};
use dashmap::DashMap;
use lru::LruCache;
use serde::{Deserialize, Serialize};
use std::num::NonZeroUsize;
use std::sync::Arc;
use std::time::{Duration, Instant};

/// Cache configuration
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct CacheConfig {
    /// Enable caching
    #[serde(default = "default_true")]
    pub enabled: bool,

    /// Maximum cache size (number of entries)
    #[serde(default = "default_cache_size")]
    pub max_size: usize,

    /// Time-to-live for cached entries in seconds
    #[serde(default = "default_ttl")]
    pub ttl_secs: u64,

    /// Enable cache persistence to disk
    #[serde(default)]
    pub persist_to_disk: bool,

    /// Directory for cache persistence
    pub cache_dir: Option<String>,
}

fn default_true() -> bool { true }
fn default_cache_size() -> usize { 1000 }
fn default_ttl() -> u64 { 3600 } // 1 hour

impl Default for CacheConfig {
    fn default() -> Self {
        Self {
            enabled: true,
            max_size: default_cache_size(),
            ttl_secs: default_ttl(),
            persist_to_disk: false,
            cache_dir: None,
        }
    }
}

/// Cached response with metadata
#[derive(Debug, Clone)]
#[allow(dead_code)]
struct CachedResponse {
    /// The cached response data
    response: CompletionResponse,

    /// When this entry was created
    created_at: Instant,

    /// When this entry expires
    expires_at: Instant,

    /// Number of times this entry was accessed
    access_count: u64,

    /// Original request hash (for validation)
    request_hash: u64,
}

/// Cache manager implementation
pub struct CacheManager {
    config: CacheConfig,
    cache: Arc<DashMap<RequestId, CachedResponse>>,
    lru_index: Arc<std::sync::Mutex<LruCache<RequestId, ()>>>,
}

impl CacheManager {
    /// Create a new cache manager with the given configuration
    #[allow(clippy::result_large_err)]
    pub fn new(config: CacheConfig) -> Result<Self> {
        if !config.enabled {
            return Ok(Self {
                config,
                cache: Arc::new(DashMap::new()),
                lru_index: Arc::new(std::sync::Mutex::new(
                    LruCache::new(NonZeroUsize::new(1).unwrap()),
                )),
            });
        }

        let size = NonZeroUsize::new(config.max_size).ok_or_else(|| {
            CarpAiError::Cache {
                message: "Cache size must be greater than 0".to_string(),
                source: None,
            }
        })?;

        Ok(Self {
            config,
            cache: Arc::new(DashMap::new()),
            lru_index: Arc::new(std::sync::Mutex::new(LruCache::new(size))),
        })
    }

    /// Generate a cache key from a completion request
    fn generate_key(&self, request: &CompletionRequest) -> RequestId {
        use std::collections::hash_map::DefaultHasher;
        use std::hash::{Hash, Hasher};

        let mut hasher = DefaultHasher::new();
        request.prompt.hash(&mut hasher);
        if let Some(ref model) = request.model {
            model.hash(&mut hasher);
        }
        // f64 doesn't implement Hash, so we convert to bits
        if let Some(temp) = request.temperature {
            temp.to_bits().hash(&mut hasher);
        }
        if let Some(tokens) = request.max_tokens {
            tokens.hash(&mut hasher);
        }

        RequestId(format!("{:x}", hasher.finish()))
    }

    /// Try to get a cached response for the given request
    ///
    /// Returns `None` if cache is disabled, no entry exists, or entry has expired.
    ///
    /// # Examples
    ///
    /// ```
    /// use carpai_sdk::{CacheManager, CacheConfig, CompletionRequest};
    ///
    /// let config = CacheConfig::default();
    /// let cache = CacheManager::new(config).unwrap();
    /// let request = CompletionRequest {
    ///     prompt: "Test prompt".to_string(),
    ///     ..Default::default()
    /// };
    ///
    /// // Cache miss returns None
    /// assert!(cache.get(&request).is_none());
    /// ```
    pub fn get(&self, request: &CompletionRequest) -> Option<CompletionResponse> {
        if !self.config.enabled {
            return None;
        }

        let key = self.generate_key(request);

        // Check if entry exists and is not expired
        if let Some(entry) = self.cache.get(&key) {
            if Instant::now() < entry.expires_at {
                // Update access count (we need to modify, so remove and re-insert)
                let mut updated = entry.clone();
                updated.access_count += 1;

                // Update LRU index
                if let Ok(mut lru) = self.lru_index.lock() {
                    lru.push(key.clone(), ());
                }

                Some(updated.response)
            } else {
                // Entry expired, remove it
                self.cache.remove(&key);
                None
            }
        } else {
            None
        }
    }

    /// Store a response in the cache
    ///
    /// Automatically evicts oldest entries if cache is at capacity.
    /// Expired entries are lazily removed on access.
    ///
    /// # Errors
    ///
    /// Returns error if cache storage fails (e.g., disk full if persist enabled).
    ///
    /// # Examples
    ///
    /// ```
    /// use carpai_sdk::{CacheManager, CacheConfig, CompletionRequest, CompletionResponse, RequestId, TokenUsage};
    ///
    /// let config = CacheConfig::default();
    /// let cache = CacheManager::new(config).unwrap();
    ///
    /// let request = CompletionRequest {
    ///     prompt: "What is Rust?".to_string(),
    ///     ..Default::default()
    /// };
    ///
    /// let response = CompletionResponse {
    ///     text: "Rust is a systems programming language.".to_string(),
    ///     request_id: RequestId::new(),
    ///     session_id: None,
    ///     model: "test".to_string(),
    ///     usage: TokenUsage { prompt_tokens: 5, completion_tokens: 10, total_tokens: 15 },
    ///     latency_ms: 50.0,
    ///     cached: false,
    ///     finish_reason: Some("stop".to_string()),
    /// };
    ///
    /// cache.put(&request, response.clone()).unwrap();
    ///
    /// // Subsequent get returns the cached response
    /// let cached = cache.get(&request);
    /// assert!(cached.is_some());
    /// ```
    #[allow(clippy::result_large_err)]
    pub fn put(&self, request: &CompletionRequest, response: CompletionResponse) -> Result<()> {
        if !self.config.enabled {
            return Ok(());
        }

        let key = self.generate_key(request);
        let now = Instant::now();

        let cached = CachedResponse {
            response,
            created_at: now,
            expires_at: now + Duration::from_secs(self.config.ttl_secs),
            access_count: 1,
            request_hash: self.compute_request_hash(request),
        };

        // Check if we need to evict entries
        if self.cache.len() >= self.config.max_size {
            self.evict_oldest();
        }

        self.cache.insert(key.clone(), cached);

        // Update LRU index
        if let Ok(mut lru) = self.lru_index.lock() {
            lru.push(key, ());
        }

        Ok(())
    }

    /// Compute a hash of the request for validation
    fn compute_request_hash(&self, request: &CompletionRequest) -> u64 {
        use std::collections::hash_map::DefaultHasher;
        use std::hash::{Hash, Hasher};

        let mut hasher = DefaultHasher::new();
        request.prompt.hash(&mut hasher);
        if let Some(ref model) = request.model {
            model.hash(&mut hasher);
        }
        hasher.finish()
    }

    /// Evict the oldest entries to make room (batch eviction for efficiency)
    fn evict_oldest(&self) {
        const EVICT_BATCH_SIZE: usize = 10;
        
        if let Ok(mut lru) = self.lru_index.lock() {
            let mut evicted = 0;
            while evicted < EVICT_BATCH_SIZE {
                match lru.pop_lru() {
                    Some((key, _)) => {
                        if self.cache.remove(&key).is_some() {
                            evicted += 1;
                        }
                    }
                    None => break, // No more entries to evict
                }
            }
            if evicted > 0 {
                tracing::debug!(evicted, "Batch eviction completed");
            }
        }
    }

    /// Invalidate a specific cache entry
    pub fn invalidate(&self, request: &CompletionRequest) -> bool {
        let key = self.generate_key(request);
        self.cache.remove(&key).is_some()
    }

    /// Clear all cached entries
    pub fn clear(&self) {
        self.cache.clear();
        if let Ok(mut lru) = self.lru_index.lock() {
            lru.clear();
        }
    }

    /// Get cache statistics
    pub fn stats(&self) -> CacheStats {
        let mut total_entries = 0usize;
        let mut expired_entries = 0usize;
        let mut total_accesses: u64 = 0;
        let now = Instant::now();

        // Single pass to collect all statistics atomically
        for entry in self.cache.iter() {
            total_entries += 1;
            if now >= entry.expires_at {
                expired_entries += 1;
            }
            total_accesses += entry.access_count;
        }

        let valid_entries = total_entries.saturating_sub(expired_entries);

        CacheStats {
            total_entries,
            valid_entries,
            expired_entries,
            total_accesses,
            hit_rate: None, // Would need tracking hits/misses
            memory_usage_bytes: None, // Would need actual measurement
        }
    }

    /// Check if caching is enabled
    pub fn is_enabled(&self) -> bool {
        self.config.enabled
    }
}

/// Cache statistics
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct CacheStats {
    pub total_entries: usize,
    pub valid_entries: usize,
    pub expired_entries: usize,
    pub total_accesses: u64,
    pub hit_rate: Option<f64>,
    pub memory_usage_bytes: Option<usize>,
}
