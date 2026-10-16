use serde::{Deserialize, Serialize};

const PROVIDER_FAILOVER_PROMPT_PREFIX: &str = "[jcode-provider-failover]";

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
pub struct ProviderFailoverPrompt {
    pub from_provider: String,
    pub from_label: String,
    pub to_provider: String,
    pub to_label: String,
    /// Exact pool-approved model request. Old prompts without a route require manual consent.
    #[serde(default)]
    pub target_route: Option<String>,
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
        // Cached permanent refresh rejection no longer carries the original
        // OAuth error code or HTTP status, but still needs account failover.
        "refresh token was previously rejected",
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
    // Explicit HTTP failures take precedence over billing words in the payload.
    if let Some(status) = parse_http_status_message(&lower) {
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

/// Parse a formatted status, a relayed HTTP status, or a leading status code.
/// Request metadata and codes embedded in identifiers or longer numbers are ignored.
fn parse_http_status_message(message: &str) -> Option<u16> {
    let lower = message.to_ascii_lowercase();
    for line in lower.lines().filter(|line| !is_request_metadata(line)) {
        let line = line.trim_start();
        let tail = ["status:", "http "]
            .iter()
            .find_map(|marker| {
                line.match_indices(marker).find_map(|(idx, _)| {
                    let boundary = idx == 0
                        || !line.as_bytes()[idx - 1].is_ascii_alphanumeric()
                            && line.as_bytes()[idx - 1] != b'_';
                    boundary.then_some(&line[idx + marker.len()..])
                })
            })
            .unwrap_or(line)
            .trim_start();
        let digits = tail.bytes().take_while(u8::is_ascii_digit).count();
        if digits == 3
            && !tail
                .as_bytes()
                .get(digits)
                .is_some_and(|byte| byte.is_ascii_alphanumeric() || *byte == b'_')
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
    fn billing_metadata_and_status_boundaries() {
        for message in [
            "model: model-402\n status: 401 Unauthorized\n response: invalid API key",
            "endpoint: https://example.test/402\n status: 500 Internal Server Error\n response: upstream unavailable",
            "auth: payment required\n status: 403 Forbidden\n response: denied",
            "HTTP 401: payment required by an unrelated service",
            "model: model-402",
            "endpoint: https://example.test/402",
            "auth: insufficient credit",
        ] {
            assert!(!is_billing_exhausted_error_message(message), "{message}");
        }
        assert!(is_billing_exhausted_error_message(
            "HTTP 402: payment required"
        ));
        assert!(is_billing_exhausted_error_message(
            "response: insufficient_balance"
        ));
        for (message, expected) in [
            ("STATUS:529 upstream busy", 529),
            ("proxy relayed HTTP 529: upstream busy", 529),
            ("429 Too Many Requests", 429),
            ("status: 100 Continue", 100),
            ("status: 599 upstream failed", 599),
            (
                "endpoint: HTTP 529\n MODEL: status: 503\n auth: HTTP 402\n status: 401 temporarily unavailable",
                401,
            ),
        ] {
            assert_eq!(
                parse_http_status_message(message),
                Some(expected),
                "{message}"
            );
        }
        for message in [
            "",
            "no status here",
            "endpoint: HTTP 529",
            "model: status: 503",
            "auth: HTTP 401",
            "model529 failed",
            "status: 5290",
            "HTTP 529busy",
            "status: 529_id",
            "mystatus: 529",
            "model_http 529",
            "status: 099",
            "status: 600",
            "status: 99",
            "status: nope",
            "endpoint: http://localhost:529",
        ] {
            assert_eq!(parse_http_status_message(message), None, "{message}");
        }
    }

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
    fn failover_prompt_roundtrips_from_error_message() {
        let prompt = ProviderFailoverPrompt {
            from_provider: "claude".to_string(),
            from_label: "Anthropic".to_string(),
            to_provider: "openai".to_string(),
            to_label: "OpenAI".to_string(),
            target_route: Some("openai-oauth@work:gpt-5.4".into()),
            reason: "rate limit".to_string(),
            estimated_input_chars: 1200,
            estimated_input_tokens: 300,
        };

        let parsed = parse_failover_prompt_message(&prompt.to_error_message()).expect("prompt");
        assert_eq!(parsed, prompt);
    }

    #[test]
    fn legacy_prompt_without_target_route_requires_manual_selection() {
        let wire = r#"[jcode-provider-failover]{"from_provider":"claude","from_label":"Anthropic","to_provider":"openai","to_label":"OpenAI","reason":"rate limit","estimated_input_chars":1200,"estimated_input_tokens":300}"#;
        let prompt = parse_failover_prompt_message(wire).expect("legacy prompt");
        assert_eq!(prompt.target_route, None);
    }

    #[test]
    fn classifier_marks_rate_limits_unavailable() {
        assert_eq!(
            classify_failover_error_message("429 Too Many Requests"),
            FailoverDecision::RetryAndMarkUnavailable
        );
    }

    #[test]
    fn classifier_marks_cached_refresh_rejection_unavailable() {
        for message in [
            "Claude OAuth refresh token was previously rejected by Anthropic and cannot be refreshed. Run `jcode login --provider claude` to mint a fresh token.",
            "claude refresh token was previously rejected by the provider and cannot be refreshed. Run `jcode login`.",
            "Failed to load credentials: REFRESH TOKEN WAS PREVIOUSLY REJECTED by the provider",
        ] {
            assert_eq!(
                classify_failover_error_message(message),
                FailoverDecision::RetryAndMarkUnavailable,
                "cached permanent refresh rejection must permit account failover: {message}",
            );
        }
        // Generic refresh failures are not evidence of invalid credentials.
        for message in [
            "Claude token refresh failed: 503 Service Unavailable",
            "Claude token refresh failed: connection reset by peer",
            "Claude token refresh failed: request timed out",
        ] {
            assert_eq!(
                classify_failover_error_message(message),
                FailoverDecision::None,
                "transient refresh failure must not poison an account: {message}",
            );
        }
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
