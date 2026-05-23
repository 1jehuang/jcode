# KV Cache Compression & Batching Usage Guide (P1-4)

## Overview

This document describes the P1-4 optimization features for KV Cache transmission in CarpAI's distributed inference engine.

### Key Features

1. **Multiple Compression Algorithms**
   - LZ4: Fast compression, good ratio (~2x)
   - Zstd: Balanced compression, better ratio (~3x)
   - Snappy: Ultra-fast, moderate ratio (~1.8x)

2. **Quantization Support**
   - FP16 (default): Full precision
   - INT8: 2x reduction with minimal accuracy loss
   - FP8: 2x reduction, newer format
   - INT4: 4x reduction, lossy (for non-critical layers)

3. **Batch Transmission**
   - Group multiple layers into single gRPC call
   - Reduce network overhead by ~75% (with batch_size=4)
   - Automatic checksum verification

## Quick Start

### Basic Usage (No Compression)

```rust
use jcode_distributed_inference::kv_cache_manager::KVCacheManager;

// Create manager with default settings
let mut manager = KVCacheManager::new();

// Store KV Cache chunks (backward compatible)
manager.assemble_and_store("request-123", chunks)?;
```

### With Compression (Recommended)

```rust
use jcode_distributed_inference::{
    kv_cache_manager::KVCacheManager,
    proto::{CompressionAlgorithm, QuantizationFormat},
};

// Create manager with LZ4 compression
let mut manager = KVCacheManager::with_compression(
    CompressionAlgorithm::CompressionLz4,
    QuantizationFormat::QuantizationNone,
    4, // max_batch_size
);

// Add layers to batch
for (layer_idx, layer_data) in layers {
    manager.add_layer_to_batch("request-123", layer_idx, layer_data);

    // Check if batch is ready to send
    if manager.is_batch_ready("request-123") {
        if let Some(chunk) = manager.flush_batch("request-123")? {
            // Send chunk via gRPC
            send_to_worker(chunk).await?;
        }
    }
}

// Flush remaining layers
if let Some(chunk) = manager.flush_batch("request-123")? {
    send_to_worker(chunk).await?;
}
```

### With Quantization (Maximum Savings)

```rust
// INT8 quantization: 2x size reduction
let mut manager = KVCacheManager::with_compression(
    CompressionAlgorithm::CompressionZstd,
    QuantizationFormat::QuantizationInt8,
    4,
);

// Expected savings: ~70-75% total reduction
// (50% from INT8 + 50% from Zstd on quantized data)
```

## Performance Comparison

| Configuration | Compression Ratio | CPU Overhead | Accuracy Loss | Use Case |
|--------------|-------------------|--------------|---------------|----------|
| None + FP16 | 1.0x (baseline) | 0% | 0% | Debugging |
| LZ4 + FP16 | ~2.0x | <5% | 0% | General purpose |
| Zstd + FP16 | ~3.0x | ~10% | 0% | Bandwidth-limited |
| LZ4 + INT8 | ~4.0x | <5% | <0.1% | Production (recommended) |
| Zstd + INT8 | ~6.0x | ~10% | <0.1% | Maximum efficiency |
| LZ4 + INT4 | ~8.0x | <5% | 1-2% | Non-critical layers |

## Batch Transmission Benefits

### Without Batching (Original)
```
Layer 0 -> gRPC call
Layer 1 -> gRPC call
Layer 2 -> gRPC call
Layer 3 -> gRPC call
Total: 4 gRPC calls, 4x header overhead
```

### With Batching (batch_size=4)
```
Layers [0,1,2,3] -> Single gRPC call
Total: 1 gRPC call, 1x header overhead
Savings: 75% reduction in RPC overhead
```

## Monitoring & Statistics

```rust
// Get per-request compression stats
if let Some(stats) = manager.get_compression_stats("request-123") {
    println!("Original size: {} KB", stats.original_size_bytes / 1024);
    println!("Compressed size: {} KB", stats.compressed_size_bytes / 1024);
    println!("Reduction: {:.1}%", stats.compression_percent());
    println!("Time: {:.2} ms", stats.compression_time_ms);
}

// Get overall summary
let stats = manager.get_stats();
println!("Cached requests: {}", stats.cached_requests);
println!("Total cache size: {:.2} MB", stats.total_size_mb);
println!("Avg compression reduction: {:.1}%", stats.avg_compression_reduction_percent);
```

## Integration with Worker Node

The worker node automatically handles decompression:

```rust
// In worker.rs - receiving compressed chunk
async fn transfer_kv_cache(&self, request: TransferKVCacheRequest) -> Result<Response<KVCacheAck>> {
    let chunk = request.into_inner();

    // Store with automatic decompression
    self.kv_manager.store_compressed_chunk(&chunk.request_id, chunk)?;

    Ok(Response::new(KVCacheAck {
        request_id: chunk.request_id,
        success: true,
        error_message: String::new(),
    }))
}
```

## Configuration Recommendations

### For LAN Deployment (<1ms latency)
```rust
// Minimal compression to reduce CPU overhead
KVCacheManager::with_compression(
    CompressionAlgorithm::CompressionLz4,
    QuantizationFormat::QuantizationNone,
    2, // Small batch size
)
```

### For WAN/Cloud Deployment (>10ms latency)
```rust
// Maximum compression to reduce bandwidth costs
KVCacheManager::with_compression(
    CompressionAlgorithm::CompressionZstd,
    QuantizationFormat::QuantizationInt8,
    8, // Large batch size
)
```

### For Mixed Workloads
```rust
// Adaptive: Use different configs per model layer
// Early layers (critical): FP16 + LZ4
// Middle layers: INT8 + LZ4
// Late layers: INT8 + Zstd
```

## Error Handling

The system includes built-in integrity verification:

```rust
// Checksum verification happens automatically
match manager.store_compressed_chunk("req-1", chunk) {
    Ok(_) => println!("Data integrity verified"),
    Err(e) if e.to_string().contains("integrity check failed") => {
        eprintln!("Checksum mismatch! Retrying transmission...");
        // Retry logic here
    }
    Err(e) => return Err(e),
}
```

## Migration from Legacy Code

Old code continues to work without changes:

```rust
// Old interface (still supported)
manager.assemble_and_store("request-id", chunks)?;

// New interface (recommended for new code)
manager.add_layer_to_batch("request-id", layer_idx, data);
manager.flush_batch("request-id")?;
```

## Future Enhancements

Potential improvements for future phases:
- GPU-accelerated compression (cuLZ4, cuZstd)
- Adaptive compression based on network conditions
- Differential compression (send only changed KV Cache)
- RDMA integration for zero-copy transfers
