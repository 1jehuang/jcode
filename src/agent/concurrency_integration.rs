//! 并发优化器集成
//!
//! 将 ConcurrencyOptimizer 集成到 Agent 的 turn 执行循环中。
//! 挂载点: src/agent/turn_execution.rs 的 run_once() 系列函数

use crate::concurrency_optimizer::{
    ConcurrencyOptimizer, ConcurrencyStats, RequestPriority
};
use std::sync::Arc;
use std::sync::atomic::{AtomicBool, Ordering};
use std::time::Duration;

/// 全局并发优化器
static CONCURRENCY_OPT: std::sync::OnceLock<Arc<ConcurrencyOptimizer>> = std::sync::OnceLock::new();
static CONCURRENCY_ENABLED: AtomicBool = AtomicBool::new(true);

/// 初始化全局并发优化器
pub fn init_concurrency_optimizer() {
    let opt = ConcurrencyOptimizer::new(500.0); // max 500 RPS
    let _ = CONCURRENCY_OPT.set(Arc::new(opt));
}

/// 获取全局并发优化器
pub fn concurrency_optimizer() -> Option<&'static Arc<ConcurrencyOptimizer>> {
    CONCURRENCY_OPT.get()
}

pub fn set_concurrency_enabled(enabled: bool) {
    CONCURRENCY_ENABLED.store(enabled, Ordering::Release);
}

pub fn is_concurrency_enabled() -> bool {
    CONCURRENCY_ENABLED.load(Ordering::Acquire)
}

// ---- Agent 集成钩子 ----

/// 在 Agent 调用 provider.complete_split 前调用。
/// 返回包装了执行器的闭包，使 API 调用受并发控制。
pub async fn wrap_api_call<F, Fut, T>(prompt: &str, priority: RequestPriority, f: F) -> Result<T, String>
where
    F: FnOnce() -> Fut + Send,
    Fut: std::future::Future<Output = Result<T, String>> + Send,
    T: Send + 'static,
{
    if !is_concurrency_enabled() {
        return f().await;
    }

    let optimizer = match concurrency_optimizer() {
        Some(o) => o,
        None => return f().await,
    };

    let wrapped_f = || async {
        f().await.map_err(|e| anyhow::anyhow!(e))
    };

    optimizer.execute(prompt, priority, wrapped_f).await
        .map_err(|e| e.to_string())
}

/// 在 Agent 工具执行前调用。
/// 允许并行执行 readonly 工具。
pub fn should_parallelize(is_readonly: bool, current_parallel: usize) -> bool {
    if !is_concurrency_enabled() || !is_readonly {
        return false;
    }
    // 最多并行10个 readonly 工具
    current_parallel < 10
}

/// 获取并发统计（用于调试/状态栏）
pub async fn get_concurrency_stats() -> ConcurrencyStats {
    match concurrency_optimizer() {
        Some(o) => o.stats().await,
        None => ConcurrencyStats::default(),
    }
}

/// 自适应调优循环（后台）
pub async fn concurrency_tune_loop() {
    loop {
        tokio::time::sleep(Duration::from_secs(30)).await; // 每30秒
        if let Some(optimizer) = concurrency_optimizer() {
            optimizer.tune().await;
            let stats = optimizer.stats().await;
            tracing::info!(
                "Concurrency tune: active={}, queue={}, p99={}ms, avg={:.1}ms, merged={}",
                stats.active_connections,
                stats.queue_depth,
                stats.p99_latency_ms,
                stats.avg_latency_ms,
                stats.merged_requests,
            );
        }
    }
}

/// 将 ConcurrencyOptimizer 绑定到 Agent
/// 在 Agent::new() 或运行时初始化时调用
pub fn bind_to_agent() {
    if CONCURRENCY_OPT.get().is_none() {
        init_concurrency_optimizer();
    }
    set_concurrency_enabled(true);
    tracing::info!("Concurrency optimizer bound to agent loop");
}

// ---- 辅助函数 ----

/// 根据工具调用确定优先级
pub fn priority_for_tool(tool_name: &str) -> RequestPriority {
    match tool_name {
        "read" | "glob" | "grep" | "ls" => RequestPriority::Low,
        "edit" | "write" | "create" => RequestPriority::High,
        "bash" | "run" => RequestPriority::Medium,
        "review" | "think" => RequestPriority::Urgent,
        _ => RequestPriority::Medium,
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[tokio::test]
    async fn test_init_and_stats() {
        init_concurrency_optimizer();
        assert!(concurrency_optimizer().is_some());

        let stats = get_concurrency_stats().await;
        assert_eq!(stats.total_requests, 0);
    }

    #[test]
    fn test_priority_for_tool() {
        assert_eq!(priority_for_tool("read"), RequestPriority::Low);
        assert_eq!(priority_for_tool("edit"), RequestPriority::High);
        assert_eq!(priority_for_tool("review"), RequestPriority::Urgent);
    }

    #[test]
    fn test_parallelize_decision() {
        assert!(!should_parallelize(false, 5));
        assert!(should_parallelize(true, 5));
        assert!(!should_parallelize(true, 10));
    }
}
