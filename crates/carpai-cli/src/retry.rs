//! # Retry Utility
//!
//! Exponential backoff retry for network operations and transient failures.
//! Used by agent_bridge and notification modules for resilient execution.

use std::future::Future;
use std::time::Duration;
use tracing::{warn, info};

/// Retry configuration with exponential backoff + jitter
#[derive(Debug, Clone)]
pub struct RetryConfig {
    /// Maximum number of retry attempts (excluding the initial attempt)
    pub max_retries: u32,
    /// Base delay in milliseconds (doubles each retry)
    pub base_delay_ms: u64,
    /// Maximum delay cap in milliseconds
    pub max_delay_ms: u64,
    /// Whether to add random jitter to delay
    pub jitter: bool,
}

impl Default for RetryConfig {
    fn default() -> Self {
        Self {
            max_retries: 3,
            base_delay_ms: 500,
            max_delay_ms: 10_000,
            jitter: true,
        }
    }
}

impl RetryConfig {
    /// Retry a fallible async operation with exponential backoff.
    ///
    /// Only retries if the error matches the `should_retry` predicate.
    /// Returns the first non-retriable error or the last retry error.
    pub async fn retry<F, Fut, T, E>(
        &self,
        operation_name: &str,
        should_retry: impl Fn(&E) -> bool,
        operation: F,
    ) -> Result<T, E>
    where
        F: Fn() -> Fut,
        Fut: Future<Output = Result<T, E>>,
    {
        let mut last_error: Option<E> = None;

        for attempt in 0..=self.max_retries {
            if attempt > 0 {
                let delay = self.backoff_delay(attempt);
                tokio::time::sleep(delay).await;
                warn!(
                    operation = %operation_name,
                    attempt,
                    max_retries = self.max_retries,
                    delay_ms = delay.as_millis(),
                    "Retrying operation"
                );
            }

            match operation().await {
                Ok(result) => {
                    if attempt > 0 {
                        info!(operation = %operation_name, attempt, "Retry succeeded");
                    }
                    return Ok(result);
                }
                Err(e) => {
                    if should_retry(&e) && attempt < self.max_retries {
                        last_error = Some(e);
                        // Will retry
                    } else {
                        return Err(e);
                    }
                }
            }
        }

        // Unreachable in practice, but compiler doesn't know that
        Err(last_error.expect("retry loop ended without error"))
    }

    /// Calculate backoff delay with optional jitter
    fn backoff_delay(&self, attempt: u32) -> Duration {
        let exponential = self.base_delay_ms * 2u64.pow(attempt.saturating_sub(1));
        let capped = exponential.min(self.max_delay_ms);

        let delay = if self.jitter {
            // Add ±25% jitter
            let jitter_range = capped / 4;
            let jitter = if jitter_range > 0 {
                fastrand::i64(0..=(jitter_range as i64)) as u64
            } else {
                0
            };
            capped + jitter
        } else {
            capped
        };

        Duration::from_millis(delay)
    }
}

/// Retry an operation with default configuration, retrying on all errors.
pub async fn retry_default<F, Fut, T, E>(
    operation_name: &str,
    operation: F,
) -> Result<T, E>
where
    F: Fn() -> Fut,
    Fut: Future<Output = Result<T, E>>,
{
    RetryConfig::default().retry(operation_name, |_: &E| true, operation).await
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::sync::atomic::{AtomicU32, Ordering};

    #[test]
    fn test_backoff_delay_increases() {
        let config = RetryConfig {
            jitter: false,
            ..Default::default()
        };
        let d1 = config.backoff_delay(1);
        let d2 = config.backoff_delay(2);
        let d3 = config.backoff_delay(3);
        assert!(d2 >= d1);
        assert!(d3 >= d2);
    }

    #[test]
    fn test_backoff_caps_at_max() {
        let config = RetryConfig {
            base_delay_ms: 1000,
            max_delay_ms: 4000,
            jitter: false,
            ..Default::default()
        };
        let d5 = config.backoff_delay(5); // 1000 * 2^4 = 16000, capped to 4000
        assert_eq!(d5.as_millis(), 4000);
    }

    #[tokio::test]
    async fn test_retry_succeeds_after_failures() {
        let config = RetryConfig::default();
        let counter = AtomicU32::new(0);

        let result = config
            .retry("test", |_: &()| true, || {
                let c = &counter;
                async move {
                    let prev = c.fetch_add(1, Ordering::SeqCst);
                    if prev < 2 {
                        Err(())
                    } else {
                        Ok("success")
                    }
                }
            })
            .await;

        assert_eq!(result.unwrap(), "success");
        assert_eq!(counter.load(Ordering::SeqCst), 3);
    }

    #[tokio::test]
    async fn test_retry_exhausts_attempts() {
        let config = RetryConfig {
            max_retries: 2,
            base_delay_ms: 10,
            max_delay_ms: 50,
            ..Default::default()
        };
        let counter = AtomicU32::new(0);

        let result: Result<(), &str> = config
            .retry("test", |_: &&str| true, || {
                let c = &counter;
                async move {
                    c.fetch_add(1, Ordering::SeqCst);
                    Err("always fail")
                }
            })
            .await;

        assert!(result.is_err());
        // 3 total: initial + 2 retries
        assert_eq!(counter.load(Ordering::SeqCst), 3);
    }

    #[tokio::test]
    async fn test_non_retriable_error() {
        let config = RetryConfig {
            max_retries: 5,
            base_delay_ms: 10,
            ..Default::default()
        };
        let counter = AtomicU32::new(0);

        let result: Result<(), &str> = config
            .retry("test", |e: &&str| *e != "fatal", || {
                let c = &counter;
                async move {
                    c.fetch_add(1, Ordering::SeqCst);
                    Err("fatal")
                }
            })
            .await;

        assert!(result.is_err());
        assert_eq!(result.unwrap_err(), "fatal");
        // Only 1 attempt since error is non-retriable
        assert_eq!(counter.load(Ordering::SeqCst), 1);
    }
}
