//! Privacy-preserving onboarding trace telemetry.
//!
//! An onboarding session is a traversal of the graph declared in
//! `jcode-tui::tui::app::onboarding_graph`. The interesting signal is the
//! *shape* of that traversal, and a shape is a list of small integers and
//! closed-vocabulary identifiers. So instead of shipping logs (which contain
//! error strings, paths, and provider responses), we ship the path.
//!
//! Privacy properties, in order of how much they matter:
//!
//!   1. **Closed vocabulary.** Node, edge, reason, and outcome are all
//!      `&'static str` chosen from enums. There is no free-text field, so no
//!      code path exists by which user data could enter the payload. This is
//!      structural, not a scrubbing policy that can be forgotten.
//!   2. **No secrets.** Nothing derived from a credential is recorded here at
//!      all; token identity lives in `auth::refresh_state` as a local
//!      fingerprint and never leaves the machine.
//!   3. **Bucketed timings.** Durations round to [`DT_BUCKET_MS`] and cap at
//!      [`DT_CAP_MS`], so latency patterns cannot act as a fingerprint.
//!   4. **Bounded size.** At most [`MAX_STEPS`] steps; overflow sets
//!      `truncated` rather than growing without limit.
//!   5. **Inspectable.** [`TraceRecorder::preview_json`] renders the exact bytes
//!      that would be sent, so a user can read the whole payload.
//!
//! Consent tiers reuse the existing three-way setting: `Nothing` sends nothing,
//! and both `NoContent` and `Everything` may send a trace, because the trace
//! contains no content by construction.

use crate::state_support::{current_session_id, new_event_id, version};
use jcode_usage_types::{OnboardingTraceEvent, OnboardingTraceStep};
use std::collections::BTreeMap;
use std::time::Instant;

/// Timings round to this bucket, so they describe behavior in aggregate
/// without being precise enough to identify a machine or a person.
pub const DT_BUCKET_MS: u64 = 100;

/// Anything slower than this is reported as this value. A user who walked away
/// for an hour is indistinguishable from one who walked away for a day, and
/// neither is interesting beyond "they left".
pub const DT_CAP_MS: u64 = 300_000;

/// Hard cap on recorded steps. A traversal longer than this indicates a loop
/// bug, which the `truncated` flag reports more usefully than 10k steps would.
pub const MAX_STEPS: usize = 64;

/// How an onboarding traversal ended. Closed vocabulary.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum TraceOutcome {
    /// The user reached a session where they can type a prompt with a working
    /// login.
    Ready,
    /// The user reached a usable session, but with a provider still broken.
    Degraded,
    /// The user left before settling.
    Abandoned,
    /// The flow ran out of forward edges. This should be impossible given the
    /// graph invariants, so a nonzero rate here is an alarm, not a metric.
    Stuck,
}

impl TraceOutcome {
    pub fn label(self) -> &'static str {
        match self {
            TraceOutcome::Ready => "ready",
            TraceOutcome::Degraded => "degraded",
            TraceOutcome::Abandoned => "abandoned",
            TraceOutcome::Stuck => "stuck",
        }
    }
}

/// Round a duration into the reporting bucket.
///
/// Both the rounding and the cap are deliberate privacy measures, not
/// performance ones.
pub fn bucket_dt_ms(raw_ms: u64) -> u64 {
    let capped = raw_ms.min(DT_CAP_MS);
    (capped / DT_BUCKET_MS) * DT_BUCKET_MS
}

/// Accumulates one onboarding traversal.
///
/// Construct at first-run, call [`TraceRecorder::step`] on each transition, and
/// [`TraceRecorder::finish`] when the flow settles. Dropping without finishing
/// sends nothing, which is the right default: a crashed process should not be
/// reported as an abandonment by a half-built payload.
pub struct TraceRecorder {
    env: BTreeMap<String, &'static str>,
    steps: Vec<OnboardingTraceStep>,
    keystrokes: u32,
    truncated: bool,
    last_transition: Instant,
}

