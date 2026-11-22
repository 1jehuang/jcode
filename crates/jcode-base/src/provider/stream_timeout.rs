//! Provider-agnostic streaming idle-timeout budgets.
//!
//! Every streaming provider path shares these helpers so a slow reasoning model
//! cannot trip a premature timeout on one transport but not another (issue
//! #434). The base budget comes from config; reasoning effort scales it, because
//! high-effort turns can think silently for many minutes before emitting a
//! single event and would otherwise look like a dead connection.

use std::time::Duration;
use tokio::time::Instant;

/// Largest multiplier [`stream_idle_timeout_multiplier_for_effort`] can return.
///
/// Clients that guard against stalls without knowing the request's reasoning
/// effort must budget against this so they never cancel a stream the provider
/// would still happily accept.
pub const MAX_STREAM_IDLE_TIMEOUT_MULTIPLIER: u32 = 4;

/// Base streaming idle timeout: max time to wait between streamed chunks/events
/// before treating the connection as dead.
///
/// Resolved from `[provider] stream_idle_timeout_secs` /
/// `JCODE_STREAM_IDLE_TIMEOUT_SECS` (default 180).
pub fn stream_idle_timeout() -> Duration {
    let secs = crate::config::config()
        .provider
        .stream_idle_timeout_secs
        .max(1);
    Duration::from_secs(secs)
}

/// Hard deadline for an entire assistant turn. Unlike the idle timeout, this
/// clock starts once when the turn begins and is never reset by stream events.
/// It bounds the 'thinking forever behind heartbeats' failure mode.
///
/// Resolved from `[provider] turn_deadline_secs` (default 1800 = 30 minutes).
pub fn turn_deadline() -> Duration {
    let secs = crate::config::config().provider.turn_deadline_secs.max(1);
    Duration::from_secs(secs)
}

/// Fixed deadline for one assistant turn. Created once at turn start; no
/// stream event, heartbeat, or retry can extend it. Callers race the stream
/// against [`TurnDeadline::sleep`] in their select loop.
#[derive(Clone, Copy, Debug)]
pub struct TurnDeadline(Instant);

impl TurnDeadline {
    /// Start a fresh total-turn deadline now.
    pub fn start() -> Self {
        Self(Instant::now() + turn_deadline())
    }

    /// Start a deadline with an explicit budget (used by tests and callers
    /// that need to override the configured default).
    pub fn with_budget(budget: Duration) -> Self {
        Self(Instant::now() + budget)
    }

    /// Remaining time, or zero if the deadline has passed.
    pub fn remaining(self) -> Duration {
        self.0.saturating_duration_since(Instant::now())
    }

    /// Whether the total turn deadline has elapsed.
    pub fn expired(self) -> bool {
        self.remaining().is_zero()
    }

    /// Sleep until the deadline. Re-create this future only when starting a
    /// new turn, never in response to a stream event.
    pub async fn sleep(self) {
        tokio::time::sleep(self.remaining()).await;
    }
}

/// Multiplier applied to the base idle timeout for a given reasoning effort.
///
/// The base budget is tuned for ordinary turns that emit tokens promptly. High
/// efforts are silent for far longer, so they need proportionally more headroom.
pub fn stream_idle_timeout_multiplier_for_effort(effort: Option<&str>) -> u32 {
    match effort
        .map(|effort| effort.trim().to_ascii_lowercase())
        .as_deref()
        .unwrap_or("")
    {
        "high" => 2,
        "xhigh" => 3,
        // `swarm` is a Jcode UI sentinel that resolve to the top
        // wire effort upstream, so budget them like `max`.
        "max" | "swarm" => MAX_STREAM_IDLE_TIMEOUT_MULTIPLIER,
        _ => 1,
    }
}

/// [`stream_idle_timeout`] scaled for the request's reasoning effort.
pub fn stream_idle_timeout_for_effort(effort: Option<&str>) -> Duration {
    stream_idle_timeout() * stream_idle_timeout_multiplier_for_effort(effort)
}

