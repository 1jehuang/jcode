use std::collections::HashMap;
use std::path::PathBuf;
use std::sync::Arc;
use tokio::sync::RwLock;

/// Cache entry metadata
#[derive(Debug, Clone)]
pub struct CacheEntry {
    pub key: String,
    pub paths: Vec<PathBuf>,
    pub size_bytes: u64,
    pub created_at: chrono::DateTime<chrono::Utc>,
    pub last_accessed: chrono::DateTime<chrono::Utc>,
    pub hit_count: u64,
}

/// Cache manager for pipeline artifacts/caching
#[derive(Debug, Clone)]
pub struct CacheManager {
    pub cache_dirs: Vec<PathBuf>,
    entries: Arc<RwLock<HashMap<String, CacheEntry>>>,
}

impl CacheManager {
    pub fn new(cache_dirs: Vec<PathBuf>) -> Self {
        CacheManager {
            cache_dirs,
            entries: Arc::new(RwLock::new(HashMap::new())),
        }
    }

    pub async fn get_entry(&self, key: &str) -> Option<CacheEntry> {
        let mut entries = self.entries.write().await;
        if let Some(entry) = entries.get_mut(key) {
            entry.last_accessed = chrono::Utc::now();
            entry.hit_count += 1;
            Some(entry.clone())
        } else {
            None
        }
    }

    pub async fn set_entry(&self, key: &str, paths: Vec<PathBuf>, size_bytes: u64) {
        let now = chrono::Utc::now();
        let entry = CacheEntry {
            key: key.to_string(),
            paths,
            size_bytes,
            created_at: now,
            last_accessed: now,
            hit_count: 0,
        };
        self.entries.write().await.insert(key.to_string(), entry);
    }

    pub async fn invalidate(&self, key: &str) {
        self.entries.write().await.remove(key);
    }

    pub async fn clear_all(&self) {
        self.entries.write().await.clear();
    }

    pub async fn stats(&self) -> CacheStats {
        let entries = self.entries.read().await;
        let total_size: u64 = entries.values().map(|e| e.size_bytes).sum();
        let total_hits: u64 = entries.values().map(|e| e.hit_count).sum();
        CacheStats {
            entry_count: entries.len() as u64,
            total_size_bytes: total_size,
            total_hit_count: total_hits,
        }
    }
}

#[derive(Debug, Clone)]
pub struct CacheStats {
    pub entry_count: u64,
    pub total_size_bytes: u64,
    pub total_hit_count: u64,
}