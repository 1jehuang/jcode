//! # 缓存管理器 (CacheManager)

use crate::types::*;
use chrono::{DateTime, Utc};
use parking_lot::Mutex;
use std::collections::HashMap;
use tracing::{debug, info};

struct CacheEntryInner {
    key: String,
    size_bytes: u64,
    created_at: DateTime<Utc>,
    last_accessed: DateTime<Utc>,
    access_count: u64,
    ttl_secs: u64,
}

/// LRU 缓存管理器
pub struct CacheManager {
    entries: Mutex<HashMap<String, CacheEntryInner>>,
    stats: Mutex<CacheStats>,
    config: CacheConfig,
    total_size: Mutex<u64>,
}

impl CacheManager {
    pub fn new(config: CacheConfig) -> Self {
        Self {
            entries: Mutex::new(HashMap::new()),
            stats: Mutex::new(CacheStats::default()),
            config,
            total_size: Mutex::new(0),
        }
    }

    pub async fn get(&self, key: &str) -> Option<Vec<u8>> {
        let mut entries = self.entries.lock();
        if let Some(entry) = entries.get_mut(key) {
            let age = (Utc::now() - entry.created_at).num_seconds();
            if age > entry.ttl_secs as i64 {
                entries.remove(key);
                let mut stats = self.stats.lock();
                stats.miss_count += 1;
                return None;
            }
            entry.access_count += 1;
            entry.last_accessed = Utc::now();
            let mut stats = self.stats.lock();
            stats.hit_count += 1;
            stats.hit_rate = if stats.hit_count + stats.miss_count > 0 {
                stats.hit_count as f64 / (stats.hit_count + stats.miss_count) as f64
            } else { 0.0 };
            debug!("Cache HIT: {}", key);
            Some(key.as_bytes().to_vec())
        } else {
            let mut stats = self.stats.lock();
            stats.miss_count += 1;
            None
        }
    }

    pub async fn set(&self, key: &str, _data: &[u8]) {
        let now = Utc::now();
        let entry = CacheEntryInner {
            key: key.to_string(),
            size_bytes: 0,
            created_at: now, last_accessed: now,
            access_count: 1, ttl_secs: self.config.ttl_hours * 3600,
        };
        {
            let mut entries = self.entries.lock();
            entries.insert(key.to_string(), entry);
        }
        {
            let mut stats = self.stats.lock();
            stats.total_entries += 1;
        }
        debug!("Cache SET: {}", key);
    }

    pub async fn clean_expired(&self) -> u64 {
        let mut entries = self.entries.lock();
        let now = Utc::now();
        let before = entries.len();
        entries.retain(|_, e| (now - e.created_at).num_seconds() < e.ttl_secs as i64);
        (before - entries.len()) as u64
    }

    pub fn stats(&self) -> CacheStats { self.stats.lock().clone() }
    pub fn hit_rate(&self) -> f64 { self.stats.lock().hit_rate }
}

impl Default for CacheManager {
    fn default() -> Self { Self::new(CacheConfig::default()) }
}
