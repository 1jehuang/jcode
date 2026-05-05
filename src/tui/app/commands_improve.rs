use super::commands::active_session_id;
use super::commands_review::{ImproveCommand, RefactorCommand};
use super::{App, DisplayMessage, ImproveMode, ProcessingStatus};
use crate::message::{ContentBlock, Message, Role};
use std::time::Instant;

pub(super) fn improve_usage() -> &'static str {
    "Usage: `/improve [focus]`, `/improve plan [focus]`, `/improve resume`, `/improve status`, or `/improve stop`"
}

pub(super) fn parse_improve_command(trimmed: &str) -> Option<Result<ImproveCommand, String>> {
    let rest = trimmed.strip_prefix("/improve")?.trim();
    if rest.is_empty() {
        return Some(Ok(ImproveCommand::Run {
            plan_only: false,
            focus: None,
        }));
    }

    if rest == "status" {
        return Some(Ok(ImproveCommand::Status));
    }

    if rest == "resume" {
        return Some(Ok(ImproveCommand::Resume));
    }

    if rest == "stop" {
        return Some(Ok(ImproveCommand::Stop));
    }

    if rest == "plan" {
        return Some(Ok(ImproveCommand::Run {
            plan_only: true,
            focus: None,
        }));
    }

    if let Some(focus) = rest.strip_prefix("plan ") {
        let focus = focus.trim();
        return Some(if focus.is_empty() {
            Err(improve_usage().to_string())
        } else {
            Ok(ImproveCommand::Run {
                plan_only: true,
                focus: Some(focus.to_string()),
            })
        });
    }

    if rest.starts_with("status ") || rest.starts_with("resume ") || rest.starts_with("stop ") {
        return Some(Err(improve_usage().to_string()));
    }

    Some(Ok(ImproveCommand::Run {
        plan_only: false,
        focus: Some(rest.to_string()),
    }))
}

pub(super) fn refactor_usage() -> &'static str {
    "Usage: `/refactor [focus]`, `/refactor plan [focus]`, `/refactor resume`, `/refactor status`, or `/refactor stop`"
}

pub(super) fn parse_refactor_command(trimmed: &str) -> Option<Result<RefactorCommand, String>> {
    let rest = trimmed.strip_prefix("/refactor")?.trim();
    if rest.is_empty() {
        return Some(Ok(RefactorCommand::Run {
            plan_only: false,
            focus: None,
        }));
    }

    if rest == "status" {
        return Some(Ok(RefactorCommand::Status));
    }

    if rest == "resume" {
        return Some(Ok(RefactorCommand::Resume));
    }

    if rest == "stop" {
        return Some(Ok(RefactorCommand::Stop));
    }

    if rest == "plan" {
        return Some(Ok(RefactorCommand::Run {
            plan_only: true,
            focus: None,
        }));
    }

    if let Some(focus) = rest.strip_prefix("plan ") {
        let focus = focus.trim();
        return Some(if focus.is_empty() {
            Err(refactor_usage().to_string())
        } else {
            Ok(RefactorCommand::Run {
                plan_only: true,
                focus: Some(focus.to_string()),
            })
        });
    }

    if rest.starts_with("status ") || rest.starts_with("resume ") || rest.starts_with("stop ") {
        return Some(Err(refactor_usage().to_string()));
    }

    Some(Ok(RefactorCommand::Run {
        plan_only: false,
        focus: Some(rest.to_string()),
    }))
}

