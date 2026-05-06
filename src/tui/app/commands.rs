pub(super) use super::commands_improve::{
    build_improve_prompt, build_improve_resume_prompt, build_refactor_prompt,
    build_refactor_resume_prompt, format_improve_status, format_refactor_status,
    handle_improve_command_local, handle_refactor_command_local, improve_launch_notice,
    improve_mode_for, improve_stop_notice, improve_stop_prompt, parse_improve_command,
    parse_refactor_command, refactor_launch_notice, refactor_mode_for, refactor_stop_notice,
    refactor_stop_prompt, restore_improve_mode, session_improve_mode_for,
};
#[cfg(test)]
pub(super) use super::commands_review::queue_autojudge_remote;
pub(super) use super::commands_review::{
    ImproveCommand, ManualSubagentSpec, RefactorCommand, autojudge_status_message,
    autoreview_status_message, build_autojudge_startup_message, build_autoreview_startup_message,
    build_judge_startup_message, build_review_startup_message, current_feedback_target_session_id,
    handle_autojudge_command_local, handle_autoreview_command_local, handle_judge_command_local,
    handle_observe_command, handle_review_command_local, launch_prompt_in_new_session_local,
    maybe_trigger_autojudge_local, maybe_trigger_autoreview_local,
    preferred_one_shot_review_override, prepare_review_spawned_session, queue_review_spawn_remote,
    reset_current_session,
};
pub(super) use super::todos_view::handle_todos_view_command;
use super::{App, DisplayMessage, LocalRewindUndoSnapshot, ProcessingStatus};
use crate::bus::{Bus, BusEvent, GitStatusCompleted, ManualToolCompleted, ToolEvent, ToolStatus};
use crate::id;
use crate::message::{ContentBlock, Message, Role};
use std::path::PathBuf;
use std::process::Command;
use std::time::Instant;

const BTW_PAGE_ID: &str = "btw";
pub(super) const REVIEW_PREFERRED_MODEL: &str = "gpt-5.5";
const POKE_OFF_UI_HINT: &str = "`/poke off` to stop.";

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub(super) enum PokeCommand {
    Trigger,
    On,
    Off,
    Status,
}

pub(super) enum PokeActivation {
    EnabledNoIncomplete,
    Queued,
    SendNow {
        incomplete_count: usize,
        poke_msg: String,
    },
}

pub(super) fn parse_poke_command(trimmed: &str) -> Option<Result<PokeCommand, String>> {
    match trimmed {
        "/poke" => Some(Ok(PokeCommand::Trigger)),
        "/poke on" => Some(Ok(PokeCommand::On)),
        "/poke off" => Some(Ok(PokeCommand::Off)),
        "/poke status" => Some(Ok(PokeCommand::Status)),
        _ if trimmed.starts_with("/poke ") => {
            Some(Err("Usage: `/poke [on|off|status]`".to_string()))
        }
        _ => None,
    }
}

pub(super) fn is_poke_message(message: &str) -> bool {
    message.starts_with("You have ")
        && message.contains(" incomplete todo")
        && message.ends_with("update the todo tool.")
}

pub(super) fn queued_messages_are_only_pokes(messages: &[String]) -> bool {
    !messages.is_empty() && messages.iter().all(|message| is_poke_message(message))
}

pub(super) fn clear_queued_poke_messages(app: &mut App) -> usize {
    let before = app.queued_messages.len();
    app.queued_messages
        .retain(|message| !is_poke_message(message));
    let removed = before.saturating_sub(app.queued_messages.len());
    if removed > 0 && !app.has_queued_followups() {
        app.pending_queued_dispatch = false;
    }
    removed
}

pub(super) fn disable_auto_poke(app: &mut App) -> usize {
    let cleared = clear_queued_poke_messages(app);
    app.auto_poke_incomplete_todos = false;
    cleared
}

pub(super) fn is_non_retryable_auto_poke_error(error: &str) -> bool {
    let lower = error.to_ascii_lowercase();

    // These failures are deterministic for the current request/session shape. Retrying the same
    // auto-poke cannot help and can create an infinite spam loop.
    let deterministic_markers = [
        "400 bad request",
        "invalid_request_error",
        "string_above_max_length",
        "string_too_long",
        "maximum length",
        "request too large",
        "payload too large",
        "body too large",
        "input too large",
        "context length exceeded",
        "context_length_exceeded",
        "maximum context length",
        "token limit exceeded",
        "invalid model",
        "model_not_found",
        "model_not_supported",
        "unsupported parameter",
        "unsupported_value",
        "invalid parameter",
        "invalid schema",
        "invalid tool",
        "invalid image",
        "image too large",
        "unsupported image",
        "unsupported file",
        "file too large",
        "content_policy_violation",
        "safety_violation",
        "permission_denied",
        "unauthorized",
        "401 unauthorized",
        "403 forbidden",
        "insufficient_quota",
        "billing",
        "credit balance",
    ];

    deterministic_markers
        .iter()
        .any(|marker| lower.contains(marker))
}