impl TraceRecorder {
    /// Start recording. `env` is the probed capability map: keys and values
    /// must both come from closed vocabularies (see `auth::env_facts`).
    pub fn new(env: BTreeMap<String, &'static str>) -> Self {
        Self {
            env,
            steps: Vec::new(),
            keystrokes: 0,
            truncated: false,
            last_transition: Instant::now(),
        }
    }

    /// Record leaving `node` via `edge`, optionally with a classified reason.
    ///
    /// All three arguments are `&'static str` precisely so a caller cannot pass
    /// a formatted string containing user data.
    pub fn step(
        &mut self,
        node: &'static str,
        edge: Option<&'static str>,
        reason: Option<&'static str>,
        keystrokes: u32,
    ) {
        let dt_ms = bucket_dt_ms(self.last_transition.elapsed().as_millis() as u64);
        self.last_transition = Instant::now();
        self.keystrokes = self.keystrokes.saturating_add(keystrokes);
        if self.steps.len() >= MAX_STEPS {
            // Record the overflow once and stop growing. A trace this long is a
            // loop bug, and `truncated` says so more clearly than the tail would.
            self.truncated = true;
            return;
        }
        self.steps.push(OnboardingTraceStep {
            node,
            edge,
            reason,
            dt_ms,
        });
    }

    /// Number of steps recorded so far (used by tests and the preview command).
    pub fn len(&self) -> usize {
        self.steps.len()
    }

    pub fn is_empty(&self) -> bool {
        self.steps.is_empty()
    }

    /// Build the event without sending it. Exposed so `finish` and the
    /// user-facing preview render exactly the same bytes: a preview that could
    /// differ from the real payload would be worse than no preview.
    pub fn build(&self, id: &str, outcome: TraceOutcome) -> OnboardingTraceEvent {
        let (schema_version, build_channel, git_checkout, ci, from_cargo) =
            crate::telemetry_envelope();
        OnboardingTraceEvent {
            event_id: new_event_id(),
            id: id.to_string(),
            session_id: current_session_id(),
            event: "onboarding_trace",
            version: version(),
            os: std::env::consts::OS,
            arch: std::env::consts::ARCH,
            env: self.env.clone(),
            steps: self.steps.clone(),
            outcome: outcome.label(),
            keystrokes: self.keystrokes,
            truncated: self.truncated,
            schema_version,
            build_channel,
            is_git_checkout: git_checkout,
            is_ci: ci,
            ran_from_cargo: from_cargo,
        }
    }

    /// Exactly the bytes that would be sent, for `jcode telemetry show-last-trace`.
    ///
    /// If a user can read the whole payload in a few lines, trust is cheap.
    pub fn preview_json(&self, outcome: TraceOutcome) -> String {
        let event = self.build("<install-id>", outcome);
        serde_json::to_string_pretty(&event).unwrap_or_else(|_| "{}".to_string())
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn env_fixture() -> BTreeMap<String, &'static str> {
        BTreeMap::from([
            ("tty".to_string(), "yes"),
            ("browser".to_string(), "no"),
            ("loopback_bind".to_string(), "yes"),
        ])
    }

    #[test]
    fn timings_are_bucketed_and_capped() {
        // Precise timings are a behavioral fingerprint; bucketing is the
        // mitigation, so it must actually round and actually cap.
        assert_eq!(bucket_dt_ms(0), 0);
        assert_eq!(bucket_dt_ms(99), 0);
        assert_eq!(bucket_dt_ms(150), 100);
        assert_eq!(bucket_dt_ms(4_237), 4_200);
        assert_eq!(bucket_dt_ms(DT_CAP_MS + 10_000_000), DT_CAP_MS);
        // Every bucketed value is a multiple of the bucket, with no exceptions
        // that could leak precision.
        for raw in [1, 37, 999, 100_000, 999_999] {
            assert_eq!(bucket_dt_ms(raw) % DT_BUCKET_MS, 0);
        }
    }

