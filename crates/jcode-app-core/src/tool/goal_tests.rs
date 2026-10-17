use super::*;
use tokio::time::{Duration, timeout};

#[tokio::test]
async fn initiative_tool_create_and_resume_round_trip() {
    let _guard = crate::storage::lock_test_env();
    let temp = tempfile::tempdir().expect("tempdir");
    let project = temp.path().join("repo");
    std::fs::create_dir_all(&project).expect("project dir");
    let prev_home = std::env::var_os("KCODE_HOME");
    crate::env::set_var("KCODE_HOME", temp.path());

    let tool = InitiativeTool::new();
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
    assert!(create.output.contains("Created initiative"));
    assert!(!create.output.contains("side panel"));

    // Creating an initiative must not spawn the side panel.
    let update = timeout(Duration::from_millis(200), bus_rx.recv()).await;
    if let Ok(Ok(event)) = update {
        assert!(
            !matches!(event, BusEvent::SidePanelUpdated(_)),
            "create must not publish a side panel update, got {:?}",
            event
        );
    }

    let persisted =
        crate::side_panel::snapshot_for_session("ses_goal_tool").expect("side panel snapshot");
    assert!(
        !persisted
            .pages
            .iter()
            .any(|page| page.id == "goal.ship-mobile-mvp"),
        "create must not write a goal page to the side panel"
    );

    let resume = tool
        .execute(json!({"action": "resume"}), ctx)
        .await
        .expect("resume goal");
    assert!(resume.output.contains("Resumed initiative"));
    assert!(resume.output.contains("finish reconnect flow"));

    if let Some(prev_home) = prev_home {
        crate::env::set_var("KCODE_HOME", prev_home);
    } else {
        crate::env::remove_var("KCODE_HOME");
    }
}

#[tokio::test]
async fn initiative_tool_list_does_not_open_side_panel_by_default() {
    let _guard = crate::storage::lock_test_env();
    let temp = tempfile::tempdir().expect("tempdir");
    let project = temp.path().join("repo");
    std::fs::create_dir_all(&project).expect("project dir");
    let prev_home = std::env::var_os("KCODE_HOME");
    crate::env::set_var("KCODE_HOME", temp.path());

    crate::goal::create_goal(
        crate::goal::GoalCreateInput {
            title: "Ship mobile MVP".to_string(),
            scope: crate::goal::GoalScope::Project,
            ..crate::goal::GoalCreateInput::default()
        },
        Some(&project),
    )
    .expect("create goal");

    let tool = InitiativeTool::new();
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
    assert!(
        !snapshot.pages.iter().any(|page| page.id == "goals"),
        "list must not open the goals overview in the side panel"
    );
    assert_eq!(snapshot.focused_page_id, None);

    if let Some(prev_home) = prev_home {
        crate::env::set_var("KCODE_HOME", prev_home);
    } else {
        crate::env::remove_var("KCODE_HOME");
    }
}

#[tokio::test]
async fn initiative_tool_update_refreshes_open_overview_without_stealing_focus() {
    let _guard = crate::storage::lock_test_env();
    let temp = tempfile::tempdir().expect("tempdir");
    let project = temp.path().join("repo");
    std::fs::create_dir_all(&project).expect("project dir");
    let prev_home = std::env::var_os("KCODE_HOME");
    crate::env::set_var("KCODE_HOME", temp.path());

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

    let tool = InitiativeTool::new();
    let ctx = ToolContext {
        session_id: "ses_goal_update".to_string(),
        message_id: "msg1".to_string(),
        tool_call_id: "tool1".to_string(),
        working_dir: Some(project.clone()),
        stdin_request_tx: None,
        graceful_shutdown_signal: None,
        execution_mode: crate::tool::ToolExecutionMode::AgentTurn,
    };

    // The user opens the overview explicitly (e.g. via /goals); the tool
    // itself never spawns the side panel.
    crate::goal::open_goals_overview_for_session("ses_goal_update", Some(&project), true)
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
        crate::env::set_var("KCODE_HOME", prev_home);
    } else {
        crate::env::remove_var("KCODE_HOME");
    }
}

#[test]
fn test_initiative_schema_milestones_define_items() {
    let schema = InitiativeTool::new().parameters_schema();
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
fn test_initiative_schema_omits_display_override() {
    let schema = InitiativeTool::new().parameters_schema();
    assert!(schema["properties"]["display"].is_null());
}

#[test]
fn test_initiative_schema_omits_public_enums_for_scope_and_status() {
    let schema = InitiativeTool::new().parameters_schema();
    assert!(schema["properties"]["scope"]["enum"].is_null());
    assert!(schema["properties"]["status"]["enum"].is_null());
}