pub(super) fn build_improve_prompt(plan_only: bool, focus: Option<&str>) -> String {
    let focus_line = focus
        .map(|focus| {
            format!(
                "\nFocus area: {}. Prefer this area when leverage is comparable, but you may choose a different task if it is clearly higher leverage.",
                focus.trim()
            )
        })
        .unwrap_or_default();

    if plan_only {
        format!(
            "You are entering improvement planning mode for this repository.\n\
Your job is to inspect the project and identify the highest-leverage improvements worth doing next.\n\
\n\
First inspect the codebase and current repo state. Then write a concise ranked todo list using `todo` with the best 3-7 candidate improvements. Prefer work that is high-impact, low-risk, and easy to validate. Consider refactors, reliability issues, missing tests, UX papercuts, docs gaps, startup/runtime performance, and profiling opportunities.\n\
\n\
This is plan-only mode: do not edit files, write patches, or otherwise modify source code or git state. Read/search/analyze freely, and you may run builds/tests/profiling commands if that helps you rank the work, but stop after presenting the todo list and brief rationale.\n\
\n\
Avoid broad speculative rewrites, cosmetic churn, and busywork. If the repo already has todos, replace them with a tighter ranked improve plan if appropriate.{}",
            focus_line,
        )
    } else {
        format!(
            "You are entering improvement mode for this repository.\n\
Your job is to identify and implement the highest-leverage safe improvements to this project, then reassess and continue only while further work is clearly worthwhile.\n\
\n\
First inspect the codebase and current repo state. Then write a concise ranked todo list using `todo` with the best 3-7 improvements to tackle next. Prefer work that is high-impact, low-risk, locally scoped, and easy to validate. Consider refactors, reliability issues, missing tests, UX papercuts, docs gaps, startup/runtime performance, and profiling opportunities.{}\n\
\n\
Execute the strongest items, updating the todo list as you go. Validate meaningful changes with builds, tests, or measurements. If you make performance claims, measure before and after when possible.\n\
\n\
After completing the batch, reassess. If strong opportunities remain, write a fresh todo list and continue. If remaining work has diminishing returns, stop and explain why the next ideas are not clearly worth the churn.\n\
\n\
Avoid broad speculative rewrites, cosmetic churn, and busywork. Do not invent work just to stay busy. If the repo already has todos, refine or replace them with the best current improve batch before continuing.",
            focus_line,
        )
    }
}

pub(super) fn build_refactor_prompt(plan_only: bool, focus: Option<&str>) -> String {
    let focus_line = focus
        .map(|focus| {
            format!(
                "\nFocus area: {}. Prefer this area when leverage is comparable, but choose a different task if it is clearly higher leverage.",
                focus.trim()
            )
        })
        .unwrap_or_default();

    if plan_only {
        format!(
            "You are entering refactor planning mode for this repository.\n\
Your job is to inspect the project and identify the highest-leverage safe refactors worth doing next.\n\
\n\
First inspect the codebase, current repo state, and the in-repo quality docs if they exist, especially `docs/REFACTORING.md`, `docs/CODE_QUALITY_10_10_PLAN.md`, and `docs/CODE_QUALITY_TODO.md`. Then write a concise ranked todo list using `todo` with the best 3-7 candidate refactors. Prefer behavior-preserving extraction, file splits, dead-code deletion, warning reduction, test isolation, and clearer module boundaries.\n\
\n\
This is plan-only mode: do not edit files, write patches, or otherwise modify source code or git state. Read/search/analyze freely, and you may run builds/tests if that helps rank the work, but stop after presenting the ranked refactor plan and brief rationale.\n\
\n\
Avoid broad speculative rewrites, cosmetic churn, and risky busywork. If the repo already has todos, tighten or replace them with the best current refactor plan.{}",
            focus_line,
        )
    } else {
        format!(
            "You are entering refactor mode for this repository.\n\
Your job is to move the codebase closer to a practical 10/10 by making the highest-leverage safe refactors, validating them, getting an independent review, and only continuing while the next batch is clearly worth the churn.\n\
\n\
First inspect the codebase, current repo state, and the in-repo quality docs if they exist, especially `docs/REFACTORING.md`, `docs/CODE_QUALITY_10_10_PLAN.md`, and `docs/CODE_QUALITY_TODO.md`. Then write a concise ranked todo list using `todo` with the best 3-7 refactors to tackle next. Prefer behavior-preserving extraction, splitting oversized modules, dead-code deletion, warning reduction, test improvements, and boundary clarification.{}\n\
\n\
For v1, do the implementation work yourself in this main session. Do not create a swarm for ordinary execution. Keep changes locally scoped and easy to validate.\n\
\n\
After each meaningful batch, use the `subagent` tool exactly once to launch an independent read-only reviewer. In that subagent prompt, explicitly forbid file edits, patch application, and git changes. Ask it to inspect the changed areas plus nearby tests and report concrete regressions, risks, abstraction problems, or follow-up refactors. Incorporate valid findings before continuing.\n\
\n\
Validate each meaningful batch with relevant builds, tests, or repo verification scripts. Prefer behavior-preserving changes first. After the batch and independent review, reassess. If strong refactors remain, write a fresh todo list and continue. If remaining work has diminishing returns or becomes too risky, stop and explain why.\n\
\n\
Avoid broad speculative rewrites, cosmetic churn, and busywork. Do not invent work just to stay busy.",
            focus_line,
        )
    }
}

