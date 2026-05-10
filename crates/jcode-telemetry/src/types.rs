// ════════════════════════════════════════════════════════════════
// 性能监控核心类型 — 对应 Claude Code telemetry 系统
// ════════════════════════════════════════════════════════════════

use std::collections::HashMap;
use serde::{Deserialize, Serialize};

// ════════════════════════════════════════════════════════════════
// Metrics
// ════════════════════════════════════════════════════════════════

/// 指标键 (结构化标签)
#[derive(Debug, Clone, Hash, PartialEq, Eq, Serialize, Deserialize)]
pub enum MetricKey {
    // === LLM 相关 ===
    LlmRequestTotal { model: String },
    LlmLatencyMs { model: String },
    LlmInputTokens { model: String },
    LlmOutputTokens { model: String },
    LlmCacheHitTokens { model: String },
    LlmCacheWriteTokens { model: String },
    LlmErrors { model: String, error_type: String },
    
    // === 工具相关 ===  
    ToolExecutionTotal { tool_name: String },
    ToolLatencyMs { tool_name: String },
    ToolErrors { tool_name: String, error_type: String },
    
    // === 会话相关 ===
    SessionCreated,
    SessionCompleted,
    SessionAborted,
    
    // === 压缩相关 ===
    CompactExecuted { strategy: String },
    CompactTokensSaved,
    
    // === 成本相关 ===
    TotalCostUsd,
    
    // === 系统资源 ===
    MemoryUsageMb,
    CpuUsagePercent,
}

/// 指标值类型
#[derive(Debug, Clone, Serialize, Deserialize)]
pub enum MetricValue {
    Counter(u64),
    Gauge(f64),
    Histogram(f64),
}

// ════════════════════════════════════════════════════════════════
// Traces / Spans
// ════════════════════════════════════════════════════════════════

/// Span 类型分类
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Serialize, Deserialize)]
pub enum SpanKind {
    /// 根 span — 整个交互
    Internal,
    /// LLM API 调用
    Client,
    /// 工具执行
    Producer,
    /// 内部处理
    Consumer,
}

/// Span 上下文 — 用于关联父子关系
#[derive(Debug, Clone)]
pub struct SpanContext {
    pub trace_id: String,
    pub span_id: String,
    pub parent_span_id: Option<String>,
    pub depth: usize,
}

/// Span — 一个可计时的工作单元
#[derive(Debug, Clone)]
pub struct Span {
    context: SpanContext,
    name: String,
    kind: SpanKind,
    start_time: chrono::DateTime<chrono::Utc>,
    end_time: Option<chrono::DateTime<chrono::Utc>>,
    attributes: HashMap<String, serde_json::Value>,
    children: Vec<Span>,
    status: SpanStatus,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
pub enum SpanStatus {
    Unset,
    Ok,
    Error { description: String },
}

impl Span {
    /// 创建根 span
    pub fn root(name: impl Into<String>) -> Self {
        let name = name.into();
        Self {
            context: SpanContext {
                trace_id: uuid::Uuid::new_v4().to_string(),
                span_id: format!("{:016x}", rand::random::<u64>()()),
                parent_span_id: None,
                depth: 0,
            },
            name,
            kind: SpanKind::Internal,
            start_time: chrono::Utc::now(),
            end_time: None,
            attributes: HashMap::new(),
            children: Vec::new(),
            status: SpanStatus::Unset,
        }
    }
    
    /// 创建子 span
    pub fn child(&self, name: impl Into<String>) -> Self {
        let name = name.into();
        let parent_id = Some(self.context.span_id.clone());
        Self {
            context: SpanContext {
                trace_id: self.context.trace_id.clone(),
                span_id: format!("{:016x}", rand::random::<u64>()()),
                parent_span_id,
                depth: self.context.depth + 1,
            },
            name,
            kind: SpanKind::Internal,
            start_time: chrono::Utc::now(),
            end_time: None,
            attributes: HashMap::new(),
            children: Vec::new(),
            status: SpanStatus::Unset,
        }
    }

