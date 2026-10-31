#![cfg_attr(test, allow(clippy::await_holding_lock))]
use super::{Tool, ToolContext, ToolOutput};
use crate::bus::{Bus, BusEvent, TodoEvent};
use crate::todo::{
    GateObservation, GateObservationKind, SEVERE_INTENT_MISUNDERSTANDING,
    TODO_INTENT_UNDERSTANDING_CONTINUATION_MESSAGE, TodoGoal, TodoGoalChange, TodoGoalField,
    TodoItem, TodoPlan, TodoPlanChange, TodoPlanField, append_gate_observations,
    feedback_loop_passes, intent_understanding_passes, load_goals, load_plan, load_todos,
    save_goals, save_plan, save_todos, update_todo_review_cycle,
};
use anyhow::{Result, bail};
use async_trait::async_trait;
use serde::Deserialize;
use serde_json::{Value, json};
use std::collections::HashMap;

pub struct TodoTool;

#[cfg(test)]
#[path = "todo_external_blocker_tests.rs"]
mod external_blocker_tests;

#[cfg(test)]
#[path = "todo_lane_dependency_tests.rs"]
mod lane_dependency_tests;

/// A prerequisite edit naming one member applies atomically to its entire lane.
fn patch_lane_after(todos: &mut [Value], id: &str, value: Value) -> Result<()> {
    let typed: Vec<TodoItem> = serde_json::from_value(Value::Array(todos.to_vec()))?;
    let target = typed
        .iter()
        .find(|todo| todo.id == id)
        .ok_or_else(|| anyhow::anyhow!("unknown todo {id:?}"))?;
    let lane = crate::todo::stages::lane_id(target);
    for (raw, todo) in todos.iter_mut().zip(&typed) {
        if crate::todo::stages::lane_id(todo) == lane {
            raw["lane_after"] = value.clone();
        }
    }
    Ok(())
}

fn validate_external_blockers(previous: &[TodoItem], todos: &[TodoItem]) -> Result<()> {
    let now = chrono::Utc::now();
    for todo in todos {
        if let Some(required) = &todo.completely_blocked
            && required.reason.trim().is_empty()
        {
            bail!(
                "completely_blocked.reason must be nonempty for todo {}",
                todo.id
            );
        }
        if let Some(blocker) = &todo.external_blocker {
            if blocker.reason.trim().is_empty() {
                bail!(
                    "external_blocker.reason must explain the external condition for todo {}",
                    todo.id
                );
            }
            let unchanged = previous
                .iter()
                .any(|old| old.id == todo.id && old.external_blocker == todo.external_blocker);
            if !unchanged && blocker.review_at <= now {
                bail!(
                    "external_blocker.review_at must be a future RFC3339 deadline for todo {}",
                    todo.id
                );
            }
        }
    }
    Ok(())
}

/// Validate the entire replacement before touching either todo storage or
/// background delivery. A terminal dependency remains valid but needs no wake.
async fn prepare_background_dependencies(
    manager: &crate::background::BackgroundTaskManager,
    session_id: &str,
    previous: &[TodoItem],
    todos: &[TodoItem],
) -> Result<()> {
    let mut ids: HashMap<&str, (bool, bool)> = HashMap::new();
    for todo in todos {
        let prior = previous.iter().find(|old| old.id == todo.id);
        let active = !crate::todo::todo_status_is_completed(&todo.status)
            && !crate::todo::todo_status_is_cancelled(&todo.status)
            && !crate::todo::todo_completely_blocked(todo);
        for id in &todo.waiting_on {
            // Check before the manager constructs any filesystem path.
            if id.is_empty()
                || !id
                    .bytes()
                    .all(|c| c.is_ascii_alphanumeric() || matches!(c, b'_' | b'-'))
            {
                bail!("Invalid waiting_on background task ID: {id}");
            }
            let is_new = !prior.is_some_and(|old| old.waiting_on.contains(id));
            let entry = ids.entry(id).or_default();
            entry.0 |= is_new;
            entry.1 |= active;
        }
    }
    let mut running = Vec::new();
    let mut awaits = Vec::new();
    for (id, (is_new, active)) in ids {
        if id.starts_with(crate::todo::await_dependency::ID_PREFIX) {
            let dependency =
                crate::todo::await_dependency::load(&crate::storage::runtime_dir(), id);
            match dependency {
                Some(dependency) => {
                    if dependency.session_id != session_id {
                        bail!("waiting_on await does not belong to this session: {id}");
                    }
                    if active && dependency.is_running() {
                        awaits.push(id);
                    }
                }
                None if is_new => bail!("Unknown waiting_on await dependency: {id}"),
                None => {}
            }
            continue;
        }
        let Some(task) = manager.status(id).await else {
            if is_new {
                bail!("Unknown waiting_on background task: {id}");
            }
            // Cleanup can remove historical task files. Retaining that same
            // todo's reference must not prevent unrelated or assessment writes.
            continue;
        };
        if task.task_id != id {
            bail!("Unknown waiting_on background task: {id}");
        }
        if task.session_id != session_id {
            bail!("waiting_on background task does not belong to this session: {id}");
        }
        if active && task.status == crate::bus::BackgroundTaskStatus::Running {
            running.push(id);
        }
    }
    for id in awaits {
        crate::server::subscribe_await_dependency(id, session_id)?;
    }
    for id in running {
        // The task may finish between validation and subscription. Terminal
        // dependencies do not block, and callers re-read current task state.
        manager.update_delivery(id, true, true).await?;
    }
    Ok(())
}

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

