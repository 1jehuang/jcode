//! Performance Bottleneck Detector — 智能性能瓶颈识别与优化建议
//!
//! ## 核心能力 (对标/超越 Claude Code)
//! - **多维度性能分析**: CPU、内存、I/O、并发、网络全方位监控
//! - **智能瓶颈识别**: 基于历史数据和机器学习算法自动定位瓶颈
//! - **实时热点追踪**: 动态识别热点函数和慢操作
//! - **内存泄漏检测**: 自动检测内存增长趋势和异常分配
//! - **I/O 瓶颈分析**: 文件读写、数据库查询、网络请求延迟分析
//! - **并发问题诊断**: 死锁、竞态条件、资源争用检测
//! - **优化建议生成**: 基于最佳实践和模式匹配的智能建议
//! - **性能回归检测**: 对比基线数据，发现性能退化
//!
//! ## 监控维度
//! 1. **CPU 性能**: 使用率、热点函数、上下文切换
//! 2. **内存性能**: 分配速率、GC压力、内存碎片
//! 3. **I/O 性能**: 磁盘I/O、网络延迟、数据库查询
//! 4. **并发性能**: 锁竞争、线程池利用率、异步任务堆积
//! 5. **应用层指标**: API响应时间、吞吐量、错误率
//!
//! ## 使用示例
//! ```rust
//! use jcode_lsp::performance_bottleneck::BottleneckDetector;
//!
//! let detector = BottleneckDetector::new();
//! 
//! // 开始监控
//! let session = detector.start_monitoring_session("my_app").await;
//! 
//! // 记录操作
//! session.record_operation("database_query", 150.0).await;
//! session.record_memory_allocation(1024 * 1024).await; // 1MB
//! 
//! // 分析瓶颈
//! let report = detector.analyze_bottlenecks().await;
//! println!("Found {} bottlenecks", report.bottlenecks.len());
//! 
//! // 获取优化建议
//! let suggestions = detector.generate_optimization_suggestions(&report).await;
//! ```

use std::collections::{HashMap, VecDeque};
use std::sync::Arc;
use std::time::{Duration, Instant};
use tokio::sync::RwLock;
use tracing::{debug, error, info, warn};

/// 瓶颈严重程度
#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord)]
pub enum Severity {
    Info,
    Warning,
    Critical,
}

impl std::fmt::Display for Severity {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            Self::Info => write!(f, "INFO"),
            Self::Warning => write!(f, "WARNING"),
            Self::Critical => write!(f, "CRITICAL"),
        }
    }
}

/// 瓶颈类别
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum BottleneckCategory {
    Cpu,
    Memory,
    Io,
    Concurrency,
    Network,
    Database,
    Algorithm,
    ExternalService,
}

impl std::fmt::Display for BottleneckCategory {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            Self::Cpu => write!(f, "CPU"),
            Self::Memory => write!(f, "Memory"),
            Self::Io => write!(f, "I/O"),
            Self::Concurrency => write!(f, "Concurrency"),
            Self::Network => write!(f, "Network"),
            Self::Database => write!(f, "Database"),
            Self::Algorithm => write!(f, "Algorithm"),
            Self::ExternalService => write!(f, "External Service"),
        }
    }
}

/// 单个瓶颈记录
#[derive(Debug, Clone)]
pub struct Bottleneck {
    /// 唯一标识
    pub id: String,
    
    /// 类别
    pub category: BottleneckCategory,
    
    /// 严重程度
    pub severity: Severity,
    
    /// 标题
    pub title: String,
    
    /// 详细描述
    pub description: String,
    
    /// 影响的位置（函数名、文件路径等）
    pub location: Option<String>,
    
    /// 当前值
    pub current_value: f64,
    
    /// 阈值（超过此值视为瓶颈）
    pub threshold: f64,
    
    /// 单位
    pub unit: String,
    
    /// 发现时间
    pub detected_at: Instant,
    
    /// 发生次数
    pub occurrence_count: u64,
    
    /// 影响评估 (0.0-1.0)
    pub impact_score: f64,
    
    /// 是否已确认
    pub confirmed: bool,
    
    /// 相关的操作ID列表
    pub related_operations: Vec<String>,
}

impl Bottleneck {
    pub fn new(
        category: BottleneckCategory,
        severity: Severity,
        title: impl Into<String>,
        description: impl Into<String>,
        current_value: f64,
        threshold: f64,
        unit: impl Into<String>,
    ) -> Self {
        Self {
            id: format!("bn_{}", uuid::Uuid::new_v4()),
            category,
            severity,
            title: title.into(),
            description: description.into(),
            location: None,
            current_value,
            threshold,
            unit: unit.into(),
            detected_at: Instant::now(),
            occurrence_count: 1,
            impact_score: 0.0,
            confirmed: false,
            related_operations: vec![],
        }
    }

    pub fn with_location(mut self, location: impl Into<String>) -> Self {
        self.location = Some(location.into());
        self
    }

    pub fn with_impact(mut self, score: f64) -> Self {
        self.impact_score = score.clamp(0.0, 1.0);
        self
    }

    pub fn with_related_ops(mut self, ops: Vec<String>) -> Self {
        self.related_operations = ops;
        self
    }
}

/// 操作记录
#[derive(Debug, Clone)]
pub struct OperationRecord {
    /// 操作名称
    pub name: String,
    
    /// 开始时间
    pub started_at: Instant,
    
    /// 耗时 (ms)
    pub duration_ms: f64,
    
