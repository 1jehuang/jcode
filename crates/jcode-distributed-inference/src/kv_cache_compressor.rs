//! KV Cache Compressor - Compression and batching for efficient transmission
//!
//! This module provides:
//! 1. Multiple compression algorithms (LZ4, Zstd, Snappy)
//! 2. Quantization support (FP16 -> INT8/FP8/INT4)
//! 3. Batch transmission to reduce gRPC call overhead
//! 4. Zero-copy serialization where possible

use crate::proto::{CompressionAlgorithm, KVCacheChunk, QuantizationFormat};
use anyhow::{Context, Result};
use std::collections::HashMap;
use tracing::{debug, info, warn};

/// Compression statistics
#[derive(Debug, Clone)]
pub struct CompressionStats {
    pub original_size_bytes: u64,
    pub compressed_size_bytes: u64,
    pub compression_ratio: f64,
    pub compression_time_ms: f64,
    pub algorithm: String,
}

impl CompressionStats {
    pub fn new() -> Self {
        Self {
            original_size_bytes: 0,
            compressed_size_bytes: 0,
            compression_ratio: 0.0,
            compression_time_ms: 0.0,
            algorithm: "none".to_string(),
        }
    }

    pub fn compression_percent(&self) -> f64 {
        if self.original_size_bytes == 0 {
            return 0.0;
        }
        (1.0 - self.compressed_size_bytes as f64 / self.original_size_bytes as f64) * 100.0
    }
}

/// KV Cache compressor with multiple algorithm support
pub struct KVCacheCompressor {
    /// Default compression algorithm
    default_algo: CompressionAlgorithm,
    /// Default quantization format
    default_quantization: QuantizationFormat,
    /// Compression statistics tracker
    stats: HashMap<String, CompressionStats>, // request_id -> stats
}

impl KVCacheCompressor {
    pub fn new(
        default_algo: CompressionAlgorithm,
        default_quantization: QuantizationFormat,
    ) -> Self {
        info!(
            "[KVCacheCompressor] Initialized with algo={:?}, quantization={:?}",
            default_algo, default_quantization
        );

        Self {
            default_algo,
            default_quantization,
            stats: HashMap::new(),
        }
    }

    /// Compress KV Cache data using specified algorithm
    pub fn compress(
        &mut self,
        data: &[u8],
        algo: CompressionAlgorithm,
    ) -> Result<(Vec<u8>, CompressionStats)> {
        let start_time = std::time::Instant::now();
        let original_size = data.len() as u64;

        let (compressed, algo_name) = match algo {
            CompressionAlgorithm::CompressionNone => {
                debug!("[KVCacheCompressor] No compression applied");
                (data.to_vec(), "none")
            }
            CompressionAlgorithm::CompressionLz4 => {
                self.compress_lz4(data)?
            }
            CompressionAlgorithm::CompressionZstd => {
                self.compress_zstd(data)?
            }
            CompressionAlgorithm::CompressionSnappy => {
                self.compress_snappy(data)?
            }
        };

        let elapsed = start_time.elapsed();
        let compressed_size = compressed.len() as u64;
        let ratio = if original_size > 0 {
            compressed_size as f64 / original_size as f64
        } else {
            0.0
        };

        let stats = CompressionStats {
            original_size_bytes: original_size,
            compressed_size_bytes: compressed_size,
            compression_ratio: ratio,
            compression_time_ms: elapsed.as_secs_f64() * 1000.0,
            algorithm: algo_name.to_string(),
        };

        info!(
            "[KVCacheCompressor] {} compression: {}KB -> {}KB ({:.1}% reduction, {:.2}ms)",
            algo_name,
            original_size / 1024,
            compressed_size / 1024,
            stats.compression_percent(),
            stats.compression_time_ms
        );

        Ok((compressed, stats))
    }

