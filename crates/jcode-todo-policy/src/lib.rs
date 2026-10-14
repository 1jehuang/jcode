//! Pure todo policy shared by every Jcode client: quality-gate checks, the
//! synthetic continuation messages (auto-poke, gate digest, ownership,
//! completion confidence, final response), and their transcript classifiers.
//!
//! No storage or runtime dependencies, so the SDK and any embedding client can
//! use the exact same policy as the TUI.

use serde::{Deserialize, Serialize};

pub mod followup;
pub mod store;

pub use followup::{
    ConfidenceSummary, Decision, FollowUp, FollowUpKind, FollowUpPolicy, MAX_GATE_ATTEMPTS,
    TodoEffects, TodoSnapshot, is_incomplete_todo,
};

/// Generic mid-task reassessment prompt. The elapsed-time policy that triggers
/// it is intentionally private so the model reassesses from evidence rather
/// than targeting a timer or evaluator boundary.
pub const TODO_LONG_SESSION_REVIEW_MESSAGE: &str = "[auto] Re-read the request. Update the todo plan and goal assessments from the evidence gathered so far. Correct anything stale or overstated, then continue the work. Do not reply or wait for the user.";
const PRE_COMPACT_TODO_LONG_SESSION_REVIEW_MESSAGE: &str = "[automated todo assessment review - not a user message] Re-read the request. Update the todo plan and goal assessments from the evidence gathered so far. Correct anything stale or overstated, then continue the work. Do not reply or wait for the user.";
const PRE_BUDGET_TODO_LONG_SESSION_REVIEW_MESSAGE: &str = "[automated todo assessment review - not a user message] Re-read the original request and reconsider the current todo plan and every goal assessment using the evidence gathered during the work so far. Correct anything stale or overstated, including intent understanding, feedback-loop relevance and coverage, autonomy, difficulty, delivery, confidence, iteration maturity, and stopping evidence. Do not reply conversationally or wait for the user. Continue the work after saving an honest updated assessment.";

/// Static quality-gate instructions should stay short enough to be read as a
/// nudge, not a replacement system prompt. Dynamic todo/goal details are added
/// separately and have their own list-size limits.
pub const TODO_QUALITY_GATE_MAX_APPROX_TOKENS: usize = 64;

pub use jcode_task_types::{
    Autonomy, ConfidenceState, DeliveryState, Difficulty, FeedbackLoopCoverage,
    FeedbackLoopRelevance, FeedbackLoopState, FeedbackLoopTraceability, IntentUnderstanding,
    IterationMaturity, TodoGoal, TodoGoalChange, TodoGoalField, TodoItem, TodoPlan, TodoPlanChange,
    TodoPlanField,
};

/// Return the canonical todo status for model-written status vocabulary.
///
/// The todo tool historically accepted any string, so persisted sessions can
/// contain natural completion synonyms such as `done` or `finished`. Keep this
/// helper tolerant for those sessions even though new tool calls advertise a
/// constrained vocabulary.
pub fn canonical_todo_status(status: &str) -> Option<&'static str> {
    let status = status.trim();
    if status.eq_ignore_ascii_case("pending") {
        Some("pending")
    } else if status.eq_ignore_ascii_case("in_progress")
        || status.eq_ignore_ascii_case("in progress")
        || status.eq_ignore_ascii_case("in-progress")
    {
        Some("in_progress")
    } else if status.eq_ignore_ascii_case("completed")
        || status.eq_ignore_ascii_case("complete")
        || status.eq_ignore_ascii_case("done")
        || status.eq_ignore_ascii_case("finished")
    {
        Some("completed")
    } else if status.eq_ignore_ascii_case("cancelled") || status.eq_ignore_ascii_case("canceled") {
        Some("cancelled")
    } else {
        None
    }
}

pub fn todo_status_is_completed(status: &str) -> bool {
    canonical_todo_status(status) == Some("completed")
}

pub fn todo_status_is_cancelled(status: &str) -> bool {
    canonical_todo_status(status) == Some("cancelled")
}

/// Whether the plan's intent understanding is solid enough to work against.
pub fn intent_understanding_passes(state: Option<IntentUnderstanding>) -> bool {
    state.is_some_and(|state| state >= IntentUnderstanding::Clear)
}

/// Whether a goal's feedback loop reports back on the requirements by itself.
pub fn feedback_loop_passes(state: Option<FeedbackLoopState>) -> bool {
    state.is_some_and(|state| state >= FeedbackLoopState::Closed)
}

/// Minimum directness expected from a completion check. More involved goals
/// need checks aligned with acceptance behavior rather than a representative
/// proxy alone.
pub fn required_feedback_loop_relevance(difficulty: Option<Difficulty>) -> FeedbackLoopRelevance {
    if difficulty.is_some_and(|difficulty| difficulty >= Difficulty::Involved) {
        FeedbackLoopRelevance::AcceptanceAligned
    } else {
        FeedbackLoopRelevance::Representative
    }
}

/// Minimum breadth expected from a completion check. More involved goals must
/// include edge cases and integration boundaries as well as their main paths.
pub fn required_feedback_loop_coverage(difficulty: Option<Difficulty>) -> FeedbackLoopCoverage {
    if difficulty.is_some_and(|difficulty| difficulty >= Difficulty::Involved) {
        FeedbackLoopCoverage::EdgeAndIntegrationPaths
    } else {
        FeedbackLoopCoverage::MainPaths
    }
}

pub fn feedback_loop_relevance_passes(goal: &TodoGoal) -> bool {
    goal.feedback_loop_relevance
        .is_some_and(|state| state >= required_feedback_loop_relevance(goal.difficulty))
}

pub fn feedback_loop_coverage_passes(goal: &TodoGoal) -> bool {
    goal.feedback_loop_coverage
        .is_some_and(|state| state >= required_feedback_loop_coverage(goal.difficulty))
}

