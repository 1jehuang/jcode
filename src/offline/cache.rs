//! SQLite Local Cache Layer for Offline Mode
//!
//! Provides local caching of:
//! - Vector embeddings (for RAG retrieval)
//! - Code snippets and symbols
//! - MCP tool results
//! - Conversation history
//!
//! When offline, automatically falls back to local cache instead of remote services.

use rusqlite::{Connection, Result as SqliteResult, params};
use serde::{Deserialize, Serialize};
use std::path::PathBuf;
use std::sync::Arc;
use tokio::sync::RwLock;
use tracing::{info, warn, debug};

/// Cache entry with metadata
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct CacheEntry {
    pub key: String,
    pub value: Vec<u8>, // Serialized data
    pub embedding: Option<Vec<f32>>, // Optional vector embedding
    pub created_at: i64, // Unix timestamp
    pub expires_at: Option<i64>, // Optional expiry
    pub access_count: u32,
}

/// Local cache manager using SQLite
pub struct LocalCache {
    db_path: PathBuf,
    conn: Arc<RwLock<Connection>>,
    max_entries: usize,
}

impl LocalCache {
    /// Create or open local cache database
    pub fn new(db_path: PathBuf, max_entries: usize) -> Result<Self, Box<dyn std::error::Error>> {
        let conn = Connection::open(&db_path)?;
        
        // Initialize schema
        conn.execute_batch(
            r#"
            CREATE TABLE IF NOT EXISTS cache_entries (
                key TEXT PRIMARY KEY,
                value BLOB NOT NULL,
                embedding BLOB,
                created_at INTEGER NOT NULL,
                expires_at INTEGER,
                access_count INTEGER DEFAULT 0
            );
            
            CREATE INDEX IF NOT EXISTS idx_created_at ON cache_entries(created_at);
            CREATE INDEX IF NOT EXISTS idx_expires_at ON cache_entries(expires_at);
            "#
        )?;
        
        info!("Local cache initialized at {:?}", db_path);
        
        Ok(Self {
            db_path,
            conn: Arc::new(RwLock::new(conn)),
            max_entries,
        })
    }

    /// Store a cache entry
    pub async fn put(&self, key: &str, value: Vec<u8>, embedding: Option<Vec<f32>>, ttl_seconds: Option<i64>) -> SqliteResult<()> {
        let conn = self.conn.write().await;
        
        let now = chrono::Utc::now().timestamp();
        let expires_at = ttl_seconds.map(|ttl| now + ttl);
        
        conn.execute(
            "INSERT OR REPLACE INTO cache_entries 
             (key, value, embedding, created_at, expires_at, access_count) 
             VALUES (?1, ?2, ?3, ?4, ?5, 0)",
            params![
                key,
                value,
                embedding.map(|e| bincode::serialize(&e).unwrap()),
                now,
                expires_at,
            ],
        )?;
        
        // Enforce max entries limit
        self.enforce_limit().await?;
        
        debug!("Cached entry: {} ({} bytes)", key, value.len());
        Ok(())
    }

    /// Retrieve a cache entry
    pub async fn get(&self, key: &str) -> SqliteResult<Option<CacheEntry>> {
        let mut conn = self.conn.write().await;
        
        let mut stmt = conn.prepare(
            "SELECT key, value, embedding, created_at, expires_at, access_count 
             FROM cache_entries WHERE key = ?1"
        )?;
        
        let result = stmt.query_row(params![key], |row| {
            let embedding_bytes: Option<Vec<u8>> = row.get(2)?;
            let embedding = embedding_bytes.map(|bytes| {
                bincode::deserialize::<Vec<f32>>(&bytes).unwrap_or_default()
            });
            
            Ok(CacheEntry {
                key: row.get(0)?,
                value: row.get(1)?,
                embedding,
                created_at: row.get(3)?,
                expires_at: row.get(4)?,
                access_count: row.get(5)?,
            })
        }).optional()?;
        
        if let Some(ref entry) = result {
            // Check expiry
            if let Some(expires) = entry.expires_at {
                let now = chrono::Utc::now().timestamp();
                if now > expires {
                    // Expired, delete and return None
                    self.delete(key).await?;
                    return Ok(None);
                }
            }
            
            // Increment access count
            conn.execute(
                "UPDATE cache_entries SET access_count = access_count + 1 WHERE key = ?1",
                params![key],
            )?;
        }
        
        Ok(result)
    }

