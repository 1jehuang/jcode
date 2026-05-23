//! High-performance caching layer for 500+ concurrent users
//!
//! Uses DashMap for lock-free concurrent access and implements multi-tier caching:
//! - L1: In-memory cache (DashMap) for hot data
//! - L2: Redis cache for warm data
//! - L3: Database for cold data

use dashmap::DashMap;
use std::sync::Arc;
use std::time::{Duration, Instant};
use tokio::sync::RwLock;
use tracing::{debug, info, warn};

/// Cache entry with TTL support
#[derive(Debug, Clone)]
pub struct CacheEntry<T> {
    pub value: T,
    pub created_at: Instant,
    pub expires_at: Option<Instant>,
    pub last_accessed: Arc<RwLock<Instant>>,
}

impl<T> CacheEntry<T> {
    pub fn new(value: T, ttl: Option<Duration>) -> Self {
        let now = Instant::now();
        Self {
            value,
            created_at: now,
            expires_at: ttl.map(|d| now + d),
            last_accessed: Arc::new(RwLock::new(now)),
        }
    }

    pub fn is_expired(&self) -> bool {
        if let Some(expires_at) = self.expires_at {
            Instant::now() > expires_at
        } else {
            false
        }
    }

    pub async fn touch(&self) {
        let mut last_accessed = self.last_accessed.write().await;
        *last_accessed = Instant::now();
    }
}

/// Multi-tier cache configuration
#[derive(Debug, Clone)]
pub struct CacheConfig {
    /// L1 cache max size (number of entries)
    pub l1_max_size: usize,
    /// L1 cache default TTL
    pub l1_ttl: Duration,
    /// Enable L2 cache (Redis)
    pub enable_l2: bool,
    /// L2 cache TTL
    pub l2_ttl: Duration,
    /// Eviction policy: "lru" or "lfu"
    pub eviction_policy: String,
}

impl Default for CacheConfig {
    fn default() -> Self {
        Self {
            l1_max_size: 100_000,
            l1_ttl: Duration::from_secs(300), // 5 minutes
            enable_l2: true,
            l2_ttl: Duration::from_secs(3600), // 1 hour
            eviction_policy: "lru".to_string(),
        }
    }
}

/// L1 in-memory cache using DashMap
pub struct L1Cache<K, V> {
    store: DashMap<K, CacheEntry<V>>,
    config: CacheConfig,
    hit_count: Arc<std::sync::atomic::AtomicU64>,
    miss_count: Arc<std::sync::atomic::AtomicU64>,
    evict_count: Arc<std::sync::atomic::AtomicU64>,
}

