//! `/goal` command family.
//!
//! Re-enables the previously disabled `/goal` command as a local-only
//! dispatcher over the durable goal/initiative storage:
//!
//! ```text
//! /goal <title>                  Create a new active goal
//! /goal status                   Show all active goals
//! /goal update <id> <message>    Add progress update
//! /goal complete <id>            Mark goal as completed
//! /goal pause <id>               Pause a goal
//! /goal cancel <id>              Abandon a goal
//! /goal resume                   Resume most recent goal
//! /goal <id>                     Open goal detail in side panel
//! ```

use super::{App, DisplayMessage};
use crate::goal::{
    create_goal, list_relevant_goals, open_goal_for_session, open_goals_overview_for_session,
    resume_goal_for_session, update_goal, GoalCreateInput, GoalScope, GoalStatus, GoalUpdateInput,
};

/// Dispatch `/goal ...`. Returns `false` when `trimmed` is not a `/goal`
/// command so callers can fall through to other handlers.
pub(super) fn handle_goal_command(app: &mut App, trimmed: &str) -> bool {
    let Some(rest) = trimmed.strip_prefix("/goal") else {
        return false;
    };
    // `/goals` (plural) and `/goals ...` are handled by `handle_goals_command`
    // in commands.rs; never swallow them here.
    if rest.starts_with('s') {
        return false;
    }
    let rest = rest.trim();

    if rest.is_empty() || matches!(rest, "help" | "--help" | "-h") {
        app.push_display_message(DisplayMessage::system(goal_usage()));
        return true;
    }
    if rest == "status" {
        return open_goals_overview(app);
    }
    if rest == "resume" {
        return resume_most_recent_goal(app);
    }
    if let Some(args) = rest.strip_prefix("update ") {
        return add_goal_update(app, args.trim());
    }
    if let Some(id) = rest.strip_prefix("complete ") {
        return set_goal_status(app, id.trim(), GoalStatus::Completed, "Completed");
    }
    if let Some(id) = rest.strip_prefix("pause ") {
        return set_goal_status(app, id.trim(), GoalStatus::Paused, "Paused");
    }
    if let Some(id) = rest.strip_prefix("cancel ") {
        return set_goal_status(app, id.trim(), GoalStatus::Abandoned, "Abandoned");
    }
    // `/goal <title>` creates a new goal; `/goal <id>` opens an existing one.
    create_or_open_goal(app, rest)
}

fn goal_usage() -> String {
    "/goal <title> | status | update <id> <message> | complete <id> | pause <id> | cancel <id> | resume | <id>"
        .to_string()
}

fn open_goals_overview(app: &mut App) -> bool {
    let session_id = super::commands::active_session_id(app);
    let working_dir = super::commands::active_working_dir(app);
    match open_goals_overview_for_session(session_id.as_str(), working_dir.as_deref(), true) {
        Ok(snapshot) => {
            app.set_side_panel_snapshot(snapshot);
            let count = list_relevant_goals(working_dir.as_deref())
                .map(|goals| goals.len())
                .unwrap_or(0);
            app.push_display_message(DisplayMessage::system(format!(
                "Opened goals overview in the side panel ({} active goal{}).",
                count,
                if count == 1 { "" } else { "s" }
            )));
            app.set_status_notice(format!("Goals: {}", count));
        }
        Err(e) => app.push_display_message(DisplayMessage::error(format!(
            "Failed to open goals overview: {}",
            e
        ))),
    }
    true
}

fn resume_most_recent_goal(app: &mut App) -> bool {
    let session_id = super::commands::active_session_id(app);
    let working_dir = super::commands::active_working_dir(app);
    match resume_goal_for_session(session_id.as_str(), working_dir.as_deref(), true) {
        Ok(Some(result)) => {
            app.set_side_panel_snapshot(result.snapshot);
            let mut msg = format!("Resumed goal {}.", result.goal.title);
            if let Some(next_step) = result.goal.next_steps.first() {
                msg.push_str(&format!(" Next step: {}", next_step));
            }
            app.push_display_message(DisplayMessage::system(msg));
            app.set_status_notice(format!("Goal: {}", result.goal.title));
        }
        Ok(None) => app.push_display_message(DisplayMessage::system(
            "No resumable goals found for this session.".to_string(),
        )),
        Err(e) => app.push_display_message(DisplayMessage::error(format!(
            "Failed to resume goal: {}",
            e
        ))),
    }
    true
}

