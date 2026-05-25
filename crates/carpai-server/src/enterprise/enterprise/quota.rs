//! 用量配额管理系统
//!
//! 提供：
//! - Token用量追踪和限制
//! - API请求速率限制
//! - 并发会话控制
//! - 分级配额策略

use chrono::{DateTime, Utc};
use serde::{Deserialize, Serialize};
use std::collections::HashMap;
use std::sync::Arc;
use tokio::sync::RwLock;
use tracing::{debug, warn};

/// 用量层级
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
pub enum UsageTier {
    /// 免费版
    Free,
    /// 专业版
    Pro,
    /// 企业版
    Enterprise,
}

impl UsageTier {
    pub fn default_limits(&self) -> QuotaLimits {
        match self {
            Self::Free => QuotaLimits {
                max_tokens_per_month: 100_000,
                max_requests_per_hour: 50,
                max_concurrent_sessions: 2,
                max_file_size_mb: 10,
                max_codebase_size_gb: 1,
                allowed_models: vec!["qwen-7b".to_string(), "llama-8b".to_string()],
                max_context_length: 8192,
                rate_limit_rpm: 10,
            },
            Self::Pro => QuotaLimits {
                max_tokens_per_month: 1_000_000,
                max_requests_per_hour: 200,
                max_concurrent_sessions: 5,
                max_file_size_mb: 50,
                max_codebase_size_gb: 10,
                allowed_models: vec![
                    "qwen-7b".to_string(),
                    "llama-8b".to_string(),
                    "qwen-32b".to_string(),
                ],
                max_context_length: 32768,
                rate_limit_rpm: 30,
            },
            Self::Enterprise => QuotaLimits {
                max_tokens_per_month: u64::MAX,
                max_requests_per_hour: u64::MAX,
                max_concurrent_sessions: 50,
                max_file_size_mb: 500,
                max_codebase_size_gb: 100,
                allowed_models: vec!["*".to_string()], // 所有模型
                max_context_length: 128000,
                rate_limit_rpm: 100,
            },
        }
    }
}

/// 配额限制
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct QuotaLimits {
    /// 每月最大Token数
    pub max_tokens_per_month: u64,
    /// 每小时最大请求数
    pub max_requests_per_hour: u64,
    /// 最大并发会话数
    pub max_concurrent_sessions: u32,
    /// 最大文件大小 (MB)
    pub max_file_size_mb: u64,
    /// 最大代码库大小 (GB)
    pub max_codebase_size_gb: u64,
    /// 允许使用的模型
    pub allowed_models: Vec<String>,
    /// 最大上下文长度
    pub max_context_length: u32,
    /// 每分钟请求速率限制
    pub rate_limit_rpm: u32,
}

impl QuotaLimits {
    /// 检查是否允许使用指定模型
    pub fn is_model_allowed(&self, model_name: &str) -> bool {
        self.allowed_models.iter().any(|m| m == "*" || m == model_name)
    }

    /// 检查文件大小是否超限
    pub fn is_file_size_allowed(&self, size_mb: u64) -> bool {
        size_mb <= self.max_file_size_mb
    }

    /// 检查代码库大小是否超限
    pub fn is_codebase_size_allowed(&self, size_gb: u64) -> bool {
        size_gb <= self.max_codebase_size_gb
    }

    /// 检查上下文长度是否超限
    pub fn is_context_length_allowed(&self, length: u32) -> bool {
        length <= self.max_context_length
    }
}

/// 重置周期
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
pub enum ResetPeriod {
    /// 每小时重置
    Hourly,
    /// 每日重置
    Daily,
    /// 每月重置
    Monthly,
}

/// 配额策略
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct QuotaPolicy {
    /// 用量层级
    pub tier: UsageTier,
    /// 限制
    pub limits: QuotaLimits,
    /// 重置周期
    pub reset_period: ResetPeriod,
    /// 是否启用软限制（超过后警告但不拒绝）
    pub soft_limit: bool,
    /// 超额百分比阈值（触发警告）
    pub warning_threshold_percent: u32,
}

impl QuotaPolicy {
    pub fn new(tier: UsageTier) -> Self {
        let limits = tier.default_limits();
        Self {
            tier,
            limits,
            reset_period: ResetPeriod::Monthly,
            soft_limit: false,
            warning_threshold_percent: 80,
        }
    }

    pub fn with_soft_limit(mut self, enabled: bool) -> Self {
        self.soft_limit = enabled;
        self
    }