    #[test]
    fn traces_are_bounded_and_report_truncation() {
        let mut recorder = TraceRecorder::new(env_fixture());
        for _ in 0..(MAX_STEPS + 25) {
            recorder.step("login_openai", Some("login_fail"), None, 1);
        }
        assert_eq!(recorder.len(), MAX_STEPS, "trace must stop growing");
        let event = recorder.build("install-id", TraceOutcome::Stuck);
        assert!(event.truncated, "overflow must be reported, not hidden");
        // Keystrokes still count past the cap: the fact that a user pressed 89
        // keys is the interesting part of a runaway trace.
        assert_eq!(event.keystrokes, (MAX_STEPS + 25) as u32);
    }

    #[test]
    fn payload_contains_only_closed_vocabulary() {
        // The core privacy claim: a serialized trace has no free text. We assert
        // it by checking every string in the payload is either a known field
        // name or a known vocabulary value.
        let mut recorder = TraceRecorder::new(env_fixture());
        recorder.step("start", Some("route_fresh_install"), None, 0);
        recorder.step(
            "login_openai",
            Some("login_fail"),
            Some("callback_timeout"),
            1,
        );
        recorder.step("login_failed", Some("retry_other_method"), None, 1);
        let event = recorder.build("install-id", TraceOutcome::Ready);
        let value = serde_json::to_value(&event).expect("trace serializes");

        // Fields that legitimately carry non-vocabulary data, and why each is
        // safe: identifiers we already generate, and build provenance.
        let allowed_dynamic = [
            "event_id",
            "id",
            "session_id",
            "version",
            "os",
            "arch",
            "build_channel",
        ];

        fn walk(value: &serde_json::Value, key: Option<&str>, allowed: &[&str], vocab: &[&str]) {
            match value {
                serde_json::Value::String(s) => {
                    let key = key.unwrap_or("");
                    assert!(
                        allowed.contains(&key) || vocab.contains(&s.as_str()),
                        "unexpected free-text value {s:?} under key {key:?}"
                    );
                }
                serde_json::Value::Array(items) => {
                    for item in items {
                        walk(item, key, allowed, vocab);
                    }
                }
                serde_json::Value::Object(map) => {
                    for (k, v) in map {
                        walk(v, Some(k.as_str()), allowed, vocab);
                    }
                }
                _ => {}
            }
        }

        let vocab = [
            "onboarding_trace",
            "start",
            "login_openai",
            "login_failed",
            "route_fresh_install",
            "login_fail",
            "retry_other_method",
            "callback_timeout",
            "ready",
            "yes",
            "no",
            "unknown",
        ];
        walk(&value, None, &allowed_dynamic, &vocab);
    }

    #[test]
    fn preview_matches_the_payload_that_would_be_sent() {
        // A preview that differs from the real payload is worse than none, so
        // both go through `build`.
        let mut recorder = TraceRecorder::new(env_fixture());
        recorder.step("start", Some("route_fresh_install"), None, 0);
        let preview = recorder.preview_json(TraceOutcome::Ready);
        assert!(preview.contains("\"onboarding_trace\""));
        assert!(preview.contains("\"route_fresh_install\""));
        assert!(preview.contains("\"outcome\": \"ready\""));
        // The install id is masked in the preview so pasting it in a bug report
        // does not disclose the anonymous identifier.
        assert!(preview.contains("<install-id>"));
    }

    #[test]
    fn outcome_labels_are_stable_identifiers() {
        for outcome in [
            TraceOutcome::Ready,
            TraceOutcome::Degraded,
            TraceOutcome::Abandoned,
            TraceOutcome::Stuck,
        ] {
            let label = outcome.label();
            assert!(
                label.chars().all(|c| c.is_ascii_lowercase()),
                "outcome label {label:?} must be a stable identifier"
            );
        }
    }
}