/// How a todo call wrote, for measuring whether agents use incremental edits
/// instead of resending the list. Counted locally in
/// `~/.jcode/stats/todo-write-modes.json`; nothing leaves the machine.
fn todo_write_mode(input: &Value) -> &'static str {
    let has = |key: &str| input.get(key).is_some_and(|value| !value.is_null());
    match (
        has("todos") || has("phases"),
        has("update") || has("add") || has("remove"),
    ) {
        (true, _) => "full",
        (false, true) => "patch",
        (false, false) if has("goals") || has("plan") => "assessment",
        _ => "read",
    }
}

fn record_todo_write_mode(input: &Value) {
    static COUNTER_LOCK: std::sync::Mutex<()> = std::sync::Mutex::new(());
    let Ok(_guard) = COUNTER_LOCK.lock() else {
        return;
    };
    let mode = todo_write_mode(input);
    let Ok(dir) = crate::storage::jcode_dir().map(|dir| dir.join("stats")) else {
        return;
    };
    let path = dir.join("todo-write-modes.json");
    let mut counts: HashMap<String, u64> = std::fs::read_to_string(&path)
        .ok()
        .and_then(|text| serde_json::from_str(&text).ok())
        .unwrap_or_default();
    *counts.entry(mode.to_string()).or_default() += 1;
    let _ = std::fs::create_dir_all(&dir);
    let _ = crate::storage::write_json_fast(&path, &counts);
}

/// Turn incremental edits into a full `todos` list so every downstream rule
/// (status validation, confidence history, gates, lane and plan sync) runs on
/// one path. `update` changes only the given fields of todos matched by `id`;
/// `add` appends new todos (or inserts each after its `after` id); `remove`
/// drops ids. Full replacements and incremental edits are mutually exclusive.
fn apply_todo_patch(mut input: Value, stored: impl FnOnce() -> Value) -> Result<Value> {
    flatten_phases(&mut input)?;
    let Some(obj) = input.as_object_mut() else {
        return Ok(input);
    };
    let patching = ["update", "add", "remove"]
        .iter()
        .any(|key| obj.get(*key).is_some_and(|value| !value.is_null()));
    if !patching {
        return Ok(input);
    }
    if obj.get("todos").is_some_and(|value| !value.is_null()) {
        bail!("send either a full `todos` list or `update`/`add`/`remove` edits, not both");
    }
    let parse_list = |value: Option<Value>| -> Result<Vec<Value>> {
        match value {
            None | Some(Value::Null) => Ok(Vec::new()),
            Some(Value::Array(items)) => Ok(items),
            Some(Value::String(raw)) if raw.trim().is_empty() => Ok(Vec::new()),
            Some(Value::String(raw)) => match serde_json::from_str::<Value>(raw.trim()) {
                Ok(Value::Array(items)) => Ok(items),
                Ok(single @ Value::Object(_)) => Ok(vec![single]),
                _ => bail!("expected a list"),
            },
            Some(single @ Value::Object(_)) => Ok(vec![single]),
            Some(other) => bail!("expected a list, got {other}"),
        }
    };
    let updates = parse_list(obj.remove("update"))?;
    let adds = parse_list(obj.remove("add"))?;
    let removes: Vec<String> = parse_list(obj.remove("remove"))?
        .into_iter()
        .map(|value| match value {
            Value::String(id) => Ok(id),
            _ => bail!("each remove entry must be an id string"),
        })
        .collect::<Result<_>>()?;

    let Value::Array(mut todos) = stored() else {
        bail!("stored todos are not a list");
    };
    let id_of = |todo: &Value| todo.get("id").and_then(Value::as_str).map(str::to_string);

    for update in updates {
        let Some(fields) = update.as_object() else {
            bail!("each update must be an object with an id");
        };
        let Some(id) = fields.get("id").and_then(Value::as_str) else {
            bail!("each update needs an `id`");
        };
        let Some(todo) = todos
            .iter_mut()
            .find(|todo| id_of(todo).as_deref() == Some(id))
        else {
            bail!("no todo with id {id:?}; use `add` for new todos");
        };
        let target = todo
            .as_object_mut()
            .ok_or_else(|| anyhow::anyhow!("stored todo {id:?} is not an object"))?;
        validate_membership(fields)?;
        for (key, value) in fields {
            if key != "id" {
                // `phase`/`lane` are the card's words for `stage`/`group`;
                // write the canonical key so an alias never sits beside it.
                let key = match key.as_str() {
                    "phase" | "wave" => "stage",
                    "lane" => "group",
                    other => other,
                };
                target.insert(key.to_string(), value.clone());
            }
        }
        if let Some(value) = fields.get("lane_after") {
            patch_lane_after(&mut todos, id, value.clone())?;
        }
    }

    for mut add in adds {
        let Some(fields) = add.as_object_mut() else {
            bail!("each added todo must be an object");
        };
        let after = fields
            .remove("after")
            .map(|value| {
                value
                    .as_str()
                    .filter(|id| !id.is_empty())
                    .map(str::to_string)
                    .ok_or_else(|| anyhow::anyhow!("after must be a nonempty todo id"))
            })
            .transpose()?;
        validate_membership(fields)?;
        let anchor = after
            .as_ref()
            .map(|id| {
                todos
                    .iter()
                    .find(|todo| id_of(todo).as_ref() == Some(id))
                    .ok_or_else(|| anyhow::anyhow!("unknown after todo {id:?}"))
            })
            .transpose()?;
        for (public, canonical) in [("phase", "stage"), ("lane", "group")] {
            if !fields.contains_key(public) && !fields.contains_key(canonical) {
                let value = anchor
                    .and_then(|todo| todo.get(canonical).or_else(|| todo.get(public)))
                    .filter(|value| !value.is_null())
                    .ok_or_else(|| {
                        anyhow::anyhow!(
                            "added todo needs {public}, or an after todo with membership"
                        )
                    })?;
                fields.insert(canonical.to_string(), value.clone());
            }
        }
        for (alias, canonical) in [("phase", "stage"), ("wave", "stage"), ("lane", "group")] {
            if let Some(value) = fields.remove(alias) {
                fields.insert(canonical.into(), value);
            }
        }
        if !fields.contains_key("lane_after") {
            if let Some(value) = todos
                .iter()
                .find(|todo| {
                    todo.get("stage") == fields.get("stage")
                        && todo.get("group") == fields.get("group")
                })
                .and_then(|todo| todo.get("lane_after"))
            {
                fields.insert("lane_after".into(), value.clone());
            }
        }
        let lane_after = fields.get("lane_after").cloned();
        let id = fields.get("id").and_then(Value::as_str).map(str::to_string);
        if let Some(id) = &id
            && todos.iter().any(|todo| id_of(todo).as_deref() == Some(id))
        {
            bail!("todo id {id:?} already exists; use `update` to change it");
        }
        match after.and_then(|after| {
            todos
                .iter()
                .position(|t| id_of(t).as_deref() == Some(&after))
        }) {
            Some(index) => todos.insert(index + 1, add),
            None => todos.push(add),
        }
        if let (Some(id), Some(value)) = (id, lane_after) {
            patch_lane_after(&mut todos, &id, value)?;
        }
    }

    if !removes.is_empty() {
        todos.retain(|todo| !id_of(todo).is_some_and(|id| removes.contains(&id)));
    }

    obj.insert("todos".to_string(), Value::Array(todos));
    Ok(input)
}

