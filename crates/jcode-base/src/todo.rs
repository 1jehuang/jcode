use crate::storage;
use anyhow::Result;
use serde::{Deserialize, Serialize};
use std::path::PathBuf;

/// Generic mid-task reassessment prompt. The elapsed-time policy that triggers
/// it is intentionally private so the model reassesses from evidence rather
/// than targeting a timer or evaluator boundary.
pub const TODO_LONG_SESSION_REVIEW_MESSAGE: &str = "[automated todo assessment review - not a user message] Re-read the original request and reconsider the current todo plan and every goal assessment using the evidence gathered during the work so far. Correct anything stale or overstated, including intent understanding, feedback-loop relevance and coverage, autonomy, difficulty, delivery, confidence, iteration maturity, and stopping evidence. Do not reply conversationally or wait for the user. Continue the work after saving an honest updated assessment.";

/// Private policy. Do not include this duration in model-facing schemas or
/// continuation text.
const TODO_LONG_SESSION_REVIEW_AFTER: chrono::Duration = chrono::Duration::minutes(30);

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
struct TodoReviewState {
    cycle_started_at: chrono::DateTime<chrono::Utc>,
    #[serde(default)]
    review_delivered: bool,
}

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
/// Deliberately small: think more about the user's intent, do not ask the user.
pub const TODO_INTENT_UNDERSTANDING_CONTINUATION_MESSAGE: &str = "Your understanding of the user's intent is not high enough. Re-read the request and think harder about what the user actually wants and left implicit, using the conversation and codebase as evidence. Form a requirement inventory covering outcomes, deliverables, constraints, prohibited actions, integration paths, edge cases, and necessary follow-through, and check the plan represents every material item. Do not ask the user; resolve the ambiguity yourself, then update the plan's user intention and understands_user_intent.";

/// Model-facing continuation for the private closed-feedback-loop check. Names
/// the assessment category without disclosing the score or threshold.
pub const TODO_CLOSED_FEEDBACK_LOOP_CONTINUATION_MESSAGE: &str = "Your feedback loop is not closed. First, improve the goal's objective and name the observation that reports back on each requirement, so progress can be measured across iterations. Generic phrases such as run tests, verify, or review count only for requirements those named checks demonstrably enforce; add separate explicit checks for non-testable requirements. Then call the todo tool again with the revised goal before continuing the task. The goal is to create a strong feedback loop you can iterate against.";

/// Pre-rename ("hill-climbability") version of the closed-feedback-loop
/// continuation. Kept only so persisted transcripts still classify it as a
/// synthetic gate message rather than a user turn.
const LEGACY_TODO_HILL_CLIMBABILITY_CONTINUATION_MESSAGE: &str = "Your hill-climbability is not high enough. First, improve the goal's objective and feedback loop so progress can be measured across iterations. Then call the todo tool again with the revised goal before continuing the task. The goal is to create a strong feedback loop you can iterate against.";

/// Model-facing continuation for the private end-to-end ownership check. It
/// asks for more work without revealing that an evaluator triggered it.
pub const TODO_OWNERSHIP_CONTINUATION_MESSAGE: &str = "[automated follow-up - not a user message] Continue the work below. Keep the todo up to date; do not reply or wait for the user.";

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

    let mut message = String::from(TODO_OWNERSHIP_CONTINUATION_MESSAGE);
    for group in groups {
        let label = group.as_deref().unwrap_or("ungrouped goal");
        let Some(goal) = goals
            .iter()
            .find(|goal| normalized_group(goal.group.as_deref()) == group)
        else {
            message.push_str(&format!(
                "\n- Goal \"{}\": clarify the goal and track the work.",
                label
            ));
            continue;
        };
        if !goal
            .delivery_state
            .is_some_and(|state| state >= required_delivery_state(goal.difficulty))
        {
            message.push_str(&format!(
                "\n- Goal \"{}\": carry the work through the complete workflow.",
                label
            ));
        }
        if !goal
            .autonomy
            .is_some_and(|state| state >= Autonomy::NecessaryFollowthrough)
        {
            message.push_str(&format!(
                "\n- Goal \"{}\": take ownership of the necessary follow-through.",
                label
            ));
        }
        if !goal
            .iteration_maturity
            .is_some_and(IterationMaturity::permits_completion)
        {
            message.push_str(&format!(
                "\n- Goal \"{}\": keep iterating and test the remaining hypotheses.",
                label
            ));
        }
        if !feedback_loop_relevance_passes(goal) {
            message.push_str(&format!(
                "\n- Goal \"{}\": validate the result through its public interfaces and acceptance behavior, including its integration boundaries.",
                label
            ));
        }
        if !feedback_loop_coverage_passes(goal) {
            message.push_str(&format!(
                "\n- Goal \"{}\": exercise the main workflows, edge cases, packaging, and likely failure modes.",
                label
            ));
        }
        if !feedback_loop_traceability_passes(goal) {
            message.push_str(&format!(
                "\n- Goal \"{}\": map every explicit requirement and changed public output to a concrete check and report its observed result.",
                label
            ));
        }
        if matches!(
            goal.iteration_maturity,
            Some(
                IterationMaturity::PlateauConfirmed
                    | IterationMaturity::ConstraintsExhausted
                    | IterationMaturity::BudgetExhausted
            )
        ) && !goal
            .stopping_evidence
            .as_deref()
            .is_some_and(|evidence| !evidence.trim().is_empty())
        {
            message.push_str(&format!(
                "\n- Goal \"{}\": gather more evidence about whether the work should stop.",
                label
            ));
        }
    }
    message
}

/// Legacy ownership-gate wording (pre delivery_state rename). Kept only so
/// persisted transcripts still classify it as a synthetic gate message.
const LEGACY_TODO_OWNERSHIP_CONTINUATION_MESSAGE: &str = "[automated todo completion gate - not a user message] Your end-to-end ownership is not high enough to finish this goal.";

/// Model-facing continuation for private completion-confidence checks.
pub const TODO_COMPLETION_CONTINUATION_MESSAGE: &str = "[automated follow-up - not a user message] Do more validation on the work below. Keep the todo up to date; do not reply or wait for the user.";

/// Model-facing continuation requesting an independent recheck without saying
/// why the private evaluator selected it.
pub const TODO_CONFIDENCE_SPIKE_CONTINUATION_MESSAGE: &str = "[automated follow-up - not a user message] Independently recheck the work below. Keep the todo up to date; do not reply or wait for the user.";

/// Final synthetic turn after every todo completion check has passed. Gate
/// continuations tell the model not to reply, so without this handoff a cycle
/// can end on a bare tool call or an internal-looking validation response.
pub const TODO_FINAL_RESPONSE_CONTINUATION_MESSAGE: &str = "[automated follow-up - not a user message] All work and quality checks are complete. Give the user the final response now. Default to fewer than 5 lines unless the user's request requires more detail. Summarize the outcome clearly; do not call the todo tool or perform more work.";

/// A completed todo is considered spike-finished when its final recorded
/// confidence step jumps this many levels or more (e.g. speculative straight
/// to validated) instead of climbing through evidence-backed states.
pub const TODO_CONFIDENCE_SPIKE_LEVELS: u8 = 2;

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
pub const TODO_GATE_DIGEST_PREFIX: &str = "[automated todo quality review - not a user message] Before you treat this turn as finished, double-check the weak points it surfaced. Do not reply conversationally or wait for the user.";

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