pub(super) fn stop_auto_poke_for_non_retryable_error(app: &mut App, error: &str) -> bool {
    if !app.auto_poke_incomplete_todos || !is_non_retryable_auto_poke_error(error) {
        return false;
    }

    let cleared = disable_auto_poke(app);
    app.rate_limit_pending_message = None;
    app.rate_limit_reset = None;
    app.push_display_message(DisplayMessage::system(format!(
        "🛑 Auto-poke stopped because the last request failed with a non-retryable error.{} Fix the request/session, then run `/poke` again if you want to resume.",
        if cleared == 0 {
            String::new()
        } else {
            format!(
                " Cleared {} queued poke follow-up{}.",
                cleared,
                if cleared == 1 { "" } else { "s" }
            )
        }
    )));
    app.set_status_notice("Poke stopped: non-retryable error");
    true
}

pub(super) fn poke_disabled_message(cleared: usize) -> String {
    format!(
        "Auto-poke disabled.{}",
        if cleared == 0 {
            String::new()
        } else {
            format!(
                " Cleared {} queued poke follow-up{}.",
                cleared,
                if cleared == 1 { "" } else { "s" }
            )
        }
    )
}

pub(super) fn poke_enabled_without_incomplete_message() -> String {
    "Auto-poke enabled. No incomplete todos found right now.".to_string()
}

pub(super) fn poke_queued_display_message() -> String {
    format!(
        "👉 /poke queued. Re-checking incomplete todos after this turn. {}",
        POKE_OFF_UI_HINT
    )
}

pub(super) fn poke_triggered_display_message(incomplete_count: usize) -> String {
    format!(
        "👉 Poking model: {} incomplete todo{}. {}",
        incomplete_count,
        if incomplete_count == 1 { "" } else { "s" },
        POKE_OFF_UI_HINT,
    )
}

pub(super) fn activate_auto_poke(app: &mut App) -> PokeActivation {
    let incomplete = incomplete_poke_todos(app);
    app.auto_poke_incomplete_todos = true;
    app.set_status_notice("Poke: ON");

    if incomplete.is_empty() {
        return PokeActivation::EnabledNoIncomplete;
    }

    if app.is_processing {
        app.set_status_notice("Poke queued after current turn");
        PokeActivation::Queued
    } else {
        let incomplete_count = incomplete.len();
        let poke_msg = build_poke_message(&incomplete);
        PokeActivation::SendNow {
            incomplete_count,
            poke_msg,
        }
    }
}

