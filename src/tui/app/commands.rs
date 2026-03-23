use super::{App, DisplayMessage, ImproveMode, ProcessingStatus};
use crate::bus::{Bus, BusEvent, ManualToolCompleted, ToolEvent, ToolStatus};
use crate::id;
use crate::message::{ContentBlock, Message, Role};
use crate::session::Session;
use std::path::PathBuf;
use std::time::Instant;

pub(super) fn reset_current_session(app: &mut App) {
    app.clear_provider_messages();
    app.clear_display_messages();
    app.queued_messages.clear();
    app.pasted_contents.clear();
    app.pending_images.clear();
    app.active_skill = None;
    app.improve_mode = None;
    let mut session = Session::create(None, None);
    session.model = Some(app.provider.model());
    app.session = session;
    app.side_panel = crate::side_panel::SidePanelSnapshot::default();
    app.provider_session_id = None;
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub(super) struct ManualSubagentSpec {
    pub(super) subagent_type: String,
    pub(super) model: Option<String>,
    pub(super) session_id: Option<String>,
    pub(super) prompt: String,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub(super) enum ImproveCommand {
    Run {
        plan_only: bool,
        focus: Option<String>,
    },
    Resume,
    Status,
    Stop,
}

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
First inspect the codebase and current repo state. Then write a concise ranked todo list using `todowrite` with the best 3-7 candidate improvements. Prefer work that is high-impact, low-risk, and easy to validate. Consider refactors, reliability issues, missing tests, UX papercuts, docs gaps, startup/runtime performance, and profiling opportunities.\n\
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
First inspect the codebase and current repo state. Then write a concise ranked todo list using `todowrite` with the best 3-7 improvements to tackle next. Prefer work that is high-impact, low-risk, locally scoped, and easy to validate. Consider refactors, reliability issues, missing tests, UX papercuts, docs gaps, startup/runtime performance, and profiling opportunities.{}\n\
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

pub(super) fn improve_mode_for(plan_only: bool) -> ImproveMode {
    if plan_only {
        ImproveMode::Plan
    } else {
        ImproveMode::Run
    }
}

pub(super) fn session_improve_mode_for(mode: ImproveMode) -> crate::session::SessionImproveMode {
    match mode {
        ImproveMode::Run => crate::session::SessionImproveMode::Run,
        ImproveMode::Plan => crate::session::SessionImproveMode::Plan,
    }
}

pub(super) fn restore_improve_mode(mode: crate::session::SessionImproveMode) -> ImproveMode {
    match mode {
        crate::session::SessionImproveMode::Run => ImproveMode::Run,
        crate::session::SessionImproveMode::Plan => ImproveMode::Plan,
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

pub(super) fn build_improve_resume_prompt(
    mode: ImproveMode,
    incomplete: &[&crate::todo::TodoItem],
) -> String {
    if incomplete.is_empty() {
        return match mode {
            ImproveMode::Run => "Resume improvement mode for this repository. Start by inspecting the current repo state, writing or refreshing a ranked todo list with `todowrite`, then continue implementing the highest-leverage safe improvements until the next ideas have diminishing returns.".to_string(),
            ImproveMode::Plan => "Resume improvement planning mode for this repository. Reinspect the current repo state, refresh the ranked improve todo list with `todowrite`, and stop after presenting the updated plan without editing files.".to_string(),
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
        ImproveMode::Run => format!(
            "Resume improvement mode. Your current improve todo list still has {} incomplete item{}:\n\n{}\nContinue the highest-leverage work, keep the todo list accurate with `todowrite`, validate meaningful changes, and once this batch is done reassess whether another batch is still worth doing.",
            incomplete.len(),
            if incomplete.len() == 1 { "" } else { "s" },
            todo_list,
        ),
        ImproveMode::Plan => format!(
            "Resume improvement planning mode. The current improve todo list has {} pending item{}:\n\n{}\nRefresh or tighten this plan using `todowrite`, keeping it ranked and concrete, then stop without editing files.",
            incomplete.len(),
            if incomplete.len() == 1 { "" } else { "s" },
            todo_list,
        ),
    }
}

fn persist_improve_mode_local(app: &mut App, mode: Option<ImproveMode>) {
    app.improve_mode = mode;
    app.session.improve_mode = mode.map(session_improve_mode_for);
    let _ = app.session.save();
}

fn start_synthetic_user_turn(app: &mut App, content: String) {
    app.add_provider_message(Message::user(&content));
    app.session.add_message(
        Role::User,
        vec![ContentBlock::Text {
            text: content,
            cache_control: None,
        }],
    );
    let _ = app.session.save();

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
    app.streaming_tps_start = None;
    app.streaming_tps_elapsed = std::time::Duration::ZERO;
    app.streaming_total_output_tokens = 0;
    app.processing_started = Some(Instant::now());
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
        if app.improve_mode.is_some() || !incomplete.is_empty() {
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

    let mode = app
        .improve_mode
        .or_else(|| app.session.improve_mode.map(restore_improve_mode))
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

fn handle_improve_command_local(app: &mut App, command: ImproveCommand) {
    match command {
        ImproveCommand::Resume => {
            let session_id = active_session_id(app);
            let todos = crate::todo::load_todos(&session_id).unwrap_or_default();
            let incomplete: Vec<_> = todos
                .iter()
                .filter(|todo| todo.status != "completed" && todo.status != "cancelled")
                .collect();

            let mode = app
                .improve_mode
                .or_else(|| app.session.improve_mode.map(restore_improve_mode));
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
                    improve_launch_notice(matches!(mode, ImproveMode::Plan), None, true),
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

            if app.improve_mode.is_none() && !app.is_processing && !has_incomplete {
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

pub(super) fn current_subagent_model_summary(app: &App) -> String {
    match app.session.subagent_model.as_deref() {
        Some(model) => format!("fixed `{}`", model),
        None => format!("inherit current (`{}`)", app.provider.model()),
    }
}

fn derive_subagent_description(prompt: &str) -> String {
    let words: Vec<&str> = prompt.split_whitespace().take(4).collect();
    if words.is_empty() {
        "Manual subagent".to_string()
    } else {
        words.join(" ")
    }
}

pub(super) fn parse_manual_subagent_spec(rest: &str) -> Result<ManualSubagentSpec, String> {
    let mut iter = rest.split_whitespace().peekable();
    let mut subagent_type = "general".to_string();
    let mut model = None;
    let mut session_id = None;
    let mut prompt_tokens = Vec::new();

    while let Some(token) = iter.next() {
        match token {
            "--type" => {
                let value = iter
                    .next()
                    .ok_or_else(|| "Missing value for `--type`.".to_string())?;
                subagent_type = value.to_string();
            }
            "--model" => {
                let value = iter
                    .next()
                    .ok_or_else(|| "Missing value for `--model`.".to_string())?;
                model = Some(value.to_string());
            }
            "--continue" => {
                let value = iter
                    .next()
                    .ok_or_else(|| "Missing value for `--continue`.".to_string())?;
                session_id = Some(value.to_string());
            }
            flag if flag.starts_with("--") => {
                return Err(format!("Unknown flag `{}`.", flag));
            }
            prompt_start => {
                prompt_tokens.push(prompt_start.to_string());
                prompt_tokens.extend(iter.map(str::to_string));
                break;
            }
        }
    }

    let prompt = prompt_tokens.join(" ").trim().to_string();
    if prompt.is_empty() {
        return Err("Missing prompt. Add text after `/subagent`.".to_string());
    }

    Ok(ManualSubagentSpec {
        subagent_type,
        model,
        session_id,
        prompt,
    })
}

fn launch_manual_subagent(app: &mut App, spec: ManualSubagentSpec) {
    let description = derive_subagent_description(&spec.prompt);
    let tool_call = crate::message::ToolCall {
        id: id::new_id("call"),
        name: "subagent".to_string(),
        input: serde_json::json!({
            "description": description,
            "prompt": spec.prompt,
            "subagent_type": spec.subagent_type,
            "model": spec.model,
            "session_id": spec.session_id,
            "command": "/subagent",
        }),
        intent: None,
    };

    app.push_display_message(DisplayMessage {
        role: "tool".to_string(),
        content: tool_call.name.clone(),
        tool_calls: vec![],
        duration_secs: None,
        title: None,
        tool_data: Some(tool_call.clone()),
    });

    let content_blocks = vec![ContentBlock::ToolUse {
        id: tool_call.id.clone(),
        name: tool_call.name.clone(),
        input: tool_call.input.clone(),
    }];
    app.add_provider_message(Message {
        role: Role::Assistant,
        content: content_blocks.clone(),
        timestamp: Some(chrono::Utc::now()),
        tool_duration_ms: None,
    });
    let message_id = app.session.add_message(Role::Assistant, content_blocks);
    let _ = app.session.save();
    app.subagent_status = Some("starting subagent".to_string());
    app.set_status_notice("Running subagent");

    let registry = app.registry.clone();
    let session_id = app.session.id.clone();
    let working_dir = app.session.working_dir.clone();
    let tool_call_for_task = tool_call.clone();
    tokio::spawn(async move {
        Bus::global().publish(BusEvent::ToolUpdated(ToolEvent {
            session_id: session_id.clone(),
            message_id: message_id.clone(),
            tool_call_id: tool_call_for_task.id.clone(),
            tool_name: tool_call_for_task.name.clone(),
            status: ToolStatus::Running,
            title: None,
        }));

        let ctx = crate::tool::ToolContext {
            session_id: session_id.clone(),
            message_id: message_id.clone(),
            tool_call_id: tool_call_for_task.id.clone(),
            working_dir: working_dir.as_deref().map(PathBuf::from),
            stdin_request_tx: None,
            graceful_shutdown_signal: None,
            execution_mode: crate::tool::ToolExecutionMode::Direct,
        };

        let start = Instant::now();
        let result = registry
            .execute(
                &tool_call_for_task.name,
                tool_call_for_task.input.clone(),
                ctx,
            )
            .await;
        let duration_ms = start.elapsed().as_millis() as u64;

        let (output, is_error, title, status) = match result {
            Ok(output) => {
                crate::telemetry::record_tool_call();
                (output.output, false, output.title, ToolStatus::Completed)
            }
            Err(error) => {
                crate::telemetry::record_tool_failure();
                (format!("Error: {}", error), true, None, ToolStatus::Error)
            }
        };

        Bus::global().publish(BusEvent::ToolUpdated(ToolEvent {
            session_id: session_id.clone(),
            message_id,
            tool_call_id: tool_call_for_task.id.clone(),
            tool_name: tool_call_for_task.name.clone(),
            status,
            title: title.clone(),
        }));

        Bus::global().publish(BusEvent::ManualToolCompleted(ManualToolCompleted {
            session_id,
            tool_call: tool_call_for_task,
            output,
            is_error,
            title,
            duration_ms,
        }));
    });
}

fn handle_subagent_model_command(app: &mut App, trimmed: &str) -> bool {
    if !trimmed.starts_with("/subagent-model") {
        return false;
    }

    if app.is_remote {
        app.push_display_message(DisplayMessage::error(
            "`/subagent-model` requires a live jcode server connection in remote mode.".to_string(),
        ));
        return true;
    }

    let rest = trimmed
        .strip_prefix("/subagent-model")
        .unwrap_or_default()
        .trim();

    if rest.is_empty() || matches!(rest, "show" | "status") {
        app.push_display_message(DisplayMessage::system(format!(
            "Subagent model for this session: {}\n\nUse `/subagent-model <name>` to pin a model, or `/subagent-model inherit` to use the current model.",
            current_subagent_model_summary(app)
        )));
        return true;
    }

    if matches!(rest, "inherit" | "reset" | "clear") {
        app.session.subagent_model = None;
        let _ = app.session.save();
        app.push_display_message(DisplayMessage::system(format!(
            "Subagent model reset to inherit the current model (`{}`).",
            app.provider.model()
        )));
        app.set_status_notice("Subagent model: inherit");
        return true;
    }

    app.session.subagent_model = Some(rest.to_string());
    let _ = app.session.save();
    app.push_display_message(DisplayMessage::system(format!(
        "Subagent model pinned to `{}` for this session.",
        rest
    )));
    app.set_status_notice(format!("Subagent model → {}", rest));
    true
}

fn handle_subagent_command(app: &mut App, trimmed: &str) -> bool {
    if !trimmed.starts_with("/subagent") || trimmed.starts_with("/subagent-model") {
        return false;
    }

    if app.is_remote {
        app.push_display_message(DisplayMessage::error(
            "`/subagent` requires a live jcode server connection in remote mode.".to_string(),
        ));
        return true;
    }

    let rest = trimmed.strip_prefix("/subagent").unwrap_or_default().trim();
    if rest.is_empty() {
        app.push_display_message(DisplayMessage::error(
            "Usage: `/subagent [--type <kind>] [--model <name>] [--continue <session_id>] <prompt>`"
                .to_string(),
        ));
        return true;
    }

    match parse_manual_subagent_spec(rest) {
        Ok(spec) => launch_manual_subagent(app, spec),
        Err(error) => {
            app.push_display_message(DisplayMessage::error(format!(
                "{}\nUsage: `/subagent [--type <kind>] [--model <name>] [--continue <session_id>] <prompt>`",
                error
            )));
        }
    }
    true
}

pub(super) fn handle_help_command(app: &mut App, trimmed: &str) -> bool {
    if let Some(topic) = trimmed
        .strip_prefix("/help ")
        .or_else(|| trimmed.strip_prefix("/? "))
    {
        if let Some(help) = app.command_help(topic) {
            app.push_display_message(DisplayMessage::system(help));
        } else {
            app.push_display_message(DisplayMessage::error(format!(
                "Unknown command '{}'. Use `/help` to list commands.",
                topic.trim()
            )));
        }
        return true;
    }

    if trimmed == "/help" || trimmed == "/?" || trimmed == "/commands" {
        app.help_scroll = Some(0);
        return true;
    }

    false
}

pub(super) fn handle_session_command(app: &mut App, trimmed: &str) -> bool {
    if handle_subagent_model_command(app, trimmed) || handle_subagent_command(app, trimmed) {
        return true;
    }

    if let Some(command) = parse_improve_command(trimmed) {
        match command {
            Ok(command) => handle_improve_command_local(app, command),
            Err(error) => app.push_display_message(DisplayMessage::error(error)),
        }
        return true;
    }

    if trimmed == "/clear" {
        reset_current_session(app);
        return true;
    }

    if trimmed == "/save" || trimmed.starts_with("/save ") {
        let label = trimmed.strip_prefix("/save").unwrap_or_default().trim();
        let label = if label.is_empty() {
            None
        } else {
            Some(label.to_string())
        };
        app.session.mark_saved(label.clone());
        if let Err(e) = app.session.save() {
            app.push_display_message(DisplayMessage::error(format!(
                "Failed to save session: {}",
                e
            )));
            return true;
        }
        app.trigger_save_memory_extraction();
        let name = app.session.display_name().to_string();
        let msg = if let Some(ref lbl) = app.session.save_label {
            format!(
                "📌 Session **{}** saved as \"**{}**\". It will appear at the top of `/resume`.",
                name, lbl,
            )
        } else {
            format!(
                "📌 Session **{}** saved. It will appear at the top of `/resume`.",
                name,
            )
        };
        app.push_display_message(DisplayMessage::system(msg));
        app.set_status_notice("Session saved");
        return true;
    }

    if trimmed == "/unsave" {
        app.session.unmark_saved();
        if let Err(e) = app.session.save() {
            app.push_display_message(DisplayMessage::error(format!(
                "Failed to save session: {}",
                e
            )));
            return true;
        }
        let name = app.session.display_name().to_string();
        app.push_display_message(DisplayMessage::system(format!(
            "Removed bookmark from session **{}**.",
            name,
        )));
        app.set_status_notice("Bookmark removed");
        return true;
    }

    if trimmed == "/memory status" {
        let default_enabled = crate::config::config().features.memory;
        app.push_display_message(DisplayMessage::system(format!(
            "Memory feature: **{}** (config default: {})",
            if app.memory_enabled {
                "enabled"
            } else {
                "disabled"
            },
            if default_enabled {
                "enabled"
            } else {
                "disabled"
            }
        )));
        return true;
    }

    if trimmed == "/memory" {
        let new_state = !app.memory_enabled;
        app.set_memory_feature_enabled(new_state);
        let label = if new_state { "ON" } else { "OFF" };
        app.set_status_notice(&format!("Memory: {}", label));
        app.push_display_message(DisplayMessage::system(format!(
            "Memory feature {} for this session.",
            if new_state { "enabled" } else { "disabled" }
        )));
        return true;
    }

    if trimmed == "/memory on" {
        app.set_memory_feature_enabled(true);
        app.set_status_notice("Memory: ON");
        app.push_display_message(DisplayMessage::system(
            "Memory feature enabled for this session.".to_string(),
        ));
        return true;
    }

    if trimmed == "/memory off" {
        app.set_memory_feature_enabled(false);
        app.set_status_notice("Memory: OFF");
        app.push_display_message(DisplayMessage::system(
            "Memory feature disabled for this session.".to_string(),
        ));
        return true;
    }

    if trimmed.starts_with("/memory ") {
        app.push_display_message(DisplayMessage::error(
            "Usage: `/memory [on|off|status]`".to_string(),
        ));
        return true;
    }

    if handle_goals_command(app, trimmed) {
        return true;
    }

    if trimmed == "/swarm" || trimmed == "/swarm status" {
        let default_enabled = crate::config::config().features.swarm;
        app.push_display_message(DisplayMessage::system(format!(
            "Swarm feature: **{}** (config default: {})",
            if app.swarm_enabled {
                "enabled"
            } else {
                "disabled"
            },
            if default_enabled {
                "enabled"
            } else {
                "disabled"
            }
        )));
        return true;
    }

    if trimmed == "/swarm on" {
        app.set_swarm_feature_enabled(true);
        app.set_status_notice("Swarm: ON");
        app.push_display_message(DisplayMessage::system(
            "Swarm feature enabled for this session.".to_string(),
        ));
        return true;
    }

    if trimmed == "/swarm off" {
        app.set_swarm_feature_enabled(false);
        app.set_status_notice("Swarm: OFF");
        app.push_display_message(DisplayMessage::system(
            "Swarm feature disabled for this session.".to_string(),
        ));
        return true;
    }

    if trimmed.starts_with("/swarm ") {
        app.push_display_message(DisplayMessage::error(
            "Usage: `/swarm [on|off|status]`".to_string(),
        ));
        return true;
    }

    if trimmed == "/rewind" {
        if app.session.messages.is_empty() {
            app.push_display_message(DisplayMessage::system(
                "No messages in conversation.".to_string(),
            ));
            return true;
        }

        let mut history = String::from("**Conversation history:**\n\n");
        for (i, msg) in app.session.messages.iter().enumerate() {
            let role_str = match msg.role {
                Role::User => "👤 User",
                Role::Assistant => "🤖 Assistant",
            };
            let content = msg.content_preview();
            let preview = crate::util::truncate_str(&content, 80);
            history.push_str(&format!("  `{}` {} - {}\n", i + 1, role_str, preview));
        }
        history.push_str("\nUse `/rewind N` to rewind to message N (removes all messages after).");

        app.push_display_message(DisplayMessage::system(history));
        return true;
    }

    if let Some(num_str) = trimmed.strip_prefix("/rewind ") {
        let num_str = num_str.trim();
        match num_str.parse::<usize>() {
            Ok(n) if n > 0 && n <= app.session.messages.len() => {
                let removed = app.session.messages.len() - n;
                app.session.truncate_messages(n);
                app.replace_provider_messages(app.session.messages_for_provider());
                app.session.updated_at = chrono::Utc::now();

                app.clear_display_messages();
                for rendered in crate::session::render_messages(&app.session) {
                    app.push_display_message(DisplayMessage {
                        role: rendered.role,
                        content: rendered.content,
                        tool_calls: rendered.tool_calls,
                        duration_secs: None,
                        title: None,
                        tool_data: rendered.tool_data,
                    });
                }

                app.provider_session_id = None;
                app.session.provider_session_id = None;
                let _ = app.session.save();

                app.push_display_message(DisplayMessage::system(format!(
                    "✓ Rewound to message {}. Removed {} message{}.",
                    n,
                    removed,
                    if removed == 1 { "" } else { "s" }
                )));
            }
            Ok(n) => {
                app.push_display_message(DisplayMessage::error(format!(
                    "Invalid message number: {}. Valid range: 1-{}",
                    n,
                    app.session.messages.len()
                )));
            }
            Err(_) => {
                app.push_display_message(DisplayMessage::error(format!(
                    "Usage: `/rewind N` where N is a message number (1-{})",
                    app.session.messages.len()
                )));
            }
        }
        return true;
    }

    if trimmed == "/poke" {
        let session_id = app.session.id.clone();
        let todos = crate::todo::load_todos(&session_id).unwrap_or_default();
        let incomplete: Vec<_> = todos
            .iter()
            .filter(|t| t.status != "completed" && t.status != "cancelled")
            .collect();

        if incomplete.is_empty() {
            app.push_display_message(DisplayMessage::system(
                "No incomplete todos found. Nothing to poke about.".to_string(),
            ));
            return true;
        }

        let mut todo_list = String::new();
        for t in &incomplete {
            let status_icon = match t.status.as_str() {
                "in_progress" => "🔄",
                _ => "⬜",
            };
            todo_list.push_str(&format!(
                "  {} [{}] {}\n",
                status_icon, t.priority, t.content
            ));
        }

        let poke_msg = format!(
            "Your todo list has {} incomplete item{}:\n\n{}\n\
            Please continue your work. Either:\n\
            1. Keep working and complete the remaining tasks\n\
            2. Update the todo list with `todo_write` if items are already done or no longer needed\n\
            3. If you genuinely need user input to proceed, say so clearly and specifically — \
            but only if truly blocked (this should be rare; prefer making reasonable assumptions)",
            incomplete.len(),
            if incomplete.len() == 1 { "" } else { "s" },
            todo_list,
        );

        if app.is_processing {
            app.cancel_requested = true;
            app.interleave_message = None;
            app.pending_soft_interrupts.clear();
            app.set_status_notice("Interrupting for poke...");
            app.push_display_message(DisplayMessage::system(format!(
                "👉 Interrupting and poking with {} incomplete todo{}...",
                incomplete.len(),
                if incomplete.len() == 1 { "" } else { "s" },
            )));
            app.queued_messages.push(poke_msg);
        } else {
            app.push_display_message(DisplayMessage::system(format!(
                "👉 Poking model with {} incomplete todo{}...",
                incomplete.len(),
                if incomplete.len() == 1 { "" } else { "s" },
            )));

            app.add_provider_message(Message::user(&poke_msg));
            app.session.add_message(
                Role::User,
                vec![ContentBlock::Text {
                    text: poke_msg,
                    cache_control: None,
                }],
            );
            let _ = app.session.save();

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
            app.streaming_tps_start = None;
            app.streaming_tps_elapsed = std::time::Duration::ZERO;
            app.streaming_total_output_tokens = 0;
            app.processing_started = Some(Instant::now());
            app.pending_turn = true;
        }

        return true;
    }

    false
}

pub(super) fn handle_goals_command(app: &mut App, trimmed: &str) -> bool {
    if trimmed == "/goals" {
        match crate::goal::open_goals_overview_for_session(
            active_session_id(app).as_str(),
            active_working_dir(app).as_deref(),
            true,
        ) {
            Ok(snapshot) => {
                app.set_side_panel_snapshot(snapshot);
                let count = crate::goal::list_relevant_goals(active_working_dir(app).as_deref())
                    .map(|goals| goals.len())
                    .unwrap_or(0);
                app.push_display_message(DisplayMessage::system(format!(
                    "Opened goals overview in the side panel ({} goal{}).",
                    count,
                    if count == 1 { "" } else { "s" }
                )));
                app.set_status_notice("Goals");
            }
            Err(e) => app.push_display_message(DisplayMessage::error(format!(
                "Failed to open goals overview: {}",
                e
            ))),
        }
        return true;
    }

    if trimmed == "/goals resume" {
        match crate::goal::resume_goal_for_session(
            active_session_id(app).as_str(),
            active_working_dir(app).as_deref(),
            true,
        ) {
            Ok(Some(result)) => {
                app.set_side_panel_snapshot(result.snapshot);
                let mut msg = format!("Resumed goal **{}**.", result.goal.title);
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
        return true;
    }

    if let Some(id) = trimmed.strip_prefix("/goals show ") {
        let id = id.trim();
        if id.is_empty() {
            app.push_display_message(DisplayMessage::error(
                "Usage: `/goals show <id>`".to_string(),
            ));
            return true;
        }
        match crate::goal::open_goal_for_session(
            active_session_id(app).as_str(),
            active_working_dir(app).as_deref(),
            id,
            true,
        ) {
            Ok(Some(result)) => {
                app.set_side_panel_snapshot(result.snapshot);
                app.push_display_message(DisplayMessage::system(format!(
                    "Opened goal **{}** in the side panel.",
                    result.goal.title
                )));
                app.set_status_notice(format!("Goal: {}", result.goal.title));
            }
            Ok(None) => {
                app.push_display_message(DisplayMessage::error(format!("Goal not found: {}", id)))
            }
            Err(e) => app
                .push_display_message(DisplayMessage::error(format!("Failed to open goal: {}", e))),
        }
        return true;
    }

    if trimmed.starts_with("/goals ") {
        app.push_display_message(DisplayMessage::error(
            "Usage: `/goals`, `/goals resume`, or `/goals show <id>`".to_string(),
        ));
        return true;
    }

    false
}

fn active_session_id(app: &App) -> String {
    if app.is_remote {
        app.remote_session_id
            .clone()
            .unwrap_or_else(|| app.session.id.clone())
    } else {
        app.session.id.clone()
    }
}

fn active_working_dir(app: &App) -> Option<std::path::PathBuf> {
    app.session
        .working_dir
        .as_deref()
        .map(std::path::PathBuf::from)
}

pub(super) fn handle_dictation_command(app: &mut App, trimmed: &str) -> bool {
    if trimmed == "/dictate" || trimmed == "/dictation" {
        app.handle_dictation_trigger();
        return true;
    }

    if trimmed.starts_with("/dictate ") || trimmed.starts_with("/dictation ") {
        app.push_display_message(DisplayMessage::error(
            "Usage: `/dictate`\nConfigure `[dictation]` in `~/.jcode/config.toml` to customize command, mode, hotkey, and timeout."
                .to_string(),
        ));
        return true;
    }

    false
}

pub(super) fn handle_config_command(app: &mut App, trimmed: &str) -> bool {
    use crate::bus::{Bus, BusEvent};

    if trimmed == "/compact mode" || trimmed == "/compact mode status" {
        let mode = app
            .registry
            .compaction()
            .try_read()
            .map(|manager| manager.mode())
            .unwrap_or_default();
        app.push_display_message(DisplayMessage::system(format!(
            "Compaction mode: **{}**\nAvailable: reactive · proactive · semantic\nUse `/compact mode <mode>` to change it for this session.",
            mode.as_str()
        )));
        return true;
    }

    if let Some(mode_str) = trimmed.strip_prefix("/compact mode ") {
        let mode_str = mode_str.trim();
        let Some(mode) = crate::config::CompactionMode::parse(mode_str) else {
            app.push_display_message(DisplayMessage::error(
                "Usage: `/compact mode <reactive|proactive|semantic>`".to_string(),
            ));
            return true;
        };

        match app.registry.compaction().try_write() {
            Ok(mut manager) => {
                manager.set_mode(mode.clone());
                let label = mode.as_str();
                app.push_display_message(DisplayMessage::system(format!(
                    "✓ Compaction mode → {}",
                    label
                )));
                app.set_status_notice(format!("Compaction: {}", label));
            }
            Err(_) => {
                app.push_display_message(DisplayMessage::error(
                    "Cannot access compaction manager (lock held)".to_string(),
                ));
            }
        }
        return true;
    }

    if trimmed == "/compact" {
        if !app.provider.supports_compaction() {
            app.push_display_message(DisplayMessage::system(
                "Manual compaction is not available for this provider.".to_string(),
            ));
            return true;
        }
        let compaction = app.registry.compaction();
        match compaction.try_write() {
            Ok(mut manager) => {
                let stats = manager.stats_with(&app.messages);
                let status_msg = format!(
                    "**Context Status:**\n\
                    • Messages: {} (active), {} (total history)\n\
                    • Token usage: ~{}k (estimate ~{}k) / {}k ({:.1}%)\n\
                    • Has summary: {}\n\
                    • Compacting: {}",
                    stats.active_messages,
                    stats.total_turns,
                    stats.effective_tokens / 1000,
                    stats.token_estimate / 1000,
                    manager.token_budget() / 1000,
                    stats.context_usage * 100.0,
                    if stats.has_summary { "yes" } else { "no" },
                    if stats.is_compacting {
                        "in progress..."
                    } else {
                        "no"
                    }
                );

                match manager.force_compact_with(&app.messages, app.provider.clone()) {
                    Ok(()) => {
                        app.push_display_message(DisplayMessage {
                            role: "system".to_string(),
                            content: format!(
                                "{}\n\n{}\n\
                                The summary will be applied automatically when ready.\n\
                                Use `/help compact` for details.",
                                status_msg,
                                App::format_compaction_started_message("manual")
                            ),
                            tool_calls: vec![],
                            duration_secs: None,
                            title: None,
                            tool_data: None,
                        });
                    }
                    Err(reason) => {
                        app.push_display_message(DisplayMessage {
                            role: "system".to_string(),
                            content: format!(
                                "{}\n\n⚠ **Cannot compact:** {}\n\
                                Try `/fix` for emergency recovery.",
                                status_msg, reason
                            ),
                            tool_calls: vec![],
                            duration_secs: None,
                            title: None,
                            tool_data: None,
                        });
                    }
                }
            }
            Err(_) => {
                app.push_display_message(DisplayMessage {
                    role: "system".to_string(),
                    content: "⚠ Cannot access compaction manager (lock held)".to_string(),
                    tool_calls: vec![],
                    duration_secs: None,
                    title: None,
                    tool_data: None,
                });
            }
        }
        return true;
    }

    if trimmed == "/fix" {
        app.run_fix_command();
        return true;
    }

    if trimmed == "/usage" {
        app.push_display_message(DisplayMessage::system(
            "Fetching usage limits from all providers...".to_string(),
        ));
        tokio::spawn(async move {
            let results = crate::usage::fetch_all_provider_usage().await;
            Bus::global().publish(BusEvent::UsageReport(results));
        });
        return true;
    }

    if trimmed == "/subscription" || trimmed == "/subscription status" {
        app.show_jcode_subscription_status();
        return true;
    }

    if trimmed == "/config" {
        use crate::config::config;
        app.push_display_message(DisplayMessage {
            role: "system".to_string(),
            content: config().display_string(),
            tool_calls: vec![],
            duration_secs: None,
            title: None,
            tool_data: None,
        });
        return true;
    }

    if trimmed == "/config init" || trimmed == "/config create" {
        use crate::config::Config;
        match Config::create_default_config_file() {
            Ok(path) => {
                app.push_display_message(DisplayMessage {
                    role: "system".to_string(),
                    content: format!(
                        "Created default config file at:\n`{}`\n\nEdit this file to customize your keybindings and settings.",
                        path.display()
                    ),
                    tool_calls: vec![],
                    duration_secs: None,
                    title: None,
                    tool_data: None,
                });
            }
            Err(e) => {
                app.push_display_message(DisplayMessage {
                    role: "system".to_string(),
                    content: format!("Failed to create config file: {}", e),
                    tool_calls: vec![],
                    duration_secs: None,
                    title: None,
                    tool_data: None,
                });
            }
        }
        return true;
    }

    if trimmed == "/config edit" {
        use crate::config::Config;
        if let Some(path) = Config::path() {
            if !path.exists() {
                if let Err(e) = Config::create_default_config_file() {
                    app.push_display_message(DisplayMessage {
                        role: "system".to_string(),
                        content: format!("Failed to create config file: {}", e),
                        tool_calls: vec![],
                        duration_secs: None,
                        title: None,
                        tool_data: None,
                    });
                    return true;
                }
            }

            let editor = std::env::var("EDITOR").unwrap_or_else(|_| "nano".to_string());
            app.push_display_message(DisplayMessage {
                role: "system".to_string(),
                content: format!(
                    "Opening config in editor...\n`{} {}`\n\n*Restart jcode after editing for changes to take effect.*",
                    editor,
                    path.display()
                ),
                tool_calls: vec![],
                duration_secs: None,
                title: None,
                tool_data: None,
            });

            let _ = std::process::Command::new(&editor).arg(&path).spawn();
        }
        return true;
    }

    if trimmed.starts_with("/config ") {
        app.push_display_message(DisplayMessage::error(
            "Usage: `/config` (show), `/config init` (create), `/config edit` (open in editor)"
                .to_string(),
        ));
        return true;
    }

    false
}

pub(super) fn handle_debug_command(app: &mut App, trimmed: &str) -> bool {
    super::debug::handle_debug_command(app, trimmed)
}

pub(super) fn handle_model_command(app: &mut App, trimmed: &str) -> bool {
    super::model_context::handle_model_command(app, trimmed)
}

pub(super) fn handle_info_command(app: &mut App, trimmed: &str) -> bool {
    super::state_ui::handle_info_command(app, trimmed)
}

pub(super) fn handle_auth_command(app: &mut App, trimmed: &str) -> bool {
    super::auth::handle_auth_command(app, trimmed)
}

pub(super) fn handle_dev_command(app: &mut App, trimmed: &str) -> bool {
    super::tui_lifecycle::handle_dev_command(app, trimmed)
}

#[cfg(test)]
mod tests {
    use super::parse_manual_subagent_spec;

    #[test]
    fn parse_manual_subagent_spec_accepts_flags_and_prompt() {
        let spec = parse_manual_subagent_spec(
            "--type research --model gpt-5.4 --continue session_123 investigate this bug",
        )
        .expect("parse manual subagent spec");

        assert_eq!(spec.subagent_type, "research");
        assert_eq!(spec.model.as_deref(), Some("gpt-5.4"));
        assert_eq!(spec.session_id.as_deref(), Some("session_123"));
        assert_eq!(spec.prompt, "investigate this bug");
    }

    #[test]
    fn parse_manual_subagent_spec_rejects_missing_prompt() {
        let err = parse_manual_subagent_spec("--model gpt-5.4")
            .expect_err("missing prompt should be rejected");
        assert!(err.contains("Missing prompt"));
    }
}
