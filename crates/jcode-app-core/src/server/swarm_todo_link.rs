//! Link the coordinator's session todos to the swarm plan.
//!
//! The todo list is the single source of truth for what the coordinator is
//! doing. Two directions:
//!
//! * Todos -> plan: [`plan_items_from_todos`] turns each lane of open todos
//!   into one plan item (`lane:<stage>:<group>`), so one agent owns a lane.
//!   Stages become dependencies, so the model never writes `blocked_by` ids.
//! * Assignment: [`link_assignment`] tags the lane's todos with the worker and
//!   seeds the worker's own todo list with the lane. From then on every save
//!   on either side mirrors into the other (see `todo::link_worker_lane`).
//! * Plan -> todos: [`sync_plan`] completes a lane's todos when its plan item
//!   completes.
//!
//! Hand-written plan items (not `lane:` ids) still match todos by id, then by
//! trimmed case-insensitive content.

use crate::plan::PlanItem;
use crate::todo::TodoItem;

fn is_open(todo: &TodoItem) -> bool {
    !matches!(todo.status.as_str(), "completed" | "cancelled")
}

use crate::todo::stages::{LANE_ID_PREFIX, lane_id};

fn is_lane_task(task_id: &str) -> bool {
    task_id.starts_with(LANE_ID_PREFIX)
}

/// Convert open todos into swarm plan items: one item per lane.
///
/// Layout comes from [`crate::todo::stages::stage_layout`]: stages run in
/// order, and inside a stage each group is a lane. A whole lane is one plan
/// item, so one agent owns it end to end and keeps its context; that agent's
/// own todo list is the lane. Every lane waits for every lane of the previous
/// stage. The model never writes ids (no cycles, no dangling references).
/// Completed and cancelled todos are skipped; a lane with nothing open is
/// skipped and never blocks.
pub(crate) fn plan_items_from_todos(todos: &[TodoItem]) -> Vec<PlanItem> {
    let open: Vec<&TodoItem> = todos.iter().filter(|todo| is_open(todo)).collect();
    let mut items = Vec::new();
    let mut previous_stage: Vec<String> = Vec::new();
    for stage in crate::todo::stages::stage_layout(&open) {
        let mut this_stage = Vec::new();
        for lane in &stage.lanes {
            let lane_todos: Vec<&TodoItem> = lane.items.iter().map(|&i| open[i]).collect();
            let first = lane_todos[0];
            let id = lane_id(first);
            let content = match &lane.group {
                Some(group) => format!(
                    "{group}: {}",
                    lane_todos
                        .iter()
                        .map(|todo| todo.content.as_str())
                        .collect::<Vec<_>>()
                        .join(", ")
                ),
                None => first.content.clone(),
            };
            items.push(PlanItem {
                content,
                status: "queued".to_string(),
                priority: first.priority.clone(),
                id: id.clone(),
                subsystem: lane.group.clone(),
                file_scope: Vec::new(),
                blocked_by: previous_stage.clone(),
                assigned_to: None,
            });
            this_stage.push(id);
        }
        previous_stage = this_stage;
    }
    items
}

/// The coordinator's open todos that make up plan lane `task_id`, in order.
fn lane_todos_mut<'a>(todos: &'a mut [TodoItem], task_id: &str) -> Vec<&'a mut TodoItem> {
    todos
        .iter_mut()
        .filter(|todo| is_open(todo) && lane_id(todo) == task_id)
        .collect()
}

fn normalize(text: &str) -> String {
    text.split_whitespace()
        .collect::<Vec<_>>()
        .join(" ")
        .to_lowercase()
}

fn find_todo<'a>(
    todos: &'a mut [TodoItem],
    task_id: &str,
    content: &str,
) -> Option<&'a mut TodoItem> {
    // Content must always match: ids alone are not trustworthy because hand
    // written plans and todo lists both tend to use "1", "2", ... and an id
    // collision would silently complete an unrelated todo. The id only breaks
    // ties between todos with identical text.
    let wanted = normalize(content);
    if wanted.is_empty() {
        return None;
    }
    let index = todos
        .iter()
        .position(|todo| todo.id == task_id && normalize(&todo.content) == wanted)
        .or_else(|| {
            todos
                .iter()
                .position(|todo| normalize(&todo.content) == wanted)
        })?;
    todos.get_mut(index)
}

