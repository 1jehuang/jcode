use super::*;

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
fn context() -> ToolContext {
    ToolContext {
        session_id: "external-blocker-tool".into(),
        message_id: "test".into(),
        tool_call_id: "test".into(),
        working_dir: None,
        stdin_request_tx: None,
        graceful_shutdown_signal: None,
        execution_mode: super::super::ToolExecutionMode::Direct,
    }
}

#[tokio::test]
async fn requires_user_action_tool_validates_updates_clears_and_cancels_reviews() {
    let _lock = crate::storage::lock_test_env();
    let _home = HomeGuard(std::env::var_os("JCODE_HOME"));
    let dir = tempfile::tempdir().unwrap();
    crate::env::set_var("JCODE_HOME", dir.path());
    let tool = TodoTool::new();
    let required = json!({"action":"Approve the account transfer", "reason":"Only the account owner can authorize the transfer"});
    let review = chrono::Utc::now() + chrono::Duration::minutes(5);
    tool.execute(
        json!({"add":[{
            "id":"a", "content":"Owner approval", "status":"pending", "priority":"high", "phase":1, "lane":"approval",
            "blocked_by":["prerequisite"],
            "external_blocker":{"reason":"Waiting for confirmation", "review_at":review}
        }]}),
        context(),
    )
    .await
    .unwrap();
    assert_eq!(
        crate::ambient::AmbientManager::new().unwrap().queue().len(),
        1
    );
    // A historical task may have been cleaned up. Retaining its useful
    // dependency reference must not block or erase the new user-only wait.
    let mut prior = load_todos("external-blocker-tool").unwrap();
    prior[0].waiting_on = vec!["historical-task".into()];
    save_todos("external-blocker-tool", &prior).unwrap();
    for bad in [
        json!({"action":"", "reason":"owner only"}),
        json!({"action":"verify", "reason":" \t\n"}),
        json!({"action":"verify"}),
        json!({"reason":"owner only"}),
    ] {
        assert!(
            tool.execute(
                json!({"update":[{"id":"a", "requires_user_action":bad}]}),
                context()
            )
            .await
            .is_err()
        );
        assert!(
            load_todos("external-blocker-tool").unwrap()[0]
                .requires_user_action
                .is_none()
        );
    }
    tool.execute(
        json!({"update":[{"id":"a", "requires_user_action":required}]}),
        context(),
    )
    .await
    .unwrap();
    let stored = load_todos("external-blocker-tool").unwrap();
    assert!(crate::todo::todo_requires_user_action(&stored[0]));
    assert!(stored[0].external_blocker.is_none());
    assert_eq!(stored[0].blocked_by, vec!["prerequisite"]);
    assert_eq!(stored[0].waiting_on, vec!["historical-task"]);
    assert!(
        crate::ambient::AmbientManager::new()
            .unwrap()
            .queue()
            .is_empty()
    );
    tool.execute(
        json!({"update":[{"id":"a", "requires_user_action":null}]}),
        context(),
    )
    .await
    .unwrap();
    assert!(
        load_todos("external-blocker-tool").unwrap()[0]
            .requires_user_action
            .is_none()
    );
    for status in ["completed", "cancelled"] {
        tool.execute(
            json!({"update":[{"id":"a", "status":"pending", "requires_user_action":required}]}),
            context(),
        )
        .await
        .unwrap();
        tool.execute(json!({"update":[{"id":"a", "status":status}]}), context())
            .await
            .unwrap();
        assert!(
            load_todos("external-blocker-tool").unwrap()[0]
                .requires_user_action
                .is_none()
        );
        assert!(
            crate::ambient::AmbientManager::new()
                .unwrap()
                .queue()
                .is_empty()
        );
    }
    tool.execute(json!({"add":[{"id":"b", "content":"Verify identity", "status":"pending", "priority":"high", "phase":1, "lane":"identity", "requires_user_action":required}]}), context()).await.unwrap();
    assert!(crate::todo::todo_requires_user_action(
        &load_todos("external-blocker-tool").unwrap()[1]
    ));
    assert!(
        crate::ambient::AmbientManager::new()
            .unwrap()
            .queue()
            .is_empty()
    );
}

#[test]
fn requires_user_action_schema_is_shared_and_strict() {
    let schema = TodoTool::new().parameters_schema();
    let update = &schema["properties"]["update"]["items"]["properties"]["requires_user_action"];
    let add = &schema["properties"]["add"]["items"]["properties"]["requires_user_action"];
    let phase = &schema["properties"]["phases"]["items"]["properties"]["lanes"]["items"]["properties"]
        ["items"]["items"]["properties"]["requires_user_action"];
    assert_eq!(update, add);
    assert_eq!(update, phase);
    assert_eq!(update["type"], json!(["object", "null"]));
    assert_eq!(update["required"], json!(["action", "reason"]));
    assert_eq!(update["additionalProperties"], false);
    assert!(
        TodoTool::new()
            .description()
            .contains("no safe authorized agent alternative")
    );
}

