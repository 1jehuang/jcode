use super::{Tool, ToolContext, ToolOutput};
use crate::bus::{Bus, BusEvent, SidePanelUpdated};
use anyhow::Result;
use async_trait::async_trait;
use serde::Deserialize;
use serde_json::{Value, json};

pub struct GoalTool;

impl GoalTool {
    pub fn new() -> Self {
        Self
    }
}

fn default_display_for_action(action: &str) -> crate::goal::GoalDisplayMode {
    match action {
        "list" | "create" | "show" | "focus" | "resume" => crate::goal::GoalDisplayMode::Focus,
        "update" | "checkpoint" => crate::goal::GoalDisplayMode::UpdateOnly,
        _ => crate::goal::GoalDisplayMode::Auto,
    }
}

fn publish_side_panel_snapshot(session_id: &str, snapshot: &crate::side_panel::SidePanelSnapshot) {
    Bus::global().publish(BusEvent::SidePanelUpdated(SidePanelUpdated {
        session_id: session_id.to_string(),
        snapshot: snapshot.clone(),
    }));
}

fn maybe_publish_goals_overview_refresh(
    session_id: &str,
    working_dir: Option<&std::path::Path>,
) -> Result<()> {
    if let Some(snapshot) =
        crate::goal::refresh_goals_overview_for_session(session_id, working_dir)?
    {
        publish_side_panel_snapshot(session_id, &snapshot);
    }
    Ok(())
}

fn goal_page_is_open(session_id: &str, goal_id: &str) -> Result<bool> {
    let page_id = crate::goal::goal_page_id(goal_id);
    let snapshot = crate::side_panel::snapshot_for_session(session_id)?;
    Ok(snapshot.pages.iter().any(|page| page.id == page_id))
}

#[derive(Debug, Deserialize)]
struct GoalInput {
    action: String,
    #[serde(default)]
    id: Option<String>,
    #[serde(default)]
    title: Option<String>,
    #[serde(default)]
    scope: Option<String>,
    #[serde(default)]
    status: Option<String>,
    #[serde(default)]
    description: Option<String>,
    #[serde(default)]
    why: Option<String>,
    #[serde(default)]
    success_criteria: Option<Vec<String>>,
    #[serde(default)]
    milestones: Option<Vec<crate::goal::GoalMilestone>>,
    #[serde(default)]
    next_steps: Option<Vec<String>>,
    #[serde(default)]
    blockers: Option<Vec<String>>,
    #[serde(default)]
    current_milestone_id: Option<String>,
    #[serde(default)]
    progress_percent: Option<u8>,
    #[serde(default)]
    checkpoint_summary: Option<String>,
    #[serde(default)]
    display: Option<String>,
}

fn goal_step_schema() -> Value {
    json!({
        "type": "object",
        "required": ["id", "content"],
            "properties": {
                "id": {
                    "type": "string",
                    "description": "ID."
                },
                "content": {
                    "type": "string",
                    "description": "Content."
                },
                "status": {
                    "type": "string",
                    "description": "Status."
                }
            },
        "additionalProperties": false
    })
}

fn goal_milestone_schema() -> Value {
    json!({
        "type": "object",
        "required": ["id", "title"],
            "properties": {
                "id": {
                    "type": "string",
                    "description": "ID."
                },
                "title": {
                    "type": "string",
                    "description": "Title."
                },
                "status": {
                    "type": "string",
                    "description": "Status."
                },
                "steps": {
                    "type": "array",
                    "items": goal_step_schema(),
                    "description": "Steps."
                }
            },
        "additionalProperties": false
    })
}

#[async_trait]
impl Tool for GoalTool {
    fn name(&self) -> &str {
        "goal"
    }

    fn description(&self) -> &str {
        "Manage goals."
    }

    fn parameters_schema(&self) -> Value {
        json!({
        "type": "object",
        "required": ["action"],
            "properties": {
                "action": {
                    "type": "string",
                    "enum": ["create", "list", "show", "resume", "update", "checkpoint", "focus"],
                    "description": "Action."
                },
                "id": {"type": "string"},
                "title": {"type": "string"},
                "scope": {"type": "string", "enum": ["project", "global"]},
                "status": {"type": "string", "enum": ["draft", "active", "paused", "blocked", "completed", "archived", "abandoned"]},
                "description": {"type": "string"},
                "why": {"type": "string"},
                "success_criteria": {"type": "array", "items": {"type": "string"}},
                "milestones": {"type": "array", "items": goal_milestone_schema()},
                "next_steps": {"type": "array", "items": {"type": "string"}},
                "blockers": {"type": "array", "items": {"type": "string"}},
                "current_milestone_id": {"type": "string"},
                "progress_percent": {"type": "integer"},
                "checkpoint_summary": {"type": "string"}
            }
        })
    }

