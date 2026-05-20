//! 增强的审计日志系统
//!
//! 提供结构化的审计日志记录和查询能力
//!
//! 核心能力:
//! 1. 结构化审计事件 - 统一的事件格式
//! 2. 事件分类 - 按类别、级别、模块筛选
//! 3. 事件追溯 - 支持因果链追踪
//! 4. 实时告警 - 异常行为检测

use chrono::{DateTime, Utc};
use serde::{Deserialize, Serialize};
use std::collections::{HashMap, HashSet};
use std::sync::atomic::{AtomicU64, Ordering};
use std::sync::Arc;
use tokio::sync::RwLock;
use tracing::{debug, error, warn};

/// 审计事件级别
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, PartialOrd, Ord, Serialize, Deserialize)]
pub enum AuditLevel {
    /// 调试信息
    Debug,
    /// 一般信息
    Info,
    /// 警告
    Warning,
    /// 错误
    Error,
    /// 严重错误
    Critical,
}

impl std::fmt::Display for AuditLevel {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            Self::Debug => write!(f, "DEBUG"),
            Self::Info => write!(f, "INFO"),
            Self::Warning => write!(f, "WARNING"),
            Self::Error => write!(f, "ERROR"),
            Self::Critical => write!(f, "CRITICAL"),
        }
    }
}

/// 审计事件类别
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Serialize, Deserialize)]
pub enum AuditCategory {
    /// 认证事件
    Authentication,
    /// 授权事件
    Authorization,
    /// 文件操作
    FileOperation,
    /// 代码执行
    CodeExecution,
    /// 工具调用
    ToolInvocation,
    /// 会话管理
    SessionManagement,
    /// 配置变更
    ConfigurationChange,
    /// 系统事件
    System,
    /// 网络事件
    Network,
    /// 数据访问
    DataAccess,
}

impl std::fmt::Display for AuditCategory {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            Self::Authentication => write!(f, "AUTH"),
            Self::Authorization => write!(f, "AUTHZ"),
            Self::FileOperation => write!(f, "FILE"),
            Self::CodeExecution => write!(f, "EXEC"),
            Self::ToolInvocation => write!(f, "TOOL"),
            Self::SessionManagement => write!(f, "SESSION"),
            Self::ConfigurationChange => write!(f, "CONFIG"),
            Self::System => write!(f, "SYSTEM"),
            Self::Network => write!(f, "NETWORK"),
            Self::DataAccess => write!(f, "DATA"),
        }
    }
}

/// 审计操作类型
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Serialize, Deserialize)]
pub enum AuditOperation {
    Create,
    Read,
    Update,
    Delete,
    Execute,
    Login,
    Logout,
    Grant,
    Revoke,
    Connect,
    Disconnect,
    Approve,
    Reject,
}

impl std::fmt::Display for AuditOperation {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            Self::Create => write!(f, "CREATE"),
            Self::Read => write!(f, "READ"),
            Self::Update => write!(f, "UPDATE"),
            Self::Delete => write!(f, "DELETE"),
            Self::Execute => write!(f, "EXECUTE"),
            Self::Login => write!(f, "LOGIN"),
            Self::Logout => write!(f, "LOGOUT"),
            Self::Grant => write!(f, "GRANT"),
            Self::Revoke => write!(f, "REVOKE"),
            Self::Connect => write!(f, "CONNECT"),
            Self::Disconnect => write!(f, "DISCONNECT"),
            Self::Approve => write!(f, "APPROVE"),
            Self::Reject => write!(f, "REJECT"),
        }
    }
}

/// 审计事件
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct AuditEvent {
    /// 事件唯一标识
    pub event_id: String,
    /// 时间戳
    pub timestamp: DateTime<Utc>,
    /// 事件级别
    pub level: AuditLevel,
    /// 事件类别
    pub category: AuditCategory,
    /// 操作类型
    pub operation: AuditOperation,
    /// 主体 (谁)
    pub subject: Subject,
    /// 客体 (对什么)
    pub object: Option<Object>,
    /// 结果
    pub outcome: Outcome,
    /// 上下文
    pub context: EventContext,
    /// 因果链 ID (关联事件)
    pub correlation_id: Option<String>,
    /// 父事件 ID
    pub parent_event_id: Option<String>,
    /// 额外数据
    pub metadata: HashMap<String, String>,
}

/// 事件主体 (执行操作的用户/系统)
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct Subject {
    /// 主体类型
    pub subject_type: SubjectType,
    /// 主体 ID
    pub id: String,
    /// 主体名称
    pub name: Option<String>,
    /// 角色
    pub roles: Vec<String>,
    /// IP 地址
    pub ip_address: Option<String>,
}