pub(super) fn activate_auto_poke_local(app: &mut App) {
    match activate_auto_poke(app) {
        PokeActivation::EnabledNoIncomplete => {
            app.push_display_message(DisplayMessage::system(
                poke_enabled_without_incomplete_message(),
            ));
        }
        PokeActivation::Queued => {
            app.push_display_message(DisplayMessage::system(poke_queued_display_message()));
        }
        PokeActivation::SendNow {
            incomplete_count,
            poke_msg,
        } => {
            app.push_display_message(DisplayMessage::system(poke_triggered_display_message(
                incomplete_count,
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
    }
}

pub(super) fn toggle_auto_poke_hotkey_local(app: &mut App) {
    if app.auto_poke_incomplete_todos {
        let cleared = disable_auto_poke(app);
        app.set_status_notice("Poke: OFF");
        app.push_display_message(DisplayMessage::system(poke_disabled_message(cleared)));
    } else {
        activate_auto_poke_local(app);
    }
}

pub(super) fn transfer_pause_message() -> String {
    "Transfer requested. Please pause after the current step, update the todo list if needed, and stop so work can continue in the transferred session."
        .to_string()
}

fn transfer_active_messages(session: &crate::session::Session) -> Vec<Message> {
    let start = session
        .compaction
        .as_ref()
        .map(|state| state.compacted_count.min(session.messages.len()))
        .unwrap_or(0);
    session.messages[start..]
        .iter()
        .map(crate::session::StoredMessage::to_message)
        .collect()
}

pub(super) fn create_transfer_session_from_parent(
    parent_session_id: &str,
    parent: &crate::session::Session,
    compaction: Option<crate::session::StoredCompactionState>,
) -> anyhow::Result<(String, String)> {
    let todos = crate::todo::load_todos(parent_session_id).unwrap_or_default();
    let mut child = crate::session::Session::create(Some(parent_session_id.to_string()), None);
    child.messages.clear();
    child.compaction = compaction;
    child.working_dir = parent.working_dir.clone();
    child.model = parent.model.clone();
    child.provider_key = parent.provider_key.clone();
    child.subagent_model = parent.subagent_model.clone();
    child.improve_mode = parent.improve_mode;
    child.autoreview_enabled = parent.autoreview_enabled;
    child.autojudge_enabled = parent.autojudge_enabled;
    child.is_canary = parent.is_canary;
    child.testing_build = parent.testing_build.clone();
    child.status = crate::session::SessionStatus::Closed;
    child.provider_session_id = None;
    child.save()?;
    crate::todo::save_todos(&child.id, &todos)?;
    Ok((child.id.clone(), child.display_name().to_string()))
}

async fn prepare_transfer_session_local(
    parent: crate::session::Session,
    provider: std::sync::Arc<dyn crate::provider::Provider>,
) -> anyhow::Result<super::PreparedTransferSession> {
    let compaction = crate::compaction::build_transfer_compaction_state(
        provider,
        transfer_active_messages(&parent),
        parent.compaction.clone(),
    )
    .await?;
    let (session_id, session_name) =
        create_transfer_session_from_parent(parent.id.as_str(), &parent, compaction)?;
    Ok(super::PreparedTransferSession {
        session_id,
        session_name,
    })
}

pub(super) fn start_local_transfer_prepare(app: &mut App) -> anyhow::Result<()> {
    if app.pending_local_transfer.is_some() {
        return Ok(());
    }

    let parent = app.session.clone();
    let provider = app.provider.fork();
    let (tx, rx) = std::sync::mpsc::channel();
    app.pending_local_transfer = Some(super::PendingLocalTransfer { receiver: rx });

    tokio::spawn(async move {
        let result = prepare_transfer_session_local(parent, provider).await;
        let _ = tx.send(result);
    });

    Ok(())
}

pub(super) fn poll_local_transfer_prepare(app: &mut App) -> bool {
    let recv_result = {
        let Some(pending) = app.pending_local_transfer.as_ref() else {
            return false;
        };
        pending.receiver.try_recv()
    };

    match recv_result {
        Ok(result) => {
            app.pending_local_transfer = None;
            app.pending_transfer_request = false;
            match result {
                Ok(prepared) => {
                    let exe = super::launch_client_executable();
                    let cwd = crate::session::Session::load(&prepared.session_id)
                        .ok()
                        .and_then(|session| session.working_dir)
                        .map(std::path::PathBuf::from)
                        .filter(|path| path.is_dir())
                        .or_else(|| std::env::current_dir().ok())
                        .unwrap_or_else(|| std::path::PathBuf::from("."));
                    let socket = std::env::var("JCODE_SOCKET").ok();
                    match super::spawn_in_new_terminal(
                        &exe,
                        &prepared.session_id,
                        &cwd,
                        socket.as_deref(),
                    ) {
                        Ok(true) => {
                            app.push_display_message(DisplayMessage::system(format!(
                                "↗ Transfer launched in **{}**.",
                                prepared.session_name
                            )));
                            app.set_status_notice("Transfer launched");
                        }
                        Ok(false) => {
                            app.push_display_message(DisplayMessage::system(format!(
                                "↗ Transfer session **{}** created.\n\nNo terminal was opened automatically. Resume manually:\n```\njcode --resume {}\n```",
                                prepared.session_name, prepared.session_id
                            )));
                            app.set_status_notice("Transfer session created");
                        }
                        Err(error) => {
                            app.push_display_message(DisplayMessage::error(format!(
                                "Transfer session **{}** was created but failed to open a window: {}\n\nResume manually: `jcode --resume {}`",
                                prepared.session_name, error, prepared.session_id
                            )));
                            app.set_status_notice("Transfer open failed");
                        }
                    }
                }
                Err(error) => {
                    app.push_display_message(DisplayMessage::error(format!(
                        "Failed to prepare transfer session: {}",
                        error
                    )));
                    app.set_status_notice("Transfer failed");
                }
            }
            true
        }
        Err(std::sync::mpsc::TryRecvError::Empty) => false,
        Err(std::sync::mpsc::TryRecvError::Disconnected) => {
            app.pending_local_transfer = None;
            app.pending_transfer_request = false;
            app.push_display_message(DisplayMessage::error(
                "Transfer preparation failed before returning a result.".to_string(),
            ));
            app.set_status_notice("Transfer failed");
            true
        }
    }
}

pub(super) fn maybe_begin_pending_local_transfer(app: &mut App) -> bool {
    if app.is_remote || app.is_processing || !app.pending_transfer_request {
        return false;
    }
    if app.pending_local_transfer.is_some() {
        return false;
    }

    match start_local_transfer_prepare(app) {
        Ok(()) => {
            app.push_display_message(DisplayMessage::system(
                "Preparing transferred session with compacted context...".to_string(),
            ));
            app.set_status_notice("Preparing transfer");
        }
        Err(error) => {
            app.pending_transfer_request = false;
            app.push_display_message(DisplayMessage::error(format!(
                "Failed to start transfer preparation: {}",
                error
            )));
            app.set_status_notice("Transfer failed");
        }
    }
    true
}

pub(super) fn handle_transfer_command_local(app: &mut App) {
    if app.pending_transfer_request || app.pending_local_transfer.is_some() {
        app.push_display_message(DisplayMessage::system(
            "A transfer is already pending.".to_string(),
        ));
        app.set_status_notice("Transfer already pending");
        return;
    }

    app.pending_transfer_request = true;
    if app.is_processing {
        app.interleave_message = Some(transfer_pause_message());
        app.push_display_message(DisplayMessage::system(
            "Queued `/transfer`. The current session will be asked to pause, then the compacted handoff will open in a new window."
                .to_string(),
        ));
        app.set_status_notice("Transfer queued after current turn");
    } else {
        let _ = maybe_begin_pending_local_transfer(app);
    }
}

pub(super) fn poke_status_message(app: &App) -> String {
    let incomplete = incomplete_poke_todos(app);
    let queued_followup = app
        .queued_messages
        .iter()
        .any(|message| is_poke_message(message));
    let mut message = format!(
        "Auto-poke: **{}**. {} incomplete todo{}.",
        if app.auto_poke_incomplete_todos {
            "ON"
        } else {
            "OFF"
        },
        incomplete.len(),
        if incomplete.len() == 1 { "" } else { "s" }
    );
    if queued_followup {
        message.push_str(" A follow-up poke is queued.");
    }
    if app.is_processing {
        message.push_str(" A turn is currently running.");
    }
    message
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
    let sandbox_root = app.session.sandbox_root.clone();
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
            sandbox_root: sandbox_root.as_deref().map(PathBuf::from),
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

fn build_btw_loading_markdown(question: &str) -> String {
    format!(
        "# `/btw`\n\n## Question\n{}\n\n## Status\nThinking…\n",
        question.trim()
    )
}

fn build_btw_system_reminder(question: &str) -> String {
    format!(
        "The user invoked `/btw`, which is a side question about the current session. \
Answer ONLY from the existing conversation/context already in memory for this session. \
Do not read files, run commands, search the web, or call any tool except `side_panel`.\n\n\
Use the `side_panel` tool exactly once with:\n\
- `action`: `write`\n\
- `page_id`: `{}`\n\
- `title`: ``/btw``\n\
- `focus`: `true`\n\n\
Write markdown with this shape:\n\
# `/btw`\n\
## Question\n<repeat the question>\n\
## Answer\n<your concise answer>\n\n\
If the answer is not already knowable from the current session context, say so clearly in the Answer section and explain that a normal prompt is needed.\n\n\
After writing the side panel content, do not add any normal chat response text.\n\n\
Question: {}",
        BTW_PAGE_ID,
        question.trim()
    )
}

fn handle_btw_command(app: &mut App, trimmed: &str) -> bool {
    if !trimmed.starts_with("/btw") {
        return false;
    }

    let question = trimmed.strip_prefix("/btw").unwrap_or_default().trim();
    if question.is_empty() {
        app.push_display_message(DisplayMessage::error(
            "Usage: `/btw <question>`".to_string(),
        ));
        return true;
    }

    match crate::side_panel::write_markdown_page(
        active_session_id(app).as_str(),
        BTW_PAGE_ID,
        Some("`/btw`"),
        &build_btw_loading_markdown(question),
        true,
    ) {
        Ok(snapshot) => app.set_side_panel_snapshot(snapshot),
        Err(error) => {
            app.push_display_message(DisplayMessage::error(format!(
                "Failed to prepare `/btw` side panel: {}",
                error
            )));
            return true;
        }
    }

    app.hidden_queued_system_messages
        .push(build_btw_system_reminder(question));
    if app.is_processing {
        app.push_display_message(DisplayMessage::system(
            "Queued `/btw` — answer will appear in the side panel after the current turn."
                .to_string(),
        ));
        app.set_status_notice("Queued /btw");
    } else {
        app.push_display_message(DisplayMessage::system(
            "Running `/btw` — answer will appear in the side panel.".to_string(),
        ));
        app.pending_queued_dispatch = true;
        app.set_status_notice("Running /btw");
    }

    true
}

fn load_catchup_candidates(app: &App) -> Vec<crate::tui::session_picker::SessionInfo> {
    let current_session_id = active_session_id(app);
    crate::tui::session_picker::load_sessions()
        .unwrap_or_default()
        .into_iter()
        .filter(|session| session.id != current_session_id && session.needs_catchup)
        .collect()
}

fn handle_catchup_command(app: &mut App, trimmed: &str) -> bool {
    if !trimmed.starts_with("/catchup") {
        return false;
    }
    if !app.is_remote {
        app.push_display_message(DisplayMessage::error(
            "`/catchup` currently requires a connected shared server session.".to_string(),
        ));
        return true;
    }

    let rest = trimmed.strip_prefix("/catchup").unwrap_or_default().trim();
    match rest {
        "" | "list" | "show" => {
            app.open_catchup_picker();
            true
        }
        "next" => {
            if app.is_processing {
                app.set_status_notice("Finish current work before Catch Up");
                return true;
            }
            let candidates = load_catchup_candidates(app);
            let total = candidates.len();
            let Some(target) = candidates.first() else {
                app.push_display_message(DisplayMessage::system(
                    "No sessions currently need catch up.".to_string(),
                ));
                app.set_status_notice("Catch Up: none waiting");
                return true;
            };

            let source_session_id = active_session_id(app);
            let target_name = crate::id::extract_session_name(&target.id)
                .map(|name| name.to_string())
                .unwrap_or_else(|| target.id.clone());
            app.queue_catchup_resume(
                target.id.clone(),
                Some(source_session_id),
                Some((1, total)),
                true,
            );
            app.push_display_message(DisplayMessage::system(format!(
                "Queued Catch Up for **{}**.",
                target_name,
            )));
            app.set_status_notice(format!("Catch Up → {}", target_name));
            true
        }
        _ => {
            app.push_display_message(DisplayMessage::error(
                "Usage: `/catchup [next|list]`".to_string(),
            ));
            true
        }
    }
}

fn handle_back_command(app: &mut App, trimmed: &str) -> bool {
    if trimmed != "/back" {
        return false;
    }
    if !app.is_remote {
        app.push_display_message(DisplayMessage::error(
            "`/back` currently requires a connected shared server session.".to_string(),
        ));
        return true;
    }
    if app.is_processing {
        app.set_status_notice("Finish current work before going back");
        return true;
    }
    let Some(target) = app.pop_catchup_return_target() else {
        app.push_display_message(DisplayMessage::system(
            "No previous Catch Up session is available.".to_string(),
        ));
        app.set_status_notice("Back: empty");
        return true;
    };

    let target_name = crate::id::extract_session_name(&target)
        .map(|name| name.to_string())
        .unwrap_or_else(|| target.clone());
    app.queue_catchup_resume(target, None, None, false);
    app.push_display_message(DisplayMessage::system(format!(
        "Queued return to **{}**.",
        target_name,
    )));
    app.set_status_notice(format!("Back → {}", target_name));
    true
}

fn git_command_repo_dir(app: &App) -> Result<PathBuf, String> {
    if let Some(path) = active_working_dir(app) {
        if path.is_dir() {
            return Ok(path);
        }

        return Err(format!(
            "Unable to run `/git`: session working directory `{}` is not accessible from this jcode client.",
            path.display()
        ));
    }

    if app.is_remote {
        return Err(
            "Unable to run `/git`: the remote session does not have a working directory."
                .to_string(),
        );
    }

    std::env::current_dir()
        .map_err(|_| "Unable to determine a working directory for `/git`.".to_string())
}

fn run_git_command(repo_dir: &std::path::Path, args: &[&str]) -> Result<String, String> {
    let output = Command::new("git")
        .args(args)
        .current_dir(repo_dir)
        .output()
        .map_err(|error| format!("Failed to run `git {}`: {}", args.join(" "), error))?;

    if !output.status.success() {
        let stderr = String::from_utf8_lossy(&output.stderr).trim().to_string();
        let failure = if stderr.is_empty() {
            format!(
                "`git {}` exited with status {}",
                args.join(" "),
                output.status
            )
        } else {
            stderr
        };
        return Err(failure);
    }

    Ok(String::from_utf8_lossy(&output.stdout)
        .trim_end()
        .to_string())
}

fn build_git_status_message_for_dir(repo_dir: PathBuf) -> Result<String, String> {
    let repo_root =
        run_git_command(&repo_dir, &["rev-parse", "--show-toplevel"]).map_err(|error| {
            format!(
                "No git repository found for `{}`: {}",
                repo_dir.display(),
                error
            )
        })?;
    let status = run_git_command(&repo_dir, &["status", "--short", "--branch"])?;

    let repo_root_path = std::path::Path::new(&repo_root);
    let relative_dir = repo_dir
        .strip_prefix(repo_root_path)
        .ok()
        .and_then(|path| {
            if path.as_os_str().is_empty() {
                None
            } else {
                Some(path.display().to_string())
            }
        })
        .unwrap_or_else(|| ".".to_string());

    let heading = if relative_dir == "." {
        format!("`/git` in `{}`", repo_root)
    } else {
        format!("`/git` in `{}` (`{}`)", repo_root, relative_dir)
    };

    Ok(format!("{heading}\n\n```text\n{status}\n```"))
}

fn handle_git_command(app: &mut App, trimmed: &str) -> bool {
    if trimmed != "/git" && trimmed != "/git status" {
        if trimmed.starts_with("/git ") {
            app.push_display_message(DisplayMessage::error(
                "Usage: `/git` or `/git status`".to_string(),
            ));
            return true;
        }
        return false;
    }

    let session_id = active_session_id(app);
    match git_command_repo_dir(app) {
        Ok(repo_dir) => {
            app.set_status_notice("Git status loading...");
            std::thread::spawn(move || {
                let result = build_git_status_message_for_dir(repo_dir);
                Bus::global().publish(BusEvent::GitStatusCompleted(GitStatusCompleted {
                    session_id,
                    result,
                }));
            });
        }
        Err(error) => app.push_display_message(DisplayMessage::error(error)),
    }
    true
}

fn transcript_opened_message(path: &std::path::Path) -> String {
    format!(
        "Opened transcript file:\n\n```text\n{}\n```",
        path.display()
    )
}

fn transcript_path_message(path: &std::path::Path) -> String {
    format!("Transcript file:\n\n```text\n{}\n```", path.display())
}

fn handle_transcript_command(app: &mut App, trimmed: &str) -> bool {
    if trimmed != "/transcript" && trimmed != "/transcript path" {
        if trimmed.starts_with("/transcript ") {
            app.push_display_message(DisplayMessage::error(
                "Usage: `/transcript` or `/transcript path`".to_string(),
            ));
            return true;
        }
        return false;
    }

    let session_id = active_session_id(app);
    let path = match crate::session::session_path(&session_id) {
        Ok(path) => path,
        Err(error) => {
            app.push_display_message(DisplayMessage::error(format!(
                "Failed to resolve transcript path: {}",
                error
            )));
            return true;
        }
    };

    if !app.is_remote && app.session.id == session_id {
        let _ = app.session.save();
    }

    if trimmed == "/transcript path" {
        app.push_display_message(DisplayMessage::system(transcript_path_message(&path)));
        app.set_status_notice("Transcript path");
        return true;
    }

    match open::that_detached(&path) {
        Ok(()) => {
            app.push_display_message(DisplayMessage::system(transcript_opened_message(&path)));
            app.set_status_notice("Transcript opened");
        }
        Err(error) => app.push_display_message(DisplayMessage::error(format!(
            "Failed to open transcript file `{}`: {}",
            path.display(),
            error
        ))),
    }

    true
}

pub(super) fn handle_git_status_completed(app: &mut App, completed: GitStatusCompleted) {
    if completed.session_id != active_session_id(app) {
        return;
    }

    match completed.result {
        Ok(message) => {
            app.push_display_message(DisplayMessage::system(message));
            app.set_status_notice("Git status");
        }
        Err(error) => app.push_display_message(DisplayMessage::error(error)),
    }
}

pub(super) fn handle_session_command(app: &mut App, trimmed: &str) -> bool {
    if handle_subagent_model_command(app, trimmed)
        || handle_subagent_command(app, trimmed)
        || handle_observe_command(app, trimmed)
        || handle_todos_view_command(app, trimmed)
        || super::commands_overnight::handle_overnight_command(app, trimmed)
        || super::split_view::handle_split_view_command(app, trimmed)
        || handle_btw_command(app, trimmed)
        || handle_transcript_command(app, trimmed)
        || handle_git_command(app, trimmed)
        || handle_catchup_command(app, trimmed)
        || handle_back_command(app, trimmed)
        || handle_autoreview_command_local(app, trimmed)
        || handle_autojudge_command_local(app, trimmed)
        || handle_review_command_local(app, trimmed)
        || handle_judge_command_local(app, trimmed)
        || handle_selfdev_command(app, trimmed)
    {
        return true;
    }

    if let Some(command) = parse_improve_command(trimmed) {
        match command {
            Ok(command) => handle_improve_command_local(app, command),
            Err(error) => app.push_display_message(DisplayMessage::error(error)),
        }
        return true;
    }

    if let Some(command) = parse_refactor_command(trimmed) {
        match command {
            Ok(command) => handle_refactor_command_local(app, command),
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

    if trimmed == "/rename" || trimmed.starts_with("/rename ") {
        let title = trimmed.strip_prefix("/rename").unwrap_or_default().trim();
        if title.is_empty() {
            app.push_display_message(DisplayMessage::error(
                "Usage: `/rename <session name>` or `/rename --clear`".to_string(),
            ));
            return true;
        }

        if title == "--clear" {
            app.session.rename_title(None);
            if let Err(e) = app.session.save() {
                app.push_display_message(DisplayMessage::error(format!(
                    "Failed to clear session name: {}",
                    e
                )));
                return true;
            }
            let name = app.session.display_name().to_string();
            app.push_display_message(DisplayMessage::system(format!(
                "Cleared custom name for session **{}**.",
                name,
            )));
            app.set_status_notice("Session name cleared");
            return true;
        }

        app.session.rename_title(Some(title.to_string()));
        if let Err(e) = app.session.save() {
            app.push_display_message(DisplayMessage::error(format!(
                "Failed to rename session: {}",
                e
            )));
            return true;
        }
        app.push_display_message(DisplayMessage::system(format!(
            "Renamed session to **{}**.",
            title,
        )));
        app.set_status_notice("Session renamed");
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
        app.set_status_notice(format!("Memory: {}", label));
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

    if trimmed == "/rewind undo" {
        let Some(snapshot) = app.rewind_undo_snapshot.take() else {
            app.push_display_message(DisplayMessage::system("No rewind to undo.".to_string()));
            return true;
        };

        let current_count = app.session.visible_conversation_message_count();
        let restored = snapshot.visible_message_count.saturating_sub(current_count);
        app.session.replace_messages(snapshot.messages);
        app.provider_session_id = snapshot.provider_session_id;
        app.session.provider_session_id = snapshot.session_provider_session_id;
        app.session.updated_at = chrono::Utc::now();
        let provider_messages = app.session.messages_for_provider_uncached();
        app.replace_provider_messages(provider_messages);

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

        let _ = app.session.save();
        app.push_display_message(DisplayMessage::system(format!(
            "✓ Undid rewind. Restored {} message{}.",
            restored,
            if restored == 1 { "" } else { "s" }
        )));
        return true;
    }

    if trimmed == "/rewind" {
        let visible_messages = app.session.visible_conversation_messages();
        if visible_messages.is_empty() {
            app.push_display_message(DisplayMessage::system(
                "No messages in conversation.".to_string(),
            ));
            return true;
        }

        let mut history = String::from("**Conversation history:**\n\n");
        for (i, msg) in visible_messages.iter().enumerate() {
            let role_str = match msg.role {
                Role::User => "👤 User",
                Role::Assistant => "🤖 Assistant",
            };
            let content = msg.content_preview();
            let preview = crate::util::truncate_str(&content, 80);
            history.push_str(&format!("  `{}` {} - {}\n", i + 1, role_str, preview));
        }
        history.push_str("\nUse `/rewind N` to rewind to message N (removes all messages after). After rewinding, use `/rewind undo` to restore the removed messages.");

        app.push_display_message(DisplayMessage::system(history));
        return true;
    }

    if let Some(num_str) = trimmed.strip_prefix("/rewind ") {
        let num_str = num_str.trim();
        let visible_count = app.session.visible_conversation_message_count();
        match num_str.parse::<usize>() {
            Ok(n) if n > 0 && n <= visible_count => {
                let removed = visible_count - n;
                app.rewind_undo_snapshot = Some(LocalRewindUndoSnapshot {
                    messages: app.session.messages.clone(),
                    provider_session_id: app.provider_session_id.clone(),
                    session_provider_session_id: app.session.provider_session_id.clone(),
                    visible_message_count: visible_count,
                });
                if let Some(stored_len) = app.session.stored_len_for_visible_conversation_message(n)
                {
                    app.session.truncate_messages(stored_len);
                }
                let provider_messages = app.session.messages_for_provider_uncached();
                app.replace_provider_messages(provider_messages);
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
                    "✓ Rewound to message {}. Removed {} message{}. Undo anytime with `/rewind undo`.",
                    n,
                    removed,
                    if removed == 1 { "" } else { "s" }
                )));
            }
            Ok(n) => {
                app.push_display_message(DisplayMessage::error(format!(
                    "Invalid message number: {}. Valid range: 1-{}",
                    n, visible_count
                )));
            }
            Err(_) => {
                app.push_display_message(DisplayMessage::error(format!(
                    "Usage: `/rewind N` where N is a message number (1-{})",
                    visible_count
                )));
            }
        }
        return true;
    }

    if let Some(command) = parse_poke_command(trimmed) {
        match command {
            Err(error) => app.push_display_message(DisplayMessage::error(error)),
            Ok(PokeCommand::Status) => {
                app.push_display_message(DisplayMessage::system(poke_status_message(app)));
            }
            Ok(PokeCommand::Off) => {
                let cleared = disable_auto_poke(app);
                app.set_status_notice("Poke: OFF");
                app.push_display_message(DisplayMessage::system(poke_disabled_message(cleared)));
            }
            Ok(PokeCommand::Trigger | PokeCommand::On) => {
                activate_auto_poke_local(app);
            }
        }

        return true;
    }

    if trimmed == "/transfer" {
        if app.is_remote {
            app.push_display_message(DisplayMessage::error(
                "`/transfer` requires an active connected session in remote mode.".to_string(),
            ));
        } else {
            handle_transfer_command_local(app);
        }
        return true;
    }

    if trimmed.starts_with("/transfer ") {
        app.push_display_message(DisplayMessage::error("Usage: `/transfer`".to_string()));
        return true;
    }

    false
}

fn handle_selfdev_command(app: &mut App, trimmed: &str) -> bool {
    if !trimmed.starts_with("/selfdev") {
        return false;
    }

    let rest = trimmed.strip_prefix("/selfdev").unwrap_or_default().trim();
    if rest == "status" {
        match crate::tool::selfdev::selfdev_status_output() {
            Ok(output) => {
                app.push_display_message(DisplayMessage::system(output.output));
                app.set_status_notice("Self-dev status");
            }
            Err(e) => app.push_display_message(DisplayMessage::error(format!(
                "Failed to read self-dev status: {}",
                e
            ))),
        }
        return true;
    }

    if rest == "help" {
        app.push_display_message(DisplayMessage::system(
            "`/selfdev`\nSpawn a new self-dev jcode session in a separate terminal.\n\n`/selfdev <prompt>`\nSpawn a new self-dev session and auto-deliver the prompt to it.\n\n`/selfdev status`\nShow current self-dev/build status."
                .to_string(),
        ));
        return true;
    }

    let prompt = if rest.is_empty() || rest == "enter" {
        None
    } else if let Some(prompt) = rest.strip_prefix("enter ") {
        let prompt = prompt.trim();
        (!prompt.is_empty()).then(|| prompt.to_string())
    } else {
        Some(rest.to_string())
    };

    match crate::tool::selfdev::enter_selfdev_session(
        Some(&active_session_id(app)),
        active_working_dir(app).as_deref(),
    ) {
        Ok(launch) => {
            let mut message = if launch.test_mode {
                format!(
                    "Created self-dev session `{}` in `{}`.\n\nTest mode skipped launching a new terminal.",
                    launch.session_id,
                    launch.repo_dir.display()
                )
            } else if launch.launched {
                format!(
                    "Spawned self-dev session `{}` in a new terminal.\n\nRepo: `{}`",
                    launch.session_id,
                    launch.repo_dir.display()
                )
            } else {
                format!(
                    "Created self-dev session `{}` but could not auto-open a supported terminal.\n\nRun manually:\n`{}`",
                    launch.session_id,
                    launch.command_preview().unwrap_or_else(|| format!(
                        "jcode --resume {} self-dev",
                        launch.session_id
                    ))
                )
            };

            if launch.inherited_context {
                message.push_str("\n\nContext was cloned from the current session.");
            }

            if let Some(prompt_text) = prompt {
                if launch.launched && !launch.test_mode {
                    crate::tool::selfdev::schedule_selfdev_prompt_delivery(
                        launch.session_id.clone(),
                        prompt_text,
                    );
                    message.push_str("\n\nPrompt delivery queued to the spawned self-dev session.");
                } else if launch.test_mode {
                    message.push_str("\n\nPrompt captured but not delivered in test mode.");
                } else {
                    message.push_str("\n\nPrompt was not auto-delivered because the self-dev terminal did not launch.");
                }
            }

            app.push_display_message(DisplayMessage::system(message));
            app.set_status_notice("Self-dev");
        }
        Err(e) => app.push_display_message(DisplayMessage::error(format!(
            "Failed to enter self-dev mode: {}",
            e
        ))),
    }

    true
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

pub(super) fn active_session_id(app: &App) -> String {
    if app.is_remote {
        app.remote_session_id
            .clone()
            .unwrap_or_else(|| app.session.id.clone())
    } else {
        app.session.id.clone()
    }
}

pub(super) fn incomplete_poke_todos(app: &App) -> Vec<crate::todo::TodoItem> {
    crate::todo::load_todos(&active_session_id(app))
        .unwrap_or_default()
        .into_iter()
        .filter(|todo| todo.status != "completed" && todo.status != "cancelled")
        .collect()
}

pub(super) fn build_poke_message(incomplete: &[crate::todo::TodoItem]) -> String {
    format!(
        "You have {} incomplete todo{}. Continue working, or update the todo tool.",
        incomplete.len(),
        if incomplete.len() == 1 { "" } else { "s" },
    )
}

pub(super) fn active_working_dir(app: &App) -> Option<std::path::PathBuf> {
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

fn alignment_label(centered: bool) -> &'static str {
    if centered { "centered" } else { "left-aligned" }
}

fn alignment_status_notice(centered: bool) -> &'static str {
    if centered {
        "Layout: Centered"
    } else {
        "Layout: Left-aligned"
    }
}

fn parse_alignment_value(raw: &str) -> Option<bool> {
    match raw.trim().to_ascii_lowercase().as_str() {
        "centered" | "center" | "centre" | "on" => Some(true),
        "left" | "left-aligned" | "left_aligned" | "off" => Some(false),
        _ => None,
    }
}

fn parse_agents_target(raw: &str) -> Option<crate::tui::AgentModelTarget> {
    match raw.trim().to_ascii_lowercase().as_str() {
        "swarm" | "agent" | "agents" | "subagent" | "subagents" => {
            Some(crate::tui::AgentModelTarget::Swarm)
        }
        "review" | "reviewer" | "code-review" | "codereview" => {
            Some(crate::tui::AgentModelTarget::Review)
        }
        "judge" | "judging" | "execution-judge" | "autojudge" => {
            Some(crate::tui::AgentModelTarget::Judge)
        }
        "memory" | "memories" | "sidecar" => Some(crate::tui::AgentModelTarget::Memory),
        "ambient" => Some(crate::tui::AgentModelTarget::Ambient),
        _ => None,
    }
}

pub(super) fn handle_agents_command(app: &mut App, trimmed: &str) -> bool {
    if !trimmed.starts_with("/agents") {
        return false;
    }

    let rest = trimmed.strip_prefix("/agents").unwrap_or_default().trim();
    if rest.is_empty() {
        app.open_agents_picker();
        return true;
    }

    let Some(target) = parse_agents_target(rest) else {
        app.push_display_message(DisplayMessage::error(
            "Usage: `/agents` or `/agents <swarm|review|judge|memory|ambient>`".to_string(),
        ));
        return true;
    };

    app.open_agent_model_picker(target);
    true
}

fn handle_alignment_command(app: &mut App, trimmed: &str) -> bool {
    if !trimmed.starts_with("/alignment") {
        return false;
    }

    let rest = trimmed
        .strip_prefix("/alignment")
        .unwrap_or_default()
        .trim();

    if rest.is_empty() || matches!(rest, "show" | "status") {
        let saved = crate::config::Config::load().display.centered;
        app.push_display_message(DisplayMessage::system(format!(
            "Alignment is currently **{}**.\nSaved default: **{}**.\n\nUse `/alignment centered` or `/alignment left` to change it permanently, or press `Alt+C` to toggle it for the current session.",
            alignment_label(app.centered),
            alignment_label(saved)
        )));
        return true;
    }

    let Some(centered) = parse_alignment_value(rest) else {
        app.push_display_message(DisplayMessage::error(
            "Usage: `/alignment` (show), `/alignment centered`, or `/alignment left`".to_string(),
        ));
        return true;
    };

    app.set_centered(centered);
    app.set_status_notice(alignment_status_notice(centered));

    match crate::config::Config::set_display_centered(centered) {
        Ok(()) => app.push_display_message(DisplayMessage::system(format!(
            "Saved default alignment: **{}**. Applied to this session immediately.",
            alignment_label(centered)
        ))),
        Err(error) => app.push_display_message(DisplayMessage::error(format!(
            "Applied **{}** alignment for this session, but failed to save it as the default: {}",
            alignment_label(centered),
            error
        ))),
    }

    true
}

pub(super) fn handle_config_command(app: &mut App, trimmed: &str) -> bool {
    if handle_alignment_command(app, trimmed) {
        return true;
    }

    if handle_agents_command(app, trimmed) {
        return true;
    }

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
                let provider_messages = app.materialized_provider_messages();
                let stats = manager.stats_with(&provider_messages);
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

                match manager.force_compact_with(&provider_messages, app.provider.clone()) {
                    Ok(()) => {
                        app.set_status_notice(App::format_compaction_progress_notice(
                            std::time::Duration::ZERO,
                        ));
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

    if handle_usage_command(app, trimmed) {
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
            if !path.exists()
                && let Err(e) = Config::create_default_config_file()
            {
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

pub(super) fn handle_usage_command(app: &mut App, trimmed: &str) -> bool {
    let Some(rest) = trimmed.strip_prefix("/usage") else {
        return false;
    };

    if !rest.is_empty()
        && !rest
            .chars()
            .next()
            .map(|c| c.is_whitespace())
            .unwrap_or(false)
    {
        return false;
    }

    app.open_usage_inline_loading();
    app.request_usage_report();
    true
}

pub(super) fn handle_feedback_command(app: &mut App, trimmed: &str) -> bool {
    let Some(rest) = trimmed.strip_prefix("/feedback") else {
        return false;
    };

    let feedback = rest.trim();
    if feedback.is_empty() {
        app.push_display_message(DisplayMessage::error(
            "Usage: `/feedback <your feedback>`".to_string(),
        ));
        return true;
    }

    crate::telemetry::record_feedback(feedback);
    app.push_display_message(DisplayMessage::system(
        "Thanks, recorded your feedback.".to_string(),
    ));
    app.set_status_notice("Feedback recorded");
    true
}

pub(super) fn handle_dev_command(app: &mut App, trimmed: &str) -> bool {
    super::tui_lifecycle_runtime::handle_dev_command(app, trimmed)
}

#[cfg(test)]
#[path = "commands_tests.rs"]
mod tests;