    /// Search by vector similarity (HNSW integration placeholder)
    pub async fn search_similar(
        &self,
        query_embedding: &[f32],
        top_k: usize,
        threshold: f32,
    ) -> SqliteResult<Vec<CacheEntry>> {
        let conn = self.conn.read().await;
        
        // TODO: Integrate HNSW index for efficient similarity search
        // For now, use simple cosine similarity on all entries with embeddings
        
        let mut stmt = conn.prepare(
            "SELECT key, value, embedding, created_at, expires_at, access_count 
             FROM cache_entries WHERE embedding IS NOT NULL"
        )?;
        
        let mut entries = Vec::new();
        let mut rows = stmt.query(params![])?;
        
        while let Some(row) = rows.next()? {
            let embedding_bytes: Vec<u8> = row.get(2)?;
            if let Ok(embedding) = bincode::deserialize::<Vec<f32>>(&embedding_bytes) {
                let similarity = Self::cosine_similarity(query_embedding, &embedding);
                
                if similarity >= threshold {
                    entries.push((
                        CacheEntry {
                            key: row.get(0)?,
                            value: row.get(1)?,
                            embedding: Some(embedding),
                            created_at: row.get(3)?,
                            expires_at: row.get(4)?,
                            access_count: row.get(5)?,
                        },
                        similarity,
                    ));
                }
            }
        }
        
        // Sort by similarity descending and take top_k
        entries.sort_by(|a, b| b.1.partial_cmp(&a.1).unwrap_or(std::cmp::Ordering::Equal));
        let result: Vec<CacheEntry> = entries.into_iter().take(top_k).map(|(e, _)| e).collect();
        
        Ok(result)
    }

    /// Delete a cache entry
    pub async fn delete(&self, key: &str) -> SqliteResult<()> {
        let conn = self.conn.write().await;
        conn.execute("DELETE FROM cache_entries WHERE key = ?1", params![key])?;
        Ok(())
    }

    /// Clear expired entries
    pub async fn clear_expired(&self) -> SqliteResult<usize> {
        let conn = self.conn.write().await;
        let now = chrono::Utc::now().timestamp();
        
        let deleted = conn.execute(
            "DELETE FROM cache_entries WHERE expires_at IS NOT NULL AND expires_at < ?1",
            params![now],
        )?;
        
        if deleted > 0 {
            info!("Cleared {} expired cache entries", deleted);
        }
        
        Ok(deleted)
    }

    /// Get cache statistics
    pub async fn stats(&self) -> SqliteResult<CacheStats> {
        let conn = self.conn.read().await;
        
        let total_entries: usize = conn.query_row(
            "SELECT COUNT(*) FROM cache_entries",
            params![],
            |row| row.get(0),
        )?;
        
        let total_size: usize = conn.query_row(
            "SELECT COALESCE(SUM(length(value)), 0) FROM cache_entries",
            params![],
            |row| row.get(0),
        )?;
        
        let with_embeddings: usize = conn.query_row(
            "SELECT COUNT(*) FROM cache_entries WHERE embedding IS NOT NULL",
            params![],
            |row| row.get(0),
        )?;
        
        Ok(CacheStats {
            total_entries,
            total_size_bytes: total_size,
            entries_with_embeddings: with_embeddings,
        })
    }

    /// Enforce maximum entry limit (LRU eviction)
    async fn enforce_limit(&self) -> SqliteResult<()> {
        let conn = self.conn.write().await;
        
        let count: usize = conn.query_row(
            "SELECT COUNT(*) FROM cache_entries",
            params![],
            |row| row.get(0),
        )?;
        
        if count > self.max_entries {
            let to_delete = count - self.max_entries;
            
            conn.execute(
                "DELETE FROM cache_entries WHERE key IN (
                    SELECT key FROM cache_entries 
                    ORDER BY access_count ASC, created_at ASC 
                    LIMIT ?1
                )",
                params![to_delete],
            )?;
            
            debug!("Evicted {} old cache entries", to_delete);
        }
        
        Ok(())
    }

    /// Calculate cosine similarity between two vectors
    fn cosine_similarity(a: &[f32], b: &[f32]) -> f32 {
        if a.len() != b.len() || a.is_empty() {
            return 0.0;
        }
        
        let dot_product: f32 = a.iter().zip(b.iter()).map(|(x, y)| x * y).sum();
        let norm_a: f32 = a.iter().map(|x| x * x).sum::<f32>().sqrt();
        let norm_b: f32 = b.iter().map(|x| x * x).sum::<f32>().sqrt();
        
        if norm_a == 0.0 || norm_b == 0.0 {
            return 0.0;
        }
        
        dot_product / (norm_a * norm_b)
    }
}

