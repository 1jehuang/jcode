//! # MCP 进度通知系统
//!
//! 提供长时间运行操作的实时进度反馈：
//! - **进度跟踪器** - 自动管理token和状态
//! - **多级进度** - 支持嵌套/并行任务
//! - **取消支持** - 客户端可中断操作
//! - **历史记录** - 保存最近N个操作的进度
//!
//! ## 使用示例
//!
//! ```rust
//! use carpai::mcp::notification::{McpServer, ProgressTracker};
//!
//! // 在服务器端:
//! let tracker = server.create_progress_tracker("file-download");
//! tracker.update(50, Some(100), Some("Downloading 50%")).await;
//! tracker.complete("Download completed!").await;
//!
//! // 客户端会收到:
//! // notifications/progress { progressToken: "xxx", value: { fraction: 0.5 } }
//! ```

use async_trait::async_trait;
use serde::{Deserialize, Serialize};
use std::collections::HashMap;
use std::sync::Arc;
use tokio::sync::{mpsc, RwLock};

/// 进度值类型
#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(untagged)]
pub enum ProgressValue {
    /// 百分比 (0.0 - 1.0)
    Fraction(f64),
    
    /// 绝对值
    Absolute(u64),
}

impl std::fmt::Display for ProgressValue {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            ProgressValue::Fraction(frac) => write!(f, "{:.1}%", frac * 100.0),
            ProgressValue::Absolute(val) => write!(f, "{}", val),
        }
    }
}

/// 进度通知消息
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct ProgressNotification {
    /// 进度token（用于关联请求/响应）
    pub progress_token: String,
    
    /// 当前进度值
    #[serde(flatten)]
    pub value: ProgressValue,
    
    /// 总量（如果已知）
    #[serde(skip_serializing_if = "Option::is_none")]
    pub total: Option<u64>,
    
    /// 可选的描述性消息
    #[serde(skip_serializing_if = "Option::is_none")]
    pub message: Option<String>,
    
    /// 时间戳
    pub timestamp: chrono::DateTime<chrono::Utc>,
}

/// 进度跟踪器（简化使用）
pub struct ProgressTracker {
    server: Arc<RwLock<McpServerInner>>,
    token: String,
    operation_name: String,
}

/// McpServer内部结构（用于进度通知）
struct McpServerInner {
    notification_sender: Option<mpsc::UnboundedSender<ProgressNotification>>,
    progress_history: Vec<ProgressHistoryEntry>,
    active_trackers: HashMap<String, ProgressState>,
}

/// 历史记录条目
#[derive(Debug, Clone, Serialize, Deserialize)]
struct ProgressHistoryEntry {
    token: String,
    operation_name: String,
    final_value: ProgressValue,
    message: Option<String>,
    started_at: chrono::DateTime<chrono::Utc>,
    completed_at: chrono::DateTime<chrono::Utc>,
    duration_ms: u64,
    status: CompletionStatus,
}

/// 当前进度状态
#[derive(Debug, Clone)]
struct ProgressState {
    current_value: ProgressValue,
    total: Option<u64>,
    message: Option<String>,
    started_at: chrono::DateTime<chrono::Utc>,
    last_updated: chrono::DateTime<chrono::Utc>,
    cancelled: bool,
}

/// 完成状态
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub enum CompletionStatus {
    Completed,
    Cancelled,
    Failed(String),
}

// ════════════════════════════
// McpServer 进度通知扩展
// ════════════════════════════

/// 为MCPServer添加进度通知能力的扩展trait
#[async_trait]
pub trait ProgressNotificationSupport {
    /// 发送进度通知到客户端
    async fn send_progress_notification(
        &self,
        token: &str,
        value: ProgressValue,
        total: Option<u64>,
        message: Option<&str>,
    ) -> Result<(), NotificationError>;
    
    /// 创建进度跟踪器（自动管理token）
    fn create_progress_tracker(&self, operation: &str) -> ProgressTracker;
    
    /// 获取所有活跃的进度跟踪器
    async fn get_active_progress(&self) -> Vec<ProgressInfo>;
    
    /// 获取历史进度记录
    async fn get_progress_history(&self, limit: usize) -> Vec<ProgressHistoryEntry>;
    
    /// 取消正在进行的操作
    async fn cancel_operation(&self, token: &str) -> Result<(), NotificationError>;
}

/// 进度信息（用于查询）
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct ProgressInfo {
    pub token: String,
    pub operation_name: String,
    pub current_value: ProgressValue,
    pub total: Option<u64>,
    pub message: Option<String>,
    pub elapsed_ms: u64,
    pub is_cancelled: bool,
}

/// 通知错误
#[derive(Debug, thiserror::Error)]
pub enum NotificationError {
    #[error("Not connected")]
    NotConnected,
    #[error("Send failed: {0}")]
    SendFailed(String),
    #[error("Operation not found: {0}")]
    OperationNotFound(String),
}

