use super::*;
use tokio::time::{Duration, timeout};

#[tokio::test]
async fn goal_tool_create_and_resume_round_trip() {
    let _guard = crate::storage::lock_test_env();
    let temp = tempfile::tempdir().expect("tempdir");
    let project = temp.path().join("repo");
    std::fs::create_dir_all(&project).expect("project dir");
    let prev_home = std::env::var_os("JCODE_HOME");
    crate::env::set_var("JCODE_HOME", temp.path());

    let tool = GoalTool::new();
    let ctx = ToolContext {
        session_id: "ses_goal_tool".to_string(),
        message_id: "msg1".to_string(),
        tool_call_id: "tool1".to_string(),
        working_dir: Some(project.clone()),
        stdin_request_tx: None,
        graceful_shutdown_signal: None,
        execution_mode: crate::tool::ToolExecutionMode::AgentTurn,
    };

    let mut bus_rx = Bus::global().subscribe();

    let create = tool
        .execute(
            json!({
                "action": "create",
                "title": "Ship mobile MVP",
                "scope": "project",
                "next_steps": ["finish reconnect flow"]
            }),
            ctx.clone(),
        )
        .await
        .expect("create goal");
    assert!(create.output.contains("Created goal"));

    let update = timeout(Duration::from_secs(1), bus_rx.recv())
        .await
        .expect("side panel update timeout")
        .expect("side panel update event");
    let snapshot = match update {
        BusEvent::SidePanelUpdated(update) => update.snapshot,
        other => panic!("expected side panel update event, got {:?}", other),
    };
    assert_eq!(
        snapshot.focused_page_id.as_deref(),
        Some("goal.ship-mobile-mvp")
    );

    let persisted =
        crate::side_panel::snapshot_for_session("ses_goal_tool").expect("side panel snapshot");
    assert_eq!(
        persisted.focused_page_id.as_deref(),
        Some("goal.ship-mobile-mvp")
    );

    let resume = tool
        .execute(json!({"action": "resume"}), ctx)
        .await
        .expect("resume goal");
    assert!(resume.output.contains("Resumed goal"));
    assert!(resume.output.contains("finish reconnect flow"));

    if let Some(prev_home) = prev_home {
        crate::env::set_var("JCODE_HOME", prev_home);
    } else {
        crate::env::remove_var("JCODE_HOME");
    }
}

#[tokio::test]
async fn goal_tool_list_opens_goals_overview_by_default() {
    let _guard = crate::storage::lock_test_env();
    let temp = tempfile::tempdir().expect("tempdir");
    let project = temp.path().join("repo");
    std::fs::create_dir_all(&project).expect("project dir");
    let prev_home = std::env::var_os("JCODE_HOME");
    crate::env::set_var("JCODE_HOME", temp.path());

    crate::goal::create_goal(
        crate::goal::GoalCreateInput {
            title: "Ship mobile MVP".to_string(),
            scope: crate::goal::GoalScope::Project,
            ..crate::goal::GoalCreateInput::default()
        },
        Some(&project),
    )
    .expect("create goal");

    let tool = GoalTool::new();
    let ctx = ToolContext {
        session_id: "ses_goal_list".to_string(),
        message_id: "msg1".to_string(),
        tool_call_id: "tool1".to_string(),
        working_dir: Some(project.clone()),
        stdin_request_tx: None,
        graceful_shutdown_signal: None,
        execution_mode: crate::tool::ToolExecutionMode::AgentTurn,
    };

    let list = tool
        .execute(json!({"action": "list"}), ctx)
        .await
        .expect("list goals");

    assert!(list.output.contains("# Goals"));
    let snapshot =
        crate::side_panel::snapshot_for_session("ses_goal_list").expect("side panel snapshot");
    assert_eq!(snapshot.focused_page_id.as_deref(), Some("goals"));

    if let Some(prev_home) = prev_home {
        crate::env::set_var("JCODE_HOME", prev_home);
    } else {
        crate::env::remove_var("JCODE_HOME");
    }
}

#[tokio::test]
async fn goal_tool_update_refreshes_open_overview_without_stealing_focus() {
    let _guard = crate::storage::lock_test_env();
    let temp = tempfile::tempdir().expect("tempdir");
    let project = temp.path().join("repo");
    std::fs::create_dir_all(&project).expect("project dir");
    let prev_home = std::env::var_os("JCODE_HOME");
    crate::env::set_var("JCODE_HOME", temp.path());

    let goal = crate::goal::create_goal(
        crate::goal::GoalCreateInput {
            title: "Ship mobile MVP".to_string(),
            scope: crate::goal::GoalScope::Project,
            next_steps: vec!["finish reconnect flow".to_string()],
            ..crate::goal::GoalCreateInput::default()
        },
        Some(&project),
    )
    .expect("create goal");

    let tool = GoalTool::new();
    let ctx = ToolContext {
        session_id: "ses_goal_update".to_string(),
        message_id: "msg1".to_string(),
        tool_call_id: "tool1".to_string(),
        working_dir: Some(project.clone()),
        stdin_request_tx: None,
        graceful_shutdown_signal: None,
        execution_mode: crate::tool::ToolExecutionMode::AgentTurn,
    };

    tool.execute(json!({"action": "list"}), ctx.clone())
        .await
        .expect("open goals overview");

    tool.execute(
        json!({
            "action": "update",
            "id": goal.id,
            "next_steps": ["ship reconnect flow"]
        }),
        ctx,
    )
    .await
    .expect("update goal");

    let snapshot =
        crate::side_panel::snapshot_for_session("ses_goal_update").expect("side panel snapshot");
    assert_eq!(snapshot.focused_page_id.as_deref(), Some("goals"));
    let goals_page = snapshot
        .pages
        .iter()
        .find(|page| page.id == "goals")
        .expect("goals page");
    assert!(goals_page.content.contains("ship reconnect flow"));

    if let Some(prev_home) = prev_home {
        crate::env::set_var("JCODE_HOME", prev_home);
    } else {
        crate::env::remove_var("JCODE_HOME");
    }
}

#[test]
fn test_goal_schema_milestones_define_items() {
    let schema = GoalTool::new().parameters_schema();
    let milestone_items = &schema["properties"]["milestones"]["items"];

    assert_eq!(milestone_items["type"], "object");
    assert_eq!(milestone_items["additionalProperties"], json!(true));
    assert_eq!(milestone_items["properties"]["steps"]["type"], "array");
    assert_eq!(
        milestone_items["properties"]["steps"]["items"]["additionalProperties"],
        json!(true)
    );
}

#[test]
fn test_goal_schema_omits_display_override() {
    let schema = GoalTool::new().parameters_schema();
    assert!(schema["properties"]["display"].is_null());
}

#[test]
fn test_goal_schema_omits_public_enums_for_scope_and_status() {
    let schema = GoalTool::new().parameters_schema();
    assert!(schema["properties"]["scope"]["enum"].is_null());
    assert!(schema["properties"]["status"]["enum"].is_null());
}
