//! 优化后的内存缓存系统
//!
//! 特性:
//! - LRU (Least Recently Used) 淘汰策略
//! - TTL (Time To Live) 过期机制
//! - 基于内容哈希的变更检测
//! - 并发安全的 RwLock 访问
//! - 缓存命中率统计

use crate::memory_graph::MemoryGraph;
use std::collections::{HashMap, VecDeque};
use std::hash::{Hash, Hasher};
use std::path::PathBuf;
use std::sync::atomic::{AtomicU64, Ordering};
use std::sync::{Arc, RwLock};
use std::time::{Duration, Instant, SystemTime};

/// 缓存条目
struct CacheEntry {
    graph: MemoryGraph,
    modified: Option<SystemTime>,
    content_hash: u64,
    created_at: Instant,
    last_accessed: Instant,
    access_count: u64,
}

/// LRU 缓存
pub struct LruCache {
    entries: HashMap<PathBuf, CacheEntry>,
    access_order: VecDeque<PathBuf>,
    max_size: usize,
    ttl: Duration,
}

impl LruCache {
    fn new(max_size: usize, ttl: Duration) -> Self {
        Self {
            entries: HashMap::new(),
            access_order: VecDeque::new(),
            max_size,
            ttl,
        }
    }

    /// 获取缓存项
    fn get(&mut self, path: &PathBuf) -> Option<&MemoryGraph> {
        // 先检查 TTL 和有效性，如果无效则直接移除
        if let Some(entry) = self.entries.get(path) {
            if entry.created_at.elapsed() > self.ttl {
                self.remove(path);
                return None;
            }
        }

        // 获取条目并更新访问信息
        let result = if let Some(entry) = self.entries.get_mut(path) {
            entry.last_accessed = Instant::now();
            entry.access_count += 1;
            // 移动到访问队列末尾 (最近使用)
            self.access_order.retain(|p| p != path);
            self.access_order.push_back(path.clone());
            Some(&entry.graph)
        } else {
            None
        };

        result
    }

    /// 插入缓存项
    fn insert(&mut self, path: PathBuf, graph: MemoryGraph, modified: Option<SystemTime>, content_hash: u64) {
        // 如果已存在，先移除
        if self.entries.contains_key(&path) {
            self.remove(&path);
        }

        // 如果缓存已满，淘汰最久未使用的
        while self.entries.len() >= self.max_size {
            if let Some(lru_path) = self.access_order.pop_front() {
                self.entries.remove(&lru_path);
            } else {
                break;
            }
        }

        let now = Instant::now();
        let entry = CacheEntry {
            graph,
            modified,
            content_hash,
            created_at: now,
            last_accessed: now,
            access_count: 0,
        };

        self.entries.insert(path.clone(), entry);
        self.access_order.push_back(path);
    }

    /// 移除缓存项
    fn remove(&mut self, path: &PathBuf) {
        self.entries.remove(path);
        self.access_order.retain(|p| p != path);
    }

    /// 检查缓存是否有效 (内容未变更)
    fn is_valid(&self, path: &PathBuf, current_hash: u64, current_mtime: Option<SystemTime>) -> bool {
        let entry = match self.entries.get(path) {
            Some(e) => e,
            None => return false,
        };

        // TTL 检查
        if entry.created_at.elapsed() > self.ttl {
            return false;
        }

        // 内容哈希检查
        if entry.content_hash != current_hash {
            return false;
        }

        // mtime 检查 (如果可用)
        if let (Some(cached_mtime), Some(current_mtime)) = (entry.modified, current_mtime) {
            // 如果文件在缓存创建之后被修改，缓存无效
            if current_mtime > cached_mtime {
                return false;
            }
        }

        true
    }

    /// 获取统计信息
    fn stats(&self) -> CacheStats {
        let total_accesses: u64 = self.entries.values().map(|e| e.access_count).sum();
        CacheStats {
            entry_count: self.entries.len(),
            max_size: self.max_size,
            total_accesses,
            avg_access_count: if self.entries.is_empty() {
                0.0
            } else {
                total_accesses as f64 / self.entries.len() as f64
            },
        }
    }