fn add_goal_update(app: &mut App, args: &str) -> bool {
    let Some((id, message)) = args.split_once(' ') else {
        app.push_display_message(DisplayMessage::error(
            "Usage: /goal update <id> <message>".to_string(),
        ));
        return true;
    };
    let id = id.trim();
    let message = message.trim();
    if id.is_empty() || message.is_empty() {
        app.push_display_message(DisplayMessage::error(
            "Usage: /goal update <id> <message>".to_string(),
        ));
        return true;
    }
    let working_dir = super::commands::active_working_dir(app);
    match update_goal(
        id,
        None,
        working_dir.as_deref(),
        GoalUpdateInput {
            checkpoint_summary: Some(message.to_string()),
            ..Default::default()
        },
    ) {
        Ok(Some(goal)) => {
            app.push_display_message(DisplayMessage::system(format!(
                "Updated goal {}: {}",
                goal.title, message
            )));
            app.set_status_notice(format!("Goal: {}", goal.title));
        }
        Ok(None) => {
            app.push_display_message(DisplayMessage::error(format!("Goal not found: {}", id)))
        }
        Err(e) => app.push_display_message(DisplayMessage::error(format!(
            "Failed to update goal: {}",
            e
        ))),
    }
    true
}

fn set_goal_status(app: &mut App, id: &str, status: GoalStatus, label: &str) -> bool {
    if id.is_empty() {
        app.push_display_message(DisplayMessage::error(format!(
            "Usage: /goal {} <id>",
            status.as_str()
        )));
        return true;
    }
    let working_dir = super::commands::active_working_dir(app);
    match update_goal(
        id,
        None,
        working_dir.as_deref(),
        GoalUpdateInput {
            status: Some(status),
            ..Default::default()
        },
    ) {
        Ok(Some(goal)) => {
            app.push_display_message(DisplayMessage::system(format!(
                "{} goal {}.",
                label, goal.title
            )));
            app.set_status_notice(format!("Goal: {}", goal.title));
        }
        Ok(None) => {
            app.push_display_message(DisplayMessage::error(format!("Goal not found: {}", id)))
        }
        Err(e) => app.push_display_message(DisplayMessage::error(format!(
            "Failed to {} goal: {}",
            label.to_lowercase(),
            e
        ))),
    }
    true
}

fn create_or_open_goal(app: &mut App, arg: &str) -> bool {
    let session_id = super::commands::active_session_id(app);
    let working_dir = super::commands::active_working_dir(app);
    // If `arg` identifies an existing goal, open its detail in the side panel.
    if let Ok(Some(result)) =
        open_goal_for_session(session_id.as_str(), working_dir.as_deref(), arg, true)
    {
        app.set_side_panel_snapshot(result.snapshot);
        app.push_display_message(DisplayMessage::system(format!(
            "Opened goal {} in the side panel.",
            result.goal.title
        )));
        app.set_status_notice(format!("Goal: {}", result.goal.title));
        return true;
    }

    // Otherwise treat `arg` as the title of a new active goal.
    let goal = match create_goal(
        GoalCreateInput {
            title: arg.to_string(),
            scope: GoalScope::Project,
            ..Default::default()
        },
        working_dir.as_deref(),
    ) {
        Ok(goal) => goal,
        Err(e) => {
            app.push_display_message(DisplayMessage::error(format!(
                "Failed to create goal: {}",
                e
            )));
            return true;
        }
    };
    if let Ok(Some(result)) =
        open_goal_for_session(session_id.as_str(), working_dir.as_deref(), &goal.id, true)
    {
        app.set_side_panel_snapshot(result.snapshot);
    }
    app.push_display_message(DisplayMessage::system(format!(
        "Created goal {}.",
        goal.title
    )));
    app.set_status_notice(format!("Goal: {}", goal.title));
    true
}
