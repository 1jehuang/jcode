//! Resume server-initiated turns that stopped on a subscription usage limit.
//!
//! Scheduled tasks, swarm worker turns, DM wakes and reload continuations run
//! without a client that could hold and resend the turn. When the provider
//! reports that the account's usage limit resets later (the Anthropic runtime
//! fails fast when the reset is more than two minutes away), the turn is not
//! broken: it has to wait for the reset. This module decides whether and when
//! to resume, and keeps at most one pending resume per session.
//!
//! Policy (shared with `jcode_provider_core::usage_limit_resume`):
//! - Resume at the reported reset plus 5-30 s of jitter, never sooner. When
//!   the error names several resets (multi-account), the earliest wins.
//! - A reset in the past waits 60 s, an unknown reset waits 15 min. Never zero.
//! - At most [`MAX_USAGE_LIMIT_RESUMES`] resumes per turn, then the turn fails.
//! - A user message to the session cancels its pending resume.

use super::live_turn::LiveTurnSwarmContext;
use super::update_member_status;
use crate::agent::Agent;
use crate::protocol::ServerEvent;
use chrono::{DateTime, Utc};
use futures::FutureExt;
use futures::future::BoxFuture;
use jcode_provider_core::usage_limit_resume::{
    MAX_USAGE_LIMIT_RESUMES, UsageLimitHint, usage_limit_hint, usage_limit_resume_delay,
    usage_limit_resume_jitter,
};
use std::collections::HashMap;
use std::sync::atomic::{AtomicU64, Ordering};
use std::sync::{Arc, LazyLock, Mutex};
use std::time::Duration;
use tokio::sync::{Mutex as AsyncMutex, OwnedMutexGuard, mpsc};
use tokio_util::sync::CancellationToken;

/// What to do with a server-initiated turn that failed with `error`.
#[derive(Debug, Clone, PartialEq, Eq)]
pub(super) enum UsageLimitDecision {
    /// Not a usage limit: the normal failure path owns it.
    NotUsageLimit,
    /// Wait `delay`, then resume. `attempt` is 1-based.
    Resume(UsageLimitResumePlan),
    /// A usage limit, but the resume budget is spent.
    GiveUp { resumes_used: u32 },
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub(super) struct UsageLimitResumePlan {
    pub delay: Duration,
    pub attempt: u32,
    pub resume_at: DateTime<Utc>,
}

impl UsageLimitResumePlan {
    /// Whole seconds until the resume, for `retry_after_secs`.
    pub fn delay_secs(&self) -> u64 {
        self.delay.as_secs().max(1)
    }