pub(super) fn improve_mode_for(plan_only: bool) -> ImproveMode {
    if plan_only {
        ImproveMode::ImprovePlan
    } else {
        ImproveMode::ImproveRun
    }
}

pub(super) fn refactor_mode_for(plan_only: bool) -> ImproveMode {
    if plan_only {
        ImproveMode::RefactorPlan
    } else {
        ImproveMode::RefactorRun
    }
}

pub(super) fn session_improve_mode_for(mode: ImproveMode) -> crate::session::SessionImproveMode {
    match mode {
        ImproveMode::ImproveRun => crate::session::SessionImproveMode::ImproveRun,
        ImproveMode::ImprovePlan => crate::session::SessionImproveMode::ImprovePlan,
        ImproveMode::RefactorRun => crate::session::SessionImproveMode::RefactorRun,
        ImproveMode::RefactorPlan => crate::session::SessionImproveMode::RefactorPlan,
    }
}

pub(super) fn restore_improve_mode(mode: crate::session::SessionImproveMode) -> ImproveMode {
    match mode {
        crate::session::SessionImproveMode::ImproveRun => ImproveMode::ImproveRun,
        crate::session::SessionImproveMode::ImprovePlan => ImproveMode::ImprovePlan,
        crate::session::SessionImproveMode::RefactorRun => ImproveMode::RefactorRun,
        crate::session::SessionImproveMode::RefactorPlan => ImproveMode::RefactorPlan,
    }
}

pub(super) fn improve_launch_notice(
    plan_only: bool,
    focus: Option<&str>,
    interrupted: bool,
) -> String {
    let action = if plan_only {
        "improvement plan"
    } else {
        "improvement loop"
    };
    let prefix = if interrupted {
        "👉 Interrupting and starting"
    } else {
        "🚀 Starting"
    };
    match focus.map(str::trim).filter(|focus| !focus.is_empty()) {
        Some(focus) => format!("{} {} focused on **{}**...", prefix, action, focus),
        None => format!("{} {}...", prefix, action),
    }
}

pub(super) fn improve_stop_notice(interrupted: bool) -> String {
    if interrupted {
        "🛑 Interrupting and stopping the improve loop at the next safe point...".to_string()
    } else {
        "🛑 Stopping the improve loop after the next safe point...".to_string()
    }
}

pub(super) fn improve_stop_prompt() -> String {
    "Stop improvement mode after the current safe point. Do not start a new improve batch. Update the todo list so it accurately reflects what is completed, cancelled, or still pending, and then summarize what remains plus why you stopped.".to_string()
}

pub(super) fn refactor_launch_notice(
    plan_only: bool,
    focus: Option<&str>,
    interrupted: bool,
) -> String {
    let action = if plan_only {
        "refactor plan"
    } else {
        "refactor loop"
    };
    let prefix = if interrupted {
        "👉 Interrupting and starting"
    } else {
        "🚀 Starting"
    };
    match focus.map(str::trim).filter(|focus| !focus.is_empty()) {
        Some(focus) => format!("{} {} focused on **{}**...", prefix, action, focus),
        None => format!("{} {}...", prefix, action),
    }
}

pub(super) fn refactor_stop_notice(interrupted: bool) -> String {
    if interrupted {
        "🛑 Interrupting and stopping the refactor loop at the next safe point...".to_string()
    } else {
        "🛑 Stopping the refactor loop after the next safe point...".to_string()
    }
}

pub(super) fn refactor_stop_prompt() -> String {
    "Stop refactor mode after the current safe point. Do not start a new refactor batch. Update the todo list so it accurately reflects what is completed, cancelled, or still pending, note any remaining high-value refactors, and summarize why you stopped. If you finished a meaningful code batch without yet running the independent read-only review subagent, run that review before stopping.".to_string()
}

