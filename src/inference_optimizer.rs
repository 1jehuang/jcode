//! GPU 推理加速引擎
//!
//! 目标：CUDA/NVMe 优化，吞吐量提升 3x。
//! 策略：
//! 1. KV 缓存优化 (PagedAttention + 内存池)
//! 2. 批量推理 (动态 batching, 最大 batch=32)
//! 3. 连续批处理 (in-flight batching)
//! 4. CUDA Graph 优化 (减少 kernel launch 开销)
//! 5. FP8/INT4 量化推理 (降低显存带宽需求)
//! 6. NVMe 直连 (减少 PCIe 拷贝)

use anyhow::Result;
use std::collections::VecDeque;
use std::sync::Arc;
use std::time::{Duration, Instant};
use tokio::sync::{Mutex, RwLock, Semaphore};

/// 批处理配置
#[derive(Debug, Clone)]
pub struct BatchConfig {
    /// 最大 batch 大小
    pub max_batch_size: usize,
    /// 最大等待时间 (等够 batch 才执行)
    pub max_wait_ms: u64,
    /// KV 缓存块大小
    pub kv_block_size: usize,
    /// 是否启用连续批处理
    pub continuous_batching: bool,
    /// 是否启用 CUDA Graph
    pub cuda_graph: bool,
    /// 量化精度 (bits)
    pub quantization_bits: u8,
}

impl Default for BatchConfig {
    fn default() -> Self {
        Self {
            max_batch_size: 32,
            max_wait_ms: 50,
            kv_block_size: 256,
            continuous_batching: true,
            cuda_graph: true,
            quantization_bits: 8, // FP8
        }
    }
}

/// 推理请求
#[derive(Debug)]
pub struct InferenceRequest {
    pub id: u64,
    pub prompt_tokens: Vec<u32>,
    pub max_tokens: usize,
    pub temperature: f32,
    pub created_at: Instant,
}

/// 推理结果
#[derive(Debug, Clone)]
pub struct InferenceResult {
    pub id: u64,
    pub output_tokens: Vec<u32>,
    pub prompt_tokens_count: usize,
    pub generated_tokens_count: usize,
    pub latency_ms: f64,
    pub tokens_per_sec: f64,
}

/// KV 缓存管理器 (PagedAttention)
pub struct KvCacheManager {
    /// 总缓存块数
    total_blocks: usize,
    /// 可用块
    free_blocks: Arc<Mutex<Vec<usize>>>,
    /// 块大小
    block_size: usize,
}

impl KvCacheManager {
    pub fn new(total_blocks: usize, block_size: usize) -> Self {
        let mut free = Vec::with_capacity(total_blocks);
        for i in 0..total_blocks {
            free.push(i);
        }
        Self {
            total_blocks,
            free_blocks: Arc::new(Mutex::new(free)),
            block_size,
        }
    }

    /// 分配 KV 缓存块
    pub async fn allocate(&self, count: usize) -> Result<Vec<usize>> {
        let mut free = self.free_blocks.lock().await;
        if free.len() < count {
            anyhow::bail!("Out of KV cache blocks: need {} but only {} available", count, free.len());
        }
        let len = free.len();
        Ok(free.drain(len - count..).collect())
    }

    /// 释放 KV 缓存块
    pub async fn free(&self, blocks: Vec<usize>) {
        let mut free = self.free_blocks.lock().await;
        free.extend(blocks);
    }

    /// 使用率
    pub async fn utilization(&self) -> f64 {
        let free = self.free_blocks.lock().await;
        1.0 - (free.len() as f64 / self.total_blocks as f64)
    }

    /// 获取块大小
    pub fn block_size(&self) -> usize {
        self.block_size
    }
}