    /// 清空缓存
    fn clear(&mut self) {
        self.entries.clear();
        self.access_order.clear();
    }
}

/// 缓存统计
#[derive(Debug, Clone, Default)]
pub struct CacheStats {
    pub entry_count: usize,
    pub max_size: usize,
    pub total_accesses: u64,
    pub avg_access_count: f64,
}

/// 全局缓存实例
static GRAPH_CACHE: std::sync::OnceLock<Arc<RwLock<LruCache>>> = std::sync::OnceLock::new();

/// 默认缓存最大条目数
const DEFAULT_MAX_SIZE: usize = 100;
/// 默认 TTL: 30 分钟
const DEFAULT_TTL_SECS: u64 = 1800;

fn graph_cache() -> &'static Arc<RwLock<LruCache>> {
    GRAPH_CACHE.get_or_init(|| Arc::new(RwLock::new(LruCache::new(DEFAULT_MAX_SIZE, Duration::from_secs(DEFAULT_TTL_SECS)))))
}

fn graph_mtime(path: &PathBuf) -> Option<SystemTime> {
    std::fs::metadata(path).ok().and_then(|m| m.modified().ok())
}

/// 计算内容哈希
fn compute_content_hash(path: &PathBuf) -> Option<u64> {
    use std::collections::hash_map::DefaultHasher;
    use std::hash::Hash;

    let content = std::fs::read_to_string(path).ok()?;
    let mut hasher = DefaultHasher::new();
    content.hash(&mut hasher);
    Some(hasher.finish())
}

/// 获取缓存的图 (如果有效)
pub(super) fn cached_graph(path: &PathBuf) -> Option<MemoryGraph> {
    let current_hash = compute_content_hash(path)?;
    let current_mtime = graph_mtime(path);

    let mut cache = graph_cache().write().ok()?;

    // 检查缓存是否有效
    if cache.is_valid(path, current_hash, current_mtime) {
        let stats = cache.stats();
        cache.get(path)?;
        debug_assert!(stats.entry_count >= 1, "Cache should have entry after get");
        cache.get(path).map(|g| g.clone())
    } else {
        // 缓存无效，移除
        cache.remove(path);
        None
    }
}

/// 缓存图
pub(super) fn cache_graph(path: PathBuf, graph: &MemoryGraph) {
    let modified = graph_mtime(&path);
    let content_hash = compute_content_hash(&path).unwrap_or(0);

    if let Ok(mut cache) = graph_cache().write() {
        cache.insert(path, graph.clone(), modified, content_hash);
    }
}

/// 获取缓存统计
pub(super) fn cache_stats() -> CacheStats {
    graph_cache().read().map(|c| c.stats()).unwrap_or_default()
}

/// 清空缓存
pub(super) fn clear_cache() {
    if let Ok(mut cache) = graph_cache().write() {
        cache.clear();
    }
}

/// 调整缓存大小
pub(super) fn resize_cache(new_max_size: usize) {
    if let Ok(mut cache) = graph_cache().write() {
        cache.max_size = new_max_size;
        // 如果当前条目超过新大小，淘汰最久未使用的
        while cache.entries.len() > cache.max_size {
            if let Some(lru_path) = cache.access_order.pop_front() {
                cache.entries.remove(&lru_path);
            } else {
                break;
            }
        }
    }
}

/// 高级缓存管理器 - 支持多级缓存
pub struct MultiLevelCache {
    /// L1: 内存缓存
    l1: Arc<RwLock<LruCache>>,
    /// L2: 磁盘缓存路径 (可选)
    l2_path: Option<PathBuf>,
    /// 命中统计 (使用原子操作实现线程安全)
    l1_hits: AtomicU64,
    l2_hits: AtomicU64,
    misses: AtomicU64,
}

