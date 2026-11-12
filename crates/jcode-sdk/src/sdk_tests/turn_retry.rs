use super::*;

fn error(message: &str) -> ApiEvent {
    ApiEvent::Error {
        code: ErrorCode::Internal,
        message: message.to_string(),
    }
}

fn stopped(reason: TurnStopReason) -> ApiEvent {
    ApiEvent::TurnStopped {
        session_id: "s".into(),
        reason,
        message: String::new(),
        provider_stop_reason: None,
    }
}

fn done() -> ApiEvent {
    ApiEvent::TurnDone {
        session_id: "s".into(),
        pending_soft_interrupts: None,
    }
}

fn scheduled(decision: &RetryDecision) -> Option<(u32, Duration)> {
    match decision {
        RetryDecision::Scheduled { attempt, delay, .. } => Some((*attempt, *delay)),
        _ => None,
    }
}

#[test]
fn transient_failures_are_retried() {
    for message in [
        "error sending request: connection reset by peer",
        "stream error: peer closed connection without sending TLS close_notify",
        "Request timed out after 180s with no data received",
        "API error 529: {\"type\":\"overloaded_error\"}",
        "HTTP 503 Service Unavailable",
        "HTTP status 500: internal server error",
        "dns error: failed to lookup address information",
    ] {
        assert_eq!(
            classify_failure(message, None),
            FailureKind::Transient,
            "{message}"
        );
    }
}

#[test]
fn deterministic_failures_are_never_retried() {
    for message in [
        "401 Unauthorized: invalid x-api-key",
        "OAuth token refresh failed: invalid_grant. Run /login",
        "insufficient_quota: You exceeded your current quota",
        "You've hit your usage limit. Resets in 3d 4h",
        "400 Bad Request: invalid_request_error: prompt is too long",
        "model_not_found: the model does not exist",
        "Context limit exceeded after 5 compaction retries",
        "content_policy_violation",
        "Interrupted by user",
        // Unknown failures are not retried: guessing could loop forever.
        "something odd happened",
    ] {
        assert_eq!(
            classify_failure(message, None),
            FailureKind::Permanent,
            "{message}"
        );
    }
}

#[test]
fn credential_markers_win_over_transient_ones() {
    assert_eq!(
        classify_failure("401 Unauthorized, then connection closed", None),
        FailureKind::Permanent
    );
}

#[test]
fn status_codes_must_stand_alone() {
    assert_eq!(
        classify_failure("failed (request id req_15030x)", None),
        FailureKind::Permanent
    );
    assert_eq!(
        classify_failure("upstream returned 502", None),
        FailureKind::Transient
    );
}

#[test]
fn rate_limits_parse_retry_hints() {
    assert_eq!(
        classify_failure("429 Too Many Requests. Please retry after 20 seconds", None),
        FailureKind::RateLimited {
            retry_after: Some(Duration::from_secs(20))
        }
    );
    assert_eq!(
        classify_failure("rate limit exceeded, retry in 2m", None),
        FailureKind::RateLimited {
            retry_after: Some(Duration::from_secs(120))
        }
    );
    assert_eq!(
        classify_failure("rate_limit_error", Some(Duration::from_secs(7))),
        FailureKind::RateLimited {
            retry_after: Some(Duration::from_secs(7))
        }
    );
}

#[test]
fn failed_turn_schedules_backoff_then_gives_up() {
    let now = Instant::now();
    let mut retry = TurnRetry::default();
    let mut delays = Vec::new();
    for _ in 0..3 {
        retry.observe(&stopped(TurnStopReason::Failure), now);
        let decision = retry.observe(&error("connection reset by peer"), now);
        let (attempt, delay) = scheduled(&decision).expect("retry scheduled");
        delays.push((attempt, delay.as_secs()));
        retry.observe(&done(), now);
        assert!(!retry.take_due(now), "not due before the delay");
        assert!(retry.take_due(now + delay));
        assert!(!retry.take_due(now + delay), "take_due fires once");
    }
    assert_eq!(delays, vec![(1, 3), (2, 6), (3, 12)]);
    assert_eq!(
        retry.observe(&error("connection reset by peer"), now),
        RetryDecision::GaveUp {
            attempts: 3,
            exhausted: true
        }
    );
}

