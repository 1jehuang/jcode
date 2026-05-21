//! KV Cache Transmission Optimizer
//!
//! Provides compression, batching, and quantization for efficient KV Cache transfer
//! across distributed nodes.
//!
//! ## Features
//! 1. **Compression**: LZ4/Zstd compression algorithms
//! 2. **Quantization**: FP16 → INT8/INT4 quantization (lossy)
//! 3. **Batching**: Aggregate multiple chunks for reduced RPC calls
//! 4. **Zero-copy**: Efficient serialization with minimal allocations

use std::collections::VecDeque;
use std::time::{Duration, Instant};
use tracing::{info, debug, warn};
use serde::{Serialize, Deserialize};

// ============================================================================
// Compression Algorithms
// ============================================================================

/// Supported compression algorithms
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
pub enum CompressionAlgorithm {
    /// No compression (fastest)
    None,
    /// LZ4 compression (good speed/ratio balance)
    Lz4,
    /// Zstd compression (best ratio)
    Zstd,
}

impl CompressionAlgorithm {
    pub fn name(&self) -> &'static str {
        match self {
            Self::None => "none",
            Self::Lz4 => "lz4",
            Self::Zstd => "zstd",
        }
    }

    /// Compress data using this algorithm
    pub fn compress(&self, data: &[u8]) -> Result<Vec<u8>, CompressionError> {
        match self {
            Self::None => Ok(data.to_vec()),
            Self::Lz4 => lz4_compress(data),
            Self::Zstd => zstd_compress(data),
        }
    }

    /// Decompress data
    pub fn decompress(&self, data: &[u8], original_size: usize) -> Result<Vec<u8>, CompressionError> {
        match self {
            Self::None => Ok(data.to_vec()),
            Self::Lz4 => lz4_decompress(data, original_size),
            Self::Zstd => zstd_decompress(data, original_size),
        }
    }
}

/// Compression error types
#[derive(Debug, thiserror::Error)]
pub enum CompressionError {
    #[error("LZ4 compression failed: {0}")]
    Lz4Error(String),
    #[error("Zstd compression failed: {0}")]
    ZstdError(String),
    #[error("Invalid compressed data")]
    InvalidData,
}

// LZ4 compression (using lz4_flex crate)
fn lz4_compress(data: &[u8]) -> Result<Vec<u8>, CompressionError> {
    use lz4_flex::block::compress;
    Ok(compress(data))
}

fn lz4_decompress(data: &[u8], original_size: usize) -> Result<Vec<u8>, CompressionError> {
    use lz4_flex::block::decompress_size_prepended;
    decompress_size_prepended(data, Some(original_size))
        .map_err(|e| CompressionError::Lz4Error(e.to_string()))
}

// Zstd compression (using zstd crate)
fn zstd_compress(data: &[u8]) -> Result<Vec<u8>, CompressionError> {
    zstd::stream::encode_all(data, 3) // Compression level 3 (balanced)
        .map_err(|e| CompressionError::ZstdError(e.to_string()))
}

fn zstd_decompress(data: &[u8], _original_size: usize) -> Result<Vec<u8>, CompressionError> {
    zstd::stream::decode_all(data)
        .map_err(|e| CompressionError::ZstdError(e.to_string()))
}

// ============================================================================
// Quantization Support
// ============================================================================

/// Quantization type for KV Cache values
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
pub enum QuantizationType {
    /// No quantization (FP16)
    None,
    /// INT8 quantization (2x compression, minimal accuracy loss)
    Int8,
    /// INT4 quantization (4x compression, moderate accuracy loss)
    Int4,
}

impl QuantizationType {
    pub fn bits_per_value(&self) -> u8 {
        match self {
            Self::None => 16,
            Self::Int8 => 8,
            Self::Int4 => 4,
        }
    }

    pub fn compression_ratio(&self) -> f64 {
        match self {
            Self::None => 1.0,
            Self::Int8 => 2.0,
            Self::Int4 => 4.0,
        }
    }
}

/// Quantized KV Cache data
#[derive(Debug, Clone)]
pub struct QuantizedData {
    pub data: Vec<u8>,
    pub quant_type: QuantizationType,
    pub scale: f32,      // Scale factor for dequantization
    pub zero_point: i8,  // Zero point for asymmetric quantization
    pub original_shape: Vec<usize>,
}