/// 批处理器 (动态 batching + 连续批处理)
pub struct BatchProcessor {
    config: BatchConfig,
    pending_queue: Arc<Mutex<VecDeque<InferenceRequest>>>,
    active_batch: Arc<Mutex<Vec<InferenceRequest>>>,
    kv_cache: Arc<KvCacheManager>,
    stats: Arc<RwLock<InferenceStats>>,
    is_running: Arc<std::sync::atomic::AtomicBool>,
}

#[derive(Debug, Clone, Default)]
pub struct InferenceStats {
    pub total_requests: u64,
    pub total_tokens_generated: u64,
    pub avg_latency_ms: f64,
    pub avg_tokens_per_sec: f64,
    pub kv_cache_utilization: f64,
    pub batch_size_avg: f64,
}

impl BatchProcessor {
    pub fn new(config: BatchConfig, kv_cache: Arc<KvCacheManager>) -> Self {
        Self {
            config,
            pending_queue: Arc::new(Mutex::new(VecDeque::new())),
            active_batch: Arc::new(Mutex::new(Vec::new())),
            kv_cache,
            stats: Arc::new(RwLock::new(InferenceStats::default())),
            is_running: Arc::new(std::sync::atomic::AtomicBool::new(false)),
        }
    }

    /// 提交推理请求到队列
    pub async fn submit(&self, request: InferenceRequest) {
        let mut queue = self.pending_queue.lock().await;
        queue.push_back(request);
    }

    /// 开始批处理循环
    pub async fn start<F>(&self, infer_fn: F) -> Result<()>
    where
        F: Fn(Vec<InferenceRequest>) -> Result<Vec<InferenceResult>> + Send + Sync + 'static,
    {
        self.is_running.store(true, std::sync::atomic::Ordering::Release);

        loop {
            if !self.is_running.load(std::sync::atomic::Ordering::Acquire) {
                break;
            }

            // 1. 收集 batch (等待足够请求或超时)
            let batch = self.collect_batch().await;
            if batch.is_empty() {
                tokio::time::sleep(Duration::from_millis(1)).await;
                continue;
            }

            // 2. 执行推理
            let batch_start = Instant::now();
            let results = match infer_fn(batch) {
                Ok(r) => r,
                Err(e) => {
                    tracing::error!("Batch inference error: {}", e);
                    continue;
                }
            };
            let _batch_duration = batch_start.elapsed();

            // 3. 更新统计
            let mut stats = self.stats.write().await;
            for r in &results {
                stats.total_requests += 1;
                stats.total_tokens_generated += r.generated_tokens_count as u64;
                stats.avg_latency_ms = (stats.avg_latency_ms * (stats.total_requests as f64 - 1.0)
                    + r.latency_ms) / stats.total_requests as f64;
                stats.avg_tokens_per_sec = (stats.avg_tokens_per_sec * (stats.total_requests as f64 - 1.0)
                    + r.tokens_per_sec) / stats.total_requests as f64;
            }
            stats.kv_cache_utilization = self.kv_cache.utilization().await;
            stats.batch_size_avg = (stats.batch_size_avg * (stats.total_requests as f64 - 1.0)
                + results.len() as f64) / stats.total_requests as f64;
        }

        Ok(())
    }

    /// 停止批处理循环
    pub fn stop(&self) {
        self.is_running.store(false, std::sync::atomic::Ordering::Release);
    }

    /// 收集 batch (等待 max_batch_size 个请求或超时)
    async fn collect_batch(&self) -> Vec<InferenceRequest> {
        let deadline = Instant::now() + Duration::from_millis(self.config.max_wait_ms);

        loop {
            let mut queue = self.pending_queue.lock().await;
            if queue.len() >= self.config.max_batch_size || Instant::now() >= deadline {
                let drain_len = queue.len().min(self.config.max_batch_size);
                let batch: Vec<InferenceRequest> = queue.drain(..drain_len).collect();
                if !batch.is_empty() {
                    return batch;
                }
            }
            drop(queue);
            tokio::time::sleep(Duration::from_millis(1)).await;
        }
    }

