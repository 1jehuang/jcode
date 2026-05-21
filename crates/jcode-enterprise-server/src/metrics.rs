//! Prometheus 监控指标收集
//!
//! 提供：
//! - HTTP请求指标
//! - LLM推理指标
//! - 系统资源指标
//! - 业务指标

use prometheus::{
    Counter, CounterVec, Gauge, GaugeVec, Histogram, HistogramVec, Registry,
    Opts,
};
use std::sync::Arc;
use tracing::info;

/// 指标收集器
pub struct MetricsCollector {
    registry: Registry,

    // HTTP指标
    pub http_requests_total: CounterVec,
    pub http_request_duration_seconds: HistogramVec,
    pub http_errors_total: CounterVec,

    // LLM指标
    pub llm_tokens_processed: CounterVec,
    pub llm_request_latency: HistogramVec,
    pub llm_active_sessions: Gauge,
    pub llm_queue_depth: Gauge,

    // 系统指标
    pub system_cpu_usage: Gauge,
    pub system_memory_usage_bytes: Gauge,
    pub system_disk_io_bytes: CounterVec,

    // 业务指标
    pub active_organizations: Gauge,
    pub active_users: Gauge,
    pub total_api_keys: Gauge,
    pub quota_violations_total: CounterVec,
}

impl MetricsCollector {
    pub fn new() -> Result<Self, Box<dyn std::error::Error + Send + Sync>> {
        let registry = Registry::new();

        // HTTP请求总数
        let http_requests_total = CounterVec::new(
            Opts::new("http_requests_total", "Total number of HTTP requests")
                .namespace("carpai"),
            &["method", "endpoint", "status"],
        )?;
        registry.register(Box::new(http_requests_total.clone()))?;

        // HTTP请求延迟
        let http_request_duration_seconds = HistogramVec::new(
            prometheus::HistogramOpts::new(
                "http_request_duration_seconds",
                "HTTP request duration in seconds",
            )
            .namespace("carpai")
            .buckets(vec![0.001, 0.005, 0.01, 0.05, 0.1, 0.25, 0.5, 1.0, 2.5, 5.0, 10.0]),
            &["method", "endpoint"],
        )?;
        registry.register(Box::new(http_request_duration_seconds.clone()))?;

        // HTTP错误总数
        let http_errors_total = CounterVec::new(
            Opts::new("http_errors_total", "Total number of HTTP errors")
                .namespace("carpai"),
            &["method", "endpoint", "error_type"],
        )?;
        registry.register(Box::new(http_errors_total.clone()))?;

        // LLM Token处理数
        let llm_tokens_processed = CounterVec::new(
            Opts::new("llm_tokens_processed_total", "Total tokens processed by LLM")
                .namespace("carpai"),
            &["model", "direction"], // direction: input/output
        )?;
        registry.register(Box::new(llm_tokens_processed.clone()))?;

        // LLM请求延迟
        let llm_request_latency = HistogramVec::new(
            prometheus::HistogramOpts::new(
                "llm_request_latency_seconds",
                "LLM request latency in seconds",
            )
            .namespace("carpai")
            .buckets(vec![0.1, 0.5, 1.0, 2.0, 5.0, 10.0, 30.0, 60.0]),
            &["model"],
        )?;
        registry.register(Box::new(llm_request_latency.clone()))?;

        // 活跃会话数
        let llm_active_sessions = Gauge::with_opts(
            Opts::new("llm_active_sessions", "Number of active sessions")
                .namespace("carpai"),
        )?;
        registry.register(Box::new(llm_active_sessions.clone()))?;

        // 队列深度
        let llm_queue_depth = Gauge::with_opts(
            Opts::new("llm_queue_depth", "Current queue depth for LLM requests")
                .namespace("carpai"),
        )?;
        registry.register(Box::new(llm_queue_depth.clone()))?;

        // CPU使用率
        let system_cpu_usage = Gauge::with_opts(
            Opts::new("system_cpu_usage_percent", "System CPU usage percentage")
                .namespace("carpai"),
        )?;
        registry.register(Box::new(system_cpu_usage.clone()))?;

        // 内存使用
        let system_memory_usage_bytes = Gauge::with_opts(
            Opts::new("system_memory_usage_bytes", "System memory usage in bytes")
                .namespace("carpai"),
        )?;
        registry.register(Box::new(system_memory_usage_bytes.clone()))?;

        // 磁盘IO
        let system_disk_io_bytes = CounterVec::new(
            Opts::new("system_disk_io_bytes_total", "Total disk I/O in bytes")
                .namespace("carpai"),
            &["operation"], // read/write
        )?;
        registry.register(Box::new(system_disk_io_bytes.clone()))?;

        // 活跃组织数
        let active_organizations = Gauge::with_opts(
            Opts::new("active_organizations", "Number of active organizations")
                .namespace("carpai"),
        )?;
        registry.register(Box::new(active_organizations.clone()))?;

        // 活跃用户数
        let active_users = Gauge::with_opts(
            Opts::new("active_users", "Number of active users")
                .namespace("carpai"),
        )?;
        registry.register(Box::new(active_users.clone()))?;

        // API密钥总数
        let total_api_keys = Gauge::with_opts(
            Opts::new("total_api_keys", "Total number of API keys")
                .namespace("carpai"),
        )?;
        registry.register(Box::new(total_api_keys.clone()))?;

        // 配额违规次数
        let quota_violations_total = CounterVec::new(
            Opts::new("quota_violations_total", "Total quota violations")
                .namespace("carpai"),
            &["violation_type", "user_id"],
        )?;
        registry.register(Box::new(quota_violations_total.clone()))?;

        info!("✅ Prometheus指标收集器已初始化");

        Ok(Self {
            registry,
            http_requests_total,
            http_request_duration_seconds,
            http_errors_total,
            llm_tokens_processed,
            llm_request_latency,
            llm_active_sessions,
            llm_queue_depth,
            system_cpu_usage,
            system_memory_usage_bytes,
            system_disk_io_bytes,
            active_organizations,
            active_users,
            total_api_keys,
            quota_violations_total,
        })
    }

