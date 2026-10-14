//! Automatic recovery for model turns that fail transiently.
//!
//! The runtime already retries a single provider request (429/5xx/network
//! errors with backoff). When those attempts are exhausted the whole turn ends
//! with [`ApiEvent::Error`]. Interactive clients then have to decide whether to
//! resume the work or leave it for the user. This module is that decision,
//! shared so every SDK client (Jcode Desktop, the TUI, embedders) behaves the
//! same way.
//!
//! [`TurnRetry`] is a small I/O-free state machine. Feed it every event of one
//! session with [`TurnRetry::observe`]. When it returns
//! [`RetryDecision::Scheduled`], wait until the returned instant, then call
//! [`TurnRetry::take_due`] and send [`CONTINUATION_REMINDER`] with
//! [`JcodeClient::send_system_reminder`](crate::JcodeClient::send_system_reminder).
//!
//! The failed prompt is already in the session transcript (the server stores
//! the user message before calling the model), so a retry continues the
//! session instead of resending the prompt. Resending it would duplicate the
//! request in history.
//!
//! Only failures that a later attempt can plausibly fix are retried: dropped
//! connections, timeouts, overloaded or unavailable providers, short rate
//! limits. Credentials, billing, quota exhaustion, invalid requests and
//! guardrail refusals fail identically every time, so they are never retried.
//! User cancellation and new user input always cancel a pending retry.

use std::time::{Duration, Instant};

use jcode_harness_api::{ApiEvent, ErrorCode, TurnStopReason};

/// Hidden instruction sent to resume a turn after a transient failure.
pub const CONTINUATION_REMINDER: &str = "[auto] The previous model request failed with a transient error and was retried automatically. Continue the task from where you left off. Do not repeat tool calls or edits that already completed.";

/// Why a failed turn is or is not worth retrying.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum FailureKind {
    /// Network drop, timeout, or a provider-side 5xx/overloaded error.
    Transient,
    /// A rate limit, optionally with the server-provided wait.
    RateLimited { retry_after: Option<Duration> },
    /// Deterministic failure: retrying the same request cannot succeed.
    Permanent,
}

/// Longest rate-limit wait worth holding a turn for. Longer windows (usage
/// caps that reset in hours or days) are reported to the user instead.
pub const MAX_RATE_LIMIT_WAIT: Duration = Duration::from_secs(5 * 60);

/// Classify a turn failure message.
///
/// Deterministic markers win over transient ones: "401 Unauthorized ...
/// connection closed" is a credential failure, not a network drop.
pub fn classify_failure(message: &str, retry_after: Option<Duration>) -> FailureKind {
    let lower = message.to_ascii_lowercase();
    if PERMANENT_MARKERS
        .iter()
        .any(|marker| lower.contains(marker))
    {
        return FailureKind::Permanent;
    }
    if retry_after.is_some()
        || RATE_LIMIT_MARKERS
            .iter()
            .any(|marker| lower.contains(marker))
        || RATE_LIMIT_CODES
            .iter()
            .any(|code| contains_code(&lower, code))
    {
        return FailureKind::RateLimited {
            retry_after: retry_after.or_else(|| parse_retry_after(&lower)),
        };
    }
    if TRANSIENT_MARKERS
        .iter()
        .any(|marker| lower.contains(marker))
        || TRANSIENT_CODES
            .iter()
            .any(|code| contains_code(&lower, code))
    {
        return FailureKind::Transient;
    }
    FailureKind::Permanent
}