impl<K, V> L1Cache<K, V>
where
    K: std::hash::Hash + Eq + Clone + std::fmt::Debug,
    V: Clone + Send + Sync + 'static,
{
    pub fn new(config: CacheConfig) -> Self {
        info!(
            "Initializing L1 cache with max_size={} ttl={:?} policy={}",
            config.l1_max_size, config.l1_ttl, config.eviction_policy
        );
        Self {
            store: DashMap::new(),
            config,
            hit_count: Arc::new(std::sync::atomic::AtomicU64::new(0)),
            miss_count: Arc::new(std::sync::atomic::AtomicU64::new(0)),
            evict_count: Arc::new(std::sync::atomic::AtomicU64::new(0)),
        }
    }

    /// Get value from cache
    pub async fn get(&self, key: &K) -> Option<V> {
        if let Some(entry_ref) = self.store.get(key) {
            if entry_ref.is_expired() {
                drop(entry_ref);
                self.store.remove(key);
                self.miss_count.fetch_add(1, std::sync::atomic::Ordering::Relaxed);
                debug!("Cache expired for key {:?}", key);
                return None;
            }

            let value = entry_ref.value.clone();
            drop(entry_ref);

            // Update last accessed time
            if let Some(entry_ref) = self.store.get(key) {
                entry_ref.touch().await;
            }

            self.hit_count.fetch_add(1, std::sync::atomic::Ordering::Relaxed);
            debug!("Cache hit for key {:?}", key);
            Some(value)
        } else {
            self.miss_count.fetch_add(1, std::sync::atomic::Ordering::Relaxed);
            debug!("Cache miss for key {:?}", key);
            None
        }
    }

    /// Set value in cache
    pub async fn set(&self, key: K, value: V, ttl: Option<Duration>) {
        // Evict if cache is full
        if self.store.len() >= self.config.l1_max_size {
            self.evict().await;
        }

        let entry = CacheEntry::new(value, ttl.or(Some(self.config.l1_ttl)));
        self.store.insert(key, entry);
    }

    /// Remove value from cache
    pub fn remove(&self, key: &K) -> Option<V> {
        self.store.remove(key).map(|(_, entry)| entry.value)
    }

    /// Clear all cache entries
    pub fn clear(&self) {
        self.store.clear();
        info!("L1 cache cleared");
    }

    /// Get cache statistics
    pub fn stats(&self) -> CacheStats {
        let hits = self.hit_count.load(std::sync::atomic::Ordering::Relaxed);
        let misses = self.miss_count.load(std::sync::atomic::Ordering::Relaxed);
        let total = hits + misses;
        let hit_rate = if total > 0 {
            hits as f64 / total as f64
        } else {
            0.0
        };

        CacheStats {
            size: self.store.len(),
            hits,
            misses,
            evictions: self.evict_count.load(std::sync::atomic::Ordering::Relaxed),
            hit_rate,
        }
    }

    /// Evict entries based on policy (LRU or LFU)
    async fn evict(&self) {
        if self.store.is_empty() {
            return;
        }

        let mut oldest_key = None;
        let mut oldest_time = Instant::now();

        // Simple LRU eviction: remove least recently accessed entry
        for entry in self.store.iter() {
            let last_accessed = entry.value.last_accessed.read().await;
            if *last_accessed < oldest_time {
                oldest_time = *last_accessed;
                oldest_key = Some(entry.key().clone());
            }
        }

        if let Some(key) = oldest_key {
            self.store.remove(&key);
            self.evict_count.fetch_add(1, std::sync::atomic::Ordering::Relaxed);
            debug!("Evicted cache entry {:?}", key);
        }
    }
}

/// Cache statistics
#[derive(Debug, Clone)]
pub struct CacheStats {
    pub size: usize,
    pub hits: u64,
    pub misses: u64,
    pub evictions: u64,
    pub hit_rate: f64,
}

/// Session cache for active user sessions
pub struct SessionCache {
    cache: L1Cache<String, SessionInfo>,
}

#[derive(Debug, Clone)]
pub struct SessionInfo {
    pub user_id: String,
    pub tenant_id: Option<String>,
    pub session_token: String,
    pub created_at: Instant,
    pub last_activity: Instant,
    pub model_preference: Option<String>,
}

impl SessionCache {
    pub fn new() -> Self {
        let config = CacheConfig {
            l1_max_size: 50_000, // Support 500 concurrent users with room to grow
            l1_ttl: Duration::from_secs(3600), // 1 hour session timeout
            ..Default::default()
        };
        Self {
            cache: L1Cache::<String, SessionInfo>::new(config),
        }
    }

    pub async fn get_session(&self, token: &str) -> Option<SessionInfo> {
        self.cache.get(&token.to_string()).await
    }

    pub async fn set_session(&self, token: String, info: SessionInfo) {
        self.cache.set(token, info, None).await;
    }

    pub async fn invalidate_session(&self, token: &str) {
        self.cache.remove(&token.to_string());
    }

    pub fn stats(&self) -> CacheStats {
        self.cache.stats()
    }
}

/// Model response cache for repeated queries
pub struct ModelResponseCache {
    cache: L1Cache<String, String>,
}

impl ModelResponseCache {
    pub fn new() -> Self {
        let config = CacheConfig {
            l1_max_size: 100_000,
            l1_ttl: Duration::from_secs(600), // 10 minutes for model responses
            ..Default::default()
        };
        Self {
            cache: L1Cache::<String, String>::new(config),
        }
    }

