//! Stalled swarm worker watchdog (issue #1703).
//!
//! A worker can sit in `running` while making no progress (a hung provider
//! stream, a turn that never settled), leaving its coordinator waiting with
//! nothing to tell it something is wrong. This module periodically checks
//! every running worker's last observed activity (streamed tokens, tool
//! start/finish, swarm task heartbeats, all folded into
//! [`crate::session_metrics`]) and whether a tool is currently executing
//! ([`crate::running_tool_registry`]). A worker that has been silent for
//! `agents.swarm_stall_after_secs` with no tool in flight is flagged as
//! stalled: `swarm list` / `swarm status` show it, and the worker's owner (or
//! the swarm coordinator) gets one notification per stall episode. With
//! `agents.swarm_stall_nudge` the worker also receives a short nudge.
//!
//! A long-running tool (a build, a test run) never counts as a stall, since
//! silence while a tool executes is expected.
//!
//! Stall state lives in a process-global registry rather than on
//! `SwarmMember`, so it never touches swarm persistence and stays cheap to
//! query from status paths.

use super::state::fanout_session_event;
use super::{SessionAgents, SessionInterruptQueues, SwarmMember, queue_soft_interrupt_for_session};
use crate::protocol::{NotificationType, ServerEvent};
use jcode_agent_runtime::SoftInterruptSource;
use std::collections::HashMap;
use std::sync::Arc;
use std::sync::{LazyLock, Mutex as StdMutex};
use std::time::{Duration, Instant};
use tokio::sync::RwLock;

/// Upper bound on how often the watchdog sweeps.
const MAX_SWEEP_INTERVAL: Duration = Duration::from_secs(60);
/// Lower bound so tiny test thresholds do not spin.
const MIN_SWEEP_INTERVAL: Duration = Duration::from_secs(5);

#[derive(Debug, Clone, Copy)]
struct StallState {
    /// Silence observed when the stall was first detected, plus the instant it
    /// was detected, so later readers can report a growing age.
    silent_secs_at_detection: u64,
    detected_at: Instant,
}

static STALLED: LazyLock<StdMutex<HashMap<String, StallState>>> =
    LazyLock::new(|| StdMutex::new(HashMap::new()));

/// Configured stall window, or `None` when the watchdog is disabled.
pub(super) fn stall_after() -> Option<Duration> {
    let secs = crate::config::config().agents.swarm_stall_after_secs;
    (secs > 0).then(|| Duration::from_secs(secs))
}

/// Sweep cadence for a given stall window: a quarter of the window, clamped.
pub(super) fn sweep_interval(stall_after: Duration) -> Duration {
    (stall_after / 4).clamp(MIN_SWEEP_INTERVAL, MAX_SWEEP_INTERVAL)
}

/// Seconds the session has been silent while flagged stalled, or `None` when
/// the watchdog does not currently consider it stalled.
pub(super) fn stalled_for_secs(session_id: &str) -> Option<u64> {
    STALLED.lock().ok().and_then(|map| {
        map.get(session_id).map(|state| {
            state
                .silent_secs_at_detection
                .saturating_add(state.detected_at.elapsed().as_secs())
        })
    })
}

/// Facts about one member the stall decision depends on.
#[derive(Debug, Clone, Copy)]
pub(super) struct StallInputs<'a> {
    pub status: &'a str,
    pub role: &'a str,
    /// Seconds since last recorded activity, if any was ever recorded.
    pub activity_age_secs: Option<u64>,
    /// Seconds since the last lifecycle status change.
    pub status_age_secs: u64,
    pub tool_running: bool,
}

/// Silence (in seconds) that makes this member stalled, or `None` if it is
/// not stalled. Pure so it can be unit tested without timers.
pub(super) fn evaluate_stall(inputs: StallInputs<'_>, stall_after: Duration) -> Option<u64> {
    if inputs.status != "running" || inputs.role == "coordinator" || inputs.tool_running {
        return None;
    }
    // Entering `running` is itself activity: a stale activity clock from a
    // previous turn must not flag a turn that only just started.
    let silent = match inputs.activity_age_secs {
        Some(age) => age.min(inputs.status_age_secs),
        None => inputs.status_age_secs,
    };
    (silent >= stall_after.as_secs()).then_some(silent)
}

/// A newly detected stall the sweep must act on.
#[derive(Debug, Clone, PartialEq, Eq)]
pub(super) struct NewStall {
    pub session_id: String,
    pub label: String,
    pub recipient_session_id: Option<String>,
    pub silent_secs: u64,
}