#[derive(Deserialize)]
struct TodoInput {
    todos: Option<Vec<TodoItem>>,
    goals: Option<Vec<TodoGoal>>,
    plan: Option<TodoPlan>,
}

fn parse_todo_input(input: Value) -> Result<TodoInput> {
    let mut input = input;
    flatten_phases(&mut input)?;
    let params: TodoInput = serde_json::from_value(normalize_todo_input(input))?;
    if let Some(todos) = &params.todos {
        let mut ids = std::collections::HashSet::new();
        for todo in todos {
            if !ids.insert(&todo.id) {
                bail!("duplicate todo id {:?}", todo.id);
            }
        }
    }
    if let Some(todo) = params.todos.as_ref().and_then(|todos| {
        todos
            .iter()
            .find(|todo| crate::todo::canonical_todo_status(&todo.status).is_none())
    }) {
        bail!(
            "invalid todo status {:?}; expected one of: pending, in_progress, completed, cancelled",
            todo.status
        );
    }
    Ok(params)
}

/// Validate aliases before serde can silently select or duplicate membership.
fn validate_membership(fields: &serde_json::Map<String, Value>) -> Result<()> {
    if ["phases", "lanes", "items"]
        .iter()
        .any(|key| fields.contains_key(*key))
    {
        bail!("incremental items must not contain hierarchy containers");
    }
    for keys in [
        ["phase", "stage", "wave"].as_slice(),
        ["lane", "group"].as_slice(),
    ] {
        let values: Vec<_> = keys.iter().filter_map(|key| fields.get(*key)).collect();
        if values.windows(2).any(|pair| pair[0] != pair[1]) {
            bail!("conflicting membership fields: {keys:?}");
        }
        for value in values {
            if keys[0] == "phase" {
                if !value
                    .as_u64()
                    .is_some_and(|phase| phase > 0 && phase <= u32::MAX as u64)
                {
                    bail!("phase must be a positive integer");
                }
            } else if !value.as_str().is_some_and(|lane| !lane.trim().is_empty()) {
                bail!("lane must be a nonempty string");
            }
        }
    }
    Ok(())
}

/// Only the public boundary is hierarchical. Storage and swarm retain stage/group.
fn flatten_phases(input: &mut Value) -> Result<()> {
    let Some(obj) = input.as_object_mut() else {
        return Ok(());
    };
    let Some(phases) = obj.get("phases") else {
        return Ok(());
    };
    if ["todos", "update", "add", "remove"]
        .iter()
        .any(|key| obj.get(*key).is_some_and(|v| !v.is_null()))
    {
        bail!("send phases or legacy todos or incremental edits, not both");
    }
    let phases = phases
        .as_array()
        .ok_or_else(|| anyhow::anyhow!("phases must be an array"))?;
    let mut phase_ids = std::collections::HashSet::new();
    let mut item_ids = std::collections::HashSet::new();
    let mut todos = Vec::new();
    for phase in phases {
        let phase = phase
            .as_object()
            .ok_or_else(|| anyhow::anyhow!("phase must be an object"))?;
        if phase
            .keys()
            .any(|key| !["phase", "lanes"].contains(&key.as_str()))
        {
            bail!("unexpected phase field");
        }
        let number = phase
            .get("phase")
            .and_then(Value::as_u64)
            .filter(|n| *n > 0 && *n <= u32::MAX as u64)
            .ok_or_else(|| anyhow::anyhow!("phase must be a positive integer"))?;
        if !phase_ids.insert(number) {
            bail!("duplicate phase {number}");
        }
        let lanes = phase
            .get("lanes")
            .and_then(Value::as_array)
            .filter(|v| !v.is_empty())
            .ok_or_else(|| anyhow::anyhow!("phase needs nonempty lanes"))?;
        let mut lane_ids = std::collections::HashSet::new();
        for lane in lanes {
            let lane = lane
                .as_object()
                .ok_or_else(|| anyhow::anyhow!("lane must be an object"))?;
            if lane
                .keys()
                .any(|key| !["lane", "items", "after"].contains(&key.as_str()))
            {
                bail!("unexpected lane field");
            }
            let name = lane
                .get("lane")
                .and_then(Value::as_str)
                .map(str::trim)
                .filter(|s| !s.is_empty())
                .ok_or_else(|| anyhow::anyhow!("lane must be a nonempty string"))?;
            if !lane_ids.insert(name) {
                bail!("duplicate lane {name:?}");
            }
            let items = lane
                .get("items")
                .and_then(Value::as_array)
                .filter(|v| !v.is_empty())
                .ok_or_else(|| anyhow::anyhow!("lane needs nonempty items"))?;
            for item in items {
                let mut item = item
                    .as_object()
                    .cloned()
                    .ok_or_else(|| anyhow::anyhow!("item must be an object"))?;
                if ["phase", "lane", "stage", "group", "wave", "lane_after"]
                    .iter()
                    .any(|key| item.contains_key(*key))
                {
                    bail!("hierarchical items must not own phase/lane/stage/group membership");
                }
                let id = item
                    .get("id")
                    .and_then(Value::as_str)
                    .ok_or_else(|| anyhow::anyhow!("item needs id"))?;
                if id.trim().is_empty() {
                    bail!("item needs a nonempty id");
                }
                if !item_ids.insert(id.to_string()) {
                    bail!("duplicate todo id {id:?}");
                }
                item.insert("stage".into(), json!(number));
                item.insert("group".into(), json!(name));
                if let Some(after) = lane.get("after") {
                    item.insert("lane_after".into(), after.clone());
                }
                todos.push(Value::Object(item));
            }
        }
    }
    obj.remove("phases");
    obj.insert("todos".into(), Value::Array(todos));
    Ok(())
}

