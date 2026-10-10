//! Turn-end follow-up policy: auto-poke and the todo quality gates.
//!
//! This is the client-side state machine that decides, after a turn ends, what
//! (if anything) to send next: the incomplete-todo poke, the one-shot
//! long-session review, the deferred gate digest, the ownership gate, the
//! completion-confidence and confidence-spike checks, and the final-response
//! handoff. It is a faithful port of the TUI's `schedule_auto_poke_followup`
//! so every client (TUI, Desktop, SDK users) runs the same policy.
//!
//! It is pure: callers fetch a [`TodoSnapshot`] (the SDK's `todo_state`),
//! call [`FollowUpPolicy::decide`], send the returned message, and apply the
//! returned [`TodoEffects`] (the SDK's `ack_todo_follow_up`).

use crate::{
    ConfidenceState, GateObservation, TODO_FINAL_RESPONSE_CONTINUATION_MESSAGE,
    TODO_LONG_SESSION_REVIEW_MESSAGE, TodoGoal, TodoItem, TodoPlan, build_auto_poke_message,
    build_gate_digest, build_todo_completion_continuation_message,
    build_todo_confidence_spike_continuation_message, build_todo_ownership_continuation_message,
    completed_groups_have_sufficient_delivery, completion_confidence_passes, spike_completed_todos,
    todo_status_is_cancelled, todo_status_is_completed,
};
use serde::{Deserialize, Serialize};

/// Gate turns allowed per completion cycle before the policy gives up rather
/// than loop on a gate the model is no longer making progress on.
pub const MAX_GATE_ATTEMPTS: u8 = 5;

/// Everything the policy reads about a session's todo state at turn end.
#[derive(Clone, Debug, Default, PartialEq, Serialize, Deserialize)]
pub struct TodoSnapshot {
    #[serde(default)]
    pub todos: Vec<TodoItem>,
    #[serde(default)]
    pub plan: TodoPlan,
    #[serde(default)]
    pub goals: Vec<TodoGoal>,
    /// Deferred quality-check points recorded by todo writes this turn.
    #[serde(default)]
    pub gate_observations: Vec<GateObservation>,
    /// The private one-shot long-session review is due and not yet delivered.
    #[serde(default)]
    pub long_session_review_due: bool,
}

/// Persistent side effects a decision requires. Apply them whether or not a
/// follow-up was produced: an empty digest still consumes the observation log.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq, Serialize, Deserialize)]
pub struct TodoEffects {
    #[serde(default)]
    pub clear_gate_observations: bool,
    #[serde(default)]
    pub mark_long_session_review_delivered: bool,
}

impl TodoEffects {
    pub fn is_empty(&self) -> bool {
        !self.clear_gate_observations && !self.mark_long_session_review_delivered
    }
}

#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum FollowUpKind {
    IncompleteTodos,
    LongSessionReview,
    GateDigest,
    Ownership,
    CompletionValidation,
    ConfidenceSpike,
    FinalResponse,
}

/// A synthetic continuation to send as the next user-role message.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct FollowUp {
    pub kind: FollowUpKind,
    /// Model-facing text. Recognized by `is_auto_poke_message`.
    pub message: String,
    /// Short user-facing notice describing what happened.
    pub notice: String,
}

#[derive(Clone, Debug, Default, PartialEq, Eq)]
pub struct Decision {
    pub follow_up: Option<FollowUp>,
    /// A user-facing notice with no follow-up (for example, a circuit breaker).
    pub notice: Option<String>,
    pub effects: TodoEffects,
}

impl Decision {
    fn send(kind: FollowUpKind, message: String, notice: impl Into<String>) -> Self {
        Self {
            follow_up: Some(FollowUp {
                kind,
                message,
                notice: notice.into(),
            }),
            ..Self::default()
        }
    }

    fn with_effects(mut self, effects: TodoEffects) -> Self {
        self.effects = effects;
        self
    }
}

/// Per-session follow-up state. Keep one per session and reuse it across turns.
#[derive(Clone, Debug, Default, PartialEq, Eq)]
pub struct FollowUpPolicy {
    armed: bool,
    default_on: bool,
    final_response_requested: bool,
    final_response_fingerprint: Option<String>,
    digest_delivered: bool,
    gate_attempts: u8,
    spike_challenged: bool,
    last_ownership_fingerprint: Option<String>,
    last_poke_fingerprint: Option<String>,
}

pub fn is_incomplete_todo(todo: &TodoItem) -> bool {
    !todo_status_is_completed(&todo.status) && !todo_status_is_cancelled(&todo.status)
}