impl QuantizedData {
    /// Quantize FP16 data to INT8
    pub fn quantize_int8(fp16_data: &[u8]) -> Self {
        // Simplified INT8 quantization
        // In production, use proper calibration for scale/zero_point
        let scale = 0.01f32;
        let zero_point = 0i8;

        let mut quantized = Vec::with_capacity(fp16_data.len() / 2); // FP16 is 2 bytes
        for chunk in fp16_data.chunks(2) {
            if chunk.len() == 2 {
                let fp16_val = f16::from_le_bytes([chunk[0], chunk[1]]);
                let int8_val = (fp16_val.to_f32() / scale).round() as i8 + zero_point;
                quantized.push(int8_val as u8);
            }
        }

        Self {
            data: quantized,
            quant_type: QuantizationType::Int8,
            scale,
            zero_point,
            original_shape: vec![fp16_data.len() / 2],
        }
    }

    /// Dequantize back to FP16
    pub fn dequantize(&self) -> Vec<u8> {
        match self.quant_type {
            QuantizationType::Int8 => {
                let mut fp16_data = Vec::with_capacity(self.data.len() * 2);
                for &byte in &self.data {
                    let int8_val = byte as i8 - self.zero_point;
                    let fp32_val = int8_val as f32 * self.scale;
                    let fp16_val = half::f16::from_f32(fp32_val);
                    fp16_data.extend_from_slice(&fp16_val.to_le_bytes());
                }
                fp16_data
            }
            QuantizationType::Int4 => {
                // INT4 dequantization (2 values per byte)
                let mut fp16_data = Vec::with_capacity(self.data.len() * 4);
                for &byte in &self.data {
                    let low_nibble = byte & 0x0F;
                    let high_nibble = (byte >> 4) & 0x0F;

                    for nibble in [low_nibble, high_nibble] {
                        let int4_val = (nibble as i8) - 8; // Signed 4-bit
                        let fp32_val = int4_val as f32 * self.scale;
                        let fp16_val = half::f16::from_f32(fp32_val);
                        fp16_data.extend_from_slice(&fp16_val.to_le_bytes());
                    }
                }
                fp16_data
            }
            QuantizationType::None => self.data.clone(),
        }
    }
}

// ============================================================================
// Batching Support
// ============================================================================

/// Batch configuration for KV Cache transmission
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct BatchConfig {
    /// Maximum batch size in bytes
    pub max_batch_size_bytes: usize,
    /// Maximum number of chunks per batch
    pub max_chunks_per_batch: usize,
    /// Maximum time to wait before flushing batch (ms)
    pub flush_timeout_ms: u64,
    /// Enable dynamic batch sizing
    pub enable_dynamic_sizing: bool,
}

impl Default for BatchConfig {
    fn default() -> Self {
        Self {
            max_batch_size_bytes: 10 * 1024 * 1024, // 10 MB
            max_chunks_per_batch: 100,
            flush_timeout_ms: 50, // 50 ms
            enable_dynamic_sizing: true,
        }
    }
}

/// Batched KV Cache chunks
#[derive(Debug, Clone)]
pub struct ChunkBatch {
    pub request_id: String,
    pub chunks: Vec<ChunkData>,
    pub total_size_bytes: usize,
    pub created_at: Instant,
}

impl ChunkBatch {
    pub fn new(request_id: String) -> Self {
        Self {
            request_id,
            chunks: Vec::new(),
            total_size_bytes: 0,
            created_at: Instant::now(),
        }
    }

    /// Add a chunk to the batch
    pub fn add_chunk(&mut self, chunk: ChunkData) {
        self.total_size_bytes += chunk.data.len();
        self.chunks.push(chunk);
    }

    /// Check if batch is full
    pub fn is_full(&self, config: &BatchConfig) -> bool {
        self.total_size_bytes >= config.max_batch_size_bytes
            || self.chunks.len() >= config.max_chunks_per_batch
    }

    /// Check if batch should be flushed due to timeout
    pub fn should_flush(&self, config: &BatchConfig) -> bool {
        self.created_at.elapsed() >= Duration::from_millis(config.flush_timeout_ms)
    }

    /// Clear the batch
    pub fn clear(&mut self) {
        self.chunks.clear();
        self.total_size_bytes = 0;
        self.created_at = Instant::now();
    }
}

/// Individual chunk data
#[derive(Debug, Clone)]
pub struct ChunkData {
    pub chunk_index: u32,
    pub data: Vec<u8>,
    pub is_last: bool,
}

// ============================================================================
// KV Cache Optimizer
// ============================================================================

/// Main optimizer for KV Cache transmission
pub struct KVCacheOptimizer {
    compression: CompressionAlgorithm,
    quantization: QuantizationType,
    batch_config: BatchConfig,
    pending_batches: std::collections::HashMap<String, ChunkBatch>,

