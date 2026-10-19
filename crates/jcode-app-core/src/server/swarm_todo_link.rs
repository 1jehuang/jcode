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
/// Consecutive todos that share a `wave` form one stage; a todo without a wave
/// is a stage of its own. Every item depends on all items of the previous
/// stage, which gives "wave 2 starts after wave 1" without exposing ids to the
/// model. Completed and cancelled todos are skipped and never block anything.
pub(crate) fn plan_items_from_todos(todos: &[TodoItem]) -> Vec<PlanItem> {
    let mut items = Vec::new();
    let mut previous_stage: Vec<String> = Vec::new();
    let mut current_stage: Vec<String> = Vec::new();
    let mut current_wave: Option<u32> = None;
    let mut first = true;

    for todo in todos.iter().filter(|todo| is_open(todo)) {
        let continues_stage = !first && todo.wave.is_some() && todo.wave == current_wave;
        if !continues_stage && !first {
            previous_stage = std::mem::take(&mut current_stage);
        }
        first = false;
        current_wave = todo.wave;
        current_stage.push(todo.id.clone());
        items.push(PlanItem {
            content: todo.content.clone(),
            status: "queued".to_string(),
            priority: todo.priority.clone(),
            id: todo.id.clone(),
            subsystem: todo.group.clone(),
            file_scope: Vec::new(),
            blocked_by: previous_stage.clone(),
            assigned_to: None,
        });
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
    if let Some(index) = todos.iter().position(|todo| todo.id == task_id) {
        return todos.get_mut(index);
    }
    let wanted = normalize(content);
    if wanted.is_empty() {
        return None;
    }
    todos
        .iter_mut()
        .find(|todo| normalize(&todo.content) == wanted)
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

    fn todo(id: &str, status: &str, wave: Option<u32>) -> TodoItem {
        TodoItem {
            id: id.to_string(),
            content: format!("task {id}"),
            status: status.to_string(),
            priority: "high".to_string(),
            wave,
            ..Default::default()
        }
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
    fn waves_become_stages_and_completed_todos_are_skipped() {
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
        assert!(apply_assignment(&mut todos, "a", "", "session_fox_1"));
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
        assert!(!apply_task_status(&mut todos, "a", "", "failed"));
        assert_eq!(todos[0].status, "in_progress");
        assert!(apply_task_status(&mut todos, "a", "", "done"));
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
}
