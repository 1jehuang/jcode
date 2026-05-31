use super::commands_improve::{interrupt_and_queue_synthetic_message, start_synthetic_user_turn};
use super::{App, DisplayMessage};

/// A parsed `/plan` command. Planning is a one-shot, plan-only action: it never
/// edits files and is not a resumable loop like `/improve` or `/refactor`.
#[derive(Clone, Debug, Eq, PartialEq)]
pub(super) struct PlanCommand {
    pub goal: Option<String>,
}

pub(super) fn parse_plan_command(trimmed: &str) -> Option<PlanCommand> {
    let rest = trimmed.strip_prefix("/plan")?;
    // Only treat `/plan` and `/plan <goal>` as a plan command, not `/planfoo`.
    if !rest.is_empty() && !rest.starts_with(char::is_whitespace) {
        return None;
    }
    let goal = rest.trim();
    Some(PlanCommand {
        goal: if goal.is_empty() {
            None
        } else {
            Some(goal.to_string())
        },
    })
}

pub(super) fn build_plan_prompt(goal: Option<&str>) -> String {
    let goal_line = match goal.map(str::trim).filter(|goal| !goal.is_empty()) {
        Some(goal) => format!("Goal: {}\n\n", goal),
        None => "Goal: produce a plan for the task or request currently in focus in this session. If the goal is ambiguous, infer the most useful interpretation from the recent conversation and repo state, and state your assumption.\n\n".to_string(),
    };

    format!(
        "You are entering planning mode.\n\
\n\
{}\
Your job is to produce a clear, concrete, actionable plan. Do NOT implement anything yet: do not edit files, write patches, or change git state. You may freely read, search, run read-only commands, and analyze the codebase so the plan is grounded in how things actually work.\n\
\n\
When the plan is ready, write it to the side panel using the `side_panel` tool so the user can review it. Use a single focused page titled \"Plan\" (overwrite it if it already exists). Structure the plan with these sections: Goal, Scope / affected areas, Approach (concrete ordered steps), Validation (how each part will be verified), and Open questions / decisions.\n\
\n\
Keep it tight and high-signal. Avoid speculative rewrites and busywork. After writing the plan to the side panel, stop and wait for the user. Do not start implementing.\n\
\n\
Only once the user approves, use the `todo` tool to turn the plan into an executable todo list and then begin the work.",
        goal_line,
    )
}

pub(super) fn plan_launch_notice(goal: Option<&str>, interrupted: bool) -> String {
    let prefix = if interrupted {
        "👉 Interrupting and planning"
    } else {
        "🧭 Planning"
    };
    match goal.map(str::trim).filter(|goal| !goal.is_empty()) {
        Some(goal) => format!(
            "{} {}... (plan-only; results go to the side panel)",
            prefix, goal
        ),
        None => format!("{}... (plan-only; results go to the side panel)", prefix),
    }
}

pub(super) fn handle_plan_command_local(app: &mut App, command: PlanCommand) {
    let prompt = build_plan_prompt(command.goal.as_deref());
    if app.is_processing {
        interrupt_and_queue_synthetic_message(
            app,
            prompt,
            "Interrupting for /plan...",
            plan_launch_notice(command.goal.as_deref(), true),
        );
    } else {
        app.push_display_message(DisplayMessage::system(plan_launch_notice(
            command.goal.as_deref(),
            false,
        )));
        start_synthetic_user_turn(app, prompt);
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn parse_plan_accepts_bare_and_goal_forms() {
        assert_eq!(
            parse_plan_command("/plan"),
            Some(PlanCommand { goal: None })
        );
        assert_eq!(
            parse_plan_command("/plan   "),
            Some(PlanCommand { goal: None })
        );
        assert_eq!(
            parse_plan_command("/plan add a compact mode"),
            Some(PlanCommand {
                goal: Some("add a compact mode".to_string())
            })
        );
    }

    #[test]
    fn parse_plan_rejects_other_commands() {
        assert_eq!(parse_plan_command("/planner foo"), None);
        assert_eq!(parse_plan_command("/improve"), None);
        assert_eq!(parse_plan_command("plan ahead"), None);
    }

    #[test]
    fn build_plan_prompt_is_plan_only_and_targets_side_panel() {
        let prompt = build_plan_prompt(Some("ship feature x"));
        assert!(prompt.contains("Goal: ship feature x"));
        assert!(prompt.contains("Do NOT implement anything yet"));
        assert!(prompt.contains("`side_panel`"));
        assert!(prompt.contains("`todo`"));

        let bare = build_plan_prompt(None);
        assert!(bare.contains("currently in focus in this session"));
    }
}
