//! Todo goal telemetry and score observation tracking.

use super::*;

/// Fold each incoming todo's confidence into its tool-maintained history.
///
/// The model reports `confidence` while working and `completion_confidence` at
/// completion. Each todo-tool write contributes at most one observation so a
/// single completion update cannot manufacture an apparent intermediate step.
/// The append-only trail lets downstream consumers distinguish an
/// evidence-driven rise (75 -> 85 -> 95 -> 100) from a bulk end-of-task stamp
/// (75 -> 100). Model-supplied `confidence_history` is ignored: the tool owns
/// this field.
pub(super) fn merge_confidence_history(previous: &[TodoItem], incoming: &mut [TodoItem]) {
    let prior: HashMap<&str, &TodoItem> = previous
        .iter()
        .map(|todo| (todo.id.as_str(), todo))
        .collect();
    for todo in incoming.iter_mut() {
        let previous_todo = prior.get(todo.id.as_str()).copied();
        let mut history = previous_todo
            .map(|prev| prev.confidence_history.clone())
            .unwrap_or_default();
        if history.is_empty()
            && let Some(value) = previous_todo.and_then(|prev| {
                if prev.status == "completed" {
                    prev.completion_confidence.or(prev.confidence)
                } else {
                    prev.confidence
                }
            })
        {
            history.push(value);
        }
        let observation = if todo.status == "completed" {
            todo.completion_confidence.or(todo.confidence)
        } else {
            todo.confidence
        };
        if let Some(value) = observation
            && history.last() != Some(&value)
        {
            history.push(value);
        }
        todo.confidence_history = history;
        // `assigned_to` links a todo to the swarm worker running it. It is set
        // by swarm dispatch, not the model (the schema does not expose it), so
        // a model rewrite of the list must not silently unassign the worker.
        if todo.assigned_to.is_none() {
            todo.assigned_to = previous_todo.and_then(|prev| prev.assigned_to.clone());
        }
        // Only a new start claims local ownership. Merely rewriting an old
        // unassigned in-progress item does not establish who is working on it.
        if todo.assigned_to.is_none()
            && todo.status == "in_progress"
            && previous_todo.is_none_or(|prev| prev.status != "in_progress")
        {
            todo.assigned_to = Some("@self".to_string());
        }
    }
}

pub(super) fn goal_group_key(group: Option<&str>) -> Option<String> {
    group
        .map(str::trim)
        .filter(|group| !group.is_empty())
        .map(str::to_string)
}

pub(super) fn todo_telemetry_update(
    previous: &[TodoItem],
    todos: &[TodoItem],
    goals: &[TodoGoal],
    plan: &TodoPlan,
) -> crate::telemetry::TodoTelemetryUpdate {
    let previous_by_id: HashMap<&str, &TodoItem> = previous
        .iter()
        .map(|todo| (todo.id.as_str(), todo))
        .collect();
    let current_by_id: HashMap<&str, &TodoItem> =
        todos.iter().map(|todo| (todo.id.as_str(), todo)).collect();

    let todos_created = current_by_id
        .keys()
        .filter(|id| !previous_by_id.contains_key(**id))
        .count()
        .min(u32::MAX as usize) as u32;
    let todos_completed = current_by_id
        .iter()
        .filter(|(id, todo)| {
            todo.status == "completed"
                && previous_by_id
                    .get(**id)
                    .is_none_or(|previous| previous.status != "completed")
        })
        .count()
        .min(u32::MAX as usize) as u32;
    let todos_abandoned = previous_by_id
        .iter()
        .filter(|(id, todo)| todo.status != "completed" && !current_by_id.contains_key(**id))
        .count()
        .min(u32::MAX as usize) as u32;
    let current_incomplete = current_by_id
        .values()
        .filter(|todo| todo.status != "completed")
        .count()
        .min(u32::MAX as usize) as u32;

    let mut group_completion: HashMap<Option<String>, bool> = HashMap::new();
    for todo in current_by_id.values() {
        let completed = todo.status == "completed";
        group_completion
            .entry(goal_group_key(todo.group.as_deref()))
            .and_modify(|all_completed| *all_completed &= completed)
            .or_insert(completed);
    }

    crate::telemetry::TodoTelemetryUpdate {
        todos_created,
        todos_completed,
        todos_abandoned,
        current_incomplete,
        list_size: todos.len().min(u32::MAX as usize) as u32,
        groups_completed: group_completion
            .values()
            .filter(|completed| **completed)
            .count()
            .min(u32::MAX as usize) as u32,
        groups_total: group_completion.len().min(u32::MAX as usize) as u32,
        confidence: crate::telemetry::TelemetryScoreSummary::from_scores(
            current_by_id
                .values()
                .filter_map(|todo| todo.confidence.map(|state| state.telemetry_score())),
        ),
        completion_confidence: crate::telemetry::TelemetryScoreSummary::from_scores(
            current_by_id
                .values()
                .filter_map(|todo| todo.completion_confidence.map(|state| state.telemetry_score())),
        ),
        understands_user_intent: crate::telemetry::TelemetryScoreSummary::from_scores(
            plan.understands_user_intent
                .map(|state| state.telemetry_score()),
        ),
        closed_feedback_loop: crate::telemetry::TelemetryScoreSummary::from_scores(
            goals
                .iter()
                .filter_map(|goal| goal.closed_feedback_loop.map(|state| state.telemetry_score())),
        ),
        feedback_loop_relevance: crate::telemetry::TelemetryScoreSummary::from_scores(
            goals.iter().filter_map(|goal| {
                goal.feedback_loop_relevance
                    .map(|state| state.telemetry_score())
            }),
        ),
        feedback_loop_coverage: crate::telemetry::TelemetryScoreSummary::from_scores(
            goals.iter().filter_map(|goal| {
                goal.feedback_loop_coverage
                    .map(|state| state.telemetry_score())
            }),
        ),
        end_to_end_ownership: crate::telemetry::TelemetryScoreSummary::from_scores(
            goals
                .iter()
                .filter_map(|goal| goal.delivery_state.map(|state| state.telemetry_score())),
        ),
    }
}

pub(super) fn record_todo_telemetry(
    previous: &[TodoItem],
    todos: &[TodoItem],
    goals: &[TodoGoal],
    plan: &TodoPlan,
) {
    crate::telemetry::record_todo_update(todo_telemetry_update(previous, todos, goals, plan));
}

/// Append `value` to `history` when it is a new observation.
///
/// One todo-tool write contributes at most one entry per score, so a single
/// bulk update cannot manufacture an apparent gradual climb.
pub(super) fn record_score_observation<T: Copy + PartialEq>(
    history: &mut Vec<T>,
    value: Option<T>,
) {
    if let Some(value) = value
        && history.last() != Some(&value)
    {
        history.push(value);
    }
}