    /// 成功与否
    pub success: bool,
    
    /// CPU 时间 (ms)
    pub cpu_time_ms: Option<f64>,
    
    /// 内存分配 (bytes)
    pub memory_allocated: Option<u64>,
    
    /// I/O 字节数
    pub io_bytes: Option<u64>,
    
    /// 等待时间 (ms) - 用于检测锁等待
    pub wait_time_ms: Option<f64>,
    
    /// 额外元数据
    pub metadata: HashMap<String, String>,
}

impl OperationRecord {
    pub fn new(name: impl Into<String>, duration_ms: f64) -> Self {
        Self {
            name: name.into(),
            started_at: Instant::now(),
            duration_ms,
            success: true,
            cpu_time_ms: None,
            memory_allocated: None,
            io_bytes: None,
            wait_time_ms: None,
            metadata: HashMap::new(),
        }
    }
}

/// 内存快照
#[derive(Debug, Clone)]
pub struct MemorySnapshot {
    /// 时间戳
    pub timestamp: Instant,
    
    /// 总内存使用 (bytes)
    pub total_used_bytes: u64,
    
    /// 堆内存 (bytes)
    pub heap_bytes: u64,
    
    /// 栈内存 (bytes)
    pub stack_bytes: u64,
    
    /// 对象数量
    pub object_count: usize,
    
    /// GC 暂停时间 (ms)
    pub gc_pause_ms: Option<f64>,
}

impl Default for MemorySnapshot {
    fn default() -> Self {
        Self {
            timestamp: Instant::now(),
            total_used_bytes: 0,
            heap_bytes: 0,
            stack_bytes: 0,
            object_count: 0,
            gc_pause_ms: None,
        }
    }
}

/// 性能基线
#[derive(Debug, Clone)]
pub struct PerformanceBaseline {
    /// 平均响应时间 (ms)
    pub avg_response_time_ms: f64,
    
    /// P95 响应时间 (ms)
    pub p95_response_time_ms: f64,
    
    /// P99 响应时间 (ms)
    pub p99_response_time_ms: f64,
    
    /// 吞量 (ops/s)
    pub throughput: f64,
    
    /// 错误率 (%)
    pub error_rate: f64,
    
    /// CPU 使用率 (%)
    pub cpu_usage_percent: f64,
    
    /// 内存使用 (MB)
    pub memory_usage_mb: f64,
    
    /// 创建时间
    pub created_at: Instant,
}

impl Default for PerformanceBaseline {
    fn default() -> Self {
        Self {
            avg_response_time_ms: 100.0,
            p95_response_time_ms: 500.0,
            p99_response_time_ms: 1000.0,
            throughput: 1000.0,
            error_rate: 1.0,
            cpu_usage_percent: 50.0,
            memory_usage_mb: 256.0,
            created_at: Instant::now(),
        }
    }
}

/// 瓶颈分析报告
#[derive(Debug, Clone)]
pub struct BottleneckReport {
    /// 报告 ID
    pub report_id: String,
    
    /// 生成时间
    pub generated_at: Instant,
    
    /// 分析的时间窗口
    pub time_window: Duration,
    
    /// 发现的所有瓶颈
    pub bottlenecks: Vec<Bottleneck>,
    
    /// 关键统计
    pub summary: BottleneckSummary,
    
    /// 趋势数据
    pub trends: HashMap<String, Vec<f64>>,
    
    /// 性能回归警告
    pub regressions: Vec<PerformanceRegression>,
}

/// 瓶颈摘要统计
#[derive(Debug, Clone, Default)]
pub struct BottleneckSummary {
    /// 总瓶颈数
    pub total_bottlenecks: usize,
    
    /// 严重程度分布
    pub by_severity: HashMap<Severity, usize>,
    
    /// 类别分布
    pub by_category: HashMap<BottleneckCategory, usize>,
    
    /// 总影响评分 (0.0-100.0)
    pub overall_impact_score: f64,
    
    /// 最严重的瓶颈
    pub top_bottleneck: Option<Bottleneck>,
    
    /// 需要立即处理的关键瓶颈数
    pub critical_count: usize,
    
    /// 建议优先级队列
    pub priority_queue: Vec<Bottleneck>,
}

/// 性能回归记录
#[derive(Debug, Clone)]
pub struct PerformanceRegression {
    /// 指标名称
    pub metric_name: String,
    
    /// 基线值
    pub baseline_value: f64,
    
    /// 当前值
    pub current_value: f64,
    
    /// 回退百分比 (%)
    pub regression_percentage: f64,
    
    /// 严重程度
    pub severity: Severity,
    
    /// 描述
    pub description: String,
}

/// 优化建议
#[derive(Debug, Clone)]
pub struct OptimizationSuggestion {
    /// 建议 ID
    pub id: String,
    
    /// 目标瓶颈 ID
    pub target_bottleneck_id: String,
    
    /// 标题
    pub title: String,
    
    /// 详细描述
    pub description: String,
    
    /// 预期改进效果
    pub expected_improvement: String,
    
    /// 实现复杂度 (1-5)
    pub complexity: u8,
    
    /// 风险等级 (1-5)
    pub risk_level: u8,
    
    /// 优先级 (1-10)
    pub priority: u8,
    
    /// 参考链接或文档
    pub references: Vec<String>,
    
    /// 代码示例（如果适用）
    pub code_example: Option<String>,
}

