//! External todo blockers use the normal durable scheduler, never a UI timer.
use super::{AmbientManager, Priority, ScheduleRequest, ScheduleTarget, ScheduledItem};
use crate::todo::{TodoItem, is_incomplete_todo};
use anyhow::Result;

#[cfg(test)]
#[path = "todo_reviews_tests.rs"]
mod tests;

/// Replace only this session's internal review entries. User-created schedules
/// and other sessions are untouched. Absolute deadlines survive reloads.
pub fn reconcile_todo_reviews(session_id: &str, todos: &[TodoItem]) -> Result<()> {
    let mut manager = AmbientManager::new()?;
    let desired: Vec<_> = todos
        .iter()
        .filter(|todo| {
            is_incomplete_todo(todo)
                && todo
                    .external_blocker
                    .as_ref()
                    .is_some_and(|blocker| !blocker.reason.trim().is_empty())
        })
        .collect();
    let stale: Vec<String> = manager
        .queue()
        .items()
        .iter()
        .filter_map(|item| {
            let ScheduleTarget::TodoReview {
                session_id: owner,
                todo_id,
                blocker,
            } = &item.target
            else {
                return None;
            };
            (owner == session_id
                && !desired.iter().any(|todo| {
                    todo.id == *todo_id && todo.external_blocker.as_ref() == Some(blocker)
                }))
            .then(|| item.id.clone())
        })
        .collect();
    for id in stale {
        manager.cancel_schedule(&id)?;
    }
    for todo in desired {
        let blocker = todo.external_blocker.as_ref().expect("checked blocker");
        if manager.queue().items().iter().any(|item| {
            matches!(&item.target,
            ScheduleTarget::TodoReview { session_id: owner, todo_id, blocker: scheduled }
            if owner == session_id && todo_id == &todo.id && scheduled == blocker)
        }) {
            continue;
        }
        // Preserve an already queued due review on unrelated writes, but do
        // not repeatedly recreate one after its durable delivery was claimed.
        if blocker.review_at <= chrono::Utc::now() {
            continue;
        }
        manager.schedule(ScheduleRequest {
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
        })?;
    }
    Ok(())
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
                    && todo.external_blocker.as_ref() == Some(blocker)
                    && !blocker.reason.trim().is_empty()
                    && blocker.review_at <= chrono::Utc::now()
            })
        })
}
