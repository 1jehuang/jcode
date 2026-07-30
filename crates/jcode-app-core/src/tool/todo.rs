use super::{Tool, ToolContext, ToolOutput};
use crate::bus::{Bus, BusEvent, TodoEvent};
use crate::todo::{
    GateObservation, GateObservationKind, LOW_CLOSED_FEEDBACK_LOOP, LOW_INTENT_UNDERSTANDING,
    SEVERE_INTENT_MISUNDERSTANDING, TODO_INTENT_UNDERSTANDING_CONTINUATION_MESSAGE,
    TODO_OWNERSHIP_CONTINUATION_MESSAGE, TodoGoal, TodoGoalChange, TodoGoalField, TodoItem,
    TodoPlan, TodoPlanChange, TodoPlanField, append_gate_observations, load_goals, load_plan,
    load_todos, newly_completed_groups_have_sufficient_ownership, save_goals, save_plan,
    save_todos,
};
use anyhow::Result;
use async_trait::async_trait;
use serde::Deserialize;
use serde_json::{Value, json};
use std::collections::HashMap;

pub struct TodoTool;

impl TodoTool {
    pub fn new() -> Self {
        Self
    }
}

/// Fold each incoming todo's confidence into its tool-maintained history.
///
/// The model reports `confidence` while working and `completion_confidence` at
/// completion. Each todo-tool write contributes at most one observation so a
/// single completion update cannot manufacture an apparent intermediate step.
/// The append-only trail lets downstream consumers distinguish an
/// evidence-driven rise (75 -> 85 -> 95 -> 100) from a bulk end-of-task stamp
/// (75 -> 100). Model-supplied `confidence_history` is ignored: the tool owns
/// this field.
fn merge_confidence_history(previous: &[TodoItem], incoming: &mut [TodoItem]) {
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
    }
}

#[derive(Deserialize)]
struct TodoInput {
    todos: Option<Vec<TodoItem>>,
    goals: Option<Vec<TodoGoal>>,
    plan: Option<TodoPlan>,
}

/// Normalize a goal's group label: trimmed, with empty/whitespace collapsed
/// to `None` (the implicit goal of an ungrouped list).
fn goal_group_key(group: Option<&str>) -> Option<String> {
    group
        .map(str::trim)
        .filter(|group| !group.is_empty())
        .map(str::to_string)
}

/// Append `value` to `history` when it is a new observation.
///
/// One todo-tool write contributes at most one entry per score, so a single
/// bulk update cannot manufacture an apparent gradual climb.
fn record_score_observation(history: &mut Vec<u8>, value: Option<u8>) {
    if let Some(value) = value
        && history.last() != Some(&value)
    {
        history.push(value);
    }
}

/// Merge incoming goal assessments with the stored ones.
///
/// Incoming goals win per group key; stored goals for groups the write does
/// not mention are retained (a todo update should not silently discard goal
/// assessments). Score histories are tool-maintained: whatever the model sends
/// for them is discarded in favor of the stored trail plus this write's
/// observation.
fn merge_goals(stored: &[TodoGoal], incoming: Option<Vec<TodoGoal>>) -> Vec<TodoGoal> {
    let Some(incoming) = incoming else {
        return stored.to_vec();
    };
    let mut merged: Vec<TodoGoal> = Vec::new();
    for mut goal in incoming {
        goal.group = goal_group_key(goal.group.as_deref());
        let previous = stored
            .iter()
            .find(|prev| goal_group_key(prev.group.as_deref()) == goal.group);
        goal.closed_feedback_loop_history = previous
            .map(|prev| prev.closed_feedback_loop_history.clone())
            .unwrap_or_default();
        goal.end_to_end_ownership_history = previous
            .map(|prev| prev.end_to_end_ownership_history.clone())
            .unwrap_or_default();
        // Field-level merge, matching `merge_plan`: a write that revises one
        // assessment must not silently erase the others. Without this the
        // turn-end digest would read a stale `None` and re-raise a point the
        // agent had already resolved.
        if let Some(prev) = previous {
            if goal.closed_feedback_loop.is_none() {
                goal.closed_feedback_loop = prev.closed_feedback_loop;
            }
            if goal.end_to_end_ownership.is_none() {
                goal.end_to_end_ownership = prev.end_to_end_ownership;
            }
            if goal.feedback_loop.is_none() {
                goal.feedback_loop = prev.feedback_loop.clone();
            }
        }
        record_score_observation(
            &mut goal.closed_feedback_loop_history,
            goal.closed_feedback_loop,
        );
        record_score_observation(
            &mut goal.end_to_end_ownership_history,
            goal.end_to_end_ownership,
        );
        if let Some(slot) = merged
            .iter_mut()
            .find(|existing| existing.group == goal.group)
        {
            *slot = goal;
        } else {
            merged.push(goal);
        }
    }
    for prev in stored {
        let key = goal_group_key(prev.group.as_deref());
        if !merged.iter().any(|goal| goal.group == key) {
            merged.push(prev.clone());
        }
    }
    merged
}

fn changed_goal_fields(before: Option<&TodoGoal>, after: Option<&TodoGoal>) -> Vec<TodoGoalField> {
    let mut fields = Vec::new();
    if before.and_then(|goal| goal.closed_feedback_loop)
        != after.and_then(|goal| goal.closed_feedback_loop)
    {
        fields.push(TodoGoalField::ClosedFeedbackLoop);
    }
    if before.and_then(|goal| goal.feedback_loop.as_ref())
        != after.and_then(|goal| goal.feedback_loop.as_ref())
    {
        fields.push(TodoGoalField::FeedbackLoop);
    }
    if before.and_then(|goal| goal.end_to_end_ownership)
        != after.and_then(|goal| goal.end_to_end_ownership)
    {
        fields.push(TodoGoalField::EndToEndOwnership);
    }
    fields
}

/// Merge the incoming plan-level intent assessment with the stored one.
///
/// User intention describes why the user asked for the work and should remain
/// stable while the agent revises its steps or scores, so an omitted intention
/// inherits the stored value. Sending an empty string clears it. The intent
/// score's history is tool-maintained, so a model-supplied trail is discarded.
fn merge_plan(stored: &TodoPlan, incoming: Option<TodoPlan>) -> TodoPlan {
    let Some(mut plan) = incoming else {
        return stored.clone();
    };
    if plan.user_intention.is_none() {
        plan.user_intention = stored.user_intention.clone();
    }
    if plan.understands_user_intent.is_none() {
        plan.understands_user_intent = stored.understands_user_intent;
    }
    plan.understands_user_intent_history = stored.understands_user_intent_history.clone();
    record_score_observation(
        &mut plan.understands_user_intent_history,
        plan.understands_user_intent,
    );
    plan
}

