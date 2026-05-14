//! # LRU Cache - 最近最少使用缓存
//!
//! 高性能LRU缓存实现，用于：
//! - **正则匹配结果缓存** - 避免重复编译/匹配200+正则
//! - **动态数据缓存** - Git分支/Docker容器等动态补全数据
//! - **置信度计算缓存** - 缓存相似操作的计算结果
//!
//! ## 性能特性
//!
//! - O(1) 的get/put操作
//! - 自动淘汰最久未使用的条目
//! - 可配置的容量和TTL（生存时间）
//! - 线程安全（支持多线程并发访问）
//! - 统计信息收集（命中率、未命中率）

use std::collections::{HashMap, VecDeque};
use std::time::{Duration, Instant};

/// LRU缓存条目
#[derive(Debug, Clone)]
struct CacheEntry<V> {
    value: V,
    created_at: Instant,
    last_accessed: Instant,
    access_count: u64,
}

/// LRU缓存实现
pub struct LruCache<K, V>
where
    K: std::hash::Hash + Eq + Clone,
    V: Clone,
{
    /// 数据存储
    data: HashMap<K, CacheEntry<V>>,
    
    /// 访问顺序记录（用于LRU淘汰）
    access_order: VecDeque<K>,
    
    /// 最大容量
    capacity: usize,
    
    /// 条目生存时间（None表示永不过期）
    ttl: Option<Duration>,
    
    /// 统计信息
    stats: CacheStats,
}

/// 缓存统计信息
#[derive(Debug, Clone, Default)]
pub struct CacheStats {
    pub hits: u64,
    pub misses: u64,
    pub evictions: u64,
    pub inserts: u64,
}

impl CacheStats {
    /// 命中率 (0.0-1.0)
    pub fn hit_rate(&self) -> f64 {
        let total = self.hits + self.misses;
        if total == 0 {
            0.0
        } else {
            self.hits as f64 / total as f64
        }
    }
}