/// Cache statistics
#[derive(Debug, Clone)]
pub struct CacheStats {
    pub total_entries: usize,
    pub total_size_bytes: usize,
    pub entries_with_embeddings: usize,
}

/// Offline mode manager
pub struct OfflineModeManager {
    cache: Arc<LocalCache>,
    is_online: Arc<RwLock<bool>>,
}

impl OfflineModeManager {
    pub fn new(cache: Arc<LocalCache>) -> Self {
        Self {
            cache,
            is_online: Arc::new(RwLock::new(true)),
        }
    }

    /// Check if currently online
    pub async fn is_online(&self) -> bool {
        *self.is_online.read().await
    }

    /// Set online/offline status
    pub async fn set_online(&self, online: bool) {
        let mut status = self.is_online.write().await;
        *status = online;
        
        if online {
            info!("Switched to online mode");
            // Sync local changes to remote (placeholder)
        } else {
            warn!("Switched to offline mode - using local cache only");
        }
    }

    /// Auto-detect network status and switch mode
    pub async fn auto_detect_mode(&self) {
        let online = Self::check_connectivity().await;
        self.set_online(online).await;
    }

    /// Check if remote services are reachable
    async fn check_connectivity() -> bool {
        // Try to ping a known endpoint
        match tokio::time::timeout(
            std::time::Duration::from_secs(2),
            reqwest::get("https://www.google.com/generate_204"),
        ).await {
            Ok(Ok(resp)) => resp.status().is_success(),
            _ => false,
        }
    }

    /// Get cache reference
    pub fn cache(&self) -> Arc<LocalCache> {
        self.cache.clone()
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use tempfile::tempdir;

    #[tokio::test]
    async fn test_cache_put_get() {
        let temp_dir = tempdir().unwrap();
        let db_path = temp_dir.path().join("cache.db");
        
        let cache = LocalCache::new(db_path, 1000).unwrap();
        
        let key = "test_key";
        let value = b"test_value".to_vec();
        
        cache.put(key, value.clone(), None, None).await.unwrap();
        
        let retrieved = cache.get(key).await.unwrap();
        assert!(retrieved.is_some());
        assert_eq!(retrieved.unwrap().value, value);
    }

    #[tokio::test]
    async fn test_cache_expiry() {
        let temp_dir = tempdir().unwrap();
        let db_path = temp_dir.path().join("cache.db");
        
        let cache = LocalCache::new(db_path, 1000).unwrap();
        
        let key = "expiring_key";
        let value = b"test_value".to_vec();
        
        // Set TTL to 1 second
        cache.put(key, value.clone(), None, Some(1)).await.unwrap();
        
        // Should be retrievable immediately
        assert!(cache.get(key).await.unwrap().is_some());
        
        // Wait for expiry
        tokio::time::sleep(tokio::time::Duration::from_secs(2)).await;
        
        // Should be expired now
        assert!(cache.get(key).await.unwrap().is_none());
    }

    #[tokio::test]
    async fn test_vector_search() {
        let temp_dir = tempdir().unwrap();
        let db_path = temp_dir.path().join("cache.db");
        
        let cache = LocalCache::new(db_path, 1000).unwrap();
        
        // Insert entries with embeddings
        let embedding1 = vec![1.0, 0.0, 0.0];
        let embedding2 = vec![0.0, 1.0, 0.0];
        let embedding3 = vec![1.0, 0.1, 0.0]; // Similar to embedding1
        
        cache.put("entry1", b"value1".to_vec(), Some(embedding1), None).await.unwrap();
        cache.put("entry2", b"value2".to_vec(), Some(embedding2), None).await.unwrap();
        cache.put("entry3", b"value3".to_vec(), Some(embedding3), None).await.unwrap();
        
        // Search for similar to embedding1
        let query = vec![1.0, 0.05, 0.0];
        let results = cache.search_similar(&query, 2, 0.9).await.unwrap();
        
        assert_eq!(results.len(), 2);
        assert_eq!(results[0].key, "entry1"); // Most similar
        assert_eq!(results[1].key, "entry3"); // Second most similar
    }
}