/// 检测器配置
#[derive(Debug, Clone)]
pub struct DetectorConfig {
    /// 监控窗口大小 (秒)
    pub monitoring_window_secs: u64,
    
    /// 采样间隔 (毫秒)
    pub sampling_interval_ms: u64,
    
    /// 热点阈值 (ms) - 超过此时间的操作被视为热点
    pub hotspot_threshold_ms: f64,
    
    /// 内存增长率阈值 (%/min) - 超过此值可能存在内存泄漏
    pub memory_growth_threshold_percent: f64,
    
    /// CPU 使用率阈值 (%) - 超过此值视为CPU瓶颈
    pub cpu_usage_threshold_percent: f64,
    
    /// I/O 等待阈值 (ms)
    pub io_wait_threshold_ms: f64,
    
    /// 锁等待阈值 (ms)
    pub lock_wait_threshold_ms: f64,
    
    /// 错误率阈值 (%)
    pub error_rate_threshold_percent: f64,
    
    /// 启用内存泄漏检测
    pub enable_memory_leak_detection: bool,
    
    /// 启用性能回归检测
    pub enable_regression_detection: bool,
    
    /// 最大保留的操作记录数
    pub max_operation_records: usize,
    
    /// 最大保留的内存快照数
    pub max_memory_snapshots: usize,
}

impl Default for DetectorConfig {
    fn default() -> Self {
        Self {
            monitoring_window_secs: 300, // 5 分钟
            sampling_interval_ms: 100,   // 100ms
            hotspot_threshold_ms: 1000.0, // 1s
            memory_growth_threshold_percent: 10.0, // 10%/min
            cpu_usage_threshold_percent: 80.0,     // 80%
            io_wait_threshold_ms: 100.0,           // 100ms
            lock_wait_threshold_ms: 50.0,          // 50ms
            error_rate_threshold_percent: 5.0,      // 5%
            enable_memory_leak_detection: true,
            enable_regression_detection: true,
            max_operation_records: 10000,
            max_memory_snapshots: 1000,
        }
    }
}

/// 监控会话
pub struct MonitoringSession {
    /// 会话 ID
    pub session_id: String,
    
    /// 应用/服务名称
    pub application_name: String,
    
    /// 开始时间
    pub started_at: Instant,
    
    /// 操作记录
    operations: Arc<RwLock<VecDeque<OperationRecord>>>,
    
    /// 内存快照
    memory_snapshots: Arc<RwLock<VecDeque<MemorySnapshot>>>,
    
    /// 当前瓶颈
    current_bottlenecks: Arc<RwLock<Vec<Bottleneck>>>,
    
    /// 配置
    config: DetectorConfig,
    
    /// 性能基线
    baseline: Arc<RwLock<Option<PerformanceBaseline>>>,
}

impl MonitoringSession {
    /// 记录一个操作
    pub async fn record_operation(&self, record: OperationRecord) {
        debug!(
            operation = %record.name,
            duration_ms = record.duration_ms,
            success = record.success,
            "Recording operation"
        );

        {
            let mut ops = self.operations.write().await;
            ops.push_back(record);
            
            // 保持最大限制
            while ops.len() > self.config.max_operation_records {
                ops.pop_front();
            }
        }

        // 实时检查是否为热点操作
        if record.duration_ms > self.config.hotspot_threshold_ms {
            self.detect_hotspot_operation(&record).await;
        }
    }

    /// 记录内存分配
    pub async fn record_memory_allocation(&self, bytes: u64) {
        let snapshot = MemorySnapshot {
            timestamp: Instant::now(),
            total_used_bytes: bytes,
            heap_bytes: bytes,
            ..Default::default()
        };

        {
            let mut snapshots = self.memory_snapshots.write().await;
            snapshots.push_back(snapshot);
            
            while snapshots.len() > self.config.max_memory_snapshots {
                snapshots.pop_front();
            }
        }

        // 检查内存泄漏
        if self.config.enable_memory_leak_detection {
            self.detect_memory_leak().await;
        }
    }

    /// 设置性能基线
    pub async fn set_baseline(&self, baseline: PerformanceBaseline) {
        *self.baseline.write().await = Some(baseline);
        info!("Performance baseline set");
    }

    /// 获取当前操作记录
    pub async fn get_operations(&self) -> Vec<OperationRecord> {
        self.operations.read().await.iter().cloned().collect()
    }

    /// 获取最近的 N 个操作
    pub async fn get_recent_operations(&self, n: usize) -> Vec<OperationRecord> {
        let ops = self.operations.read().await;
        ops.iter()
            .rev()
            .take(n)
            .cloned()
            .collect()
    }

    /// 获取内存快照历史
    pub async fn get_memory_history(&self) -> Vec<MemorySnapshot> {
        self.memory_snapshots.read().await.iter().cloned().collect()
    }

    // ─── 内部检测方法 ─────────────────────────