    pub fn with_warning_threshold(mut self, percent: u32) -> Self {
        self.warning_threshold_percent = percent;
        self
    }
}

/// 用量记录
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct UsageRecord {
    /// 用户ID
    pub user_id: String,
    /// 组织ID
    pub org_id: String,
    /// Token使用量
    pub tokens_used: u64,
    /// 请求次数
    pub request_count: u64,
    /// 当前活跃会话数
    pub active_sessions: u32,
    /// 最后更新时间
    pub last_updated: DateTime<Utc>,
    /// 周期开始时间
    pub period_start: DateTime<Utc>,
}

impl UsageRecord {
    pub fn new(user_id: String, org_id: String) -> Self {
        let now = Utc::now();
        Self {
            user_id,
            org_id,
            tokens_used: 0,
            request_count: 0,
            active_sessions: 0,
            last_updated: now,
            period_start: now,
        }
    }

    /// 增加Token用量
    pub fn add_tokens(&mut self, count: u64) {
        self.tokens_used += count;
        self.last_updated = Utc::now();
    }

    /// 增加请求计数
    pub fn add_request(&mut self) {
        self.request_count += 1;
        self.last_updated = Utc::now();
    }

    /// 增加活跃会话
    pub fn increment_sessions(&mut self) {
        self.active_sessions += 1;
    }

    /// 减少活跃会话
    pub fn decrement_sessions(&mut self) {
        if self.active_sessions > 0 {
            self.active_sessions -= 1;
        }
    }

    /// 重置周期用量
    pub fn reset_period(&mut self) {
        self.tokens_used = 0;
        self.request_count = 0;
        self.period_start = Utc::now();
    }
}

/// 用量摘要
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct UsageSummary {
    pub user_id: String,
    pub org_id: String,
    pub tier: UsageTier,
    pub tokens_used: u64,
    pub tokens_limit: u64,
    pub tokens_remaining: u64,
    pub usage_percent: f64,
    pub requests_this_hour: u64,
    pub active_sessions: u32,
    pub session_limit: u32,
    pub is_over_quota: bool,
    pub warning: Option<String>,
}

/// 配额错误
#[derive(Debug, Clone, thiserror::Error, Serialize, Deserialize)]
pub enum QuotaError {
    #[error("Token配额已用尽 (已用 {used}/{limit})")]
    TokenQuotaExceeded { used: u64, limit: u64 },

    #[error("请求速率超限 (当前 {current} rpm, 限制 {limit} rpm)")]
    RateLimitExceeded { current: u32, limit: u32 },

    #[error("并发会话数超限 (当前 {current}, 限制 {limit})")]
    ConcurrentSessionExceeded { current: u32, limit: u32 },

    #[error("模型 '{model}' 不在允许列表中")]
    ModelNotAllowed { model: String },

    #[error("文件大小超限 (当前 {current}MB, 限制 {limit}MB)")]
    FileSizeExceeded { current: u64, limit: u64 },

    #[error("代码库大小超限 (当前 {current}GB, 限制 {limit}GB)")]
    CodebaseSizeExceeded { current: u64, limit: u64 },

    #[error("上下文长度超限 (当前 {current}, 限制 {limit})")]
    ContextLengthExceeded { current: u32, limit: u32 },
}

/// 用量追踪器
pub struct UsageTracker {
    /// 用户用量记录
    records: HashMap<String, UsageRecord>,
    /// 用户配额策略
    policies: HashMap<String, QuotaPolicy>,
    /// 请求速率追踪 (user_id -> Vec<timestamp>)
    request_timestamps: HashMap<String, Vec<DateTime<Utc>>>,
}

impl UsageTracker {
    pub fn new() -> Self {
        Self {
            records: HashMap::new(),
            policies: HashMap::new(),
            request_timestamps: HashMap::new(),
        }
    }

    /// 设置用户配额策略
    pub fn set_policy(&mut self, user_id: String, policy: QuotaPolicy) {
        self.policies.insert(user_id, policy);
    }

    /// 获取或创建用量记录
    fn get_or_create_record(&mut self, user_id: &str, org_id: &str) -> &mut UsageRecord {
        if !self.records.contains_key(user_id) {
            self.records.insert(
                user_id.to_string(),
                UsageRecord::new(user_id.to_string(), org_id.to_string()),
            );
        }
        self.records.get_mut(user_id).unwrap()
    }