/// 主体类型
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
pub enum SubjectType {
    User,
    Agent,
    System,
    Service,
}

/// 事件客体 (操作的对象)
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct Object {
    /// 客体类型
    pub object_type: String,
    /// 客体 ID
    pub id: String,
    /// 客体名称
    pub name: String,
    /// 路径 (如果是文件)
    pub path: Option<String>,
}

/// 操作结果
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct Outcome {
    /// 结果状态
    pub status: OutcomeStatus,
    /// 错误码
    pub error_code: Option<String>,
    /// 错误消息
    pub error_message: Option<String>,
    /// 持续时间
    pub duration_ms: Option<u64>,
}

/// 结果状态
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
pub enum OutcomeStatus {
    Success,
    Failure,
    PartialSuccess,
    Pending,
    Unknown,
}

/// 事件上下文
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct EventContext {
    /// 会话 ID
    pub session_id: Option<String>,
    /// 工作目录
    pub working_directory: Option<String>,
    /// 语言/环境
    pub environment: Option<String>,
    /// 组件/模块
    pub component: String,
    /// 函数名
    pub function: Option<String>,
}

/// 审计日志过滤器
#[derive(Debug, Clone, Default)]
pub struct AuditFilter {
    /// 按级别过滤
    pub levels: Option<HashSet<AuditLevel>>,
    /// 按类别过滤
    pub categories: Option<HashSet<AuditCategory>>,
    /// 按操作过滤
    pub operations: Option<HashSet<AuditOperation>>,
    /// 按主体 ID 过滤
    pub subject_ids: Option<HashSet<String>>,
    /// 按客体 ID 过滤
    pub object_ids: Option<HashSet<String>>,
    /// 时间范围
    pub time_range: Option<TimeRange>,
    /// 搜索文本
    pub search_text: Option<String>,
}

/// 时间范围
#[derive(Debug, Clone)]
pub struct TimeRange {
    pub start: DateTime<Utc>,
    pub end: DateTime<Utc>,
}

/// 审计统计
#[derive(Debug, Clone, Default)]
pub struct AuditStats {
    pub total_events: u64,
    pub events_by_level: HashMap<AuditLevel, u64>,
    pub events_by_category: HashMap<AuditCategory, u64>,
    pub events_by_operation: HashMap<AuditOperation, u64>,
    pub failure_count: u64,
    pub avg_duration_ms: f64,
}

/// 审计日志存储
pub struct AuditLogStore {
    events: Vec<AuditEvent>,
    max_events: usize,
    stats: AuditStats,
}

impl AuditLogStore {
    pub fn new(max_events: usize) -> Self {
        Self {
            events: Vec::with_capacity(max_events),
            max_events,
            stats: AuditStats::default(),
        }
    }

    /// 添加事件
    pub fn add_event(&mut self, event: AuditEvent) {
        // 更新统计
        self.update_stats(&event);

        // 添加到事件列表
        self.events.push(event);

        // 如果超过最大容量，移除最旧的事件
        if self.events.len() > self.max_events {
            self.events.remove(0);
        }
    }

    /// 更新统计
    fn update_stats(&mut self, event: &AuditEvent) {
        self.stats.total_events += 1;

        *self.stats.events_by_level.entry(event.level).or_insert(0) += 1;
        *self.stats.events_by_category.entry(event.category).or_insert(0) += 1;
        *self.stats.events_by_operation.entry(event.operation).or_insert(0) += 1;

        if event.outcome.status == OutcomeStatus::Failure {
            self.stats.failure_count += 1;
        }

        if let Some(duration) = event.outcome.duration_ms {
            let n = self.stats.total_events as f64;
            self.stats.avg_duration_ms = (self.stats.avg_duration_ms * (n - 1.0) + duration as f64) / n;
        }
    }

    /// 查询事件
    pub fn query(&self, filter: &AuditFilter) -> Vec<&AuditEvent> {
        self.events.iter().filter(|e| self.matches_filter(e, filter)).collect()
    }

