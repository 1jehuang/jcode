//! Per-request retry sleep accounting. Request/stream time is deliberately excluded.
use std::time::Duration;

#[derive(Default)]
pub(super) struct RetrySleepBudget {
    remaining: Option<Duration>,
}

impl RetrySleepBudget {
    pub(super) fn new(seconds: Option<u64>) -> Self {
        Self {
            remaining: seconds.map(Duration::from_secs),
        }
    }

    /// Reserve the entire required sleep or refuse it, never shortening a hint.
    pub(super) fn reserve(&mut self, delay: Duration) -> bool {
        match self.remaining {
            Some(remaining) => match remaining.checked_sub(delay) {
                Some(next) => {
                    self.remaining = Some(next);
                    true
                }
                None => false,
            },
            None => true,
        }
    }
}

pub(super) fn retry_reason(error: &anyhow::Error) -> jcode_message_types::RetryReason {
    use jcode_message_types::RetryReason;
    for source in error.chain() {
        let message = source.to_string();
        // The wrapper is constructed from the response's actual status. Never
        // interpret status-like digits or prose inside its untrusted body.
        if let Some(status) = message.strip_prefix("Anthropic API error (") {
            return match status.split_whitespace().next() {
                Some("429") => RetryReason::RateLimit,
                Some("529") => RetryReason::Overloaded,
                _ => RetryReason::Unknown,
            };
        }
        if let Some(payload) = message.strip_prefix("Retryable stream error: ") {
            return match serde_json::from_str::<serde_json::Value>(payload)
                .ok()
                .and_then(|value| value.get("error")?.get("type")?.as_str().map(str::to_owned))
                .as_deref()
            {
                Some("rate_limit_error") => RetryReason::RateLimit,
                Some("overloaded_error") => RetryReason::Overloaded,
                _ => RetryReason::Unknown,
            };
        }
        if source.is::<reqwest::Error>() || message.starts_with("Stream read timeout: ") {
            return RetryReason::Transport;
        }
    }
    RetryReason::Unknown
}

#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    fn cumulative_reservations_do_not_truncate_or_consume_refused_waits() {
        let mut budget = RetrySleepBudget::new(Some(30));
        assert!(!budget.reserve(Duration::from_secs(60)));
        assert!(!budget.reserve(Duration::from_secs(60)));
        assert!(budget.reserve(Duration::from_secs(10)));
        assert!(!budget.reserve(Duration::from_secs(21)));
        assert!(budget.reserve(Duration::from_secs(20)));
        assert!(!budget.reserve(Duration::from_nanos(1)));
    }
    #[test]
    fn disabled_and_zero_budgets() {
        let mut disabled = RetrySleepBudget::new(None);
        for _ in 0..10 {
            assert!(disabled.reserve(Duration::from_secs(60)));
        }
        let mut zero = RetrySleepBudget::new(Some(0));
        assert!(zero.reserve(Duration::ZERO));
        assert!(!zero.reserve(Duration::from_nanos(1)));
    }
    #[test]
    fn metadata_has_only_finite_reasons() {
        use jcode_message_types::RetryReason::*;
        for (message, expected) in [
            ("Anthropic API error (429 Too Many Requests): {}", RateLimit),
            (
                "Anthropic API error (529 <unknown status code>): rate_limit_error",
                Overloaded,
            ),
            (
                "Anthropic API error (500 Internal Server Error): request_id abc429",
                Unknown,
            ),
            (
                "Anthropic API error (500 Internal Server Error): connection reset",
                Unknown,
            ),
            (
                "Stream read timeout: no data received for 180 seconds",
                Transport,
            ),
            (
                r#"Retryable stream error: {"error":{"type":"rate_limit_error","message":"test"}}"#,
                RateLimit,
            ),
            (
                r#"Retryable stream error: {"error":{"type":"overloaded_error","message":"rate_limit_error"}}"#,
                Overloaded,
            ),
            (
                r#"Retryable stream error: {"error":{"type":"api_error","message":"429 overloaded connection reset"}}"#,
                Unknown,
            ),
            ("HTTP 500 request_id abc429", Unknown),
            ("connection reset", Unknown),
            ("rate_limit_error", Unknown),
        ] {
            assert_eq!(retry_reason(&anyhow::anyhow!(message)), expected);
        }
    }
}
