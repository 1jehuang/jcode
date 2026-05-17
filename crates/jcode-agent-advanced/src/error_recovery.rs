// ════════════════════════════════════════════════════════════════
// 错误恢复策略系统
// 对应 Claude Code: query.ts 错误处理 + 重试逻辑
//
// 核心能力:
//   1. ErrorClassifier — 智能错误分类 (可重试/可降级/致命)
//   2. RetryPolicy — 可配置的重试策略 (指数退避 + 抖动)
//   3. BackoffStrategy — 退避算法 (固定/线性/指数/指数+抖动)
//   4. ToolFallbackRegistry — 工具降级注册表
//   5. RecoveryAction — 恢复动作枚举
// ════════════════════════════════════════════════════════════════

use std::collections::HashMap;
use std::time::Duration;

use serde::{Deserialize, Serialize};
use thiserror::Error;
use rand::Rng;

use super::types::TerminalState;

/// 错误分类 — 冶定恢复策略
#[derive(Debug, Clone, PartialEq, Eq, Hash, Serialize, Deserialize)]
pub enum ErrorCategory {
    /// 网络超时 — 可重试
    NetworkTimeout,
    /// 速率限制 — 需要退避后重试
    RateLimited { retry_after_secs: u64 },
    /// 服务端错误 (5xx) — 可能可重试
    ServerError(u16),
    /// 认证/授权失败 — 不应自动重试
    AuthenticationFailed,
    /// 无效请求参数 — 应修正后重试
    InvalidRequest,
    /// 模型过载 — 可切换模型或等待
    ModelOverloaded,
    /// 上下文过长 — 需要 compact
    ContextLengthExceeded,
    /// 工具执行失败 — 可能需要替代工具
    ToolExecutionFailed,
    /// 权限被拒绝 — 需要用户介入
    PermissionDenied,
    /// 成本超限 — 终止
    BudgetExceeded,
    /// 未分类的内部错误
    Unknown,
}

impl std::fmt::Display for ErrorCategory {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            Self::NetworkTimeout => write!(f, "网络超时"),
            Self::RateLimited { retry_after_secs } => {
                write!(f, "速率限制 ({retry_after_secs}s 后重试)")
            }
            Self::ServerError(code) => write!(f, "服务端错误 HTTP {code}"),
            Self::AuthenticationFailed => write!(f, "认证失败"),
            Self::InvalidRequest => write!(f, "无效请求"),
            Self::ModelOverloaded => write!(f, "模型过载"),
            Self::ContextLengthExceeded => write!(f, "上下文长度超限"),
            Self::ToolExecutionFailed => write!(f, "工具执行失败"),
            Self::PermissionDenied => write!(f, "权限被拒绝"),
            Self::BudgetExceeded => write!(f, "成本预算耗尽"),
            Self::Unknown => write!(f, "未知错误"),
        }
    }
}

/// 恢复动作
#[derive(Debug, Clone, Serialize, Deserialize)]
pub enum RecoveryAction {
    /// 立即重试
    RetryNow,
    /// 延迟后退避重试
    RetryWithBackoff(Duration),
    /// 切换到备用模型
    SwitchModel(String),
    /// 切换到备用工具
    SwitchTool { from: String, to: String },
    /// 执行上下文压缩后重试
    CompactAndRetry,
    /// 通知用户并等待输入
    AskUser(String),
    /// 无法恢复，终止
    Terminate(TerminalState),
}

/// 错误分类器 — 根据错误信息判断类别和推荐动作
pub struct ErrorClassifier {
    /// 自定义规则覆盖
    custom_rules: HashMap<String, ErrorCategory>,
    
    /// 工具特定的降级映射
    tool_fallbacks: HashMap<String, String>,
}

impl ErrorClassifier {
    pub fn new() -> Self {
        Self {
            custom_rules: HashMap::new(),
            tool_fallbacks: HashMap::new(),
        }
    }
    
    /// 注册自定义分类规则
    pub fn add_rule(&mut self, pattern: &str, category: ErrorCategory) {
        self.custom_rules.insert(pattern.to_lowercase(), category);
    }
    
    /// 注册工具降级映射
    pub fn add_tool_fallback(&mut self, primary: &str, fallback: &str) {
        self.tool_fallbacks.insert(primary.to_lowercase(), fallback.to_string());
    }
    
