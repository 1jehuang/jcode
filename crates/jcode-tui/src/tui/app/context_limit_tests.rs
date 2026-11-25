use super::*;

#[test]
fn context_limit_error_recognizes_canonical_codes_and_ntl_wording() {
    for error in [
        r#"HTTP 400: {"error":"The chat context is too large.","code":"prompt_too_large"}"#,
        "The chat context is too large. Start a new chat or summarize the current one.",
        "prompt_too_large",
        "context_length_exceeded",
        "CONTEXT_LENGTH_EXCEEDED",
        "This model's maximum context length is 200000 tokens",
    ] {
        assert!(is_context_limit_error(error), "{error}");
    }
}

#[test]
fn context_limit_error_does_not_relabel_image_payload_or_rate_limit_failures() {
    for error in [
        "HTTP 413: Payload Too Large",
        "HTTP 400: image is too large",
        "HTTP 400: invalid image data",
        "HTTP 400: message exceeds the 50,000 character limit",
        "HTTP 400: invalid_request_error",
        "rate limit exceeded, retry after 20s",
    ] {
        assert!(!is_context_limit_error(error), "{error}");
    }
}
