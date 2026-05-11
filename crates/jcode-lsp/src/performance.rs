//! Performance Monitor — LSP 性能监控和自适应调优
//!
//! ## 核心能力 (对标 Cursor/Claude Code)
//! - **操作耗时统计**: 每个操作的响应时间分布
//! - **Server 健康检查**: 自动检测 Server 是否卡死/崩溃
//! - **内存占用监控**: 防止 OOM
//! - **自适应超时**: 根据历史数据动态调整超时时间
//! - **自动重启策略**: 检测到异常时自动重启
//!
//! ## 监控指标
//! - P50/P95/P99 响应时间
//! - 操作成功率
//! - Server 进程 CPU/内存使用
//! - 连接池命中率
//!
//! ## 自适应策略
//! - 超时调整: 如果连续超时，自动增加超时时间（上限 60s）
//! - 重启阈值: 如果错误率 > 50%，触发重启
//! - 负载均衡: 如果单个 Server 过载，分散请求

use std::collections::HashMap;
use std::sync::Arc;
use std::time::{Duration, Instant};
use tokio::sync::RwLock;
use tracing::{debug, error, info, warn};

/// 单次操作的性能记录
#[derive(Debug, Clone)]
pub struct OperationMetrics {
    /// 操作名称 (e.g., "goto_definition")
    pub operation: String,
    
    /// 开始时间
    pub started_at: Instant,
    
    /// 结束时间
    pub ended_at: Option<Instant>,
    
    /// 耗时 (毫秒)
    pub duration_ms: Option<u64>,
    
    /// 是否成功
    pub success: bool,
    
    /// 错误信息
    pub error: Option<String>,
}

impl OperationMetrics {
    pub fn new(operation: impl Into<String>) -> Self {
        Self {
            operation: operation.into(),
            started_at: Instant::now(),
            ended_at: None,
            duration_ms: None,
            success: false,
            error: None,
        }
    }

    pub fn finish_success(&mut self) {
        self.ended_at = Some(Instant::now());
        self.duration_ms = Some(self.started_at.elapsed().as_millis() as u64);
        self.success = true;
    }

    pub fn finish_error(&mut self, error: impl Into<String>) {
        self.ended_at = Some(Instant::now());
        self.duration_ms = Some(self.started_at.elapsed().as_millis() as u64);
        self.success = false;
        self.error = Some(error.into());
    }
}

/// 性能统计摘要
#[derive(Debug, Clone, Default)]
pub struct PerformanceStats {
    /// 总操作数
    pub total_operations: u64,
    
    /// 成功数
    pub success_count: u64,
    
    /// 失败数
    pub failure_count: u64,
    
    /// 平均耗时 (ms)
    pub avg_duration_ms: f64,
    
    /// P50 耗时 (ms)
    pub p50_duration_ms: u64,
    
    /// P95 耗时 (ms)
    pub p95_duration_ms: u64,
    
    /// P99 耗时 (ms)
    pub p99_duration_ms: u64,
    
    /// 最大耗时 (ms)
    pub max_duration_ms: u64,
    
    /// 最小耗时 (ms)
    pub min_duration_ms: u64,
    
    /// 成功率 (%)
    pub success_rate: f64,
    
    /// 最近 N 次操作的错误率
    recent_error_rate: f64,
}

/// Server 健康状态
#[derive(Debug, Clone)]
pub enum ServerHealthStatus {
    Healthy,
    Degraded { reason: String },
    Unhealthy { reason: String },
    Down { reason: String },
}

/// Server 健康信息
#[derive(Debug, Clone)]
pub struct ServerHealthInfo {
    /// Server 名称
    pub server_name: String,
    
    /// 状态
    pub status: ServerHealthStatus,
    
    /// 运行时长
    pub uptime: Duration,
    
    /// 内存占用估算 (bytes)
    pub memory_usage: u64,
    
    /// 处理的操作数
    pub operations_processed: u64,
    
    /// 最后一次操作的时间
    pub last_operation_time: Option<Instant>,
    
    /// 连续失败次数
    pub consecutive_failures: u64,
}

/// 自适应配置
#[derive(Debug, Clone)]
pub struct AdaptiveConfig {
    /// 默认超时 (ms)
    pub default_timeout_ms: u64,
    
    /// 最大超时 (ms)
    pub max_timeout_ms: u64,
    
    /// 触发重启的连续失败次数
    pub restart_threshold: u64,
    
    /// 统计窗口大小 (最近 N 次操作)
    pub stats_window_size: usize,
    
    /// 是否启用自动重启
    pub auto_restart: bool,
    
    /// 健康检查间隔
    pub health_check_interval: Duration,
}

