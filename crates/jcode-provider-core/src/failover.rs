use serde::{Deserialize, Serialize};

const PROVIDER_FAILOVER_PROMPT_PREFIX: &str = "[jcode-provider-failover]";

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
pub struct ProviderFailoverPrompt {
    pub from_provider: String,
    pub from_label: String,
    pub to_provider: String,
    pub to_label: String,
    pub reason: String,
    pub estimated_input_chars: usize,
    pub estimated_input_tokens: usize,
}

impl ProviderFailoverPrompt {
    pub fn to_error_message(&self) -> String {
        let payload = serde_json::to_string(self).unwrap_or_else(|_| "{}".to_string());
        format!(
            "{PROVIDER_FAILOVER_PROMPT_PREFIX}{payload}\n{} is unavailable; switching to {} would resend about {} input tokens (~{} chars).",
            self.from_label, self.to_label, self.estimated_input_tokens, self.estimated_input_chars,
        )
    }
}

pub fn parse_failover_prompt_message(message: &str) -> Option<ProviderFailoverPrompt> {
    let line = message.lines().next()?.trim();
    let json = line.strip_prefix(PROVIDER_FAILOVER_PROMPT_PREFIX)?;
    serde_json::from_str(json).ok()
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum FailoverDecision {
    None,
    RetryNextProvider,
    RetryAndMarkUnavailable,
}

impl FailoverDecision {
    pub fn should_failover(self) -> bool {
        !matches!(self, Self::None)
    }

    pub fn should_mark_provider_unavailable(self) -> bool {
        matches!(self, Self::RetryAndMarkUnavailable)
    }

    pub fn as_str(self) -> &'static str {
        match self {
            Self::None => "none",
            Self::RetryNextProvider => "retry-next-provider",
            Self::RetryAndMarkUnavailable => "retry-and-mark-unavailable",
        }
    }
}

fn contains_independent_status_code(haystack: &str, code: &str) -> bool {
    let haystack_bytes = haystack.as_bytes();
    let code_len = code.len();

    haystack.match_indices(code).any(|(start, _)| {
        let before_ok = start == 0 || !haystack_bytes[start - 1].is_ascii_digit();
        let end = start + code_len;
        let after_ok = end == haystack_bytes.len() || !haystack_bytes[end].is_ascii_digit();
        before_ok && after_ok
    })
}

pub fn classify_failover_error_message(message: &str) -> FailoverDecision {
    let lower = message.to_ascii_lowercase();

    let request_size_or_context = [
        "context length",
        "context_length",
        "context window",
        "maximum context",
        "prompt is too long",
        "input is too long",
        "too many tokens",
        "max tokens",
        "token limit",
        "token_limit",
        "413 payload too large",
        "413 request entity too large",
    ]
    .iter()
    .any(|needle| lower.contains(needle))
        || contains_independent_status_code(&lower, "413");
    if request_size_or_context {
        return FailoverDecision::RetryNextProvider;
    }

    let rate_or_quota = [
        "rate limit",
        "rate-limited",
        "too many requests",
        "quota",
        "credit balance",
        "credits have run out",
        "insufficient credit",
        "billing",
        "payment required",
        "usage tier",
    ]
    .iter()
    .any(|needle| lower.contains(needle))
        || contains_independent_status_code(&lower, "429")
        || contains_independent_status_code(&lower, "402");
    if rate_or_quota {
        return FailoverDecision::RetryAndMarkUnavailable;
    }

    let auth_or_access = [
        "access denied",
        "not accessible by integration",
        "provider unavailable",
        "provider not available",
        "provider is unavailable",
        "provider currently unavailable",
        "provider not configured",
        "credentials are not configured",
        "no credentials",
        "token exchange failed",
        "authentication failed",
        "unauthorized",
        "forbidden",
    ]
    .iter()
    .any(|needle| lower.contains(needle))
        || contains_independent_status_code(&lower, "401")
        || contains_independent_status_code(&lower, "403");
    if auth_or_access {
        return FailoverDecision::RetryAndMarkUnavailable;
    }

    FailoverDecision::None
}

/// True when an error means "this account is out of credit / payment
/// required" (HTTP 402 or billing wording) and will not recover by itself.
///
/// A usage-window 402 that carries a reset instant (`resets_at`/`reset_at`,
/// e.g. Openference's "1500 requests per 5 hours") is excluded: jcode holds
/// and auto-resumes those instead of switching provider. Only the response
/// body is searched for the reset fields (the text after `response:` in a
/// formatted HTTP error), so an endpoint URL or query string that happens to
/// contain those words does not hide a real out-of-credit error.
pub fn is_billing_exhausted_error_message(message: &str) -> bool {
    let lower = message.to_ascii_lowercase();
    let payload = if let Some(idx) = lower.find("response:") {
        lower[idx + "response:".len()..].to_string()
    } else {
        lower
            .lines()
            .filter(|line| !is_request_metadata(line))
            .collect::<Vec<_>>()
            .join("\n")
    };
    if payload.contains("resets_at") || payload.contains("reset_at") {
        return false;
    }
    // An explicit authentication, rate-limit or server failure cannot become
    // billing exhaustion because a model, endpoint or hint mentions payment.
    if let Some(status) = billing_http_status(&lower) {
        return status == 402;
    }
    contains_independent_status_code(&payload, "402")
        || [
            "payment required",
            "insufficient credit",
            "insufficient balance",
            "insufficient_balance",
            "credit balance",
            "credits have run out",
            "out of credit",
        ]
        .iter()
        .any(|needle| payload.contains(needle))
}