#[test]
fn bridge_turn_done_after_failure_keeps_the_retry() {
    // The harness API bridge emits TurnStopped, Error, then TurnDone for a
    // failed turn. The closing TurnDone must not cancel the retry.
    let now = Instant::now();
    let mut retry = TurnRetry::default();
    retry.observe(&stopped(TurnStopReason::Failure), now);
    assert!(scheduled(&retry.observe(&error("connection reset"), now)).is_some());
    // Held until the failed turn closes, even after the delay.
    assert!(!retry.take_due(now + Duration::from_secs(5)));
    assert_eq!(retry.observe(&done(), now), RetryDecision::None);
    assert_eq!(retry.attempts(), 1);
    assert!(retry.take_due(now + Duration::from_secs(5)));
    // The retried turn succeeds: the budget resets.
    retry.observe(&done(), now);
    assert_eq!(retry.attempts(), 0);
}

#[test]
fn lost_close_event_does_not_hold_the_retry_forever() {
    let now = Instant::now();
    let mut retry = TurnRetry::default();
    retry.observe(&stopped(TurnStopReason::Failure), now);
    retry.observe(&error("connection reset"), now);
    assert!(retry.take_due(now + Duration::from_secs(60)));
}

#[test]
fn completed_turn_and_user_input_reset_the_budget() {
    let now = Instant::now();
    let mut retry = TurnRetry::default();
    retry.observe(&error("timed out"), now);
    assert_eq!(retry.attempts(), 1);
    assert!(retry.take_due(now + Duration::from_secs(60)));
    assert_eq!(retry.observe(&done(), now), RetryDecision::None);
    assert_eq!(retry.attempts(), 0);

    retry.observe(&error("timed out"), now);
    assert_eq!(retry.user_sent(), RetryDecision::Cancelled);
    assert_eq!(retry.pending(), None);
    assert_eq!(retry.attempts(), 0);
}

#[test]
fn cancellation_and_other_clients_drop_a_pending_retry() {
    let now = Instant::now();
    let mut retry = TurnRetry::default();
    retry.observe(&error("timed out"), now);
    let cancelled = ApiEvent::SessionStatus {
        session_id: "s".into(),
        status: "cancelled".into(),
        pending_soft_interrupts: None,
    };
    assert_eq!(retry.observe(&cancelled, now), RetryDecision::Cancelled);

    // Fresh input, not a late error from the cancelled provider request.
    retry.user_sent();
    retry.observe(&error("timed out"), now);
    let accepted = ApiEvent::MessageAccepted {
        session_id: "s".into(),
    };
    assert_eq!(retry.observe(&accepted, now), RetryDecision::Cancelled);
    assert!(!retry.take_due(now + Duration::from_secs(60)));
}

#[test]
fn interrupted_crashed_and_guardrail_turns_are_not_retried() {
    let now = Instant::now();
    for reason in [
        TurnStopReason::Interrupted,
        TurnStopReason::Crash,
        TurnStopReason::ProviderGuardrail,
        TurnStopReason::LimitReached,
    ] {
        let mut retry = TurnRetry::default();
        retry.observe(&stopped(reason), now);
        assert!(
            matches!(
                retry.observe(&error("connection reset"), now),
                RetryDecision::GaveUp {
                    exhausted: false,
                    ..
                }
            ),
            "{reason:?}"
        );
    }
}

#[test]
fn request_errors_and_busy_rejections_are_ignored() {
    let now = Instant::now();
    let mut retry = TurnRetry::default();
    let unknown = ApiEvent::Error {
        code: ErrorCode::UnknownSession,
        message: "timed out".into(),
    };
    assert_eq!(retry.observe(&unknown, now), RetryDecision::None);
    assert_eq!(
        retry.observe(&error("Already processing a message"), now),
        RetryDecision::None
    );
    assert_eq!(retry.pending(), None);
}

#[test]
fn long_rate_limits_are_left_to_the_user() {
    let now = Instant::now();
    let mut retry = TurnRetry::default();
    assert!(matches!(
        retry.observe(&error("429 rate limit, retry after 2 hours"), now),
        RetryDecision::GaveUp { .. }
    ));
    let decision = retry.observe(&error("429 rate limit, retry after 45 seconds"), now);
    assert_eq!(scheduled(&decision), Some((1, Duration::from_secs(45))));
}

#[test]
fn disabled_policy_never_schedules() {
    let now = Instant::now();
    let mut retry = TurnRetry::new(RetryPolicy {
        max_attempts: 0,
        ..RetryPolicy::default()
    });
    assert!(matches!(
        retry.observe(&error("timed out"), now),
        RetryDecision::GaveUp {
            exhausted: true,
            ..
        }
    ));
}