/// Credentials, billing, quota, invalid requests, policy refusals, and
/// failures the runtime handles itself (context overflow is auto-compacted).
const PERMANENT_MARKERS: &[&str] = &[
    // Credentials.
    "unauthorized",
    "status 401",
    "error 401",
    "403 forbidden",
    "permission_denied",
    "authentication_error",
    "invalid api key",
    "invalid x-api-key",
    "incorrect api key",
    "api key not valid",
    "token refresh failed",
    "refresh_token_invalidated",
    "invalid_grant",
    "token expired",
    "no refresh token",
    "credentials have been revoked",
    "re-authenticate",
    "please log in again",
    "run /login",
    // Billing and plan limits.
    "insufficient_quota",
    "payment required",
    "billing",
    "credit balance",
    "out of credits",
    "more credits",
    "usage_limit_reached",
    "usage limit",
    "quota exceeded",
    "quota_exceeded",
    // Request shape.
    "400 bad request",
    "invalid_request_error",
    "context length",
    "context_length_exceeded",
    "maximum context length",
    "context limit exceeded",
    "token limit exceeded",
    "request too large",
    "payload too large",
    "string_too_long",
    "string_above_max_length",
    "invalid model",
    "model_not_found",
    "model_not_supported",
    "unsupported model",
    "unsupportedmodel",
    "model does not exist",
    "unsupported parameter",
    "invalid parameter",
    "invalid schema",
    "invalid image",
    "image too large",
    // Policy.
    "content_policy_violation",
    "safety_violation",
    "guardrail",
    // Cancellation.
    "interrupted by user",
    "cancelled",
    "canceled",
];

const RATE_LIMIT_MARKERS: &[&str] = &["rate limit", "rate_limit", "ratelimit", "too many requests"];

/// HTTP statuses matched only as standalone numbers, so a request id such as
/// `req_15030` is not mistaken for a 503.
const RATE_LIMIT_CODES: &[&str] = &["429"];
const TRANSIENT_CODES: &[&str] = &["500", "502", "503", "504", "529"];

fn contains_code(text: &str, code: &str) -> bool {
    text.match_indices(code).any(|(start, _)| {
        let before = text[..start].chars().next_back();
        let after = text[start + code.len()..].chars().next();
        !before.is_some_and(|c| c.is_ascii_alphanumeric())
            && !after.is_some_and(|c| c.is_ascii_alphanumeric())
    })
}

const TRANSIENT_MARKERS: &[&str] = &[
    // Network.
    "connection reset",
    "connection closed",
    "connection refused",
    "connection aborted",
    "connection error",
    "connection lost",
    "broken pipe",
    "network",
    "no route to host",
    "host is down",
    "dns error",
    "failed to lookup address",
    "temporary failure in name resolution",
    "could not resolve host",
    "error trying to connect",
    "client error (connect)",
    "unexpected eof",
    "end of file before message completed",
    "connection closed before message completed",
    "close_notify",
    "peer closed connection",
    "tls handshake eof",
    "incomplete message",
    "error decoding response body",
    "request or response body error",
    "stream ended",
    "stream error",
    "stream closed",
    "stream stalled",
    "no data received",
    // Timeouts.
    "timed out",
    "timeout",
    // Provider side.
    "500 internal server error",
    "internal server error",
    "bad gateway",
    "service unavailable",
    "gateway timeout",
    "overloaded",
    "server_error",
    "server error",
    "api_error",
    "temporarily unavailable",
    "try again",
];

fn parse_retry_after(lower: &str) -> Option<Duration> {
    let tail = &lower[lower.find("retry")?..];
    let mut words = tail.split_whitespace().take(8).peekable();
    while let Some(word) = words.next() {
        let digits: String = word.chars().take_while(char::is_ascii_digit).collect();
        if digits.is_empty() {
            continue;
        }
        let value: u64 = digits.parse().ok()?;
        let unit = match word[digits.len()..].trim_matches(|c: char| !c.is_ascii_alphabetic()) {
            "" => words
                .peek()
                .map(|next| {
                    next.trim_matches(|c: char| !c.is_ascii_alphabetic())
                        .to_string()
                })
                .unwrap_or_default(),
            unit => unit.to_string(),
        };
        let secs = match unit.as_str() {
            "ms" | "milliseconds" => return Some(Duration::from_millis(value)),
            "m" | "min" | "mins" | "minute" | "minutes" => value.saturating_mul(60),
            "h" | "hr" | "hour" | "hours" => value.saturating_mul(3600),
            _ => value,
        };
        return (secs > 0).then(|| Duration::from_secs(secs));
    }
    None
}