    async fn execute(&self, input: Value, ctx: ToolContext) -> Result<ToolOutput> {
        let params: GoalInput = serde_json::from_value(input)?;
        let working_dir = ctx.working_dir.as_deref();
        let display = params
            .display
            .as_deref()
            .and_then(crate::goal::GoalDisplayMode::parse)
            .unwrap_or_else(|| default_display_for_action(&params.action));

        match params.action.as_str() {
            "list" => {
                let goals = crate::goal::list_relevant_goals(working_dir)?;
                if display != crate::goal::GoalDisplayMode::None {
                    let focus = display != crate::goal::GoalDisplayMode::UpdateOnly;
                    let snapshot = crate::goal::open_goals_overview_for_session(
                        &ctx.session_id,
                        working_dir,
                        focus,
                    )?;
                    publish_side_panel_snapshot(&ctx.session_id, &snapshot);
                }
                Ok(ToolOutput::new(crate::goal::render_goals_overview(&goals))
                    .with_title(format!("{} goals", goals.len()))
                    .with_metadata(serde_json::to_value(&goals)?))
            }
            "create" => {
                let title = params
                    .title
                    .as_deref()
                    .ok_or_else(|| anyhow::anyhow!("title is required for create"))?;
                let scope = params
                    .scope
                    .as_deref()
                    .and_then(crate::goal::GoalScope::parse)
                    .unwrap_or(crate::goal::GoalScope::Project);
                let goal = crate::goal::create_goal(
                    crate::goal::GoalCreateInput {
                        id: params.id.clone(),
                        title: title.to_string(),
                        scope,
                        description: params.description.clone(),
                        why: params.why.clone(),
                        success_criteria: params.success_criteria.unwrap_or_default(),
                        milestones: params.milestones.unwrap_or_default(),
                        next_steps: params.next_steps.unwrap_or_default(),
                        blockers: params.blockers.unwrap_or_default(),
                        current_milestone_id: params.current_milestone_id.clone(),
                        progress_percent: params.progress_percent,
                    },
                    working_dir,
                )?;
                let metadata = serde_json::to_value(&goal)?;
                let output = if display == crate::goal::GoalDisplayMode::None {
                    ToolOutput::new(format!("Created goal `{}` ({})", goal.id, goal.title))
                } else {
                    let snapshot =
                        crate::goal::write_goal_page(&ctx.session_id, working_dir, &goal, display)?;
                    publish_side_panel_snapshot(&ctx.session_id, &snapshot);
                    maybe_publish_goals_overview_refresh(&ctx.session_id, working_dir)?;
                    ToolOutput::new(format!(
                        "Created goal `{}` ({}) and opened it in the side panel.",
                        goal.id, goal.title
                    ))
                };
                Ok(output
                    .with_title(goal.title.clone())
                    .with_metadata(metadata))
            }
            "show" | "focus" => {
                let id = params
                    .id
                    .as_deref()
                    .ok_or_else(|| anyhow::anyhow!("id is required for show/focus"))?;
                if display == crate::goal::GoalDisplayMode::None {
                    let Some(goal) = crate::goal::load_goal(id, None, working_dir)? else {
                        anyhow::bail!("goal not found: {}", id);
                    };
                    crate::goal::attach_goal_to_session(&ctx.session_id, &goal, working_dir)?;
                    Ok(ToolOutput::new(crate::goal::render_goal_detail(&goal))
                        .with_title(goal.title.clone())
                        .with_metadata(serde_json::to_value(&goal)?))
                } else {
                    let Some(result) = crate::goal::open_goal_for_session(
                        &ctx.session_id,
                        working_dir,
                        id,
                        params.action == "focus" || display == crate::goal::GoalDisplayMode::Focus,
                    )?
                    else {
                        anyhow::bail!("goal not found: {}", id);
                    };
                    publish_side_panel_snapshot(&ctx.session_id, &result.snapshot);
                    Ok(
                        ToolOutput::new(crate::goal::render_goal_detail(&result.goal))
                            .with_title(result.goal.title.clone())
                            .with_metadata(serde_json::to_value(&result.goal)?),
                    )
                }
            }
            "resume" => {
                let goal = if display == crate::goal::GoalDisplayMode::None {
                    let Some(goal) = crate::goal::resume_goal(&ctx.session_id, working_dir)? else {
                        return Ok(ToolOutput::new("No resumable goals found."));
                    };
                    crate::goal::attach_goal_to_session(&ctx.session_id, &goal, working_dir)?;
                    goal
                } else {
                    let Some(result) = crate::goal::resume_goal_for_session(
                        &ctx.session_id,
                        working_dir,
                        display == crate::goal::GoalDisplayMode::Focus,
                    )?
                    else {
                        return Ok(ToolOutput::new("No resumable goals found."));
                    };
                    publish_side_panel_snapshot(&ctx.session_id, &result.snapshot);
                    result.goal
                };
                let mut output = format!("Resumed goal `{}` ({})", goal.id, goal.title);
                if let Some(progress) = goal.progress_percent {
                    output.push_str(&format!(" — {}%", progress));
                }
                if let Some(next_step) = goal.next_steps.first() {
                    output.push_str(&format!("\nNext step: {}", next_step));
                }
                Ok(ToolOutput::new(output)
                    .with_title(goal.title.clone())
                    .with_metadata(serde_json::to_value(&goal)?))
            }
            "update" | "checkpoint" => {
                let id = params
                    .id
                    .as_deref()
                    .ok_or_else(|| anyhow::anyhow!("id is required for update/checkpoint"))?;
                let status = params
                    .status
                    .as_deref()
                    .map(|value| {
                        crate::goal::GoalStatus::parse(value)
                            .ok_or_else(|| anyhow::anyhow!("invalid goal status: {}", value))
                    })
                    .transpose()?;
                let goal = crate::goal::update_goal(
                    id,
                    params
                        .scope
                        .as_deref()
                        .and_then(crate::goal::GoalScope::parse),
                    working_dir,
                    crate::goal::GoalUpdateInput {
                        title: params.title.clone(),
                        description: params.description.clone(),
                        why: params.why.clone(),
                        status,
                        success_criteria: params.success_criteria.clone(),
                        milestones: params.milestones.clone(),
                        next_steps: params.next_steps.clone(),
                        blockers: params.blockers.clone(),
                        current_milestone_id: if params.current_milestone_id.is_some() {
                            Some(params.current_milestone_id.clone())
                        } else {
                            None
                        },
                        progress_percent: if params.progress_percent.is_some() {
                            Some(params.progress_percent)
                        } else {
                            None
                        },
                        checkpoint_summary: if params.action == "checkpoint" {
                            params
                                .checkpoint_summary
                                .clone()
                                .or(params.description.clone())
                        } else {
                            params.checkpoint_summary.clone()
                        },
                    },
                )?
                .ok_or_else(|| anyhow::anyhow!("goal not found: {}", id))?;
                if display != crate::goal::GoalDisplayMode::None {
                    let should_write_goal_page = match display {
                        crate::goal::GoalDisplayMode::None => false,
                        crate::goal::GoalDisplayMode::UpdateOnly => {
                            goal_page_is_open(&ctx.session_id, &goal.id)?
                        }
                        crate::goal::GoalDisplayMode::Auto
                        | crate::goal::GoalDisplayMode::Focus => true,
                    };
                    if should_write_goal_page {
                        let snapshot = crate::goal::write_goal_page(
                            &ctx.session_id,
                            working_dir,
                            &goal,
                            display,
                        )?;
                        publish_side_panel_snapshot(&ctx.session_id, &snapshot);
                    }
                    maybe_publish_goals_overview_refresh(&ctx.session_id, working_dir)?;
                }
                Ok(
                    ToolOutput::new(format!("Updated goal `{}` ({})", goal.id, goal.title))
                        .with_title(goal.title.clone())
                        .with_metadata(serde_json::to_value(&goal)?),
                )
            }
            other => anyhow::bail!("unknown goal action: {}", other),
        }
    }
}

#[cfg(test)]
mod schema_tests {
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

        let snapshot = crate::side_panel::snapshot_for_session("ses_goal_update")
            .expect("side panel snapshot");
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
        assert_eq!(milestone_items["required"], json!(["id", "title"]));
        assert_eq!(milestone_items["properties"]["steps"]["type"], "array");
        assert_eq!(
            milestone_items["properties"]["steps"]["items"]["required"],
            json!(["id", "content"])
        );
    }

    #[test]
    fn test_goal_schema_omits_display_override() {
        let schema = GoalTool::new().parameters_schema();
        assert!(schema["properties"]["display"].is_null());
    }
}
