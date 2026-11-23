//! Deterministic non-retryable auto-poke error classification.

pub(crate) fn is_non_retryable_auto_poke_error(error: &str) -> bool {
    // Include locally cached refresh rejections, which have neither an HTTP
    // status nor the original provider error code. Do not classify generic
    // refresh failures here: network and 5xx failures must remain retryable.
    if crate::auth::refresh_state::error_is_permanent_rejection(error) {
        return true;
    }
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

/// Whether `error` is a transient connectivity failure (DNS, name resolution,
/// routing, unreachable host) that the agent itself cannot repair by resending
/// immediately. These are NOT non-retryable: they resolve once the network
/// environment recovers, so callers route them to a network-wait/resume path
/// rather than stopping auto-poke. Kept separate from
/// [`is_non_retryable_auto_poke_error`] precisely so a transient disconnect is
/// never treated as a permanent failure.
pub(crate) fn is_auto_poke_connectivity_error(error: &str) -> bool {
    // Delegate to the shared connectivity classifier (jcode-app-core's
    // network_retry) so this list can never drift out of sync with the wait-
    // for-network path, then add wrappers specific to this call site.
    if crate::network_retry::classify_message(error).is_some() {
        return true;
    }

    let lower = error.to_ascii_lowercase();

    let connectivity_markers = [
        "failed to send openai-compatible chat request",
        "could not resolve host",
        "couldn't resolve host",
    ];

    connectivity_markers
        .iter()
        .any(|marker| lower.contains(marker))
}

/// Whether `error` is a deterministic model/endpoint-capability failure that can
/// never succeed by resending the identical request: the configured model is not
/// valid for the configured endpoint (e.g. Volcengine Ark's coding-plan endpoint
/// returning `404 UnsupportedModel` for a model that lacks the coding plan
/// feature, or a plain model-not-found). Unlike the broader
/// [`is_non_retryable_auto_poke_error`] set (which also covers billing, payload
/// size, auth, etc.), this is narrow enough that we can fail fast *regardless of
/// auto-poke* during reconnect/recovery continuation instead of burning the
/// retry budget on a request that is structurally guaranteed to 4xx. See #387.
pub(crate) fn is_fatal_model_endpoint_error(error: &str) -> bool {
    let lower = error.to_ascii_lowercase();

    let model_endpoint_markers = [
        "unsupportedmodel",
        "unsupported model",
        "does not support the coding plan",
        "coding plan feature",
        "model_not_found",
        "model_not_supported",
        "invalid model",
        "the model does not exist",
        "model does not exist",
    ];

    model_endpoint_markers
        .iter()
        .any(|marker| lower.contains(marker))
}