pub(super) fn build_improve_resume_prompt(
    mode: ImproveMode,
    incomplete: &[&crate::todo::TodoItem],
) -> String {
    if incomplete.is_empty() {
        return match mode {
            ImproveMode::ImproveRun => "Resume improvement mode for this repository. Start by inspecting the current repo state, writing or refreshing a ranked todo list with `todo`, then continue implementing the highest-leverage safe improvements until the next ideas have diminishing returns.".to_string(),
            ImproveMode::ImprovePlan => "Resume improvement planning mode for this repository. Reinspect the current repo state, refresh the ranked improve todo list with `todo`, and stop after presenting the updated plan without editing files.".to_string(),
            ImproveMode::RefactorRun | ImproveMode::RefactorPlan => {
                "Resume improvement mode for this repository by first writing an improve-oriented todo list with `todo`, then continue only with high-leverage safe improvements.".to_string()
            }
        };
    }

    let mut todo_list = String::new();
    for todo in incomplete {
        let icon = if todo.status == "in_progress" {
            "🔄"
        } else {
            "⬜"
        };
        todo_list.push_str(&format!(
            "  {} [{}] {}\n",
            icon, todo.priority, todo.content
        ));
    }

    match mode {
        ImproveMode::ImproveRun => format!(
            "Resume improvement mode. Your current improve todo list still has {} incomplete item{}:\n\n{}\nContinue the highest-leverage work, keep the todo list accurate with `todo`, validate meaningful changes, and once this batch is done reassess whether another batch is still worth doing.",
            incomplete.len(),
            if incomplete.len() == 1 { "" } else { "s" },
            todo_list,
        ),
        ImproveMode::ImprovePlan => format!(
            "Resume improvement planning mode. The current improve todo list has {} pending item{}:\n\n{}\nRefresh or tighten this plan using `todo`, keeping it ranked and concrete, then stop without editing files.",
            incomplete.len(),
            if incomplete.len() == 1 { "" } else { "s" },
            todo_list,
        ),
        ImproveMode::RefactorRun | ImproveMode::RefactorPlan => format!(
            "Resume improvement mode with these incomplete items:\n\n{}\nContinue only the highest-leverage safe improvements and keep the todo list accurate with `todo`.",
            todo_list,
        ),
    }
}

pub(super) fn build_refactor_resume_prompt(
    mode: ImproveMode,
    incomplete: &[&crate::todo::TodoItem],
) -> String {
    if incomplete.is_empty() {
        return match mode {
            ImproveMode::RefactorRun => "Resume refactor mode for this repository. Start by inspecting the current repo state and relevant quality docs, write or refresh a ranked refactor todo list with `todo`, implement the highest-leverage safe refactors yourself, validate them, run an independent read-only review subagent after each meaningful batch, and continue only while more work is clearly worth the churn.".to_string(),
            ImproveMode::RefactorPlan => "Resume refactor planning mode for this repository. Reinspect the current repo state and quality docs, refresh the ranked refactor todo list with `todo`, and stop after presenting the updated plan without editing files.".to_string(),
            ImproveMode::ImproveRun | ImproveMode::ImprovePlan => {
                "Resume refactor mode for this repository by first producing a ranked refactor todo list with `todo`, then continue only with high-leverage safe refactors.".to_string()
            }
        };
    }

    let mut todo_list = String::new();
    for todo in incomplete {
        let icon = if todo.status == "in_progress" {
            "🔄"
        } else {
            "⬜"
        };
        todo_list.push_str(&format!(
            "  {} [{}] {}\n",
            icon, todo.priority, todo.content
        ));
    }

    match mode {
        ImproveMode::RefactorRun => format!(
            "Resume refactor mode. Your current refactor todo list still has {} incomplete item{}:\n\n{}\nContinue the highest-leverage safe refactors yourself in this session, keep the todo list accurate with `todo`, validate meaningful changes, run one independent read-only review subagent after each meaningful batch, and then reassess whether another batch is still worth doing.",
            incomplete.len(),
            if incomplete.len() == 1 { "" } else { "s" },
            todo_list,
        ),
        ImproveMode::RefactorPlan => format!(
            "Resume refactor planning mode. The current refactor todo list has {} pending item{}:\n\n{}\nRefresh or tighten this plan using `todo`, keeping it ranked and concrete, then stop without editing files.",
            incomplete.len(),
            if incomplete.len() == 1 { "" } else { "s" },
            todo_list,
        ),
        ImproveMode::ImproveRun | ImproveMode::ImprovePlan => format!(
            "Resume refactor mode with these incomplete items:\n\n{}\nConvert them into the best current refactor batch, then continue only with high-leverage safe refactors.",
            todo_list,
        ),
    }
}