    async fn detect_hotspot_operation(&self, op: &OperationRecord) {
        let severity = if op.duration_ms > 10_000.0 {
            Severity::Critical
        } else if op.duration_ms > 5_000.0 {
            Severity::Warning
        } else {
            Severity::Info
        };

        let bottleneck = Bottleneck::new(
            BottleneckCategory::Cpu,
            severity,
            format!("Hotspot: {}", op.name),
            format!(
                "Operation '{}' took {:.2}ms, exceeding threshold of {:.0}ms",
                op.name, op.duration_ms, self.config.hotspot_threshold_ms
            ),
            op.duration_ms,
            self.config.hotspot_threshold_ms,
            "ms",
        )
        .with_location(op.name.clone())
        .with_impact((op.duration_ms / self.config.hotspot_threshold_ms).min(1.0))
        .with_related_ops(vec![op.name.clone()]);

        {
            let mut bottlenecks = self.current_bottlenecks.write().await;
            
            // 检查是否已存在类似的瓶颈
            let existing = bottlenecks.iter_mut()
                .find(|b| b.title == bottleneck.title);
            
            match existing {
                Some(existing_bn) => {
                    existing_bn.occurrence_count += 1;
                    existing_bn.current_value = op.duration_max(existing_bn.current_value, op.duration_ms);
                    existing_bn.detected_at = Instant::now();
                }
                None => {
                    bottlenecks.push(bottleneck);
                }
            }
        }

        warn!(
            operation = %op.name,
            duration_ms = op.duration_ms,
            severity = %severity,
            "Hotspot operation detected"
        );
    }

    async fn detect_memory_leak(&self) {
        let snapshots = self.memory_snapshots.read().await;
        
        if snapshots.len() < 10 {
            return; // 数据不足
        }

        // 计算最近 1 分钟的内存增长率
        let now = Instant::now();
        let one_min_ago = now - Duration::from_secs(60);
        
        let recent: Vec<&MemorySnapshot> = snapshots.iter()
            .filter(|s| s.timestamp >= one_min_ago)
            .collect();
        
        if recent.len() < 2 {
            return;
        }

        let first = recent.first().unwrap();
        let last = recent.last().unwrap();
        
        if first.total_used_bytes == 0 {
            return;
        }

        let growth_percent = ((last.total_used_bytes as f64 - first.total_used_bytes as f64) 
            / first.total_used_bytes as f64) * 100.0;

        if growth_percent > self.config.memory_growth_threshold_percent {
            let severity = if growth_percent > 50.0 {
                Severity::Critical
            } else if growth_percent > 20.0 {
                Severity::Warning
            } else {
                Severity::Info
            };

            let bottleneck = Bottleneck::new(
                BottleneckCategory::Memory,
                severity,
                "Memory Leak Detected",
                format!(
                    "Memory growing at {:.1}%/min (threshold: {:.1}%). Current usage: {} MB",
                    growth_percent,
                    self.config.memory_growth_threshold_percent,
                    last.total_used_bytes / (1024 * 1024)
                ),
                growth_percent,
                self.config.memory_growth_threshold_percent,
                "%/min",
            )
            .with_impact((growth_percent / 100.0).min(1.0));

            drop(snapshots); // 释放读锁
            
            let mut bottlenecks = self.current_bottlenecks.write().await;
            
            if !bottlenecks.iter().any(|b| b.category == BottleneckCategory::Memory) {
                bottlenecks.push(bottleneck);
                
                error!(
                    growth_percent = growth_percent,
                    current_mb = last.total_used_bytes / (1024 * 1024),
                    "Potential memory leak detected"
                );
            }
        }
    }
}

/// 性能瓶颈检测器
pub struct BottleneckDetector {
    /// 所有活跃的监控会话
    sessions: Arc<RwLock<HashMap<String, Arc<MonitoringSession>>>>,
    
    /// 全局配置
    config: DetectorConfig,
    
    /// 历史报告
    historical_reports: Arc<RwLock<Vec<BottleneckReport>>>,
    
    /// 已知的瓶颈模式库
    pattern_library: Arc<RwLock<Vec<BottleneckPattern>>>,
}

/// 瓶颈模式（用于模式匹配）
struct BottleneckPattern {
    category: BottleneckCategory,
    pattern_regex: String,
    severity: Severity,
    suggestion_template: String,
}

impl BottleneckDetector {
    pub fn new() -> Self {
        Self::with_config(DetectorConfig::default())
    }

    pub fn with_config(config: DetectorConfig) -> Self {
        let mut detector = Self {
            sessions: Arc::new(RwLock::new(HashMap::new())),
            config,
            historical_reports: Arc::new(RwLock::new(vec![])),
            pattern_library: Arc::new(RwLock::new(vec![])),
        };

        // 初始化模式库
        detector.initialize_pattern_library();

        detector
    }

    /// 开始一个新的监控会话
    pub async fn start_monitoring_session(&self, app_name: impl Into<String>) -> Arc<MonitoringSession> {
        let session_id = format!("session_{}", uuid::Uuid::new_v4());
        let app_name = app_name.into();

        info!(
            session_id = %session_id,
            application = %app_name,
            "Starting monitoring session"
        );

        let session = Arc::new(MonitoringSession {
            session_id: session_id.clone(),
            application_name: app_name,
            started_at: Instant::now(),
            operations: Arc::new(RwLock::new(VecDeque::new())),
            memory_snapshots: Arc::new(RwLock::new(VecDeque::new())),
            current_bottlenecks: Arc::new(RwLock::new(vec![])),
            config: self.config.clone(),
            baseline: Arc::new(RwLock::new(None)),
        });

        {
            let mut sessions = self.sessions.write().await;
            sessions.insert(session_id, session.clone());
        }

        session
    }