    /// Member status detail, e.g. "usage limit, resuming at 13:30 UTC (1/3)".
    pub fn status_detail(&self) -> String {
        format!(
            "usage limit, resuming at {} UTC ({}/{})",
            self.resume_at.format("%H:%M"),
            self.attempt,
            MAX_USAGE_LIMIT_RESUMES
        )
    }
}

pub(super) fn decide(error: &anyhow::Error, resumes_used: u32) -> UsageLimitDecision {
    decide_with_jitter(error, resumes_used, usage_limit_resume_jitter())
}

pub(super) fn decide_with_jitter(
    error: &anyhow::Error,
    resumes_used: u32,
    jitter: Duration,
) -> UsageLimitDecision {
    let Some(hint) = usage_limit_hint(error) else {
        return UsageLimitDecision::NotUsageLimit;
    };
    plan_for_hint(hint, resumes_used, jitter)
}

fn plan_for_hint(hint: UsageLimitHint, resumes_used: u32, jitter: Duration) -> UsageLimitDecision {
    if resumes_used >= MAX_USAGE_LIMIT_RESUMES {
        return UsageLimitDecision::GiveUp { resumes_used };
    }
    let delay = usage_limit_resume_delay(hint.reset_in, jitter);
    let resume_at = Utc::now()
        + chrono::Duration::from_std(delay).unwrap_or_else(|_| chrono::Duration::hours(24));
    UsageLimitDecision::Resume(UsageLimitResumePlan {
        delay,
        attempt: resumes_used + 1,
        resume_at,
    })
}

/// Message for a turn that gave up after spending its resume budget.
pub(super) fn give_up_message(error: &anyhow::Error, resumes_used: u32) -> String {
    format!(
        "Usage limit still reached after {} automatic resume{}; giving up. {}",
        resumes_used,
        if resumes_used == 1 { "" } else { "s" },
        crate::util::format_error_chain(error)
    )
}

/// `retry_after_secs` for a failed turn's `ServerEvent::Error`: the stream's
/// own hint, else the usage-limit reset.
pub(super) fn error_retry_after_secs(error: &anyhow::Error) -> Option<u64> {
    error
        .downcast_ref::<jcode_agent_runtime::StreamError>()
        .and_then(|stream_error| stream_error.retry_after_secs)
        .or_else(|| usage_limit_hint(error).and_then(|hint| hint.reset_in_secs()))
}

// ---------------------------------------------------------------------------
// One pending resume per session
// ---------------------------------------------------------------------------

static NEXT_TICKET: AtomicU64 = AtomicU64::new(1);
static PENDING: LazyLock<Mutex<HashMap<String, (u64, CancellationToken)>>> =
    LazyLock::new(|| Mutex::new(HashMap::new()));

/// A registered pending resume. Dropping it unregisters it (if it is still
/// the session's current one).
pub(super) struct ResumeTicket {
    session_id: String,
    id: u64,
    token: CancellationToken,
}

/// Register the pending resume for `session_id`, cancelling any earlier one,
/// so a session never has more than one.
pub(super) fn register_pending_resume(session_id: &str) -> ResumeTicket {
    let id = NEXT_TICKET.fetch_add(1, Ordering::Relaxed);
    let token = CancellationToken::new();
    let previous = PENDING
        .lock()
        .unwrap_or_else(|poisoned| poisoned.into_inner())
        .insert(session_id.to_string(), (id, token.clone()));
    if let Some((_, previous)) = previous {
        crate::logging::info(&format!(
            "Usage-limit resume for {session_id} replaced an earlier pending resume"
        ));
        previous.cancel();
    }
    ResumeTicket {
        session_id: session_id.to_string(),
        id,
        token,
    }
}

/// Cancel the pending resume for `session_id`, if any. Called when the user
/// sends the session a message: their turn supersedes the resume.
pub(super) fn cancel_pending_resume(session_id: &str) -> bool {
    let removed = PENDING
        .lock()
        .unwrap_or_else(|poisoned| poisoned.into_inner())
        .remove(session_id);
    match removed {
        Some((_, token)) => {
            crate::logging::info(&format!(
                "Cancelled pending usage-limit resume for {session_id}: a new message arrived"
            ));
            token.cancel();
            true
        }
        None => false,
    }
}

pub(super) fn has_pending_resume(session_id: &str) -> bool {
    PENDING
        .lock()
        .unwrap_or_else(|poisoned| poisoned.into_inner())
        .contains_key(session_id)
}

impl ResumeTicket {
    /// Sleep `delay`. Returns false when the resume was cancelled or replaced.
    pub(super) async fn wait(&self, delay: Duration) -> bool {
        tokio::select! {
            _ = self.token.cancelled() => false,
            _ = tokio::time::sleep(delay) => !self.token.is_cancelled(),
        }
    }
}

impl Drop for ResumeTicket {
    fn drop(&mut self) {
        let mut pending = PENDING
            .lock()
            .unwrap_or_else(|poisoned| poisoned.into_inner());
        if pending
            .get(&self.session_id)
            .is_some_and(|(id, _)| *id == self.id)
        {
            pending.remove(&self.session_id);
        }
    }
}

// ---------------------------------------------------------------------------
// Resume driver shared by every server-initiated turn runner
// ---------------------------------------------------------------------------

/// Called while a turn waits for its reset, e.g. to keep a swarm task's plan
/// progress in step. Receives the plan of the scheduled resume.
pub(super) type OnUsageLimitWait<'a> =
    &'a (dyn Fn(&UsageLimitResumePlan) -> BoxFuture<'static, ()> + Send + Sync);

/// No-op [`OnUsageLimitWait`].
pub(super) fn no_wait_hook(_: &UsageLimitResumePlan) -> BoxFuture<'static, ()> {
    Box::pin(async {})
}

/// How a failed server-initiated turn ended after
/// [`resume_turn_after_usage_limit`].
pub(super) enum ResumeOutcome {
    /// The error was not a usage limit. The caller's failure path owns it.
    NotUsageLimit {
        error: anyhow::Error,
        guard: Option<OwnedMutexGuard<Agent>>,
    },
    /// A resume completed the turn. `guard` keeps the agent reserved until
    /// the caller publishes the terminal status.
    Completed {
        guard: OwnedMutexGuard<Agent>,
        completion_report: Option<String>,
    },
    /// The turn failed for good: the resume budget is spent, or a resume hit
    /// another error. `retry_after_secs` is `None` when nothing will retry.
    Failed {
        error: anyhow::Error,
        retry_after_secs: Option<u64>,
        guard: Option<OwnedMutexGuard<Agent>>,
    },
    /// A user message (or another turn) took the session over while the turn
    /// waited. Nothing more to publish for this turn.
    Superseded,
}