/// Pure update used by [`link_assignment`]. Returns true when `todos` changed.
pub(crate) fn apply_assignment(
    todos: &mut [TodoItem],
    task_id: &str,
    content: &str,
    worker_session: &str,
) -> bool {
    if is_lane_task(task_id) {
        // The worker owns the whole lane; its own todo list drives each
        // item's status, so only the owner is recorded here.
        let mut changed = false;
        for todo in lane_todos_mut(todos, task_id) {
            if todo.assigned_to.as_deref() != Some(worker_session) {
                todo.assigned_to = Some(worker_session.to_string());
                changed = true;
            }
        }
        return changed;
    }
    let Some(todo) = find_todo(todos, task_id, content) else {
        return false;
    };
    if !is_open(todo) {
        return false;
    }
    let mut changed = false;
    if todo.assigned_to.as_deref() != Some(worker_session) {
        todo.assigned_to = Some(worker_session.to_string());
        changed = true;
    }
    if todo.status == "pending" {
        todo.status = "in_progress".to_string();
        changed = true;
    }
    changed
}

/// Pure update used by [`sync_task_status`]. Only a successful plan
/// completion completes the todo; failures keep it open (the failed worker
/// stays visible on the row via its ✗ mark) so the coordinator decides.
pub(crate) fn apply_task_status(
    todos: &mut [TodoItem],
    task_id: &str,
    content: &str,
    plan_status: &str,
) -> bool {
    if is_lane_task(task_id) {
        if !crate::plan::is_completed_status(plan_status) {
            return false;
        }
        let mut changed = false;
        for todo in lane_todos_mut(todos, task_id) {
            todo.status = "completed".to_string();
            changed = true;
        }
        return changed;
    }
    let Some(todo) = find_todo(todos, task_id, content) else {
        return false;
    };
    if crate::plan::is_completed_status(plan_status) && todo.status != "completed" {
        todo.status = "completed".to_string();
        return true;
    }
    false
}

/// Extra context for a worker's assignment message.
///
/// For a lane, the worker's own todo list has just been set to the lane, so
/// the brief lists those todos with their details and the editing rules. For
/// a hand-written plan item, it is that todo's `details`.
pub(crate) fn assignment_brief(
    coordinator_session: &str,
    task_id: &str,
    content: &str,
) -> Option<String> {
    let mut todos = crate::todo::load_todos(coordinator_session).ok()?;
    if !is_lane_task(task_id) {
        return details_for(&mut todos, task_id, content);
    }
    let lane = crate::todo::stages::lane_todos_for_worker(&todos, task_id);
    if lane.is_empty() {
        return None;
    }
    Some(lane_brief(&lane))
}

fn lane_brief(lane: &[TodoItem]) -> String {
    let mut brief = String::from(
        "Your todo list is this lane, in order. It stays in sync with the coordinator: \
         update it with the todo tool as you work (mark items in progress and done, add \
         details, split or add steps). Keep to this lane; message the coordinator for \
         anything outside it.\n",
    );
    for todo in lane {
        brief.push_str(&format!("- [{}] {}", todo.status, todo.content));
        if let Some(details) = todo
            .details
            .as_deref()
            .map(str::trim)
            .filter(|d| !d.is_empty())
        {
            brief.push_str(&format!(": {details}"));
        }
        brief.push('\n');
    }
    brief.trim_end().to_string()
}

fn details_for(todos: &mut [TodoItem], task_id: &str, content: &str) -> Option<String> {
    find_todo(todos, task_id, content)?
        .details
        .as_deref()
        .map(str::trim)
        .filter(|details| !details.is_empty())
        .map(str::to_string)
}

fn update_todos(session_id: &str, update: impl FnOnce(&mut Vec<TodoItem>) -> bool) {
    let Ok(mut todos) = crate::todo::load_todos(session_id) else {
        return;
    };
    if todos.is_empty() || !update(&mut todos) {
        return;
    }
    if let Err(error) = crate::todo::save_todos(session_id, &todos) {
        crate::logging::warn(&format!(
            "swarm todo link: failed to save todos for {session_id}: {error}"
        ));
    }
}

/// Record that `worker_session` took plan task `task_id` on the coordinator's
/// matching todo. Silently does nothing when no todo matches.
pub(crate) fn link_assignment(
    coordinator_session: &str,
    task_id: &str,
    content: &str,
    worker_session: &str,
) {
    update_todos(coordinator_session, |todos| {
        apply_assignment(todos, task_id, content, worker_session)
    });
    if is_lane_task(task_id)
        && let Err(error) =
            crate::todo::link_worker_lane(coordinator_session, worker_session, task_id)
    {
        crate::logging::warn(&format!(
            "swarm todo link: failed to seed {worker_session} with {task_id}: {error}"
        ));
    }
}