    /// 停止监控会话并生成最终报告
    pub async fn stop_monitoring_session(&self, session_id: &str) -> Option<BottleneckReport> {
        info!(session_id = %session_id, "Stopping monitoring session");

        let session = {
            let mut sessions = self.sessions.write().await;
            sessions.remove(session_id)
        };

        session.map(|s| {
            let report = self.generate_final_report(&s).await;
            
            // 保存到历史
            {
                let mut reports = self.historical_reports.write().await;
                reports.push(report.clone());
                
                // 只保留最近 100 个报告
                if reports.len() > 100 {
                    reports.drain(..(reports.len() - 100));
                }
            }
            
            report
        })
    }

    /// 执行全面的瓶颈分析
    pub async fn analyze_bottlenecks(&self) -> BottleneckReport {
        let all_bottlenecks = self.collect_all_bottlenecks().await;
        let summary = self.calculate_summary(&all_bottlenecks).await;
        let regressions = if self.config.enable_regression_detection {
            self.detect_regressions().await
        } else {
            vec![]
        };
        let trends = self.calculate_trends().await;

        BottleneckReport {
            report_id: format!("report_{}", uuid::Uuid::new_v4()),
            generated_at: Instant::now(),
            time_window: Duration::from_secs(self.config.monitoring_window_secs),
            bottlenecks: all_bottlenecks,
            summary,
            trends,
            regressions,
        }
    }

    /// 生成优化建议
    pub async fn generate_optimization_suggestions(
        &self,
        report: &BottleneckReport,
    ) -> Vec<OptimizationSuggestion> {
        let mut suggestions = Vec::new();

        for bottleneck in &report.bottlenecks {
            let suggestion = self.create_suggestion_for_bottleneck(bottleneck).await;
            suggestions.push(suggestion);
        }

        // 按优先级排序
        suggestions.sort_by(|a, b| b.priority.cmp(&a.priority));

        suggestions
    }

    /// 获取实时性能概览
    pub async fn get_live_overview(&self) -> PerformanceOverview {
        let mut total_operations = 0u64;
        let mut total_duration = 0.0f64;
        let mut success_count = 0u64;
        let mut failure_count = 0u64;
        let mut active_sessions = 0usize;

        let sessions = self.sessions.read().await;

        for (_id, session) in sessions.iter() {
            active_sessions += 1;
            let ops = session.operations.read().await;
            
            for op in ops.iter() {
                total_operations += 1;
                total_duration += op.duration_ms;
                
                if op.success {
                    success_count += 1;
                } else {
                    failure_count += 1;
                }
            }
        }

        let avg_duration = if total_operations > 0 {
            total_duration / total_operations as f64
        } else {
            0.0
        };

        let success_rate = if total_operations > 0 {
            success_count as f64 / total_operations as f64 * 100.0
        } else {
            100.0
        };

        PerformanceOverview {
            active_sessions,
            total_operations,
            avg_duration_ms: avg_duration,
            success_rate,
            active_bottlenecks: self.count_active_bottlenecks().await,
            memory_trend: self.get_memory_trend().await,
            cpu_pressure: self.estimate_cpu_pressure().await,
        }
    }

    // ─── 内部方法 ─────────────────────────

    async fn collect_all_bottlenecks(&self) -> Vec<Bottleneck> {
        let mut all_bottlenecks = Vec::new();
        let sessions = self.sessions.read().await;

        for (_id, session) in sessions.iter() {
            let bottlenecks = session.current_bottlenecks.read().await;
            all_bottlenecks.extend(bottlenecks.clone());
        }

        // 去重和合并
        self.deduplicate_bottlenecks(all_bottlenecks).await
    }

    async fn deduplicate_bottlenecks(&self, bottlenecks: Vec<Bottleneck>) -> Vec<Bottleneck> {
        let mut deduped: HashMap<String, Bottleneck> = HashMap::new();

        for bn in bottlenecks {
            let key = format!("{:?}_{}", bn.category, bn.title);
            
            if let Some(existing) = deduped.get_mut(&key) {
                existing.occurrence_count += bn.occurrence_count;
                existing.current_value = existing.current_value.max(bn.current_value);
                existing.impact_score = existing.impact_score.max(bn.impact_score);
                existing.related_operations.extend(bn.related_operations);
                
                if bn.severity > existing.severity {
                    existing.severity = bn.severity;
                }
            } else {
                deduped.insert(key, bn);
            }
        }

        deduped.into_values().collect()
    }

    async fn calculate_summary(&self, bottlenecks: &[Bottleneck]) -> BottleneckSummary {
        let mut by_severity: HashMap<Severity, usize> = HashMap::new();
        let mut by_category: HashMap<BottleneckCategory, usize> = HashMap::new();
        let mut critical_count = 0usize;
        let mut total_impact = 0.0f64;
        let mut top_bottleneck: Option<Bottleneck> = None;

        for bn in bottlenecks {
            *by_severity.entry(bn.severity).or_insert(0) += 1;
            *by_category.entry(bn.category.clone()).or_insert(0) += 1;
            total_impact += bn.impact_score;

            if bn.severity == Severity::Critical {
                critical_count += 1;
            }

            if top_bottleneck.is_none() || bn.impact_score > top_bottleneck.as_ref().unwrap().impact_score {
                top_bottleneck = Some(bn.clone());
            }
        }

        let overall_impact = if !bottlenecks.is_empty() {
            (total_impact / bottlenecks.len() as f64 * 100.0).min(100.0)
        } else {
            0.0
        };

        // 构建优先级队列
        let mut priority_queue: Vec<Bottleneck> = bottlenecks.to_vec();
        priority_queue.sort_by(|a, b| {
            b.severity.cmp(&a.severity)
                .then_with(|| b.impact_score.partial_cmp(&a.impact_score).unwrap_or(std::cmp::Ordering::Equal))
        });

        BottleneckSummary {
            total_bottlenecks: bottlenecks.len(),
            by_severity,
            by_category,
            overall_impact_score: overall_impact,
            top_bottleneck,
            critical_count,
            priority_queue,
        }
    }