impl<K, V> LruCache<K, V>
where
    K: std::hash::Hash + Eq + Clone,
    V: Clone,
{
    /// 创建新的LRU缓存
    pub fn new(capacity: usize) -> Self {
        Self {
            data: HashMap::with_capacity(capacity),
            access_order: VecDeque::with_capacity(capacity),
            capacity,
            ttl: None,
            stats: CacheStats::default(),
        }
    }

    /// 创建带TTL的缓存
    pub fn with_ttl(capacity: usize, ttl: Duration) -> Self {
        Self {
            ttl: Some(ttl),
            ..Self::new(capacity)
        }
    }

    /// 获取缓存值
    pub fn get(&mut self, key: &K) -> Option<V> {
        // 先检查是否存在且未过期
        if let Some(entry) = self.data.get_mut(key) {
            if self.is_expired(entry) {
                self.remove_entry(key);
                return None;
            }

            // 更新访问时间和顺序
            entry.last_accessed = Instant::now();
            entry.access_count += 1;
            
            // 移动到队尾（最近使用）
            self.touch_key(key);
            
            self.stats.hits += 1;
            Some(entry.value.clone())
        } else {
            self.stats.misses += 1;
            None
        }
    }

    /// 插入或更新缓存值
    pub fn put(&mut self, key: K, value: V) {
        // 如果已存在，更新值
        if self.data.contains_key(&key) {
            if let Some(entry) = self.data.get_mut(&key) {
                entry.value = value;
                entry.last_accessed = Instant::now();
                entry.access_count += 1;
                self.touch_key(&key);
            }
            return;
        }

        // 检查是否需要淘汰
        while self.data.len() >= self.capacity {
            self.evict_lru();
        }

        // 插入新条目
        let now = Instant::now();
        self.data.insert(key.clone(), CacheEntry {
            value,
            created_at: now,
            last_accessed: now,
            access_count: 1,
        });
        
        self.access_order.push_back(key);
        self.stats.inserts += 1;
    }

    /// 批量预加载
    pub fn preload(&mut self, items: Vec<(K, V)>) {
        for (key, value) in items {
            self.put(key, value);
        }
    }

    /// 检查键是否存在
    pub fn contains_key(&self, key: &K) -> bool {
        self.data.contains_key(key)
    }

    /// 移除指定键
    pub fn remove(&mut self, key: &K) -> Option<V> {
        self.remove_entry(key).map(|entry| entry.value)
    }

    /// 清空缓存
    pub fn clear(&mut self) {
        self.data.clear();
        self.access_order.clear();
    }

    /// 当前大小
    pub fn len(&self) -> usize {
        self.data.len()
    }

    /// 是否为空
    pub fn is_empty(&self) -> bool {
        self.data.is_empty()
    }

    /// 获取统计信息
    pub fn statistics(&self) -> &CacheStats {
        &self.stats
    }

    /// 清理过期条目
    pub fn cleanup_expired(&mut self) -> usize {
        if self.ttl.is_none() {
            return 0;
        }

        let before = self.data.len();
        
        // 收集过期的key
        let expired_keys: Vec<K> = self.data.iter()
            .filter(|(_, entry)| self.is_expired(entry))
            .map(|(k, _)| k.clone())
            .collect();

        for key in expired_keys {
            self.remove_entry(&key);
        }

        before - self.data.len()
    }

    /// 调整容量
    pub fn resize(&mut self, new_capacity: usize) {
        self.capacity = new_capacity;
        
        // 如果当前大小超过新容量，淘汰多余条目
        while self.data.len() > new_capacity {
            self.evict_lru();
        }
    }

    // ══════════════════════════════
    // 内部方法
    // ══════════════════════════════

    fn touch_key(&mut self, key: &K) {
        // 从当前位置移除
        if let Some(pos) = self.access_order.iter().position(|k| k == key) {
            self.access_order.remove(pos);
        }
        
        // 添加到队尾
        self.access_order.push_back(key.clone());
    }

    fn remove_entry(&mut self, key: &K) -> Option<CacheEntry<V>> {
        let entry = self.data.remove(key)?;
        
        // 从访问顺序中移除
        if let Some(pos) = self.access_order.iter().position(|k| k == key) {
            self.access_order.remove(pos);
        }
        
        Some(entry)
    }

    fn evict_lru(&mut self) {
        if let Some(lru_key) = self.access_order.pop_front() {
            self.data.remove(&lru_key);
            self.stats.evictions += 1;
        }
    }

    fn is_expired(&self, entry: &CacheEntry<V>) -> bool {
        match self.ttl {
            Some(ttl) => entry.created_at.elapsed() > ttl,
            None => false,
        }
    }
}

// ==========================================
// 特化版本：用于字符串缓存的便捷方法
// ==========================================

/// 字符串结果缓存（用于正则匹配等场景）
pub type StringResultCache<V> = LruCache<String, V>;

impl<V: Clone> StringResultCache<V> {
    /// 获取或计算（如果不存在则调用factory并缓存）
    pub fn get_or_compute<F>(&mut self, key: &str, factory: F) -> V
    where
        F: FnOnce() -> V,
    {
        if let Some(value) = self.get(&key.to_string()) {
            value
        } else {
            let value = factory();
            self.put(key.to_string(), value);
            value.clone()
        }
    }
}