fn is_request_metadata(line: &str) -> bool {
    ["endpoint:", "model:", "auth:"]
        .iter()
        .any(|prefix| line.trim_start().starts_with(prefix))
}

fn billing_http_status(message: &str) -> Option<u16> {
    for line in message.lines().filter(|line| !is_request_metadata(line)) {
        let line = line.trim_start();
        let tail = line
            .split_once("status:")
            .map(|(_, tail)| tail)
            .or_else(|| line.split_once("http ").map(|(_, tail)| tail))
            .unwrap_or(line)
            .trim_start();
        let digits = tail.bytes().take_while(u8::is_ascii_digit).count();
        if digits == 3
            && let Ok(status) = tail[..digits].parse::<u16>()
            && (100..600).contains(&status)
        {
            return Some(status);
        }
    }
    None
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn billing_exhausted_detects_402_and_credit_wording() {
        let body = "OpenAI-compatible chat request failed\n  status: 402 Payment Required\n  response: {\"error\":{\"message\":\"Insufficient credits\",\"type\":\"insufficient_quota\",\"code\":402}}";
        assert!(is_billing_exhausted_error_message(body));
        assert!(is_billing_exhausted_error_message(
            "Your credit balance is too low"
        ));
    }

    #[test]
    fn billing_exhausted_skips_usage_window_402_with_reset_time() {
        let body = "status: 402 Payment Required\n  response: {\"error\":\"Request limit exceeded (1500 per 5 hours)\",\"type\":\"insufficient_quota\",\"code\":\"window_quota_exceeded\",\"resets_at\":\"2026-09-24T09:00:00.000Z\"}";
        assert!(!is_billing_exhausted_error_message(body));
    }

    #[test]
    fn billing_exhausted_ignores_reset_words_in_the_endpoint_url() {
        let body = "OpenAI-compatible chat request failed\n  endpoint: http://127.0.0.1:9/v1/chat/completions?reset_at=0&resets_at=1\n  model: glm-5.3\n  status: 402 Payment Required\n  response: {\"error\":{\"message\":\"Insufficient credits\",\"code\":402}}";
        assert!(is_billing_exhausted_error_message(body));
    }

    #[test]
    fn billing_exhausted_ignores_rate_limits_and_embedded_digits() {
        assert!(!is_billing_exhausted_error_message("429 Too Many Requests"));
        assert!(!is_billing_exhausted_error_message("model 4021 failed"));
    }

    #[test]
    fn billing_exhausted_metadata_does_not_override_explicit_http_failure() {
        for message in [
            "request failed\n  model: model-402\n  status: 401 Unauthorized\n  response: invalid API key",
            "request failed\n  endpoint: https://example.test/402\n  status: 500 Internal Server Error\n  response: upstream unavailable",
            "request failed\n  auth: payment required\n  status: 403 Forbidden\n  response: denied",
            "HTTP 401: payment required by an unrelated service",
        ] {
            assert!(!is_billing_exhausted_error_message(message), "{message}");
        }
    }

    #[test]
    fn billing_exhausted_ignores_metadata_without_a_response_section() {
        for message in [
            "request failed\n  model: model-402",
            "request failed\n  endpoint: https://example.test/402",
            "request failed\n  auth: insufficient credit",
        ] {
            assert!(!is_billing_exhausted_error_message(message), "{message}");
        }
        assert!(is_billing_exhausted_error_message(
            "HTTP 402: payment required"
        ));
        assert!(is_billing_exhausted_error_message(
            "response: insufficient_balance"
        ));
    }

    #[test]
    fn failover_prompt_roundtrips_from_error_message() {
        let prompt = ProviderFailoverPrompt {
            from_provider: "claude".to_string(),
            from_label: "Anthropic".to_string(),
            to_provider: "openai".to_string(),
            to_label: "OpenAI".to_string(),
            reason: "rate limit".to_string(),
            estimated_input_chars: 1200,
            estimated_input_tokens: 300,
        };

        let parsed = parse_failover_prompt_message(&prompt.to_error_message()).expect("prompt");
        assert_eq!(parsed, prompt);
    }

    #[test]
    fn classifier_marks_rate_limits_unavailable() {
        assert_eq!(
            classify_failover_error_message("429 Too Many Requests"),
            FailoverDecision::RetryAndMarkUnavailable
        );
    }

    #[test]
    fn classifier_retries_context_errors_without_marking_unavailable() {
        assert_eq!(
            classify_failover_error_message("context length exceeded"),
            FailoverDecision::RetryNextProvider
        );
    }

    #[test]
    fn classifier_ignores_embedded_status_digits() {
        assert_eq!(
            classify_failover_error_message("model version 4130 failed"),
            FailoverDecision::None
        );
    }
}