pub fn required_feedback_loop_traceability(
    difficulty: Option<Difficulty>,
) -> FeedbackLoopTraceability {
    if difficulty.is_some_and(|difficulty| difficulty >= Difficulty::Involved) {
        FeedbackLoopTraceability::Complete
    } else {
        FeedbackLoopTraceability::Partial
    }
}

pub fn feedback_loop_traceability_passes(goal: &TodoGoal) -> bool {
    goal.feedback_loop_traceability
        .is_some_and(|state| state >= required_feedback_loop_traceability(goal.difficulty))
}

/// Whether a completed todo carries enough evidence behind its completion.
pub fn completion_confidence_passes(state: Option<ConfidenceState>) -> bool {
    state.is_some_and(|state| state >= ConfidenceState::Validated)
}

/// The minimum delivery state for a completed goal. Outcome delivery is only
/// appropriate when the request itself includes operational delivery, so it
/// must not be inferred from difficulty.
pub fn required_delivery_state(_difficulty: Option<Difficulty>) -> DeliveryState {
    DeliveryState::WorkflowValidated
}

/// Whether a completed goal's delivery and validation clear their
/// difficulty-calibrated bars.
pub fn delivery_state_passes(goal: &TodoGoal) -> bool {
    let delivery_passes = goal
        .delivery_state
        .is_some_and(|state| state >= required_delivery_state(goal.difficulty));
    let autonomy_passes = goal
        .autonomy
        .is_some_and(|state| state >= Autonomy::NecessaryFollowthrough);
    let iteration_passes = goal
        .iteration_maturity
        .is_some_and(IterationMaturity::permits_completion);
    let stopping_evidence_passes = !matches!(
        goal.iteration_maturity,
        Some(
            IterationMaturity::PlateauConfirmed
                | IterationMaturity::ConstraintsExhausted
                | IterationMaturity::BudgetExhausted
        )
    ) || goal
        .stopping_evidence
        .as_deref()
        .is_some_and(|evidence| !evidence.trim().is_empty());
    delivery_passes
        && autonomy_passes
        && iteration_passes
        && stopping_evidence_passes
        && feedback_loop_relevance_passes(goal)
        && feedback_loop_coverage_passes(goal)
        && feedback_loop_traceability_passes(goal)
}

/// Pre-plan-intent-rewrite alignment continuation. Kept only so persisted
/// transcripts still classify it as a synthetic gate message, not a user turn.
const LEGACY_TODO_ALIGNMENT_CONTINUATION_MESSAGE: &str = "Your alignment score is not high enough. Build a requirement inventory from the user's request, including outcomes, deliverables, constraints, prohibited actions, integration paths, edge cases, and necessary follow-through. Revise the plan and its stated user intention to represent every material item. Then map each item to an explicit observation or check in a feedback loop. Generic instructions to run tests, verify, or review count only for requirements those checks actually enforce; add separate checks for non-testable requirements. Reassess the weaker link before continuing the task.";

/// Model-facing continuation for the private intent-understanding check.
pub const TODO_INTENT_UNDERSTANDING_CONTINUATION_MESSAGE: &str = "[auto] Understand the user's intent better. Try to avoid asking the user. Make sure the todo is up to date.";
const PRE_COMPACT_TODO_INTENT_UNDERSTANDING_CONTINUATION_MESSAGE: &str = "Understand the user's intent better. Try to avoid asking the user. Make sure the todo is up to date.";

/// Previous verbose wording, retained so persisted sessions still classify it
/// as a hidden quality-gate message after the concise rewrite.
const PRE_CONCISE_TODO_INTENT_UNDERSTANDING_CONTINUATION_MESSAGE: &str = "Your understanding of the user's intent is not high enough. Re-read the request and think harder about what the user actually wants and left implicit, using the conversation and codebase as evidence. Form a requirement inventory covering outcomes, deliverables, constraints, prohibited actions, integration paths, edge cases, and necessary follow-through, and check the plan represents every material item. Do not ask the user; resolve the ambiguity yourself, then update the plan's user intention and understands_user_intent.";
const PRE_TODO_REMINDER_INTENT_UNDERSTANDING_CONTINUATION_MESSAGE: &str =
    "Understand the user's intent better. Try to avoid asking the user.";

/// Model-facing continuation for the private closed-feedback-loop check. Names
/// the assessment category without disclosing the score or threshold.
pub const TODO_CLOSED_FEEDBACK_LOOP_CONTINUATION_MESSAGE: &str = "[auto] Your feedback loop isn't good enough. Think about what feedback loops you need. Make sure the todo is up to date.";
const PRE_COMPACT_TODO_CLOSED_FEEDBACK_LOOP_CONTINUATION_MESSAGE: &str = "Improve the goal's feedback loop. Name a concrete check for each requirement and what result will show it passed. Update the todo, then continue the work.";
const PRE_TODO_REMINDER_CLOSED_FEEDBACK_LOOP_CONTINUATION_MESSAGE: &str = "Improve the goal's feedback loop. Name a concrete check for each requirement and what result will show it passed. Update the goal, then continue the work.";
const PRE_BUDGET_TODO_CLOSED_FEEDBACK_LOOP_CONTINUATION_MESSAGE: &str = "Your feedback loop is not closed. First, improve the goal's objective and name the observation that reports back on each requirement, so progress can be measured across iterations. Generic phrases such as run tests, verify, or review count only for requirements those named checks demonstrably enforce; add separate explicit checks for non-testable requirements. Then call the todo tool again with the revised goal before continuing the task. The goal is to create a strong feedback loop you can iterate against.";