/// [`stream_idle_timeout`] scaled by the maximum effort multiplier.
///
/// Use this where the reasoning effort is unknown but the budget must still
/// outlast any legitimate provider-side wait.
pub fn max_stream_idle_timeout() -> Duration {
    stream_idle_timeout() * MAX_STREAM_IDLE_TIMEOUT_MULTIPLIER
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn turn_deadline_is_independent_of_idle_timeout() {
        // The total deadline must not be derived from the idle timer, or a
        // heartbeat-only stream could extend it forever.
        let total = turn_deadline();
        assert_eq!(total, Duration::from_secs(1800));
        assert!(total > max_stream_idle_timeout());
    }

    #[tokio::test(start_paused = true)]
    async fn heartbeat_only_stream_hits_total_deadline() {
        // A stream emitting only heartbeats keeps resetting the idle timer,
        // but the total deadline is fixed at turn start. Simulate many
        // heartbeat arrivals and verify the deadline instant never moves.
        let budget = Duration::from_secs(300);
        let deadline = TurnDeadline::with_budget(budget);
        let fixed = deadline.remaining();
        for _ in 0..1000 {
            tokio::time::advance(Duration::from_secs(1)).await;
            // Each heartbeat would reset an idle timer here. The total
            // deadline must keep counting down from its original instant.
            assert!(deadline.remaining() <= fixed);
        }
        // After the full budget elapses, the deadline is expired regardless
        // of how many heartbeats arrived.
        tokio::time::advance(Duration::from_secs(1)).await;
        assert!(deadline.expired(), "heartbeats must not extend total deadline");
    }

    #[test]
    fn multiplier_scales_with_reasoning_effort() {
        // Ordinary efforts keep the base budget; the model emits tokens promptly.
        for effort in [
            None,
            Some("none"),
            Some("minimal"),
            Some("low"),
            Some("medium"),
        ] {
            assert_eq!(
                stream_idle_timeout_multiplier_for_effort(effort),
                1,
                "unexpected multiplier for {effort:?}"
            );
        }

        // High efforts think silently for minutes, so they need more headroom.
        assert_eq!(stream_idle_timeout_multiplier_for_effort(Some("high")), 2);
        assert_eq!(stream_idle_timeout_multiplier_for_effort(Some("xhigh")), 3);
        assert_eq!(stream_idle_timeout_multiplier_for_effort(Some("max")), 4);

        // The swarm sentinel resolves to the top wire effort upstream.
        assert_eq!(stream_idle_timeout_multiplier_for_effort(Some("swarm")), 4);

        // Casing and padding come from config/CLI input, so normalize both.
        assert_eq!(stream_idle_timeout_multiplier_for_effort(Some("  MAX ")), 4);

        // An unknown/future effort must fall back to the base budget rather
        // than panicking or silently inheriting a huge multiplier.
        assert_eq!(
            stream_idle_timeout_multiplier_for_effort(Some("ultra-turbo")),
            1
        );

        // No effort may exceed the ceiling clients budget against.
        for effort in [
            "none", "minimal", "low", "medium", "high", "xhigh", "max", "swarm",
        ] {
            assert!(
                stream_idle_timeout_multiplier_for_effort(Some(effort))
                    <= MAX_STREAM_IDLE_TIMEOUT_MULTIPLIER,
                "{effort} exceeds MAX_STREAM_IDLE_TIMEOUT_MULTIPLIER"
            );
        }
    }

    #[test]
    fn effort_scaling_never_shrinks_the_base_budget() {
        let base = stream_idle_timeout();
        assert_eq!(stream_idle_timeout_for_effort(None), base);
        assert!(stream_idle_timeout_for_effort(Some("max")) > base);
        assert_eq!(
            max_stream_idle_timeout(),
            base * MAX_STREAM_IDLE_TIMEOUT_MULTIPLIER
        );
        // The unknown-effort ceiling must cover the largest known effort, or a
        // client stall guard could fire before the provider's own timeout.
        assert!(max_stream_idle_timeout() >= stream_idle_timeout_for_effort(Some("max")));
    }
}