#[tokio::test]
async fn external_blocker_schedule_failure_clears_suppression() {
    let _lock = crate::storage::lock_test_env();
    let _home = HomeGuard(std::env::var_os("JCODE_HOME"));
    let dir = tempfile::tempdir().unwrap();
    crate::env::set_var("JCODE_HOME", dir.path());
    let manager = crate::ambient::AmbientManager::new().unwrap();
    drop(manager);
    // A directory at the lock path deterministically denies queue mutation
    // while ordinary todo persistence remains available.
    let queue_path = dir.path().join("ambient/queue.json");
    std::fs::create_dir_all(queue_path.with_extension("json.lock")).unwrap();
    let review = chrono::Utc::now() + chrono::Duration::minutes(5);
    let error = TodoTool::new()
        .execute(
            json!({"todos":[{
                "id":"a", "content":"await acceptance", "status":"pending", "priority":"high",
                "external_blocker":{"reason":"Coordinator acceptance", "review_at":review}
            }]}),
            context(),
        )
        .await
        .unwrap_err();
    assert!(
        error.to_string().contains("blockers were cleared"),
        "{error:#}"
    );
    let stored = load_todos("external-blocker-tool").unwrap();
    assert_eq!(stored[0].status, "pending");
    assert!(stored[0].external_blocker.is_none());
    assert!(!crate::todo::todo_has_external_blocker(&stored[0]));
}

#[tokio::test]
async fn external_blocker_tool_validates_schedules_updates_clears_and_completes() {
    let _lock = crate::storage::lock_test_env();
    let _home = HomeGuard(std::env::var_os("JCODE_HOME"));
    let dir = tempfile::tempdir().unwrap();
    crate::env::set_var("JCODE_HOME", dir.path());
    let tool = TodoTool::new();
    let review = chrono::Utc::now() + chrono::Duration::minutes(5);
    let blocker = json!({"reason":"Coordinator acceptance", "review_at":review});
    tool.execute(
        json!({"phases":[{"phase":1,"lanes":[{"lane":"review","items":[{
            "id":"a", "content":"Await acceptance", "status":"pending", "priority":"high",
            "external_blocker":blocker
        }]}]}]}),
        context(),
    )
    .await
    .unwrap();
    let stored = load_todos("external-blocker-tool").unwrap();
    assert!(crate::todo::todo_has_external_blocker(&stored[0]));
    let scheduled = crate::ambient::AmbientManager::new()
        .unwrap()
        .queue()
        .items()[0]
        .clone();
    assert_eq!(scheduled.scheduled_for, review);
    for bad in [
        json!({"reason":"", "review_at":review}),
        json!({"reason":"wait"}),
        json!({"reason":"wait", "review_at":"never"}),
        json!({"reason":"wait", "review_at":"2000-01-01T00:00:00Z"}),
    ] {
        assert!(
            tool.execute(
                json!({"update":[{"id":"a", "external_blocker":bad}]}),
                context()
            )
            .await
            .is_err()
        );
        assert_eq!(load_todos("external-blocker-tool").unwrap(), stored);
        assert_eq!(
            crate::ambient::AmbientManager::new().unwrap().queue().len(),
            1
        );
    }
    tool.execute(
        json!({"update":[{"id":"a", "content":"Still awaiting acceptance"}]}),
        context(),
    )
    .await
    .unwrap();
    assert_eq!(
        crate::ambient::AmbientManager::new()
            .unwrap()
            .queue()
            .items()[0]
            .id,
        scheduled.id
    );
    let later = review + chrono::Duration::minutes(5);
    tool.execute(json!({"update":[{"id":"a", "external_blocker":{"reason":"New evidence pending", "review_at":later}}]}), context()).await.unwrap();
    let replaced = crate::ambient::AmbientManager::new()
        .unwrap()
        .queue()
        .items()[0]
        .clone();
    assert_ne!(replaced.id, scheduled.id);
    assert_eq!(replaced.scheduled_for, later);
    assert!(!crate::ambient::scheduled_todo_review_is_current(
        &scheduled
    ));
    tool.execute(
        json!({"update":[{"id":"a", "external_blocker":null}]}),
        context(),
    )
    .await
    .unwrap();
    assert!(
        load_todos("external-blocker-tool").unwrap()[0]
            .external_blocker
            .is_none()
    );
    assert!(
        crate::ambient::AmbientManager::new()
            .unwrap()
            .queue()
            .is_empty()
    );
    tool.execute(
        json!({"update":[{"id":"a", "external_blocker":blocker}]}),
        context(),
    )
    .await
    .unwrap();
    tool.execute(
        json!({"update":[{"id":"a", "status":"completed"}]}),
        context(),
    )
    .await
    .unwrap();
    assert!(
        load_todos("external-blocker-tool").unwrap()[0]
            .external_blocker
            .is_none()
    );
    assert!(
        crate::ambient::AmbientManager::new()
            .unwrap()
            .queue()
            .is_empty()
    );
}