/// Normalize a goal's group label: trimmed, with empty/whitespace collapsed
/// to `None` (the implicit goal of an ungrouped list).
fn goal_group_key(group: Option<&str>) -> Option<String> {
    group
        .map(str::trim)
        .filter(|group| !group.is_empty())
        .map(str::to_string)
}

fn todo_telemetry_update(
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
                .filter_map(|todo| todo.confidence.map(|state| state.legacy_score())),
        ),
        completion_confidence: crate::telemetry::TelemetryScoreSummary::from_scores(
            current_by_id
                .values()
                .filter_map(|todo| todo.completion_confidence.map(|state| state.legacy_score())),
        ),
        understands_user_intent: crate::telemetry::TelemetryScoreSummary::from_scores(
            plan.understands_user_intent
                .map(|state| state.legacy_score()),
        ),
        closed_feedback_loop: crate::telemetry::TelemetryScoreSummary::from_scores(
            goals
                .iter()
                .filter_map(|goal| goal.closed_feedback_loop.map(|state| state.legacy_score())),
        ),
        feedback_loop_relevance: crate::telemetry::TelemetryScoreSummary::from_scores(
            goals.iter().filter_map(|goal| {
                goal.feedback_loop_relevance
                    .map(|state| state.legacy_score())
            }),
        ),
        feedback_loop_coverage: crate::telemetry::TelemetryScoreSummary::from_scores(
            goals.iter().filter_map(|goal| {
                goal.feedback_loop_coverage
                    .map(|state| state.legacy_score())
            }),
        ),
        end_to_end_ownership: crate::telemetry::TelemetryScoreSummary::from_scores(
            goals
                .iter()
                .filter_map(|goal| goal.delivery_state.map(|state| state.legacy_score())),
        ),
    }
}

