//! KV Cache 管理器 — 负责跨节点传输时的 KV Cache 组装与存储

use crate::proto::KVCacheChunk;
use anyhow::Result;
use std::collections::HashMap;
use tracing::{info, debug};

/// KV Cache 管理器
pub struct KVCacheManager {
    /// 存储已接收的 KV Cache (request_id -> assembled_data)
    cache_store: HashMap<String, Vec<u8>>,
}

impl KVCacheManager {
    pub fn new() -> Self {
        Self {
            cache_store: HashMap::new(),
        }
    }

    /// 组装并存储 KV Cache 分片
    pub fn assemble_and_store(
        &mut self,
        request_id: &str,
        chunks: Vec<KVCacheChunk>,
    ) -> Result<()> {
        debug!("[KVCache] 组装 {} 个分片: request_id={}", chunks.len(), request_id);

        // 按 chunk_index 排序
        let mut sorted_chunks = chunks;
        sorted_chunks.sort_by_key(|c| c.chunk_index);

        // 拼接数据
        let mut assembled_data = Vec::new();
        for chunk in &sorted_chunks {
            assembled_data.extend_from_slice(&chunk.data);
        }

        info!(
            "[KVCache] 组装完成: request_id={}, total_size={}KB",
            request_id,
            assembled_data.len() / 1024
        );

        self.cache_store.insert(request_id.to_string(), assembled_data);
        Ok(())
    }

    /// 获取已存储的 KV Cache
    pub fn get_cache(&self, request_id: &str) -> Option<&Vec<u8>> {
        self.cache_store.get(request_id)
    }

    /// 清理过期缓存
    pub fn evict_cache(&mut self, request_id: &str) -> Option<Vec<u8>> {
        self.cache_store.remove(request_id)
    }

    /// 获取缓存统计信息
    pub fn get_stats(&self) -> CacheStats {
        let total_size_bytes: usize = self.cache_store.values().map(|v| v.len()).sum();
        CacheStats {
            cached_requests: self.cache_store.len(),
            total_size_mb: total_size_bytes as f64 / 1024.0 / 1024.0,
        }
    }
}

/// 缓存统计信息
#[derive(Debug, Clone)]
pub struct CacheStats {
    pub cached_requests: usize,
    pub total_size_mb: f64,
}