/// Pure whole-plan reconcile: complete todos whose plan item is done and
/// record any worker assignment that has not landed yet. Idempotent.
pub(crate) fn apply_plan(todos: &mut [TodoItem], items: &[PlanItem]) -> bool {
    let mut changed = false;
    for item in items {
        if let Some(worker) = item.assigned_to.as_deref() {
            let already = if is_lane_task(&item.id) {
                lane_todos_mut(todos, &item.id)
                    .iter()
                    .all(|todo| todo.assigned_to.as_deref() == Some(worker))
            } else {
                find_todo(todos, &item.id, &item.content)
                    .is_some_and(|todo| todo.assigned_to.is_some())
            };
            if !already {
                changed |= apply_assignment(todos, &item.id, &item.content, worker);
            }
        }
        changed |= apply_task_status(todos, &item.id, &item.content, &item.status);
    }
    changed
}

/// Reconcile the coordinator's todos with the current plan. Called after
/// every plan broadcast so every completion path (turn end, `report`,
/// manual status changes) reaches the todo list without per-path hooks.
pub(crate) fn sync_plan(coordinator_session: &str, items: &[PlanItem]) {
    if items.is_empty() {
        return;
    }
    update_todos(coordinator_session, |todos| apply_plan(todos, items));
}

#[cfg(test)]
mod tests {
    use super::*;

    fn todo(id: &str, status: &str, stage: Option<u32>) -> TodoItem {
        TodoItem {
            id: id.to_string(),
            content: format!("task {id}"),
            status: status.to_string(),
            priority: "high".to_string(),
            stage,
            ..Default::default()
        }
    }

    fn laned(id: &str, stage: u32, group: &str) -> TodoItem {
        let mut item = todo(id, "pending", Some(stage));
        item.group = Some(group.to_string());
        item
    }

    fn deps(items: &[PlanItem]) -> Vec<(String, Vec<String>)> {
        items
            .iter()
            .map(|item| (item.id.clone(), item.blocked_by.clone()))
            .collect()
    }

    #[test]
    fn sequential_todos_chain() {
        let items =
            plan_items_from_todos(&[todo("a", "pending", None), todo("b", "pending", None)]);
        assert_eq!(
            deps(&items),
            vec![
                ("lane:todo:a".into(), vec![]),
                ("lane:todo:b".into(), vec!["lane:todo:a".to_string()])
            ]
        );
    }

    #[test]
    fn each_lane_is_one_item_and_stages_chain() {
        let items = plan_items_from_todos(&[
            laned("p1", 1, "parser"),
            laned("u1", 1, "ui"),
            laned("p2", 1, "parser"),
            laned("i1", 2, "integration"),
            laned("i2", 2, "integration"),
            laned("r1", 2, "release"),
        ]);
        let stage_one = vec!["lane:1:parser".to_string(), "lane:1:ui".to_string()];
        assert_eq!(
            deps(&items),
            vec![
                ("lane:1:parser".into(), vec![]),
                ("lane:1:ui".into(), vec![]),
                ("lane:2:integration".into(), stage_one.clone()),
                ("lane:2:release".into(), stage_one),
            ]
        );
        // One agent gets the whole lane, so the item names every step.
        assert_eq!(items[0].content, "parser: task p1, task p2");
        assert_eq!(items[0].subsystem.as_deref(), Some("parser"));
    }

    #[test]
    fn stages_order_and_completed_todos_are_skipped() {
        let items = plan_items_from_todos(&[
            todo("done", "completed", Some(1)),
            todo("a", "pending", Some(1)),
            todo("b", "pending", Some(2)),
            todo("c", "in_progress", Some(2)),
            todo("d", "pending", Some(3)),
            todo("x", "cancelled", Some(3)),
        ]);
        let lane = |id: &str| format!("lane:todo:{id}");
        assert_eq!(
            deps(&items),
            vec![
                (lane("a"), vec![]),
                (lane("b"), vec![lane("a")]),
                (lane("c"), vec![lane("a")]),
                (lane("d"), vec![lane("b"), lane("c")]),
            ]
        );
        assert!(items.iter().all(|item| item.status == "queued"));
    }

    #[test]
    fn details_reach_the_matching_todo_only() {
        let mut todos = vec![todo("a", "pending", None), todo("b", "pending", None)];
        todos[0].details = Some("  Edit src/x.rs; keep API stable.  ".into());
        assert_eq!(
            details_for(&mut todos, "a", "task a").as_deref(),
            Some("Edit src/x.rs; keep API stable.")
        );
        assert_eq!(details_for(&mut todos, "b", "task b"), None);
        todos[1].details = Some("   ".into());
        assert_eq!(details_for(&mut todos, "b", "task b"), None);
    }

