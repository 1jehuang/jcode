//! External todo blockers use the normal durable scheduler, never a UI timer.
use super::{AmbientManager, Priority, ScheduleRequest, ScheduleTarget, ScheduledItem};
use crate::todo::{TodoItem, is_incomplete_todo};
use anyhow::Result;

#[cfg(test)]
#[path = "todo_reviews_tests.rs"]
mod tests;

/// Reconcile a supplied snapshot, primarily for callers already owning its state.
/// The complete queue reconciliation is one locked reload/mutate/save operation.
pub fn reconcile_todo_reviews(session_id: &str, todos: &[TodoItem]) -> Result<()> {
    AmbientManager::new()?.transact_queue(|items| {
        reconcile_items(items, session_id, todos);
        Ok(())
    })
}

/// Production todo writes reload the current todo state under the queue lock so
/// an older concurrent writer cannot overwrite a newer review generation.
pub fn reconcile_stored_todo_reviews(session_id: &str) -> Result<()> {
    AmbientManager::new()?.transact_queue(|items| {
        let todos = crate::todo::load_todos(session_id)?;
        reconcile_items(items, session_id, &todos);
        Ok(())
    })
}

fn reconcile_items(items: &mut Vec<ScheduledItem>, session_id: &str, todos: &[TodoItem]) {
    let desired: Vec<_> = todos
        .iter()
        .filter(|todo| {
            is_incomplete_todo(todo)
                && !crate::todo::todo_completely_blocked(todo)
                && todo
                    .external_blocker
                    .as_ref()
                    .is_some_and(|blocker| !blocker.reason.trim().is_empty())
        })
        .collect();
    // Keep one existing ID for each exact current generation, preserving user
    // schedules and foreign sessions. Repair any historical duplicate reviews.
    let mut seen = std::collections::HashSet::new();
    items.retain(|item| {
        let ScheduleTarget::TodoReview {
            session_id: owner,
            todo_id,
            blocker,
        } = &item.target
        else {
            return true;
        };
        if owner != session_id {
            return true;
        }
        desired
            .iter()
            .any(|todo| todo.id == *todo_id && todo.external_blocker.as_ref() == Some(blocker))
            && seen.insert(todo_id.clone())
    });
    for todo in desired {
        let blocker = todo.external_blocker.as_ref().expect("checked blocker");
        if seen.contains(&todo.id) || blocker.review_at <= chrono::Utc::now() {
            continue;
        }
        items.push(AmbientManager::scheduled_item(ScheduleRequest {
            wake_in_minutes: None,
            wake_at: Some(blocker.review_at),
            context: format!(
                "[todo blocker review] Re-read todo {} before taking action. Its external blocker review deadline has arrived: {}. This is a request to reassess, not proof the blocker resolved and not permission to complete or reopen the todo. If the todo is now completed, cancelled, cleared, or has a different blocker/deadline, ignore this stale reminder. Otherwise check the external condition, then clear the blocker and continue useful work, or update it with fresh evidence and another finite review_at.",
                todo.id, blocker.reason
            ),
            priority: Priority::Normal,
            target: ScheduleTarget::TodoReview {
                session_id: session_id.to_string(), todo_id: todo.id.clone(), blocker: blocker.clone(),
            },
            created_by_session: session_id.to_string(),
            working_dir: None,
            task_description: Some(format!("Reassess external blocker: {}", todo.content)),
            relevant_files: Vec::new(), git_branch: None, additional_context: None,
        }));
        seen.insert(todo.id.clone());
    }
}

/// A queued entry can already have been claimed when the todo is updated.
/// Compare the complete blocker, not only its ID, before waking any agent.
pub fn scheduled_todo_review_is_current(item: &ScheduledItem) -> bool {
    let ScheduleTarget::TodoReview {
        session_id,
        todo_id,
        blocker,
    } = &item.target
    else {
        return true;
    };
    crate::todo::load_todos(session_id)
        .ok()
        .is_some_and(|todos| {
            todos.iter().any(|todo| {
                todo.id == *todo_id
                    && is_incomplete_todo(todo)
                    && !crate::todo::todo_completely_blocked(todo)
                    && todo.external_blocker.as_ref() == Some(blocker)
                    && !blocker.reason.trim().is_empty()
                    && blocker.review_at <= chrono::Utc::now()
            })
        })
}