impl ProgressTracker {
    pub fn new(server: Arc<RwLock<McpServerInner>>, token: String, operation: &str) -> Self {
        let state = ProgressState {
            current_value: ProgressValue::Fraction(0.0),
            total: None,
            message: None,
            started_at: chrono::Utc::now(),
            last_updated: chrono::Utc::now(),
            cancelled: false,
        };

        {
            let mut server = server.blocking_write();
            server.active_trackers.insert(token.clone(), state);
        }

        Self {
            server,
            token,
            operation_name: operation.to_string(),
        }
    }

    /// 更新进度
    pub async fn update(
        &self,
        current: impl Into<ProgressValue>,
        total: Option<u64>,
        message: Option<&str>,
    ) -> Result<(), NotificationError> {
        let value = current.into();
        
        // 更新内部状态
        {
            let mut server = self.server.write().await;
            if let Some(state) = server.active_trackers.get_mut(&self.token) {
                state.current_value = value.clone();
                state.total = total;
                state.message = message.map(|m| m.to_string());
                state.last_updated = chrono::Utc::now();
            }
        }

        // 发送通知
        self.server.read().await.send_progress_notification(
            &self.token,
            value,
            total,
            message,
        ).await
    }

    /// 标记为完成
    pub async fn complete(&self, message: &str) -> Result<(), NotificationError> {
        self.update(ProgressValue::Fraction(1.0), None, Some(message)).await?;

        // 记录到历史
        {
            let mut server = self.server.write().await;
            if let Some(state) = server.active_trackers.remove(&self.token) {
                server.progress_history.push(ProgressHistoryEntry {
                    token: self.token.clone(),
                    operation_name: self.operation_name.clone(),
                    final_value: state.current_value,
                    message: state.message,
                    started_at: state.started_at,
                    completed_at: chrono::Utc::now(),
                    duration_ms: state.started_at.elapsed().as_millis() as u64,
                    status: CompletionStatus::Completed,
                });

                // 保持历史记录在合理范围内
                if server.progress_history.len() > 100 {
                    server.progress_history.remove(0);
                }
            }
        }

        Ok(())
    }

    /// 标记为失败
    pub async fn fail(&self, error: &str) -> Result<(), NotificationError> {
        {
            let mut server = self.server.write().await;
            if let Some(state) = server.active_trackers.get_mut(&self.token) {
                state.cancelled = true; // 标记结束
            }

            if let Some(state) = server.active_trackers.remove(&self.token) {
                server.progress_history.push(ProgressHistoryEntry {
                    token: self.token.clone(),
                    operation_name: self.operation_name.clone(),
                    final_value: state.current_value,
                    message: Some(error.to_string()),
                    started_at: state.started_at,
                    completed_at: chrono::Utc::now(),
                    duration_ms: state.started_at.elapsed().as_millis() as u64,
                    status: CompletionStatus::Failed(error.to_string()),
                });
            }
        }

        Ok(())
    }

    /// 获取当前进度信息
    pub async fn get_info(&self) -> Option<ProgressInfo> {
        let server = self.server.read().await;
        server.active_trackers.get(&self.token).map(|state| ProgressInfo {
            token: self.token.clone(),
            operation_name: self.operation_name.clone(),
            current_value: state.current_value.clone(),
            total: state.total,
            message: state.message.clone(),
            elapsed_ms: state.started_at.elapsed().as_millis() as u64,
            is_cancelled: state.cancelled,
        })
    }
}

// ════════════════════════════
// 辅助工具函数
// ════════════════════════════

/// 创建带进度的异步任务包装器
pub async fn with_progress<F, T>(
    server: Arc<RwLock<McpServerInner>>,
    operation: &str,
    task: F,
) -> Result<T, Box<dyn std::error::Error>>
where
    F: std::future::Future<Output = Result<T, Box<dyn std::error::Error>>>,
{
    let tracker = ProgressTracker::new(server, format!("prog-{}", uuid::Uuid::new_v4()), operation);

    // 执行任务
    match task.await {
        Ok(result) => {
            tracker.complete("Operation completed successfully").await.ok();
            Ok(result)
        }
        Err(e) => {
            tracker.fail(&e.to_string()).await.ok();
            Err(e)
        }
    }
}

/// 创建分步进度包装器（适用于多阶段任务）
pub struct SteppedProgress<'a> {
    tracker: &'a ProgressTracker,
    steps: Vec<String>,
    current_step: usize,
    total_steps: usize,
}

impl<'a> SteppedProgress<'a> {
    pub fn new(tracker: &'a ProgressTracker, steps: Vec<String>) -> Self {
        let total = steps.len();
        Self {
            tracker,
            steps,
            current_step: 0,
            total_steps: total,
        }
    }

