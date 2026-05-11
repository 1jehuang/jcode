//! LSP Result Cache — 高性能结果缓存系统
//!
//! ## 核心能力
//! - TTL-based 缓存（默认 5 秒）
//! - 自动过期清理
//! - 并发安全
//! - 内存占用控制
//!
//! ## 设计目标
//! - 减少 LSP Server 调用次数（特别是重复查询）
//! - 提升响应速度（缓存命中 < 1ms）
//! - 控制内存使用（LRU + TTL）

use std::collections::HashMap;
use std::sync::Arc;
use std::time::{Duration, Instant};
use tokio::sync::RwLock;
use tracing::{debug};

/// 默认缓存 TTL (5 seconds)
const DEFAULT_CACHE_TTL: Duration = Duration::from_secs(5);

/// 最大缓存条目数
const MAX_CACHE_ENTRIES: usize = 1000;

/// 缓存条目
struct CacheEntry<T> {
    value: T,
    created_at: Instant,
    access_count: u64,
}

impl<T> CacheEntry<T> {
    fn new(value: T) -> Self {
        Self {
            value,
            created_at: Instant::now(),
            access_count: 0,
        }
    }

    fn is_expired(&self, ttl: Duration) -> bool {
        self.created_at.elapsed() > ttl
    }

    fn touch(&mut self) {
        self.access_count += 1;
    }
}

/// LSP 结果缓存
pub struct LspResultCache<T> {
    cache: RwLock<HashMap<String, CacheEntry<T>>>,
    ttl: Duration,
    max_entries: usize,
    hits: Arc<RwLock<u64>>,
    misses: Arc<RwLock<u64>>,
}

impl<T: Clone + Send + Sync + 'static> LspResultCache<T> {
    /// 创建新的缓存实例
    pub fn new() -> Self {
        Self::with_ttl(DEFAULT_CACHE_TTL)
    }

    /// 创建带自定义 TTL 的缓存
    pub fn with_ttl(ttl: Duration) -> Self {
        Self {
            cache: RwLock::new(HashMap::new()),
            ttl,
            max_entries: MAX_CACHE_ENTRIES,
            hits: Arc::new(RwLock::new(0)),
            misses: Arc::new(RwLock::new(0)),
        }
    }

    /// 设置最大缓存条目数
    pub fn with_max_entries(mut self, max: usize) -> Self {
        self.max_entries = max;
        self
    }

    /// 获取缓存值（如果存在且未过期）
    pub async fn get(&self, key: &str) -> Option<T> {
        let mut cache = self.cache.write().await;
        
        if let Some(entry) = cache.get_mut(key) {
            if !entry.is_expired(self.ttl) {
                entry.touch();
                *self.hits.write().await += 1;
                debug!(key = %key, "Cache hit");
                return Some(entry.value.clone());
            } else {
                // 过期，移除
                cache.remove(key);
            }
        }

        *self.misses.write().await += 1;
        debug!(key = %key, "Cache miss");
        None
    }

    /// 设置缓存值
    pub async fn set(&self, key: &str, value: T) {
        let mut cache = self.cache.write().await;

        // 如果超过最大条目数，清理最旧的条目
        if cache.len() >= self.max_entries && !cache.contains_key(key) {
            self.evict_oldest(&mut cache).await;
        }

        cache.insert(key.to_string(), CacheEntry::new(value));
        debug!(key = %key, entries = cache.len(), "Cache set");
    }

    /// 获取或计算值（如果缓存未命中）
    pub async fn get_or_compute<F, Fut>(&self, key: &str, compute_fn: F) -> T
    where
        F: FnOnce() -> Fut,
        Fut: std::future::Future<Output = T>,
    {
        // 先尝试从缓存获取
        if let Some(cached) = self.get(key).await {
            return cached;
        }

        // 缓存未命中，计算新值
        let value = compute_fn().await;
        
        // 存入缓存
        self.set(key, value.clone()).await;
        
        value
    }

    /// 清除指定键的缓存
    pub async fn invalidate(&self, key: &str) {
        let mut cache = self.cache.write().await;
        cache.remove(key);
        debug!(key = %key, "Cache invalidated");
    }

    /// 清除所有缓存
    pub async fn clear(&self) {
        let mut cache = self.cache.write().await;
        let count = cache.len();
        cache.clear();
        debug!(entries = count, "Cache cleared");
    }

    /// 清理过期条目
    pub async fn cleanup_expired(&self) -> usize {
        let mut cache = self.cache.write().await;
        let before = cache.len();
        
        cache.retain(|_key, entry| !entry.is_expired(self.ttl));
        
        let removed = before - cache.len();
        if removed > 0 {
            debug!(removed, remaining = cache.len(), "Cleaned up expired entries");
        }
        
        removed
    }

    /// 获取缓存统计信息
    pub async fn stats(&self) -> CacheStats {
        let hits = *self.hits.read().await;
        let misses = *self.misses.read().await;
        let entries = self.cache.read().await.len();

        CacheStats {
            entries,
            hits,
            misses,
            hit_rate: if hits + misses > 0 {
                Some(hits as f64 / (hits + misses) as f64)
            } else {
                None
            },
        }
    }

    /// 异步清理过期条目（后台任务）
    pub async fn start_cleanup_task(self: Arc<Self>, interval: Duration) -> tokio::task::JoinHandle<()> {
        tokio::spawn(async move {
            loop {
                tokio::time::sleep(interval).await;
                self.cleanup_expired().await;
            }
        })
    }

    // ─── 内部方法 ─────────────────────────

    async fn evict_oldest(&self, cache: &mut HashMap<String, CacheEntry<T>>) {
        // 找到最旧的条目并移除
        if let Some(oldest_key) = cache.iter()
            .min_by_key(|(_, entry)| entry.created_at)
            .map(|(k, _)| k.clone())
        {
            cache.remove(&oldest_key);
            debug!(key = %oldest_key, "Evicted oldest cache entry");
        }
    }
}