fn current_mode_for(app: &App, predicate: impl Fn(ImproveMode) -> bool) -> Option<ImproveMode> {
    app.improve_mode
        .or_else(|| app.session.improve_mode.map(restore_improve_mode))
        .filter(|mode| predicate(*mode))
}

fn persist_improve_mode_local(app: &mut App, mode: Option<ImproveMode>) {
    app.improve_mode = mode;
    app.session.improve_mode = mode.map(session_improve_mode_for);
    if let Err(e) = app.session.save() {
        crate::logging::warn(&format!("Failed to save session: {}", e));
    }
}

fn start_synthetic_user_turn(app: &mut App, content: String) {
    app.commit_pending_streaming_assistant_message();
    app.add_provider_message(Message::user(&content));
    app.session.add_message(
        Role::User,
        vec![ContentBlock::Text {
            text: content,
            cache_control: None,
        }],
    );
    if let Err(e) = app.session.save() {
        crate::logging::warn(&format!("Failed to save session: {}", e));
    }

    app.is_processing = true;
    app.status = ProcessingStatus::Sending;
    app.clear_streaming_render_state();
    app.stream_buffer.clear();
    app.thought_line_inserted = false;
    app.thinking_prefix_emitted = false;
    app.thinking_buffer.clear();
    app.streaming_tool_calls.clear();
    app.batch_progress = None;
    app.streaming_input_tokens = 0;
    app.streaming_output_tokens = 0;
    app.streaming_cache_read_tokens = None;
    app.streaming_cache_creation_tokens = None;
    app.upstream_provider = None;
    app.status_detail = None;
    app.streaming_tps_start = None;
    app.streaming_tps_elapsed = std::time::Duration::ZERO;
    app.streaming_tps_collect_output = false;
    app.streaming_total_output_tokens = 0;
    app.streaming_tps_observed_output_tokens = 0;
    app.streaming_tps_observed_elapsed = std::time::Duration::ZERO;
    app.processing_started = Some(Instant::now());
    app.visible_turn_started = Some(Instant::now());
    app.pending_turn = true;
}

fn interrupt_and_queue_synthetic_message(
    app: &mut App,
    content: String,
    status_notice: &str,
    display_notice: String,
) {
    app.cancel_requested = true;
    app.interleave_message = None;
    app.pending_soft_interrupts.clear();
    app.pending_soft_interrupt_requests.clear();
    app.set_status_notice(status_notice);
    app.push_display_message(DisplayMessage::system(display_notice));
    app.queued_messages.push(content);
}

pub(super) fn format_improve_status(app: &App) -> String {
    let session_id = active_session_id(app);
    let todos = crate::todo::load_todos(&session_id).unwrap_or_default();
    let completed = todos.iter().filter(|t| t.status == "completed").count();
    let cancelled = todos.iter().filter(|t| t.status == "cancelled").count();
    let incomplete: Vec<_> = todos
        .iter()
        .filter(|t| t.status != "completed" && t.status != "cancelled")
        .collect();

    let phase = if app.is_processing {
        if current_mode_for(app, ImproveMode::is_improve).is_some() || !incomplete.is_empty() {
            "running"
        } else {
            "busy (no improve batch detected yet)"
        }
    } else if !incomplete.is_empty() {
        "paused / resumable"
    } else if completed > 0 || cancelled > 0 {
        "idle (last improve batch finished)"
    } else {
        "idle"
    };

    let mode = current_mode_for(app, ImproveMode::is_improve)
        .map(|mode| mode.status_label())
        .unwrap_or("not yet started in this session");

    let mut lines = vec![
        format!("Improve status: **{}**", phase),
        format!("Last requested mode: **{}**", mode),
        format!(
            "Todos: {} incomplete · {} completed · {} cancelled",
            incomplete.len(),
            completed,
            cancelled
        ),
    ];

    if !incomplete.is_empty() {
        lines.push(String::new());
        lines.push("Current improve batch:".to_string());
        for todo in incomplete.iter().take(5) {
            let icon = if todo.status == "in_progress" {
                "🔄"
            } else {
                "⬜"
            };
            lines.push(format!("- {} [{}] {}", icon, todo.priority, todo.content));
        }
        if incomplete.len() > 5 {
            lines.push(format!("- …and {} more", incomplete.len() - 5));
        }
    } else {
        lines.push(String::new());
        lines.push("No current improve todo batch for this session.".to_string());
    }

    lines.push(String::new());
    lines.push("Use `/improve` to start/continue, `/improve resume` to continue the last saved mode, `/improve plan` for plan-only mode, or `/improve stop` to halt after a safe point.".to_string());
    lines.join("\n")
}