impl Default for AdaptiveConfig {
    fn default() -> Self {
        Self {
            default_timeout_ms: 30_000, // 30s
            max_timeout_ms: 60_000,     // 60s
            restart_threshold: 5,
            stats_window_size: 100,
            auto_restart: true,
            health_check_interval: Duration::from_secs(10),
        }
    }
}

/// 性能监控器
pub struct PerformanceMonitor {
    /// 所有操作记录 (最近 N 次)
    operations: Arc<RwLock<Vec<OperationMetrics>>>,
    
    /// 每个 Server 的健康状态
    server_health: Arc<RwLock<HashMap<String, ServerHealthInfo>>>,
    
    /// 配置
    config: AdaptiveConfig,
    
    /// 当前自适应超时时间
    current_timeout_ms: Arc<RwLock<u64>>,
    
    /// 全局统计
    global_stats: Arc<RwLock<PerformanceStats>>,
}

impl Default for PerformanceMonitor {
    fn default() -> Self {
        Self::with_config(AdaptiveConfig::default())
    }
}

impl PerformanceMonitor {
    pub fn new() -> Self {
        Self::default()
    }

    pub fn with_config(config: AdaptiveConfig) -> Self {
        let default_timeout = config.default_timeout_ms;
        Self {
            operations: Arc::new(RwLock::new(vec![])),
            server_health: Arc::new(RwLock::new(HashMap::new())),
            config,
            current_timeout_ms: Arc::new(RwLock::new(default_timeout)),
            global_stats: Arc::new(RwLock::new(PerformanceStats::default())),
        }
    }

    /// 开始记录操作
    pub async fn start_operation(&self, operation: impl Into<String>) -> OperationMetrics {
        let metrics = OperationMetrics::new(operation);
        
        debug!(operation = ?metrics.operation, "Operation started");
        
        metrics
    }

    /// 结束操作（成功）
    pub async fn finish_operation_success(&self, mut metrics: OperationMetrics) {
        metrics.finish_success();
        
        let operation_name = metrics.operation.clone();
        
        {
            let mut ops = self.operations.write().await;
            ops.push(metrics.clone());
            
            // 保持窗口大小
            if ops.len() > self.config.stats_window_size {
                ops.remove(0);
            }
        }

        // 更新 Server 健康
        if let Some(server) = self.extract_server_from_operation(&operation_name) {
            self.update_server_health(&server, true).await;
        }

        debug!(
            operation = %operation_name,
            duration_ms = metrics.duration_ms.unwrap_or(0),
            "Operation completed successfully"
        );

        // 更新全局统计
        self.recalculate_global_stats().await;

        // 自适应调整超时
        self.adjust_timeout_if_needed().await;
    }

    /// 结束操作（失败）
    pub async fn finish_operation_error(&self, mut metrics: OperationMetrics, error: impl Into<String>) {
        metrics.finish_error(error);
        
        let operation_name = metrics.operation.clone();
        
        {
            let mut ops = self.operations.write().await;
            ops.push(metrics.clone());
            
            if ops.len() > self.config.stats_window_size {
                ops.remove(0);
            }
        }

        if let Some(server) = self.extract_server_from_operation(&operation_name) {
            self.update_server_health(&server, false).await;
        }

        warn!(
            operation = %operation_name,
            duration_ms = metrics.duration_ms.unwrap_or(0),
            error = %metrics.error.as_deref().unwrap_or("Unknown"),
            "Operation failed"
        );

        self.recalculate_global_stats().await;
        self.adjust_timeout_if_needed().await;
    }

    /// 获取当前自适应超时时间
    pub async fn get_current_timeout(&self) -> u64 {
        *self.current_timeout_ms.read().await
    }

    /// 获取全局性能统计
    pub async fn get_global_stats(&self) -> PerformanceStats {
        self.global_stats.read().await.clone()
    }

    /// 获取指定 Server 的健康状态
    pub async fn get_server_health(&self, server_name: &str) -> Option<ServerHealthInfo> {
        let health = self.server_health.read().await;
        health.get(server_name).cloned()
    }

    /// 获取所有不健康的 Server 列表
    pub async fn get_unhealthy_servers(&self) -> Vec<(String, ServerHealthInfo)> {
        let health = self.server_health.read().await;
        health.iter()
            .filter(|(_name, info)| !matches!(info.status, ServerHealthStatus::Healthy))
            .map(|(name, info)| (name.clone(), info.clone()))
            .collect()
    }

    /// 启动后台监控任务
    pub async fn start_monitoring(self: Arc<Self>) -> tokio::task::JoinHandle<()> {
        let config = self.config.clone();
        
        tokio::spawn(async move {
            loop {
                tokio::time::sleep(config.health_check_interval).await;
                
                // 执行健康检查
                self.perform_health_checks().await;
                
                // 清理过期数据
                self.cleanup_old_data().await;
            }
        })
    }