impl<T: Clone + Send + Sync + 'static> Default for LspResultCache<T> {
    fn default() -> Self {
        Self::new()
    }
}

/// 缓存统计信息
#[derive(Debug, Clone)]
pub struct CacheStats {
    pub entries: usize,
    pub hits: u64,
    pub misses: u64,
    pub hit_rate: Option<f64>,
}

impl std::fmt::Display for CacheStats {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        write!(
            f,
            "Cache[entries={}, hits={}, misses={}, hit_rate={:.2}%]",
            self.entries,
            self.hits,
            self.misses,
            self.hit_rate.map_or(0.0, |r| r * 100.0)
        )
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[tokio::test]
    async fn test_cache_basic_operations() {
        let cache = LspResultCache::with_ttl(Duration::from_secs(10));

        // 初始为空
        assert!(cache.get("key1").await.is_none());

        // 设置值
        cache.set("key1", "value1".to_string()).await;

        // 获取值
        assert_eq!(cache.get("key1").await, Some("value1".to_string()));

        // 统计
        let stats = cache.stats().await;
        assert_eq!(stats.entries, 1);
        assert_eq!(stats.hits, 1);
        assert_eq!(stats.misses, 1); // 第一次是 miss
    }

    #[tokio::test]
    async fn test_cache_expiration() {
        let cache = LspResultCache::with_ttl(Duration::from_millis(50));

        cache.set("key1", "value1".to_string()).await;

        // 立即获取应该命中
        assert!(cache.get("key1").await.is_some());

        // 等待过期
        tokio::time::sleep(Duration::from_millis(60)).await;

        // 过期后应该是 miss
        assert!(cache.get("key1").await.is_none());
    }

    #[tokio::test]
    async fn test_cache_get_or_compute() {
        let cache = LspResultCache::<String>::new();
        let call_count = Arc::new(std::sync::atomic::AtomicU32::new(0));

        let result = cache
            .get_or_compute("key1", || {
                let counter = call_count.clone();
                async move {
                    counter.fetch_add(1, std::sync::atomic::Ordering::SeqCst);
                    "computed_value".to_string()
                }
            })
            .await;

        assert_eq!(result, "computed_value");
        assert_eq!(call_count.load(std::sync::atomic::Ordering::SeqCst), 1);

        // 第二次应该从缓存获取
        let result2 = cache
            .get_or_compute("key1", || {
                let counter = call_count.clone();
                async move {
                    counter.fetch_add(1, std::sync::atomic::Ordering::SeqCst);
                    "should_not_be_called".to_string()
                }
            })
            .await;

        assert_eq!(result2, "computed_value");
        assert_eq!(call_count.load(std::sync::atomic::Ordering::SeqCst), 1); // 不应该再次调用
    }

    #[tokio::test]
    async fn test_cache_invalidate() {
        let cache = LspResultCache::new();

        cache.set("key1", "value1".to_string()).await;
        assert!(cache.get("key1").await.is_some());

        cache.invalidate("key1").await;
        assert!(cache.get("key1").await.is_none());
    }

    #[tokio::test]
    async fn test_cache_stats() {
        let cache = LspResultCache::new();

        // Miss
        cache.get("nonexistent").await;
        
        // Set and Hit
        cache.set("key1", "val".to_string()).await;
        cache.get("key1").await;

        let stats = cache.stats().await;
        assert_eq!(stats.entries, 1);
        assert_eq!(stats.misses, 1);
        assert_eq!(stats.hits, 1);
        assert!(stats.hit_rate.unwrap() > 0.4); // 50% 左右
    }
}
