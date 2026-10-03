//! The fail-fast usage-limit error carries its reset time in machine-readable
//! form, so a server-initiated turn can resume at the reset and clients get
//! `retry_after_secs`.

use super::{anthropic_status_error, tag_usage_limit_reset};
use jcode_provider_core::usage_limit_resume::usage_limit_hint;
use reqwest::header::{HeaderMap, HeaderValue};

fn unified_limit_headers(reset_in_secs: i64) -> HeaderMap {
    let mut headers = HeaderMap::new();
    headers.insert(
        "anthropic-ratelimit-unified-status",
        HeaderValue::from_static("rejected"),
    );
    headers.insert(
        "anthropic-ratelimit-unified-reset",
        HeaderValue::from_str(&(chrono::Utc::now().timestamp() + reset_in_secs).to_string())
            .unwrap(),
    );
    headers.insert("retry-after", HeaderValue::from_static("2400"));
    headers
}

fn failed_response(headers: &HeaderMap, body: &str) -> anyhow::Error {
    let status = reqwest::StatusCode::TOO_MANY_REQUESTS;
    tag_usage_limit_reset(anthropic_status_error(status, headers, body), headers)
}

#[test]
fn far_usage_limit_carries_its_reset_in_seconds() {
    // 40m 37s: the message rounds to "resets in 41m", the typed reset keeps
    // the exact second from the unified headers.
    let reset_in = 40 * 60 + 37;
    let headers = unified_limit_headers(reset_in);
    let error = failed_response(&headers, "rate_limit_error");
    assert!(
        error.chain().any(|cause| cause
            .downcast_ref::<jcode_provider_core::usage_limit_resume::UsageLimitReset>()
            .is_some()),
        "the fail-fast error must carry a machine-readable reset"
    );
    let hint = usage_limit_hint(&error).expect("far limit is a usage limit");
    let secs = hint
        .reset_in_secs()
        .expect("reset from the unified headers");
    assert!(
        (reset_in as u64 - 2..=reset_in as u64).contains(&secs),
        "{secs}"
    );
    // Display text is unchanged, and the fail-fast type is still on the chain.
    assert!(error.to_string().contains("resets in 41m"), "{error}");
    assert!(
        error
            .chain()
            .any(|cause| cause.downcast_ref::<super::UsageLimitExhausted>().is_some())
    );
}

#[test]
fn body_only_usage_limit_is_a_usage_limit_without_a_reset() {
    let error = failed_response(&HeaderMap::new(), "You have reached your usage limit");
    let hint = usage_limit_hint(&error).expect("body names a usage limit");
    assert_eq!(hint.reset_in, None);
}

#[test]
fn short_rate_limits_are_not_tagged() {
    let mut headers = HeaderMap::new();
    headers.insert("retry-after", HeaderValue::from_static("7"));
    let error = failed_response(&headers, "rate_limit_error");
    assert!(usage_limit_hint(&error).is_none());
    assert!(
        jcode_provider_core::retry_after::retry_after_from_error(&error).is_some(),
        "an ordinary 429 keeps its Retry-After hint"
    );
    let near = failed_response(&unified_limit_headers(30), "rate_limit_error");
    assert!(usage_limit_hint(&near).is_none());
}