#[test]
fn failed_send_reschedules() {
    let now = Instant::now();
    let mut retry = TurnRetry::default();
    retry.observe(&error("timed out"), now);
    assert!(retry.take_due(now + Duration::from_secs(10)));
    retry.send_failed(now);
    assert_eq!(
        retry.pending(),
        Some(now + RetryPolicy::default().base_delay)
    );
}

#[test]
fn structured_rate_limit_hints_ignore_sibling_limits() {
    for (message, seconds) in [
        (
            r#"API error 429: {"max_rpm":1200,"retry_after_seconds":17}"#,
            17,
        ),
        (
            r#"429: {"error":{"retry_after_seconds":2.3}} diagnostics"#,
            3,
        ),
        (r#"429: {"errors":[{"retry_after_seconds":9}]}"#, 9),
    ] {
        assert_eq!(
            classify_failure(message, None),
            FailureKind::RateLimited {
                retry_after: Some(Duration::from_secs(seconds))
            },
            "{message}"
        );
    }
    for value in ["-1", "0", "\"20\"", "null"] {
        let message =
            format!(r#"429: {{"retry_after_seconds":{value},"max_rpm":1200}} retry in 1 second"#);
        assert_eq!(
            classify_failure(&message, None),
            FailureKind::RateLimited { retry_after: None }
        );
    }
}

#[test]
fn day_hints_do_not_become_short_retries() {
    let now = Instant::now();
    for message in [
        "429 rate limit, retry after 2 days",
        "429 rate limit, retry in 1d",
        r#"429: {"retry_after_seconds":1e100}"#,
    ] {
        let mut retry = TurnRetry::default();
        assert!(
            matches!(
                retry.observe(&error(message), now),
                RetryDecision::GaveUp { .. }
            ),
            "{message}"
        );
        assert_eq!(retry.pending(), None);
    }
}

#[test]
fn failover_consent_and_invalid_requests_override_transient_words() {
    for message in [
        r#"[jcode-provider-failover]{"reason":"503 temporarily unavailable"}"#,
        "unsupported_value: server error for invalid option",
        "invalid tool schema, try again",
        "body too large: network request rejected",
        "refresh token has already been used, connection closed",
        "model does not support the coding plan: 503",
    ] {
        assert_eq!(
            classify_failure(message, None),
            FailureKind::Permanent,
            "{message}"
        );
    }
}

#[test]
fn cancellation_stays_cancelled_when_late_errors_arrive() {
    let now = Instant::now();
    for status in ["cancelled", "canceled", "interrupted", "crashed"] {
        let mut retry = TurnRetry::default();
        retry.observe(&error("timed out"), now);
        assert_eq!(
            retry.observe(
                &ApiEvent::SessionStatus {
                    session_id: "s".into(),
                    status: status.into(),
                    pending_soft_interrupts: None,
                },
                now
            ),
            RetryDecision::Cancelled
        );
        retry.observe(&done(), now);
        assert!(matches!(
            retry.observe(&error("connection reset"), now),
            RetryDecision::GaveUp {
                exhausted: false,
                ..
            }
        ));
        assert!(!retry.take_due(now + Duration::from_secs(60)), "{status}");
        retry.user_sent();
        assert!(scheduled(&retry.observe(&error("timed out"), now)).is_some());
    }
}

#[test]
fn terminal_stop_drops_existing_retry_without_needing_an_error() {
    let now = Instant::now();
    for reason in [
        TurnStopReason::Interrupted,
        TurnStopReason::Crash,
        TurnStopReason::ProviderGuardrail,
        TurnStopReason::LimitReached,
    ] {
        let mut retry = TurnRetry::default();
        retry.observe(&error("timed out"), now);
        assert_eq!(
            retry.observe(&stopped(reason), now),
            RetryDecision::Cancelled
        );
        assert!(!retry.take_due(now + Duration::from_secs(60)));
    }
}

#[test]
fn permanent_error_disarms_previously_scheduled_retry() {
    let now = Instant::now();
    let mut retry = TurnRetry::default();
    retry.observe(&error("timed out"), now);
    retry.observe(&error("401 Unauthorized"), now);
    assert_eq!(retry.pending(), None);
    assert!(!retry.take_due(now + Duration::from_secs(60)));
}
