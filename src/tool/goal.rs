#![cfg_attr(test, allow(clippy::await_holding_lock))]

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
        "additionalProperties": true
    })
}

fn goal_milestone_schema() -> Value {
    json!({
        "type": "object",
        "properties": {
            "steps": {
                "type": "array",
                "items": goal_step_schema()
            }
        },
        "additionalProperties": true
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
                "intent": super::intent_schema_property(),
                "action": {
                    "type": "string",
                    "enum": ["create", "list", "show", "resume", "update", "checkpoint", "focus"],
                    "description": "Action."
                },
                "id": {"type": "string"},
                "title": {"type": "string"},
                "scope": {"type": "string"},
                "status": {"type": "string"},
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
        let action_label = params.action.clone();
        let goal_id_label = params.id.clone().unwrap_or_else(|| "<none>".to_string());
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
        .map_err(|err| {
            crate::logging::warn(&format!(
                "[tool:goal] action failed action={} goal_id={} session_id={} error={}",
                action_label, goal_id_label, ctx.session_id, err
            ));
            err
        })
    }
}

#[cfg(test)]
#[path = "goal_tests.rs"]
mod goal_tests;
