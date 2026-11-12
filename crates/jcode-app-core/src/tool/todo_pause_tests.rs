//! Item pauses (`waiting_on`, `requires_user_auth`) and the goal-level
//! `cannot_complete` declaration. Items have no block of their own.
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

const SESSION: &str = "todo-pause-tool";

fn context() -> ToolContext {
    ToolContext {
        session_id: SESSION.into(),
        message_id: "test".into(),
        tool_call_id: "test".into(),
        working_dir: None,
        stdin_request_tx: None,
        graceful_shutdown_signal: None,
        execution_mode: super::super::ToolExecutionMode::Direct,
    }
}

fn item(extra: Value) -> Value {
    let mut item = json!({
        "id":"a", "content":"Deploy", "status":"pending", "priority":"high",
        "phase":1, "lane":"release"
    });
    for (key, value) in extra.as_object().unwrap() {
        item[key] = value.clone();
    }
    item
}

#[tokio::test]
async fn requires_user_auth_validates_updates_clears_and_ends_with_the_todo() {
    let _lock = crate::storage::lock_test_env();
    let _home = HomeGuard(std::env::var_os("JCODE_HOME"));
    let dir = tempfile::tempdir().unwrap();
    crate::env::set_var("JCODE_HOME", dir.path());
    let tool = TodoTool::new();
    let auth = json!({"reason":"Log in to the deploy dashboard"});
    tool.execute(json!({"add":[item(json!({}))]}), context())
        .await
        .unwrap();
    for bad in [json!({"reason":""}), json!({"reason":" \t\n"}), json!({})] {
        assert!(
            tool.execute(
                json!({"update":[{"id":"a", "requires_user_auth":bad}]}),
                context()
            )
            .await
            .is_err()
        );
        assert!(load_todos(SESSION).unwrap()[0].requires_user_auth.is_none());
    }
    tool.execute(
        json!({"update":[{"id":"a", "requires_user_auth":auth}]}),
        context(),
    )
    .await
    .unwrap();
    let stored = load_todos(SESSION).unwrap();
    assert!(crate::todo::todo_requires_user_auth(&stored[0]));
    assert_eq!(stored[0].status, "pending");
    tool.execute(
        json!({"update":[{"id":"a", "requires_user_auth":null}]}),
        context(),
    )
    .await
    .unwrap();
    assert!(load_todos(SESSION).unwrap()[0].requires_user_auth.is_none());
    for status in ["completed", "cancelled"] {
        tool.execute(
            json!({"update":[{"id":"a", "status":"pending", "requires_user_auth":auth}]}),
            context(),
        )
        .await
        .unwrap();
        tool.execute(json!({"update":[{"id":"a", "status":status}]}), context())
            .await
            .unwrap();
        assert!(load_todos(SESSION).unwrap()[0].requires_user_auth.is_none());
    }
}