    /// 检查配额
    pub fn check_quota(
        &mut self,
        user_id: &str,
        org_id: &str,
        request_type: &QuotaRequest,
    ) -> Result<(), QuotaError> {
        let policy = self.policies.get(user_id).cloned().unwrap_or_else(|| {
            QuotaPolicy::new(UsageTier::Free)
        });

        let tokens_used;
        let active_sessions;
        {
            let record = self.get_or_create_record(user_id, org_id);
            tokens_used = record.tokens_used;
            active_sessions = record.active_sessions;
        }

        match request_type {
            QuotaRequest::ChatCompletion { tokens, model } => {
                // 检查Token配额
                if tokens_used + tokens > policy.limits.max_tokens_per_month {
                    return Err(QuotaError::TokenQuotaExceeded {
                        used: tokens_used,
                        limit: policy.limits.max_tokens_per_month,
                    });
                }

                // 检查模型权限
                if !policy.limits.is_model_allowed(model) {
                    return Err(QuotaError::ModelNotAllowed {
                        model: model.clone(),
                    });
                }

                // 检查速率限制
                self.check_rate_limit(user_id, policy.limits.rate_limit_rpm)?;
            }

            QuotaRequest::StartSession => {
                // 检查并发会话
                if active_sessions >= policy.limits.max_concurrent_sessions {
                    return Err(QuotaError::ConcurrentSessionExceeded {
                        current: active_sessions,
                        limit: policy.limits.max_concurrent_sessions,
                    });
                }
            }

            QuotaRequest::UploadFile { size_mb } => {
                if !policy.limits.is_file_size_allowed(*size_mb) {
                    return Err(QuotaError::FileSizeExceeded {
                        current: *size_mb,
                        limit: policy.limits.max_file_size_mb,
                    });
                }
            }

            QuotaRequest::IndexCodebase { size_gb } => {
                if !policy.limits.is_codebase_size_allowed(*size_gb) {
                    return Err(QuotaError::CodebaseSizeExceeded {
                        current: *size_gb,
                        limit: policy.limits.max_codebase_size_gb,
                    });
                }
            }
        }

        Ok(())
    }

    /// 记录用量
    pub fn record_usage(
        &mut self,
        user_id: &str,
        org_id: &str,
        usage: UsageUpdate,
    ) {
        let record = self.get_or_create_record(user_id, org_id);

        match usage {
            UsageUpdate::Tokens(count) => {
                record.add_tokens(count);
            }
            UsageUpdate::Request => {
                record.add_request();
                self.record_request_timestamp(user_id);
            }
            UsageUpdate::SessionStart => {
                record.increment_sessions();
            }
            UsageUpdate::SessionEnd => {
                record.decrement_sessions();
            }
        }
    }

    /// 检查速率限制
    fn check_rate_limit(&mut self, user_id: &str, limit_rpm: u32) -> Result<(), QuotaError> {
        let now = Utc::now();
        let timestamps = self.request_timestamps.entry(user_id.to_string()).or_insert_with(Vec::new);

        // 清理1分钟前的时间戳
        timestamps.retain(|ts| now.signed_duration_since(*ts).num_seconds() < 60);

        let current_rpm = timestamps.len() as u32;

        if current_rpm >= limit_rpm {
            return Err(QuotaError::RateLimitExceeded {
                current: current_rpm,
                limit: limit_rpm,
            });
        }

        Ok(())
    }

    /// 记录请求时间戳
    fn record_request_timestamp(&mut self, user_id: &str) {
        self.request_timestamps
            .entry(user_id.to_string())
            .or_insert_with(Vec::new)
            .push(Utc::now());
    }

    /// 获取用量摘要
    pub fn get_usage_summary(&self, user_id: &str) -> Option<UsageSummary> {
        let record = self.records.get(user_id)?;
        let policy = self.policies.get(user_id).cloned().unwrap_or_else(|| {
            QuotaPolicy::new(UsageTier::Free)
        });

        let tokens_remaining = policy.limits.max_tokens_per_month.saturating_sub(record.tokens_used);
        let usage_percent = if policy.limits.max_tokens_per_month > 0 {
            (record.tokens_used as f64 / policy.limits.max_tokens_per_month as f64) * 100.0
        } else {
            0.0
        };

        let is_over_quota = record.tokens_used >= policy.limits.max_tokens_per_month;

        let warning = if usage_percent >= policy.warning_threshold_percent as f64 {
            Some(format!(
                "用量已达到{}%，剩余 {} tokens",
                usage_percent as u32, tokens_remaining
            ))
        } else {
            None
        };

        Some(UsageSummary {
            user_id: record.user_id.clone(),
            org_id: record.org_id.clone(),
            tier: policy.tier,
            tokens_used: record.tokens_used,
            tokens_limit: policy.limits.max_tokens_per_month,
            tokens_remaining,
            usage_percent,
            requests_this_hour: record.request_count,
            active_sessions: record.active_sessions,
            session_limit: policy.limits.max_concurrent_sessions,
            is_over_quota,
            warning,
        })
    }

