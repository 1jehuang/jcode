//! KV Cache Manager - 负责跨节点传输时的 KV Cache 组装、压缩与存储
//!
//! P1-4 Optimization: Added compression and batching support
//! - Multiple compression algorithms (LZ4, Zstd, Snappy)
//! - Quantization support (FP16 -> INT8/FP8/INT4)
//! - Batch transmission to reduce gRPC overhead

use crate::proto::{CompressionAlgorithm, KVCacheChunk, QuantizationFormat};
use crate::kv_cache_compressor::{KVCacheCompressor, KVCacheBatchTransmitter, CompressionStats};
use anyhow::Result;
use std::collections::HashMap;
use tracing::{info, debug, warn};

/// KV Cache 管理器（支持压缩和批量传输）
pub struct KVCacheManager {
    /// 存储已接收的 KV Cache (request_id -> assembled_data)
    cache_store: HashMap<String, Vec<u8>>,
    
    /// KV Cache compressor (P1-4 optimization)
    compressor: KVCacheCompressor,
    
    /// Batch transmitters per request (P1-4 optimization)
    batch_transmitters: HashMap<String, KVCacheBatchTransmitter>,
    
    /// Default compression settings
    default_compression: CompressionAlgorithm,
    default_quantization: QuantizationFormat,
    
    /// Maximum batch size for layer grouping
    max_batch_size: usize,
}

impl KVCacheManager {
    pub fn new() -> Self {
        Self {
            cache_store: HashMap::new(),
            compressor: KVCacheCompressor::new(
                CompressionAlgorithm::CompressionLz4,
                QuantizationFormat::QuantizationNone,
            ),
            batch_transmitters: HashMap::new(),
            default_compression: CompressionAlgorithm::CompressionLz4,
            default_quantization: QuantizationFormat::QuantizationNone,
            max_batch_size: 4, // Default: batch up to 4 layers
        }
    }

    /// Create with custom compression settings
    pub fn with_compression(
        compression: CompressionAlgorithm,
        quantization: QuantizationFormat,
        max_batch_size: usize,
    ) -> Self {
        info!(
            "[KVCacheManager] Initialized with compression={:?}, quantization={:?}, batch_size={}",
            compression, quantization, max_batch_size
        );

        Self {
            cache_store: HashMap::new(),
            compressor: KVCacheCompressor::new(compression, quantization),
            batch_transmitters: HashMap::new(),
            default_compression: compression,
            default_quantization: quantization,
            max_batch_size,
        }
    }

    /// 组装并存储 KV Cache 分片（旧接口，保持向后兼容）
    pub fn assemble_and_store(
        &mut self,
        request_id: &str,
        chunks: Vec<KVCacheChunk>,
    ) -> Result<()> {
        debug!("[KVCache] 组装 {} 个分片: request_id={}", chunks.len(), request_id);

        // 按 chunk_index 排序
        let mut sorted_chunks = chunks;
        sorted_chunks.sort_by_key(|c| c.chunk_index);

        // 拼接数据（处理可能的压缩）
        let mut assembled_data = Vec::new();
        for chunk in &sorted_chunks {
            let compression_algo = CompressionAlgorithm::try_from(chunk.compression_algo)
                .unwrap_or(CompressionAlgorithm::CompressionNone);

            if compression_algo != CompressionAlgorithm::CompressionNone {
                // Decompress if needed
                let decompressed = self.compressor.decompress(
                    &chunk.data,
                    compression_algo,
                    chunk.original_size_bytes,
                )?;
                assembled_data.extend_from_slice(&decompressed);
            } else {
                assembled_data.extend_from_slice(&chunk.data);
            }
        }

        info!(
            "[KVCache] 组装完成: request_id={}, total_size={}KB",
            request_id,
            assembled_data.len() / 1024
        );

        self.cache_store.insert(request_id.to_string(), assembled_data);
        Ok(())
    }