#[test]
fn waiting_on_accepts_object_or_list_and_keeps_the_reason_with_its_ids() {
    let initial = json!([item(
        json!({"waiting_on":["old"], "waiting_reason":"old reason"})
    )]);
    let patched = |waiting_on: Value| {
        let input = apply_todo_patch(
            json!({"update":[{"id":"a", "waiting_on":waiting_on}]}),
            || initial.clone(),
        )
        .unwrap();
        parse_todo_input(input).unwrap().todos.unwrap().remove(0)
    };
    let todo = patched(json!({"ids":["build"], "reason":" Benchmark still running "}));
    assert_eq!(todo.waiting_on, vec!["build"]);
    assert_eq!(
        todo.waiting_reason.as_deref(),
        Some("Benchmark still running")
    );
    // Providers sometimes stringify nested objects.
    let todo = patched(json!(r#"{"ids":["build"],"reason":"Worker reports back"}"#));
    assert_eq!(todo.waiting_on, vec!["build"]);
    assert_eq!(todo.waiting_reason.as_deref(), Some("Worker reports back"));
    // A bare list is still accepted, and replacing the IDs drops the old reason.
    let todo = patched(json!(["new"]));
    assert_eq!(todo.waiting_on, vec!["new"]);
    assert_eq!(todo.waiting_reason, None);
    for cleared in [
        Value::Null,
        json!([]),
        json!({"ids":[], "reason":"nothing"}),
    ] {
        let todo = patched(cleared);
        assert!(todo.waiting_on.is_empty());
    }
    // An unrelated edit leaves the stored wait alone.
    let input = apply_todo_patch(json!({"update":[{"id":"a", "priority":"low"}]}), || {
        initial.clone()
    })
    .unwrap();
    let todo = parse_todo_input(input).unwrap().todos.unwrap().remove(0);
    assert_eq!(todo.waiting_on, vec!["old"]);
    assert_eq!(todo.waiting_reason.as_deref(), Some("old reason"));
}

#[tokio::test]
async fn removed_item_blockers_are_rejected_with_the_replacement() {
    let _lock = crate::storage::lock_test_env();
    let _home = HomeGuard(std::env::var_os("JCODE_HOME"));
    let dir = tempfile::tempdir().unwrap();
    crate::env::set_var("JCODE_HOME", dir.path());
    let tool = TodoTool::new();
    tool.execute(json!({"add":[item(json!({}))]}), context())
        .await
        .unwrap();
    let before = load_todos(SESSION).unwrap();
    for (field, value) in [
        ("completely_blocked", json!({"reason":"No way forward"})),
        (
            "external_blocker",
            json!({"reason":"Waiting", "review_at":"2099-01-01T00:00:00Z"}),
        ),
    ] {
        let error = tool
            .execute(json!({"update":[{"id":"a", field:value}]}), context())
            .await
            .unwrap_err()
            .to_string();
        assert!(error.contains(&format!("{field} was removed")), "{error}");
        for replacement in ["waiting_on", "requires_user_auth", "cannot_complete"] {
            assert!(error.contains(replacement), "{error}");
        }
        assert_eq!(load_todos(SESSION).unwrap(), before);
        // Strict-schema providers send every unknown-to-them optional as null.
        tool.execute(json!({"update":[{"id":"a", field:null}]}), context())
            .await
            .unwrap();
    }
}

#[tokio::test]
async fn goal_cannot_complete_is_recorded_reported_and_cleared_by_empty_string() {
    let _lock = crate::storage::lock_test_env();
    let _home = HomeGuard(std::env::var_os("JCODE_HOME"));
    let dir = tempfile::tempdir().unwrap();
    crate::env::set_var("JCODE_HOME", dir.path());
    let tool = TodoTool::new();
    tool.execute(json!({"add":[item(json!({}))]}), context())
        .await
        .unwrap();
    let group = load_todos(SESSION).unwrap()[0].group.clone();
    let write = |cannot_complete: Value| {
        let mut goal = json!({"closed_feedback_loop":"usable"});
        if let Some(group) = &group {
            goal["group"] = json!(group);
        }
        if !cannot_complete.is_null() {
            goal["cannot_complete"] = cannot_complete;
        }
        json!({"goals":[goal]})
    };
    let output = tool
        .execute(
            write(json!(" Source data was deleted upstream ")),
            context(),
        )
        .await
        .unwrap();
    let goals = load_goals(SESSION).unwrap();
    assert_eq!(
        goals[0].cannot_complete.as_deref(),
        Some("Source data was deleted upstream")
    );
    let todos = load_todos(SESSION).unwrap();
    assert!(crate::todo::todo_goal_cannot_complete(&todos[0], &goals));
    assert_eq!(todos[0].status, "pending", "the todo itself is untouched");
    assert!(
        output.output.contains("Source data was deleted upstream"),
        "{}",
        output.output
    );
    // Omitting the field on a later assessment keeps the declaration.
    tool.execute(write(Value::Null), context()).await.unwrap();
    assert!(load_goals(SESSION).unwrap()[0].cannot_complete.is_some());
    tool.execute(write(json!("")), context()).await.unwrap();
    let goals = load_goals(SESSION).unwrap();
    assert!(goals[0].cannot_complete.is_none());
    assert!(!crate::todo::todo_goal_cannot_complete(&todos[0], &goals));
}

#[test]
fn pause_schema_is_shared_strict_and_carries_the_guidance() {
    let schema = TodoTool::new().parameters_schema();
    let properties = &schema["properties"];
    // Guidance is written once, on add. update and phases repeat the shape
    // without the description.
    let update = &properties["add"]["items"]["properties"];
    for field in ["waiting_on", "requires_user_auth"] {
        let canonical = &update[field];
        let mut bare = canonical.clone();
        bare.as_object_mut().unwrap().remove("description");
        assert_eq!(bare, properties["update"]["items"]["properties"][field]);
        assert_eq!(
            bare,
            properties["phases"]["items"]["properties"]["lanes"]["items"]["properties"]["items"]["items"]
                ["properties"][field]
        );
        assert_eq!(canonical["type"], json!(["object", "null"]));
        assert_eq!(canonical["additionalProperties"], false);
        assert!(canonical["description"].is_string());
    }
    assert_eq!(update["waiting_on"]["required"], json!(["ids", "reason"]));
    assert_eq!(update["requires_user_auth"]["required"], json!(["reason"]));
    for removed in [
        "completely_blocked",
        "external_blocker",
        "requires_user_action",
    ] {
        assert!(update.get(removed).is_none(), "{removed}");
    }
    // Pause rules live on the fields, not in the one-sentence tool description.
    let description = TodoTool::new().description().to_string();
    assert!(!description.contains("block"), "{description}");
    assert_eq!(description.matches(['.', ';']).count(), 2, "{description}");
    let auth = update["requires_user_auth"]["description"]
        .as_str()
        .unwrap();
    assert!(auth.starts_with("Pause until the user completes an authentication"));
    assert!(auth.contains("Approvals, decisions, retries"));
    assert!(auth.contains("use waiting_on"));
    let goal = &properties["goals"]["items"]["properties"]["cannot_complete"];
    assert_eq!(goal["type"], "string");
    let goal = goal["description"].as_str().unwrap();
    assert!(goal.contains("genuinely impossible"));
    assert!(goal.contains("Not for failures you can retry or work around"));
    assert!(!schema.to_string().contains("review_at"));
}