    /// 获取Prometheus格式的指标
    pub fn gather_metrics(&self) -> String {
        use prometheus::Encoder;
        let encoder = prometheus::TextEncoder::new();
        let metric_families = self.registry.gather();
        let mut buffer = Vec::new();
        encoder.encode(&metric_families, &mut buffer).unwrap();
        String::from_utf8(buffer).unwrap_or_default()
    }

    /// 记录HTTP请求
    pub fn record_http_request(&self, method: &str, endpoint: &str, status: u16, duration: f64) {
        self.http_requests_total
            .with_label_values(&[method, endpoint, &status.to_string()])
            .inc();

        self.http_request_duration_seconds
            .with_label_values(&[method, endpoint])
            .observe(duration);

        if status >= 400 {
            self.http_errors_total
                .with_label_values(&[method, endpoint, &format!("{}", status)])
                .inc();
        }
    }

    /// 记录LLM Token处理
    pub fn record_llm_tokens(&self, model: &str, direction: &str, count: u64) {
        self.llm_tokens_processed
            .with_label_values(&[model, direction])
            .inc_by(count as f64);
    }

    /// 记录LLM请求延迟
    pub fn record_llm_latency(&self, model: &str, latency: f64) {
        self.llm_request_latency
            .with_label_values(&[model])
            .observe(latency);
    }

    /// 设置活跃会话数
    pub fn set_active_sessions(&self, count: u32) {
        self.llm_active_sessions.set(count as f64);
    }

    /// 设置队列深度
    pub fn set_queue_depth(&self, depth: u32) {
        self.llm_queue_depth.set(depth as f64);
    }

    /// 更新CPU使用率
    pub fn set_cpu_usage(&self, percent: f64) {
        self.system_cpu_usage.set(percent);
    }

    /// 更新内存使用
    pub fn set_memory_usage(&self, bytes: u64) {
        self.system_memory_usage_bytes.set(bytes as f64);
    }

    /// 记录磁盘IO
    pub fn record_disk_io(&self, operation: &str, bytes: u64) {
        self.system_disk_io_bytes
            .with_label_values(&[operation])
            .inc_by(bytes as f64);
    }

    /// 设置活跃组织数
    pub fn set_active_organizations(&self, count: u32) {
        self.active_organizations.set(count as f64);
    }

    /// 设置活跃用户数
    pub fn set_active_users(&self, count: u32) {
        self.active_users.set(count as f64);
    }

    /// 设置API密钥总数
    pub fn set_total_api_keys(&self, count: u32) {
        self.total_api_keys.set(count as f64);
    }

    /// 记录配额违规
    pub fn record_quota_violation(&self, violation_type: &str, user_id: &str) {
        self.quota_violations_total
            .with_label_values(&[violation_type, user_id])
            .inc();
    }
}

impl Default for MetricsCollector {
    fn default() -> Self {
        Self::new().expect("Failed to create metrics collector")
    }
}

/// 共享的指标收集器
pub type SharedMetricsCollector = Arc<MetricsCollector>;

/// 创建Axum路由用于暴露/metrics端点
pub fn create_metrics_router(
    collector: SharedMetricsCollector,
) -> axum::Router {
    use axum::{routing::get, Router};

    Router::new()
        .route("/metrics", get(move || async move {
            collector.gather_metrics()
        }))
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_metrics_collector_creation() {
        let collector = MetricsCollector::new();
        assert!(collector.is_ok());
    }

    #[test]
    fn test_record_http_request() {
        let collector = MetricsCollector::new().unwrap();
        collector.record_http_request("GET", "/api/test", 200, 0.5);
        let metrics = collector.gather_metrics();
        assert!(metrics.contains("http_requests_total"));
    }

    #[test]
    fn test_record_llm_tokens() {
        let collector = MetricsCollector::new().unwrap();
        collector.record_llm_tokens("qwen-7b", "input", 100);
        let metrics = collector.gather_metrics();
        assert!(metrics.contains("llm_tokens_processed_total"));
    }
}