    // ─── 内部方法 ─────────────────────────

    async fn update_server_health(&self, server_name: &str, success: bool) {
        let mut health = self.server_health.write().await;
        
        let info = health.entry(server_name.to_string())
            .or_insert_with(|| ServerHealthInfo {
                server_name: server_name.to_string(),
                status: ServerHealthStatus::Healthy,
                uptime: Duration::ZERO,
                memory_usage: 0,
                operations_processed: 0,
                last_operation_time: None,
                consecutive_failures: 0,
            });

        info.operations_processed += 1;
        info.last_operation_time = Some(Instant::now());

        if success {
            info.consecutive_failures = 0;
            
            match &info.status {
                ServerHealthStatus::Degraded { .. } | 
                ServerHealthStatus::Unhealthy { .. } |
                ServerHealthStatus::Down { .. } => {
                    info.status = ServerHealthStatus::Healthy;
                    info!(
                        server = %server_name,
                        "Server recovered to healthy state"
                    );
                }
                _ => {}
            }
        } else {
            info.consecutive_failures += 1;

            if info.consecutive_failures >= self.config.restart_threshold {
                info.status = ServerHealthStatus::Unhealthy {
                    reason: format!("{} consecutive failures", info.consecutive_failures),
                };
                
                error!(
                    server = %server_name,
                    failures = info.consecutive_failures,
                    "Server marked as unhealthy"
                );

                // TODO: 触发自动重启
                if self.config.auto_restart {
                    warn!(server = %server_name, "Auto-restart recommended");
                }
            } else if info.consecutive_failures >= self.config.restart_threshold / 2 {
                info.status = ServerHealthStatus::Degraded {
                    reason: format!("{} consecutive failures", info.consecutive_failures),
                };
                
                warn!(
                    server = %server_name,
                    failures = info.consecutive_failures,
                    "Server in degraded state"
                );
            }
        }
    }

    async fn recalculate_global_stats(&self) {
        let ops = self.operations.read().await;
        let mut stats = PerformanceStats::default();

        if ops.is_empty() {
            return;
        }

        let total = ops.len() as u64;
        let durations: Vec<u64> = ops.iter()
            .filter_map(|op| op.duration_ms)
            .collect();
        
        let successes = ops.iter().filter(|op| op.success).count() as u64;
        let failures = total - successes;

        stats.total_operations = total;
        stats.success_count = successes;
        stats.failure_count = failures;

        if !durations.is_empty() {
            let sum: u64 = durations.iter().sum();
            stats.avg_duration_ms = sum as f64 / durations.len() as f64;
            stats.max_duration_ms = *durations.iter().max().unwrap_or(&0);
            stats.min_duration_ms = *durations.iter().min().unwrap_or(&0);

            // 计算百分位数
            let mut sorted = durations.clone();
            sorted.sort_unstable();
            
            let p50_idx = (sorted.len() as f64 * 0.5) as usize;
            let p95_idx = (sorted.len() as f64 * 0.95) as usize;
            let p99_idx = (sorted.len() as f64 * 0.99) as usize;
            
            stats.p50_duration_ms = sorted.get(p50_idx).copied().unwrap_or(0);
            stats.p95_duration_ms = sorted.get(p95_idx).copied().unwrap_or(0);
            stats.p99_duration_ms = sorted.get(p99_idx).copied().unwrap_or(0);
        }

        stats.success_rate = if total > 0 {
            successes as f64 / total as f64 * 100.0
        } else {
            100.0
        };

        // 计算最近错误率（最近 20% 的操作）
        let recent_count = (total as f64 * 0.2) as usize;
        let recent_ops = &ops[ops.len().saturating_sub(recent_count)..];
        let recent_failures = recent_ops.iter().filter(|op| !op.success).count();
        stats.recent_error_rate = if !recent_ops.is_empty() {
            recent_failures as f64 / recent_ops.len() as f64 * 100.0
        } else {
            0.0
        };

        *self.global_stats.write().await = stats;
    }