    /// 设置属性
    pub fn set_attribute(mut self, key: impl Into<String>, value: impl Into<serde_json::Value>) -> Self {
        self.attributes.insert(key.into(), value.into());
        self
    }
    
    /// 结束 span 并记录耗时
    pub fn finish(&mut self) -> Duration {
        let end = chrono::Utc::now();
        self.end_time = Some(end);
        end.signed_duration_since(self.start_time).to_std().unwrap_or_default()
    }

    /// 获取 span 深度
    pub fn depth(&self) -> usize {
        self.context.depth
    }
    
    /// 获取总耗时 (ms)
    pub fn duration_ms(&self) -> Option<f64> {
        self.end_time.map(|end| {
            end.signed_duration_since(self.start_time).num_milliseconds() as f64
        })
    }

    /// 是否已完成
    pub fn is_finished(&self) -> bool {
        self.end_time.is_some()
    }

    pub fn name(&self) -> &str {
        &self.name
    }

    pub fn status(&self) -> &SpanStatus {
        &self.status
    }
}

use std::time::Duration;

// ════════════════════════════════════════════════════════════════
// Cost Tracking
// ════════════════════════════════════════════════════════════════

/// 单次 API 调用的成本明细
#[derive(Debug, Clone, Default, Serialize, Deserialize)]
pub struct ApiCostDetail {
    pub model: String,
    pub input_tokens: u32,
    pub output_tokens: u32,
    pub cache_read_input_tokens: u32,
    pub cache_write_input_tokens: u32,
    pub cost_usd: f64,
}

/// 会话累计成本
#[derive(Debug, Clone, Default, Serialize, Deserialize)]
pub struct SessionCostSummary {
    pub total_cost_usd: f64,
    pub total_input_tokens: u64,
    pub total_output_tokens: u64,
    pub total_cache_read_tokens: u64,
    pub api_calls: u32,
    pub by_model: HashMap<String, ModelCostBreakdown>,
}

#[derive(Debug, Clone, Default, Serialize, Deserialize)]
pub struct ModelCostBreakdown {
    pub calls: u32,
    pub cost_usd: f64,
    pub input_tokens: u64,
    pub output_tokens: u64,
}

// ════════════════════════════════════════════════════════════════
// Resource Monitoring
// ════════════════════════════════════════════════════════════════

/// 资源使用快照
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct ResourceSnapshot {
    /// 时间戳
    pub timestamp: chrono::DateTime<chrono::Utc>,
    
    /// 内存使用量 (MB)
    pub memory_used_mb: u64,
    
    /// 内存总量 (MB)
    pub memory_total_mb: u64,
    
    /// CPU 使用率 (%), None 表示无法获取
    pub cpu_usage_percent: Option<f64>,
    
    /// 堆内存使用量 (MB, 仅运行时支持)
    pub heap_used_mb: Option<u64>,
    
    /// 磁盘使用信息
    pub disk_usage: Vec<DiskUsageInfo>,
    
    /// 活跃连接数
    pub active_connections: u32,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct DiskUsageInfo {
    pub mount_point: String,
    pub used_gb: f64,
    pub total_gb: f64,
    pub usage_percent: f64,
}

/// 健康状态
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub enum HealthStatus {
    Healthy,
    Degraded,
    Unhealthy,
}

/// 健康检查结果
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct HealthCheckResult {
    pub overall_status: HealthStatus,
    pub checks: HashMap<String, ComponentHealth>,
    pub timestamp: chrono::DateTime<chrono::Utc>,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct ComponentHealth {
    pub status: HealthStatus,
    pub message: Option<String>,
    pub latency_ms: Option<u64>,
}

/// 慢操作记录
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct SlowOperationRecord {
    pub operation_type: SlowOperationType,
    pub duration_ms: u64,
    pub threshold_ms: u64,
    pub details: serde_json::Value,
    pub timestamp: chrono::DateTime<chrono::Utc>,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub enum SlowOperationType {
    LlmRequest,
    ToolExecution(String),
    FileOperation,
    NetworkRequest,
}