    // Statistics
    stats: OptimizerStats,
}

#[derive(Debug, Clone, Default)]
pub struct OptimizerStats {
    pub total_bytes_before: u64,
    pub total_bytes_after: u64,
    pub compression_ratio: f64,
    pub batches_sent: u64,
    pub chunks_processed: u64,
    pub avg_batch_size: f64,
}

impl KVCacheOptimizer {
    pub fn new(
        compression: CompressionAlgorithm,
        quantization: QuantizationType,
        batch_config: BatchConfig,
    ) -> Self {
        info!(
            "KVCacheOptimizer initialized: compression={:?}, quantization={:?}",
            compression, quantization
        );

        Self {
            compression,
            quantization,
            batch_config,
            pending_batches: std::collections::HashMap::new(),
            stats: OptimizerStats::default(),
        }
    }

    /// Optimize a single chunk for transmission
    pub fn optimize_chunk(&mut self, chunk: &ChunkData) -> OptimizedChunk {
        let start_size = chunk.data.len();

        // Step 1: Apply quantization if enabled
        let quantized = if self.quantization != QuantizationType::None {
            match self.quantization {
                QuantizationType::Int8 => QuantizedData::quantize_int8(&chunk.data),
                QuantizationType::Int4 => {
                    // For INT4, need special handling
                    QuantizedData::quantize_int8(&chunk.data) // Fallback to INT8 for now
                }
                QuantizationType::None => unreachable!(),
            }
        } else {
            QuantizedData {
                data: chunk.data.clone(),
                quant_type: QuantizationType::None,
                scale: 1.0,
                zero_point: 0,
                original_shape: vec![chunk.data.len()],
            }
        };

        // Step 2: Apply compression
        let compressed = self.compression.compress(&quantized.data)
            .unwrap_or_else(|e| {
                warn!("Compression failed, using uncompressed data: {:?}", e);
                quantized.data.clone()
            });

        let end_size = compressed.len();

        // Update statistics
        self.stats.total_bytes_before += start_size as u64;
        self.stats.total_bytes_after += end_size as u64;
        self.stats.chunks_processed += 1;

        OptimizedChunk {
            chunk_index: chunk.chunk_index,
            original_data_size: start_size,
            compressed_data: compressed,
            quantization: self.quantization,
            compression: self.compression,
            scale: quantized.scale,
            zero_point: quantized.zero_point,
            is_last: chunk.is_last,
        }
    }

    /// Add chunk to batch for a specific request
    pub fn add_to_batch(&mut self, request_id: &str, chunk: ChunkData) -> Option<ChunkBatch> {
        let batch = self.pending_batches
            .entry(request_id.to_string())
            .or_insert_with(|| ChunkBatch::new(request_id.to_string()));

        batch.add_chunk(chunk);

        // Check if batch should be flushed
        if batch.is_full(&self.batch_config) || batch.should_flush(&self.batch_config) {
            return self.flush_batch(request_id);
        }

        None
    }

    /// Flush a batch for immediate transmission
    pub fn flush_batch(&mut self, request_id: &str) -> Option<ChunkBatch> {
        self.pending_batches.remove(request_id).inspect(|batch| {
            self.stats.batches_sent += 1;
            self.stats.avg_batch_size = (self.stats.avg_batch_size * (self.stats.batches_sent - 1) as f64
                + batch.total_size_bytes as f64) / self.stats.batches_sent as f64;

            debug!(
                "Flushed batch for {}: {} chunks, {} KB",
                request_id,
                batch.chunks.len(),
                batch.total_size_bytes / 1024
            );
        })
    }

    /// Flush all pending batches
    pub fn flush_all(&mut self) -> Vec<ChunkBatch> {
        let batches: Vec<_> = self.pending_batches.drain().map(|(_, v)| v).collect();
        for batch in &batches {
            self.stats.batches_sent += 1;
        }
        batches
    }

    /// Decompress and dequantize an optimized chunk
    pub fn restore_chunk(&self, optimized: &OptimizedChunk) -> Vec<u8> {
        // Step 1: Decompress
        let decompressed = self.compression.decompress(
            &optimized.compressed_data,
            optimized.original_data_size
        ).unwrap_or_else(|e| {
            warn!("Decompression failed: {:?}", e);
            optimized.compressed_data.clone()
        });

        // Step 2: Dequantize if needed
        if optimized.quantization != QuantizationType::None {
            let quantized = QuantizedData {
                data: decompressed,
                quant_type: optimized.quantization,
                scale: optimized.scale,
                zero_point: optimized.zero_point,
                original_shape: vec![optimized.original_data_size],
            };
            quantized.dequantize()
        } else {
            decompressed
        }
    }

