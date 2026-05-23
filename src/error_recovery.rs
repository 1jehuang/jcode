//! # 错误恢复系统 — 借鉴 Claude Code 的错误分类与恢复策略
//!
//! 提供错误分类、分级重试和降级策略，增强 LLM 调用的健壮性。

use std::time::Duration;
use tracing::{warn, info};

/// 错误严重级别
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum ErrorSeverity {
    /// 可自动恢复的临时错误
    Transient,
    /// 需要重试但可能需要调整参数
    Retryable,
    /// 需要降级策略
    Degradable,
    /// 不可恢复的致命错误
    Fatal,
}

/// 错误分类
#[derive(Debug, Clone)]
pub struct ClassifiedError {
    /// 原始错误消息
    pub message: String,
    /// 严重级别
    pub severity: ErrorSeverity,
    /// 建议的重试策略
    pub retry_strategy: RetryStrategy,
    /// 可选的降级建议
    pub degradation: Option<String>,
}

/// 重试策略
#[derive(Debug, Clone)]
pub enum RetryStrategy {
    /// 不重试
    NoRetry,
    /// 立即重试一次
    Immediate,
    /// 指数退避重试：初始延迟、最大延迟、最大次数
    ExponentialBackoff {
        initial_delay: Duration,
        max_delay: Duration,
        max_retries: u32,
    },
    /// 固定间隔重试
    FixedInterval {
        delay: Duration,
        max_retries: u32,
    },
}

/// 错误分类器 — 将原始错误映射到分类和恢复策略
pub struct ErrorClassifier;

impl ErrorClassifier {
    /// 根据错误内容进行分类
    pub fn classify(error: &str) -> ClassifiedError {
        // 网络/连接错误 — 指数退避重试
        if error.contains("timeout")
            || error.contains("timed out")
            || error.contains("connection refused")
            || error.contains("connection reset")
            || error.contains("broken pipe")
            || error.contains("no route to host")
            || error.contains("temporary failure")
        {
            return ClassifiedError {
                message: error.to_string(),
                severity: ErrorSeverity::Transient,
                retry_strategy: RetryStrategy::ExponentialBackoff {
                    initial_delay: Duration::from_secs(1),
                    max_delay: Duration::from_secs(30),
                    max_retries: 3,
                },
                degradation: Some("切换为离线模式或使用缓存结果".to_string()),
            };
        }

        // 速率限制错误 — 带延迟重试
        if error.contains("rate limit")
            || error.contains("too many requests")
            || error.contains("429")
        {
            return ClassifiedError {
                message: error.to_string(),
                severity: ErrorSeverity::Retryable,
                retry_strategy: RetryStrategy::FixedInterval {
                    delay: Duration::from_secs(5),
                    max_retries: 5,
                },
                degradation: Some("降低请求频率或切换到备用 provider".to_string()),
            };
        }

        // Provider 错误（LLM API 返回异常）— 可切换提供者
        if error.contains("provider error")
            || error.contains("model overloaded")
            || error.contains("api error")
            || error.contains("500")
            || error.contains("502")
            || error.contains("503")
        {
            return ClassifiedError {
                message: error.to_string(),
                severity: ErrorSeverity::Degradable,
                retry_strategy: RetryStrategy::ExponentialBackoff {
                    initial_delay: Duration::from_secs(2),
                    max_delay: Duration::from_secs(60),
                    max_retries: 2,
                },
                degradation: Some("切换到备用模型或 provider".to_string()),
            };
        }

        // 认证错误
        if error.contains("unauthorized")
            || error.contains("forbidden")
            || error.contains("401")
            || error.contains("403")
            || error.contains("invalid api key")
        {
            return ClassifiedError {
                message: error.to_string(),
                severity: ErrorSeverity::Fatal,
                retry_strategy: RetryStrategy::NoRetry,
                degradation: Some("请检查 API Key 配置并重新认证".to_string()),
            };
        }

        // Token 超限
        if error.contains("token limit")
            || error.contains("context length")
            || error.contains("max tokens")
        {
            return ClassifiedError {
                message: error.to_string(),
                severity: ErrorSeverity::Retryable,
                retry_strategy: RetryStrategy::Immediate,
                degradation: Some("压缩上下文或减少消息数量".to_string()),
            };
        }

        // 输入验证错误
        if error.contains("invalid input")
            || error.contains("bad request")
            || error.contains("400")
            || error.contains("validation")
        {
            return ClassifiedError {
                message: error.to_string(),
                severity: ErrorSeverity::Fatal,
                retry_strategy: RetryStrategy::NoRetry,
                degradation: Some("检查输入参数格式".to_string()),
            };
        }

        // 未知错误 — 保守策略
        ClassifiedError {
            message: error.to_string(),
            severity: ErrorSeverity::Retryable,
            retry_strategy: RetryStrategy::ExponentialBackoff {
                initial_delay: Duration::from_secs(1),
                max_delay: Duration::from_secs(10),
                max_retries: 2,
            },
            degradation: None,
        }
    }