impl MultiLevelCache {
    pub fn new(max_memory_entries: usize, ttl_secs: u64) -> Self {
        Self {
            l1: Arc::new(RwLock::new(LruCache::new(max_memory_entries, Duration::from_secs(ttl_secs)))),
            l2_path: None,
            l1_hits: AtomicU64::new(0),
            l2_hits: AtomicU64::new(0),
            misses: AtomicU64::new(0),
        }
    }

    /// 设置磁盘缓存路径
    pub fn with_disk_cache(mut self, path: PathBuf) -> Self {
        self.l2_path = Some(path);
        self
    }

    /// 获取缓存项
    pub fn get(&self, path: &PathBuf) -> Option<MemoryGraph> {
        // L1 查找 (需要写锁因为 get 会修改访问时间)
        if let Ok(mut cache) = self.l1.write() {
            let current_hash = compute_content_hash(path)?;
            let current_mtime = graph_mtime(path);

            if cache.is_valid(path, current_hash, current_mtime) {
                self.l1_hits.fetch_add(1, Ordering::Relaxed);
                return cache.get(path).cloned();
            }
        }

        // TODO: L2 磁盘查找

        self.misses.fetch_add(1, Ordering::Relaxed);
        None
    }

    /// 插入缓存项
    pub fn insert(&self, path: PathBuf, graph: MemoryGraph) {
        if let Ok(mut cache) = self.l1.write() {
            let modified = graph_mtime(&path);
            let content_hash = compute_content_hash(&path).unwrap_or(0);
            cache.insert(path, graph, modified, content_hash);
        }

        // TODO: 异步写入 L2
    }

    /// 获取命中率统计
    pub fn hit_rate(&self) -> f64 {
        let total = self.l1_hits.load(Ordering::Relaxed)
            + self.l2_hits.load(Ordering::Relaxed)
            + self.misses.load(Ordering::Relaxed);
        if total == 0 {
            return 0.0;
        }
        (self.l1_hits.load(Ordering::Relaxed) + self.l2_hits.load(Ordering::Relaxed)) as f64 / total as f64
    }

    /// 重置统计
    pub fn reset_stats(&self) {
        self.l1_hits.store(0, Ordering::Relaxed);
        self.l2_hits.store(0, Ordering::Relaxed);
        self.misses.store(0, Ordering::Relaxed);
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use tempfile::TempDir;

    #[test]
    fn test_lru_cache() {
        let mut cache = LruCache::new(3, Duration::from_secs(3600));

        // 创建临时图
        let graph1 = MemoryGraph::new();
        let graph2 = MemoryGraph::new();
        let graph3 = MemoryGraph::new();
        let graph4 = MemoryGraph::new();

        let path1 = PathBuf::from("/test1");
        let path2 = PathBuf::from("/test2");
        let path3 = PathBuf::from("/test3");
        let path4 = PathBuf::from("/test4");

        // 插入
        cache.insert(path1.clone(), graph1.clone(), None, 1);
        cache.insert(path2.clone(), graph2.clone(), None, 2);
        cache.insert(path3.clone(), graph3.clone(), None, 3);

        assert_eq!(cache.entries.len(), 3);

        // 访问 path1 使其成为最近使用
        let _ = cache.get(&path1);

        // 插入新项，path2 应该被淘汰
        cache.insert(path4.clone(), graph4.clone(), None, 4);

        assert_eq!(cache.entries.len(), 3);
        assert!(!cache.entries.contains_key(&path2));
        assert!(cache.entries.contains_key(&path1));
        assert!(cache.entries.contains_key(&path3));
    }

    #[test]
    fn test_cache_stats() {
        let mut cache = LruCache::new(10, Duration::from_secs(3600));

        let graph = MemoryGraph::new();
        let path = PathBuf::from("/test");

        cache.insert(path.clone(), graph.clone(), None, 1);

        // 多次访问
        let _ = cache.get(&path);
        let _ = cache.get(&path);
        let _ = cache.get(&path);

        let stats = cache.stats();
        assert_eq!(stats.entry_count, 1);
        assert_eq!(stats.total_accesses, 3);
    }
}