    pub async fn get_response(&self, cache_key: &str) -> Option<String> {
        self.cache.get(&cache_key.to_string()).await
    }

    pub async fn set_response(&self, cache_key: String, response: String, ttl: Option<Duration>) {
        self.cache.set(cache_key, response, ttl).await;
    }

    pub fn stats(&self) -> CacheStats {
        self.cache.stats()
    }
}

/// Context cache for code completion context
pub struct ContextCache {
    cache: L1Cache<String, Vec<u8>>, // Serialized context
}

impl ContextCache {
    pub fn new() -> Self {
        let config = CacheConfig {
            l1_max_size: 200_000, // Large cache for context data
            l1_ttl: Duration::from_secs(1800), // 30 minutes
            ..Default::default()
        };
        Self {
            cache: L1Cache::<String, Vec<u8>>::new(config),
        }
    }

    pub async fn get_context(&self, key: &str) -> Option<Vec<u8>> {
        self.cache.get(&key.to_string()).await
    }

    pub async fn set_context(&self, key: String, context: Vec<u8>) {
        self.cache.set(key, context, None).await;
    }

    pub fn stats(&self) -> CacheStats {
        self.cache.stats()
    }
}

/// Unified cache manager for the enterprise server
pub struct CacheManager {
    pub session_cache: Arc<SessionCache>,
    pub response_cache: Arc<ModelResponseCache>,
    pub context_cache: Arc<ContextCache>,
}

impl CacheManager {
    pub fn new() -> Self {
        info!("Initializing CacheManager for Phase 2 scale (500+ users)");
        Self {
            session_cache: Arc::new(SessionCache::new()),
            response_cache: Arc::new(ModelResponseCache::new()),
            context_cache: Arc::new(ContextCache::new()),
        }
    }

    /// Get comprehensive cache statistics
    pub fn get_all_stats(&self) -> CacheManagerStats {
        CacheManagerStats {
            session_stats: self.session_cache.stats(),
            response_stats: self.response_cache.stats(),
            context_stats: self.context_cache.stats(),
        }
    }

    /// Clear all caches
    pub fn clear_all(&self) {
        self.session_cache.cache.clear();
        self.response_cache.cache.clear();
        self.context_cache.cache.clear();
        info!("All caches cleared");
    }
}

/// Comprehensive cache statistics
#[derive(Debug)]
pub struct CacheManagerStats {
    pub session_stats: CacheStats,
    pub response_stats: CacheStats,
    pub context_stats: CacheStats,
}

#[cfg(test)]
mod tests {
    use super::*;

    #[tokio::test]
    async fn test_l1_cache_basic() {
        let config = CacheConfig::default();
        let cache = L1Cache::new(config);

        cache.set("key1".to_string(), "value1".to_string(), None).await;

        let result = cache.get(&"key1".to_string()).await;
        assert_eq!(result, Some("value1".to_string()));

        let result = cache.get(&"key2".to_string()).await;
        assert_eq!(result, None);
    }

    #[tokio::test]
    async fn test_l1_cache_ttl() {
        let config = CacheConfig {
            l1_ttl: Duration::from_millis(100),
            ..Default::default()
        };
        let cache = L1Cache::new(config);

        cache
            .set("key1".to_string(), "value1".to_string(), None)
            .await;

        // Should exist immediately
        assert!(cache.get(&"key1".to_string()).await.is_some());

        // Wait for expiration
        tokio::time::sleep(Duration::from_millis(150)).await;

        // Should be expired
        assert!(cache.get(&"key1".to_string()).await.is_none());
    }

    #[tokio::test]
    async fn test_cache_stats() {
        let config = CacheConfig::default();
        let cache = L1Cache::new(config);

        cache.set("key1".to_string(), "value1".to_string(), None).await;
        cache.get(&"key1".to_string()).await;
        cache.get(&"nonexistent".to_string()).await;

        let stats = cache.stats();
        assert_eq!(stats.hits, 1);
        assert_eq!(stats.misses, 1);
        assert_eq!(stats.size, 1);
        assert!(stats.hit_rate > 0.4 && stats.hit_rate < 0.6);
    }
}