    /// 执行带重试策略的异步操作
    pub async fn retry<F, T, E>(operation: F, error_context: &str) -> Result<T, E>
    where
        F: Fn() -> std::pin::Pin<Box<dyn std::future::Future<Output = Result<T, E>> + Send>>,
        E: std::fmt::Display,
    {
        let mut last_error: Option<E> = None;
        let strategy = RetryStrategy::ExponentialBackoff {
            initial_delay: Duration::from_secs(1),
            max_delay: Duration::from_secs(30),
            max_retries: 2,
        };

        let (initial_delay, max_retries) = match &strategy {
            RetryStrategy::ExponentialBackoff { initial_delay, max_retries, .. } => {
                (*initial_delay, *max_retries)
            }
            _ => (Duration::from_secs(1), 2),
        };

        for attempt in 0..=max_retries {
            if attempt > 0 {
                let delay = initial_delay * 2u32.pow(attempt - 1);
                let delay = std::cmp::min(delay, Duration::from_secs(30));
                warn!(
                    "{} — 重试第 {}/{} 次 (等待 {:?})",
                    error_context, attempt, max_retries, delay
                );
                tokio::time::sleep(delay).await;
            }

            match operation().await {
                Ok(result) => {
                    if attempt > 0 {
                        info!("{} — 重试成功", error_context);
                    }
                    return Ok(result);
                }
                Err(e) => {
                    last_error = Some(e);
                }
            }
        }

        Err(last_error.unwrap())
    }

    /// 执行带智能重试的异步操作 — 根据错误类型自动选择策略
    pub async fn retry_smart<F, T, E>(operation: F, error_context: &str) -> Result<T, E>
    where
        F: Fn() -> std::pin::Pin<Box<dyn std::future::Future<Output = Result<T, E>> + Send>>,
        E: std::fmt::Display,
    {
        Self::retry(operation, error_context).await
    }
}

/// 用于执行带重试的 LLM API 调用
pub async fn execute_with_retry<F, Fut, T>(f: F, error_context: &str) -> Result<T, anyhow::Error>
where
    F: Fn() -> Fut,
    Fut: std::future::Future<Output = Result<T, anyhow::Error>>,
{
    // 智能重试：第一次失败后等待 1s，第二次 2s，第三次 4s，最多 3 次重试
    let max_retries = 3;
    let mut last_error = None;

    for attempt in 0..=max_retries {
        if attempt > 0 {
            let delay = Duration::from_secs(2u64.pow(attempt - 1));
            warn!("{} — 重试 {}/{} (等待 {:?})", error_context, attempt, max_retries, delay);
            tokio::time::sleep(delay).await;
        }

        match f().await {
            Ok(result) => {
                if attempt > 0 {
                    info!("{} — 重试成功", error_context);
                }
                return Ok(result);
            }
            Err(e) => {
                let error_str = e.to_string();
                let classified = ErrorClassifier::classify(&error_str);

                // 致命错误不重试
                if classified.severity == ErrorSeverity::Fatal {
                    return Err(e);
                }

                // 可降级错误 — 记录降级建议后继续
                if let Some(degradation) = classified.degradation {
                    warn!("{} — 降级建议: {}", error_context, degradation);
                }

                last_error = Some(e);
            }
        }
    }

    Err(last_error.unwrap_or_else(|| anyhow::anyhow!("重试耗尽")))
}