    /// Decompress KV Cache data
    pub fn decompress(
        &self,
        compressed_data: &[u8],
        algo: CompressionAlgorithm,
        expected_original_size: u64,
    ) -> Result<Vec<u8>> {
        match algo {
            CompressionAlgorithm::CompressionNone => {
                Ok(compressed_data.to_vec())
            }
            CompressionAlgorithm::CompressionLz4 => {
                self.decompress_lz4(compressed_data, expected_original_size)
            }
            CompressionAlgorithm::CompressionZstd => {
                self.decompress_zstd(compressed_data)
            }
            CompressionAlgorithm::CompressionSnappy => {
                self.decompress_snappy(compressed_data)
            }
        }
    }

    /// Apply quantization to FP16 data
    pub fn quantize(
        &self,
        fp16_data: &[u8],
        format: QuantizationFormat,
    ) -> Result<(Vec<u8>, Vec<u8>)> {
        // Returns (quantized_data, scale_factors)
        match format {
            QuantizationFormat::QuantizationNone => {
                Ok((fp16_data.to_vec(), vec![]))
            }
            QuantizationFormat::QuantizationInt8 => {
                self.quantize_int8(fp16_data)
            }
            QuantizationFormat::QuantizationFp8 => {
                self.quantize_fp8(fp16_data)
            }
            QuantizationFormat::QuantizationInt4 => {
                self.quantize_int4(fp16_data)
            }
        }
    }

    /// Dequantize data back to FP16
    pub fn dequantize(
        &self,
        quantized_data: &[u8],
        scale_factors: &[u8],
        format: QuantizationFormat,
    ) -> Result<Vec<u8>> {
        match format {
            QuantizationFormat::QuantizationNone => {
                Ok(quantized_data.to_vec())
            }
            QuantizationFormat::QuantizationInt8 => {
                self.dequantize_int8(quantized_data, scale_factors)
            }
            QuantizationFormat::QuantizationFp8 => {
                self.dequantize_fp8(quantized_data)
            }
            QuantizationFormat::QuantizationInt4 => {
                self.dequantize_int4(quantized_data, scale_factors)
            }
        }
    }

    /// Create a batched KV Cache chunk from multiple layers
    pub fn create_batch_chunk(
        &mut self,
        request_id: &str,
        layer_chunks: Vec<(i32, Vec<u8>)>, // (layer_index, data)
        compression_algo: CompressionAlgorithm,
        quantization_format: QuantizationFormat,
    ) -> Result<KVCacheChunk> {
        let num_layers = layer_chunks.len();
        debug!(
            "[KVCacheCompressor] Creating batch chunk: {} layers, request_id={}",
            num_layers, request_id
        );

        // Collect layer indices
        let layer_indices: Vec<i32> = layer_chunks.iter().map(|(idx, _)| *idx).collect();

        // Concatenate all layer data
        let mut combined_data = Vec::new();
        for (_, data) in &layer_chunks {
            combined_data.extend_from_slice(data);
        }

        // Apply quantization if needed
        let (quantized_data, scale_factors) = if quantization_format != QuantizationFormat::QuantizationNone {
            self.quantize(&combined_data, quantization_format)?
        } else {
            (combined_data.clone(), vec![])
        };

        // Apply compression
        let (compressed_data, stats) = self.compress(&quantized_data, compression_algo)?;

        // Store stats
        self.stats.insert(request_id.to_string(), stats);

        // Calculate checksum (CRC32)
        let checksum = format!("{:08x}", crc32fast::hash(&compressed_data));

        let chunk = KVCacheChunk {
            request_id: request_id.to_string(),
            layer_index: layer_indices[0], // Primary layer index
            chunk_index: 0,
            data: compressed_data,
            compression_algo: compression_algo as i32,
            quantization_format: quantization_format as i32,
            original_size_bytes: combined_data.len() as u64,
            batch_size: num_layers as i32,
            layer_indices,
            is_last: true,
            checksum,
        };

        info!(
            "[KVCacheCompressor] Batch chunk created: {} layers, {}KB total",
            num_layers,
            chunk.data.len() / 1024
        );

        Ok(chunk)
    }

