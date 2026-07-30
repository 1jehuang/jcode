use crate::storage;
use anyhow::Result;
use serde::{Deserialize, Serialize};
use std::path::PathBuf;

pub use jcode_task_types::{
    TodoGoal, TodoGoalChange, TodoGoalField, TodoItem, TodoPlan, TodoPlanChange, TodoPlanField,
};

/// Minimum passing score for 0-100 quality assessments. Scores below this do
/// not provide enough evidence to clear their respective quality gate.
pub const QUALITY_GATE_THRESHOLD: u8 = 96;

/// Goals whose closed-feedback-loop score is strictly below this are considered
/// open-loop: no observation reports back whether the work satisfies the
/// requirements, so there is nothing credible to iterate against.
pub const LOW_CLOSED_FEEDBACK_LOOP: u8 = QUALITY_GATE_THRESHOLD;

/// Below this score the agent does not yet understand the user's intent well
/// enough to work confidently against it.
pub const LOW_INTENT_UNDERSTANDING: u8 = QUALITY_GATE_THRESHOLD;

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

/// Model-facing continuation for the private end-to-end ownership check. Names
/// the assessment category without disclosing the score or threshold.
pub const TODO_OWNERSHIP_CONTINUATION_MESSAGE: &str = "Your end-to-end ownership is not high enough to complete this goal. Take ownership of the full user outcome, not just the immediate implementation. Follow the work through every relevant integration and runtime path, resolve consequential gaps, validate the complete workflow, and finish the necessary follow-through. Then call the todo tool again, setting a higher `end_to_end_ownership` on the goal for this group; until that field is raised the write is rejected and the stored todo list is left unchanged.";

/// Model-facing continuation for private completion-confidence checks. Names
/// the assessment category without disclosing scores, items, or thresholds.
pub const TODO_COMPLETION_CONTINUATION_MESSAGE: &str = "[automated todo completion gate - not a user message] Your completion confidence is missing or not high enough. Do not reply conversationally or wait for the user. Instead: Validate the completed result more thoroughly with concrete evidence, address any remaining issues, then call the todo tool again with updated completion_confidence values that reflect the validation you performed.";

/// Model-facing continuation for a completed todo whose confidence rose too
/// sharply at the end. It names the behavior without disclosing the numeric
/// cutoff, individual todo, or recorded scores.
pub const TODO_CONFIDENCE_SPIKE_CONTINUATION_MESSAGE: &str = "[automated todo completion gate - not a user message] Your completion confidence rose too sharply to count as independently validated. Do not reply conversationally or wait for the user. Instead: recheck the completed result using concrete evidence, address any issues you find, then call the todo tool again with completion_confidence values that reflect the validation you performed.";

/// A completed todo is considered spike-finished when its final recorded
/// confidence increase is at least this large.
pub const TODO_CONFIDENCE_SPIKE: u8 = 15;

/// Below this score on the very first plan write, the agent is admitting it
/// does not yet know what it is being asked to do. That is worth one immediate
/// nudge, because a whole turn spent on the wrong task cannot be recovered at
/// turn end. Every other write-time check is deferred to the turn-end digest.
pub const SEVERE_INTENT_MISUNDERSTANDING: u8 = 60;

/// Which deferred quality check a recorded observation came from.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum GateObservationKind {
    IntentUnderstanding,
    ClosedFeedbackLoop,
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
    /// The score observed when this point was flagged.
    pub score: Option<u8>,
}

/// Header for the turn-end digest of unresolved quality-check points.
///
/// Deliberately framed as "double-check these" rather than as a refusal: by
/// turn end the work is done, so the useful action is verification, not
/// replanning. Names categories without disclosing scores or thresholds.
pub const TODO_GATE_DIGEST_PREFIX: &str = "[automated todo quality review - not a user message] Before you treat this turn as finished, double-check the weak points it surfaced. Do not reply conversationally or wait for the user.";

/// Whether the score behind this observation has since reached its threshold.
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
        GateObservationKind::IntentUnderstanding => plan
            .understands_user_intent
            .is_some_and(|score| score >= LOW_INTENT_UNDERSTANDING),
        GateObservationKind::ClosedFeedbackLoop => goals
            .iter()
            .find(|goal| normalized_group(goal.group.as_deref()) == observation.group)
            .and_then(|goal| goal.closed_feedback_loop)
            .is_some_and(|score| score >= LOW_CLOSED_FEEDBACK_LOOP),
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