pub(super) fn format_refactor_status(app: &App) -> String {
    let session_id = active_session_id(app);
    let todos = crate::todo::load_todos(&session_id).unwrap_or_default();
    let completed = todos.iter().filter(|t| t.status == "completed").count();
    let cancelled = todos.iter().filter(|t| t.status == "cancelled").count();
    let incomplete: Vec<_> = todos
        .iter()
        .filter(|t| t.status != "completed" && t.status != "cancelled")
        .collect();

    let phase = if app.is_processing {
        if current_mode_for(app, ImproveMode::is_refactor).is_some() || !incomplete.is_empty() {
            "running"
        } else {
            "busy (no refactor batch detected yet)"
        }
    } else if !incomplete.is_empty() {
        "paused / resumable"
    } else if completed > 0 || cancelled > 0 {
        "idle (last refactor batch finished)"
    } else {
        "idle"
    };

    let mode = current_mode_for(app, ImproveMode::is_refactor)
        .map(|mode| mode.status_label())
        .unwrap_or("not yet started in this session");

    let mut lines = vec![
        format!("Refactor status: **{}**", phase),
        format!("Last requested mode: **{}**", mode),
        format!(
            "Todos: {} incomplete · {} completed · {} cancelled",
            incomplete.len(),
            completed,
            cancelled
        ),
    ];

    if !incomplete.is_empty() {
        lines.push(String::new());
        lines.push("Current refactor batch:".to_string());
        for todo in incomplete.iter().take(5) {
            let icon = if todo.status == "in_progress" {
                "🔄"
            } else {
                "⬜"
            };
            lines.push(format!("- {} [{}] {}", icon, todo.priority, todo.content));
        }
        if incomplete.len() > 5 {
            lines.push(format!("- …and {} more", incomplete.len() - 5));
        }
    } else {
        lines.push(String::new());
        lines.push("No current refactor todo batch for this session.".to_string());
    }

    lines.push(String::new());
    lines.push("Use `/refactor` to start/continue, `/refactor resume` to continue the last saved mode, `/refactor plan` for plan-only mode, or `/refactor stop` to halt after a safe point.".to_string());
    lines.join("\n")
}

pub(super) fn handle_improve_command_local(app: &mut App, command: ImproveCommand) {
    match command {
        ImproveCommand::Resume => {
            let session_id = active_session_id(app);
            let todos = crate::todo::load_todos(&session_id).unwrap_or_default();
            let incomplete: Vec<_> = todos
                .iter()
                .filter(|todo| todo.status != "completed" && todo.status != "cancelled")
                .collect();

            let mode = current_mode_for(app, ImproveMode::is_improve);
            let Some(mode) = mode else {
                app.push_display_message(DisplayMessage::system(
                    "No saved improve run found for this session. Use `/improve` or `/improve plan` to start one."
                        .to_string(),
                ));
                return;
            };

            persist_improve_mode_local(app, Some(mode));
            let prompt = build_improve_resume_prompt(mode, &incomplete);
            if app.is_processing {
                interrupt_and_queue_synthetic_message(
                    app,
                    prompt,
                    "Interrupting for /improve resume...",
                    improve_launch_notice(matches!(mode, ImproveMode::ImprovePlan), None, true),
                );
            } else {
                app.push_display_message(DisplayMessage::system(format!(
                    "♻️ Resuming {}...",
                    mode.status_label()
                )));
                start_synthetic_user_turn(app, prompt);
            }
        }
        ImproveCommand::Status => {
            app.push_display_message(DisplayMessage::system(format_improve_status(app)));
        }
        ImproveCommand::Stop => {
            let session_id = active_session_id(app);
            let todos = crate::todo::load_todos(&session_id).unwrap_or_default();
            let has_incomplete = todos
                .iter()
                .any(|todo| todo.status != "completed" && todo.status != "cancelled");

            if current_mode_for(app, ImproveMode::is_improve).is_none()
                && !app.is_processing
                && !has_incomplete
            {
                app.push_display_message(DisplayMessage::system(
                    "No active improve loop to stop. Use `/improve` to start one.".to_string(),
                ));
                return;
            }

            persist_improve_mode_local(app, None);
            let stop_prompt = improve_stop_prompt();
            if app.is_processing {
                interrupt_and_queue_synthetic_message(
                    app,
                    stop_prompt,
                    "Interrupting for /improve stop...",
                    improve_stop_notice(true),
                );
            } else {
                app.push_display_message(DisplayMessage::system(improve_stop_notice(false)));
                start_synthetic_user_turn(app, stop_prompt);
            }
        }
        ImproveCommand::Run { plan_only, focus } => {
            let mode = improve_mode_for(plan_only);
            persist_improve_mode_local(app, Some(mode));
            let prompt = build_improve_prompt(plan_only, focus.as_deref());
            if app.is_processing {
                interrupt_and_queue_synthetic_message(
                    app,
                    prompt,
                    if plan_only {
                        "Interrupting for /improve plan..."
                    } else {
                        "Interrupting for /improve..."
                    },
                    improve_launch_notice(plan_only, focus.as_deref(), true),
                );
            } else {
                app.push_display_message(DisplayMessage::system(improve_launch_notice(
                    plan_only,
                    focus.as_deref(),
                    false,
                )));
                start_synthetic_user_turn(app, prompt);
            }
        }
    }
}