    /// 分类错误
    pub fn classify(&self, error: &dyn std::error::Error) -> (ErrorCategory, Vec<RecoveryAction>) {
        let error_msg = error.to_string().to_lowercase();
        
        // 检查自定义规则
        for (pattern, category) in &self.custom_rules {
            if error_msg.contains(pattern.as_str()) {
                return (category.clone(), self.suggest_actions(&category));
            }
        }
        
        // 默认分类规则 (对应 Claude Code 的错误处理逻辑)
        let category = if error_msg.contains("timeout") || error_msg.contains("timed out") {
            ErrorCategory::NetworkTimeout
        } else if error_msg.contains("rate limit") || error_msg.contains("429") {
            let retry_after = extract_retry_after(&error_msg);
            ErrorCategory::RateLimited { retry_after_secs: retry_after.unwrap_or(60) }
        } else if error_msg.contains("overloaded") || error_msg.contains("529") {
            ErrorCategory::ModelOverloaded
        } else if error_msg.contains("context length") || error_msg.contains("too long") 
            || error_msg.contains("maximum context") {
            ErrorCategory::ContextLengthExceeded
        } else if error_msg.contains("401") || error_msg.contains("403") 
            || error_msg.contains("auth") || error_msg.contains("unauthorized") {
            ErrorCategory::AuthenticationFailed
        } else if error_msg.contains("400") || error_msg.contains("invalid") {
            ErrorCategory::InvalidRequest
        } else if error_msg.contains("500") || error_msg.contains("502") 
            || error_msg.contains("503") {
            ErrorCategory::ServerError(extract_status_code(&error_msg).unwrap_or(500))
        } else if error_msg.contains("permission") || error_msg.contains("denied") {
            ErrorCategory::PermissionDenied
        } else if error_msg.contains("budget") || error_msg.contains("cost limit") {
            ErrorCategory::BudgetExceeded
        } else {
            ErrorCategory::Unknown
        };
        
        (category.clone(), self.suggest_actions(&category))
    }
    
    fn suggest_actions(&self, category: &ErrorCategory) -> Vec<RecoveryAction> {
        match category {
            ErrorCategory::NetworkTimeout | ErrorCategory::RateLimited { .. } 
                | ErrorCategory::ModelOverloaded | ErrorCategory::ServerError(_) => {
                vec![RecoveryAction::RetryWithBackoff(
                    Duration::from_millis(BACKOFF_INITIAL_MS)
                )]
            }
            ErrorCategory::ContextLengthExceeded => {
                vec![RecoveryAction::CompactAndRetry]
            }
            ErrorCategory::ToolExecutionFailed => {
                // 如果有工具降级可用，建议切换
                vec![RecoveryAction::RetryNow]  // 先尝试简单重试
            }
            ErrorCategory::PermissionDenied => {
                vec![RecoveryAction::AskUser(
                    "权限被拒绝，是否允许此操作？".to_string()
                )]
            }
            ErrorCategory::BudgetExceeded => {
                vec![RecoveryAction::Terminate(TerminalState::Error {
                    message: "成本预算已用完".to_string(),
                    recoverable: false,
                })]
            }
            _ => vec![RecoveryAction::Terminate(TerminalState::Error {
                message: format!("{category}"),
                recoverable: false,
            })],
        }
    }
}

impl Default for ErrorClassifier {
    fn default() -> Self {
        Self::new()
    }
}

// ════════════════════════════════════════════════════════════════

/// 退避策略
#[derive(Debug, Clone, Serialize, Deserialize)]
pub enum BackoffStrategy {
    /// 固定间隔
    Fixed { interval_ms: u64 },
    /// 线性增长: delay = base * attempt
    Linear { base_ms: u64 },
    /// 指数增长: delay = base * 2^attempt
    Exponential { base_ms: u64 },
    /// 指数 + 抖动 (推荐): delay = base * 2^attempt * random(0.8, 1.2)
    ExponentialWithJitter { base_ms: u64, jitter_factor: f64 },
}

impl BackoffStrategy {
    /// 创建推荐的指数退避策略 (Claude Code 默认)
    pub fn exponential_default() -> Self {
        Self::ExponentialWithJitter {
            base_ms: BACKOFF_INITIAL_MS,
            jitter_factor: BACKOFF_JITTER_FACTOR,
        }
    }
    
