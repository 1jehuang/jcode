use super::*;
use crate::todo::{TodoExternalBlocker, TodoItem};

struct HomeGuard(Option<std::ffi::OsString>);
impl Drop for HomeGuard {
    fn drop(&mut self) {
        if let Some(old) = self.0.take() {
            crate::env::set_var("JCODE_HOME", old);
        } else {
            crate::env::remove_var("JCODE_HOME");
        }
    }
}

fn blocked(id: &str) -> TodoItem {
    TodoItem {
        id: id.into(),
        content: id.into(),
        status: "pending".into(),
        external_blocker: Some(TodoExternalBlocker {
            reason: "Coordinator acceptance of frozen source".into(),
            review_at: chrono::Utc::now() + chrono::Duration::minutes(20),
        }),
        ..Default::default()
    }
}

#[test]
fn external_blocker_reviews_are_durable_stable_and_cancel_only_owned_entries() {
    let _lock = crate::storage::lock_test_env();
    let _home = HomeGuard(std::env::var_os("JCODE_HOME"));
    let dir = tempfile::tempdir().unwrap();
    crate::env::set_var("JCODE_HOME", dir.path());
    let mut todo = blocked("acceptance");
    let other = blocked("other");
    reconcile_todo_reviews("other-session", &[other]).unwrap();
    let mut manager = AmbientManager::new().unwrap();
    let user_id = manager
        .schedule(ScheduleRequest {
            wake_in_minutes: Some(60),
            wake_at: None,
            context: "user reminder".into(),
            priority: Priority::Normal,
            target: ScheduleTarget::Session {
                session_id: "owner".into(),
            },
            created_by_session: "owner".into(),
            working_dir: None,
            task_description: None,
            relevant_files: vec![],
            git_branch: None,
            additional_context: None,
        })
        .unwrap();
    reconcile_todo_reviews("owner", std::slice::from_ref(&todo)).unwrap();
    let manager = AmbientManager::new().unwrap();
    assert_eq!(manager.queue().len(), 3);
    let old = manager
        .queue()
        .items()
        .iter()
        .find(|item| {
            matches!(&item.target,
        ScheduleTarget::TodoReview { session_id, .. } if session_id == "owner")
        })
        .unwrap()
        .clone();
    assert_eq!(
        old.scheduled_for,
        todo.external_blocker.as_ref().unwrap().review_at
    );
    assert!(crate::ambient::format_scheduled_session_message(&old).contains("not proof"));
    reconcile_todo_reviews("owner", std::slice::from_ref(&todo)).unwrap();
    assert!(
        AmbientManager::new()
            .unwrap()
            .queue()
            .items()
            .iter()
            .any(|item| item.id == old.id)
    );
    todo.external_blocker.as_mut().unwrap().reason =
        "Awaiting updated coordinator acceptance".into();
    reconcile_todo_reviews("owner", std::slice::from_ref(&todo)).unwrap();
    assert!(
        !AmbientManager::new()
            .unwrap()
            .queue()
            .items()
            .iter()
            .any(|item| item.id == old.id)
    );
    for status in ["completed", "cancelled"] {
        todo.status = status.into();
        reconcile_todo_reviews("owner", std::slice::from_ref(&todo)).unwrap();
        let manager = AmbientManager::new().unwrap();
        assert_eq!(manager.queue().len(), 2);
        assert!(
            manager
                .queue()
                .items()
                .iter()
                .any(|item| item.id == user_id)
        );
    }
    todo.status = "pending".into();
    reconcile_todo_reviews("owner", std::slice::from_ref(&todo)).unwrap();
    todo.external_blocker = None;
    reconcile_todo_reviews("owner", &[todo]).unwrap();
    assert_eq!(AmbientManager::new().unwrap().queue().len(), 2);
    reconcile_todo_reviews("other-session", &[]).unwrap();
    let manager = AmbientManager::new().unwrap();
    assert_eq!(manager.queue().len(), 1);
    assert_eq!(manager.queue().items()[0].id, user_id);
}

#[test]
fn external_blocker_due_review_rechecks_live_todo_and_never_changes_status() {
    let _lock = crate::storage::lock_test_env();
    let _home = HomeGuard(std::env::var_os("JCODE_HOME"));
    let dir = tempfile::tempdir().unwrap();
    crate::env::set_var("JCODE_HOME", dir.path());
    let mut todo = blocked("acceptance");
    reconcile_todo_reviews("owner", std::slice::from_ref(&todo)).unwrap();
    let mut item = AmbientManager::new().unwrap().queue().items()[0].clone();
    let due = chrono::Utc::now() - chrono::Duration::seconds(1);
    todo.external_blocker.as_mut().unwrap().review_at = due;
    if let ScheduleTarget::TodoReview { blocker, .. } = &mut item.target {
        blocker.review_at = due;
    }
    let mut manager = AmbientManager::new().unwrap();
    manager.cancel_schedule(&item.id).unwrap();
    let due_id = manager
        .schedule(ScheduleRequest {
            wake_in_minutes: None,
            wake_at: Some(due),
            context: item.context.clone(),
            priority: Priority::Normal,
            target: item.target.clone(),
            created_by_session: "owner".into(),
            working_dir: None,
            task_description: None,
            relevant_files: vec![],
            git_branch: None,
            additional_context: None,
        })
        .unwrap();
    reconcile_todo_reviews("owner", std::slice::from_ref(&todo)).unwrap();
    assert_eq!(AmbientManager::new().unwrap().queue().items()[0].id, due_id);
    AmbientManager::new()
        .unwrap()
        .cancel_schedule(&due_id)
        .unwrap();
    reconcile_todo_reviews("owner", std::slice::from_ref(&todo)).unwrap();
    assert!(AmbientManager::new().unwrap().queue().is_empty());
    crate::todo::save_todos("owner", std::slice::from_ref(&todo)).unwrap();
    assert!(scheduled_todo_review_is_current(&item));
    assert_eq!(
        crate::todo::load_todos("owner").unwrap()[0].status,
        "pending"
    );
    for status in ["completed", "cancelled"] {
        let mut changed = todo.clone();
        changed.status = status.into();
        crate::todo::save_todos("owner", &[changed]).unwrap();
        assert!(!scheduled_todo_review_is_current(&item));
    }
    let mut changed = todo.clone();
    changed.external_blocker = None;
    crate::todo::save_todos("owner", &[changed]).unwrap();
    assert!(!scheduled_todo_review_is_current(&item));
    todo.external_blocker.as_mut().unwrap().review_at += chrono::Duration::minutes(30);
    crate::todo::save_todos("owner", &[todo]).unwrap();
    assert!(!scheduled_todo_review_is_current(&item));
    crate::todo::save_todos("owner", &[]).unwrap();
    assert!(!scheduled_todo_review_is_current(&item));
}
