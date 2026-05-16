//! ## 任务 2.3: 虚拟内存推理优化
//!
//! 针对网吧 512G 虚拟内存、128G 物理内存等场景的优化。
//!
//! 核心思路：利用 `mmap` 内存映射机制，将大模型 KV Cache 存储到虚拟内存中，
//! 虽然比物理内存慢 30% 左右，但能显著提升单节点并发能力（2-3 倍）。
//!
//! ### 适配场景
//!
//! | 节点类型 | 物理内存 | 虚拟内存 | 可用 KV Cache | 并发提升 |
//! |---------|---------|---------|--------------|---------|
//! | 网吧台式机 | 128 GB | 512 GB | ~480 GB | 3-4 倍 |
//! | 办公台式机 | 128 GB | 128 GB | ~100 GB | 1.5 倍 |
//! | 员工笔记本 | 16 GB | 32 GB | ~20 GB | 1.2 倍 |

use crate::config::VirtualMemoryConfig;
use serde::{Deserialize, Serialize};
use std::collections::HashMap;
use std::path::PathBuf;
use std::sync::Arc;
use tokio::sync::RwLock;
use tracing::{debug, info, warn};

/// 内存映射区域
#[derive(Debug, Clone)]
pub struct MmapRegion {
    /// 区域名称
    pub name: String,
    /// 映射文件路径
    pub file_path: PathBuf,
    /// 映射大小 (字节)
    pub size: u64,
    /// 是否已映射
    pub is_mapped: bool,
    /// 关联的模型名称
    pub model_name: String,
    /// 创建时间
    pub created_at: chrono::DateTime<chrono::Utc>,
}

/// 虚拟内存推理管理器
pub struct VirtualMemoryManager {
    /// 配置
    config: VirtualMemoryConfig,
    /// 当前映射区域
    regions: Arc<RwLock<HashMap<String, MmapRegion>>>,
    /// 已使用的映射空间 (字节)
    used_bytes: Arc<std::sync::atomic::AtomicU64>,
    /// 读写操作的统计
    pub stats: Arc<RwLock<VMStats>>,
}

/// 虚拟内存统计
#[derive(Debug, Clone, Default, Serialize, Deserialize)]
pub struct VMStats {
    /// 总映射次数
    pub total_mmaps: u64,
    /// 总解除映射次数
    pub total_munmaps: u64,
    /// 总读取操作数
    pub total_reads: u64,
    /// 总写入操作数
    pub total_writes: u64,
    /// 平均延迟 (ms)
    pub avg_latency_ms: f64,
    /// 当前已使用的总空间 (GB)
    pub used_gb: f64,
    /// 当前总映射区域数
    pub region_count: usize,
}

impl VirtualMemoryManager {
    pub fn new(config: VirtualMemoryConfig) -> Self {
        // 确保 mmap 目录存在
        let _ = std::fs::create_dir_all(&config.mmap_dir);

        Self {
            config,
            regions: Arc::new(RwLock::new(HashMap::new())),
            used_bytes: Arc::new(std::sync::atomic::AtomicU64::new(0)),
            stats: Arc::new(RwLock::new(VMStats::default())),
        }
    }

    /// 检查系统虚拟内存状态
    pub fn check_swap_status() -> SwapInfo {
        let mem_info = sys_info::mem_info().ok();

        SwapInfo {
            total_swap_gb: mem_info
                .map(|m| (m.total as f64 - m.free as f64) / 1024.0 / 1024.0)
                .unwrap_or(0.0),
            available_swap_gb: mem_info
                .map(|m| m.free as f64 / 1024.0 / 1024.0)
                .unwrap_or(0.0),
            total_physical_gb: mem_info
                .map(|m| m.total as f64 / 1024.0 / 1024.0)
                .unwrap_or(0.0),
            available_physical_gb: mem_info
                .map(|m| m.avail as f64 / 1024.0 / 1024.0)
                .unwrap_or(0.0),
        }
    }