    /// Extract individual layers from a batch chunk
    pub fn extract_layers_from_batch(
        &self,
        chunk: &KVCacheChunk,
        layer_sizes: Vec<usize>, // Expected size for each layer
    ) -> Result<Vec<(i32, Vec<u8>)>> {
        if chunk.batch_size <= 1 {
            // Not a batch, return as single layer
            let decompressed = self.decompress(
                &chunk.data,
                CompressionAlgorithm::try_from(chunk.compression_algo).unwrap_or(CompressionAlgorithm::CompressionNone),
                chunk.original_size_bytes,
            )?;

            let dequantized = self.dequantize(
                &decompressed,
                &[],
                QuantizationFormat::try_from(chunk.quantization_format).unwrap_or(QuantizationFormat::QuantizationNone),
            )?;

            return Ok(vec![(chunk.layer_index, dequantized)]);
        }

        // Verify checksum
        let computed_checksum = format!("{:08x}", crc32fast::hash(&chunk.data));
        if computed_checksum != chunk.checksum {
            warn!(
                "[KVCacheCompressor] Checksum mismatch: expected={}, got={}",
                chunk.checksum, computed_checksum
            );
            return Err(anyhow::anyhow!("Data integrity check failed"));
        }

        // Decompress
        let compression_algo = CompressionAlgorithm::try_from(chunk.compression_algo)
            .unwrap_or(CompressionAlgorithm::CompressionNone);

        let decompressed = self.decompress(
            &chunk.data,
            compression_algo,
            chunk.original_size_bytes,
        )?;

        // Dequantize
        let quant_format = QuantizationFormat::try_from(chunk.quantization_format)
            .unwrap_or(QuantizationFormat::QuantizationNone);

        let dequantized = self.dequantize(
            &decompressed,
            &[],
            quant_format,
        )?;

        // Split into individual layers
        let mut layers = Vec::new();
        let mut offset = 0;

        for (i, &layer_idx) in chunk.layer_indices.iter().enumerate() {
            let expected_size = if i < layer_sizes.len() {
                layer_sizes[i]
            } else {
                // Estimate equal distribution if sizes not provided
                dequantized.len() / chunk.layer_indices.len()
            };

            let end = std::cmp::min(offset + expected_size, dequantized.len());
            let layer_data = dequantized[offset..end].to_vec();
            layers.push((layer_idx, layer_data));
            offset = end;
        }

        debug!(
            "[KVCacheCompressor] Extracted {} layers from batch chunk",
            layers.len()
        );

        Ok(layers)
    }

    /// Get compression statistics for a request
    pub fn get_stats(&self, request_id: &str) -> Option<&CompressionStats> {
        self.stats.get(request_id)
    }

    /// Get overall statistics summary
    pub fn get_summary_stats(&self) -> HashMap<String, f64> {
        if self.stats.is_empty() {
            return HashMap::new();
        }

        let total_original: u64 = self.stats.values().map(|s| s.original_size_bytes).sum();
        let total_compressed: u64 = self.stats.values().map(|s| s.compressed_size_bytes).sum();
        let avg_ratio = if total_original > 0 {
            total_compressed as f64 / total_original as f64
        } else {
            0.0
        };

        let mut summary = HashMap::new();
        summary.insert("total_requests".to_string(), self.stats.len() as f64);
        summary.insert("total_original_mb".to_string(), total_original as f64 / 1024.0 / 1024.0);
        summary.insert("total_compressed_mb".to_string(), total_compressed as f64 / 1024.0 / 1024.0);
        summary.insert("avg_compression_ratio".to_string(), avg_ratio);
        summary.insert("avg_reduction_percent".to_string(), (1.0 - avg_ratio) * 100.0);

        summary
    }

    // ========================================================================
    // Private compression methods
    // ========================================================================