impl FollowUpPolicy {
    /// `enabled` is the configured default (`features.auto_poke`).
    pub fn new(enabled: bool) -> Self {
        Self {
            armed: enabled,
            default_on: enabled,
            ..Self::default()
        }
    }

    pub fn is_enabled(&self) -> bool {
        self.armed || self.default_on
    }

    /// `/poke on`: arm, restore default-on re-arming, and start a fresh cycle.
    pub fn enable(&mut self) {
        *self = Self::new(true);
    }

    /// `/poke off` or a circuit breaker that must stick for the session.
    pub fn disable(&mut self) {
        *self = Self::new(false);
    }

    /// Forget per-cycle progress without changing whether poking is enabled.
    /// Call when the user cancels or a turn fails.
    pub fn reset_cycle(&mut self) {
        let enabled = self.default_on;
        let armed = self.armed;
        *self = Self::new(enabled);
        self.armed = armed;
    }

    /// Decide the next follow-up after a turn ended. The caller must already
    /// have checked that no user prompt or other follow-up is pending.
    pub fn decide(&mut self, snapshot: &TodoSnapshot) -> Decision {
        let todos = &snapshot.todos;
        let goals = &snapshot.goals;
        // A circuit breaker clears `armed` while every todo is complete. Re-arm
        // for new open work, or one trip silences poking for the session.
        if self.default_on && todos.iter().any(is_incomplete_todo) {
            self.armed = true;
        }
        if !self.armed {
            return Decision::default();
        }

        let fingerprint = serde_json::to_string(&(todos, &snapshot.plan, goals)).ok();
        if self.final_response_fingerprint.is_some() {
            if self.final_response_fingerprint == fingerprint {
                // Neither elapsed time nor a stale observation starts a new
                // cycle; only a todo change does.
                return Decision::default();
            }
            self.final_response_fingerprint = None;
            self.final_response_requested = false;
            self.digest_delivered = false;
            self.gate_attempts = 0;
            self.spike_challenged = false;
            self.last_ownership_fingerprint = None;
        }

        if !todos.is_empty() && snapshot.long_session_review_due {
            return Decision::send(
                FollowUpKind::LongSessionReview,
                TODO_LONG_SESSION_REVIEW_MESSAGE.to_string(),
                "🔍 Rechecking the plan and assessments after extended work...",
            )
            .with_effects(TodoEffects {
                mark_long_session_review_delivered: true,
                ..TodoEffects::default()
            });
        }

        let incomplete: Vec<&TodoItem> = todos.iter().filter(|t| is_incomplete_todo(t)).collect();
        if !incomplete.is_empty() {
            return self.poke_incomplete(&incomplete);
        }

        // Completing or removing the list ends the prior poke cycle.
        self.last_poke_fingerprint = None;
        if todos.is_empty() {
            // Stay armed: a todo-free turn must not disable poking for later
            // turns that do leave work open.
            self.final_response_requested = false;
            self.last_ownership_fingerprint = None;
            return Decision::default();
        }

        // Deferred quality checks land once, at the end of the work.
        let mut effects = TodoEffects::default();
        if !self.digest_delivered && !snapshot.gate_observations.is_empty() {
            effects.clear_gate_observations = true;
            if let Some(digest) =
                build_gate_digest(&snapshot.gate_observations, &snapshot.plan, goals)
            {
                self.digest_delivered = true;
                return Decision::send(
                    FollowUpKind::GateDigest,
                    digest,
                    "🔍 We asked the agent to double-check this turn's weak points.",
                )
                .with_effects(effects);
            }
        }

        let ownership_needs_followup = !completed_groups_have_sufficient_delivery(todos, goals);
        let gate_budget_left = self.gate_attempts < MAX_GATE_ATTEMPTS;
        let ownership_message = build_todo_ownership_continuation_message(todos, goals);
        let ownership_fingerprint = Some(ownership_message.clone());
        if ownership_needs_followup && self.last_ownership_fingerprint == ownership_fingerprint {
            // Already asked about this exact gap: keep the honest assessment
            // and stop rather than buy a turn that repeats the final answer.
            return Decision::default().with_effects(effects);
        }
        if ownership_needs_followup && gate_budget_left {
            self.last_ownership_fingerprint = ownership_fingerprint;
            self.gate_attempts = self.gate_attempts.saturating_add(1);
            return Decision::send(
                FollowUpKind::Ownership,
                ownership_message,
                "🔍 Checking end-to-end ownership before finishing...",
            )
            .with_effects(effects);
        }

        let summary = ConfidenceSummary::of(todos);
        let needs_spike_challenge = summary.spike_detected && !self.spike_challenged;
        if (summary.needs_validation || needs_spike_challenge) && gate_budget_left {
            self.gate_attempts = self.gate_attempts.saturating_add(1);
            let decision = if summary.needs_validation {
                Decision::send(
                    FollowUpKind::CompletionValidation,
                    build_todo_completion_continuation_message(todos),
                    "🔍 Double-checking confidence for you...",
                )
            } else {
                self.spike_challenged = true;
                Decision::send(
                    FollowUpKind::ConfidenceSpike,
                    build_todo_confidence_spike_continuation_message(todos),
                    "🔍 Double-checking confidence jumps...",
                )
            };
            return decision.with_effects(effects);
        }
        if (ownership_needs_followup || summary.needs_validation || needs_spike_challenge)
            && !gate_budget_left
        {
            // The gate keeps failing without progress. Nudging again would
            // loop forever, so stop and surface the stall.
            self.armed = false;
            self.spike_challenged = false;
            self.gate_attempts = 0;
            self.digest_delivered = false;
            return Decision {
                notice: Some(
                    "⚠️ We nudged the agent several times but its validation still isn't holding up. We stopped poking; review the remaining todos yourself."
                        .to_string(),
                ),
                ..Decision::default()
            }
            .with_effects(effects);
        }

        // Cycle finished cleanly.
        self.armed = self.default_on;
        self.digest_delivered = false;
        self.gate_attempts = 0;
        if !self.final_response_requested {
            self.final_response_requested = true;
            self.final_response_fingerprint = fingerprint;
            return Decision::send(
                FollowUpKind::FinalResponse,
                TODO_FINAL_RESPONSE_CONTINUATION_MESSAGE.to_string(),
                format!(
                    "✅ All todos done. Completion confidence: {}.",
                    summary.label()
                ),
            )
            .with_effects(effects);
        }
        Decision::default().with_effects(effects)
    }