/// Pre-rename ("hill-climbability") version of the closed-feedback-loop
/// continuation. Kept only so persisted transcripts still classify it as a
/// synthetic gate message rather than a user turn.
const LEGACY_TODO_HILL_CLIMBABILITY_CONTINUATION_MESSAGE: &str = "Your hill-climbability is not high enough. First, improve the goal's objective and feedback loop so progress can be measured across iterations. Then call the todo tool again with the revised goal before continuing the task. The goal is to create a strong feedback loop you can iterate against.";

/// Model-facing continuation for the private end-to-end ownership check. It
/// asks for more work without revealing that an evaluator triggered it.
pub const TODO_OWNERSHIP_CONTINUATION_MESSAGE: &str =
    "[auto] Continue the work below. Keep the todo up to date; do not reply or wait for the user.";
const PRE_COMPACT_TODO_OWNERSHIP_CONTINUATION_MESSAGE: &str = "[automated follow-up - not a user message] Continue the work below. Keep the todo up to date; do not reply or wait for the user.";

/// Actionable ownership gaps, rather than the stricter quality assessment in
/// `delivery_state_passes`. Missing metadata is not evidence of unfinished work,
/// and staying within the requested scope is not a failure of ownership. The
/// deferred feedback-loop review already handles validation-quality concerns.
fn ownership_followup_reasons(goal: &TodoGoal) -> Vec<&'static str> {
    let stopped = matches!(
        goal.iteration_maturity,
        Some(
            IterationMaturity::PlateauConfirmed
                | IterationMaturity::ConstraintsExhausted
                | IterationMaturity::BudgetExhausted
        )
    );
    let has_stopping_evidence = goal
        .stopping_evidence
        .as_deref()
        .is_some_and(|evidence| !evidence.trim().is_empty());
    // More autonomous work cannot remove an explicitly documented constraint
    // or exhausted budget. Preserve the honest assessment instead of demanding
    // a higher score or repeatedly asking for the unavailable acceptance path.
    if stopped && has_stopping_evidence {
        return Vec::new();
    }

    let mut reasons = Vec::new();
    if goal
        .delivery_state
        .is_some_and(|state| state < required_delivery_state(goal.difficulty))
    {
        reasons.push("carry the work through the complete workflow.");
    }
    if goal
        .iteration_maturity
        .is_some_and(|state| !state.permits_completion())
    {
        reasons.push("keep iterating and test the remaining hypotheses.");
    }
    if stopped && !has_stopping_evidence {
        reasons.push("gather more evidence about whether the work should stop.");
    }
    reasons
}

/// Build an ownership continuation that directs work toward each affected goal
/// without exposing fields, scores, thresholds, or pass/fail language.
pub fn build_todo_ownership_continuation_message(todos: &[TodoItem], goals: &[TodoGoal]) -> String {
    let mut groups: Vec<Option<String>> = Vec::new();
    for todo in todos {
        let group = normalized_group(todo.group.as_deref());
        if group_is_complete(todos, &group) && !groups.contains(&group) {
            groups.push(group);
        }
    }

    // The continuation also identifies the outstanding gaps for deduplication.
    // Reordering todos must not make the same issues appear new.
    groups.sort();
    let mut message = String::from(TODO_OWNERSHIP_CONTINUATION_MESSAGE);
    for group in groups {
        let label = group.as_deref().unwrap_or("ungrouped goal");
        let Some(goal) = goals
            .iter()
            .find(|goal| normalized_group(goal.group.as_deref()) == group)
        else {
            continue;
        };
        for reason in ownership_followup_reasons(goal) {
            message.push_str(&format!("\n- Goal \"{}\": {}", label, reason));
        }
    }
    message
}

/// Legacy ownership-gate wording (pre delivery_state rename). Kept only so
/// persisted transcripts still classify it as a synthetic gate message.
const LEGACY_TODO_OWNERSHIP_CONTINUATION_MESSAGE: &str = "[automated todo completion gate - not a user message] Your end-to-end ownership is not high enough to finish this goal.";

/// Model-facing continuation for private completion-confidence checks.
pub const TODO_COMPLETION_CONTINUATION_MESSAGE: &str = "[auto] Do more validation on the work below. Keep the todo up to date; do not reply or wait for the user.";
const PRE_COMPACT_TODO_COMPLETION_CONTINUATION_MESSAGE: &str = "[automated follow-up - not a user message] Do more validation on the work below. Keep the todo up to date; do not reply or wait for the user.";

/// Model-facing continuation identifying the items whose confidence jumped and
/// asking for one explicit double-check without exposing scores or thresholds.
pub const TODO_CONFIDENCE_SPIKE_CONTINUATION_MESSAGE: &str = "[auto] You had a confidence jump in the items below. Double-check that these are correct. Keep the todo up to date; do not reply or wait for the user.";
const PRE_COMPACT_TODO_CONFIDENCE_SPIKE_CONTINUATION_MESSAGE: &str = "[automated follow-up - not a user message] You had a confidence jump in the items below. Double-check that these are correct. Keep the todo up to date; do not reply or wait for the user.";

/// Final synthetic turn after no more automatic checks are needed. Gate
/// continuations tell the model not to reply, so without this handoff a cycle
/// can end on a bare tool call or an internal-looking validation response.
pub const TODO_FINAL_RESPONSE_CONTINUATION_MESSAGE: &str = "[auto] Give the user a concise final response now, including any remaining limitations or blockers. Do not call the todo tool or do more work.";
const PRE_NARROWED_TODO_FINAL_RESPONSE_CONTINUATION_MESSAGE: &str = "[auto] Quality checks passed. Give the user a concise final response now. Do not call the todo tool or do more work.";
const PRE_COMPACT_TODO_FINAL_RESPONSE_CONTINUATION_MESSAGE: &str = "[automated follow-up - not a user message] Quality checks passed. Give the user a concise final response now. Do not call the todo tool or do more work.";
const PRE_BUDGET_TODO_FINAL_RESPONSE_CONTINUATION_MESSAGE: &str = "[automated follow-up - not a user message] All work and quality checks are complete. Give the user the final response now. Default to fewer than 5 lines unless the user's request requires more detail. Summarize the outcome clearly; do not call the todo tool or perform more work.";