/// Whether every completed todo group currently has a passing delivery
/// assessment. This is evaluated at turn finish, after the todo update has
/// already been persisted, so a weak assessment can block completion without
/// discarding the model's state transition.
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
            .is_some_and(delivery_state_passes)
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

/// Completed todos whose final confidence step jumped levels rather than
/// climbing through evidence-backed states. Older todo records may not have a
/// history, so they fall back to comparing planning and completion confidence.
pub fn spike_completed_todos(todos: &[TodoItem]) -> Vec<&TodoItem> {
    fn is_spike(from: ConfidenceState, to: ConfidenceState) -> bool {
        to.level().saturating_sub(from.level()) >= TODO_CONFIDENCE_SPIKE_LEVELS
    }
    todos
        .iter()
        .filter(|todo| todo.status == "completed")
        .filter(|todo| match todo.confidence_history.as_slice() {
            [] => todo
                .confidence
                .zip(todo.completion_confidence)
                .is_some_and(|(first, last)| is_spike(first, last)),
            [_] => false,
            history => is_spike(history[history.len() - 2], history[history.len() - 1]),
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
/// so the recheck targets those items.
pub fn build_todo_confidence_spike_continuation_message(todos: &[TodoItem]) -> String {
    let spiked = spike_completed_todos(todos);
    let mut message = String::from(TODO_CONFIDENCE_SPIKE_CONTINUATION_MESSAGE);
    append_named_todos(&mut message, "Recheck:", &spiked);
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
        || trimmed.starts_with(LEGACY_TODO_HILL_CLIMBABILITY_CONTINUATION_MESSAGE)
        || trimmed.starts_with(LEGACY_TODO_ALIGNMENT_CONTINUATION_MESSAGE)
        || trimmed.starts_with(TODO_INTENT_UNDERSTANDING_CONTINUATION_MESSAGE)
        || trimmed.starts_with(TODO_OWNERSHIP_CONTINUATION_MESSAGE)
        || trimmed.starts_with(LEGACY_TODO_OWNERSHIP_CONTINUATION_MESSAGE)
        || trimmed.starts_with(TODO_COMPLETION_CONTINUATION_MESSAGE)
        || trimmed.starts_with(TODO_CONFIDENCE_SPIKE_CONTINUATION_MESSAGE)
        || trimmed.starts_with(TODO_FINAL_RESPONSE_CONTINUATION_MESSAGE)
        || trimmed.starts_with(LEGACY_TODO_COMPLETION_CONTINUATION_MESSAGE)
        || trimmed.starts_with(LEGACY_TODO_CONFIDENCE_SPIKE_CONTINUATION_MESSAGE)
        || trimmed.starts_with(LEGACY_TODO_CONFIDENCE_SUMMARY_PREFIX)
        || trimmed.starts_with(TODO_GATE_DIGEST_PREFIX)
        || trimmed.starts_with(TODO_LONG_SESSION_REVIEW_MESSAGE)
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
        || trimmed.starts_with(LEGACY_TODO_CONFIDENCE_SPIKE_CONTINUATION_MESSAGE)
    {
        return Some("🔍 Double-checking a confidence jump for you...");
    }
    if trimmed.starts_with(TODO_FINAL_RESPONSE_CONTINUATION_MESSAGE) {
        return Some("✅ Preparing the final response...");
    }
    if trimmed.starts_with(TODO_COMPLETION_CONTINUATION_MESSAGE)
        || trimmed.starts_with(LEGACY_TODO_COMPLETION_CONTINUATION_MESSAGE)
        || trimmed.starts_with(LEGACY_TODO_CONFIDENCE_SUMMARY_PREFIX)
    {
        return Some("🔍 Double-checking confidence for you...");
    }
    if trimmed.starts_with(TODO_GATE_DIGEST_PREFIX) {
        return Some("🔍 Reviewing the weak points of this turn for you...");
    }
    if trimmed.starts_with(TODO_LONG_SESSION_REVIEW_MESSAGE) {
        return Some("🔍 Rechecking the plan and assessments after extended work...");
    }
    if trimmed.starts_with(TODO_OWNERSHIP_CONTINUATION_MESSAGE)
        || trimmed.starts_with(LEGACY_TODO_OWNERSHIP_CONTINUATION_MESSAGE)
    {
        return Some("🔍 Checking the delivery state of the finished work...");
    }
    if trimmed.starts_with(TODO_INTENT_UNDERSTANDING_CONTINUATION_MESSAGE) {
        return Some("🔍 Re-checking the request was understood...");
    }
    if trimmed.starts_with(TODO_CLOSED_FEEDBACK_LOOP_CONTINUATION_MESSAGE)
        || trimmed.starts_with(LEGACY_TODO_HILL_CLIMBABILITY_CONTINUATION_MESSAGE)
        || trimmed.starts_with(LEGACY_TODO_ALIGNMENT_CONTINUATION_MESSAGE)
    {
        return Some("🔍 Asking for a stronger way to verify this work...");
    }
    // Incomplete-todos poke: the count is genuinely useful, and it is already
    // short, so it keeps its own text.
    None
}

pub fn load_todos(session_id: &str) -> Result<Vec<TodoItem>> {
    let path = todo_path(session_id)?;
    if !path.exists() {
        return Ok(Vec::new());
    }
    storage::read_json(&path).or_else(|_| Ok(Vec::new()))
}

pub fn todos_exist(session_id: &str) -> Result<bool> {
    Ok(todo_path(session_id)?.exists())
}

pub fn save_todos(session_id: &str, todos: &[TodoItem]) -> Result<()> {
    let path = todo_path(session_id)?;
    storage::write_json_fast(&path, todos)
}

fn todo_path(session_id: &str) -> Result<PathBuf> {
    let base = storage::jcode_dir()?;
    Ok(base.join("todos").join(format!("{}.json", session_id)))
}

fn todo_review_path(session_id: &str) -> Result<PathBuf> {
    let base = storage::jcode_dir()?;
    Ok(base
        .join("todos")
        .join(format!("{}-review-state.json", session_id)))
}

/// Record the beginning of a fresh todo cycle without exposing timing metadata
/// through the model-facing todo payload. Replacing a fully completed list with
/// new open work starts a new cycle; ordinary edits retain the original clock.
pub fn update_todo_review_cycle(
    session_id: &str,
    previous: &[TodoItem],
    incoming: &[TodoItem],
) -> Result<()> {
    if incoming.is_empty() {
        return Ok(());
    }
    let path = todo_review_path(session_id)?;
    let previous_complete = !previous.is_empty()
        && previous
            .iter()
            .all(|todo| todo.status.eq_ignore_ascii_case("completed"));
    let incoming_has_open = incoming
        .iter()
        .any(|todo| !todo.status.eq_ignore_ascii_case("completed"));
    if !path.exists() || (previous_complete && incoming_has_open) {
        storage::write_json_fast(
            &path,
            &TodoReviewState {
                cycle_started_at: chrono::Utc::now(),
                review_delivered: false,
            },
        )?;
    }
    Ok(())
}

/// Atomically decide and mark whether the one-shot long-session assessment
/// review is due. Marking before queueing prevents reloads from duplicating it.
pub fn take_long_session_review_if_due(session_id: &str) -> Result<bool> {
    let path = todo_review_path(session_id)?;
    if !path.exists() {
        return Ok(false);
    }
    let mut state: TodoReviewState = storage::read_json(&path)?;
    if state.review_delivered
        || chrono::Utc::now() - state.cycle_started_at < TODO_LONG_SESSION_REVIEW_AFTER
    {
        return Ok(false);
    }
    state.review_delivered = true;
    storage::write_json_fast(&path, &state)?;
    Ok(true)
}

/// Goal-level assessments live beside the todo list in a separate file so the
/// todo list format (a bare `Vec<TodoItem>` array) stays readable by every
/// existing consumer.
pub fn load_goals(session_id: &str) -> Result<Vec<TodoGoal>> {
    let path = goals_path(session_id)?;
    if !path.exists() {
        return Ok(Vec::new());
    }
    storage::read_json(&path).or_else(|_| Ok(Vec::new()))
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

/// Load todo state for a session and derive its best title hint.
pub fn load_session_title(session_id: &str) -> Option<String> {
    let todos = load_todos(session_id).ok()?;
    let plan = load_plan(session_id).unwrap_or_default();
    derive_session_title(&todos, &plan)
}

pub fn save_goals(session_id: &str, goals: &[TodoGoal]) -> Result<()> {
    let path = goals_path(session_id)?;
    storage::write_json_fast(&path, goals)
}

fn goals_path(session_id: &str) -> Result<PathBuf> {
    let base = storage::jcode_dir()?;
    Ok(base
        .join("todos")
        .join(format!("{}-goals.json", session_id)))
}

/// The plan-level intent assessment lives in its own file beside the todo list
/// and per-group goals, so each format stays independently readable.
pub fn load_plan(session_id: &str) -> Result<TodoPlan> {
    let path = plan_path(session_id)?;
    if !path.exists() {
        return Ok(TodoPlan::default());
    }
    storage::read_json(&path).or_else(|_| Ok(TodoPlan::default()))
}

pub fn save_plan(session_id: &str, plan: &TodoPlan) -> Result<()> {
    let path = plan_path(session_id)?;
    storage::write_json_fast(&path, plan)
}

fn plan_path(session_id: &str) -> Result<PathBuf> {
    let base = storage::jcode_dir()?;
    Ok(base.join("todos").join(format!("{}-plan.json", session_id)))
}

/// Deferred quality-check observations for the current turn.
///
/// Kept in its own file for the same reason goals and plan are: each format
/// stays independently readable. This one is turn-scoped rather than durable,
/// cleared once the digest has been delivered.
pub fn load_gate_observations(session_id: &str) -> Result<Vec<GateObservation>> {
    let path = gate_observations_path(session_id)?;
    if !path.exists() {
        return Ok(Vec::new());
    }
    storage::read_json(&path).or_else(|_| Ok(Vec::new()))
}

pub fn save_gate_observations(session_id: &str, observations: &[GateObservation]) -> Result<()> {
    let path = gate_observations_path(session_id)?;
    storage::write_json_fast(&path, observations)
}

/// Append this write's observations, capped so a very long iterative turn
/// cannot grow the file without bound. The digest collapses repeats anyway, so
/// dropping the oldest entries past the cap costs no information the reminder
/// would have used.
pub fn append_gate_observations(session_id: &str, new: &[GateObservation]) -> Result<()> {
    if new.is_empty() {
        return Ok(());
    }
    let mut observations = load_gate_observations(session_id).unwrap_or_default();
    observations.extend(new.iter().cloned());
    if observations.len() > MAX_GATE_OBSERVATIONS {
        let excess = observations.len() - MAX_GATE_OBSERVATIONS;
        observations.drain(0..excess);
    }
    save_gate_observations(session_id, &observations)
}

pub fn clear_gate_observations(session_id: &str) -> Result<()> {
    let path = gate_observations_path(session_id)?;
    if path.exists() {
        std::fs::remove_file(&path)?;
    }
    Ok(())
}

/// Upper bound on retained observations per turn.
const MAX_GATE_OBSERVATIONS: usize = 256;

fn gate_observations_path(session_id: &str) -> Result<PathBuf> {
    let base = storage::jcode_dir()?;
    Ok(base
        .join("todos")
        .join(format!("{}-gate-observations.json", session_id)))
}

#[cfg(test)]
mod tests {
    use super::*;

    fn intent_observation(state: Option<IntentUnderstanding>) -> GateObservation {
        GateObservation {
            kind: GateObservationKind::IntentUnderstanding,
            group: None,
            state: state.map(|state| state.as_str().to_string()),
        }
    }

    fn loop_observation(group: Option<&str>, state: Option<FeedbackLoopState>) -> GateObservation {
        GateObservation {
            kind: GateObservationKind::ClosedFeedbackLoop,
            group: group.map(str::to_string),
            state: state.map(|state| state.as_str().to_string()),
        }
    }

    #[test]
    fn substitute_and_blocked_checks_do_not_pass_involved_acceptance_gate() {
        let goal = |relevance| TodoGoal {
            difficulty: Some(Difficulty::Involved),
            feedback_loop_relevance: Some(relevance),
            ..Default::default()
        };

        assert!(!feedback_loop_relevance_passes(&goal(
            FeedbackLoopRelevance::Synthetic
        )));
        assert!(!feedback_loop_relevance_passes(&goal(
            FeedbackLoopRelevance::Representative
        )));
        assert!(!feedback_loop_relevance_passes(&goal(
            FeedbackLoopRelevance::AcceptanceBlocked
        )));
        assert!(feedback_loop_relevance_passes(&goal(
            FeedbackLoopRelevance::AcceptanceAligned
        )));
    }

    /// A score that climbed only after work was underway still gets raised, and
    /// is described as the coverage gap it is. Suppressing it would let an agent
    /// clear the gate by writing a good assessment at the end, after the work it
    /// was supposed to govern was already done and booked.
    #[test]
    fn digest_still_raises_a_point_whose_score_climbed_late() {
        let observations = vec![
            intent_observation(Some(IntentUnderstanding::Uncertain)),
            intent_observation(Some(IntentUnderstanding::Partial)),
            intent_observation(Some(IntentUnderstanding::Partial)),
        ];
        let climbed = TodoPlan {
            understands_user_intent: Some(IntentUnderstanding::Clear),
            understands_user_intent_history: vec![
                IntentUnderstanding::Uncertain,
                IntentUnderstanding::Partial,
                IntentUnderstanding::Clear,
            ],
            ..Default::default()
        };
        let digest = build_gate_digest(&observations, &climbed, &[])
            .expect("a late climb must still be raised, not silently dropped");
        assert_eq!(digest.matches("\n- ").count(), 1);
        // Worded as "you started without understanding", not "you never
        // understood": the latter would contradict the passing final state and
        // invite the model to argue with the reminder instead of acting on it.
        assert!(digest.contains("started this work without understanding"));
        assert!(digest.contains("(flagged 3 times this turn)"));
        assert!(!digest.contains("never became solid"));
    }

    /// The late-climb wording is per point, so one turn can carry both a goal
    /// that closed its loop late and a goal that never closed one at all.
    #[test]
    fn digest_words_late_climbs_and_never_closed_goals_differently() {
        let observations = vec![
            loop_observation(Some("closed late"), Some(FeedbackLoopState::Usable)),
            loop_observation(Some("never closed"), Some(FeedbackLoopState::Usable)),
        ];
        let goals = vec![
            TodoGoal {
                group: Some("closed late".to_string()),
                closed_feedback_loop: Some(FeedbackLoopState::Closed),
                ..Default::default()
            },
            TodoGoal {
                group: Some("never closed".to_string()),
                closed_feedback_loop: Some(FeedbackLoopState::Usable),
                ..Default::default()
            },
        ];
        let digest = build_gate_digest(&observations, &TodoPlan::default(), &goals)
            .expect("both goals should be surfaced");
        assert_eq!(digest.matches("\n- ").count(), 2);
        assert!(digest.contains(
            "the goal for \"closed late\" was worked on before its feedback loop was closed"
        ));
        assert!(digest.contains("the goal for \"never closed\" never closed its feedback loop"));
    }

    #[test]
    fn digest_directs_weak_relevance_and_coverage_to_real_failure_surfaces() {
        let observations = vec![
            GateObservation {
                kind: GateObservationKind::FeedbackLoopRelevance,
                group: Some("release".to_string()),
                state: Some("indirect".to_string()),
            },
            GateObservation {
                kind: GateObservationKind::FeedbackLoopCoverage,
                group: Some("release".to_string()),
                state: Some("narrow".to_string()),
            },
        ];
        let digest = build_gate_digest(&observations, &TodoPlan::default(), &[])
            .expect("weak feedback-loop dimensions should be surfaced");

        for guidance in [
            "public interfaces",
            "integration boundaries",
            "custom harness",
            "externally blocked",
            "main workflows",
            "edge cases",
            "packaging",
            "likely failure modes",
        ] {
            assert!(
                digest.contains(guidance),
                "digest omitted {guidance}: {digest}"
            );
        }
        assert!(!digest.to_ascii_lowercase().contains("threshold"));
        assert!(!digest.contains("representative+main_paths"));
    }

    #[test]
    fn digest_reports_a_point_that_never_resolved() {
        let observations = vec![intent_observation(Some(IntentUnderstanding::Partial))];
        let unresolved = TodoPlan {
            understands_user_intent: Some(IntentUnderstanding::Partial),
            understands_user_intent_history: vec![IntentUnderstanding::Partial],
            ..Default::default()
        };
        let digest = build_gate_digest(&observations, &unresolved, &[])
            .expect("an unresolved point should be surfaced");
        assert!(digest.starts_with(TODO_GATE_DIGEST_PREFIX));
        assert!(digest.contains("what the user actually wants"));
        // Framed as verification, since by turn end the work is already done.
        assert!(digest.contains("double-check"));
        // Private calibration stays private.
        assert!(!digest.to_ascii_lowercase().contains("threshold"));
    }

    /// A long iterative turn flags the same point on every write. The digest
    /// must collapse those into one line, not a wall of duplicates.
    #[test]
    fn digest_collapses_repeats_and_counts_them() {
        let observations: Vec<GateObservation> = (0..9)
            .map(|_| loop_observation(Some("utf16 transcode"), Some(FeedbackLoopState::Strong)))
            .collect();
        let goals = vec![TodoGoal {
            group: Some("utf16 transcode".to_string()),
            closed_feedback_loop: Some(FeedbackLoopState::Strong),
            ..Default::default()
        }];
        let digest = build_gate_digest(&observations, &TodoPlan::default(), &goals)
            .expect("an unresolved goal should be surfaced");
        assert_eq!(
            digest.matches("\n- ").count(),
            1,
            "nine identical flags should collapse to one line: {digest}"
        );
        assert!(digest.contains("flagged 9 times"));
        assert!(digest.contains("utf16 transcode"));
    }

    /// Each goal gets its own line, named by group, so a multi-goal turn does
    /// not blur two different problems into one instruction.
    #[test]
    fn digest_separates_goals_by_group() {
        let observations = vec![
            loop_observation(Some("transcode"), Some(FeedbackLoopState::Usable)),
            loop_observation(Some("render"), Some(FeedbackLoopState::Usable)),
            loop_observation(Some("render"), Some(FeedbackLoopState::Usable)),
        ];
        let goals = vec![
            TodoGoal {
                group: Some("transcode".to_string()),
                closed_feedback_loop: Some(FeedbackLoopState::Usable),
                ..Default::default()
            },
            TodoGoal {
                group: Some("render".to_string()),
                closed_feedback_loop: Some(FeedbackLoopState::Usable),
                ..Default::default()
            },
        ];
        let digest = build_gate_digest(&observations, &TodoPlan::default(), &goals)
            .expect("both goals should be surfaced");
        assert_eq!(digest.matches("\n- ").count(), 2);
        assert!(digest.contains("transcode"));
        assert!(digest.contains("render"));
        // The repeated goal is collapsed and counted, the single one is not.
        assert!(digest.contains("(flagged 2 times this turn)"));
    }

    /// An ungrouped goal has no label to name, so the line must still read
    /// cleanly rather than rendering an empty quoted string.
    #[test]
    fn digest_handles_the_ungrouped_goal() {
        let digest = build_gate_digest(
            &[loop_observation(None, Some(FeedbackLoopState::Absent))],
            &TodoPlan::default(),
            &[TodoGoal {
                closed_feedback_loop: Some(FeedbackLoopState::Absent),
                ..Default::default()
            }],
        )
        .expect("ungrouped goal should be surfaced");
        assert!(!digest.contains("\"\""));
        assert!(digest.contains("the goal never closed its feedback loop"));
    }

    #[test]
    fn digest_is_empty_without_observations() {
        assert_eq!(build_gate_digest(&[], &TodoPlan::default(), &[]), None);
    }

    /// The digest is persisted as a user-role message so the model treats it as
    /// a continuation, so reload must not re-render it as a user prompt.
    #[test]
    fn digest_is_recognized_as_a_synthetic_message() {
        let digest = build_gate_digest(
            &[intent_observation(Some(IntentUnderstanding::Partial))],
            &TodoPlan::default(),
            &[],
        )
        .expect("digest");
        assert!(is_auto_poke_message(&digest));
    }

    #[test]
    fn gate_observations_round_trip_and_clear() {
        let _guard = crate::storage::lock_test_env();
        let previous_home = std::env::var_os("JCODE_HOME");
        let dir = tempfile::TempDir::new().expect("tempdir");
        crate::env::set_var("JCODE_HOME", dir.path());

        let session = "gate-observation-round-trip";
        assert!(
            load_gate_observations(session)
                .expect("load empty")
                .is_empty()
        );
        append_gate_observations(
            session,
            &[intent_observation(Some(IntentUnderstanding::Partial))],
        )
        .expect("append");
        append_gate_observations(
            session,
            &[loop_observation(
                Some("perf"),
                Some(FeedbackLoopState::Strong),
            )],
        )
        .expect("append");
        let stored = load_gate_observations(session).expect("load");
        assert_eq!(stored.len(), 2);
        assert_eq!(stored[0].kind, GateObservationKind::IntentUnderstanding);
        assert_eq!(stored[1].group.as_deref(), Some("perf"));

        clear_gate_observations(session).expect("clear");
        assert!(load_gate_observations(session).expect("reload").is_empty());
        // Clearing an absent log is not an error, since the digest path clears
        // unconditionally.
        clear_gate_observations(session).expect("clear again");

        match previous_home {
            Some(value) => crate::env::set_var("JCODE_HOME", value),
            None => crate::env::remove_var("JCODE_HOME"),
        }
    }

    /// A very long turn must not grow the log without bound. Repeats collapse in
    /// the digest anyway, so dropping the oldest costs nothing it would report.
    #[test]
    fn gate_observation_log_is_capped() {
        let _guard = crate::storage::lock_test_env();
        let previous_home = std::env::var_os("JCODE_HOME");
        let dir = tempfile::TempDir::new().expect("tempdir");
        crate::env::set_var("JCODE_HOME", dir.path());

        let session = "gate-observation-cap";
        let batch: Vec<GateObservation> = (0..MAX_GATE_OBSERVATIONS + 50)
            .map(|_| intent_observation(Some(IntentUnderstanding::Partial)))
            .collect();
        append_gate_observations(session, &batch).expect("append");
        assert_eq!(
            load_gate_observations(session).expect("load").len(),
            MAX_GATE_OBSERVATIONS
        );

        match previous_home {
            Some(value) => crate::env::set_var("JCODE_HOME", value),
            None => crate::env::remove_var("JCODE_HOME"),
        }
    }

    #[test]
    fn built_auto_poke_messages_are_detected() {
        assert!(is_auto_poke_message(&build_auto_poke_message(1)));
        assert!(is_auto_poke_message(&build_auto_poke_message(3)));
        assert!(is_auto_poke_message(
            TODO_CLOSED_FEEDBACK_LOOP_CONTINUATION_MESSAGE
        ));
        assert!(is_auto_poke_message(
            LEGACY_TODO_ALIGNMENT_CONTINUATION_MESSAGE
        ));
        assert!(is_auto_poke_message(
            TODO_INTENT_UNDERSTANDING_CONTINUATION_MESSAGE
        ));
        assert!(is_auto_poke_message(TODO_OWNERSHIP_CONTINUATION_MESSAGE));
        assert!(is_auto_poke_message(TODO_COMPLETION_CONTINUATION_MESSAGE));
        assert!(is_auto_poke_message(
            TODO_CONFIDENCE_SPIKE_CONTINUATION_MESSAGE
        ));
        assert!(is_auto_poke_message(
            TODO_FINAL_RESPONSE_CONTINUATION_MESSAGE
        ));
        assert_eq!(
            auto_poke_display_summary(TODO_FINAL_RESPONSE_CONTINUATION_MESSAGE),
            Some("✅ Preparing the final response...")
        );
        assert!(is_auto_poke_message(LEGACY_TODO_CONFIDENCE_SUMMARY_PREFIX));
    }

    #[test]
    fn quality_continuations_are_actionable_without_private_calibration() {
        for (message, category) in [
            (
                TODO_CLOSED_FEEDBACK_LOOP_CONTINUATION_MESSAGE,
                "feedback loop is not closed",
            ),
            (
                TODO_INTENT_UNDERSTANDING_CONTINUATION_MESSAGE,
                "understanding of the user's intent",
            ),
            (TODO_OWNERSHIP_CONTINUATION_MESSAGE, "continue the work"),
            (TODO_COMPLETION_CONTINUATION_MESSAGE, "more validation"),
            (
                TODO_CONFIDENCE_SPIKE_CONTINUATION_MESSAGE,
                "independently recheck",
            ),
        ] {
            let lower = message.to_ascii_lowercase();
            assert!(lower.contains(category));
            assert!(!message.chars().any(|ch| ch.is_ascii_digit()));
            for disclosure in ["threshold", "percent", "quality gate"] {
                assert!(
                    !lower.contains(disclosure),
                    "category-only continuation disclosed {disclosure}: {message}"
                );
            }
            if category != "alignment score" {
                assert!(
                    !lower.contains("score"),
                    "category-only continuation disclosed score: {message}"
                );
            }
        }

        assert!(TODO_CLOSED_FEEDBACK_LOOP_CONTINUATION_MESSAGE.contains("strong feedback loop"));
        assert!(TODO_CLOSED_FEEDBACK_LOOP_CONTINUATION_MESSAGE.contains("First, improve"));
        assert!(
            TODO_CLOSED_FEEDBACK_LOOP_CONTINUATION_MESSAGE.contains("call the todo tool again")
        );
        assert!(
            TODO_CLOSED_FEEDBACK_LOOP_CONTINUATION_MESSAGE.contains("before continuing the task")
        );
        // Deliberately terse: think harder about intent, never block on the user.
        assert!(TODO_INTENT_UNDERSTANDING_CONTINUATION_MESSAGE.contains("think harder"));
        assert!(
            TODO_INTENT_UNDERSTANDING_CONTINUATION_MESSAGE.contains("what the user actually wants")
        );
        assert!(TODO_INTENT_UNDERSTANDING_CONTINUATION_MESSAGE.contains("Do not ask the user"));
        for message in [
            TODO_OWNERSHIP_CONTINUATION_MESSAGE,
            TODO_COMPLETION_CONTINUATION_MESSAGE,
            TODO_CONFIDENCE_SPIKE_CONTINUATION_MESSAGE,
        ] {
            let lower = message.to_ascii_lowercase();
            for evaluator_term in ["gate", "flagged", "failed", "threshold", "confidence"] {
                assert!(
                    !lower.contains(evaluator_term),
                    "disclosed {evaluator_term}: {message}"
                );
            }
        }
    }

    /// The model must be told which items it should recheck, otherwise the
    /// nudge is a guess-the-weak-spot puzzle. Scores stay private.
    #[test]
    fn completion_continuation_names_the_failing_todos() {
        let mut missing = todo("write the parser", "completed", None);
        missing.completion_confidence = None;
        let mut weak = todo("wire up the CLI flag", "completed", None);
        weak.completion_confidence = Some(ConfidenceState::Plausible);
        let mut strong = todo("rename the module", "completed", None);
        strong.completion_confidence = Some(ConfidenceState::Validated);
        let open = todo("ship it", "in_progress", None);

        let message =
            build_todo_completion_continuation_message(&[missing, weak, strong.clone(), open]);
        assert!(message.starts_with(TODO_COMPLETION_CONTINUATION_MESSAGE));
        assert!(message.contains("\"write the parser\""));
        assert!(message.contains("\"wire up the CLI flag\""));
        // Passing and unfinished todos are not what the gate is asking about.
        assert!(!message.contains("\"rename the module\""));
        assert!(!message.contains("\"ship it\""));
        for disclosure in ["70", "99", "threshold"] {
            assert!(!message.contains(disclosure), "disclosed {disclosure}");
        }

        // Only the weighted average failed: no individual item is nameable, so
        // the fallback still tells the model what to do.
        let average_only = build_todo_completion_continuation_message(&[strong]);
        assert!(average_only.starts_with(TODO_COMPLETION_CONTINUATION_MESSAGE));
        assert!(average_only.contains("Validate further:"));
        assert!(average_only.contains("rename the module"));
    }

    #[test]
    fn spike_continuation_names_the_spiked_todos() {
        let mut spiked = todo("port the tests", "completed", None);
        spiked.completion_confidence = Some(ConfidenceState::Verified);
        spiked.confidence_history = vec![ConfidenceState::Plausible, ConfidenceState::Verified];
        let mut steady = todo("update docs", "completed", None);
        steady.completion_confidence = Some(ConfidenceState::Validated);
        steady.confidence_history = vec![ConfidenceState::Plausible, ConfidenceState::Validated];

        let message = build_todo_confidence_spike_continuation_message(&[spiked, steady]);
        assert!(message.starts_with(TODO_CONFIDENCE_SPIKE_CONTINUATION_MESSAGE));
        assert!(message.contains("\"port the tests\""));
        assert!(!message.contains("\"update docs\""));
    }

    /// A 40-item plan must not turn one gate nudge into a wall of text.
    #[test]
    fn completion_continuation_caps_named_todos() {
        let todos: Vec<TodoItem> = (0..40)
            .map(|index| {
                let mut item = todo(&format!("task {index}"), "completed", None);
                item.completion_confidence = None;
                item
            })
            .collect();
        let message = build_todo_completion_continuation_message(&todos);
        assert!(message.contains("\"task 0\""));
        assert!(!message.contains("\"task 30\""));
        assert!(message.contains(&format!("(and {} more)", 40 - GATE_NAMED_TODO_LIMIT)));
    }

    #[test]
    fn detailed_gate_continuations_are_not_mistaken_for_user_messages() {
        let mut item = todo("do the thing", "completed", None);
        item.completion_confidence = Some(ConfidenceState::Speculative);
        item.confidence_history = vec![ConfidenceState::Speculative];
        let todos = [item];
        assert!(is_auto_poke_message(
            &build_todo_completion_continuation_message(&todos)
        ));
        assert!(is_auto_poke_message(
            &build_todo_confidence_spike_continuation_message(&todos)
        ));
    }

    /// The transcript must not replay model-facing gate instructions at the
    /// user; they get a short "we are checking" line instead.
    #[test]
    fn gate_continuations_have_short_user_facing_summaries() {
        let mut item = todo("do the thing", "completed", None);
        item.completion_confidence = Some(ConfidenceState::Speculative);
        item.confidence_history = vec![ConfidenceState::Speculative];
        let todos = [item];

        for message in [
            build_todo_completion_continuation_message(&todos),
            build_todo_confidence_spike_continuation_message(&todos),
            TODO_OWNERSHIP_CONTINUATION_MESSAGE.to_string(),
            TODO_INTENT_UNDERSTANDING_CONTINUATION_MESSAGE.to_string(),
            TODO_CLOSED_FEEDBACK_LOOP_CONTINUATION_MESSAGE.to_string(),
        ] {
            let summary = auto_poke_display_summary(&message).expect("summary");
            assert!(summary.chars().count() < 70, "too long: {summary}");
            assert!(!summary.contains("do the thing"));
            assert!(!summary.contains("completion_confidence"));
        }

        // The incomplete-todos poke is already short and its count is useful.
        assert!(auto_poke_display_summary(&build_auto_poke_message(3)).is_none());
        // Real user prompts are never rewritten.
        assert!(auto_poke_display_summary("fix the login bug").is_none());
    }

    #[test]
    fn confidence_spike_classifier_distinguishes_bulk_stamp_from_stepped_rise() {
        let mut bulk = todo("bulk", "completed", None);
        bulk.confidence = Some(ConfidenceState::Plausible);
        bulk.completion_confidence = Some(ConfidenceState::Verified);
        bulk.confidence_history = vec![ConfidenceState::Plausible, ConfidenceState::Verified];

        let mut stepped = todo("stepped", "completed", None);
        stepped.confidence = Some(ConfidenceState::Verified);
        stepped.completion_confidence = Some(ConfidenceState::Verified);
        stepped.confidence_history = vec![
            ConfidenceState::Plausible,
            ConfidenceState::Validated,
            ConfidenceState::Verified,
        ];

        let todos = [bulk, stepped];
        let spiked = spike_completed_todos(&todos);
        assert_eq!(spiked.len(), 1);
        assert_eq!(spiked[0].content, "bulk");
    }

    #[test]
    fn confidence_spike_classifier_includes_boundary_and_legacy_fallback() {
        let mut boundary = todo("boundary", "completed", None);
        boundary.confidence = Some(ConfidenceState::Plausible);
        boundary.completion_confidence = Some(ConfidenceState::Verified);
        boundary.confidence_history = vec![ConfidenceState::Plausible, ConfidenceState::Verified];

        let mut legacy = todo("legacy", "completed", None);
        legacy.confidence = Some(ConfidenceState::Speculative);
        legacy.completion_confidence = Some(ConfidenceState::Verified);

        let todos = [boundary, legacy];
        let spiked = spike_completed_todos(&todos);
        assert_eq!(
            spiked
                .iter()
                .map(|todo| todo.content.as_str())
                .collect::<Vec<_>>(),
            vec!["boundary", "legacy"]
        );
    }

    #[test]
    fn real_user_prompts_are_not_detected_as_pokes() {
        assert!(!is_auto_poke_message("fix the login bug"));
        assert!(!is_auto_poke_message(
            "You have 2 incomplete todos. Continue working, or update the todo tool.\n\nalso please fix the tests"
        ));
        assert!(!is_auto_poke_message(""));
    }

    fn todo(content: &str, status: &str, group: Option<&str>) -> TodoItem {
        TodoItem {
            content: content.to_string(),
            status: status.to_string(),
            priority: "high".to_string(),
            id: content.to_ascii_lowercase().replace(' ', "-"),
            group: group.map(str::to_string),
            confidence: None,
            completion_confidence: None,
            confidence_history: Vec::new(),
            blocked_by: Vec::new(),
            assigned_to: None,
        }
    }

    #[test]
    fn long_session_review_is_private_durable_and_one_shot() {
        let _guard = storage::lock_test_env();
        let previous_home = std::env::var_os("JCODE_HOME");
        let dir = tempfile::TempDir::new().expect("tempdir");
        crate::env::set_var("JCODE_HOME", dir.path());
        let session = "long-review-one-shot";
        let todos = vec![todo("work", "in_progress", Some("ship"))];

        update_todo_review_cycle(session, &[], &todos).expect("start cycle");
        assert!(!take_long_session_review_if_due(session).expect("fresh cycle"));

        let path = todo_review_path(session).expect("review path");
        storage::write_json_fast(
            &path,
            &TodoReviewState {
                cycle_started_at: chrono::Utc::now()
                    - TODO_LONG_SESSION_REVIEW_AFTER
                    - chrono::Duration::seconds(1),
                review_delivered: false,
            },
        )
        .expect("age cycle");
        assert!(take_long_session_review_if_due(session).expect("due review"));
        assert!(!take_long_session_review_if_due(session).expect("one shot"));
        assert!(!TODO_LONG_SESSION_REVIEW_MESSAGE.contains("30"));
        assert!(
            !TODO_LONG_SESSION_REVIEW_MESSAGE
                .to_ascii_lowercase()
                .contains("threshold")
        );
        assert!(is_auto_poke_message(TODO_LONG_SESSION_REVIEW_MESSAGE));

        match previous_home {
            Some(value) => crate::env::set_var("JCODE_HOME", value),
            None => crate::env::remove_var("JCODE_HOME"),
        }
    }

    fn delivery_goal(group: Option<&str>, delivery: Option<DeliveryState>) -> TodoGoal {
        TodoGoal {
            group: group.map(str::to_string),
            delivery_state: delivery,
            autonomy: Some(Autonomy::NecessaryFollowthrough),
            iteration_maturity: Some(IterationMaturity::OutcomeReached),
            feedback_loop_relevance: Some(FeedbackLoopRelevance::Representative),
            feedback_loop_coverage: Some(FeedbackLoopCoverage::MainPaths),
            feedback_loop_traceability: Some(FeedbackLoopTraceability::Complete),
            ..Default::default()
        }
    }

    #[test]
    fn newly_completed_group_requires_sufficient_delivery() {
        let previous = vec![todo("work", "in_progress", Some("ship"))];
        let completed = vec![todo("work", "completed", Some("ship"))];

        for delivery in [
            None,
            Some(DeliveryState::ChangeMade),
            Some(DeliveryState::Integrated),
        ] {
            assert!(!newly_completed_groups_have_sufficient_delivery(
                &previous,
                &completed,
                &[delivery_goal(Some("ship"), delivery)],
            ));
        }
        assert!(newly_completed_groups_have_sufficient_delivery(
            &previous,
            &completed,
            &[delivery_goal(
                Some("ship"),
                Some(DeliveryState::WorkflowValidated)
            )],
        ));
    }

    /// Difficulty never raises the delivery-state bar by itself. Operational
    /// outcome delivery comes from the request, not implementation difficulty.
    #[test]
    fn difficulty_is_descriptive_and_never_raises_the_delivery_bar() {
        let previous = vec![todo("work", "in_progress", Some("ship"))];
        let completed = vec![todo("work", "completed", Some("ship"))];

        let mut hard = delivery_goal(Some("ship"), Some(DeliveryState::WorkflowValidated));
        hard.difficulty = Some(Difficulty::Hard);
        hard.feedback_loop_relevance = Some(FeedbackLoopRelevance::AcceptanceAligned);
        hard.feedback_loop_coverage = Some(FeedbackLoopCoverage::EdgeAndIntegrationPaths);
        assert!(newly_completed_groups_have_sufficient_delivery(
            &previous,
            &completed,
            &[hard.clone()],
        ));
        hard.delivery_state = Some(DeliveryState::OutcomeDelivered);
        assert!(newly_completed_groups_have_sufficient_delivery(
            &previous,
            &completed,
            &[hard],
        ));

        // A trivial goal, and one with no difficulty at all, pass at
        // workflow_validated: absent difficulty is never punished.
        for difficulty in [None, Some(Difficulty::Trivial), Some(Difficulty::Routine)] {
            let mut goal = delivery_goal(Some("ship"), Some(DeliveryState::WorkflowValidated));
            goal.difficulty = difficulty;
            assert!(newly_completed_groups_have_sufficient_delivery(
                &previous,
                &completed,
                &[goal],
            ));
        }

        assert_eq!(
            required_delivery_state(Some(Difficulty::Involved)),
            DeliveryState::WorkflowValidated
        );
        assert_eq!(
            required_delivery_state(None),
            DeliveryState::WorkflowValidated
        );
    }

    #[test]
    fn feedback_loop_completion_bar_scales_at_involved_difficulty() {
        let mut ordinary = delivery_goal(None, Some(DeliveryState::WorkflowValidated));
        assert!(delivery_state_passes(&ordinary));
        ordinary.feedback_loop_relevance = Some(FeedbackLoopRelevance::Indirect);
        assert!(!delivery_state_passes(&ordinary));
        ordinary.feedback_loop_relevance = Some(FeedbackLoopRelevance::Representative);
        ordinary.feedback_loop_coverage = Some(FeedbackLoopCoverage::Narrow);
        assert!(!delivery_state_passes(&ordinary));

        let mut involved = delivery_goal(None, Some(DeliveryState::WorkflowValidated));
        involved.difficulty = Some(Difficulty::Involved);
        assert!(!delivery_state_passes(&involved));
        involved.feedback_loop_relevance = Some(FeedbackLoopRelevance::AcceptanceAligned);
        involved.feedback_loop_coverage = Some(FeedbackLoopCoverage::EdgeAndIntegrationPaths);
        assert!(delivery_state_passes(&involved));

        involved.feedback_loop_relevance = None;
        assert!(!delivery_state_passes(&involved));
        involved.feedback_loop_relevance = Some(FeedbackLoopRelevance::AcceptanceAligned);
        involved.feedback_loop_coverage = None;
        assert!(!delivery_state_passes(&involved));
    }

    #[test]
    fn feedback_loop_traceability_scales_at_involved_difficulty() {
        let mut goal = delivery_goal(None, Some(DeliveryState::WorkflowValidated));
        goal.feedback_loop_traceability = Some(FeedbackLoopTraceability::Partial);
        assert!(delivery_state_passes(&goal));

        goal.difficulty = Some(Difficulty::Involved);
        goal.feedback_loop_relevance = Some(FeedbackLoopRelevance::AcceptanceAligned);
        goal.feedback_loop_coverage = Some(FeedbackLoopCoverage::EdgeAndIntegrationPaths);
        assert!(!delivery_state_passes(&goal));

        goal.feedback_loop_traceability = Some(FeedbackLoopTraceability::Complete);
        assert!(delivery_state_passes(&goal));
        goal.feedback_loop_traceability = None;
        assert!(!delivery_state_passes(&goal));
    }

    #[test]
    fn research_completion_requires_stopping_evidence() {
        let previous = vec![todo("work", "in_progress", Some("ship"))];
        let completed = vec![todo("work", "completed", Some("ship"))];
        let mut goal = delivery_goal(Some("ship"), Some(DeliveryState::WorkflowValidated));
        goal.difficulty = Some(Difficulty::Research);
        goal.feedback_loop_relevance = Some(FeedbackLoopRelevance::AcceptanceAligned);
        goal.feedback_loop_coverage = Some(FeedbackLoopCoverage::EdgeAndIntegrationPaths);
        goal.iteration_maturity = Some(IterationMaturity::PlateauConfirmed);

        assert!(!newly_completed_groups_have_sufficient_delivery(
            &previous,
            &completed,
            &[goal.clone()],
        ));
        goal.stopping_evidence =
            Some("Three materially different approaches plateaued at the same score".to_string());
        assert!(newly_completed_groups_have_sufficient_delivery(
            &previous,
            &completed,
            &[goal],
        ));
    }

    #[test]
    fn autonomy_requires_necessary_followthrough_at_completion() {
        let previous = vec![todo("work", "in_progress", Some("ship"))];
        let completed = vec![todo("work", "completed", Some("ship"))];
        for autonomy in [None, Some(Autonomy::RequestedOnly)] {
            let mut goal = delivery_goal(Some("ship"), Some(DeliveryState::WorkflowValidated));
            goal.autonomy = autonomy;
            assert!(!newly_completed_groups_have_sufficient_delivery(
                &previous,
                &completed,
                &[goal],
            ));
        }
        for autonomy in [
            Autonomy::NecessaryFollowthrough,
            Autonomy::Proactive,
            Autonomy::Stewardship,
        ] {
            let mut goal = delivery_goal(Some("ship"), Some(DeliveryState::WorkflowValidated));
            goal.autonomy = Some(autonomy);
            assert!(newly_completed_groups_have_sufficient_delivery(
                &previous,
                &completed,
                &[goal],
            ));
        }
    }

    #[test]
    fn iteration_maturity_requires_terminal_basis_and_supporting_evidence() {
        let previous = vec![todo("work", "in_progress", Some("search"))];
        let completed = vec![todo("work", "completed", Some("search"))];
        for maturity in [
            None,
            Some(IterationMaturity::NotStarted),
            Some(IterationMaturity::Exploring),
            Some(IterationMaturity::Improving),
            Some(IterationMaturity::PlateauUnproven),
        ] {
            let mut goal = delivery_goal(Some("search"), Some(DeliveryState::WorkflowValidated));
            goal.iteration_maturity = maturity;
            assert!(!newly_completed_groups_have_sufficient_delivery(
                &previous,
                &completed,
                &[goal],
            ));
        }

        let mut plateau = delivery_goal(Some("search"), Some(DeliveryState::WorkflowValidated));
        plateau.iteration_maturity = Some(IterationMaturity::PlateauConfirmed);
        assert!(!newly_completed_groups_have_sufficient_delivery(
            &previous,
            &completed,
            &[plateau.clone()],
        ));
        plateau.stopping_evidence = Some(
            "Two distinct post-best approaches regressed under the same benchmark".to_string(),
        );
        assert!(newly_completed_groups_have_sufficient_delivery(
            &previous,
            &completed,
            &[plateau],
        ));

        let outcome = delivery_goal(Some("search"), Some(DeliveryState::WorkflowValidated));
        assert!(newly_completed_groups_have_sufficient_delivery(
            &previous,
            &completed,
            &[outcome],
        ));
    }

    #[test]
    fn delivery_is_not_required_before_group_completion() {
        let previous = vec![todo("work", "pending", Some("ship"))];
        let in_progress = vec![todo("work", "in_progress", Some("ship"))];

        assert!(newly_completed_groups_have_sufficient_delivery(
            &previous,
            &in_progress,
            &[],
        ));
    }

    #[test]
    fn delivery_gate_normalizes_groups_and_supports_ungrouped_work() {
        let previous = vec![todo("work", "in_progress", Some(" ship "))];
        let completed = vec![todo("work", "completed", Some("ship"))];
        assert!(newly_completed_groups_have_sufficient_delivery(
            &previous,
            &completed,
            &[delivery_goal(
                Some(" ship"),
                Some(DeliveryState::OutcomeDelivered)
            )],
        ));

        let previous = vec![todo("work", "in_progress", None)];
        let completed = vec![todo("work", "completed", None)];
        assert!(newly_completed_groups_have_sufficient_delivery(
            &previous,
            &completed,
            &[delivery_goal(None, Some(DeliveryState::OutcomeDelivered))],
        ));
    }

    /// The turn-finish gate must tell the model how to clear it without implying
    /// that the todo write which triggered the check was discarded.
    #[test]
    fn ownership_message_names_the_field_that_must_be_raised() {
        assert!(TODO_OWNERSHIP_CONTINUATION_MESSAGE.contains("Continue the work below"));
        for private_calibration in [
            "necessary_followthrough",
            "outcome_reached",
            "plateau_confirmed",
            "budget_exhausted",
        ] {
            assert!(!TODO_OWNERSHIP_CONTINUATION_MESSAGE.contains(private_calibration));
        }
        assert!(
            TODO_OWNERSHIP_CONTINUATION_MESSAGE.contains("Keep the todo up to date"),
            "the ownership nudge must say how to update the assessment"
        );
        assert!(
            !TODO_OWNERSHIP_CONTINUATION_MESSAGE.contains("rejected")
                && !TODO_OWNERSHIP_CONTINUATION_MESSAGE.contains("unchanged"),
            "the turn-finish nudge must not claim the already-saved write was discarded"
        );
        assert!(TODO_COMPLETION_CONTINUATION_MESSAGE.contains("more validation"));
    }

    #[test]
    fn ownership_continuation_identifies_each_failing_field_and_goal() {
        let todos = vec![todo("work", "completed", Some("ship"))];
        let mut goal = delivery_goal(Some("ship"), Some(DeliveryState::Integrated));
        goal.autonomy = Some(Autonomy::RequestedOnly);
        goal.iteration_maturity = Some(IterationMaturity::PlateauConfirmed);
        goal.stopping_evidence = None;

        let message = build_todo_ownership_continuation_message(&todos, &[goal]);
        assert!(message.contains("Goal \"ship\""));
        assert!(message.contains("complete workflow"));
        assert!(message.contains("ownership of the necessary follow-through"));
        assert!(message.contains("evidence about whether the work should stop"));
        assert!(!message.contains("workflow_validated"));
        assert!(!message.contains("necessary_followthrough"));
        assert!(!message.contains("outcome_reached"));
        // PlateauConfirmed is terminal, so it must not also be diagnosed as an
        // iteration_maturity failure. Its missing evidence is the exact defect.
        assert!(!message.contains("remaining hypotheses"));
        assert!(is_auto_poke_message(&message));
    }

    #[test]
    fn ownership_continuation_reports_missing_goal_assessment() {
        let todos = vec![todo("work", "completed", Some("ship"))];
        let message = build_todo_ownership_continuation_message(&todos, &[]);
        assert!(message.contains("Goal \"ship\": clarify the goal and track the work"));
    }

    #[test]
    fn completed_groups_require_sufficient_delivery_at_turn_finish() {
        let incomplete = vec![todo("work", "in_progress", Some("ship"))];
        assert!(completed_groups_have_sufficient_delivery(&incomplete, &[]));

        let completed = vec![todo("work", "completed", Some("ship"))];
        for delivery in [
            None,
            Some(DeliveryState::ChangeMade),
            Some(DeliveryState::Integrated),
        ] {
            assert!(!completed_groups_have_sufficient_delivery(
                &completed,
                &[delivery_goal(Some("ship"), delivery)],
            ));
        }
        assert!(completed_groups_have_sufficient_delivery(
            &completed,
            &[delivery_goal(
                Some("ship"),
                Some(DeliveryState::WorkflowValidated)
            )],
        ));
    }

    #[test]
    fn delivery_gate_grandfathers_preexisting_completed_groups() {
        let completed = vec![todo("legacy", "completed", Some("legacy"))];
        assert!(newly_completed_groups_have_sufficient_delivery(
            &completed,
            &completed,
            &[],
        ));
    }

    #[test]
    fn session_title_prefers_in_progress_todo_group() {
        let todos = vec![
            todo("old task", "pending", Some("Older goal")),
            todo("current task", "in_progress", Some("Fix resume names")),
            todo("later task", "pending", Some("Later goal")),
        ];

        assert_eq!(
            derive_session_title(&todos, &TodoPlan::default()).as_deref(),
            Some("Fix resume names")
        );
    }

    #[test]
    fn session_title_uses_latest_incomplete_group_when_nothing_is_active() {
        let todos = vec![
            todo("finished", "completed", Some("Old goal")),
            todo("next", "pending", Some("Current goal")),
        ];

        assert_eq!(
            derive_session_title(&todos, &TodoPlan::default()).as_deref(),
            Some("Current goal")
        );
    }

    #[test]
    fn ungrouped_session_title_prefers_plan_intention_then_item_content() {
        let todos = vec![todo("Run targeted tests", "in_progress", None)];
        let plan = TodoPlan {
            user_intention: Some("Keep resumed work easy to identify".to_string()),
            understands_user_intent: Some(IntentUnderstanding::Clear),
            ..Default::default()
        };

        assert_eq!(
            derive_session_title(&todos, &plan).as_deref(),
            Some("Keep resumed work easy to identify")
        );
        assert_eq!(
            derive_session_title(&todos, &TodoPlan::default()).as_deref(),
            Some("Run targeted tests")
        );
    }

    #[test]
    fn plan_intent_fields_round_trip_through_storage() {
        let _guard = crate::storage::lock_test_env();
        let previous_home = std::env::var_os("JCODE_HOME");
        let dir = tempfile::TempDir::new().expect("tempdir");
        crate::env::set_var("JCODE_HOME", dir.path());

        let plan = TodoPlan {
            user_intention: Some("Preserve why the user requested the work".to_string()),
            understands_user_intent: Some(IntentUnderstanding::Clear),
            ..Default::default()
        };
        save_plan("user-intention-round-trip", &plan).expect("save plan");
        let stored =
            std::fs::read_to_string(plan_path("user-intention-round-trip").expect("plan path"))
                .expect("read stored plan");
        assert!(stored.contains("\"understands_user_intent\""));
        assert!(!stored.contains("\"alignment_score\""));
        assert!(!stored.contains("\"user_intention_alignment\""));

        let loaded = load_plan("user-intention-round-trip").expect("load plan");
        assert_eq!(loaded, plan);

        match previous_home {
            Some(value) => crate::env::set_var("JCODE_HOME", value),
            None => crate::env::remove_var("JCODE_HOME"),
        }
    }
}
