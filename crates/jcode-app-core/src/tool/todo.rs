use super::{Tool, ToolContext, ToolOutput};
use crate::bus::{Bus, BusEvent, TodoEvent};
use crate::todo::{
    LOW_ALIGNMENT_SCORE, LOW_HILL_CLIMBABILITY, TODO_ALIGNMENT_CONTINUATION_MESSAGE,
    TODO_HILL_CLIMBABILITY_CONTINUATION_MESSAGE, TODO_OWNERSHIP_CONTINUATION_MESSAGE, TodoGoal,
    TodoGoalChange, TodoGoalField, TodoItem, load_goals, load_todos,
    newly_completed_groups_have_sufficient_ownership, save_goals, save_todos,
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
}

/// Normalize a goal's group label: trimmed, with empty/whitespace collapsed
/// to `None` (the implicit goal of an ungrouped list).
fn goal_group_key(group: Option<&str>) -> Option<String> {
    group
        .map(str::trim)
        .filter(|group| !group.is_empty())
        .map(str::to_string)
}

/// Merge incoming goal assessments with the stored ones.
///
/// Incoming goals win per group key; stored goals for groups the write does
/// not mention are retained (a todo update should not silently discard goal
/// assessments).
fn merge_goals(stored: &[TodoGoal], incoming: Option<Vec<TodoGoal>>) -> Vec<TodoGoal> {
    let Some(incoming) = incoming else {
        return stored.to_vec();
    };
    let mut merged: Vec<TodoGoal> = Vec::new();
    for mut goal in incoming {
        goal.group = goal_group_key(goal.group.as_deref());
        // User intention describes why the user asked for the goal and should
        // remain stable while the agent revises metrics, feedback, or scores.
        // An omitted intention therefore inherits the current value for the
        // same goal. Sending an empty string remains an explicit way to clear
        // its visible value.
        if goal.user_intention.is_none() {
            goal.user_intention = merged
                .iter()
                .find(|existing| existing.group == goal.group)
                .or_else(|| {
                    stored
                        .iter()
                        .find(|existing| goal_group_key(existing.group.as_deref()) == goal.group)
                })
                .and_then(|existing| existing.user_intention.clone());
        }
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
    if before.and_then(|goal| goal.user_intention.as_ref())
        != after.and_then(|goal| goal.user_intention.as_ref())
    {
        fields.push(TodoGoalField::UserIntention);
    }
    if before.and_then(|goal| goal.alignment_score) != after.and_then(|goal| goal.alignment_score) {
        fields.push(TodoGoalField::AlignmentScore);
    }
    if before.and_then(|goal| goal.hill_climbability)
        != after.and_then(|goal| goal.hill_climbability)
    {
        fields.push(TodoGoalField::HillClimbability);
    }
    if before.and_then(|goal| goal.objective.as_ref())
        != after.and_then(|goal| goal.objective.as_ref())
    {
        fields.push(TodoGoalField::Objective);
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

/// Reframe nudges for goals whose representation coverage or hill-climbability
/// is too low to support a trustworthy feedback loop.
///
/// A low score means there is no credible metric to iterate against, so the
/// objective must be reframed into something measurable. The nudge is
/// intentionally returned on every applicable todo write until the goal reaches
/// the threshold or its work closes.
fn take_reframe_nudges(goals: &[TodoGoal], todos: &[TodoItem]) -> Vec<String> {
    let mut nudges = Vec::new();
    for goal in goals {
        let group_open = todos.iter().any(|todo| {
            goal_group_key(todo.group.as_deref()) == goal.group
                && todo.status != "completed"
                && todo.status != "cancelled"
        });
        if !group_open {
            continue;
        }
        if goal
            .alignment_score
            .is_none_or(|score| score < LOW_ALIGNMENT_SCORE)
        {
            nudges.push(TODO_ALIGNMENT_CONTINUATION_MESSAGE.to_string());
        }
        if goal
            .hill_climbability
            .is_none_or(|score| score < LOW_HILL_CLIMBABILITY)
        {
            nudges.push(TODO_HILL_CLIMBABILITY_CONTINUATION_MESSAGE.to_string());
        }
    }
    nudges
}

fn build_todo_output(
    todos: Vec<TodoItem>,
    goals: Vec<TodoGoal>,
    goal_changes: Option<Vec<TodoGoalChange>>,
    continuations: impl IntoIterator<Item = String>,
) -> Result<ToolOutput> {
    let remaining = todos
        .iter()
        .filter(|todo| todo.status != "completed")
        .count();
    let mut text = serde_json::to_string_pretty(&todos)?;
    if !goals.is_empty() {
        text.push_str("\n\nGoals:\n");
        text.push_str(&serde_json::to_string_pretty(&goals)?);
    }
    if let Some(goal_changes) = goal_changes.as_ref().filter(|changes| !changes.is_empty()) {
        text.push_str("\n\nGoal updates:\n");
        text.push_str(&serde_json::to_string_pretty(goal_changes)?);
    }
    for continuation in continuations {
        text.push_str("\n\n");
        text.push_str(&continuation);
    }
    let mut metadata = json!({"todos": todos, "goals": goals});
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
                                "description": "Optional group label. Todos sharing a group render together under one header. Use one group per coherent goal (e.g. 'optimize rendering'). When the user steers into new work, start a new group instead of renaming the existing one. Omit for an ungrouped flat list."
                            },
                            "confidence": {
                                "type": "integer",
                                "minimum": 0,
                                "maximum": 100,
                                "description": "Self-assessed confidence, 0-100, that this todo can be completed correctly. Reassess it as evidence accumulates while working."
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
                "goals": {
                    "type": "array",
                    "description": "Optional goal-level assessments, one per todo group. Use group: null for an ungrouped list. Stored assessments for groups omitted from an update are retained.",
                    "items": {
                        "type": "object",
                        "required": ["alignment_score", "hill_climbability", "feedback_loop"],
                        "properties": {
                            "group": {
                                "type": "string",
                                "description": "Group label this goal describes. Omit or null for the ungrouped list."
                            },
                            "user_intention": {
                                "type": "string",
                                "description": "Optional concise statement of the user's underlying reason or desired outcome for this goal. Omit on later updates to retain the stored intention."
                            },
                            "alignment_score": {
                                "type": "integer",
                                "minimum": 0,
                                "maximum": 100,
                                "description": "Self-assessment, 0-100, of how faithfully the objective and feedback loop together represent the user's stated request and underlying intention. Score the weaker link rather than averaging: (1) whether the objective captures the intended outcome, and (2) whether the feedback loop can detect achievement or failure across every material requirement, constraint, integration path, edge case, and necessary follow-through. This measures representation coverage, not implementation progress or completion confidence."
                            },
                            "hill_climbability": {
                                "type": "integer",
                                "minimum": 0,
                                "maximum": 100,
                                "description": "Self-assessment, 0-100, of how readily progress toward this goal can be measured and compared across iterations."
                            },
                            "objective": {
                                "type": "string",
                                "description": "Optional concise measurable outcome that faithfully represents the user's intention, including material constraints and necessary follow-through."
                            },
                            "feedback_loop": {
                                "type": "string",
                                "description": "Concrete process, observation, or check used to compare progress across iterations and detect whether the objective and user intention are satisfied or violated. It should cover material requirements, constraints, integration paths, edge cases, and necessary follow-through."
                            },
                            "end_to_end_ownership": {
                                "type": "integer",
                                "minimum": 0,
                                "maximum": 100,
                                "description": "Completion-time self-assessment, 0-100, of whether the full intended user outcome and its necessary follow-through were delivered, rather than only the immediate implementation. Use only when completing the goal."
                            }
                        }
                    }
                }
            }
        })
    }

    async fn execute(&self, input: Value, ctx: ToolContext) -> Result<ToolOutput> {
        let params: TodoInput = serde_json::from_value(normalize_todo_input(input))?;
        let operation = if params.todos.is_some() || params.goals.is_some() {
            "write"
        } else {
            "read"
        };
        let result = if params.todos.is_some() || params.goals.is_some() {
            // Goals-only writes keep the stored todo list.
            let previous = load_todos(&ctx.session_id).unwrap_or_default();
            let mut todos = params.todos.unwrap_or_else(|| previous.clone());
            merge_confidence_history(&previous, &mut todos);
            (|| {
                let stored_goals = load_goals(&ctx.session_id).unwrap_or_default();
                let goals = merge_goals(&stored_goals, params.goals);
                if !newly_completed_groups_have_sufficient_ownership(&previous, &todos, &goals) {
                    crate::telemetry::record_todo_gate(crate::telemetry::TodoGateKind::Ownership);
                    return build_todo_output(
                        previous,
                        stored_goals,
                        None,
                        [TODO_OWNERSHIP_CONTINUATION_MESSAGE.to_string()],
                    );
                }
                let nudges = take_reframe_nudges(&goals, &todos);
                for _ in &nudges {
                    crate::telemetry::record_todo_gate(
                        crate::telemetry::TodoGateKind::HillClimbability,
                    );
                }
                // Goal-only writes, especially hill-climbability quality-gate
                // retries, should render the assessment fields that changed
                // instead of repeating an otherwise identical todo plan.
                let concise_goal_changes = (todos == previous && !stored_goals.is_empty())
                    .then(|| goal_changes(&stored_goals, &goals));
                save_todos(&ctx.session_id, &todos)?;
                save_goals(&ctx.session_id, &goals)?;

                Bus::global().publish(BusEvent::TodoUpdated(TodoEvent {
                    session_id: ctx.session_id.clone(),
                    todos: todos.clone(),
                }));

                build_todo_output(todos, goals, concise_goal_changes, nudges)
            })()
        } else {
            (|| {
                let todos = load_todos(&ctx.session_id)?;
                let goals = load_goals(&ctx.session_id).unwrap_or_default();
                build_todo_output(todos, goals, None, Vec::new())
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
        assert_eq!(props.len(), 3);
        assert!(props.contains_key("intent"));
        assert!(props.contains_key("todos"));
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
        assert!(!item_props.contains_key("hill_climbability"));
        assert_eq!(
            item_props["confidence"]["description"],
            "Self-assessed confidence, 0-100, that this todo can be completed correctly. Reassess it as evidence accumulates while working."
        );

        let goal_props = props["goals"]
            .get("items")
            .and_then(|v| v.get("properties"))
            .and_then(|v| v.as_object())
            .expect("goals should describe item objects");
        assert!(goal_props.contains_key("group"));
        assert!(goal_props.contains_key("user_intention"));
        assert!(goal_props.contains_key("alignment_score"));
        assert!(!goal_props.contains_key("user_intention_alignment"));
        assert!(goal_props.contains_key("hill_climbability"));
        assert!(goal_props.contains_key("objective"));
        assert!(goal_props.contains_key("feedback_loop"));
        assert!(goal_props.contains_key("end_to_end_ownership"));
        assert_eq!(goal_props.len(), 7);

        let goal_required = props["goals"]["items"]["required"]
            .as_array()
            .expect("goals should advertise required fields");
        assert!(
            goal_required
                .iter()
                .any(|value| value == "hill_climbability")
        );
        assert!(goal_required.iter().any(|value| value == "feedback_loop"));
        assert!(goal_required.iter().any(|value| value == "alignment_score"));

        let alignment_description = goal_props["alignment_score"]
            .get("description")
            .and_then(Value::as_str)
            .expect("alignment score should describe representation coverage");
        for required_concept in [
            "weaker link",
            "objective captures the intended outcome",
            "feedback loop can detect achievement or failure",
            "every material requirement",
            "integration path",
            "edge case",
            "necessary follow-through",
            "not implementation progress",
        ] {
            assert!(
                alignment_description.contains(required_concept),
                "alignment description omitted {required_concept}: {alignment_description}"
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
        assert!(ownership_description.contains("Use only when completing the goal."));
        assert!(ownership_description.contains("full intended user outcome"));
        assert!(ownership_description.contains("necessary follow-through"));
        assert!(!ownership_description.contains("90"));
        assert!(!ownership_description.contains("91"));
        assert!(
            !ownership_description
                .to_ascii_lowercase()
                .contains("threshold")
        );

        let hill_description = goal_props["hill_climbability"]
            .get("description")
            .and_then(Value::as_str)
            .expect("hill-climbability should describe the assessment neutrally");
        assert!(!hill_description.contains(&LOW_HILL_CLIMBABILITY.to_string()));
        assert!(!hill_description.to_ascii_lowercase().contains("threshold"));

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
    fn accepts_goals_including_string_coercion() {
        let input = json!({
            "goals": [
                {"group": "optimize grep", "user_intention": "make repository search feel instant", "alignment_score": "97", "hill_climbability": "95", "objective": "p50 under 50ms", "feedback_loop": "run the grep benchmark and compare p50"},
                {"hill_climbability": 20}
            ]
        });
        let parsed = parse(input).expect("goals should parse");
        let goals = parsed.goals.expect("goals present");
        assert_eq!(goals[0].hill_climbability, Some(95));
        assert_eq!(goals[0].alignment_score, Some(97));
        assert_eq!(
            goals[0].user_intention.as_deref(),
            Some("make repository search feel instant")
        );
        assert_eq!(goals[0].objective.as_deref(), Some("p50 under 50ms"));
        assert_eq!(
            goals[0].feedback_loop.as_deref(),
            Some("run the grep benchmark and compare p50")
        );
        // Runtime parsing remains backward-compatible with stored or older
        // provider payloads even though the advertised schema requires the field.
        assert_eq!(goals[1].alignment_score, None);
        assert_eq!(goals[1].feedback_loop, None);
        assert_eq!(goals[1].group, None);
    }

    #[test]
    fn accepts_legacy_alignment_key_but_serializes_the_new_name() {
        let parsed = parse(json!({
            "goals": [{
                "user_intention_alignment": "97",
                "hill_climbability": 96,
                "feedback_loop": "compare every requirement against observed behavior"
            }]
        }))
        .expect("legacy alignment key should remain readable");
        let goal = parsed.goals.expect("goals present").remove(0);
        assert_eq!(goal.alignment_score, Some(97));

        let serialized = serde_json::to_value(goal).expect("goal should serialize");
        assert_eq!(serialized["alignment_score"], 97);
        assert!(serialized.get("user_intention_alignment").is_none());

        let legacy_field: TodoGoalField = serde_json::from_str("\"user_intention_alignment\"")
            .expect("legacy goal-change field should deserialize");
        assert_eq!(legacy_field, TodoGoalField::AlignmentScore);
        assert_eq!(
            serde_json::to_string(&legacy_field).expect("goal field should serialize"),
            "\"alignment_score\""
        );
    }

    fn goal(group: Option<&str>, score: u8) -> TodoGoal {
        TodoGoal {
            group: group.map(str::to_string),
            alignment_score: Some(100),
            hill_climbability: Some(score),
            ..Default::default()
        }
    }

    #[test]
    fn merge_goals_retains_unmentioned_goals() {
        let stored = vec![goal(Some("a"), 20), goal(Some("b"), 90)];
        // Rewrite goal 'a', leave 'b' alone.
        let merged = merge_goals(&stored, Some(vec![goal(Some(" a "), 30)]));
        assert_eq!(merged.len(), 2);
        assert_eq!(merged[0].group.as_deref(), Some("a"));
        assert_eq!(merged[0].hill_climbability, Some(30));
        assert_eq!(merged[1].group.as_deref(), Some("b"));
        // No incoming goals: stored goals unchanged.
        assert_eq!(merge_goals(&stored, None).len(), 2);
    }

    #[test]
    fn merge_goals_retains_user_intention_when_update_omits_it() {
        let mut stored_goal = goal(Some("a"), 20);
        stored_goal.user_intention = Some("make search feel instant".to_string());
        stored_goal.alignment_score = Some(60);
        let stored = vec![stored_goal];

        let mut updated_goal = goal(Some("a"), 90);
        updated_goal.alignment_score = Some(95);
        let merged = merge_goals(&stored, Some(vec![updated_goal]));

        assert_eq!(merged[0].hill_climbability, Some(90));
        assert_eq!(merged[0].alignment_score, Some(95));
        assert_eq!(
            merged[0].user_intention.as_deref(),
            Some("make search feel instant")
        );
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
        let goals = vec![goal(Some("ship"), 96)];
        let output = build_todo_output(
            todos.clone(),
            goals.clone(),
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
            Some(json!({"todos": todos, "goals": goals}))
        );
    }

    #[test]
    fn goal_changes_include_only_updated_quality_fields() {
        let before = TodoGoal {
            group: Some("search".to_string()),
            user_intention: Some("make search feel instant".to_string()),
            alignment_score: Some(99),
            hill_climbability: Some(90),
            objective: Some("Keep p50 below 50ms".to_string()),
            feedback_loop: Some("Run one benchmark".to_string()),
            end_to_end_ownership: None,
        };
        let after = TodoGoal {
            hill_climbability: Some(98),
            feedback_loop: Some("Run five benchmarks and compare p50".to_string()),
            ..before.clone()
        };

        let changes = goal_changes(&[before.clone()], &[after.clone()]);

        assert_eq!(changes.len(), 1);
        assert_eq!(changes[0].before.as_ref(), Some(&before));
        assert_eq!(changes[0].after.as_ref(), Some(&after));
        assert_eq!(
            changes[0].fields,
            vec![TodoGoalField::HillClimbability, TodoGoalField::FeedbackLoop,]
        );
    }

    #[test]
    fn reframe_nudge_recurs_for_every_low_open_goal_write() {
        let todos = vec![open_todo(Some("design"))];
        let goals = vec![goal(Some("design"), 95), goal(Some("perf"), 96)];
        let nudges = take_reframe_nudges(&goals, &todos);
        assert_eq!(nudges.len(), 1);
        assert_eq!(nudges[0], TODO_HILL_CLIMBABILITY_CONTINUATION_MESSAGE);
        assert!(!nudges[0].contains("95"));
        assert!(nudges[0].contains("hill-climbability"));
        assert!(!nudges[0].to_ascii_lowercase().contains("threshold"));
        assert!(!nudges[0].to_ascii_lowercase().contains("gate"));
        // A subsequent write receives the same generic guidance while the
        // private condition remains applicable.
        assert_eq!(take_reframe_nudges(&goals, &todos).len(), 1);
    }

    #[test]
    fn alignment_nudge_requires_comprehensive_objective_and_feedback_coverage() {
        let todos = vec![open_todo(Some("coverage"))];
        let mut goal = goal(Some("coverage"), 96);
        goal.alignment_score = Some(95);
        let nudges = take_reframe_nudges(&[goal], &todos);

        assert_eq!(nudges, vec![TODO_ALIGNMENT_CONTINUATION_MESSAGE]);
        assert!(nudges[0].contains("objective"));
        assert!(nudges[0].contains("feedback loop"));
        assert!(nudges[0].contains("every material requirement"));
        assert!(!nudges[0].contains("95"));
        assert!(!nudges[0].to_ascii_lowercase().contains("threshold"));
    }

    #[test]
    fn alignment_and_hill_nudges_report_both_independent_weak_links() {
        let todos = vec![open_todo(Some("coverage"))];
        let mut goal = goal(Some("coverage"), 95);
        goal.alignment_score = Some(95);
        let nudges = take_reframe_nudges(&[goal], &todos);

        assert_eq!(
            nudges,
            vec![
                TODO_ALIGNMENT_CONTINUATION_MESSAGE,
                TODO_HILL_CLIMBABILITY_CONTINUATION_MESSAGE,
            ]
        );
    }

    #[test]
    fn missing_quality_scores_do_not_bypass_open_goal_gates() {
        let todos = vec![open_todo(Some("coverage"))];
        let mut goal = goal(Some("coverage"), 96);
        goal.alignment_score = None;
        goal.hill_climbability = None;

        assert_eq!(
            take_reframe_nudges(&[goal], &todos),
            vec![
                TODO_ALIGNMENT_CONTINUATION_MESSAGE,
                TODO_HILL_CLIMBABILITY_CONTINUATION_MESSAGE,
            ]
        );
    }

    #[test]
    fn reframe_nudge_skips_closed_goals() {
        // Low goal whose todos are all completed: nothing to reframe.
        let mut done = open_todo(Some("legacy"));
        done.status = "completed".to_string();
        let goals = vec![goal(Some("legacy"), 10)];
        assert!(take_reframe_nudges(&goals, &[done]).is_empty());
    }

    #[test]
    fn reframe_nudge_covers_ungrouped_implicit_goal() {
        let todos = vec![open_todo(None)];
        let goals = vec![goal(None, 15)];
        let nudges = take_reframe_nudges(&goals, &todos);
        assert_eq!(nudges.len(), 1);
        assert_eq!(nudges[0], TODO_HILL_CLIMBABILITY_CONTINUATION_MESSAGE);
    }

    #[test]
    fn garbage_string_still_errors() {
        assert!(parse(json!({"todos": "not json at all"})).is_err());
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