    fn poke_incomplete(&mut self, incomplete: &[&TodoItem]) -> Decision {
        self.final_response_requested = false;
        // Open work begins a new completion cycle.
        self.spike_challenged = false;
        self.last_ownership_fingerprint = None;
        let message = build_auto_poke_message(incomplete.len());
        let fingerprint = serde_json::to_string(incomplete).unwrap_or_else(|_| message.clone());
        if self.last_poke_fingerprint.as_ref() == Some(&fingerprint) {
            // Same open work as the last poke: the model is stalled, not
            // progressing. Do not loop.
            return Decision::default();
        }
        // Open todos mean the model is iterating; gate exhaustion only trips
        // when the gate itself stops moving.
        self.gate_attempts = 0;
        self.last_poke_fingerprint = Some(fingerprint);
        let count = incomplete.len();
        Decision::send(
            FollowUpKind::IncompleteTodos,
            message,
            format!(
                "👉 {count} incomplete todo{}. We poked it for you. /poke off to stop.",
                if count == 1 { "" } else { "s" }
            ),
        )
    }
}

/// Completion-confidence summary over the completed todos.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct ConfidenceSummary {
    /// Priority-weighted average of recorded completion confidence (0-100).
    pub average: Option<u8>,
    pub needs_validation: bool,
    pub spike_detected: bool,
}

fn priority_weight(priority: &str) -> u32 {
    match priority {
        "high" => 3,
        "medium" => 2,
        _ => 1,
    }
}

impl ConfidenceSummary {
    pub fn of(todos: &[TodoItem]) -> Self {
        let completed: Vec<&TodoItem> = todos
            .iter()
            .filter(|todo| todo_status_is_completed(&todo.status))
            .collect();
        let (mut sum, mut weight) = (0u32, 0u32);
        for todo in &completed {
            if let Some(state) = todo.completion_confidence {
                let w = priority_weight(&todo.priority);
                sum += u32::from(state.legacy_score()) * w;
                weight += w;
            }
        }
        let average = (weight > 0).then(|| ((sum + weight / 2) / weight) as u8);
        let needs_validation = !completed.is_empty()
            && completed.iter().any(|todo| {
                todo.completion_confidence.is_none()
                    || !completion_confidence_passes(todo.completion_confidence)
            });
        Self {
            average,
            needs_validation,
            spike_detected: !spike_completed_todos(todos).is_empty(),
        }
    }

    pub fn label(&self) -> String {
        self.average
            .map(ConfidenceState::from_legacy_score)
            .map(|state| state.as_str().to_string())
            .unwrap_or_else(|| "unknown".to_string())
    }
}

#[cfg(test)]
#[path = "followup_tests.rs"]
mod tests;