/// A completed todo is considered spike-finished when its final recorded
/// confidence step jumps from speculative straight to verified. Ordinary gains
/// after validation (including plausible to verified) do not merit another turn.
pub const TODO_CONFIDENCE_SPIKE_LEVELS: u8 = 3;

/// A first plan write that admits to not knowing what it is being asked to do
/// gets one immediate nudge, because a whole turn spent on the wrong task
/// cannot be recovered at turn end. Every other write-time check is deferred
/// to the turn-end digest.
pub const SEVERE_INTENT_MISUNDERSTANDING: IntentUnderstanding = IntentUnderstanding::Uncertain;

/// Which deferred quality check a recorded observation came from.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum GateObservationKind {
    IntentUnderstanding,
    ClosedFeedbackLoop,
    FeedbackLoopRelevance,
    FeedbackLoopCoverage,
    FeedbackLoopTraceability,
}

/// A point during the turn that would previously have interrupted the model
/// with a quality-gate continuation.
///
/// Recording instead of interrupting is the whole point: assessments like
/// intent understanding start low and rise as the agent explores the codebase,
/// so a check that fires the moment a score is low mostly punishes agents that
/// are already in the process of fixing it. These observations are replayed
/// once at turn end and filtered against the final scores, so only the points
/// that never resolved are surfaced.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct GateObservation {
    pub kind: GateObservationKind,
    /// Todo group for goal-scoped observations; `None` for plan-level ones.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub group: Option<String>,
    /// The semantic state observed when this point was flagged, as its string
    /// form. Legacy logs stored a numeric `score`; those entries load with
    /// `state: None`, which the digest already treats as an unresolved point.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub state: Option<String>,
}

/// Header for the turn-end digest of unresolved quality-check points.
///
/// Deliberately framed as "double-check these" rather than as a refusal: by
/// turn end the work is done, so the useful action is verification, not
/// replanning. Names categories without disclosing scores or thresholds.
pub const TODO_GATE_DIGEST_PREFIX: &str = "[auto] Before you treat this turn as finished, double-check the weak points it surfaced. Keep the todo up to date. Do not reply or wait for the user.";
const PRE_COMPACT_TODO_GATE_DIGEST_PREFIX: &str = "Before you treat this turn as finished, double-check the weak points it surfaced. Keep the todo up to date. Do not reply or wait for the user.";
const LABELED_TODO_GATE_DIGEST_PREFIX: &str = "[automated todo quality review - not a user message] Before you treat this turn as finished, double-check the weak points it surfaced. Do not reply conversationally or wait for the user.";

/// Whether the state behind this observation has since reached its bar.
///
/// This no longer suppresses the observation: a loop that closed only after
/// work was already underway did not govern the work done before it. It selects
/// the wording instead, so a late climb is described as a coverage gap rather
/// than as a goal that never had a loop at all.
fn observation_score_later_cleared(
    observation: &GateObservation,
    plan: &TodoPlan,
    goals: &[TodoGoal],
) -> bool {
    match observation.kind {
        GateObservationKind::IntentUnderstanding => {
            intent_understanding_passes(plan.understands_user_intent)
        }
        GateObservationKind::ClosedFeedbackLoop => feedback_loop_passes(
            goals
                .iter()
                .find(|goal| normalized_group(goal.group.as_deref()) == observation.group)
                .and_then(|goal| goal.closed_feedback_loop),
        ),
        GateObservationKind::FeedbackLoopRelevance => goals
            .iter()
            .find(|goal| normalized_group(goal.group.as_deref()) == observation.group)
            .is_some_and(feedback_loop_relevance_passes),
        GateObservationKind::FeedbackLoopCoverage => goals
            .iter()
            .find(|goal| normalized_group(goal.group.as_deref()) == observation.group)
            .is_some_and(feedback_loop_coverage_passes),
        GateObservationKind::FeedbackLoopTraceability => goals
            .iter()
            .find(|goal| normalized_group(goal.group.as_deref()) == observation.group)
            .is_some_and(feedback_loop_traceability_passes),
    }
}