fn plan_change(before: &TodoPlan, after: &TodoPlan) -> Option<TodoPlanChange> {
    let mut fields = Vec::new();
    if before.user_intention != after.user_intention {
        fields.push(TodoPlanField::UserIntention);
    }
    if before.understands_user_intent != after.understands_user_intent {
        fields.push(TodoPlanField::UnderstandsUserIntent);
    }
    (!fields.is_empty()).then(|| TodoPlanChange {
        before: Some(before.clone()),
        after: Some(after.clone()),
        fields,
    })
}

fn goal_changes(before: &[TodoGoal], after: &[TodoGoal]) -> Vec<TodoGoalChange> {
    let mut changes = Vec::new();
    for current in after {
        let key = goal_group_key(current.group.as_deref());
        let previous = before
            .iter()
            .find(|goal| goal_group_key(goal.group.as_deref()) == key);
        let fields = changed_goal_fields(previous, Some(current));
        if !fields.is_empty() {
            changes.push(TodoGoalChange {
                before: previous.cloned(),
                after: Some(current.clone()),
                fields,
            });
        }
    }
    for previous in before {
        let key = goal_group_key(previous.group.as_deref());
        if after
            .iter()
            .any(|goal| goal_group_key(goal.group.as_deref()) == key)
        {
            continue;
        }
        let fields = changed_goal_fields(Some(previous), None);
        if !fields.is_empty() {
            changes.push(TodoGoalChange {
                before: Some(previous.clone()),
                after: None,
                fields,
            });
        }
    }
    changes
}

/// Record the points this write would previously have interrupted on, and
/// return the rare continuation that is still worth sending immediately.
///
/// Previously both checks emitted a continuation on every applicable write for
/// as long as the score stayed low. That punished the common healthy case:
/// understanding of a request starts low and rises as the agent explores, so an
/// agent already resolving the ambiguity was repeatedly told to stop and go
/// resolve the ambiguity. On long iterative turns the same text reattached to
/// every todo call, spending reasoning on re-justifying the plan instead of on
/// the work.
///
/// So the checks are deferred: observations accumulate and are replayed once at
/// turn end by `build_gate_digest`. Deferred, not forgiven. A score that climbs
/// late is still raised, because the work done while it was low was never
/// governed by the better loop that arrived afterwards. The one exception is a
/// first plan write that scores severely low, where the agent is admitting it
/// does not know the task at all and a whole turn of wrong work cannot be undone
/// at turn end.
fn record_reframe_observations(
    plan: &TodoPlan,
    goals: &[TodoGoal],
    todos: &[TodoItem],
    previous: &[TodoItem],
) -> (Vec<GateObservation>, Vec<String>) {
    let mut observations = Vec::new();
    let mut immediate = Vec::new();
    let any_open = todos
        .iter()
        .any(|todo| todo.status != "completed" && todo.status != "cancelled");
    if any_open
        && plan
            .understands_user_intent
            .is_none_or(|score| score < LOW_INTENT_UNDERSTANDING)
    {
        observations.push(GateObservation {
            kind: GateObservationKind::IntentUnderstanding,
            group: None,
            score: plan.understands_user_intent,
        });
        // Only on the first observation of the plan, so a persistently low
        // score is reported once at turn end rather than on every write.
        let first_assessment = plan.understands_user_intent_history.len() <= 1;
        if first_assessment
            && plan
                .understands_user_intent
                .is_some_and(|score| score < SEVERE_INTENT_MISUNDERSTANDING)
        {
            immediate.push(TODO_INTENT_UNDERSTANDING_CONTINUATION_MESSAGE.to_string());
        }
    }
    let closed_now = crate::todo::groups_closed_by_update(previous, todos);
    for goal in goals {
        let group_open = todos.iter().any(|todo| {
            goal_group_key(todo.group.as_deref()) == goal.group
                && todo.status != "completed"
                && todo.status != "cancelled"
        });
        // A group this write closes counts too: a goal created and finished in
        // one step is otherwise never observed, and one-step completions are
        // where a weak feedback loop hides best.
        if !group_open && !closed_now.contains(&goal.group) {
            continue;
        }
        if goal
            .closed_feedback_loop
            .is_none_or(|score| score < LOW_CLOSED_FEEDBACK_LOOP)
        {
            observations.push(GateObservation {
                kind: GateObservationKind::ClosedFeedbackLoop,
                group: goal.group.clone(),
                score: goal.closed_feedback_loop,
            });
        }
    }
    (observations, immediate)
}

fn build_todo_output(
    todos: Vec<TodoItem>,
    plan: TodoPlan,
    goals: Vec<TodoGoal>,
    plan_change: Option<TodoPlanChange>,
    goal_changes: Option<Vec<TodoGoalChange>>,
    continuations: impl IntoIterator<Item = String>,
) -> Result<ToolOutput> {
    let remaining = todos
        .iter()
        .filter(|todo| todo.status != "completed")
        .count();
    let mut text = serde_json::to_string_pretty(&todos)?;
    if plan != TodoPlan::default() {
        text.push_str("\n\nPlan:\n");
        text.push_str(&serde_json::to_string_pretty(&plan)?);
    }
    if !goals.is_empty() {
        text.push_str("\n\nGoals:\n");
        text.push_str(&serde_json::to_string_pretty(&goals)?);
    }
    if let Some(plan_change) = plan_change.as_ref() {
        text.push_str("\n\nPlan updates:\n");
        text.push_str(&serde_json::to_string_pretty(plan_change)?);
    }
    if let Some(goal_changes) = goal_changes.as_ref().filter(|changes| !changes.is_empty()) {
        text.push_str("\n\nGoal updates:\n");
        text.push_str(&serde_json::to_string_pretty(goal_changes)?);
    }
    for continuation in continuations {
        text.push_str("\n\n");
        text.push_str(&continuation);
    }
    let mut metadata = json!({"todos": todos, "plan": plan, "goals": goals});
    if let Some(plan_change) = plan_change {
        metadata["plan_update"] = serde_json::to_value(plan_change)?;
    }
    if let Some(goal_changes) = goal_changes.filter(|changes| !changes.is_empty()) {
        metadata["goal_updates"] = serde_json::to_value(goal_changes)?;
    }
    Ok(ToolOutput::new(text)
        .with_title(format!("{} todos", remaining))
        .with_metadata(metadata))
}