/// Longest a retry waits for the failed turn's closing TurnDone.
const CLOSE_GRACE: Duration = Duration::from_secs(10);

/// Tuning for [`TurnRetry`].
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct RetryPolicy {
    /// Automatic retries per user request. Zero disables retrying.
    pub max_attempts: u32,
    /// Delay before the first retry. Later retries double it.
    pub base_delay: Duration,
    /// Upper bound for the exponential delay (not for server rate-limit waits).
    pub max_delay: Duration,
}

impl Default for RetryPolicy {
    fn default() -> Self {
        Self {
            max_attempts: 3,
            base_delay: Duration::from_secs(3),
            max_delay: Duration::from_secs(30),
        }
    }
}

impl RetryPolicy {
    fn delay(&self, attempt: u32, kind: FailureKind) -> Option<Duration> {
        if let FailureKind::RateLimited {
            retry_after: Some(wait),
        } = kind
        {
            return (wait <= MAX_RATE_LIMIT_WAIT).then_some(wait.max(self.base_delay));
        }
        let factor = 1u32 << attempt.saturating_sub(1).min(16);
        Some(self.base_delay.saturating_mul(factor).min(self.max_delay))
    }
}

/// What a client should do after observing an event.
#[derive(Clone, Debug, PartialEq, Eq)]
pub enum RetryDecision {
    /// Nothing changed.
    None,
    /// A retry is scheduled. Show the status and call
    /// [`TurnRetry::take_due`] once `at` has passed.
    Scheduled {
        attempt: u32,
        max_attempts: u32,
        at: Instant,
        delay: Duration,
        reason: String,
    },
    /// The failure will not be retried. `exhausted` is true when retries ran
    /// out, false for failures that are never retried.
    GaveUp { attempts: u32, exhausted: bool },
    /// A pending retry was dropped (user input, cancel, or another client
    /// resumed the session).
    Cancelled,
}

/// Per-session auto-retry state. See the module docs.
#[derive(Clone, Debug, Default)]
pub struct TurnRetry {
    policy: RetryPolicy,
    attempts: u32,
    due: Option<Instant>,
    /// The failed turn was announced by TurnStopped, so the API bridge will
    /// close it with TurnDone. Hold the retry until then, or that late
    /// TurnDone would look like the retried turn succeeding.
    awaiting_close: bool,
    /// The last terminal stop for the running turn, which explains its error.
    stop: Option<TurnStopReason>,
}

impl TurnRetry {
    pub fn new(policy: RetryPolicy) -> Self {
        Self {
            policy,
            ..Self::default()
        }
    }

    pub fn policy(&self) -> RetryPolicy {
        self.policy
    }

    /// The time of the pending retry, if any.
    pub fn pending(&self) -> Option<Instant> {
        self.due
    }

    /// Retries already used for the current user request.
    pub fn attempts(&self) -> u32 {
        self.attempts
    }

    /// The user sent new input. Its request gets a fresh retry budget, and
    /// any retry for the old one is dropped.
    pub fn user_sent(&mut self) -> RetryDecision {
        self.attempts = 0;
        self.stop = None;
        self.awaiting_close = false;
        self.cancel_pending()
    }

    /// The user cancelled, cleared, or rewound the session.
    pub fn user_cancelled(&mut self) -> RetryDecision {
        self.user_sent()
    }

    /// Returns true once, when a scheduled retry is due. The caller should
    /// then send [`CONTINUATION_REMINDER`] as a system reminder.
    pub fn take_due(&mut self, now: Instant) -> bool {
        match self.due {
            // Never hold a retry forever on a close event that was lost.
            Some(at) if now >= at && (!self.awaiting_close || now >= at + CLOSE_GRACE) => {
                self.due = None;
                self.stop = None;
                self.awaiting_close = false;
                true
            }
            _ => false,
        }
    }