    #[cfg(feature = "lz4")]
    fn compress_lz4(&self, data: &[u8]) -> Result<(Vec<u8>, &'static str)> {
        use lz4_flex::block::compress;
        let compressed = compress(data);
        Ok((compressed, "lz4"))
    }

    #[cfg(not(feature = "lz4"))]
    fn compress_lz4(&self, data: &[u8]) -> Result<(Vec<u8>, &'static str)> {
        warn!("[KVCacheCompressor] LZ4 feature not enabled, falling back to no compression");
        Ok((data.to_vec(), "none"))
    }

    #[cfg(feature = "zstd")]
    fn compress_zstd(&self, data: &[u8]) -> Result<(Vec<u8>, &'static str)> {
        use zstd::stream::encode_all;
        let compressed = encode_all(data, 3)?; // Compression level 3 (balanced)
        Ok((compressed, "zstd"))
    }

    #[cfg(not(feature = "zstd"))]
    fn compress_zstd(&self, data: &[u8]) -> Result<(Vec<u8>, &'static str)> {
        warn!("[KVCacheCompressor] Zstd feature not enabled, falling back to no compression");
        Ok((data.to_vec(), "none"))
    }

    #[cfg(feature = "snap")]
    fn compress_snappy(&self, data: &[u8]) -> Result<(Vec<u8>, &'static str)> {
        use snap::raw::Encoder;
        let mut encoder = Encoder::new();
        let compressed = encoder.compress_vec(data)?;
        Ok((compressed, "snappy"))
    }

    #[cfg(not(feature = "snap"))]
    fn compress_snappy(&self, data: &[u8]) -> Result<(Vec<u8>, &'static str)> {
        warn!("[KVCacheCompressor] Snappy feature not enabled, falling back to no compression");
        Ok((data.to_vec(), "none"))
    }

    #[cfg(feature = "lz4")]
    fn decompress_lz4(&self, data: &[u8], expected_size: u64) -> Result<Vec<u8>> {
        use lz4_flex::block::decompress_size_prepended;
        let decompressed = decompress_size_prepended(data)
            .context("LZ4 decompression failed")?;
        Ok(decompressed)
    }

    #[cfg(not(feature = "lz4"))]
    fn decompress_lz4(&self, data: &[u8], _expected_size: u64) -> Result<Vec<u8>> {
        Ok(data.to_vec())
    }

    #[cfg(feature = "zstd")]
    fn decompress_zstd(&self, data: &[u8]) -> Result<Vec<u8>> {
        use zstd::stream::decode_all;
        let decompressed = decode_all(data)?;
        Ok(decompressed)
    }

    #[cfg(not(feature = "zstd"))]
    fn decompress_zstd(&self, data: &[u8]) -> Result<Vec<u8>> {
        Ok(data.to_vec())
    }

    #[cfg(feature = "snap")]
    fn decompress_snappy(&self, data: &[u8]) -> Result<Vec<u8>> {
        use snap::raw::Decoder;
        let mut decoder = Decoder::new();
        let decompressed = decoder.decompress_vec(data)?;
        Ok(decompressed)
    }

    #[cfg(not(feature = "snap"))]
    fn decompress_snappy(&self, data: &[u8]) -> Result<Vec<u8>> {
        Ok(data.to_vec())
    }

    // ========================================================================
    // Private quantization methods (simplified implementations)
    // ========================================================================

