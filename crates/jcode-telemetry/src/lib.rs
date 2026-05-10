// jcode-telemetry
// ════════════════════════════════════════════════════════════════
// 性能监控与诊断系统 - 移植自 Claude Code
//
// 核心能力:
//   1. OpenTelemetry 集成 — Metrics + Traces + Logs 三信号
//   2. Span 层次结构 — Interaction > LLM Request > Tool Execution
//   3. 延迟分解 — TTFT / TTLT / Tool Latency 分阶段计时
//   4. Token 追踪 — input/output/cache_read/cache_write/cost
//   5. 成本预算管理 — 实时成本追踪 + 预算告警
//   6. 资源监控 — CPU/Memory/Heap 使用量追踪
//   7. 慢请求检测 — 超过阈值自动标记
//   8. 健康检查 — 系统自检 + Provider 连通性
//
// 对应 Claude Code 源码:
//   - src/utils/telemetry/instrumentation.ts (826行) — OTel 初始化
//   - src/utils/telemetry/sessionTracing.ts (928行) — Span 层次
//   - src/utils/telemetry/events.ts (75行) — 事件格式
//   - src/utils/telemetry/betaSessionTracing.ts (492行) — 增强追踪
//   - src/utils/cost-tracker.ts — 成本追踪
//   - src/utils/stats.ts / statsCache.ts — 统计缓存
// ════════════════════════════════════════════════════════════════

mod types;
mod metrics;
mod tracer;
mod cost_tracker;
mod resource_monitor;
mod health_check;
mod slow_operation_detector;
pub mod commit_attribution;

pub use types::*;
pub use metrics::{MetricsCollector, MetricKey, MetricValue};
pub use tracer::{
    TelemetryTracer,
    SpanContext,
    Span,
    SpanKind,
};
pub use cost_tracker::CostTracker;
pub use resource_monitor::ResourceMonitor;
pub use health_check::HealthChecker;
pub use commit_attribution::{
    CommitAttributionTracker, CommitAttribution, FileAttribution,
    FileChangeKind, AttributionSnapshot,
};

/// 默认指标收集间隔 (ms)
pub const DEFAULT_METRICS_INTERVAL_MS: u64 = 60_000;  // 1 min

/// 默认日志导出间隔 (ms)
pub const DEFAULT_LOGS_INTERVAL_MS: u64 = 5_000;  // 5 sec

/// 默认 Trace 导出间隔 (ms)
pub const DEFAULT_TRACES_INTERVAL_MS: u64 = 5_000;  // 5 sec

/// 慢操作阈值 (ms) — 超过此值被记录为慢操作
pub const SLOW_OPERATION_THRESHOLD_MS: u64 = 10_000;  // 10s

/// 慢 API 请求阈值 (ms)
pub const SLOW_API_THRESHOLD_MS: u64 = 30_000;  // 30s

/// 慢工具执行阈值 (ms)
pub const SLOW_TOOL_THRESHOLD_MS: u64 = 60_000;  // 60s

/// 内存使用告警阈值 (MB)
pub const MEMORY_WARNING_THRESHOLD_MB: u64 = 512;

/// 内存使用严重告警阈值 (MB)  
 pub const MEMORY_CRITICAL_THRESHOLD_MB: u64 = 1024;

/// 最大 Span 层级深度
pub const MAX_SPAN_DEPTH: usize = 20;

/// Token 成本估算表 (USD per 1M tokens, 近似值)
pub fn estimate_token_cost(model_id: &str, input_tokens: u32, output_tokens: u32) -> f64 {
    let rates = match model_id.to_lowercase().as_str() {
        s if s.contains("claude-3-5") || s.contains("claude-sonnet-4") => (3.0, 15.0),
        s if s.contains("claude-3-opus") => (15.0, 75.0),
        s if s.contains("claude-3-haiku") => (0.25, 1.25),
        s if s.contains("gpt-4o") || s.contains("gpt-4-turbo") => (5.0, 15.0),
        s if s.contains("gpt-4") && !s.contains("turbo") && !s.contains("o") => (30.0, 60.0),
        s if s.contains("gpt-3.5") || s.contains("gpt-35") => (0.5, 1.5),
        s if s.contains("deepseek") || s.contains("qwen") => (0.14, 0.28),  // 国产模型通常更便宜
        _ => (2.0, 6.0),  // 默认中档价格
    };
    
    let input_cost = (input_tokens as f64 / 1_000_000.0) * rates.0;
    let output_cost = (output_tokens as f64 / 1_000_000.0) * rates.1;
    input_cost + output_cost
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_token_cost_estimation() {
        // Claude Sonnet 价格
        let cost = estimate_token_cost("claude-3-5-sonnet", 1000, 500);
        assert!(cost > 0.0);
        assert!(cost < 0.01);  // 应该很便宜
        
        // 国产 DeepSeek 应该更便宜
        let cost_cn = estimate_token_cost("deepseek-chat", 1000, 500);
        assert!(cost_cn < cost);
    }

    #[tokio::test]
    async fn test_resource_monitor() {
        let monitor = ResourceMonitor::new();
        
        let snapshot = monitor.snapshot().await;
        assert!(snapshot.memory_used_mb > 0);
        
        // CPU 使用率在 0-100 之间
        if let Some(cpu) = snapshot.cpu_usage_percent {
            assert!(cpu >= 0.0 && cpu <= 100.0);
        }
    }

    #[test]
    fn test_span_hierarchy() {
        let root_span = Span::root("Interaction");
        let llm_span = root_span.child("LLM Request");
        let tool_span = llm_span.child("Tool Execution");
        
        assert_eq!(root_span.depth(), 0);
        assert_eq!(llm_span.depth(), 1);
        assert_eq!(tool_span.depth(), 2);
    }

    #[test]
    fn test_health_check_categories() {
        let checker = HealthChecker::new();
        
        // 所有类别应存在
        let categories = checker.check_categories();
        assert!(categories.len() >= 4);  // 至少 system, memory, disk, network
    }

    #[tokio::test]  
    async fn test_metrics_collection() {
        let collector = MetricsCollector::new();
        
        collector.record(MetricKey::LlmRequestTotal { model: "test".into() }, MetricValue::Counter(1));
        collector.record(MetricKey::LlmLatencyMs { model: "test".into() }, MetricValue::Histogram(150.0));
        
        let snapshot = collector.snapshot();
        assert!(!snapshot.is_empty());
    }
}