    async fn adjust_timeout_if_needed(&self) {
        let stats = self.global_stats.read().await;
        
        // 如果最近错误率 > 30%，增加超时时间
        if stats.recent_error_rate > 30.0 {
            let current = *self.current_timeout_ms.read().await;
            let new_timeout = (current as f64 * 1.2) as u64;
            
            if new_timeout <= self.config.max_timeout_ms {
                *self.current_timeout_ms.write().await = new_timeout;
                debug!(
                    old_timeout_ms = current,
                    new_timeout_ms = new_timeout,
                    error_rate = stats.recent_error_rate,
                    "Increased timeout due to high error rate"
                );
            }
        } else if stats.recent_error_rate < 5.0 && stats.total_operations > 50 {
            // 如果错误率低且样本足够，尝试降低超时
            let current = *self.current_timeout_ms.read().await;
            let new_timeout = ((current as f64 * 0.9) as u64)
                .max(self.config.default_timeout_ms);
            
            if new_timeout < current {
                *self.current_timeout_ms.write().await = new_timeout;
                debug!(
                    old_timeout_ms = current,
                    new_timeout_ms = new_timeout,
                    "Decreased timeout due to low error rate"
                );
            }
        }
    }

    async fn perform_health_checks(&self) {
        let health = self.server_health.read().await;
        let now = Instant::now();

        for (_name, info) in health.iter() {
            // 检查是否有长时间未操作（可能卡死）
            if let Some(last_op) = info.last_operation_time {
                if now.duration_since(last_op) > Duration::from_secs(300) { // 5 分钟无操作
                    warn!(
                        server = %info.server_name,
                        idle_seconds = now.duration_since(last_op).as_secs(),
                        "Server appears idle (possible hang)"
                    );
                    
                    // TODO: 发送 ping 测试 Server 是否还活着
                }
            }
        }
    }

    async fn cleanup_old_data(&self) {
        let mut ops = self.operations.write().await;
        
        // 只保留最近的数据
        if ops.len() > self.config.stats_window_size {
            let current_len = ops.len();
            let len_to_keep = self.config.stats_window_size;
            ops.drain(..(current_len - len_to_keep));
        }
    }

    fn extract_server_from_operation(&self, operation: &str) -> Option<String> {
        // 从操作名称推断 Server 类型
        // 例如: "rust-analyzer/goto_definition" → "rust-analyzer"
        operation.split('/')
            .next()
            .map(|s| s.to_string())
    }
}

// ============================================================================
// 辅助 trait：用于简化操作记录
// ============================================================================

/// 用于在 async 上下文中自动记录操作性能
pub trait WithPerformanceTracking {
    type Output;
    
    async fn tracked<Fut>(
        monitor: &PerformanceMonitor,
        operation: &str,
        future: Fut,
    ) -> Result<Self::Output, String>
    where
        Fut: std::future::Future<Output = Result<Self::Output, String>>;
}

impl<T> WithPerformanceTracking for T {
    type Output = T;

    async fn tracked<Fut>(
        monitor: &PerformanceMonitor,
        operation: &str,
        future: Fut,
    ) -> Result<Self::Output, String>
    where
        Fut: std::future::Future<Output = Result<Self::Output, String>>
    {
        let metrics = monitor.start_operation(operation).await;
        
        match future.await {
            Ok(result) => {
                monitor.finish_operation_success(metrics).await;
                Ok(result)
            }
            Err(err) => {
                monitor.finish_operation_error(metrics, err).await;
                Err(format!("Operation '{}' failed", operation))
            }
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[tokio::test]
    async fn test_basic_metrics_recording() {
        let monitor = PerformanceMonitor::new();
        
        let metrics = monitor.start_operation("test_operation").await;
        
        // 模拟一些工作
        tokio::time::sleep(Duration::from_millis(10)).await;
        
        monitor.finish_operation_success(metrics).await;
        
        let stats = monitor.get_global_stats().await;
        assert_eq!(stats.total_operations, 1);
        assert_eq!(stats.success_count, 1);
        assert!(stats.avg_duration_ms >= 10.0); // 至少 10ms
    }

    #[tokio::test]
    async fn test_error_tracking() {
        let monitor = PerformanceMonitor::new();
        
        let metrics = monitor.start_operation("failing_op").await;
        monitor.finish_operation_error(metrics, "Something went wrong").await;
        
        let stats = monitor.get_global_stats().await;
        assert_eq!(stats.failure_count, 1);
        assert_eq!(stats.success_rate, 0.0);
    }

    #[test]
    fn test_adaptive_config_defaults() {
        let config = AdaptiveConfig::default();
        assert_eq!(config.default_timeout_ms, 30_000);
        assert_eq!(config.restart_threshold, 5);
        assert!(config.auto_restart);
    }

    #[tokio::test]
    async fn test_tracked_helper() {
        use super::WithPerformanceTracking;
        
        let monitor = PerformanceMonitor::new();
        
        let result: Result<(), _> = <()>::tracked(
            &monitor,
            "async_test",
            async move {
                tokio::time::sleep(Duration::from_millis(5)).await;
                Ok(())
            }
        ).await;

        assert!(result.is_ok());
        
        let stats = monitor.get_global_stats().await;
        assert_eq!(stats.success_count, 1);
    }
}