    /// 检查事件是否匹配过滤器
    fn matches_filter(&self, event: &AuditEvent, filter: &AuditFilter) -> bool {
        // 级别过滤
        if let Some(levels) = &filter.levels {
            if !levels.contains(&event.level) {
                return false;
            }
        }

        // 类别过滤
        if let Some(categories) = &filter.categories {
            if !categories.contains(&event.category) {
                return false;
            }
        }

        // 操作过滤
        if let Some(operations) = &filter.operations {
            if !operations.contains(&event.operation) {
                return false;
            }
        }

        // 主体 ID 过滤
        if let Some(subject_ids) = &filter.subject_ids {
            if !subject_ids.contains(&event.subject.id) {
                return false;
            }
        }

        // 时间范围过滤
        if let Some(time_range) = &filter.time_range {
            if event.timestamp < time_range.start || event.timestamp > time_range.end {
                return false;
            }
        }

        // 搜索文本过滤
        if let Some(text) = &filter.search_text {
            let text_lower = text.to_lowercase();
            let search_in = format!(
                "{} {} {} {}",
                event.subject.id,
                event.context.component,
                event.object.as_ref().map(|o| &o.name).unwrap_or(&String::new()),
                event.metadata.values().map(|s| s.as_str()).collect::<Vec<_>>().join(" ")
            ).to_lowercase();

            if !search_in.contains(&text_lower) {
                return false;
            }
        }

        true
    }

    /// 获取统计
    pub fn stats(&self) -> &AuditStats {
        &self.stats
    }

    /// 清空
    pub fn clear(&mut self) {
        self.events.clear();
        self.stats = AuditStats::default();
    }
}

/// 审计日志管理器
pub struct AuditLogManager {
    store: Arc<RwLock<AuditLogStore>>,
    /// 实时告警回调
    alert_callbacks: Vec<Arc<dyn Fn(&AuditEvent) + Send + Sync>>,
    /// 事件计数器 (使用原子操作实现线程安全)
    event_counter: AtomicU64,
}

impl AuditLogManager {
    pub fn new(max_events: usize) -> Self {
        Self {
            store: Arc::new(RwLock::new(AuditLogStore::new(max_events))),
            alert_callbacks: Vec::new(),
            event_counter: AtomicU64::new(0),
        }
    }

    /// 记录审计事件
    pub async fn log(&self, event: AuditEvent) {
        // 生成事件 ID
        let mut event_with_id = event;
        let counter = self.event_counter.fetch_add(1, Ordering::Relaxed);
        event_with_id.event_id = format!("AUD-{}-{:08x}", Utc::now().format("%Y%m%d%H%M%S"), counter);

        // 检查是否需要告警
        if self.should_alert(&event_with_id) {
            self.trigger_alerts(&event_with_id).await;
        }

        // 存储事件
        self.store.write().await.add_event(event_with_id.clone());

        // 同时记录到标准日志
        match event_with_id.level {
            AuditLevel::Error | AuditLevel::Critical => {
                error!(
                    event_id = %event_with_id.event_id,
                    category = %event_with_id.category,
                    operation = %event_with_id.operation,
                    subject = %event_with_id.subject.id,
                    "Audit event: {}",
                    event_with_id.outcome.status == OutcomeStatus::Success
                );
            }
            AuditLevel::Warning => {
                warn!(
                    event_id = %event_with_id.event_id,
                    category = %event_with_id.category,
                    operation = %event_with_id.operation,
                    subject = %event_with_id.subject.id,
                    "Audit event: {}",
                    event_with_id.outcome.status == OutcomeStatus::Success
                );
            }
            _ => {
                debug!(
                    event_id = %event_with_id.event_id,
                    category = %event_with_id.category,
                    operation = %event_with_id.operation,
                    subject = %event_with_id.subject.id,
                    "Audit event"
                );
            }
        }
    }

    /// 查询事件
    pub async fn query(&self, filter: AuditFilter) -> Vec<AuditEvent> {
        self.store
            .read()
            .await
            .query(&filter)
            .into_iter()
            .cloned()
            .collect()
    }

    /// 获取统计
    pub async fn stats(&self) -> AuditStats {
        self.store.read().await.stats().clone()
    }

    /// 检查是否需要告警
    fn should_alert(&self, event: &AuditEvent) -> bool {
        matches!(
            event.level,
            AuditLevel::Error | AuditLevel::Critical
        ) || matches!(
            event.category,
            AuditCategory::Authentication | AuditCategory::Authorization
        ) && event.outcome.status == OutcomeStatus::Failure
    }

    /// 触发告警
    async fn trigger_alerts(&self, event: &AuditEvent) {
        for callback in &self.alert_callbacks {
            callback(event);
        }
    }

    /// 注册告警回调
    pub fn register_alert_callback(&mut self, callback: Arc<dyn Fn(&AuditEvent) + Send + Sync>) {
        self.alert_callbacks.push(callback);
    }

    /// 清空日志
    pub async fn clear(&self) {
        self.store.write().await.clear();
    }
}