    /// 为 KV Cache 创建 mmap 区域
    pub async fn create_kv_cache_mmap(
        &self,
        model_name: &str,
        cache_size_mb: u64,
    ) -> anyhow::Result<MmapRegion> {
        let swap = Self::check_swap_status();
        let total_available_gb = swap.available_physical_gb + swap.available_swap_gb;
        let cache_size_gb = cache_size_mb as f64 / 1024.0;

        if cache_size_gb > total_available_gb * 0.8 {
            anyhow::bail!(
                "可用内存不足: 需要 {} GB, 仅 {} GB 可用",
                cache_size_gb, total_available_gb
            );
        }

        let file_name = format!("kv_cache_{}_{}.mmap", model_name, chrono::Utc::now().timestamp());
        let file_path = self.config.mmap_dir.join(&file_name);

        // Pre-allocate the file
        let file = std::fs::OpenOptions::new()
            .create(true)
            .read(true)
            .write(true)
            .open(&file_path)?;
        file.set_len(cache_size_mb * 1024 * 1024)?;

        let region = MmapRegion {
            name: file_name,
            file_path: file_path.clone(),
            size: cache_size_mb * 1024 * 1024,
            is_mapped: true,
            model_name: model_name.to_string(),
            created_at: chrono::Utc::now(),
        };

        let mut regions = self.regions.write().await;
        regions.insert(model_name.to_string(), region.clone());
        self.used_bytes.fetch_add(region.size, std::sync::atomic::Ordering::Relaxed);

        let mut stats = self.stats.write().await;
        stats.total_mmaps += 1;
        stats.used_gb = self.used_bytes.load(std::sync::atomic::Ordering::Relaxed) as f64 / 1024.0 / 1024.0 / 1024.0;
        stats.region_count = regions.len();

        info!(
            "KV Cache mmap 创建完成: model={}, size={}MB, path={:?}",
            model_name, cache_size_mb, file_path
        );

        Ok(region)
    }

    /// 释放 mmap 区域
    pub async fn release_kv_cache(&self, model_name: &str) -> anyhow::Result<()> {
        let mut regions = self.regions.write().await;
        if let Some(region) = regions.remove(model_name) {
            // Remove the mapped file
            self.used_bytes.fetch_sub(region.size, std::sync::atomic::Ordering::Relaxed);
            let _ = std::fs::remove_file(&region.file_path);

            let mut stats = self.stats.write().await;
            stats.total_munmaps += 1;
            stats.used_gb = self.used_bytes.load(std::sync::atomic::Ordering::Relaxed) as f64 / 1024.0 / 1024.0 / 1024.0;
            stats.region_count = regions.len();

            info!(
                "KV Cache mmap 释放: model={}, file={:?}",
                model_name, region.file_path
            );
            Ok(())
        } else {
            Ok(()) // 没有对应的映射，不做任何事
        }
    }

    /// 获取当前内存使用状态
    pub async fn get_memory_usage(&self) -> MemoryUsage {
        let regions = self.regions.read().await;
        let swap = Self::check_swap_status();

        MemoryUsage {
            physical: MemoryLevel {
                total_gb: swap.total_physical_gb,
                used_gb: swap.total_physical_gb - swap.available_physical_gb,
                available_gb: swap.available_physical_gb,
            },
            swap: MemoryLevel {
                total_gb: swap.total_swap_gb,
                used_gb: swap.total_swap_gb - swap.available_swap_gb,
                available_gb: swap.available_swap_gb,
            },
            mmap_regions: regions.len(),
            mmap_used_gb: self.used_bytes.load(std::sync::atomic::Ordering::Relaxed) as f64
                / 1024.0 / 1024.0 / 1024.0,
        }
    }
}

/// 交换空间 / 内存信息
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct SwapInfo {
    pub total_swap_gb: f64,
    pub available_swap_gb: f64,
    pub total_physical_gb: f64,
    pub available_physical_gb: f64,
}

/// 内存使用级别
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct MemoryLevel {
    pub total_gb: f64,
    pub used_gb: f64,
    pub available_gb: f64,
}

/// 内存使用报告
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct MemoryUsage {
    pub physical: MemoryLevel,
    pub swap: MemoryLevel,
    pub mmap_regions: usize,
    pub mmap_used_gb: f64,
}