/// Leniently normalize raw todo-tool arguments before strict deserialization.
///
/// Some providers (notably Claude tool calling) intermittently emit tool
/// arguments as JSON *strings* instead of native types: the whole `todos`
/// array as one stringified JSON blob, individual items as stringified
/// objects, or numeric fields like `confidence` as `"90"`. Strict
/// `serde_json::from_value` rejects these with `invalid type: string ...`,
/// failing the entire call (issue #357; same provider quirk as #106).
fn normalize_todo_input(mut input: Value) -> Value {
    let Some(obj) = input.as_object_mut() else {
        return input;
    };
    if let Some(plan) = obj.get_mut("plan") {
        if let Value::String(raw) = plan {
            let trimmed = raw.trim();
            if trimmed.is_empty() {
                *plan = Value::Null;
            } else if let Ok(parsed @ (Value::Object(_) | Value::Null)) =
                serde_json::from_str::<Value>(trimmed)
            {
                *plan = parsed;
            }
        }
        if let Some(fields) = plan.as_object_mut() {
            for key in [
                "alignment_score",
                "user_intention_alignment",
                "understands_user_intent",
            ] {
                if let Some(value) = fields.get_mut(key) {
                    coerce_value_to_integer(value);
                }
            }
        }
    }
    for key in ["todos", "goals"] {
        let Some(entries) = obj.get_mut(key) else {
            continue;
        };

        // Whole array sent as a stringified JSON blob.
        if let Value::String(raw) = entries {
            let trimmed = raw.trim();
            if trimmed.is_empty() {
                *entries = Value::Null;
            } else if let Ok(parsed @ (Value::Array(_) | Value::Null)) =
                serde_json::from_str::<Value>(trimmed)
            {
                *entries = parsed;
            }
        }

        if let Value::Array(items) = entries {
            for item in items.iter_mut() {
                // Individual item sent as a stringified JSON object.
                if let Value::String(raw) = item
                    && let Ok(parsed @ Value::Object(_)) = serde_json::from_str::<Value>(raw.trim())
                {
                    *item = parsed;
                }
                let Some(fields) = item.as_object_mut() else {
                    continue;
                };
                for key in [
                    "confidence",
                    "completion_confidence",
                    "alignment_score",
                    "user_intention_alignment",
                    "closed_feedback_loop",
                    // Pre-rename alias; some prompts and replayed transcripts
                    // still carry the old key.
                    "hill_climbability",
                    "end_to_end_ownership",
                ] {
                    if let Some(value) = fields.get_mut(key) {
                        coerce_value_to_integer(value);
                    }
                }
            }
        }
    }
    input
}

/// Coerce a numeric string (`"90"`) or whole float (`90.0`) to a JSON integer,
/// and an empty string to `null`. Leaves anything else untouched so strict
/// deserialization can report a precise error.
fn coerce_value_to_integer(value: &mut Value) {
    match value {
        Value::String(raw) => {
            let trimmed = raw.trim();
            if trimmed.is_empty() {
                *value = Value::Null;
            } else if let Ok(parsed) = trimmed.parse::<u64>() {
                *value = Value::from(parsed);
            }
        }
        Value::Number(num) => {
            if num.as_u64().is_none()
                && let Some(float) = num.as_f64()
                && float.fract() == 0.0
                && (0.0..=u64::MAX as f64).contains(&float)
            {
                *value = Value::from(float as u64);
            }
        }
        _ => {}
    }
}

#[async_trait]
impl Tool for TodoTool {
    fn name(&self) -> &str {
        "todo"
    }

    fn description(&self) -> &str {
        // SECURITY/EVAL: This is model-visible calibration text. Keep it
        // deliberately handwritten. Never generate it from gate constants or
        // interpolate private thresholds, because that would teach the model
        // how to target the evaluator instead of reporting an honest assessment.
        "Read or update structured todo items and optional goal-level assessments."
    }

    fn parameters_schema(&self) -> Value {
        json!({
            "type": "object",
            "properties": {
                "intent": super::intent_schema_property(),
                "todos": {
                    "type": "array",
                    "description": "Todo list to save.",
                    "items": {
                        "type": "object",
                        "required": ["content", "status", "priority", "id", "confidence"],
                        "properties": {
                            "content": {
                                "type": "string",
                                "description": "Task."
                            },
                            "status": {
                                "type": "string",
                                "description": "Status."
                            },
                            "priority": {
                                "type": "string",
                                "description": "Priority."
                            },
                            "id": {
                                "type": "string",
                                "description": "ID."
                            },
                            "group": {
                                "type": "string",
                                "description": "Optional group label; one group per coherent goal, new direction = new group. Omit for flat list."
                            },
                            "confidence": {
                                "type": "integer",
                                "minimum": 0,
                                "maximum": 100,
                                "description": "Confidence 0-100 that this todo can be completed correctly; reassess as evidence accumulates."
                            },
                            "completion_confidence": {
                                "type": "integer",
                                "minimum": 0,
                                "maximum": 100,
                                "description": "Self-assessed confidence, 0-100, that this todo was completed correctly. Use only for completed items."
                            }
                        }
                    }
                },
                "plan": {
                    "type": "object",
                    "description": "Plan-level understanding of the request. Send on first write and whenever understanding changes.",
                    "required": ["user_intention", "understands_user_intent"],
                    "properties": {
                        "user_intention": {
                            "type": "string",
                            "description": "What the user actually wants: underlying reason and desired end state. Omit later to retain."
                        },
                        "understands_user_intent": {
                            "type": "integer",
                            "minimum": 0,
                            "maximum": 100,
                            "description": "0-100: how well you understand what the user actually wants. Score low when guessing at intent."
                        }
                    }
                },
                "goals": {
                    "type": "array",
                    "description": "Goal-level assessments, one per todo group (null = ungrouped). Omitted groups are retained.",
                    "items": {
                        "type": "object",
                        "required": ["closed_feedback_loop", "feedback_loop"],
                        "properties": {
                            "group": {
                                "type": "string",
                                "description": "Group label this goal describes. Omit or null for the ungrouped list."
                            },
                            "closed_feedback_loop": {
                                "type": "integer",
                                "minimum": 0,
                                "maximum": 100,
                                "description": "0-100: how much of this goal's correctness the feedback_loop can verify on its own."
                            },
                            "feedback_loop": {
                                "type": "string",
                                "description": "Requirement-to-check process: an explicit observation or check for each requirement of this goal."
                            },
                            "end_to_end_ownership": {
                                "type": "integer",
                                "minimum": 0,
                                "maximum": 100,
                                "description": "Completion-time 0-100: was the full intended user outcome delivered, with follow-through."
                            }
                        }
                    }
                }
            }
        })
    }