/// Build the turn-end reminder from this turn's recorded observations.
///
/// Every point recorded during the turn is surfaced, including ones whose score
/// later rose past the threshold. A late climb is exactly the case worth
/// raising: if the goal had no measurable loop while the work was being done,
/// that work never benefited from the better loop the agent eventually wrote
/// down, so the score reads as passing while the result behind it is unchecked.
/// The score still decides the wording, so a late climb is asked to extend its
/// loop back over the earlier work rather than being told it has no loop.
///
/// Repeats of the same point collapse into one line with a count, so a long
/// iterative turn cannot generate a wall of duplicates. Returns `None` when
/// nothing was recorded.
pub fn build_gate_digest(
    observations: &[GateObservation],
    plan: &TodoPlan,
    goals: &[TodoGoal],
) -> Option<String> {
    // (kind, group, times flagged, score later cleared)
    let mut points: Vec<(GateObservationKind, Option<String>, usize, bool)> = Vec::new();
    for observation in observations {
        let cleared = observation_score_later_cleared(observation, plan, goals);
        match points
            .iter_mut()
            .find(|(kind, group, _, _)| *kind == observation.kind && *group == observation.group)
        {
            Some((_, _, count, _)) => *count += 1,
            None => points.push((observation.kind, observation.group.clone(), 1, cleared)),
        }
    }
    if points.is_empty() {
        return None;
    }

    let mut message = String::from(TODO_GATE_DIGEST_PREFIX);
    for (kind, group, count, cleared) in &points {
        let detail = match (kind, cleared) {
            (GateObservationKind::IntentUnderstanding, false) => {
                "your understanding of what the user actually wants never became solid. Re-read the request, confirm the work you did matches it, and state any interpretation you had to guess at.".to_string()
            }
            (GateObservationKind::IntentUnderstanding, true) => {
                "you started this work without understanding what the user actually wants, and only settled it later. Re-check the work you did before it settled against the request you now understand, and state any interpretation you had to guess at.".to_string()
            }
            (GateObservationKind::ClosedFeedbackLoop, false) => {
                let label = group
                    .as_deref()
                    .map(|group| format!(" for \"{}\"", group))
                    .unwrap_or_default();
                format!(
                    "the goal{} never closed its feedback loop: no observation reported back on whether the work satisfied the requirements. Confirm the result is actually better, with concrete evidence rather than inspection.",
                    label
                )
            }
            (GateObservationKind::ClosedFeedbackLoop, true) => {
                let label = group
                    .as_deref()
                    .map(|group| format!(" for \"{}\"", group))
                    .unwrap_or_default();
                format!(
                    "the goal{} was worked on before its feedback loop was closed, so the loop you ended up with never ran over that earlier work. Run it over the whole result now and report what it actually reported back.",
                    label
                )
            }
            (GateObservationKind::FeedbackLoopRelevance, false) => {
                let label = group
                    .as_deref()
                    .map(|group| format!(" for \"{}\"", group))
                    .unwrap_or_default();
                format!(
                    "the checks{} did not directly represent how the result will be used or accepted. Exercise the real project's public interfaces, integration boundaries, or end-user acceptance path and report the observed behavior. A custom harness, stub, mock, copied source, or synthetic fixture is useful evidence but cannot replace that path; if the real path is externally blocked, record that constraint honestly.",
                    label
                )
            }
            (GateObservationKind::FeedbackLoopRelevance, true) => {
                let label = group
                    .as_deref()
                    .map(|group| format!(" for \"{}\"", group))
                    .unwrap_or_default();
                format!(
                    "the representative checks{} were identified only after earlier work was done. Run them over the whole result now, including public interfaces and integration boundaries.",
                    label
                )
            }
            (GateObservationKind::FeedbackLoopCoverage, false) => {
                let label = group
                    .as_deref()
                    .map(|group| format!(" for \"{}\"", group))
                    .unwrap_or_default();
                format!(
                    "the checks{} covered too narrow a path. Exercise the main workflows, edge cases, packaging, integration paths, and likely failure modes that could invalidate the result.",
                    label
                )
            }
            (GateObservationKind::FeedbackLoopCoverage, true) => {
                let label = group
                    .as_deref()
                    .map(|group| format!(" for \"{}\"", group))
                    .unwrap_or_default();
                format!(
                    "the broader checks{} were identified only after earlier work was done. Run them over the whole result now, including edge cases, packaging, integration paths, and likely failure modes.",
                    label
                )
            }
            (GateObservationKind::FeedbackLoopTraceability, false) => {
                let label = group
                    .as_deref()
                    .map(|group| format!(" for \"{}\"", group))
                    .unwrap_or_default();
                format!(
                    "the checks{} were not traced to every explicit requirement and changed public output. Map each one to a concrete check and report the observed result; aggregate test counts do not establish this mapping.",
                    label
                )
            }
            (GateObservationKind::FeedbackLoopTraceability, true) => {
                let label = group
                    .as_deref()
                    .map(|group| format!(" for \"{}\"", group))
                    .unwrap_or_default();
                format!(
                    "complete requirement-to-check traceability{} was identified only after earlier work was done. Run every mapped check over the whole result now and report what each requirement and changed public output actually did.",
                    label
                )
            }
        };
        let repeats = if *count > 1 {
            format!(" (flagged {} times this turn)", count)
        } else {
            String::new()
        };
        message.push_str(&format!("\n- {}{}", detail, repeats));
    }
    message.push_str(
        "\nAddress the points above, then update the todo tool with the assessments that reflect what you verified.",
    );
    Some(message)
}

const LEGACY_TODO_CONFIDENCE_SUMMARY_PREFIX: &str = "All todos are done. Todo confidence summary:";
/// Pre-gate-rewrite texts (before the "[automated todo completion gate" prefix)
/// still exist in persisted transcripts; keep detecting them so reload/resume
/// does not re-render them as user prompts.
const LEGACY_TODO_COMPLETION_CONTINUATION_MESSAGE: &str =
    "Your completion confidence is missing or not high enough.";
const LEGACY_TODO_CONFIDENCE_SPIKE_CONTINUATION_MESSAGE: &str =
    "Your completion confidence rose too sharply to count as independently validated.";
/// Wording used immediately before the evidence-backed framing. Persisted
/// sessions can still contain it and must keep treating it as a hidden gate.
const PRE_EVIDENCE_TODO_CONFIDENCE_SPIKE_CONTINUATION_MESSAGE: &str = "[automated follow-up - not a user message] Independently recheck the work below. Keep the todo up to date; do not reply or wait for the user.";

fn normalized_group(group: Option<&str>) -> Option<String> {
    group
        .map(str::trim)
        .filter(|group| !group.is_empty())
        .map(str::to_string)
}

fn group_is_complete(todos: &[TodoItem], group: &Option<String>) -> bool {
    let mut matching = todos
        .iter()
        .filter(|todo| normalized_group(todo.group.as_deref()) == *group)
        .peekable();
    matching.peek().is_some() && matching.all(|todo| todo.status == "completed")
}