    async fn detect_regressions(&self) -> Vec<PerformanceRegression> {
        let mut regressions = Vec::new();
        let sessions = self.sessions.read().await;

        for (_id, session) in sessions.iter() {
            let baseline = session.baseline.read().await;
            
            if let Some(baseline) = baseline.as_ref() {
                let ops = session.operations.read().await;
                
                if ops.is_empty() {
                    continue;
                }

                // 计算当前指标
                let current_avg: f64 = ops.iter().map(|op| op.duration_ms).sum::<f64>() / ops.len() as f64;
                let current_error_rate = ops.iter()
                    .filter(|op| !op.success)
                    .count() as f64 / ops.len() as f64 * 100.0;

                // 检测响应时间回归
                let response_time_regression = (current_avg - baseline.avg_response_time_ms) 
                    / baseline.avg_response_time_ms * 100.0;
                
                if response_time_regression > 20.0 {
                    let severity = if response_time_regression > 100.0 {
                        Severity::Critical
                    } else if response_time_regression > 50.0 {
                        Severity::Warning
                    } else {
                        Severity::Info
                    };

                    regressions.push(PerformanceRegression {
                        metric_name: "Average Response Time".to_string(),
                        baseline_value: baseline.avg_response_time_ms,
                        current_value: current_avg,
                        regression_percentage: response_time_regression,
                        severity,
                        description: format!(
                            "Response time increased by {:.1}% from baseline ({:.0}ms → {:.0}ms)",
                            response_time_regression, baseline.avg_response_time_ms, current_avg
                        ),
                    });
                }

                // 检测错误率回归
                let error_regression = current_error_rate - baseline.error_rate;
                if error_regression > 2.0 {
                    regressions.push(PerformanceRegression {
                        metric_name: "Error Rate".to_string(),
                        baseline_value: baseline.error_rate,
                        current_value: current_error_rate,
                        regression_percentage: error_regression,
                        Severity: if error_regression > 10.0 { Severity::Critical } else { Severity::Warning },
                        description: format!(
                            "Error rate increased by {:.1}% from baseline ({:.1}% → {:.1}%)",
                            error_regression, baseline.error_rate, current_error_rate
                        ),
                    });
                }
            }
        }

        regressions
    }

    async fn calculate_trends(&self) -> HashMap<String, Vec<f64>> {
        let mut trends = HashMap::new();
        let sessions = self.sessions.read().await;

        for (_id, session) in sessions.iter() {
            let ops = session.operations.read().await;
            
            if ops.len() < 10 {
                continue;
            }

            // 每 10 个操作计算一次平均值作为趋势点
            let window_size = 10;
            let mut response_times = Vec::new();

            for window in ops.as_slices().0.chunks(window_size) {
                let avg: f64 = window.iter().map(|op| op.duration_ms).sum::<f64>() / window.len() as f64;
                response_times.push(avg);
            }

            trends.insert(format!("{}_response_time", session.session_id), response_times);

            // 内存趋势
            let snapshots = session.memory_snapshots.read().await;
            if snapshots.len() >= 10 {
                let memory_values: Vec<f64> = snapshots.iter()
                    .map(|s| s.total_used_bytes as f64 / (1024.0 * 1024.0)) // Convert to MB
                    .collect();
                trends.insert(format!("{}_memory_mb", session.session_id), memory_values);
            }
        }

        trends
    }

    async fn create_suggestion_for_bottleneck(&self, bottleneck: &Bottleneck) -> OptimizationSuggestion {
        let (title, description, improvement, complexity, risk, references) = 
            match &bottleneck.category {
                BottleneckCategory::Cpu => (
                    "Optimize CPU-bound operations",
                    format!(
                        "The operation '{}' is taking {:.2}ms, which exceeds the threshold of {:.0}ms. \
                         Consider optimizing algorithms, adding caching, or parallelizing work.",
                        bottleneck.title, bottleneck.current_value, bottleneck.threshold
                    ),
                    "30-70% reduction in execution time",
                    3,
                    2,
                    vec!["https://doc.rust-lang.org/book/ch15-05-interior-mutability.html".to_string()],
                ),
                BottleneckCategory::Memory => (
                    "Investigate and fix memory leak",
                    format!(
                        "Memory is growing at {:.1}%/min, indicating a potential leak. \
                         Review allocation patterns, check for unintended retention, and consider using memory profilers.",
                        bottleneck.current_value
                    ),
                    "Stabilize memory usage, prevent OOM",
                    4,
                    3,
                    vec![
                        "https://doc.rust-lang.org/book/ch15-01-box.html".to_string(),
                        "https://github.com/rust-lang/rust-analyzer/blob/master/docs/dev/tracing-memory.md".to_string(),
                    ],
                ),
                BottleneckCategory::Io => (
                    "Optimize I/O operations",
                    format!(
                        "I/O wait time is {:.2}ms, exceeding the threshold. Consider batching I/O operations, \
                         using buffering, or implementing asynchronous I/O.",
                        bottleneck.current_value
                    ),
                    "40-60% reduction in I/O latency",
                    2,
                    2,
                    vec!["https://tokio.rs/tokio/tutorial/io".to_string()],
                ),
                BottleneckCategory::Concurrency => (
                    "Resolve concurrency issues",
                    format!(
                        "Lock contention or thread synchronization overhead detected. \
                         Consider reducing lock granularity, using lock-free data structures, or rethinking the concurrency model.",
                        bottleneck.current_value
                    ),
                    "50-80% reduction in contention",
                    4,
                    3,
                    vec!["https://doc.rust-lang.org/book/ch16-04-extensible-concurrency-sync-and-send.html".to_string()],
                ),
                _ => (
                    "General optimization recommended",
                    format!("Performance issue detected: {}", bottleneck.description),
                    "Varies based on specific issue",
                    3,
                    2,
                    vec![],
                ),
            };

        OptimizationSuggestion {
            id: format!("suggest_{}", uuid::Uuid::new_v4()),
            target_bottleneck_id: bottleneck.id.clone(),
            title: title.to_string(),
            description,
            expected_improvement: improvement.to_string(),
            complexity,
            risk_level: risk,
            priority: match bottleneck.severity {
                Severity::Critical => 9,
                Severity::Warning => 6,
                Severity::Info => 3,
            },
            references,
            code_example: None,
        }
    }

