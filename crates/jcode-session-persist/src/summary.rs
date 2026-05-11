//! Summary — 会话摘要生成
//!
//! ## 核心能力
//! - 会话统计信息
//! - 关键指标计算
//! - 摘要报告生成

use crate::types::SessionId;
use serde::{Deserialize, Serialize};
use tracing::info;

/// 会话摘要
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct SessionSummary {
    pub session_id: SessionId,
    pub total_messages: usize,
    pub user_messages: usize,
    pub assistant_messages: usize,
    pub total_tokens: u64,
    pub duration_secs: f64,
    pub created_at: String,
    pub completed_at: Option<String>,
}

/// 摘要管理器
pub struct SummaryManager;

impl SummaryManager {
    /// 创建新的摘要管理器
    pub fn new() -> Self {
        Self
    }

    /// 生成会话摘要
    pub fn generate_summary(
        &self,
        session_id: &SessionId,
        messages: &[crate::types::Message],
        start_time: std::time::Instant,
    ) -> SessionSummary {
        let mut user_count = 0;
        let mut assistant_count = 0;
        
        for msg in messages {
            match msg.role.as_str() {
                "user" => user_count += 1,
                "assistant" | "system" => assistant_count += 1,
                _ => {}
            }
        }

        let duration = start_time.elapsed().as_secs_f64();

        info!(
            session = %session_id,
            messages = messages.len(),
            duration = duration,
            "Generated summary"
        );

        SessionSummary {
            session_id: session_id.clone(),
            total_messages: messages.len(),
            user_messages: user_count,
            assistant_messages: assistant_count,
            total_tokens: 0, // TODO: 计算实际 token 数
            duration_secs: duration,
            created_at: chrono::Utc::now().to_rfc3339(),
            completed_at: None,
        }
    }

    /// 更新摘要（会话完成时调用）
    pub fn mark_completed(summary: &mut SessionSummary) {
        summary.completed_at = Some(chrono::Utc::now().to_rfc3339());
    }
}