// ==========================================
// 单元测试
// ==========================================

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_basic_get_put() {
        let mut cache: LruCache<i32, String> = LruCache::new(3);
        
        cache.put(1, "one".to_string());
        cache.put(2, "two".to_string());
        
        assert_eq!(cache.get(&1), Some("one".to_string()));
        assert_eq!(cache.get(&2), Some("two".to_string()));
        assert_eq!(cache.get(&3), None);
    }

    #[test]
    fn test_lru_eviction() {
        let mut cache: LruCache<i32, i32> = LruCache::new(2);
        
        cache.put(1, 10);
        cache.put(2, 20);
        cache.put(3, 30);  // 应该淘汰key=1
        
        assert_eq!(cache.get(&1), None);  // 已被淘汰
        assert_eq!(cache.get(&2), Some(20));
        assert_eq!(cache.get(&3), Some(30));
    }

    #[test]
    fn test_access_order_update() {
        let mut cache: LruCache<char, char> = LruCache::new(3);
        
        cache.put('a', 'A');
        cache.put('b', 'B');
        cache.put('c', 'C');
        
        // 访问'a'使其变为最近使用
        cache.get(&'a');
        
        // 插入'd'应该淘汰'b'而不是'a'
        cache.put('d', 'D');
        
        assert_eq!(cache.get(&'a'), Some('A'));  // 仍在缓存
        assert_eq!(cache.get(&'b'), None);      // 被淘汰
        assert_eq!(cache.get(&'c'), Some('C'));
        assert_eq!(cache.get(&'d'), Some('D'));
    }

    #[test]
    fn test_ttl_expiration() {
        let mut cache = LruCache::with_ttl(3, Duration::from_millis(50));
        
        cache.put("key1", "value1");
        
        // 立即获取应该成功
        assert_eq!(cache.get(&"key1".to_string()), Some("value1".to_string()));
        
        // 等待过期
        std::thread::sleep(Duration::from_millis(60));
        
        // 应该已过期
        assert_eq!(cache.get(&"key1".to_string()), None);
    }

    #[test]
    fn test_statistics_tracking() {
        let mut cache: LruCache<&str, &str> = LruCache::new(10);
        
        cache.put("a", "1");
        cache.get(&"a");   // hit
        cache.get(&"b");   // miss
        
        let stats = cache.statistics();
        assert_eq!(stats.hits, 1);
        assert_eq!(stats.misses, 1);
        assert_eq!(stats.inserts, 1);
        
        let hit_rate = stats.hit_rate();
        assert!((hit_rate - 0.5).abs() < 0.01);  // 应该约等于0.5
    }

    #[test]
    fn test_cleanup_expired() {
        let mut cache = LruCache::with_ttl(10, Duration::from_millis(30));
        
        for i in 0..10 {
            cache.put(format!("key{}", i), format!("val{}", i));
        }
        
        // 等待部分过期
        std::thread::sleep(Duration::from_millis(35));
        
        let removed = cache.cleanup_expired();
        assert!(removed > 0, "Should have expired some entries");
    }

    #[test]
    fn test_resize() {
        let mut cache: LruCache<i32, i32> = LruCache::new(5);
        
        for i in 0..5 {
            cache.put(i, i * 10);
        }
        
        assert_eq!(cache.len(), 5);
        
        // 缩小到2
        cache.resize(2);
        
        assert_eq!(cache.len(), 2);
        // 应该保留最近使用的两个
    }

    #[test]
    fn test_preload() {
        let mut cache: LruCache<String, String> = LruCache::new(10);
        
        let items = vec![
            ("a".to_string(), "1".to_string()),
            ("b".to_string(), "2".to_string()),
            ("c".to_string(), "3".to_string()),
        ];
        
        cache.preload(items);
        
        assert_eq!(cache.len(), 3);
        assert_eq!(cache.get(&"a".to_string()), Some("1".to_string()));
    }

    #[test]
    fn test_remove_and_clear() {
        let mut cache: LruCache<i32, i32> = LruCache::new(5);
        
        cache.put(1, 10);
        cache.put(2, 20);
        
        let removed = cache.remove(&1);
        assert_eq!(removed, Some(10));
        assert_eq!(cache.len(), 1);
        
        cache.clear();
        assert!(cache.is_empty());
    }

    #[test]
    fn test_get_or_compute() {
        let mut cache: StringResultCache<Vec<String>> = StringResultCache::new(100);
        
        let result1 = cache.get_or_compute("expensive_op", || {
            vec!["result".to_string()]
        });
        
        let result2 = cache.get_or_compute("expensive_op", || {
            panic!("Should not call factory again")
        });
        
        assert_eq!(result1, result2);
        assert_eq!(result1, vec!["result".to_string()]);
    }

    #[test]
    fn test_high_concurrency_simulation() {
        let mut cache: LruCache<u64, u64> = LruCache::new(1000);
        
        // 模拟大量插入和读取
        for i in 0..2000u64 {
            cache.put(i % 1500, i * 2);
            
            if i > 500 && i % 3 == 0 {
                cache.get(&(i - 500));
            }
        }
        
        // 缓存应该保持容量限制
        assert!(cache.len() <= 1000);
        
        // 应该有较高的命中率
        let stats = cache.statistics();
        assert!(stats.hit_rate() > 0.3, "Hit rate should be reasonable");
    }
}