/// Drive a server-initiated turn that failed with `error` through the
/// usage-limit policy: wait for the reset (member shown as `rate_limited`,
/// clients told when the server resumes), continue the turn from the stored
/// transcript, and repeat until it completes, fails otherwise, or spends
/// [`MAX_USAGE_LIMIT_RESUMES`]. `held` is the caller's reservation of the
/// agent; it is released while waiting so the user can use the session.
#[expect(
    clippy::too_many_arguments,
    reason = "the resume needs the agent, its turn inputs, and the swarm status sinks"
)]
pub(super) async fn resume_turn_after_usage_limit(
    agent: Arc<AsyncMutex<Agent>>,
    held: Option<OwnedMutexGuard<Agent>>,
    session_id: &str,
    error: anyhow::Error,
    system_reminder: Option<String>,
    event_tx: &mpsc::UnboundedSender<ServerEvent>,
    swarm: &LiveTurnSwarmContext,
    on_wait: OnUsageLimitWait<'_>,
) -> ResumeOutcome {
    let mut error = error;
    let mut held = held;
    let mut resumes_used = 0u32;
    loop {
        let plan = match decide(&error, resumes_used) {
            UsageLimitDecision::NotUsageLimit if resumes_used == 0 => {
                return ResumeOutcome::NotUsageLimit { error, guard: held };
            }
            UsageLimitDecision::NotUsageLimit => {
                let retry_after_secs = error_retry_after_secs(&error);
                return ResumeOutcome::Failed {
                    error,
                    retry_after_secs,
                    guard: held,
                };
            }
            UsageLimitDecision::GiveUp { resumes_used } => {
                crate::logging::warn(&format!(
                    "Usage-limit resume budget spent for {session_id} after {resumes_used} resumes: {error}"
                ));
                return ResumeOutcome::Failed {
                    error: anyhow::anyhow!(give_up_message(&error, resumes_used)),
                    retry_after_secs: None,
                    guard: held,
                };
            }
            UsageLimitDecision::Resume(plan) => plan,
        };

        crate::logging::warn(&format!(
            "Server-initiated turn for {session_id} hit a usage limit; resuming in {}s at {} (attempt {}/{}): {error}",
            plan.delay.as_secs(),
            plan.resume_at.format("%H:%M:%S UTC"),
            plan.attempt,
            MAX_USAGE_LIMIT_RESUMES
        ));
        let ticket = register_pending_resume(session_id);
        update_member_status(
            session_id,
            "rate_limited",
            Some(plan.status_detail()),
            &swarm.members,
            &swarm.swarms_by_id,
            Some(&swarm.event_history),
            Some(&swarm.event_counter),
            Some(&swarm.event_tx),
        )
        .await;
        on_wait(&plan).await;
        // `server_resumes` tells attached clients the server owns the
        // resume, so they show when it resumes instead of an error and do
        // not schedule a resend of their own. Terminal errors never set it.
        let _ = event_tx.send(ServerEvent::Error {
            id: 0,
            message: crate::util::format_error_chain(&error),
            retry_after_secs: Some(plan.delay_secs()),
            server_resumes: true,
        });
        // Let the user use the session while this turn waits.
        drop(held.take());

        if !ticket.wait(plan.delay).await {
            crate::logging::info(&format!(
                "Usage-limit resume for {session_id} was cancelled or replaced"
            ));
            return ResumeOutcome::Superseded;
        }
        let Ok(mut guard) = Arc::clone(&agent).try_lock_owned() else {
            crate::logging::info(&format!(
                "Usage-limit resume for {session_id} skipped: another turn is running"
            ));
            return ResumeOutcome::Superseded;
        };
        drop(ticket);
        resumes_used += 1;

        update_member_status(
            session_id,
            "running",
            Some(format!(
                "resuming after usage limit ({}/{})",
                plan.attempt, MAX_USAGE_LIMIT_RESUMES
            )),
            &swarm.members,
            &swarm.swarms_by_id,
            Some(&swarm.event_history),
            Some(&swarm.event_counter),
            Some(&swarm.event_tx),
        )
        .await;
        let start_message_index = guard.message_count();
        let result = std::panic::AssertUnwindSafe(
            guard.resume_turn_streaming_mpsc(system_reminder.clone(), event_tx.clone()),
        )
        .catch_unwind()
        .await
        .unwrap_or_else(|_| Err(anyhow::anyhow!("Processing task panicked while resuming")));
        match result {
            Ok(()) => {
                let completion_report = guard.latest_assistant_text_after(start_message_index);
                return ResumeOutcome::Completed {
                    guard,
                    completion_report,
                };
            }
            Err(next) => {
                error = next;
                held = Some(guard);
            }
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use jcode_provider_core::usage_limit_resume::{
        PAST_RESET_RESUME_DELAY, UNKNOWN_RESET_RESUME_DELAY, with_usage_limit_reset,
    };

    fn limit(reset_in: Option<Duration>) -> anyhow::Error {
        with_usage_limit_reset(
            anyhow::anyhow!(
                "Anthropic API error (429 Too Many Requests): rate_limit_error Usage limit reached for this Claude account."
            ),
            reset_in,
        )
    }

    #[test]
    fn resumes_at_the_reset_plus_jitter_and_counts_attempts() {
        let jitter = Duration::from_secs(7);
        let decision = decide_with_jitter(&limit(Some(Duration::from_secs(1200))), 0, jitter);
        let UsageLimitDecision::Resume(plan) = decision else {
            panic!("expected a resume, got {decision:?}");
        };
        assert!(plan.delay >= Duration::from_secs(1205) && plan.delay <= Duration::from_secs(1207));
        assert_eq!(plan.attempt, 1);
        assert!(
            plan.status_detail()
                .starts_with("usage limit, resuming at ")
        );
        assert!(plan.status_detail().ends_with("UTC (1/3)"));
    }

    #[test]
    fn past_or_missing_reset_never_resumes_immediately() {
        let past = decide_with_jitter(&limit(Some(Duration::ZERO)), 0, Duration::ZERO);
        let missing = decide_with_jitter(&limit(None), 0, Duration::ZERO);
        match (past, missing) {
            (UsageLimitDecision::Resume(past), UsageLimitDecision::Resume(missing)) => {
                assert_eq!(past.delay, PAST_RESET_RESUME_DELAY);
                assert_eq!(missing.delay, UNKNOWN_RESET_RESUME_DELAY);
            }
            other => panic!("expected resumes, got {other:?}"),
        }
    }

    #[test]
    fn gives_up_after_the_budget_and_ignores_other_errors() {
        assert_eq!(
            decide_with_jitter(&limit(Some(Duration::from_secs(5))), 3, Duration::ZERO),
            UsageLimitDecision::GiveUp { resumes_used: 3 }
        );
        assert_eq!(
            decide_with_jitter(
                &anyhow::anyhow!("Anthropic API error (500): overloaded"),
                0,
                Duration::ZERO
            ),
            UsageLimitDecision::NotUsageLimit
        );
    }

    #[test]
    fn error_retry_after_prefers_stream_hint_then_usage_reset() {
        let stream: anyhow::Error =
            jcode_agent_runtime::StreamError::new("rate limited".into(), Some(42)).into();
        assert_eq!(error_retry_after_secs(&stream), Some(42));
        let secs = error_retry_after_secs(&limit(Some(Duration::from_secs(600)))).unwrap();
        assert!((599..=600).contains(&secs), "{secs}");
        assert_eq!(error_retry_after_secs(&anyhow::anyhow!("boom")), None);
    }

    #[tokio::test(start_paused = true)]
    async fn only_one_pending_resume_per_session() {
        let session = "usage-limit-resume-unit-one-pending";
        let first = register_pending_resume(session);
        let second = register_pending_resume(session);
        assert!(
            !first.wait(Duration::from_secs(60)).await,
            "the earlier resume is replaced"
        );
        assert!(has_pending_resume(session));
        drop(first);
        assert!(
            has_pending_resume(session),
            "dropping a replaced ticket keeps the new one"
        );
        assert!(second.wait(Duration::from_secs(60)).await);
        drop(second);
        assert!(!has_pending_resume(session));
    }

    #[tokio::test(start_paused = true)]
    async fn cancel_stops_the_pending_resume() {
        let session = "usage-limit-resume-unit-cancel";
        let ticket = register_pending_resume(session);
        assert!(cancel_pending_resume(session));
        assert!(!ticket.wait(Duration::from_secs(60)).await);
        assert!(!cancel_pending_resume(session));
    }
}
