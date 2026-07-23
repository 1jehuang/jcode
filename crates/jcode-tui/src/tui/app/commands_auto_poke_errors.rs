//! Deterministic non-retryable auto-poke error classification.

pub(crate) fn is_non_retryable_auto_poke_error(error: &str) -> bool {
    let lower = error.to_ascii_lowercase();

    // These failures are deterministic for the current request/session shape. Retrying the same
    // auto-poke cannot help and can create an infinite spam loop.
    let deterministic_markers = [
        "400 bad request",
        "invalid_request_error",
        "string_above_max_length",
        "string_too_long",
        "maximum length",
        "request too large",
        "payload too large",
        "body too large",
        "input too large",
        "context length exceeded",
        "context_length_exceeded",
        "maximum context length",
        "token limit exceeded",
        "invalid model",
        "model_not_found",
        "model_not_supported",
        "unsupportedmodel",
        "unsupported model",
        "does not support the coding plan",
        "coding plan feature",
        "unsupported parameter",
        "unsupported_value",
        "invalid parameter",
        "invalid schema",
        "invalid tool",
        "invalid image",
        "image too large",
        "unsupported image",
        "unsupported file",
        "file too large",
        "content_policy_violation",
        "safety_violation",
        "permission_denied",
        "unauthorized",
        "401 unauthorized",
        "403 forbidden",
        "insufficient_quota",
        "402 payment required",
        "payment required",
        "requires more credits",
        "add more credits",
        "more credits",
        "billing",
        "credit balance",
        "out of credits",
        // Plan/usage-window exhaustion (e.g. OpenAI OAuth
        // `usage_limit_reached`). These reset hours-to-days later, so
        // re-poking the same request just burns refused API calls in a loop.
        "usage_limit_reached",
        "usage limit has been reached",
        "usage limit reached",
        "quota exceeded",
        "quota_exceeded",
    ];

    deterministic_markers
        .iter()
        .any(|marker| lower.contains(marker))
}