    /// Store a compressed and batched KV Cache chunk (P1-4 optimized interface)
    pub fn store_compressed_chunk(
        &mut self,
        request_id: &str,
        chunk: KVCacheChunk,
    ) -> Result<()> {
        debug!(
            "[KVCache] Storing compressed chunk: request_id={}, compression={:?}, batch_size={}",
            request_id,
            CompressionAlgorithm::try_from(chunk.compression_algo).unwrap_or(CompressionAlgorithm::CompressionNone),
            chunk.batch_size
        );

        // Verify checksum
        if !chunk.checksum.is_empty() {
            let computed_checksum = format!("{:x}", crc32_fast::crc32(&chunk.data));
            if computed_checksum != chunk.checksum {
                warn!(
                    "[KVCache] Checksum mismatch for request {}: expected={}, got={}",
                    request_id, chunk.checksum, computed_checksum
                );
                return Err(anyhow::anyhow!("Data integrity check failed"));
            }
        }

        // Extract layers from batch if needed
        let layers = self.compressor.extract_layers_from_batch(&chunk, vec![])?;

        // Assemble full data
        let mut assembled_data = Vec::new();
        for (_, layer_data) in layers {
            assembled_data.extend_from_slice(&layer_data);
        }

        self.cache_store.insert(request_id.to_string(), assembled_data);

        // Record compression stats
        if let Some(stats) = self.compressor.get_stats(request_id) {
            info!(
                "[KVCache] Stored with compression: {} -> {} ({:.1}% reduction)",
                stats.original_size_bytes / 1024,
                stats.compressed_size_bytes / 1024,
                stats.compression_percent()
            );
        }

        Ok(())
    }

    /// Add a layer to batch transmitter (P1-4 optimization)
    pub fn add_layer_to_batch(&mut self, request_id: &str, layer_index: i32, data: Vec<u8>) {
        let transmitter = self.batch_transmitters
            .entry(request_id.to_string())
            .or_insert_with(|| KVCacheBatchTransmitter::new(self.max_batch_size));

        transmitter.add_layer(layer_index, data);
    }

    /// Flush batch and get ready-to-send chunk (P1-4 optimization)
    pub fn flush_batch(&mut self, request_id: &str) -> Result<Option<KVCacheChunk>> {
        if let Some(transmitter) = self.batch_transmitters.get_mut(request_id) {
            if let Some(chunk) = transmitter.flush_batch(request_id)? {
                return Ok(Some(chunk));
            }
        }
        Ok(None)
    }

    /// Check if batch is ready to send (P1-4 optimization)
    pub fn is_batch_ready(&self, request_id: &str) -> bool {
        self.batch_transmitters
            .get(request_id)
            .map(|t| t.is_batch_ready())
            .unwrap_or(false)
    }

    /// 获取已存储的 KV Cache
    pub fn get_cache(&self, request_id: &str) -> Option<&Vec<u8>> {
        self.cache_store.get(request_id)
    }

    /// 清理过期缓存
    pub fn evict_cache(&mut self, request_id: &str) -> Option<Vec<u8>> {
        // Also clean up batch transmitter
        self.batch_transmitters.remove(request_id);
        self.cache_store.remove(request_id)
    }

    /// 获取缓存统计信息
    pub fn get_stats(&self) -> CacheStats {
        let total_size_bytes: usize = self.cache_store.values().map(|v| v.len()).sum();

        // Get compression summary stats
        let compression_summary = self.compressor.get_summary_stats();
        let avg_reduction = compression_summary
            .get("avg_reduction_percent")
            .copied()
            .unwrap_or(0.0);

        CacheStats {
            cached_requests: self.cache_store.len(),
            total_size_mb: total_size_bytes as f64 / 1024.0 / 1024.0,
            avg_compression_reduction_percent: avg_reduction,
        }
    }

    /// Get detailed compression statistics
    pub fn get_compression_stats(&self, request_id: &str) -> Option<&CompressionStats> {
        self.compressor.get_stats(request_id)
    }
}

/// 缓存统计信息
#[derive(Debug, Clone)]
pub struct CacheStats {
    pub cached_requests: usize,
    pub total_size_mb: f64,
    /// Average compression reduction percentage (P1-4 optimization)
    pub avg_compression_reduction_percent: f64,
}