/// Recompute the stalled set from a member snapshot and return members that
/// became stalled since the previous sweep. Members that recovered (activity
/// resumed, a tool started, the status changed) are cleared so a later stall
/// notifies again.
pub(super) fn update_stalls(
    members: &HashMap<String, SwarmMember>,
    coordinators: &HashMap<String, String>,
    stall_after: Duration,
    activity_age: impl Fn(&str) -> Option<u64>,
    tool_running: impl Fn(&str) -> bool,
) -> Vec<NewStall> {
    let mut now_stalled: HashMap<String, u64> = HashMap::new();
    for member in members.values() {
        let inputs = StallInputs {
            status: &member.status,
            role: &member.role,
            activity_age_secs: activity_age(&member.session_id),
            status_age_secs: member.last_status_change.elapsed().as_secs(),
            tool_running: tool_running(&member.session_id),
        };
        if let Some(silent) = evaluate_stall(inputs, stall_after) {
            now_stalled.insert(member.session_id.clone(), silent);
        }
    }

    let Ok(mut registry) = STALLED.lock() else {
        return Vec::new();
    };
    registry.retain(|session_id, _| now_stalled.contains_key(session_id));

    let mut fresh = Vec::new();
    for (session_id, silent) in now_stalled {
        if registry.contains_key(&session_id) {
            continue;
        }
        registry.insert(
            session_id.clone(),
            StallState {
                silent_secs_at_detection: silent,
                detected_at: Instant::now(),
            },
        );
        let Some(member) = members.get(&session_id) else {
            continue;
        };
        let recipient_session_id = member
            .report_back_to_session_id
            .clone()
            .or_else(|| {
                member
                    .swarm_id
                    .as_ref()
                    .and_then(|swarm_id| coordinators.get(swarm_id).cloned())
            })
            .filter(|recipient| recipient != &session_id);
        let label = member
            .friendly_name
            .clone()
            .unwrap_or_else(|| session_id[..8.min(session_id.len())].to_string());
        fresh.push(NewStall {
            session_id,
            label,
            recipient_session_id,
            silent_secs: silent,
        });
    }
    fresh.sort_by(|a, b| a.session_id.cmp(&b.session_id));
    fresh
}

fn format_duration(secs: u64) -> String {
    if secs >= 3600 {
        format!("{}h{}m", secs / 3600, (secs % 3600) / 60)
    } else if secs >= 60 {
        format!("{}m", secs / 60)
    } else {
        format!("{}s", secs)
    }
}

pub(super) fn coordinator_message(stall: &NewStall) -> String {
    format!(
        "⚠ Worker {} looks stalled: status is running but it has shown no activity (no streamed tokens, tool calls, or heartbeats) for {} and no tool is executing. Check it with `swarm status`, message it, or use `swarm retry` / `swarm replace`.",
        stall.label,
        format_duration(stall.silent_secs)
    )
}

pub(super) fn nudge_message(silent_secs: u64) -> String {
    format!(
        "[swarm watchdog] No progress from this session for {}. If you are blocked, report it with `swarm report` (status blocked). Otherwise continue your assigned task.",
        format_duration(silent_secs)
    )
}

/// One watchdog pass: detect new stalls, notify their owner once, and
/// optionally nudge the worker.
pub(super) async fn sweep_stalled_workers(
    swarm_members: &Arc<RwLock<HashMap<String, SwarmMember>>>,
    swarm_coordinators: &Arc<RwLock<HashMap<String, String>>>,
    sessions: &SessionAgents,
    soft_interrupt_queues: &SessionInterruptQueues,
) {
    let Some(stall_after) = stall_after() else {
        if let Ok(mut registry) = STALLED.lock() {
            registry.clear();
        }
        return;
    };
    let fresh = {
        let members = swarm_members.read().await;
        let coordinators = swarm_coordinators.read().await;
        update_stalls(
            &members,
            &coordinators,
            stall_after,
            crate::session_metrics::last_activity_age_secs,
            |sid| crate::running_tool_registry::current(sid).is_some(),
        )
    };
    let nudge = crate::config::config().agents.swarm_stall_nudge;
    for stall in fresh {
        crate::logging::warn(&format!(
            "Swarm watchdog: worker {} ({}) stalled, silent for {}s",
            stall.label, stall.session_id, stall.silent_secs
        ));
        if let Some(recipient) = stall.recipient_session_id.as_deref() {
            let _ = fanout_session_event(
                swarm_members,
                recipient,
                ServerEvent::Notification {
                    from_session: stall.session_id.clone(),
                    from_name: Some(stall.label.clone()),
                    notification_type: NotificationType::Message {
                        scope: Some("swarm".to_string()),
                        channel: None,
                        tldr: Some(format!("{} stalled", stall.label)),
                    },
                    message: coordinator_message(&stall),
                },
            )
            .await;
        }
        if nudge {
            let _ = queue_soft_interrupt_for_session(
                &stall.session_id,
                nudge_message(stall.silent_secs),
                false,
                SoftInterruptSource::System,
                soft_interrupt_queues,
                sessions,
            )
            .await;
        }
    }
}

#[cfg(test)]
pub(super) fn clear_for_tests(session_ids: &[&str]) {
    if let Ok(mut registry) = STALLED.lock() {
        for sid in session_ids {
            registry.remove(*sid);
        }
    }
}

#[cfg(test)]
#[path = "swarm_watchdog_tests.rs"]
mod tests;