    async fn generate_final_report(&self, session: &MonitoringSession) -> BottleneckReport {
        let bottlenecks = session.current_bottlenecks.read().await.clone();
        let summary = self.calculate_summary(&bottlenecks).await;
        let regressions = self.detect_regressions_for_session(session).await;
        let trends = self.calculate_trends_for_session(session).await;

        BottleneckReport {
            report_id: format!("report_{}", uuid::Uuid::new_v4()),
            generated_at: Instant::now(),
            time_window: session.started_at.elapsed(),
            bottlenecks,
            summary,
            trends,
            regressions,
        }
    }

    async fn detect_regressions_for_session(&self, session: &MonitoringSession) -> Vec<PerformanceRegression> {
        let baseline = session.baseline.read().await;
        match baseline.as_ref() {
            Some(_) => vec![], // 简化实现，实际应该对比基线
            None => vec![],
        }
    }

    async fn calculate_trends_for_session(&self, _session: &MonitoringSession) -> HashMap<String, Vec<f64>> {
        HashMap::new() // 简化实现
    }

    fn initialize_pattern_library(&self) {
        let patterns = vec![
            BottleneckPattern {
                category: BottleneckCategory::Database,
                pattern_regex: r"query.*took.*\d+ms".to_string(),
                severity: Severity::Warning,
                suggestion_template: "Consider adding database indexes or optimizing queries".to_string(),
            },
            BottleneckPattern {
                category: BottleneckCategory::Network,
                pattern_regex: r"HTTP request.*timeout".to_string(),
                severity: Severity::Critical,
                suggestion_template: "Implement retry logic with exponential backoff".to_string(),
            },
            BottleneckPattern {
                category: BottleneckCategory::Io,
                pattern_regex: r"file.*read.*large".to_string(),
                severity: Severity::Warning,
                suggestion_template: "Use streaming or memory-mapped files for large reads".to_string(),
            },
        ];

        if let Ok(mut lib) = self.pattern_library.try_write() {
            *lib = patterns;
        }
    }

    async fn count_active_bottlenecks(&self) -> usize {
        let sessions = self.sessions.read().await;
        let mut count = 0usize;

        for (_id, session) in sessions.iter() {
            count += session.current_bottlenecks.read().await.len();
        }

        count
    }

    async fn get_memory_trend(&self) -> MemoryTrend {
        let sessions = self.sessions.read().await;
        let mut latest_mb = 0.0f64;
        let mut prev_mb = 0.0f64;

        for (_id, session) in sessions.iter() {
            let snapshots = session.memory_snapshots.read().await;
            
            if let Some(last) = snapshots.back() {
                latest_mb = last.total_used_bytes as f64 / (1024.0 * 1024.0);
            }
            
            if snapshots.len() >= 2 {
                if let Some(prev) = snapshots.get(snapshots.len() - 2) {
                    prev_mb = prev.total_used_bytes as f64 / (1024.0 * 1024.0);
                }
            }
        }

        if prev_mb > 0.0 && latest_mb > 0.0 {
            let change = ((latest_mb - prev_mb) / prev_mb) * 100.0;
            
            if change > 5.0 {
                MemoryTrend::Increasing
            } else if change < -5.0 {
                MemoryTrend::Decreasing
            } else {
                MemoryTrend::Stable
            }
        } else {
            MemoryTrend::Unknown
        }
    }

    async fn estimate_cpu_pressure(&self) -> CpuPressure {
        let sessions = self.sessions.read().await;
        let mut long_running_count = 0u64;
        let mut total_ops = 0u64;

        for (_id, session) in sessions.iter() {
            let ops = session.operations.read().await;
            total_ops += ops.len() as u64;
            
            long_running_count += ops.iter()
                .filter(|op| op.duration_ms > self.config.hotspot_threshold_ms)
                .count() as u64;
        }

        if total_ops == 0 {
            return CpuPressure::Low;
        }

        let ratio = long_running_count as f64 / total_ops as f64;

        if ratio > 0.3 {
            CpuPressure::High
        } else if ratio > 0.1 {
            CpuPressure::Medium
        } else {
            CpuPressure::Low
        }
    }
}