    /// The retry could not be sent (for example the connection is down).
    /// Reschedule it rather than losing it.
    pub fn send_failed(&mut self, now: Instant) {
        self.due = Some(now + self.policy.base_delay);
    }

    /// Observe one event from the session's stream.
    pub fn observe(&mut self, event: &ApiEvent, now: Instant) -> RetryDecision {
        match event {
            ApiEvent::TurnStopped { reason, .. } => {
                self.stop = Some(*reason);
                RetryDecision::None
            }
            // A completed turn proves the session works again. The API
            // bridge also closes a failed turn with TurnDone right after its
            // Error. That close neither cancels the retry nor resets the
            // budget. Real new work cancels a retry through MessageAccepted.
            ApiEvent::TurnDone { .. } => {
                if std::mem::take(&mut self.awaiting_close) {
                    return RetryDecision::None;
                }
                if self.due.is_none() {
                    self.attempts = 0;
                }
                self.stop = None;
                RetryDecision::None
            }
            ApiEvent::SessionStatus { status, .. }
                if matches!(status.as_str(), "cancelled" | "canceled") =>
            {
                self.user_cancelled()
            }
            // Another client (or the server) started work in this session.
            ApiEvent::MessageAccepted { .. } if self.due.is_some() => self.cancel_pending(),
            ApiEvent::Error { code, message } => self.observe_error(*code, message, now),
            _ => RetryDecision::None,
        }
    }

    fn observe_error(&mut self, code: ErrorCode, message: &str, now: Instant) -> RetryDecision {
        let stop = self.stop.take();
        // Only failed model turns are retried. Request-level errors (unknown
        // session, invalid request) are answered to the caller, not streamed
        // as turn failures, and a busy rejection is handled by steering.
        if code != ErrorCode::Internal || message.contains("Already processing") {
            return RetryDecision::None;
        }
        match stop {
            Some(TurnStopReason::Failure | TurnStopReason::Unknown) | None => {}
            Some(
                TurnStopReason::Interrupted
                | TurnStopReason::Crash
                | TurnStopReason::ProviderGuardrail
                | TurnStopReason::LimitReached,
            ) => {
                return RetryDecision::GaveUp {
                    attempts: self.attempts,
                    exhausted: false,
                };
            }
        }
        let kind = classify_failure(message, None);
        if kind == FailureKind::Permanent {
            return RetryDecision::GaveUp {
                attempts: self.attempts,
                exhausted: false,
            };
        }
        if self.attempts >= self.policy.max_attempts {
            return RetryDecision::GaveUp {
                attempts: self.attempts,
                exhausted: true,
            };
        }
        let Some(delay) = self.policy.delay(self.attempts + 1, kind) else {
            return RetryDecision::GaveUp {
                attempts: self.attempts,
                exhausted: false,
            };
        };
        self.attempts += 1;
        let at = now + delay;
        self.due = Some(at);
        self.awaiting_close = stop.is_some();
        RetryDecision::Scheduled {
            attempt: self.attempts,
            max_attempts: self.policy.max_attempts,
            at,
            delay,
            reason: summarize(message),
        }
    }

    fn cancel_pending(&mut self) -> RetryDecision {
        if self.due.take().is_some() {
            RetryDecision::Cancelled
        } else {
            RetryDecision::None
        }
    }
}

/// First line of an error, trimmed for a status line.
fn summarize(message: &str) -> String {
    let line = message
        .lines()
        .find(|line| !line.trim().is_empty())
        .unwrap_or("")
        .trim();
    let mut out: String = line.chars().take(120).collect();
    if line.chars().count() > 120 {
        out.push('…');
    }
    out
}

#[cfg(test)]
#[path = "sdk_tests/turn_retry.rs"]
mod tests;