fn record_todo_telemetry(
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
fn record_score_observation<T: Copy + PartialEq>(history: &mut Vec<T>, value: Option<T>) {
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
        goal.feedback_loop_relevance_history = previous
            .map(|prev| prev.feedback_loop_relevance_history.clone())
            .unwrap_or_default();
        goal.feedback_loop_coverage_history = previous
            .map(|prev| prev.feedback_loop_coverage_history.clone())
            .unwrap_or_default();
        goal.feedback_loop_traceability_history = previous
            .map(|prev| prev.feedback_loop_traceability_history.clone())
            .unwrap_or_default();
        goal.delivery_state_history = previous
            .map(|prev| prev.delivery_state_history.clone())
            .unwrap_or_default();
        // Field-level merge, matching `merge_plan`: a write that revises one
        // assessment must not silently erase the others. Without this the
        // turn-end digest would read a stale `None` and re-raise a point the
        // agent had already resolved.
        if let Some(prev) = previous {
            if goal.closed_feedback_loop.is_none() {
                goal.closed_feedback_loop = prev.closed_feedback_loop;
            }
            if goal.delivery_state.is_none() {
                goal.delivery_state = prev.delivery_state;
            }
            if goal.feedback_loop_relevance.is_none() {
                goal.feedback_loop_relevance = prev.feedback_loop_relevance;
            }
            if goal.feedback_loop_coverage.is_none() {
                goal.feedback_loop_coverage = prev.feedback_loop_coverage;
            }
            if goal.feedback_loop_traceability.is_none() {
                goal.feedback_loop_traceability = prev.feedback_loop_traceability;
            }
            if goal.difficulty.is_none() {
                goal.difficulty = prev.difficulty;
            }
            if goal.autonomy.is_none() {
                goal.autonomy = prev.autonomy;
            }
            if goal.iteration_maturity.is_none() {
                goal.iteration_maturity = prev.iteration_maturity;
            }
            if goal.feedback_loop.is_none() {
                goal.feedback_loop = prev.feedback_loop.clone();
            }
            if goal.stopping_evidence.is_none() {
                goal.stopping_evidence = prev.stopping_evidence.clone();
            }
        }
        record_score_observation(
            &mut goal.closed_feedback_loop_history,
            goal.closed_feedback_loop,
        );
        record_score_observation(
            &mut goal.feedback_loop_relevance_history,
            goal.feedback_loop_relevance,
        );
        record_score_observation(
            &mut goal.feedback_loop_coverage_history,
            goal.feedback_loop_coverage,
        );
        record_score_observation(
            &mut goal.feedback_loop_traceability_history,
            goal.feedback_loop_traceability,
        );
        record_score_observation(&mut goal.delivery_state_history, goal.delivery_state);
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

/// Drop retained goals whose todo group no longer exists in `todos`.
///
/// `merge_goals` deliberately keeps goals a write does not mention, so a
/// goals-only or partial update cannot erase assessments. But when the agent
/// moves on to a new task and replaces the todo list wholesale, goals from the
/// finished task have no todos left to describe and were shown indefinitely in
/// the todos panel (issue #695). Goals whose group still has a todo, and the
/// ungrouped goal for a flat list, are always kept.
fn prune_orphaned_goals(goals: Vec<TodoGoal>, todos: &[TodoItem]) -> Vec<TodoGoal> {
    if todos.is_empty() {
        return goals;
    }
    let live_groups: std::collections::HashSet<Option<String>> = todos
        .iter()
        .map(|todo| goal_group_key(todo.group.as_deref()))
        .collect();
    goals
        .into_iter()
        .filter(|goal| live_groups.contains(&goal_group_key(goal.group.as_deref())))
        .collect()
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
    if before.and_then(|goal| goal.feedback_loop_relevance)
        != after.and_then(|goal| goal.feedback_loop_relevance)
    {
        fields.push(TodoGoalField::FeedbackLoopRelevance);
    }
    if before.and_then(|goal| goal.feedback_loop_coverage)
        != after.and_then(|goal| goal.feedback_loop_coverage)
    {
        fields.push(TodoGoalField::FeedbackLoopCoverage);
    }
    if before.and_then(|goal| goal.feedback_loop_traceability)
        != after.and_then(|goal| goal.feedback_loop_traceability)
    {
        fields.push(TodoGoalField::FeedbackLoopTraceability);
    }
    if before.and_then(|goal| goal.delivery_state) != after.and_then(|goal| goal.delivery_state) {
        fields.push(TodoGoalField::DeliveryState);
    }
    if before.and_then(|goal| goal.autonomy) != after.and_then(|goal| goal.autonomy) {
        fields.push(TodoGoalField::Autonomy);
    }
    if before.and_then(|goal| goal.iteration_maturity)
        != after.and_then(|goal| goal.iteration_maturity)
    {
        fields.push(TodoGoalField::IterationMaturity);
    }
    if before.and_then(|goal| goal.stopping_evidence.as_ref())
        != after.and_then(|goal| goal.stopping_evidence.as_ref())
    {
        fields.push(TodoGoalField::StoppingEvidence);
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
    if any_open && !intent_understanding_passes(plan.understands_user_intent) {
        observations.push(GateObservation {
            kind: GateObservationKind::IntentUnderstanding,
            group: None,
            state: plan
                .understands_user_intent
                .map(|state| state.as_str().to_string()),
        });
        // Only on the first observation of the plan, so a persistently low
        // assessment is reported once at turn end rather than on every write.
        let first_assessment = plan.understands_user_intent_history.len() <= 1;
        if first_assessment
            && plan
                .understands_user_intent
                .is_some_and(|state| state <= SEVERE_INTENT_MISUNDERSTANDING)
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
        if !feedback_loop_passes(goal.closed_feedback_loop) {
            observations.push(GateObservation {
                kind: GateObservationKind::ClosedFeedbackLoop,
                group: goal.group.clone(),
                state: goal
                    .closed_feedback_loop
                    .map(|state| state.as_str().to_string()),
            });
        }
        if !crate::todo::feedback_loop_relevance_passes(goal) {
            observations.push(GateObservation {
                kind: GateObservationKind::FeedbackLoopRelevance,
                group: goal.group.clone(),
                state: goal
                    .feedback_loop_relevance
                    .map(|state| state.as_str().to_string()),
            });
        }
        if !crate::todo::feedback_loop_coverage_passes(goal) {
            observations.push(GateObservation {
                kind: GateObservationKind::FeedbackLoopCoverage,
                group: goal.group.clone(),
                state: goal
                    .feedback_loop_coverage
                    .map(|state| state.as_str().to_string()),
            });
        }
        if !crate::todo::feedback_loop_traceability_passes(goal) {
            observations.push(GateObservation {
                kind: GateObservationKind::FeedbackLoopTraceability,
                group: goal.group.clone(),
                state: goal
                    .feedback_loop_traceability
                    .map(|state| state.as_str().to_string()),
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
                    coerce_empty_string_to_null(value);
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
                if key == "todos"
                    && let Some(Value::String(status)) = fields.get_mut("status")
                    && let Some(canonical) = crate::todo::canonical_todo_status(status)
                {
                    *status = canonical.to_string();
                }
                for key in [
                    "confidence",
                    "completion_confidence",
                    "alignment_score",
                    "user_intention_alignment",
                    "closed_feedback_loop",
                    // Pre-rename aliases; some prompts and replayed transcripts
                    // still carry the old keys.
                    "hill_climbability",
                    "end_to_end_ownership",
                    "delivery_state",
                    "feedback_loop_relevance",
                    "feedback_loop_coverage",
                    "feedback_loop_traceability",
                    "difficulty",
                    "autonomy",
                ] {
                    if let Some(value) = fields.get_mut(key) {
                        coerce_empty_string_to_null(value);
                    }
                }
            }
        }
    }
    input
}

/// Coerce an empty or whitespace-only string to `null` so an omitted-but-sent
/// assessment reads as absent. Numeric legacy scores (ints, floats, numeric
/// strings) are handled by the semantic-state deserializers themselves, so no
/// numeric coercion is needed here anymore.
fn coerce_empty_string_to_null(value: &mut Value) {
    if let Value::String(raw) = value
        && raw.trim().is_empty()
    {
        *value = Value::Null;
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
        "Read or update todos and goal assessments. For multi-step work, write the plan \
         once with `phases` containing lanes containing items. Then use `update` by id for status/field changes and `add` \
         for new work. Reserve full replacement for restructuring; remove rarely. \
         Mark work in_progress before starting and completed as soon as it is done. \
         Keep the live list current after meaningful steps. Use short titles, with \
         longer context in details. Organize parallel lanes within sequential phases. \
         A lane may set `after` to prerequisite todo IDs in earlier phases, replacing \
         the whole-phase barrier so it can start once all those items are completed. \
         Omit `after` for normal phase ordering. Incremental `lane_after` edits on one \
         item update its entire lane; null restores normal ordering. \
         Set waiting_on to existing background task IDs or swarm await_members dependency IDs from this session when work \
         must wait. They block only while running, never after completion. Saving \
         enables completion notify/wake. Use [] or null to clear waiting_on. \
         Block only when progress requires an indispensable action that only someone else can perform, \
         and no safe workaround exists after investigation. The reason must name who must do exactly \
         what to unblock this item, and why the agent cannot do it or work around it. Valid examples: \
         a credential only its owner can provide, required data only its custodian can supply, or \
         explicit approval genuinely required for an irreversible action. Implementation choices, \
         uncertainty, task size, prioritization, and agent coordination never qualify. Decide, \
         investigate, split the work, or coordinate directly instead. Do not invent approval \
         requirements for already-authorized work. Continue independent items. \
         For an external blocker without a dependency ID, set external_blocker to \
         {reason, review_at}, with a concrete reason and future RFC3339 deadline. \
         Keep the todo pending/in_progress. Pokes pause until review_at and a durable \
         scheduler wakes this session to reassess, not assume resolution or completion. \
         Clear external_blocker with null when unblocked. Use this instead of a separate \
         schedule call for todo blocker reviews. completely_blocked: No way forward."
    }

    fn parameters_schema(&self) -> Value {
        let mut schema = json!({
            "type": "object",
            "properties": {
                "intent": super::intent_schema_property(),
                "update": {
                    "type": "array",
                    "description": "Patch todos by id and changed fields only. Unmentioned data stays untouched. Prefer for status updates.",
                    "items": {
                        "type": "object",
                        "required": ["id"],
                        "properties": {"id": {"type": "string"}},
                        "additionalProperties": true
                    }
                },
                "add": {
                    "type": "array",
                    "description": "Append new todos (same fields as todos). Optional `after`: id to insert after.",
                    "items": {
                        "type": "object",
                        "required": ["content", "status", "priority", "id", "confidence"],
                        "additionalProperties": true
                    }
                },
                "remove": {
                    "type": "array",
                    "description": "Ids of todos to delete. Prefer marking status cancelled.",
                    "items": {"type": "string"}
                },
                "todos": {
                    "type": "array",
                    "description": "Full todo list; replaces the whole list. Use for the initial plan or a real restructure, otherwise prefer update/add/remove.",
                    "items": {
                        "type": "object",
                        "required": ["content", "status", "priority", "id", "confidence"],
                        "properties": {
                            "content": {
                                "type": "string",
                                "description": "Short title, ideally under 40 characters; todos render side by side. Put longer context in details."
                            },
                            "details": {
                                "type": "string",
                                "description": "Longer brief for the assigned worker: files, acceptance criteria, context. Hidden from the card."
                            },
                            "waiting_on": {
                                "type": ["array", "null"],
                                "items": {"type": "string"},
                                "description": "Session task/await_members IDs: pending blocks+notify/wake; done/timeout unblocks. []/null clears."
                            },
                            "completely_blocked": {
                                "type": ["object", "null"],
                                "required": ["reason"],
                                "additionalProperties": false,
                                "properties": {
                                    "reason": {"type": "string", "minLength": 1, "description": "Who must do exactly what, and why the agent cannot do it or work around it."}
                                },
                                "description": "Only when an indispensable action can be performed only by someone else and no safe workaround exists after investigation. Name who must do exactly what, and why the agent cannot do it or work around it. Not for implementation decisions, uncertainty, task size, or agent coordination."
                            },
                            "external_blocker": {
                                "type": ["object", "null"],
                                "required": ["reason", "review_at"],
                                "additionalProperties": false,
                                "properties": {
                                    "reason": {"type": "string", "minLength": 1, "description": "Who must do exactly what, and why the agent cannot do it or work around it."},
                                    "review_at": {"type": "string", "format": "date-time", "description": "Future finite RFC3339 reassessment deadline. Does not mean the blocker resolves then."}
                                },
                                "description": "Only when an indispensable action can be performed only by someone else and no safe workaround exists after investigation. Name who must do exactly what, and why the agent cannot do it or work around it. Not for implementation decisions, uncertainty, task size, or agent coordination. No dependency ID: pause until durable review_at to reassess. null cancels."
                            },
                            "status": {
                                "type": "string",
                                "enum": ["pending", "in_progress", "completed", "cancelled"],
                                "description": "Status. Use completed when the task is done."
                            },
                            "priority": {
                                "type": "string",
                                "description": "Priority."
                            },
                            "id": {
                                "type": "string",
                                "description": "ID."
                            },
                            "lane": {
                                "type": "string",
                                "description": "Lane name: coherent goal owned by one worker, tasks ordered within it. Omit for flat list."
                            },
                            "phase": {
                                "type": "integer",
                                "minimum": 1,
                                "description": "Sequential phase (1, 2...). Lanes inside each phase run in parallel. Omit for list order."
                            },
                            "confidence": {
                                "type": "string",
                                "enum": ["speculative", "plausible", "validated", "verified"],
                                "description": "Evidence state that this todo can be completed correctly; reassess as evidence accumulates."
                            },
                            "completion_confidence": {
                                "type": "string",
                                "enum": ["speculative", "plausible", "validated", "verified"],
                                "description": "Evidence state behind this todo's completion. Use only for completed items."
                            },
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
                            "type": "string",
                            "enum": ["uncertain", "partial", "clear", "complete"],
                            "description": "How well you understand what the user wants. Report uncertain or partial when guessing."
                        }
                    }
                },
                "goals": {
                    "type": "array",
                    "description": "Goal-level assessments, one per todo group (null = ungrouped). Omitted groups are retained.",
                    "items": {
                        "type": "object",
                        "required": ["closed_feedback_loop", "feedback_loop", "feedback_loop_relevance", "feedback_loop_coverage", "feedback_loop_traceability"],
                        "properties": {
                            "group": {
                                "type": "string",
                                "description": "Group label this goal describes. Omit or null for the ungrouped list."
                            },
                            "closed_feedback_loop": {
                                "type": "string",
                                "enum": ["absent", "weak", "usable", "strong", "closed"],
                                "description": "How much of this goal's correctness the feedback_loop can verify on its own."
                            },
                            "feedback_loop": {
                                "type": "string",
                                "description": "Requirement-to-check process: an explicit observation or check for each requirement of this goal."
                            },
                            "feedback_loop_relevance": {
                                "type": "string",
                                "enum": ["indirect", "synthetic", "representative", "acceptance_blocked", "acceptance_aligned"],
                                "description": "How directly checks represent observable acceptance behavior. indirect = inspection or an internal proxy; synthetic = custom harnesses, stubs, mocks, copied sources, or synthetic fixtures; representative = real public interfaces but not the complete acceptance workflow; acceptance_blocked = the real acceptance workflow was attempted but an external constraint prevented a result; acceptance_aligned = the real project build, integration test, or end-user workflow passed. Substitute-only validation is never acceptance_aligned."
                            },
                            "feedback_loop_coverage": {
                                "type": "string",
                                "enum": ["narrow", "main_paths", "edge_and_integration_paths"],
                                "description": "Coverage of integration boundaries, edge cases, packaging, and likely failure modes."
                            },
                            "feedback_loop_traceability": {
                                "type": "string",
                                "enum": ["unmapped", "partial", "complete"],
                                "description": "How completely each requirement and changed output maps to a concrete check and observed result."
                            },
                            "delivery_state": {
                                "type": "string",
                                "enum": ["change_made", "integrated", "workflow_validated", "outcome_delivered"],
                                "description": "Completion-time: how far the result actually traveled toward the user's outcome."
                            },
                            "difficulty": {
                                "type": "string",
                                "enum": ["trivial", "routine", "involved", "complex", "hard", "expert", "research", "open_ended"],
                                "description": "Honest intrinsic difficulty of this goal. Descriptive only."
                            },
                            "autonomy": {
                                "type": "string",
                                "enum": ["requested_only", "necessary_followthrough", "proactive", "stewardship"],
                                "description": "How far beyond the literal request the work went. Assess from what was completed."
                            },
                            "iteration_maturity": {
                                "type": "string",
                                "enum": ["not_started", "exploring", "improving", "plateau_unproven", "outcome_reached", "constraints_exhausted", "plateau_confirmed", "budget_exhausted"],
                                "description": "How far the feedback loop was actually exercised, and any evidence-based reason to stop iterating."
                            },
                            "stopping_evidence": {
                                "type": "string",
                                "description": "Evidence for the reported iteration_maturity: attempts, observations, or a real budget limit."
                            }
                        }
                    }
                }
            }
        });
        // Patch additions accept the same fields as full todos, plus an
        // insertion position. Keep their declarations in sync for providers
        // that require every `required` entry to appear in `properties`.
        schema["properties"]["todos"]["items"]["properties"]["lane_after"] = json!({
            "type": ["array", "null"], "minItems": 1, "uniqueItems": true,
            "items": {"type": "string", "minLength": 1},
            "description": "Lane-wide prerequisite todo IDs in strictly earlier phases by first appearance. Editing one member updates the entire lane. Null restores the default phase barrier."
        });
        let mut add_properties = schema["properties"]["todos"]["items"]["properties"].clone();
        add_properties["after"] = json!({
            "type": "string",
            "description": "Existing todo id to insert after."
        });
        schema["properties"]["update"]["items"]["properties"] =
            schema["properties"]["todos"]["items"]["properties"].clone();
        schema["properties"]["update"]["items"]["additionalProperties"] = json!(false);
        schema["properties"]["add"]["items"]["properties"] = add_properties;
        schema["properties"]["add"]["items"]["additionalProperties"] = json!(false);
        schema["properties"]["update"]["items"]["properties"]["waiting_on"] =
            schema["properties"]["todos"]["items"]["properties"]["waiting_on"].clone();
        schema["properties"]["update"]["items"]["properties"]["external_blocker"] =
            schema["properties"]["todos"]["items"]["properties"]["external_blocker"].clone();
        schema["properties"]["update"]["items"]["properties"]["completely_blocked"] =
            schema["properties"]["todos"]["items"]["properties"]["completely_blocked"].clone();
        // The item schema is built locally above. Remove flat aliases from the
        // hierarchical representation without assuming the JSON object shape.
        let mut item = schema["properties"]["todos"]["items"].clone();
        if let Some(properties) = schema["properties"].as_object_mut() {
            properties.remove("todos");
        }
        if let Some(properties) = item["properties"].as_object_mut() {
            properties.remove("phase");
            properties.remove("lane");
            properties.remove("lane_after");
        }
        item["additionalProperties"] = json!(false);
        schema["properties"]["phases"] = json!({
            "type": "array",
            "description": "Replace plan: sequential phases, parallel lanes, ordered items. A lane's optional after todo IDs replace its phase barrier for early start. [] clears.",
            "items": {"type": "object", "required": ["phase", "lanes"], "additionalProperties": false,
                "properties": {"phase": {"type": "integer", "minimum": 1}, "lanes": {
                    "type": "array", "minItems": 1, "items": {"type": "object", "required": ["lane", "items"], "additionalProperties": false,
                        "properties": {"lane": {"type": "string", "minLength": 1}, "after": {
                            "type": ["array", "null"], "minItems": 1, "uniqueItems": true,
                            "items": {"type": "string", "minLength": 1},
                            "description": "Prerequisite todo IDs from strictly earlier phases. Omit or null for the default phase barrier."
                        }, "items": {"type": "array", "minItems": 1, "items": item}}
                    }
                }}}
        });
        schema["properties"]["add"]["description"] = json!(
            "Add items with explicit phase and lane, or inherit missing membership from an existing after id."
        );
        schema
    }

    async fn execute(&self, input: Value, ctx: ToolContext) -> Result<ToolOutput> {
        let write_mode_input = input.clone();
        let input = apply_todo_patch(input, || {
            serde_json::to_value(load_todos(&ctx.session_id).unwrap_or_default())
                .unwrap_or(Value::Array(Vec::new()))
        })?;
        let params = parse_todo_input(input)?;
        let is_write = params.todos.is_some() || params.goals.is_some() || params.plan.is_some();
        let operation = if is_write { "write" } else { "read" };
        let result = if is_write {
            // Goals/plan-only writes keep the stored todo list.
            let previous = load_todos(&ctx.session_id).unwrap_or_default();
            let mut todos = params.todos.unwrap_or_else(|| previous.clone());
            crate::todo::store::validate_session_lane_dependencies(
                &crate::storage::jcode_dir()?,
                &ctx.session_id,
                &todos,
            )
            .map_err(anyhow::Error::msg)?;
            for todo in &mut todos {
                if !crate::todo::is_incomplete_todo(todo) {
                    todo.external_blocker = None;
                    todo.completely_blocked = None;
                } else if todo.completely_blocked.is_some() {
                    // Complete blockers have no timed review. Existing task
                    // and todo dependencies remain useful once unblocked.
                    todo.external_blocker = None;
                }
            }
            validate_external_blockers(&previous, &todos)?;
            prepare_background_dependencies(
                crate::background::global(),
                &ctx.session_id,
                &previous,
                &todos,
            )
            .await?;
            record_todo_write_mode(&write_mode_input);
            merge_confidence_history(&previous, &mut todos);
            (|| {
                let stored_goals = load_goals(&ctx.session_id).unwrap_or_default();
                let stored_plan = load_plan(&ctx.session_id).unwrap_or_default();
                let goals = prune_orphaned_goals(merge_goals(&stored_goals, params.goals), &todos);
                let plan = merge_plan(&stored_plan, params.plan);
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
                        GateObservationKind::FeedbackLoopRelevance => {
                            crate::telemetry::TodoGateKind::FeedbackLoopRelevance
                        }
                        GateObservationKind::FeedbackLoopCoverage => {
                            crate::telemetry::TodoGateKind::FeedbackLoopCoverage
                        }
                        GateObservationKind::FeedbackLoopTraceability => {
                            crate::telemetry::TodoGateKind::FeedbackLoopTraceability
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
                if previous
                    .iter()
                    .chain(&todos)
                    .any(|todo| todo.external_blocker.is_some())
                {
                    if let Err(error) =
                        crate::ambient::reconcile_stored_todo_reviews(&ctx.session_id)
                    {
                        // A failed durable wake must not leave silent suppression.
                        // Keep unrelated/newer todo fields, clearing only the exact
                        // blocker generations this write attempted to schedule.
                        let mut current = load_todos(&ctx.session_id)?;
                        for todo in &mut current {
                            if todo.external_blocker.is_some()
                                && todos.iter().any(|attempted| {
                                    attempted.id == todo.id
                                        && attempted.external_blocker == todo.external_blocker
                                })
                            {
                                todo.external_blocker = None;
                            }
                        }
                        save_todos(&ctx.session_id, &current)?;
                        bail!(
                            "Could not persist durable todo reviews; matching external blockers were cleared so work remains actionable: {error:#}"
                        );
                    }
                    super::ambient::nudge_schedule_runner();
                }
                save_goals(&ctx.session_id, &goals)?;
                save_plan(&ctx.session_id, &plan)?;
                if let Err(err) = update_todo_review_cycle(&ctx.session_id, &previous, &todos) {
                    crate::logging::warn(&format!(
                        "[tool:todo] failed to update review cycle session_id={} error={}",
                        ctx.session_id, err
                    ));
                }
                record_todo_telemetry(&previous, &todos, &goals, &plan);

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
            record_todo_write_mode(&write_mode_input);
            (|| {
                let todos = load_todos(&ctx.session_id)?;
                let goals = load_goals(&ctx.session_id).unwrap_or_default();
                let plan = load_plan(&ctx.session_id).unwrap_or_default();
                record_todo_telemetry(&todos, &todos, &goals, &plan);
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
#[path = "todo_tests.rs"]
mod tests;