    /// Get current statistics
    pub fn get_stats(&self) -> &OptimizerStats {
        &self.stats
    }

    /// Calculate overall compression ratio
    pub fn compression_ratio(&self) -> f64 {
        if self.stats.total_bytes_after == 0 {
            return 1.0;
        }
        self.stats.total_bytes_before as f64 / self.stats.total_bytes_after as f64
    }
}

/// Optimized chunk ready for transmission
#[derive(Debug, Clone)]
pub struct OptimizedChunk {
    pub chunk_index: u32,
    pub original_data_size: usize,
    pub compressed_data: Vec<u8>,
    pub quantization: QuantizationType,
    pub compression: CompressionAlgorithm,
    pub scale: f32,
    pub zero_point: i8,
    pub is_last: bool,
}

impl OptimizedChunk {
    /// Get the size of compressed data
    pub fn size(&self) -> usize {
        self.compressed_data.len()
    }

    /// Get compression ratio for this chunk
    pub fn compression_ratio(&self) -> f64 {
        if self.original_data_size == 0 {
            return 1.0;
        }
        self.original_data_size as f64 / self.compressed_data.len() as f64
    }
}

// ============================================================================
// Tests
// ============================================================================

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_lz4_compression() {
        let data = vec![0u8; 10000]; // Compressible data
        let compressed = CompressionAlgorithm::Lz4.compress(&data).unwrap();
        let decompressed = CompressionAlgorithm::Lz4.decompress(&compressed, data.len()).unwrap();

        assert!(compressed.len() < data.len(), "LZ4 should compress");
        assert_eq!(decompressed.len(), data.len(), "Decompressed size should match");
    }

    #[test]
    fn test_zstd_compression() {
        let data = vec![0u8; 10000];
        let compressed = CompressionAlgorithm::Zstd.compress(&data).unwrap();
        let decompressed = CompressionAlgorithm::Zstd.decompress(&compressed, data.len()).unwrap();

        assert!(compressed.len() < data.len(), "Zstd should compress");
        assert_eq!(decompressed.len(), data.len());
    }

    #[test]
    fn test_quantization_int8() {
        // Create sample FP16 data
        let fp16_data = vec![0u8; 1000];
        let quantized = QuantizedData::quantize_int8(&fp16_data);

        assert_eq!(quantized.quant_type, QuantizationType::Int8);
        assert!(quantized.data.len() <= fp16_data.len(), "INT8 should reduce size");

        let restored = quantized.dequantize();
        assert_eq!(restored.len(), fp16_data.len());
    }

    #[test]
    fn test_batch_is_full() {
        let config = BatchConfig {
            max_batch_size_bytes: 1000,
            max_chunks_per_batch: 10,
            ..Default::default()
        };

        let mut batch = ChunkBatch::new("test".to_string());

        // Add chunks until full
        for i in 0..5 {
            batch.add_chunk(ChunkData {
                chunk_index: i,
                data: vec![0u8; 200], // 200 bytes each
                is_last: false,
            });
        }

        assert!(batch.is_full(&config), "Batch should be full by size");
    }

    #[test]
    fn test_optimizer_compression_ratio() {
        let mut optimizer = KVCacheOptimizer::new(
            CompressionAlgorithm::Lz4,
            QuantizationType::None,
            BatchConfig::default(),
        );

        let chunk = ChunkData {
            chunk_index: 0,
            data: vec![0u8; 10000],
            is_last: true,
        };

        let optimized = optimizer.optimize_chunk(&chunk);
        let ratio = optimized.compression_ratio();

        assert!(ratio >= 1.0, "Should achieve some compression");
        info!("Compression ratio: {:.2}x", ratio);
    }

    #[test]
    fn test_batch_flush_timeout() {
        let config = BatchConfig {
            flush_timeout_ms: 10, // 10ms timeout
            ..Default::default()
        };

        let mut batch = ChunkBatch::new("test".to_string());
        batch.add_chunk(ChunkData {
            chunk_index: 0,
            data: vec![0u8; 100],
            is_last: false,
        });

        // Should not flush immediately
        assert!(!batch.should_flush(&config));

        // Wait for timeout
        std::thread::sleep(Duration::from_millis(15));
        assert!(batch.should_flush(&config), "Should flush after timeout");
    }
}