    /// 重置用户用量
    pub fn reset_user_usage(&mut self, user_id: &str) {
        if let Some(record) = self.records.get_mut(user_id) {
            record.reset_period();
        }
    }

    /// 清理过期数据
    pub fn cleanup_expired(&mut self, max_age_hours: i64) {
        let now = Utc::now();
        self.records.retain(|_, record| {
            now.signed_duration_since(record.last_updated).num_hours() < max_age_hours
        });
        self.request_timestamps.retain(|_, timestamps| {
            timestamps.iter().any(|ts| {
                now.signed_duration_since(*ts).num_hours() < max_age_hours
            })
        });
    }
}

impl Default for UsageTracker {
    fn default() -> Self {
        Self::new()
    }
}

/// 配额请求类型
#[derive(Debug, Clone)]
pub enum QuotaRequest {
    ChatCompletion {
        tokens: u64,
        model: String,
    },
    StartSession,
    UploadFile {
        size_mb: u64,
    },
    IndexCodebase {
        size_gb: u64,
    },
}

/// 用量更新类型
#[derive(Debug, Clone)]
pub enum UsageUpdate {
    Tokens(u64),
    Request,
    SessionStart,
    SessionEnd,
}

/// 共享的用量追踪器
pub type SharedUsageTracker = Arc<RwLock<UsageTracker>>;

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_free_tier_limits() {
        let limits = UsageTier::Free.default_limits();
        assert_eq!(limits.max_tokens_per_month, 100_000);
        assert_eq!(limits.max_concurrent_sessions, 2);
        assert!(!limits.is_model_allowed("gpt-4"));
    }

    #[test]
    fn test_enterprise_tier_limits() {
        let limits = UsageTier::Enterprise.default_limits();
        assert_eq!(limits.max_tokens_per_month, u64::MAX);
        assert!(limits.is_model_allowed("any-model"));
    }

    #[test]
    fn test_quota_check_token_exceeded() {
        let mut tracker = UsageTracker::new();
        let policy = QuotaPolicy::new(UsageTier::Free);
        tracker.set_policy("user1".to_string(), policy);

        // 模拟接近配额
        let record = tracker.get_or_create_record("user1", "org1");
        record.add_tokens(99_999);

        // 尝试使用更多Token
        let result = tracker.check_quota(
            "user1",
            "org1",
            &QuotaRequest::ChatCompletion {
                tokens: 100,
                model: "qwen-7b".to_string(),
            },
        );

        assert!(result.is_err());
    }

    #[test]
    fn test_quota_check_success() {
        let mut tracker = UsageTracker::new();
        let policy = QuotaPolicy::new(UsageTier::Free);
        tracker.set_policy("user1".to_string(), policy);

        let result = tracker.check_quota(
            "user1",
            "org1",
            &QuotaRequest::ChatCompletion {
                tokens: 100,
                model: "qwen-7b".to_string(),
            },
        );

        assert!(result.is_ok());
    }

    #[test]
    fn test_model_not_allowed() {
        let mut tracker = UsageTracker::new();
        let policy = QuotaPolicy::new(UsageTier::Free);
        tracker.set_policy("user1".to_string(), policy);

        let result = tracker.check_quota(
            "user1",
            "org1",
            &QuotaRequest::ChatCompletion {
                tokens: 100,
                model: "gpt-4".to_string(),
            },
        );

        assert!(matches!(result, Err(QuotaError::ModelNotAllowed { .. })));
    }

    #[test]
    fn test_usage_summary() {
        let mut tracker = UsageTracker::new();
        let policy = QuotaPolicy::new(UsageTier::Free);
        tracker.set_policy("user1".to_string(), policy.clone());

        // 记录一些用量
        tracker.record_usage("user1", "org1", UsageUpdate::Tokens(50_000));

        let summary = tracker.get_usage_summary("user1").unwrap();
        assert_eq!(summary.tokens_used, 50_000);
        assert_eq!(summary.tokens_limit, 100_000);
        assert_eq!(summary.tokens_remaining, 50_000);
        assert_eq!(summary.usage_percent, 50.0);
    }
}