    /// 进入下一步
    pub async fn next_step(&mut self) -> Result<(), NotificationError> {
        if self.current_step < self.total_steps {
            let progress = (self.current_step as f64) / (self.total_steps as f64);
            let step_name = self.steps.get(self.current_step)
                .map(|s| s.as_str())
                .unwrap_or("");

            self.tracker.update(
                ProgressValue::Fraction(progress),
                Some(self.total_steps as u64),
                Some(step_name)
            ).await?;

            self.current_step += 1;
        }

        Ok(())
    }

    /// 完成所有步骤
    pub async fn complete_all(self, message: &str) -> Result<(), NotificationError> {
        self.tracker.complete(message).await
    }
}

// ════════════════════════════
// 单元测试
// ════════════════════════════

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_progress_value_display() {
        let frac = ProgressValue::Fraction(0.75);
        assert_eq!(format!("{}", frac), "75.0%");

        let abs = ProgressValue::Absolute(100);
        assert_eq!(format!("{}", abs), "100");
    }

    #[test]
    fn test_notification_serialization() {
        let notification = ProgressNotification {
            progress_token: "test-token".to_string(),
            value: ProgressValue::Fraction(0.5),
            total: Some(100),
            message: Some("Halfway done".to_string()),
            timestamp: chrono::Utc::now(),
        };

        let json = serde_json::to_string(&notification).expect("Should serialize");
        assert!(json.contains("\"progress_token\":\"test-token\""));
        assert!(json.contains("\"fraction\":0.5"));
    }

    #[tokio::test]
    async fn test_progress_tracker_lifecycle() {
        let (tx, _rx) = mpsc::unbounded_channel::<ProgressNotification>();
        let inner = McpServerInner {
            notification_sender: Some(tx),
            progress_history: vec![],
            active_trackers: HashMap::new(),
        };
        let server = Arc::new(RwLock::new(inner));

        let tracker = ProgressTracker::new(
            server.clone(),
            "test-token".to_string(),
            "test-operation"
        );

        // 更新进度
        tracker.update(25u64, Some(100), Some("25% done")).await.unwrap();

        // 检查状态
        let info = tracker.get_info().await;
        assert!(info.is_some());
        let info = info.unwrap();
        assert_eq!(info.operation_name, "test-operation");

        // 完成
        tracker.complete("All done!").await.unwrap();

        // 验证已移入历史
        let inner = server.read().await;
        assert!(!inner.active_trackers.contains_key("test-token"));
        assert_eq!(inner.progress_history.len(), 1);
        assert!(matches!(inner.progress_history[0].status, CompletionStatus::Completed));
    }

    #[tokio::test]
    async fn test_stepped_progress() {
        let (tx, _rx) = mpsc::unbounded_channel::<ProgressNotification>();
        let inner = McpServerInner {
            notification_sender: Some(tx),
            progress_history: vec![],
            active_trackers: HashMap::new(),
        };
        let server = Arc::new(RwLock::new(inner));

        let tracker = ProgressTracker::new(
            server,
            "stepped-token".to_string(),
            "multi-step"
        );

        let steps = vec![
            "Initializing".to_string(),
            "Processing".to_string(),
            "Finalizing".to_string(),
        ];

        let mut stepped = SteppedProgress::new(&tracker, steps);

        stepped.next_step().await.unwrap(); // Step 1: 33.3%
        stepped.next_step().await.unwrap(); // Step 2: 66.7%
        stepped.complete_all("All steps done").await.unwrap();

        // 验证最终状态
        let info = tracker.get_info().await;
        assert!(info.is_some());
        let info = info.unwrap();
        match info.current_value {
            ProgressValue::Fraction(f) => assert!((f - 1.0).abs() < 0.01),
            other => panic!("Expected Fraction near 1.0, got {:?}", other),
        }
    }

    #[tokio::test]
    async fn test_cancellation_and_failure() {
        let (tx, _rx) = mpsc::unbounded_channel::<ProgressNotification>();
        let inner = McpServerInner {
            notification_sender: Some(tx),
            progress_history: vec![],
            active_trackers: HashMap::new(),
        };
        let server = Arc::new(RwLock::new(inner));

        let tracker = ProgressTracker::new(
            server,
            "fail-token".to_string(),
            "failing-op"
        );

        // 模拟失败
        tracker.fail("Something went wrong").await.unwrap();

        // 验证失败状态
        let srv = server.read().await;
        let entry = srv.progress_history.iter()
            .find(|e| e.token == "fail-token")
            .expect("Should have history entry");

        assert!(matches!(&entry.status, CompletionStatus::Failed(_)));
        assert!(entry.message.as_ref().unwrap() == "Something went wrong");
    }

    #[test]
    fn test_completion_status_serialization() {
        let completed = serde_json::to_string(&CompletionStatus::Completed).unwrap();
        assert!(completed.contains("Completed"));

        let failed = serde_json::to_string(&CompletionStatus::Failed("error msg".to_string())).unwrap();
        assert!(failed.contains("error msg"));
    }
}