    async fn execute(&self, input: Value, ctx: ToolContext) -> Result<ToolOutput> {
        let params: TodoInput = serde_json::from_value(normalize_todo_input(input))?;
        let is_write = params.todos.is_some() || params.goals.is_some() || params.plan.is_some();
        let operation = if is_write { "write" } else { "read" };
        let result = if is_write {
            // Goals/plan-only writes keep the stored todo list.
            let previous = load_todos(&ctx.session_id).unwrap_or_default();
            let mut todos = params.todos.unwrap_or_else(|| previous.clone());
            merge_confidence_history(&previous, &mut todos);
            (|| {
                let stored_goals = load_goals(&ctx.session_id).unwrap_or_default();
                let stored_plan = load_plan(&ctx.session_id).unwrap_or_default();
                let goals = merge_goals(&stored_goals, params.goals);
                let plan = merge_plan(&stored_plan, params.plan);
                if !newly_completed_groups_have_sufficient_ownership(&previous, &todos, &goals) {
                    crate::telemetry::record_todo_gate(crate::telemetry::TodoGateKind::Ownership);
                    return build_todo_output(
                        previous,
                        stored_plan,
                        stored_goals,
                        None,
                        None,
                        [TODO_OWNERSHIP_CONTINUATION_MESSAGE.to_string()],
                    );
                }
                let (observations, nudges) =
                    record_reframe_observations(&plan, &goals, &todos, &previous);
                for observation in &observations {
                    let kind = match observation.kind {
                        GateObservationKind::IntentUnderstanding => {
                            crate::telemetry::TodoGateKind::IntentUnderstanding
                        }
                        GateObservationKind::ClosedFeedbackLoop => {
                            crate::telemetry::TodoGateKind::ClosedFeedbackLoop
                        }
                    };
                    crate::telemetry::record_todo_gate(kind);
                }
                // Best-effort: a failure to persist the observation log must not
                // fail the todo write itself. The cost is a missing reminder.
                if let Err(err) = append_gate_observations(&ctx.session_id, &observations) {
                    crate::logging::warn(&format!(
                        "[tool:todo] failed to record gate observations session_id={} error={}",
                        ctx.session_id, err
                    ));
                }
                // Assessment-only writes, especially quality-gate retries,
                // should render the fields that changed instead of repeating an
                // otherwise identical todo plan.
                let assessment_only = todos == previous;
                let concise_goal_changes = (assessment_only && !stored_goals.is_empty())
                    .then(|| goal_changes(&stored_goals, &goals));
                let concise_plan_change = assessment_only
                    .then(|| plan_change(&stored_plan, &plan))
                    .flatten();
                save_todos(&ctx.session_id, &todos)?;
                save_goals(&ctx.session_id, &goals)?;
                save_plan(&ctx.session_id, &plan)?;

                Bus::global().publish(BusEvent::TodoUpdated(TodoEvent {
                    session_id: ctx.session_id.clone(),
                    todos: todos.clone(),
                }));

                build_todo_output(
                    todos,
                    plan,
                    goals,
                    concise_plan_change,
                    concise_goal_changes,
                    nudges,
                )
            })()
        } else {
            (|| {
                let todos = load_todos(&ctx.session_id)?;
                let goals = load_goals(&ctx.session_id).unwrap_or_default();
                let plan = load_plan(&ctx.session_id).unwrap_or_default();
                build_todo_output(todos, plan, goals, None, None, Vec::new())
            })()
        };
        result.map_err(|err| {
            crate::logging::warn(&format!(
                "[tool:todo] operation failed operation={} session_id={} error={}",
                operation, ctx.session_id, err
            ));
            err
        })
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn tool_is_named_todo() {
        assert_eq!(TodoTool::new().name(), "todo");
    }

    #[test]
    fn schema_advertises_intent_and_todos() {
        let schema = TodoTool::new().parameters_schema();
        let props = schema
            .get("properties")
            .and_then(|v| v.as_object())
            .expect("todo schema should have properties");
        assert_eq!(props.len(), 4);
        assert!(props.contains_key("intent"));
        assert!(props.contains_key("todos"));
        assert!(props.contains_key("plan"));
        assert!(props.contains_key("goals"));

        let item = props["todos"]
            .get("items")
            .and_then(|v| v.as_object())
            .expect("todos should describe item objects");
        let required = item
            .get("required")
            .and_then(|v| v.as_array())
            .expect("todo item should advertise required fields");
        assert!(required.iter().any(|v| v == "confidence"));
        let item_props = item
            .get("properties")
            .and_then(|v| v.as_object())
            .expect("todo item should advertise properties");
        assert!(item_props.contains_key("confidence"));
        assert!(item_props.contains_key("completion_confidence"));
        assert!(!item_props.contains_key("closed_feedback_loop"));
        assert_eq!(
            item_props["confidence"]["description"],
            "Confidence 0-100 that this todo can be completed correctly; reassess as evidence accumulates."
        );

        let plan_props = props["plan"]
            .get("properties")
            .and_then(|v| v.as_object())
            .expect("plan should describe properties");
        assert!(plan_props.contains_key("user_intention"));
        assert!(plan_props.contains_key("understands_user_intent"));
        assert!(!plan_props.contains_key("alignment_score"));
        assert!(!plan_props.contains_key("user_intention_alignment"));
        assert_eq!(plan_props.len(), 2);
        let plan_required = props["plan"]["required"]
            .as_array()
            .expect("plan should advertise required fields");
        assert!(plan_required.iter().any(|value| value == "user_intention"));
        assert!(
            plan_required
                .iter()
                .any(|value| value == "understands_user_intent")
        );

        let goal_props = props["goals"]
            .get("items")
            .and_then(|v| v.get("properties"))
            .and_then(|v| v.as_object())
            .expect("goals should describe item objects");
        assert!(goal_props.contains_key("group"));
        assert!(goal_props.contains_key("closed_feedback_loop"));
        assert!(goal_props.contains_key("feedback_loop"));
        assert!(goal_props.contains_key("end_to_end_ownership"));
        // Intent lives on the plan, not per goal.
        assert!(!goal_props.contains_key("user_intention"));
        assert!(!goal_props.contains_key("alignment_score"));
        assert!(!goal_props.contains_key("objective"));
        assert_eq!(goal_props.len(), 4);

        let goal_required = props["goals"]["items"]["required"]
            .as_array()
            .expect("goals should advertise required fields");
        assert!(
            goal_required
                .iter()
                .any(|value| value == "closed_feedback_loop")
        );
        assert!(goal_required.iter().any(|value| value == "feedback_loop"));

        let alignment_description = plan_props["understands_user_intent"]
            .get("description")
            .and_then(Value::as_str)
            .expect("alignment score should describe representation coverage");
        assert!(alignment_description.contains("what the user actually wants"));
        assert!(alignment_description.contains("Score low when guessing"));
        // The detailed calibration rubric moved out of the always-on schema
        // into the gate continuation messages, which are paid only when a
        // write is rejected.
        for required_concept in [
            "requirement inventory",
            "outcomes, deliverables, constraints, prohibited actions",
            "integration paths, edge cases, and necessary follow-through",
            "Do not ask the user",
        ] {
            assert!(
                crate::todo::TODO_INTENT_UNDERSTANDING_CONTINUATION_MESSAGE
                    .contains(required_concept),
                "intent gate message omitted {required_concept}"
            );
        }
        let feedback_description = goal_props["feedback_loop"]
            .get("description")
            .and_then(Value::as_str)
            .expect("feedback loop should describe requirement-to-check coverage");
        assert!(feedback_description.contains("requirement-to-check"));
        assert!(feedback_description.contains("explicit observation or check"));
        for required_concept in [
            "reports back on each requirement",
            "run tests, verify, or review count only",
            "non-testable requirements",
        ] {
            assert!(
                crate::todo::TODO_CLOSED_FEEDBACK_LOOP_CONTINUATION_MESSAGE
                    .contains(required_concept),
                "feedback gate message omitted {required_concept}"
            );
        }
        assert!(
            !alignment_description
                .to_ascii_lowercase()
                .contains("threshold")
        );

        let ownership_description = goal_props["end_to_end_ownership"]
            .get("description")
            .and_then(Value::as_str)
            .expect("ownership should have a neutral description");
        assert!(ownership_description.contains("full intended user outcome"));
        assert!(ownership_description.contains("follow-through"));
        assert!(!ownership_description.contains("90"));
        assert!(
            !ownership_description
                .to_ascii_lowercase()
                .contains("threshold")
        );

        let loop_description = goal_props["closed_feedback_loop"]
            .get("description")
            .and_then(Value::as_str)
            .expect("closed feedback loop should describe the assessment neutrally");
        assert!(!loop_description.contains(&LOW_CLOSED_FEEDBACK_LOOP.to_string()));
        assert!(!loop_description.to_ascii_lowercase().contains("threshold"));

        let model_visible_schema = serde_json::to_string(&schema)
            .expect("todo schema should serialize")
            .to_ascii_lowercase();
        for disclosure in [
            "threshold",
            "quality gate",
            "internal quality check",
            "not jump",
            "test that passes",
            "isn't high enough",
        ] {
            assert!(
                !model_visible_schema.contains(disclosure),
                "model-visible todo schema disclosed calibration wording: {disclosure}"
            );
        }
        for domain_hint in [
            "visual quality",
            "screenshot",
            "browser",
            "viewport",
            "console error",
        ] {
            assert!(
                !model_visible_schema.contains(domain_hint),
                "model-visible todo schema biased visual-work feedback: {domain_hint}"
            );
        }
    }

    fn parse(input: Value) -> Result<TodoInput, serde_json::Error> {
        serde_json::from_value(normalize_todo_input(input))
    }

    #[test]
    fn accepts_stringified_todos_array() {
        let input = json!({
            "todos": "[{\"content\":\"a\",\"status\":\"pending\",\"priority\":\"high\",\"id\":\"1\",\"confidence\":90}]"
        });
        let parsed = parse(input).expect("stringified todos array should parse");
        let todos = parsed.todos.expect("todos present");
        assert_eq!(todos.len(), 1);
        assert_eq!(todos[0].content, "a");
        assert_eq!(todos[0].confidence, Some(90));
    }

    #[test]
    fn accepts_stringified_todo_items_and_string_confidence() {
        let input = json!({
            "todos": [
                "{\"content\":\"b\",\"status\":\"completed\",\"priority\":\"low\",\"id\":\"2\",\"confidence\":\"85\",\"completion_confidence\":\"95\"}",
                {"content": "c", "status": "pending", "priority": "high", "id": "3", "confidence": "70"}
            ]
        });
        let parsed = parse(input).expect("string-coerced items should parse");
        let todos = parsed.todos.expect("todos present");
        assert_eq!(todos.len(), 2);
        assert_eq!(todos[0].confidence, Some(85));
        assert_eq!(todos[0].completion_confidence, Some(95));
        assert_eq!(todos[1].confidence, Some(70));
    }

    #[test]
    fn accepts_float_confidence_and_empty_string_as_none() {
        let input = json!({
            "todos": [
                {"content": "d", "status": "pending", "priority": "high", "id": "4", "confidence": 90.0, "completion_confidence": ""}
            ]
        });
        let parsed = parse(input).expect("float confidence should parse");
        let todos = parsed.todos.expect("todos present");
        assert_eq!(todos[0].confidence, Some(90));
        assert_eq!(todos[0].completion_confidence, None);
    }

    #[test]
    fn empty_string_todos_means_read() {
        let parsed = parse(json!({"todos": ""})).expect("empty string should parse");
        assert!(parsed.todos.is_none());
    }

    #[test]
    fn native_input_still_parses() {
        let input = json!({
            "todos": [
                {"content": "e", "status": "pending", "priority": "high", "id": "5", "confidence": 80}
            ]
        });
        let parsed = parse(input).expect("native input should parse");
        assert_eq!(parsed.todos.expect("todos present")[0].confidence, Some(80));
    }

    #[test]
    fn accepts_goals_and_plan_including_string_coercion() {
        let input = json!({
            "plan": {"user_intention": "make repository search feel instant", "understands_user_intent": "97"},
            "goals": [
                {"group": "optimize grep", "closed_feedback_loop": "95", "feedback_loop": "run the grep benchmark and compare p50"},
                {"closed_feedback_loop": 20}
            ]
        });
        let parsed = parse(input).expect("goals and plan should parse");
        let plan = parsed.plan.expect("plan present");
        assert_eq!(plan.understands_user_intent, Some(97));
        assert_eq!(
            plan.user_intention.as_deref(),
            Some("make repository search feel instant")
        );
        let goals = parsed.goals.expect("goals present");
        assert_eq!(goals[0].closed_feedback_loop, Some(95));
        assert_eq!(
            goals[0].feedback_loop.as_deref(),
            Some("run the grep benchmark and compare p50")
        );
        // Runtime parsing remains backward-compatible with stored or older
        // provider payloads even though the advertised schema requires the field.
        assert_eq!(goals[1].feedback_loop, None);
        assert_eq!(goals[1].group, None);
    }

    #[test]
    fn stringified_plan_object_is_accepted() {
        let parsed = parse(json!({
            "plan": "{\"user_intention\":\"ship it\",\"understands_user_intent\":\"96\"}"
        }))
        .expect("stringified plan should parse");
        let plan = parsed.plan.expect("plan present");
        assert_eq!(plan.user_intention.as_deref(), Some("ship it"));
        assert_eq!(plan.understands_user_intent, Some(96));
    }

    #[test]
    fn accepts_legacy_plan_alignment_key_but_serializes_the_new_name() {
        let parsed = parse(json!({
            "plan": {"user_intention_alignment": "97"}
        }))
        .expect("legacy alignment key should remain readable");
        let plan = parsed.plan.expect("plan present");
        assert_eq!(plan.understands_user_intent, Some(97));

        let serialized = serde_json::to_value(plan).expect("plan should serialize");
        assert_eq!(serialized["understands_user_intent"], 97);
        assert!(serialized.get("user_intention_alignment").is_none());

        let legacy_field: TodoPlanField = serde_json::from_str("\"user_intention_alignment\"")
            .expect("legacy plan-change field should deserialize");
        assert_eq!(legacy_field, TodoPlanField::UnderstandsUserIntent);
        assert_eq!(
            serde_json::to_string(&legacy_field).expect("plan field should serialize"),
            "\"understands_user_intent\""
        );
    }

    fn goal(group: Option<&str>, score: u8) -> TodoGoal {
        TodoGoal {
            group: group.map(str::to_string),
            closed_feedback_loop: Some(score),
            ..Default::default()
        }
    }

    /// A plan whose intent assessment clears the private gate, so goal-level
    /// tests observe only closed feedback loop behavior.
    fn aligned_plan() -> TodoPlan {
        TodoPlan {
            user_intention: Some("understood".to_string()),
            understands_user_intent: Some(100),
            understands_user_intent_history: vec![100],
        }
    }

    #[test]
    fn merge_goals_retains_unmentioned_goals() {
        let stored = vec![goal(Some("a"), 20), goal(Some("b"), 90)];
        // Rewrite goal 'a', leave 'b' alone.
        let merged = merge_goals(&stored, Some(vec![goal(Some(" a "), 30)]));
        assert_eq!(merged.len(), 2);
        assert_eq!(merged[0].group.as_deref(), Some("a"));
        assert_eq!(merged[0].closed_feedback_loop, Some(30));
        assert_eq!(merged[1].group.as_deref(), Some("b"));
        // No incoming goals: stored goals unchanged.
        assert_eq!(merge_goals(&stored, None).len(), 2);
    }

    #[test]
    fn merge_plan_retains_stored_intent_when_update_omits_fields() {
        let stored = TodoPlan {
            user_intention: Some("make search feel instant".to_string()),
            understands_user_intent: Some(60),
            understands_user_intent_history: vec![60],
        };

        let merged = merge_plan(
            &stored,
            Some(TodoPlan {
                user_intention: None,
                understands_user_intent: Some(95),
                ..Default::default()
            }),
        );
        assert_eq!(
            merged.user_intention.as_deref(),
            Some("make search feel instant")
        );
        assert_eq!(merged.understands_user_intent, Some(95));

        // An omitted plan leaves the stored assessment untouched.
        assert_eq!(merge_plan(&stored, None), stored);
    }

    #[test]
    fn plan_change_reports_only_updated_intent_fields() {
        let before = aligned_plan();
        let after = TodoPlan {
            user_intention: Some("understood better".to_string()),
            ..before.clone()
        };

        let change = plan_change(&before, &after).expect("intent change should be reported");
        assert_eq!(change.fields, vec![TodoPlanField::UserIntention]);
        assert_eq!(change.before.as_ref(), Some(&before));
        assert_eq!(change.after.as_ref(), Some(&after));
        assert!(plan_change(&before, &before).is_none());
    }

    fn open_todo(group: Option<&str>) -> TodoItem {
        TodoItem {
            id: "t1".to_string(),
            content: "work".to_string(),
            status: "in_progress".to_string(),
            priority: "high".to_string(),
            group: group.map(str::to_string),
            ..Default::default()
        }
    }

    #[test]
    fn ownership_gate_output_preserves_the_saved_todo_card() {
        let todos = vec![open_todo(Some("ship"))];
        let plan = aligned_plan();
        let goals = vec![goal(Some("ship"), 96)];
        let output = build_todo_output(
            todos.clone(),
            plan.clone(),
            goals.clone(),
            None,
            None,
            [TODO_OWNERSHIP_CONTINUATION_MESSAGE.to_string()],
        )
        .expect("ownership gate should produce a structured todo result");

        assert_eq!(output.title.as_deref(), Some("1 todos"));
        assert!(output.output.starts_with('['));
        assert!(output.output.contains("\"status\": \"in_progress\""));
        assert!(output.output.contains(TODO_OWNERSHIP_CONTINUATION_MESSAGE));
        assert_eq!(
            output.metadata,
            Some(json!({"todos": todos, "plan": plan, "goals": goals}))
        );
    }

    fn test_ctx(session_id: &str) -> ToolContext {
        ToolContext {
            session_id: session_id.to_string(),
            message_id: session_id.to_string(),
            tool_call_id: "call".to_string(),
            working_dir: None,
            stdin_request_tx: None,
            graceful_shutdown_signal: None,
            execution_mode: crate::tool::ToolExecutionMode::Direct,
        }
    }

    /// End-to-end through the real tool, which is what the model actually sees.
    /// A first plan write with honestly-moderate scores must come back clean:
    /// this is the exact case that previously returned two nudges and spent the
    /// turn re-justifying the plan instead of doing the work.
    #[tokio::test]
    async fn a_moderate_first_write_returns_no_continuation_and_records_instead() {
        let _guard = crate::storage::lock_test_env();
        let previous_home = std::env::var_os("JCODE_HOME");
        let dir = tempfile::TempDir::new().expect("tempdir");
        crate::env::set_var("JCODE_HOME", dir.path());
        let session = "gate-deferral-execute";

        let output = TodoTool::new()
            .execute(
                json!({
                    "todos": [{
                        "content": "make utf16 transcode faster",
                        "status": "in_progress",
                        "priority": "high",
                        "id": "opt",
                        "group": "speed",
                        "confidence": 70,
                    }],
                    "plan": {
                        "user_intention": "beat the baseline",
                        "understands_user_intent": 82,
                    },
                    "goals": [{
                        "group": "speed",
                        "closed_feedback_loop": 80,
                        "feedback_loop": "run ./grade and read the score",
                    }],
                }),
                test_ctx(session),
            )
            .await
            .expect("todo write should succeed");

        assert!(
            !output
                .output
                .contains(TODO_INTENT_UNDERSTANDING_CONTINUATION_MESSAGE),
            "a moderate first write must not be interrupted: {}",
            output.output
        );
        assert!(
            !output
                .output
                .to_ascii_lowercase()
                .contains("not high enough"),
            "no gate text should reach the model mid-turn: {}",
            output.output
        );

        // The points were recorded for the turn-end digest instead.
        let observations = crate::todo::load_gate_observations(session).expect("observations");
        assert_eq!(observations.len(), 2);

        // Histories are accumulating, which is what the digest reasons over.
        let plan = load_plan(session).expect("plan");
        assert_eq!(plan.understands_user_intent_history, vec![82]);
        let goals = load_goals(session).expect("goals");
        assert_eq!(goals[0].closed_feedback_loop_history, vec![80]);

        // Second write at a higher score: still silent, history grows, and the
        // digest now has the trajectory available.
        let output = TodoTool::new()
            .execute(
                json!({"plan": {"understands_user_intent": 97}}),
                test_ctx(session),
            )
            .await
            .expect("second write should succeed");
        assert!(
            !output
                .output
                .to_ascii_lowercase()
                .contains("not high enough")
        );
        let plan = load_plan(session).expect("plan");
        assert_eq!(plan.understands_user_intent_history, vec![82, 97]);

        // The climb does not erase the point. The turn began without solid
        // understanding, so the work done before it settled still needs a
        // re-check; the wording just reflects that it settled late.
        let observations = crate::todo::load_gate_observations(session).expect("observations");
        let goals = load_goals(session).expect("goals");
        let digest = crate::todo::build_gate_digest(&observations, &plan, &goals)
            .expect("both recorded points should be surfaced");
        assert!(digest.contains("started this work without understanding"));
        assert!(digest.contains("feedback loop"));

        match previous_home {
            Some(value) => crate::env::set_var("JCODE_HOME", value),
            None => crate::env::remove_var("JCODE_HOME"),
        }
    }

    #[test]
    fn goal_changes_include_only_updated_quality_fields() {
        let before = TodoGoal {
            group: Some("search".to_string()),
            closed_feedback_loop: Some(90),
            feedback_loop: Some("Run one benchmark".to_string()),
            end_to_end_ownership: None,
            ..Default::default()
        };
        let after = TodoGoal {
            closed_feedback_loop: Some(98),
            feedback_loop: Some("Run five benchmarks and compare p50".to_string()),
            ..before.clone()
        };

        let changes = goal_changes(&[before.clone()], &[after.clone()]);

        assert_eq!(changes.len(), 1);
        assert_eq!(changes[0].before.as_ref(), Some(&before));
        assert_eq!(changes[0].after.as_ref(), Some(&after));
        assert_eq!(
            changes[0].fields,
            vec![
                TodoGoalField::ClosedFeedbackLoop,
                TodoGoalField::FeedbackLoop,
            ]
        );
    }

    /// The core behavior change: a low score records an observation for the
    /// turn-end digest instead of interrupting the write, and repeated writes
    /// do not re-interrupt.
    #[test]
    fn low_open_goal_records_an_observation_without_interrupting() {
        let todos = vec![open_todo(Some("design"))];
        let plan = aligned_plan();
        let goals = vec![goal(Some("design"), 95), goal(Some("perf"), 96)];
        let (observations, nudges) = record_reframe_observations(&plan, &goals, &todos, &[]);

        assert!(
            nudges.is_empty(),
            "a low closed feedback loop score must not interrupt the write"
        );
        assert_eq!(
            observations,
            vec![GateObservation {
                kind: GateObservationKind::ClosedFeedbackLoop,
                group: Some("design".to_string()),
                score: Some(95),
            }]
        );
        // A subsequent write still records, still does not interrupt.
        let (again, nudges) = record_reframe_observations(&plan, &goals, &todos, &[]);
        assert_eq!(again, observations);
        assert!(nudges.is_empty());
    }

    #[test]
    fn low_intent_is_plan_level_and_independent_of_goals() {
        let todos = vec![open_todo(Some("coverage"))];
        let plan = TodoPlan {
            user_intention: Some("partially understood".to_string()),
            understands_user_intent: Some(95),
            understands_user_intent_history: vec![95],
        };
        let (observations, nudges) =
            record_reframe_observations(&plan, &[goal(Some("coverage"), 96)], &todos, &[]);

        assert_eq!(
            observations,
            vec![GateObservation {
                kind: GateObservationKind::IntentUnderstanding,
                group: None,
                score: Some(95),
            }]
        );
        // 95 is below threshold but nowhere near severe, so exploration is
        // given the chance to resolve it rather than being interrupted.
        assert!(nudges.is_empty());
    }

    /// The single retained immediate nudge: the agent's first plan write says it
    /// does not understand the task at all, and a whole turn of wrong work
    /// cannot be undone at turn end.
    #[test]
    fn severely_low_first_intent_still_nudges_immediately() {
        let todos = vec![open_todo(None)];
        let plan = TodoPlan {
            user_intention: Some("guessing".to_string()),
            understands_user_intent: Some(40),
            understands_user_intent_history: vec![40],
        };
        let (_, nudges) = record_reframe_observations(&plan, &[], &todos, &[]);
        assert_eq!(nudges, vec![TODO_INTENT_UNDERSTANDING_CONTINUATION_MESSAGE]);
        assert!(!nudges[0].contains("40"));
        assert!(!nudges[0].to_ascii_lowercase().contains("threshold"));

        // Once the plan has a history, the same severe score is deferred to the
        // digest rather than nudged again on every write.
        let later = TodoPlan {
            understands_user_intent_history: vec![40, 41],
            ..plan
        };
        let (_, nudges) = record_reframe_observations(&later, &[], &todos, &[]);
        assert!(nudges.is_empty());
    }

    /// Work that was already complete before this write is grandfathered: the
    /// turn cannot go back and improve a loop over work it did not do.
    #[test]
    fn work_already_closed_before_this_write_records_nothing() {
        let mut done = open_todo(None);
        done.status = "completed".to_string();
        let already = vec![done.clone()];
        let (observations, nudges) = record_reframe_observations(
            &TodoPlan::default(),
            &[goal(None, 10)],
            &already,
            &already,
        );
        assert!(observations.is_empty());
        assert!(nudges.is_empty());
    }

    /// A group created and finished in one write must still be observed. This is
    /// where a weak feedback loop hides best: declare it done in one step and no
    /// "still open" check ever sees it.
    #[test]
    fn a_group_closed_by_this_write_is_still_observed() {
        let mut done = open_todo(Some("one shot"));
        done.status = "completed".to_string();
        let (observations, nudges) = record_reframe_observations(
            &aligned_plan(),
            &[goal(Some("one shot"), 40)],
            &[done],
            &[],
        );
        assert!(nudges.is_empty());
        assert_eq!(
            observations,
            vec![GateObservation {
                kind: GateObservationKind::ClosedFeedbackLoop,
                group: Some("one shot".to_string()),
                score: Some(40),
            }]
        );
    }

    #[test]
    fn both_weak_links_are_recorded_independently() {
        let todos = vec![open_todo(Some("coverage"))];
        let plan = TodoPlan {
            user_intention: Some("partially understood".to_string()),
            understands_user_intent: Some(95),
            understands_user_intent_history: vec![95],
        };
        let (observations, _) =
            record_reframe_observations(&plan, &[goal(Some("coverage"), 95)], &todos, &[]);
        assert_eq!(
            observations
                .iter()
                .map(|observation| observation.kind)
                .collect::<Vec<_>>(),
            vec![
                GateObservationKind::IntentUnderstanding,
                GateObservationKind::ClosedFeedbackLoop,
            ]
        );
    }

    #[test]
    fn missing_quality_scores_still_record_observations() {
        let todos = vec![open_todo(Some("coverage"))];
        let mut goal = goal(Some("coverage"), 96);
        goal.closed_feedback_loop = None;

        let (observations, _) =
            record_reframe_observations(&TodoPlan::default(), &[goal], &todos, &[]);
        assert_eq!(
            observations
                .iter()
                .map(|observation| observation.kind)
                .collect::<Vec<_>>(),
            vec![
                GateObservationKind::IntentUnderstanding,
                GateObservationKind::ClosedFeedbackLoop,
            ]
        );
    }

    /// Groups already complete before this write are grandfathered, so a
    /// long-lived session does not re-flag work from previous turns.
    #[test]
    fn observations_skip_goals_closed_in_an_earlier_write() {
        let mut done = open_todo(Some("legacy"));
        done.status = "completed".to_string();
        let already = vec![done];
        let goals = vec![goal(Some("legacy"), 10)];
        let (observations, _) =
            record_reframe_observations(&aligned_plan(), &goals, &already, &already);
        assert!(observations.is_empty());
    }

    #[test]
    fn observations_cover_the_ungrouped_implicit_goal() {
        let todos = vec![open_todo(None)];
        let goals = vec![goal(None, 15)];
        let (observations, _) = record_reframe_observations(&aligned_plan(), &goals, &todos, &[]);
        assert_eq!(
            observations,
            vec![GateObservation {
                kind: GateObservationKind::ClosedFeedbackLoop,
                group: None,
                score: Some(15),
            }]
        );
    }

    /// Tool-owned histories are the substrate the turn-end digest reasons over,
    /// so a model-supplied trail must not be able to fabricate a climb.
    #[test]
    fn plan_and_goal_score_histories_are_tool_maintained() {
        let stored = TodoPlan {
            user_intention: Some("ship it".to_string()),
            understands_user_intent: Some(70),
            understands_user_intent_history: vec![70],
        };
        let merged = merge_plan(
            &stored,
            Some(TodoPlan {
                understands_user_intent: Some(88),
                // Forged trail: discarded in favor of the stored one.
                understands_user_intent_history: vec![1, 2, 3],
                ..Default::default()
            }),
        );
        assert_eq!(merged.understands_user_intent_history, vec![70, 88]);
        assert_eq!(merged.user_intention.as_deref(), Some("ship it"));

        // Re-sending the same score does not manufacture an extra step.
        let merged = merge_plan(
            &merged,
            Some(TodoPlan {
                understands_user_intent: Some(88),
                ..Default::default()
            }),
        );
        assert_eq!(merged.understands_user_intent_history, vec![70, 88]);

        let stored_goals = merge_goals(&[], Some(vec![goal(Some("perf"), 60)]));
        assert_eq!(stored_goals[0].closed_feedback_loop_history, vec![60]);
        let merged_goals = merge_goals(&stored_goals, Some(vec![goal(Some("perf"), 91)]));
        assert_eq!(merged_goals[0].closed_feedback_loop_history, vec![60, 91]);
    }

    /// A write that revises one assessment must not erase the others, or the
    /// digest would read a stale `None` and re-raise a resolved point.
    #[test]
    fn omitted_goal_fields_inherit_the_stored_assessment() {
        let stored = merge_goals(
            &[],
            Some(vec![TodoGoal {
                group: Some("perf".to_string()),
                closed_feedback_loop: Some(97),
                feedback_loop: Some("cargo bench".to_string()),
                end_to_end_ownership: Some(96),
                ..Default::default()
            }]),
        );
        let merged = merge_goals(
            &stored,
            Some(vec![TodoGoal {
                group: Some("perf".to_string()),
                ..Default::default()
            }]),
        );
        assert_eq!(merged[0].closed_feedback_loop, Some(97));
        assert_eq!(merged[0].feedback_loop.as_deref(), Some("cargo bench"));
        assert_eq!(merged[0].end_to_end_ownership, Some(96));
    }

    #[test]
    fn garbage_string_still_errors() {
        assert!(parse(json!({"todos": "not json at all"})).is_err());
    }

    /// Sessions and model calls written before the rename carry
    /// `hill_climbability`. Those must keep loading, or resuming an old session
    /// silently drops its goal assessments and re-raises resolved gate points.
    #[test]
    fn pre_rename_hill_climbability_keys_still_load() {
        let goal: crate::todo::TodoGoal = serde_json::from_value(json!({
            "group": "optimize grep",
            "hill_climbability": 91,
            "hill_climbability_history": [70, 91],
            "feedback_loop": "cargo bench grep"
        }))
        .expect("the pre-rename key must still deserialize");
        assert_eq!(goal.closed_feedback_loop, Some(91));
        assert_eq!(goal.closed_feedback_loop_history, vec![70, 91]);

        let goals = parse(json!({
            "goals": [{"group": "optimize grep", "hill_climbability": "88", "feedback_loop": "bench"}]
        }))
        .expect("a pre-rename tool call must still parse")
        .goals
        .expect("goals should be present");
        assert_eq!(goals[0].closed_feedback_loop, Some(88));
    }

    fn history_todo(id: &str, confidence: Option<u8>, history: Vec<u8>) -> TodoItem {
        TodoItem {
            id: id.to_string(),
            content: format!("todo {id}"),
            status: "in_progress".to_string(),
            priority: "high".to_string(),
            confidence,
            confidence_history: history,
            ..Default::default()
        }
    }

    #[test]
    fn confidence_history_appends_changes_and_skips_repeats() {
        let previous = vec![history_todo("1", Some(75), vec![75])];
        // Same confidence again: no new entry.
        let mut incoming = vec![history_todo("1", Some(75), Vec::new())];
        merge_confidence_history(&previous, &mut incoming);
        assert_eq!(incoming[0].confidence_history, vec![75]);
        // Raised confidence: appended.
        let mut incoming = vec![history_todo("1", Some(90), Vec::new())];
        merge_confidence_history(&previous, &mut incoming);
        assert_eq!(incoming[0].confidence_history, vec![75, 90]);
    }

    #[test]
    fn confidence_history_records_completion_confidence() {
        let previous = vec![history_todo("1", Some(75), vec![75])];
        let mut done = history_todo("1", Some(100), Vec::new());
        done.status = "completed".to_string();
        done.completion_confidence = Some(100);
        let mut incoming = vec![done];
        merge_confidence_history(&previous, &mut incoming);
        // 75 (planning) -> 100 (final bulk stamp): the spike stays visible.
        assert_eq!(incoming[0].confidence_history, vec![75, 100]);
    }

    #[test]
    fn completion_write_contributes_only_one_final_confidence_observation() {
        let previous = vec![history_todo("1", Some(70), vec![70])];
        let mut done = history_todo("1", Some(90), Vec::new());
        done.status = "completed".to_string();
        done.completion_confidence = Some(100);

        let mut incoming = vec![done];
        merge_confidence_history(&previous, &mut incoming);

        assert_eq!(incoming[0].confidence_history, vec![70, 100]);
    }

    #[test]
    fn confidence_history_seeds_legacy_todos_before_completion() {
        let previous = vec![history_todo("1", Some(70), Vec::new())];
        let mut done = history_todo("1", Some(90), Vec::new());
        done.status = "completed".to_string();
        done.completion_confidence = Some(100);

        let mut incoming = vec![done];
        merge_confidence_history(&previous, &mut incoming);

        assert_eq!(incoming[0].confidence_history, vec![70, 100]);
    }

    #[test]
    fn confidence_history_ignores_model_supplied_history_for_new_todos() {
        let mut incoming = vec![history_todo("9", Some(80), vec![1, 2, 3])];
        merge_confidence_history(&[], &mut incoming);
        assert_eq!(incoming[0].confidence_history, vec![80]);
    }
}