pub(super) fn handle_refactor_command_local(app: &mut App, command: RefactorCommand) {
    match command {
        RefactorCommand::Resume => {
            let session_id = active_session_id(app);
            let todos = crate::todo::load_todos(&session_id).unwrap_or_default();
            let incomplete: Vec<_> = todos
                .iter()
                .filter(|todo| todo.status != "completed" && todo.status != "cancelled")
                .collect();

            let mode = current_mode_for(app, ImproveMode::is_refactor);
            let Some(mode) = mode else {
                app.push_display_message(DisplayMessage::system(
                    "No saved refactor run found for this session. Use `/refactor` or `/refactor plan` to start one."
                        .to_string(),
                ));
                return;
            };

            persist_improve_mode_local(app, Some(mode));
            let prompt = build_refactor_resume_prompt(mode, &incomplete);
            if app.is_processing {
                interrupt_and_queue_synthetic_message(
                    app,
                    prompt,
                    "Interrupting for /refactor resume...",
                    refactor_launch_notice(matches!(mode, ImproveMode::RefactorPlan), None, true),
                );
            } else {
                app.push_display_message(DisplayMessage::system(format!(
                    "♻️ Resuming {}...",
                    mode.status_label()
                )));
                start_synthetic_user_turn(app, prompt);
            }
        }
        RefactorCommand::Status => {
            app.push_display_message(DisplayMessage::system(format_refactor_status(app)));
        }
        RefactorCommand::Stop => {
            let session_id = active_session_id(app);
            let todos = crate::todo::load_todos(&session_id).unwrap_or_default();
            let has_incomplete = todos
                .iter()
                .any(|todo| todo.status != "completed" && todo.status != "cancelled");

            if current_mode_for(app, ImproveMode::is_refactor).is_none()
                && !app.is_processing
                && !has_incomplete
            {
                app.push_display_message(DisplayMessage::system(
                    "No active refactor loop to stop. Use `/refactor` to start one.".to_string(),
                ));
                return;
            }

            persist_improve_mode_local(app, None);
            let stop_prompt = refactor_stop_prompt();
            if app.is_processing {
                interrupt_and_queue_synthetic_message(
                    app,
                    stop_prompt,
                    "Interrupting for /refactor stop...",
                    refactor_stop_notice(true),
                );
            } else {
                app.push_display_message(DisplayMessage::system(refactor_stop_notice(false)));
                start_synthetic_user_turn(app, stop_prompt);
            }
        }
        RefactorCommand::Run { plan_only, focus } => {
            let mode = refactor_mode_for(plan_only);
            persist_improve_mode_local(app, Some(mode));
            let prompt = build_refactor_prompt(plan_only, focus.as_deref());
            if app.is_processing {
                interrupt_and_queue_synthetic_message(
                    app,
                    prompt,
                    if plan_only {
                        "Interrupting for /refactor plan..."
                    } else {
                        "Interrupting for /refactor..."
                    },
                    refactor_launch_notice(plan_only, focus.as_deref(), true),
                );
            } else {
                app.push_display_message(DisplayMessage::system(refactor_launch_notice(
                    plan_only,
                    focus.as_deref(),
                    false,
                )));
                start_synthetic_user_turn(app, prompt);
            }
        }
    }
}