/// Whether every group newly closed by this update has a sufficient recorded
/// delivery state for its difficulty. Groups completed before this check was
/// introduced are intentionally grandfathered so existing sessions stay writable.
pub fn newly_completed_groups_have_sufficient_delivery(
    previous: &[TodoItem],
    incoming: &[TodoItem],
    goals: &[TodoGoal],
) -> bool {
    let mut groups: Vec<Option<String>> = Vec::new();
    for todo in incoming {
        let group = normalized_group(todo.group.as_deref());
        if !groups.contains(&group) {
            groups.push(group);
        }
    }

    groups.into_iter().all(|group| {
        if !group_is_complete(incoming, &group) || group_is_complete(previous, &group) {
            return true;
        }
        goals
            .iter()
            .find(|goal| normalized_group(goal.group.as_deref()) == group)
            .is_some_and(delivery_state_passes)
    })
}

/// Whether completed groups have no actionable ownership gaps at turn finish.
/// Unlike the strict delivery assessment, absent metadata and documented stops
/// do not justify an automatic continuation.
pub fn completed_groups_have_sufficient_delivery(todos: &[TodoItem], goals: &[TodoGoal]) -> bool {
    let mut groups: Vec<Option<String>> = Vec::new();
    for todo in todos {
        let group = normalized_group(todo.group.as_deref());
        if !groups.contains(&group) {
            groups.push(group);
        }
    }

    groups.into_iter().all(|group| {
        if !group_is_complete(todos, &group) {
            return true;
        }
        goals
            .iter()
            .find(|goal| normalized_group(goal.group.as_deref()) == group)
            .is_none_or(|goal| ownership_followup_reasons(goal).is_empty())
    })
}

/// Groups that this update closes: complete in `incoming`, not complete before.
///
/// Quality checks need these as well as the still-open groups. A turn that
/// creates and finishes a group in a single write would otherwise record no
/// observation at all, and the weakest goals are exactly the ones most likely to
/// be declared done in one step.
pub fn groups_closed_by_update(
    previous: &[TodoItem],
    incoming: &[TodoItem],
) -> Vec<Option<String>> {
    let mut groups: Vec<Option<String>> = Vec::new();
    for todo in incoming {
        let group = normalized_group(todo.group.as_deref());
        if groups.contains(&group) {
            continue;
        }
        if group_is_complete(incoming, &group) && !group_is_complete(previous, &group) {
            groups.push(group);
        }
    }
    groups
}

/// Completed todos whose final recorded confidence step jumped the entire
/// scale. Planning confidence and completion confidence describe different
/// things, so records without an observed history cannot establish a spike.
pub fn spike_completed_todos(todos: &[TodoItem]) -> Vec<&TodoItem> {
    fn is_spike(from: ConfidenceState, to: ConfidenceState) -> bool {
        to.level().saturating_sub(from.level()) >= TODO_CONFIDENCE_SPIKE_LEVELS
    }
    todos
        .iter()
        .filter(|todo| todo_status_is_completed(&todo.status))
        .filter(|todo| match todo.confidence_history.as_slice() {
            [] | [_] => false,
            history => {
                let last = history[history.len() - 1];
                todo.completion_confidence == Some(last)
                    && is_spike(history[history.len() - 2], last)
            }
        })
        .collect()
}

/// Build the synthetic auto-poke continuation prompt sent when the model
/// stops with incomplete todos. Kept here so every producer (TUI auto-poke,
/// `jcode run` auto-poke) and the transcript renderer agree on the exact text.
pub fn build_auto_poke_message(incomplete_count: usize) -> String {
    format!(
        "You have {} incomplete todo{}. Continue working, or update the todo tool.",
        incomplete_count,
        if incomplete_count == 1 { "" } else { "s" },
    )
}

/// Longest list of named todos a gate continuation will spell out, so a big
/// plan cannot turn one nudge into a wall of text.
const GATE_NAMED_TODO_LIMIT: usize = 6;

fn quoted_todo_label(todo: &TodoItem) -> String {
    let content = todo.content.trim();
    let label: String = if content.chars().count() > 80 {
        format!("{}…", content.chars().take(79).collect::<String>())
    } else {
        content.to_string()
    };
    format!("\"{}\"", label)
}

fn append_named_todos(message: &mut String, lead: &str, todos: &[&TodoItem]) {
    if todos.is_empty() {
        return;
    }
    message.push_str("\n- ");
    message.push_str(lead);
    message.push(' ');
    let named: Vec<String> = todos
        .iter()
        .take(GATE_NAMED_TODO_LIMIT)
        .map(|todo| quoted_todo_label(todo))
        .collect();
    message.push_str(&named.join(", "));
    if todos.len() > named.len() {
        message.push_str(&format!(" (and {} more)", todos.len() - named.len()));
    }
    message.push('.');
}

/// Follow-up naming exactly which completed todos need more validation, without
/// exposing evaluator language, scores, thresholds, or the internal reason.
pub fn build_todo_completion_continuation_message(todos: &[TodoItem]) -> String {
    let completed: Vec<&TodoItem> = todos
        .iter()
        .filter(|todo| todo.status == "completed")
        .collect();
    let missing: Vec<&TodoItem> = completed
        .iter()
        .copied()
        .filter(|todo| todo.completion_confidence.is_none())
        .collect();
    let weak: Vec<&TodoItem> = completed
        .iter()
        .copied()
        .filter(|todo| {
            todo.completion_confidence
                .is_some_and(|state| !completion_confidence_passes(Some(state)))
        })
        .collect();

    let mut message = String::from(TODO_COMPLETION_CONTINUATION_MESSAGE);
    let needs_validation: Vec<&TodoItem> = completed
        .iter()
        .copied()
        .filter(|todo| missing.contains(todo) || weak.contains(todo))
        .collect();
    let targets = if needs_validation.is_empty() {
        &completed
    } else {
        &needs_validation
    };
    append_named_todos(&mut message, "Validate further:", targets);
    message
}