    /// 计算第 N 次重试的延迟时间 (毫秒，含抖动)
    pub fn calculate_delay(&self, attempt: u32) -> Option<u64> {
        match self {
            Self::Fixed { interval_ms } => Some(*interval_ms),
            
            Self::Linear { base_ms } => {
                Some(base_ms.saturating_mul(attempt))
            }
            
            Self::Exponential { base_ms } => {
                let delay = (*base_ms as f64) * 2i32.pow(attempt) as f64;
                Some(delay.min(BACKOFF_MAX_MS as f64) as u64)
            }
            
            Self::ExponentialWithJitter { base_ms, jitter_factor } => {
                let raw_delay = (*base_ms as f64) * 2i32.pow(attempt.min(10)) as f64;
                let capped_delay = raw_delay.min(BACKOFF_MAX_MS as f64);
                
                // 添加随机抖动: ±jitter_factor%
                let range = capped_delay * jitter_factor;
                let jitter = rand::rng().gen_range(-range..=range);
                
                Some((capped_delay + jitter).max(1.0) as u64)
            }
        }
    }
}

/// 重试策略配置
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct RetryPolicy {
    /// 最大重试次数
    pub max_attempts: u32,
    
    /// 退避策略
    pub backoff: BackoffStrategy,
    
    /// 是否仅对特定错误类别重试
    pub retryable_categories: Vec<ErrorCategory>,
    
    /// 超时后是否放弃剩余重试
    pub abort_on_timeout: bool,
}

impl RetryPolicy {
    /// 创建标准指数退避策略
    pub fn exponential(max_attempts: u32) -> Self {
        Self {
            max_attempts,
            backoff: BackoffStrategy::exponential_default(),
            retryable_categories: vec![
                ErrorCategory::NetworkTimeout,
                ErrorCategory::RateLimited { retry_after_secs: 60 },
                ErrorCategory::ModelErrorOverloaded,
                ErrorCategory::ServerError(503),
            ],
            abort_on_timeout: true,
        }
    }
    
    /// 创建无重试策略
    pub fn no_retry() -> Self {
        Self {
            max_attempts: 0,
            backoff: BackoffStrategy::Fixed { interval_ms: 0 },
            retryable_categories: vec![],
            abort_on_timeout: true,
        }
    }
    
    /// 判断某次尝试是否应该重试
    pub fn should_retry(&self, attempt: u32, category: &ErrorCategory) -> bool {
        if attempt >= self.max_attempts {
            return false;
        }
        
        if self.retryable_categories.is_empty() {
            return true;  // 空 = 所有都可重试
        }
        
        self.retryable_categories.iter().any(|c| {
            std::mem::discriminant(c) == std::mem::discriminant(category)
        })
    }
    
    /// 获取下次重试延迟
    pub fn next_delay(&self, attempt: u32) -> Option<Duration> {
        self.backoff.calculate_delay(attempt)
            .map(|ms| Duration::from_millis(ms))
    }
}

impl Default for RetryPolicy {
    fn default() -> Self {
        Self::exponential(MAX_RETRY_ATTEMPTS)
    }
}

// ════════════════════════════════════════════════════════════════

/// 工具降级注册表
#[derive(Debug, Clone, Default)]
pub struct ToolFallbackRegistry {
    /// 工具名 -> 备选工具列表 (按优先级排序)
    fallbacks: HashMap<String, Vec<String>>,
}

impl ToolFallbackRegistry {
    pub fn new() -> Self {
        Self::default()
    }
    
    /// 注册工具降级链
    pub fn register(&mut self, primary: &str, fallbacks: Vec<&str>) {
        self.fallbacks.insert(
            primary.to_lowercase(), 
            fallbacks.into_iter().map(|s| s.to_lowercase()).collect()
        );
    }
    
    /// 获取工具的备选项
    pub fn get_fallbacks(&self, tool_name: &str) -> Option<Vec<String>> {
        self.fallbacks.get(&tool_name.to_lowercase()).cloned()
    }
    
    /// 检查是否有备选方案
    pub fn has_fallback(&self, tool_name: &str) -> bool {
        self.fallbacks.contains_key(&tool_name.to_lowercase())
    }
}

// ════════════════════════════════════════════════════════════════
// Helper functions
// ════════════════════════════════════════════════════════════════

fn extract_retry_after(error_msg: &str) -> Option<u64> {
    // 匹配 "retry-after" 或 "retry_after" 数字
    let re = regex::Regex::new(r"(?:retry[-_]?after)[:\s]+(\d+)").ok()?;
    re.captures(error_msg)?
        .get(1)?
        .as_str()
        .parse::<u64>()
        .ok()
}

fn extract_status_code(error_msg: &str) -> Option<u16> {
    let re = regex::Regex::new(r"\b(5\d{2})\b").ok()?;
    re.captures(error_msg)?
        .get(1)?
        .as_str()
        .parse::<u16>()
        .ok()
}
