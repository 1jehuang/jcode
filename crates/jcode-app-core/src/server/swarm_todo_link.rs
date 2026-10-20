//! Link the coordinator's session todos to the swarm plan.
//!
//! The todo list is the single source of truth for what the coordinator is
//! doing. Two directions:
//!
//! * Todos -> plan: [`plan_items_from_todos`] turns open todos into plan items.
//!   Waves become dependency stages, so the model never writes `blocked_by`
//!   ids by hand (no cycles, no dangling references).
//! * Plan -> todos: [`link_assignment`] and [`sync_task_status`] write the
//!   worker session and task status back onto the matching todo, which drives
//!   the agent emoji in the pinned todo band.
//!
//! Matching is by id first, then by trimmed case-insensitive content, so a
//! plan proposed by hand still lights up the matching todo.

use crate::plan::PlanItem;
use crate::todo::TodoItem;

fn is_open(todo: &TodoItem) -> bool {
    !matches!(todo.status.as_str(), "completed" | "cancelled")
}

/// Convert open todos into swarm plan items.
///
/// Layout comes from [`crate::todo::stages::stage_layout`]: stages run in
/// order, and inside a stage each group is a lane. The first item of a lane
/// waits for every item of the previous stage. Later items in a lane wait for
/// the item before them in that lane, so a lane runs in order while different
/// lanes run in parallel. The model never writes ids (no cycles, no dangling
/// references). Completed and cancelled todos are skipped and never block.
pub(crate) fn plan_items_from_todos(todos: &[TodoItem]) -> Vec<PlanItem> {
    let open: Vec<&TodoItem> = todos.iter().filter(|todo| is_open(todo)).collect();
    let mut items = Vec::new();
    let mut previous_stage: Vec<String> = Vec::new();
    for stage in crate::todo::stages::stage_layout(&open) {
        for lane in &stage.lanes {
            let mut blocked_by = previous_stage.clone();
            for &index in &lane.items {
                let todo = open[index];
                items.push(PlanItem {
                    content: todo.content.clone(),
                    status: "queued".to_string(),
                    priority: todo.priority.clone(),
                    id: todo.id.clone(),
                    subsystem: todo.group.clone(),
                    file_scope: Vec::new(),
                    blocked_by: std::mem::take(&mut blocked_by),
                    assigned_to: None,
                });
                blocked_by = vec![todo.id.clone()];
            }
        }
        previous_stage = stage.item_indices().map(|i| open[i].id.clone()).collect();
    }
    items
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
    let Some(todo) = find_todo(todos, task_id, content) else {
        return false;
    };
    if crate::plan::is_completed_status(plan_status) && todo.status != "completed" {
        todo.status = "completed".to_string();
        return true;
    }
    false
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
}

/// Pure whole-plan reconcile: complete todos whose plan item is done and
/// record any worker assignment that has not landed yet. Idempotent.
pub(crate) fn apply_plan(todos: &mut [TodoItem], items: &[PlanItem]) -> bool {
    let mut changed = false;
    for item in items {
        if let Some(worker) = item.assigned_to.as_deref() {
            let already = find_todo(todos, &item.id, &item.content)
                .is_some_and(|todo| todo.assigned_to.is_some());
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
            vec![("a".into(), vec![]), ("b".into(), vec!["a".to_string()])]
        );
    }

    #[test]
    fn groups_are_sequential_lanes_inside_a_stage() {
        let items = plan_items_from_todos(&[
            laned("p1", 1, "parser"),
            laned("u1", 1, "ui"),
            laned("p2", 1, "parser"),
            laned("i1", 2, "integration"),
            laned("i2", 2, "integration"),
            laned("r1", 2, "release"),
        ]);
        assert_eq!(
            deps(&items),
            vec![
                ("p1".into(), vec![]),
                ("p2".into(), vec!["p1".to_string()]),
                ("u1".into(), vec![]),
                (
                    "i1".into(),
                    vec!["p1".to_string(), "p2".to_string(), "u1".to_string()]
                ),
                ("i2".into(), vec!["i1".to_string()]),
                (
                    "r1".into(),
                    vec!["p1".to_string(), "p2".to_string(), "u1".to_string()]
                ),
            ]
        );
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
        assert_eq!(
            deps(&items),
            vec![
                ("a".into(), vec![]),
                ("b".into(), vec!["a".to_string()]),
                ("c".into(), vec!["a".to_string()]),
                ("d".into(), vec!["b".to_string(), "c".to_string()]),
            ]
        );
        assert!(items.iter().all(|item| item.status == "queued"));
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
    fn plan_reconcile_links_and_completes_idempotently() {
        let mut todos = vec![todo("a", "pending", Some(1)), todo("b", "pending", Some(1))];
        let mut items = plan_items_from_todos(&todos);
        items[0].assigned_to = Some("session_fox_1".into());
        items[0].status = "done".into();
        items[1].assigned_to = Some("session_ox_2".into());
        items[1].status = "running".into();
        assert!(apply_plan(&mut todos, &items));
        assert_eq!(todos[0].status, "completed");
        assert_eq!(todos[0].assigned_to.as_deref(), Some("session_fox_1"));
        assert_eq!(todos[1].status, "in_progress");
        assert_eq!(todos[1].assigned_to.as_deref(), Some("session_ox_2"));
        assert!(!apply_plan(&mut todos, &items), "second pass is a no-op");
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