/// Spike-gate continuation naming the completed todos whose confidence jumped,
/// so the double-check targets those items.
pub fn build_todo_confidence_spike_continuation_message(todos: &[TodoItem]) -> String {
    let spiked = spike_completed_todos(todos);
    let mut message = String::from(TODO_CONFIDENCE_SPIKE_CONTINUATION_MESSAGE);
    append_named_todos(&mut message, "Confidence jumped:", &spiked);
    message
}

/// True when `message` is a synthetic auto-poke continuation (the
/// incomplete-todos poke or the todo confidence summary) rather than a real
/// user prompt.
///
/// These are persisted as `Role::User` so the model treats them as a normal
/// continuation turn, but they are not something the user typed. The live UI
/// hides them (showing an "Auto-poking..." notice instead), and the session
/// renderer uses this to avoid re-rendering them as user prompts on
/// reload/resume/remote attach.
pub fn is_auto_poke_message(message: &str) -> bool {
    let trimmed = message.trim();
    (trimmed.starts_with("You have ")
        && trimmed.contains(" incomplete todo")
        && trimmed.ends_with("update the todo tool."))
        || trimmed.starts_with(TODO_CLOSED_FEEDBACK_LOOP_CONTINUATION_MESSAGE)
        || trimmed.starts_with(PRE_COMPACT_TODO_CLOSED_FEEDBACK_LOOP_CONTINUATION_MESSAGE)
        || trimmed.starts_with(PRE_TODO_REMINDER_CLOSED_FEEDBACK_LOOP_CONTINUATION_MESSAGE)
        || trimmed.starts_with(PRE_BUDGET_TODO_CLOSED_FEEDBACK_LOOP_CONTINUATION_MESSAGE)
        || trimmed.starts_with(LEGACY_TODO_HILL_CLIMBABILITY_CONTINUATION_MESSAGE)
        || trimmed.starts_with(LEGACY_TODO_ALIGNMENT_CONTINUATION_MESSAGE)
        || trimmed.starts_with(TODO_INTENT_UNDERSTANDING_CONTINUATION_MESSAGE)
        || trimmed.starts_with(PRE_COMPACT_TODO_INTENT_UNDERSTANDING_CONTINUATION_MESSAGE)
        || trimmed.starts_with(PRE_TODO_REMINDER_INTENT_UNDERSTANDING_CONTINUATION_MESSAGE)
        || trimmed.starts_with(PRE_CONCISE_TODO_INTENT_UNDERSTANDING_CONTINUATION_MESSAGE)
        || trimmed.starts_with(TODO_OWNERSHIP_CONTINUATION_MESSAGE)
        || trimmed.starts_with(PRE_COMPACT_TODO_OWNERSHIP_CONTINUATION_MESSAGE)
        || trimmed.starts_with(LEGACY_TODO_OWNERSHIP_CONTINUATION_MESSAGE)
        || trimmed.starts_with(TODO_COMPLETION_CONTINUATION_MESSAGE)
        || trimmed.starts_with(PRE_COMPACT_TODO_COMPLETION_CONTINUATION_MESSAGE)
        || trimmed.starts_with(TODO_CONFIDENCE_SPIKE_CONTINUATION_MESSAGE)
        || trimmed.starts_with(PRE_COMPACT_TODO_CONFIDENCE_SPIKE_CONTINUATION_MESSAGE)
        || trimmed.starts_with(TODO_FINAL_RESPONSE_CONTINUATION_MESSAGE)
        || trimmed.starts_with(PRE_NARROWED_TODO_FINAL_RESPONSE_CONTINUATION_MESSAGE)
        || trimmed.starts_with(PRE_COMPACT_TODO_FINAL_RESPONSE_CONTINUATION_MESSAGE)
        || trimmed.starts_with(PRE_BUDGET_TODO_FINAL_RESPONSE_CONTINUATION_MESSAGE)
        || trimmed.starts_with(LEGACY_TODO_COMPLETION_CONTINUATION_MESSAGE)
        || trimmed.starts_with(LEGACY_TODO_CONFIDENCE_SPIKE_CONTINUATION_MESSAGE)
        || trimmed.starts_with(PRE_EVIDENCE_TODO_CONFIDENCE_SPIKE_CONTINUATION_MESSAGE)
        || trimmed.starts_with(LEGACY_TODO_CONFIDENCE_SUMMARY_PREFIX)
        || trimmed.starts_with(TODO_GATE_DIGEST_PREFIX)
        || trimmed.starts_with(PRE_COMPACT_TODO_GATE_DIGEST_PREFIX)
        || trimmed.starts_with(LABELED_TODO_GATE_DIGEST_PREFIX)
        || trimmed.starts_with(TODO_LONG_SESSION_REVIEW_MESSAGE)
        || trimmed.starts_with(PRE_COMPACT_TODO_LONG_SESSION_REVIEW_MESSAGE)
        || trimmed.starts_with(PRE_BUDGET_TODO_LONG_SESSION_REVIEW_MESSAGE)
}