    #[test]
    fn assignment_matches_by_id_then_content() {
        let mut todos = vec![todo("a", "pending", None), todo("b", "pending", None)];
        assert!(apply_assignment(&mut todos, "a", "task a", "session_fox_1"));
        assert_eq!(todos[0].assigned_to.as_deref(), Some("session_fox_1"));
        assert_eq!(todos[0].status, "in_progress");

        assert!(apply_assignment(
            &mut todos,
            "plan-7",
            "  Task   B ",
            "session_ox_2"
        ));
        assert_eq!(todos[1].assigned_to.as_deref(), Some("session_ox_2"));
        assert!(!apply_assignment(
            &mut todos,
            "zzz",
            "nothing",
            "session_ox_2"
        ));
    }

    #[test]
    fn only_success_completes_the_todo() {
        let mut todos = vec![todo("a", "in_progress", None)];
        assert!(!apply_task_status(&mut todos, "a", "task a", "failed"));
        assert_eq!(todos[0].status, "in_progress");
        assert!(apply_task_status(&mut todos, "a", "task a", "done"));
        assert_eq!(todos[0].status, "completed");
    }

    #[test]
    fn plan_reconcile_links_lanes_and_completes_idempotently() {
        let mut todos = vec![
            laned("p1", 1, "parser"),
            laned("p2", 1, "parser"),
            laned("u1", 1, "ui"),
        ];
        let mut items = plan_items_from_todos(&todos);
        items[0].assigned_to = Some("session_fox_1".into());
        items[0].status = "done".into();
        items[1].assigned_to = Some("session_ox_2".into());
        items[1].status = "running".into();
        assert!(apply_plan(&mut todos, &items));
        // A finished lane completes every todo in it, owned by one worker.
        assert_eq!(todos[0].status, "completed");
        assert_eq!(todos[1].status, "completed");
        assert_eq!(todos[0].assigned_to.as_deref(), Some("session_fox_1"));
        assert_eq!(todos[1].assigned_to.as_deref(), Some("session_fox_1"));
        // A running lane's status is driven by the worker's own todo list,
        // so assignment only records the owner.
        assert_eq!(todos[2].status, "pending");
        assert_eq!(todos[2].assigned_to.as_deref(), Some("session_ox_2"));
        assert!(!apply_plan(&mut todos, &items), "second pass is a no-op");
    }

    #[test]
    fn lane_brief_lists_steps_with_details_and_rules() {
        let mut todos = vec![laned("p1", 1, "parser"), laned("p2", 1, "parser")];
        todos[1].details = Some("touch src/lexer.rs".into());
        let lane = crate::todo::stages::lane_todos_for_worker(&todos, "lane:1:parser");
        let brief = lane_brief(&lane);
        assert!(brief.starts_with("Your todo list is this lane"), "{brief}");
        assert!(brief.contains("- [pending] task p1\n"), "{brief}");
        assert!(
            brief.ends_with("- [pending] task p2: touch src/lexer.rs"),
            "{brief}"
        );
    }

    #[test]
    fn colliding_ids_never_touch_an_unrelated_todo() {
        // A hand-proposed plan and the todo list both number from "1".
        let mut todos = vec![TodoItem {
            id: "1".into(),
            content: "write the release notes".into(),
            status: "pending".into(),
            ..Default::default()
        }];
        let item = PlanItem {
            id: "1".into(),
            content: "refactor the parser".into(),
            status: "done".into(),
            priority: "high".into(),
            subsystem: None,
            file_scope: vec![],
            blocked_by: vec![],
            assigned_to: Some("session_fox_1".into()),
        };
        assert!(!apply_plan(&mut todos, &[item]));
        assert_eq!(todos[0].status, "pending");
        assert!(todos[0].assigned_to.is_none());
    }

    #[test]
    fn duplicate_text_prefers_matching_id() {
        let mut todos = vec![todo("a", "pending", None), todo("b", "pending", None)];
        todos[1].content = "task a".into();
        assert!(apply_assignment(&mut todos, "b", "task a", "session_ox_2"));
        assert!(todos[0].assigned_to.is_none());
        assert_eq!(todos[1].assigned_to.as_deref(), Some("session_ox_2"));
    }

    #[test]
    fn edge_inputs_are_noops() {
        assert!(plan_items_from_todos(&[]).is_empty());
        assert!(plan_items_from_todos(&[todo("a", "completed", Some(1))]).is_empty());
        let mut empty: Vec<TodoItem> = vec![];
        assert!(!apply_plan(
            &mut empty,
            &plan_items_from_todos(&[todo("a", "pending", None)])
        ));
        // A completed todo is not reopened or reassigned by a later running item.
        let mut todos = vec![todo("a", "completed", None)];
        let mut items = plan_items_from_todos(&[todo("a", "pending", None)]);
        items[0].status = "running".into();
        items[0].assigned_to = Some("session_fox_1".into());
        apply_plan(&mut todos, &items);
        assert_eq!(todos[0].status, "completed");
    }
}