    /// INT8 quantization: FP16 -> INT8 with per-channel scaling
    fn quantize_int8(&self, fp16_data: &[u8]) -> Result<(Vec<u8>, Vec<u8>)> {
        // Simplified: In production, use proper FP16 parsing and quantization
        // This is a placeholder that demonstrates the concept
        let num_elements = fp16_data.len() / 2; // FP16 is 2 bytes
        let mut int8_data = Vec::with_capacity(num_elements);
        let mut scales = Vec::with_capacity(num_elements / 128); // Per 128 elements

        // Simple uniform quantization (production should use proper FP16 decoding)
        let max_val = 127.0f32;
        for i in 0..num_elements {
            // Read FP16 value (simplified - assuming it's already in range [-1, 1])
            let val = if i * 2 + 1 < fp16_data.len() {
                ((fp16_data[i * 2] as u16 | (fp16_data[i * 2 + 1] as u16) << 8) as f32) / 32768.0
            } else {
                0.0
            };

            // Quantize to INT8
            let quantized = (val * max_val).clamp(-127.0, 127.0) as i8;
            int8_data.push(quantized as u8);
        }

        Ok((int8_data, scales))
    }

    fn dequantize_int8(&self, int8_data: &[u8], _scales: &[u8]) -> Result<Vec<u8>> {
        // Convert INT8 back to FP16 (simplified)
        let mut fp16_data = Vec::with_capacity(int8_data.len() * 2);

        for &val in int8_data {
            let fval = (val as i8) as f32 / 127.0;
            // Convert to FP16 representation (simplified)
            let fp16_bits = (fval * 32768.0) as u16;
            fp16_data.push((fp16_bits & 0xFF) as u8);
            fp16_data.push(((fp16_bits >> 8) & 0xFF) as u8);
        }

        Ok(fp16_data)
    }

    /// FP8 quantization (E4M3 format)
    fn quantize_fp8(&self, fp16_data: &[u8]) -> Result<(Vec<u8>, Vec<u8>)> {
        // FP8 reduces size by 2x compared to FP16
        // This is a simplified implementation
        let num_elements = fp16_data.len() / 2;
        let mut fp8_data = Vec::with_capacity(num_elements);

        for i in 0..num_elements {
            // Simplified FP16 to FP8 conversion
            if i * 2 + 1 < fp16_data.len() {
                fp8_data.push(fp16_data[i * 2]); // Keep lower byte as approximation
            }
        }

        Ok((fp8_data, vec![]))
    }

    fn dequantize_fp8(&self, fp8_data: &[u8]) -> Result<Vec<u8>> {
        // FP8 back to FP16
        let mut fp16_data = Vec::with_capacity(fp8_data.len() * 2);

        for &val in fp8_data {
            fp16_data.push(val);
            fp16_data.push(0); // Zero-extend to FP16
        }

        Ok(fp16_data)
    }

    /// INT4 quantization: 2x INT4 values packed into each byte
    fn quantize_int4(&self, fp16_data: &[u8]) -> Result<(Vec<u8>, Vec<u8>)> {
        let num_elements = fp16_data.len() / 2;
        let mut int4_packed = Vec::with_capacity(num_elements / 2);
        let mut scales = Vec::new();

        for i in (0..num_elements).step_by(2) {
            // Quantize two FP16 values to INT4
            let val1 = if i * 2 + 1 < fp16_data.len() {
                ((fp16_data[i * 2] as u16 | (fp16_data[i * 2 + 1] as u16) << 8) as f32) / 32768.0
            } else {
                0.0
            };

            let val2 = if (i + 1) * 2 + 1 < fp16_data.len() {
                ((fp16_data[(i + 1) * 2] as u16 | (fp16_data[(i + 1) * 2 + 1] as u16) << 8) as f32) / 32768.0
            } else {
                0.0
            };

            let q1 = (val1 * 7.0).clamp(-7.0, 7.0) as u8 & 0x0F;
            let q2 = (val2 * 7.0).clamp(-7.0, 7.0) as u8 & 0x0F;

            // Pack two INT4 values into one byte
            int4_packed.push((q2 << 4) | q1);
        }

        Ok((int4_packed, scales))
    }