/// Short, user-facing stand-in for a synthetic auto-poke/gate continuation.
///
/// The continuations themselves are written for the model and name specific
/// todos and required fields. Showing that wall of instructions in the
/// transcript (on reload/resume, where the live short notice is gone) buries the
/// conversation, so the UI renders this one-liner instead.
pub fn auto_poke_display_summary(message: &str) -> Option<&'static str> {
    let trimmed = message.trim();
    if !is_auto_poke_message(trimmed) {
        return None;
    }
    if trimmed.starts_with(TODO_CONFIDENCE_SPIKE_CONTINUATION_MESSAGE)
        || trimmed.starts_with(PRE_COMPACT_TODO_CONFIDENCE_SPIKE_CONTINUATION_MESSAGE)
        || trimmed.starts_with(LEGACY_TODO_CONFIDENCE_SPIKE_CONTINUATION_MESSAGE)
        || trimmed.starts_with(PRE_EVIDENCE_TODO_CONFIDENCE_SPIKE_CONTINUATION_MESSAGE)
    {
        return Some("🔍 Double-checking confidence jumps...");
    }
    if trimmed.starts_with(TODO_FINAL_RESPONSE_CONTINUATION_MESSAGE)
        || trimmed.starts_with(PRE_NARROWED_TODO_FINAL_RESPONSE_CONTINUATION_MESSAGE)
        || trimmed.starts_with(PRE_COMPACT_TODO_FINAL_RESPONSE_CONTINUATION_MESSAGE)
        || trimmed.starts_with(PRE_BUDGET_TODO_FINAL_RESPONSE_CONTINUATION_MESSAGE)
    {
        return Some("✅ Preparing the final response...");
    }
    if trimmed.starts_with(TODO_COMPLETION_CONTINUATION_MESSAGE)
        || trimmed.starts_with(PRE_COMPACT_TODO_COMPLETION_CONTINUATION_MESSAGE)
        || trimmed.starts_with(LEGACY_TODO_COMPLETION_CONTINUATION_MESSAGE)
        || trimmed.starts_with(LEGACY_TODO_CONFIDENCE_SUMMARY_PREFIX)
    {
        return Some("🔍 Double-checking confidence for you...");
    }
    if trimmed.starts_with(TODO_GATE_DIGEST_PREFIX)
        || trimmed.starts_with(PRE_COMPACT_TODO_GATE_DIGEST_PREFIX)
        || trimmed.starts_with(LABELED_TODO_GATE_DIGEST_PREFIX)
    {
        return Some("🔍 Reviewing the weak points of this turn for you...");
    }
    if trimmed.starts_with(TODO_LONG_SESSION_REVIEW_MESSAGE)
        || trimmed.starts_with(PRE_COMPACT_TODO_LONG_SESSION_REVIEW_MESSAGE)
        || trimmed.starts_with(PRE_BUDGET_TODO_LONG_SESSION_REVIEW_MESSAGE)
    {
        return Some("🔍 Rechecking the plan and assessments after extended work...");
    }
    if trimmed.starts_with(TODO_OWNERSHIP_CONTINUATION_MESSAGE)
        || trimmed.starts_with(PRE_COMPACT_TODO_OWNERSHIP_CONTINUATION_MESSAGE)
        || trimmed.starts_with(LEGACY_TODO_OWNERSHIP_CONTINUATION_MESSAGE)
    {
        return Some("🔍 Checking the delivery state of the finished work...");
    }
    if trimmed.starts_with(TODO_INTENT_UNDERSTANDING_CONTINUATION_MESSAGE)
        || trimmed.starts_with(PRE_COMPACT_TODO_INTENT_UNDERSTANDING_CONTINUATION_MESSAGE)
        || trimmed.starts_with(PRE_TODO_REMINDER_INTENT_UNDERSTANDING_CONTINUATION_MESSAGE)
        || trimmed.starts_with(PRE_CONCISE_TODO_INTENT_UNDERSTANDING_CONTINUATION_MESSAGE)
    {
        return Some("🔍 Re-checking the request was understood...");
    }
    if trimmed.starts_with(TODO_CLOSED_FEEDBACK_LOOP_CONTINUATION_MESSAGE)
        || trimmed.starts_with(PRE_COMPACT_TODO_CLOSED_FEEDBACK_LOOP_CONTINUATION_MESSAGE)
        || trimmed.starts_with(PRE_TODO_REMINDER_CLOSED_FEEDBACK_LOOP_CONTINUATION_MESSAGE)
        || trimmed.starts_with(PRE_BUDGET_TODO_CLOSED_FEEDBACK_LOOP_CONTINUATION_MESSAGE)
        || trimmed.starts_with(LEGACY_TODO_HILL_CLIMBABILITY_CONTINUATION_MESSAGE)
        || trimmed.starts_with(LEGACY_TODO_ALIGNMENT_CONTINUATION_MESSAGE)
    {
        return Some("🔍 Asking for a stronger way to verify this work...");
    }
    // Incomplete-todos poke: the count is genuinely useful, and it is already
    // short, so it keeps its own text.
    None
}

/// Derive a concise session-title hint from the todo tool's persisted plan.
///
/// Todo groups are intended to name coherent goals, so the group containing the
/// current (or latest incomplete) item is the strongest signal. Ungrouped plans
/// fall back to the plan's user intention, then item text.
pub fn derive_session_title(todos: &[TodoItem], plan: &TodoPlan) -> Option<String> {
    fn non_empty(value: Option<&str>) -> Option<String> {
        value
            .map(str::trim)
            .filter(|value| !value.is_empty())
            .map(str::to_string)
    }

    let current = todos
        .iter()
        .rev()
        .find(|todo| todo.status.eq_ignore_ascii_case("in_progress"))
        .or_else(|| {
            todos
                .iter()
                .rev()
                .find(|todo| !todo.status.eq_ignore_ascii_case("completed"))
        })
        .or_else(|| todos.last());

    if let Some(todo) = current {
        if let Some(group) = non_empty(todo.group.as_deref()) {
            return Some(group);
        }

        if let Some(user_intention) = non_empty(plan.user_intention.as_deref()) {
            return Some(user_intention);
        }

        return non_empty(Some(&todo.content));
    }

    non_empty(plan.user_intention.as_deref())
}

#[cfg(test)]
#[path = "tests.rs"]
mod tests;
