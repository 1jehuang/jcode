//! 缓存优化器集成
//!
//! 将 TokenCacheOptimizer 集成到提供的 Anthropic API 调用流程中。
//! 挂载点: src/provider/anthropic.rs 的 complete_split() 和 stream_response()

use crate::cache_optimizer::{TokenCacheOptimizer, CacheOptimizerConfig, CacheStats};
use std::sync::Arc;
use std::time::{Duration, Instant};
use std::sync::atomic::{AtomicBool, Ordering};
use tokio::sync::RwLock;

/// 全局缓存优化器实例
static CACHE_OPTIMIZER: std::sync::OnceLock<Arc<TokenCacheOptimizer>> = std::sync::OnceLock::new();
/// 是否启用
static CACHE_ENABLED: AtomicBool = AtomicBool::new(true);

/// 初始化全局缓存优化器
pub fn init_cache_optimizer() {
    let config = CacheOptimizerConfig {
        l1_capacity: 100_000,  // 10万条
        l2_capacity_mb: 1024,  // 1GB
        dedup_ratio: 0.3,      // 30%压缩
        prefetch_depth: 3,     // 预取3步
    };
    let _ = CACHE_OPTIMIZER.set(Arc::new(TokenCacheOptimizer::new(config)));
}

/// 获取全局缓存优化器
pub fn cache_optimizer() -> Option<&'static Arc<TokenCacheOptimizer>> {
    CACHE_OPTIMIZER.get()
}

/// 启用/禁用缓存
pub fn set_cache_enabled(enabled: bool) {
    CACHE_ENABLED.store(enabled, Ordering::Release);
}

/// 缓存是否启用
pub fn is_cache_enabled() -> bool {
    CACHE_ENABLED.load(Ordering::Acquire)
}

// ---- 提供者集成 (Anthropic) 钩子 ----

/// 在向 Anthropic API 发送请求前调用。
/// 检查缓存是否存在，如果命中则跳过 API 调用直接返回缓存结果。
/// prompt_hash: 提示的 hash
/// prefix_tokens: 前缀 token 序列
/// 返回: Some(缓存结果) 或 None (未命中，需要调用 API)
pub async fn pre_api_call(prompt_hash: u64, prefix_tokens: &[u32]) -> Option<Vec<u32>> {
    if !is_cache_enabled() {
        return None;
    }

    let optimizer = cache_optimizer()?;

    // 1. 计算缓存键
    let key = TokenCacheOptimizer::compute_cache_key(
        &prompt_hash.to_string(),
        prefix_tokens,
    );

    // 2. 查找 L1/L2 缓存
    if let Some(entry) = optimizer.get(key).await {
        // 3. 预取相关条目 (提升后续命中率)
        optimizer.prefetch(&[key]).await;
        return Some(entry.response_prefix);
    }

    None
}

/// 在 API 调用完成后调用。
/// 将结果存入缓存以供后续使用。
pub async fn post_api_call(
    prompt_hash: u64,
    prefix_tokens: &[u32],
    response_tokens: &[u32],
    frequency: f64,
) {
    if !is_cache_enabled() || response_tokens.is_empty() {
        return;
    }

    let optimizer = match cache_optimizer() {
        Some(o) => o,
        None => return,
    };

    let key = TokenCacheOptimizer::compute_cache_key(
        &prompt_hash.to_string(),
        prefix_tokens,
    );

    let entry = crate::cache_optimizer::TokenCacheEntry {
        tokens: prefix_tokens.to_vec(),
        prompt_hash,
        response_prefix: response_tokens.to_vec(),
        created_at: Instant::now(),
        access_count: 1,
        frequency,
    };

    optimizer.put(key, entry).await;
}

/// 获取缓存被指标（用于显示在状态栏/调试信息中）
pub async fn get_cache_hit_rate() -> f64 {
    match cache_optimizer() {
        Some(o) => o.stats().await.hit_rate(),
        None => 0.0,
    }
}

// ---- 工具集成 ----

/// 缓存清理后台任务
pub async fn cache_maintenance_loop() {
    loop {
        tokio::time::sleep(Duration::from_secs(300)).await; // 每5分钟
        if let Some(optimizer) = cache_optimizer() {
            optimizer.evict_expired(Duration::from_secs(3600)).await; // 1小时过期
            let stats = optimizer.stats().await;
            tracing::info!(
                "Cache maintenance: hit_rate={:.2}%, memory={:.1}MB, entries={}",
                stats.hit_rate() * 100.0,
                stats.memory_usage_mb,
                stats.total_requests,
            );
        }
    }
}

// ---- 单元测试 ----
#[cfg(test)]
mod tests {
    use super::*;

    #[tokio::test]
    async fn test_cache_integration_flow() {
        init_cache_optimizer();
        assert!(cache_optimizer().is_some());

        // 初始无缓存 -> miss
        let result = pre_api_call(42, &[1, 2, 3]).await;
        assert!(result.is_none());

        // 存入缓存
        post_api_call(42, &[1, 2, 3], &[10, 20, 30], 0.8).await;

        // 再次查询 -> hit
        let result = pre_api_call(42, &[1, 2, 3]).await;
        assert!(result.is_some());
        assert_eq!(result.unwrap(), vec![10, 20, 30]);
    }

    #[test]
    fn test_cache_enable_disable() {
        assert!(is_cache_enabled());
        set_cache_enabled(false);
        assert!(!is_cache_enabled());
        set_cache_enabled(true);
        assert!(is_cache_enabled());
    }
}