/// 便捷函数: 创建认证事件
pub fn create_auth_event(
    operation: AuditOperation,
    subject_id: &str,
    outcome_status: OutcomeStatus,
    error_message: Option<String>,
) -> AuditEvent {
    let now = Utc::now();
    AuditEvent {
        event_id: String::new(), // 由 manager 设置
        timestamp: now,
        level: match outcome_status {
            OutcomeStatus::Success => AuditLevel::Info,
            OutcomeStatus::Failure => AuditLevel::Warning,
            _ => AuditLevel::Info,
        },
        category: AuditCategory::Authentication,
        operation,
        subject: Subject {
            subject_type: SubjectType::User,
            id: subject_id.to_string(),
            name: None,
            roles: Vec::new(),
            ip_address: None,
        },
        object: None,
        outcome: Outcome {
            status: outcome_status,
            error_code: None,
            error_message,
            duration_ms: None,
        },
        context: EventContext {
            session_id: None,
            working_directory: None,
            environment: None,
            component: "auth".to_string(),
            function: None,
        },
        correlation_id: None,
        parent_event_id: None,
        metadata: HashMap::new(),
    }
}

/// 便捷函数: 创建文件操作事件
pub fn create_file_event(
    operation: AuditOperation,
    subject_id: &str,
    file_path: &str,
    outcome_status: OutcomeStatus,
    duration_ms: Option<u64>,
) -> AuditEvent {
    let now = Utc::now();
    AuditEvent {
        event_id: String::new(),
        timestamp: now,
        level: AuditLevel::Info,
        category: AuditCategory::FileOperation,
        operation,
        subject: Subject {
            subject_type: SubjectType::User,
            id: subject_id.to_string(),
            name: None,
            roles: Vec::new(),
            ip_address: None,
        },
        object: Some(Object {
            object_type: "file".to_string(),
            id: file_path.to_string(),
            name: std::path::Path::new(file_path)
                .file_name()
                .map(|s| s.to_string_lossy().to_string())
                .unwrap_or_default(),
            path: Some(file_path.to_string()),
        }),
        outcome: Outcome {
            status: outcome_status,
            error_code: None,
            error_message: None,
            duration_ms,
        },
        context: EventContext {
            session_id: None,
            working_directory: std::env::current_dir().ok().map(|p| p.to_string_lossy().to_string()),
            environment: None,
            component: "file_operation".to_string(),
            function: None,
        },
        correlation_id: None,
        parent_event_id: None,
        metadata: HashMap::new(),
    }
}

/// 便捷函数: 创建工具调用事件
pub fn create_tool_event(
    tool_name: &str,
    subject_id: &str,
    outcome_status: OutcomeStatus,
    duration_ms: Option<u64>,
) -> AuditEvent {
    let now = Utc::now();
    AuditEvent {
        event_id: String::new(),
        timestamp: now,
        level: match outcome_status {
            OutcomeStatus::Success => AuditLevel::Info,
            OutcomeStatus::Failure => AuditLevel::Warning,
            _ => AuditLevel::Info,
        },
        category: AuditCategory::ToolInvocation,
        operation: AuditOperation::Execute,
        subject: Subject {
            subject_type: SubjectType::Agent,
            id: subject_id.to_string(),
            name: Some(tool_name.to_string()),
            roles: Vec::new(),
            ip_address: None,
        },
        object: Some(Object {
            object_type: "tool".to_string(),
            id: tool_name.to_string(),
            name: tool_name.to_string(),
            path: None,
        }),
        outcome: Outcome {
            status: outcome_status,
            error_code: None,
            error_message: None,
            duration_ms,
        },
        context: EventContext {
            session_id: None,
            working_directory: None,
            environment: None,
            component: "tool".to_string(),
            function: Some(tool_name.to_string()),
        },
        correlation_id: None,
        parent_event_id: None,
        metadata: HashMap::new(),
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[tokio::test]
    async fn test_audit_log_manager() {
        let manager = AuditLogManager::new(100);

        // 创建并记录事件
        let event = create_auth_event(
            AuditOperation::Login,
            "user123",
            OutcomeStatus::Success,
            None,
        );

        manager.log(event).await;

        // 查询事件
        let filter = AuditFilter {
            categories: Some(vec![AuditCategory::Authentication].into_iter().collect()),
            ..Default::default()
        };

        let results = manager.query(filter).await;
        assert_eq!(results.len(), 1);
    }

    #[tokio::test]
    async fn test_audit_stats() {
        let manager = AuditLogManager::new(100);

        // 添加多个事件
        for i in 0..5 {
            let event = create_tool_event(
                &format!("tool_{}", i),
                "agent1",
                if i % 2 == 0 { OutcomeStatus::Success } else { OutcomeStatus::Failure },
                Some(100),
            );
            manager.log(event).await;
        }

        let stats = manager.stats().await;
        assert_eq!(stats.total_events, 5);
        assert_eq!(stats.failure_count, 2);
    }
}