    /// 获取统计
    pub async fn stats(&self) -> InferenceStats {
        let mut stats = self.stats.write().await;
        stats.kv_cache_utilization = self.kv_cache.utilization().await;
        stats.clone()
    }
}

/// NVMe 优化 (直接 I/O, 减少拷贝)
pub struct NvmeOptimizer;

impl NvmeOptimizer {
    /// 建议的 NVMe 优化参数
    pub fn optimal_io_size() -> usize {
        // NVMe SSD 的最佳 I/O 大小是 4K 对齐
        4096
    }

    /// 启用直接 I/O (跳过 page cache)
    pub fn enable_direct_io(path: &std::path::Path) -> Result<std::fs::File> {
        use std::os::windows::io::FromRawHandle;
        // Windows 上使用 FILE_FLAG_NO_BUFFERING
        // Linux 上使用 O_DIRECT

        #[cfg(target_os = "linux")]
        {
            use std::os::unix::fs::OpenOptionsExt;
            let file = std::fs::OpenOptions::new()
                .read(true)
                .write(true)
                .custom_flags(libc::O_DIRECT)
                .open(path)?;
            return Ok(file);
        }

        #[cfg(not(target_os = "linux"))]
        {
            // 非 Linux 系统回退到普通 I/O
            Ok(std::fs::File::open(path)?)
        }
    }

    /// 内存对齐分配 (NVMe 要求)
    pub fn aligned_alloc(size: usize) -> Vec<u8> {
        let align = Self::optimal_io_size();
        let aligned_size = (size + align - 1) & !(align - 1);
        let mut buf = Vec::with_capacity(aligned_size);
        buf.resize(aligned_size, 0);
        buf
    }
}

/// CUDA Graph 优化 (减少 kernel launch 开销)
pub struct CudaGraphOptimizer;

impl CudaGraphOptimizer {
    /// 建议的 graph 大小
    pub fn optimal_graph_nodes() -> usize {
        1024 // 1000+ kernel 的 graph 可最大化收益
    }

    /// 是否启用 graph（基于 batch 大小）
    pub fn should_use_graph(batch_size: usize) -> bool {
        batch_size >= 4 // 小 batch 时 graph 收益有限
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[tokio::test]
    async fn test_kv_cache_alloc_free() {
        let cache = KvCacheManager::new(100, 256);
        let blocks = cache.allocate(10).await.unwrap();
        assert_eq!(blocks.len(), 10);
        assert!(cache.utilization().await > 0.0);

        cache.free(blocks).await;
        assert!(cache.utilization().await < 0.01);
    }

    #[tokio::test]
    async fn test_kv_cache_oom() {
        let cache = KvCacheManager::new(5, 256);
        let result = cache.allocate(10).await;
        assert!(result.is_err());
    }

    #[test]
    fn test_nvme_aligned_alloc() {
        let buf = NvmeOptimizer::aligned_alloc(100);
        assert_eq!(buf.len(), NvmeOptimizer::optimal_io_size());
        assert!(buf.len() >= 100);
    }

    #[test]
    fn test_cuda_graph_decision() {
        assert!(!CudaGraphOptimizer::should_use_graph(1));
        assert!(!CudaGraphOptimizer::should_use_graph(2));
        assert!(CudaGraphOptimizer::should_use_graph(4));
    }

    #[tokio::test]
    async fn test_batch_collection() {
        let kv = Arc::new(KvCacheManager::new(1000, 256));
        let processor = BatchProcessor::new(BatchConfig::default(), kv);

        for i in 0..5 {
            processor.submit(InferenceRequest {
                id: i,
                prompt_tokens: vec![1, 2, 3],
                max_tokens: 100,
                temperature: 0.7,
                created_at: Instant::now(),
            }).await;
        }

        let batch = processor.collect_batch().await;
        assert_eq!(batch.len(), 5);
    }
}