/// Whether every group newly closed by this update has a sufficient assessment
/// of ownership over its full outcome. Groups completed before this check was
/// introduced are intentionally grandfathered so existing sessions stay writable.
pub fn newly_completed_groups_have_sufficient_ownership(
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
            .and_then(|goal| goal.end_to_end_ownership)
            .is_some_and(|score| score >= QUALITY_GATE_THRESHOLD)
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

/// Completed todos whose final confidence increase was abrupt rather than
/// accumulated in smaller evidence-backed steps. Older todo records may not
/// have a history, so they fall back to comparing planning and completion
/// confidence.
pub fn spike_completed_todos(todos: &[TodoItem]) -> Vec<&TodoItem> {
    todos
        .iter()
        .filter(|todo| todo.status == "completed")
        .filter(|todo| match todo.confidence_history.as_slice() {
            [] => todo
                .confidence
                .zip(todo.completion_confidence)
                .is_some_and(|(first, last)| last.saturating_sub(first) >= TODO_CONFIDENCE_SPIKE),
            [_] => false,
            history => {
                let n = history.len();
                history[n - 1].saturating_sub(history[n - 2]) >= TODO_CONFIDENCE_SPIKE
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
        || trimmed.starts_with(TODO_COMPLETION_CONTINUATION_MESSAGE)
        || trimmed.starts_with(TODO_CONFIDENCE_SPIKE_CONTINUATION_MESSAGE)
        || trimmed.starts_with(LEGACY_TODO_COMPLETION_CONTINUATION_MESSAGE)
        || trimmed.starts_with(LEGACY_TODO_CONFIDENCE_SPIKE_CONTINUATION_MESSAGE)
        || trimmed.starts_with(LEGACY_TODO_CONFIDENCE_SUMMARY_PREFIX)
        || trimmed.starts_with(TODO_GATE_DIGEST_PREFIX)
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

    fn intent_observation(score: Option<u8>) -> GateObservation {
        GateObservation {
            kind: GateObservationKind::IntentUnderstanding,
            group: None,
            score,
        }
    }

    fn loop_observation(group: Option<&str>, score: Option<u8>) -> GateObservation {
        GateObservation {
            kind: GateObservationKind::ClosedFeedbackLoop,
            group: group.map(str::to_string),
            score,
        }
    }

    /// A score that climbed only after work was underway still gets raised, and
    /// is described as the coverage gap it is. Suppressing it would let an agent
    /// clear the gate by writing a good assessment at the end, after the work it
    /// was supposed to govern was already done and booked.
    #[test]
    fn digest_still_raises_a_point_whose_score_climbed_late() {
        let observations = vec![
            intent_observation(Some(70)),
            intent_observation(Some(80)),
            intent_observation(Some(92)),
        ];
        let climbed = TodoPlan {
            understands_user_intent: Some(QUALITY_GATE_THRESHOLD),
            understands_user_intent_history: vec![70, 80, 92, QUALITY_GATE_THRESHOLD],
            ..Default::default()
        };
        let digest = build_gate_digest(&observations, &climbed, &[])
            .expect("a late climb must still be raised, not silently dropped");
        assert_eq!(digest.matches("\n- ").count(), 1);
        // Worded as "you started without understanding", not "you never
        // understood": the latter would contradict the passing final score and
        // invite the model to argue with the reminder instead of acting on it.
        assert!(digest.contains("started this work without understanding"));
        assert!(digest.contains("(flagged 3 times this turn)"));
        assert!(!digest.contains("never became solid"));
        assert!(!digest.contains(&QUALITY_GATE_THRESHOLD.to_string()));
    }

    /// The late-climb wording is per point, so one turn can carry both a goal
    /// that closed its loop late and a goal that never closed one at all.
    #[test]
    fn digest_words_late_climbs_and_never_closed_goals_differently() {
        let observations = vec![
            loop_observation(Some("closed late"), Some(50)),
            loop_observation(Some("never closed"), Some(50)),
        ];
        let goals = vec![
            TodoGoal {
                group: Some("closed late".to_string()),
                closed_feedback_loop: Some(QUALITY_GATE_THRESHOLD),
                ..Default::default()
            },
            TodoGoal {
                group: Some("never closed".to_string()),
                closed_feedback_loop: Some(50),
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
    fn digest_reports_a_point_that_never_resolved() {
        let observations = vec![intent_observation(Some(70))];
        let unresolved = TodoPlan {
            understands_user_intent: Some(70),
            understands_user_intent_history: vec![70],
            ..Default::default()
        };
        let digest = build_gate_digest(&observations, &unresolved, &[])
            .expect("an unresolved point should be surfaced");
        assert!(digest.starts_with(TODO_GATE_DIGEST_PREFIX));
        assert!(digest.contains("what the user actually wants"));
        // Framed as verification, since by turn end the work is already done.
        assert!(digest.contains("double-check"));
        // Private calibration stays private.
        assert!(!digest.contains("70"));
        assert!(!digest.contains(&QUALITY_GATE_THRESHOLD.to_string()));
        assert!(!digest.to_ascii_lowercase().contains("threshold"));
    }

    /// A long iterative turn flags the same point on every write. The digest
    /// must collapse those into one line, not a wall of duplicates.
    #[test]
    fn digest_collapses_repeats_and_counts_them() {
        let observations: Vec<GateObservation> = (0..9)
            .map(|_| loop_observation(Some("utf16 transcode"), Some(84)))
            .collect();
        let goals = vec![TodoGoal {
            group: Some("utf16 transcode".to_string()),
            closed_feedback_loop: Some(84),
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
            loop_observation(Some("transcode"), Some(50)),
            loop_observation(Some("render"), Some(50)),
            loop_observation(Some("render"), Some(55)),
        ];
        let goals = vec![
            TodoGoal {
                group: Some("transcode".to_string()),
                closed_feedback_loop: Some(50),
                ..Default::default()
            },
            TodoGoal {
                group: Some("render".to_string()),
                closed_feedback_loop: Some(55),
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
            &[loop_observation(None, Some(10))],
            &TodoPlan::default(),
            &[TodoGoal {
                closed_feedback_loop: Some(10),
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
        let digest = build_gate_digest(&[intent_observation(Some(70))], &TodoPlan::default(), &[])
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
        append_gate_observations(session, &[intent_observation(Some(70))]).expect("append");
        append_gate_observations(session, &[loop_observation(Some("perf"), Some(80))])
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
            .map(|_| intent_observation(Some(70)))
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
            (TODO_OWNERSHIP_CONTINUATION_MESSAGE, "end-to-end ownership"),
            (
                TODO_COMPLETION_CONTINUATION_MESSAGE,
                "completion confidence",
            ),
            (
                TODO_CONFIDENCE_SPIKE_CONTINUATION_MESSAGE,
                "completion confidence",
            ),
        ] {
            let lower = message.to_ascii_lowercase();
            assert!(lower.contains(category));
            assert!(!message.chars().any(|ch| ch.is_ascii_digit()));
            for disclosure in ["threshold", "percent", "below", "quality gate"] {
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
        assert!(TODO_OWNERSHIP_CONTINUATION_MESSAGE.contains("full user outcome"));
        assert!(TODO_OWNERSHIP_CONTINUATION_MESSAGE.contains("complete workflow"));
        assert!(TODO_OWNERSHIP_CONTINUATION_MESSAGE.contains("necessary follow-through"));
        assert!(TODO_COMPLETION_CONTINUATION_MESSAGE.contains("Validate the completed result"));
        assert!(TODO_CONFIDENCE_SPIKE_CONTINUATION_MESSAGE.contains("concrete evidence"));
        assert!(TODO_CONFIDENCE_SPIKE_CONTINUATION_MESSAGE.contains("rose too sharply"));
    }

    #[test]
    fn confidence_spike_classifier_distinguishes_bulk_stamp_from_stepped_rise() {
        let mut bulk = todo("bulk", "completed", None);
        bulk.confidence = Some(70);
        bulk.completion_confidence = Some(100);
        bulk.confidence_history = vec![70, 100];

        let mut stepped = todo("stepped", "completed", None);
        stepped.confidence = Some(100);
        stepped.completion_confidence = Some(100);
        stepped.confidence_history = vec![70, 80, 90, 100];

        let todos = [bulk, stepped];
        let spiked = spike_completed_todos(&todos);
        assert_eq!(spiked.len(), 1);
        assert_eq!(spiked[0].content, "bulk");
    }

    #[test]
    fn confidence_spike_classifier_includes_boundary_and_legacy_fallback() {
        let mut boundary = todo("boundary", "completed", None);
        boundary.confidence = Some(85);
        boundary.completion_confidence = Some(100);
        boundary.confidence_history = vec![85, 100];

        let mut legacy = todo("legacy", "completed", None);
        legacy.confidence = Some(80);
        legacy.completion_confidence = Some(100);

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

    fn ownership_goal(group: Option<&str>, ownership: Option<u8>) -> TodoGoal {
        TodoGoal {
            group: group.map(str::to_string),
            end_to_end_ownership: ownership,
            ..Default::default()
        }
    }

    #[test]
    fn newly_completed_group_requires_sufficient_end_to_end_ownership() {
        let previous = vec![todo("work", "in_progress", Some("ship"))];
        let completed = vec![todo("work", "completed", Some("ship"))];

        for ownership in [None, Some(0), Some(95)] {
            assert!(!newly_completed_groups_have_sufficient_ownership(
                &previous,
                &completed,
                &[ownership_goal(Some("ship"), ownership)],
            ));
        }
        assert!(newly_completed_groups_have_sufficient_ownership(
            &previous,
            &completed,
            &[ownership_goal(Some("ship"), Some(96))],
        ));
    }

    #[test]
    fn ownership_is_not_required_before_group_completion() {
        let previous = vec![todo("work", "pending", Some("ship"))];
        let in_progress = vec![todo("work", "in_progress", Some("ship"))];

        assert!(newly_completed_groups_have_sufficient_ownership(
            &previous,
            &in_progress,
            &[],
        ));
    }

    #[test]
    fn ownership_gate_normalizes_groups_and_supports_ungrouped_work() {
        let previous = vec![todo("work", "in_progress", Some(" ship "))];
        let completed = vec![todo("work", "completed", Some("ship"))];
        assert!(newly_completed_groups_have_sufficient_ownership(
            &previous,
            &completed,
            &[ownership_goal(Some(" ship"), Some(96))],
        ));

        let previous = vec![todo("work", "in_progress", None)];
        let completed = vec![todo("work", "completed", None)];
        assert!(newly_completed_groups_have_sufficient_ownership(
            &previous,
            &completed,
            &[ownership_goal(None, Some(96))],
        ));
    }

    /// The rejection is silent about *how* to clear it unless the message names
    /// the field. A caller that cannot tell which field to raise reads the
    /// rejection as a stuck tool and retries the same payload indefinitely.
    #[test]
    fn ownership_message_names_the_field_that_must_be_raised() {
        assert!(
            TODO_OWNERSHIP_CONTINUATION_MESSAGE.contains("end_to_end_ownership"),
            "the ownership nudge must name the field to raise"
        );
        assert!(
            TODO_OWNERSHIP_CONTINUATION_MESSAGE.contains("call the todo tool again"),
            "the ownership nudge must say to retry the write"
        );
        // The write is discarded, so a caller must know its list was not saved.
        assert!(
            TODO_OWNERSHIP_CONTINUATION_MESSAGE.contains("unchanged"),
            "the ownership nudge must disclose that the write was rejected"
        );
        // Every gate message that requires a specific field should name it, so
        // this property is asserted for the sibling gates too.
        assert!(TODO_COMPLETION_CONTINUATION_MESSAGE.contains("completion_confidence"));
    }

    #[test]
    fn ownership_gate_grandfathers_preexisting_completed_groups() {
        let completed = vec![todo("legacy", "completed", Some("legacy"))];
        assert!(newly_completed_groups_have_sufficient_ownership(
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
            understands_user_intent: Some(97),
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
            understands_user_intent: Some(97),
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