    fn dequantize_int4(&self, int4_packed: &[u8], _scales: &[u8]) -> Result<Vec<u8>> {
        let mut fp16_data = Vec::with_capacity(int4_packed.len() * 4);

        for &packed in int4_packed {
            let q1 = (packed & 0x0F) as i8;
            let q2 = ((packed >> 4) & 0x0F) as i8;

            // Dequantize INT4 to FP16
            for q in [q1, q2] {
                let fval = q as f32 / 7.0;
                let fp16_bits = (fval * 32768.0) as u16;
                fp16_data.push((fp16_bits & 0xFF) as u8);
                fp16_data.push(((fp16_bits >> 8) & 0xFF) as u8);
            }
        }

        Ok(fp16_data)
    }
}

/// Batch transmitter for efficient KV Cache transfer
pub struct KVCacheBatchTransmitter {
    compressor: KVCacheCompressor,
    /// Maximum batch size (number of layers per transmission)
    max_batch_size: usize,
    /// Pending chunks waiting to be batched
    pending_chunks: Vec<(i32, Vec<u8>)>,
}

impl KVCacheBatchTransmitter {
    pub fn new(max_batch_size: usize) -> Self {
        Self {
            compressor: KVCacheCompressor::new(
                CompressionAlgorithm::CompressionLz4,
                QuantizationFormat::QuantizationNone,
            ),
            max_batch_size,
            pending_chunks: Vec::new(),
        }
    }

    /// Add a layer chunk to the batch
    pub fn add_layer(&mut self, layer_index: i32, data: Vec<u8>) {
        self.pending_chunks.push((layer_index, data));
    }

    /// Flush the current batch and create a chunk for transmission
    pub fn flush_batch(&mut self, request_id: &str) -> Result<Option<KVCacheChunk>> {
        if self.pending_chunks.is_empty() {
            return Ok(None);
        }

        let chunks = std::mem::take(&mut self.pending_chunks);
        let chunk = self.compressor.create_batch_chunk(
            request_id,
            chunks,
            CompressionAlgorithm::CompressionLz4,
            QuantizationFormat::QuantizationNone,
        )?;

        Ok(Some(chunk))
    }

    /// Check if batch is ready to flush
    pub fn is_batch_ready(&self) -> bool {
        self.pending_chunks.len() >= self.max_batch_size
    }

    /// Get compressor reference for custom operations
    pub fn compressor(&mut self) -> &mut KVCacheCompressor {
        &mut self.compressor
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_compression_stats() {
        let stats = CompressionStats {
            original_size_bytes: 10000,
            compressed_size_bytes: 5000,
            compression_ratio: 0.5,
            compression_time_ms: 10.0,
            algorithm: "lz4".to_string(),
        };

        assert_eq!(stats.compression_percent(), 50.0);
    }

    #[test]
    fn test_no_compression_passthrough() {
        let mut compressor = KVCacheCompressor::new(
            CompressionAlgorithm::CompressionNone,
            QuantizationFormat::QuantizationNone,
        );

        let data = vec![1u8, 2, 3, 4, 5];
        let (compressed, stats) = compressor.compress(&data, CompressionAlgorithm::CompressionNone).unwrap();

        assert_eq!(compressed, data);
        assert_eq!(stats.compression_ratio, 1.0);
        assert_eq!(stats.algorithm, "none");
    }

    #[test]
    fn test_batch_chunk_creation() {
        let mut compressor = KVCacheCompressor::new(
            CompressionAlgorithm::CompressionNone,
            QuantizationFormat::QuantizationNone,
        );

        let layer_chunks = vec![
            (0, vec![1u8, 2, 3]),
            (1, vec![4u8, 5, 6]),
            (2, vec![7u8, 8, 9]),
        ];

        let chunk = compressor.create_batch_chunk(
            "test-request",
            layer_chunks,
            CompressionAlgorithm::CompressionNone,
            QuantizationFormat::QuantizationNone,
        ).unwrap();

        assert_eq!(chunk.request_id, "test-request");
        assert_eq!(chunk.batch_size, 3);
        assert_eq!(chunk.layer_indices, vec![0, 1, 2]);
        assert_eq!(chunk.data.len(), 9);
    }
}