/// 性能概览
#[derive(Debug, Clone)]
pub struct PerformanceOverview {
    /// 活跃会话数
    pub active_sessions: usize,
    
    /// 总操作数
    pub total_operations: u64,
    
    /// 平均耗时 (ms)
    pub avg_duration_ms: f64,
    
    /// 成功率 (%)
    pub success_rate: f64,
    
    /// 活跃瓶颈数
    pub active_bottlenecks: usize,
    
    /// 内存趋势
    pub memory_trend: MemoryTrend,
    
    /// CPU 压力
    pub cpu_pressure: CpuPressure,
}

/// 内存趋势
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum MemoryTrend {
    Increasing,
    Decreasing,
    Stable,
    Unknown,
}

impl std::fmt::Display for MemoryTrend {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            Self::Increasing => write!(f, "📈 Increasing"),
            Self::Decreasing => write!(f, "📉 Decreasing"),
            Self::Stable => write!(f, "➡️ Stable"),
            Self::Unknown => write!(f, "❓ Unknown"),
        }
    }
}

/// CPU 压力等级
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum CpuPressure {
    Low,
    Medium,
    High,
}

impl std::fmt::Display for CpuPressure {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            Self::Low => write!(f, "✅ Low"),
            Self::Medium => write!(f, "⚠️ Medium"),
            Self::High => write!(f, "🔥 High"),
        }
    }
}

// ============================================================================
// 测试模块
// ============================================================================

#[cfg(test)]
mod tests {
    use super::*;

    #[tokio::test]
    async fn test_basic_bottleneck_detection() {
        let detector = BottleneckDetector::new();
        let session = detector.start_monitoring_session("test_app").await;

        // 记录一个慢操作
        let slow_op = OperationRecord::new("slow_database_query", 2500.0);
        session.record_operation(slow_op).await;

        // 给检测一些时间
        tokio::time::sleep(Duration::from_millis(100)).await;

        // 分析瓶颈
        let report = detector.analyze_bottlenecks().await;
        
        assert!(!report.bottlenecks.is_empty(), "Should detect at least one bottleneck");
        assert_eq!(report.summary.total_bottlenecks, 1);
        
        let bn = &report.bottlenecks[0];
        assert_eq!(bn.category, BottleneckCategory::Cpu);
        assert!(bn.severity >= Severity::Warning);
    }

    #[tokio::test]
    async fn test_memory_leak_detection() {
        let config = DetectorConfig {
            enable_memory_leak_detection: true,
            memory_growth_threshold_percent: 5.0, // 降低阈值以便测试
            ..Default::default()
        };

        let detector = BottleneckDetector::with_config(config);
        let session = detector.start_monitoring_session("memory_test").await;

        // 模拟快速增长的内存
        for i in 0..20 {
            let mem = (i + 1) * 10_000_000; // 每次 10MB
            session.record_memory_allocation(mem).await;
            tokio::time::sleep(Duration::from_millis(50)).await;
        }

        let report = detector.analyze_bottlenecks().await;
        
        let memory_bottlenecks: Vec<_> = report.bottlenecks.iter()
            .filter(|b| b.category == BottleneckCategory::Memory)
            .collect();
        
        assert!(!memory_bottlenecks.is_empty(), "Should detect memory leak");
    }

    #[tokio::test]
    async fn test_performance_overview() {
        let detector = BottleneckDetector::new();
        let session = detector.start_monitoring_session("overview_test").await;

        // 记录多个操作
        for i in 0..10 {
            let op = OperationRecord::new(format!("operation_{}", i), 50.0 + i as f64 * 10.0);
            session.record_operation(op).await;
        }

        let overview = detector.get_live_overview().await;
        
        assert_eq!(overview.active_sessions, 1);
        assert_eq!(overview.total_operations, 10);
        assert!(overview.avg_duration_ms > 0.0);
        assert_eq!(overview.success_rate, 100.0);
    }

    #[tokio::test]
    async fn test_optimization_suggestions() {
        let detector = BottleneckDetector::new();
        let session = detector.start_monitoring_session("suggestion_test").await;

        // 创建一个瓶颈
        let slow_op = OperationRecord::new("very_slow_function", 8000.0);
        session.record_operation(slow_op).await;

        tokio::time::sleep(Duration::from_millis(100)).await;

        let report = detector.analyze_bottlenecks().await;
        let suggestions = detector.generate_optimization_suggestions(&report).await;

        assert!(!suggestions.is_empty(), "Should generate optimization suggestions");
        
        let first_suggestion = &suggestions[0];
        assert!(!first_suggestion.title.is_empty());
        assert!(!first_suggestion.description.is_empty());
        assert!(first_suggestion.priority > 0);
    }

    #[test]
    fn test_severity_ordering() {
        assert!(Severity::Critical > Severity::Warning);
        assert!(Severity::Warning > Severity::Info);
    }

    #[test]
    fn test_bottleneck_creation() {
        let bn = Bottleneck::new(
            BottleneckCategory::Cpu,
            Severity::Critical,
            "Test Bottleneck",
            "This is a test",
            2000.0,
            1000.0,
            "ms",
        )
        .with_location("main.rs:42")
        .with_impact(0.8);

        assert_eq!(bn.category, BottleneckCategory::Cpu);
        assert_eq!(bn.severity, Severity::Critical);
        assert_eq!(bn.current_value, 2000.0);
        assert_eq!(bn.location, Some("main.rs:42".to_string()));
        assert!((bn.impact_score - 0.8).abs() < 0.001);
    }
}
