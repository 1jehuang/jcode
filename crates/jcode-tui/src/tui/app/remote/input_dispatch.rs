use super::super::{PendingRemoteMessage, PendingSplitPrompt};
use super::*;
use crate::tui::app as app_mod;

#[expect(
    clippy::too_many_arguments,
    reason = "remote send needs explicit message payload, reminders, retry metadata, and image attachments"
)]
pub(in crate::tui::app) async fn begin_remote_send(
    app: &mut App,
    remote: &mut RemoteConnection,
    content: String,
    images: Vec<(String, String)>,
    is_system: bool,
    system_reminder: Option<String>,
    auto_retry: bool,
    retry_attempts: u8,
) -> Result<u64> {
    app.prepare_cloud_environment(remote).await?;
    let recovery = app.cloud_recovery_reminder();
    let system_reminder = match (system_reminder, recovery.as_ref()) {
        (Some(reminder), Some(recovery)) => Some(format!("{reminder}\n\n{recovery}")),
        (None, Some(recovery)) => Some(recovery.clone()),
        (reminder, None) => reminder,
    };
    let msg_id = remote
        .send_message_with_images_reminder_and_skill(
            content.clone(),
            images.clone(),
            system_reminder.clone(),
            app.active_skill.clone(),
        )
        .await?;
    if recovery.is_some() {
        app.cloud_recovery_context = None;
        app.cloud_recovery_auto_pending = false;
    }
    app.current_message_id = Some(msg_id);
    app.deferred_stream_done_id = None;
    app.is_processing = true;
    app.status = ProcessingStatus::Sending;
    app.status_detail = None;
    app.processing_started = Some(Instant::now());
    if !content.is_empty() {
        if is_system {
            app.visible_turn_started.get_or_insert_with(Instant::now);
        } else {
            app.visible_turn_started = Some(Instant::now());
        }
    }
    app.last_stream_activity = Some(Instant::now());
    app.remote_resume_activity = None;
    app.reset_streaming_tps();
    // New turn -> new API call: the next usage report must replace, not merge
    // into, the previous call's cache counters (issue #441). Newer servers
    // also emit KvCacheRequest per call, which re-arms this flag per call.
    app.mark_stream_usage_call_boundary();
    app.thought_line_inserted = false;
    app.thinking_prefix_emitted = false;
    app.thinking_buffer.clear();
    app.rate_limit_pending_message = Some(PendingRemoteMessage {
        content,
        images,
        is_system,
        system_reminder,
        auto_retry,
        retry_attempts,
        retry_at: None,
    });
    app.autoreview_after_current_turn = !is_system;
    app.autojudge_after_current_turn = !is_system;
    remote.reset_call_output_tokens_seen();
    Ok(msg_id)
}

pub(in crate::tui::app) fn restore_prepared_remote_input(
    app: &mut App,
    prepared: input::PreparedInput,
) {
    app.input = prepared.raw_input;
    app.cursor_pos = app.input.len();
    app.pending_images = prepared.images;
}

pub(in crate::tui::app) fn history_matches_pending_startup_prompt(app: &App) -> bool {
    if !app.submit_input_on_startup || !app.pending_images.is_empty() || app.input.trim().is_empty()
    {
        return false;
    }

    app.display_messages()
        .iter()
        .rev()
        .find(|message| message.role == "user")
        .is_some_and(|message| message.content == app.input)
}

/// Restore the visible user turn for a startup prompt that was sent before the
/// bootstrap History payload arrived. History replaces all display messages,
/// and the server does not emit a separate user-message event for this request.
pub(in crate::tui::app) fn restore_pending_startup_prompt_echo(app: &mut App) {
    let Some(prompt) = app.pending_startup_prompt_echo.take() else {
        return;
    };
    let already_visible = app
        .display_messages()
        .iter()
        .rev()
        .find(|message| message.role == "user")
        .is_some_and(|message| message.content == prompt);
    if !already_visible {
        app.push_display_message(DisplayMessage::user(prompt));
    }
}

pub(in crate::tui::app) async fn submit_prepared_remote_input(
    app: &mut App,
    remote: &mut RemoteConnection,
    prepared: input::PreparedInput,
) -> Result<()> {
    if app.remote_model_switch_in_flight || app.auth_catalog_refresh_pending {
        app.pending_prompt_after_model_switch = Some(prepared);
        app.set_status_notice(if app.auth_catalog_refresh_pending {
            "Prompt queued until model setup completes"
        } else {
            "Prompt queued until model switch completes"
        });
        return Ok(());
    }

    // Submitting before the bootstrap History payload has been applied is racy:
    // the session-change branch of the History handler calls
    // `clear_display_messages()`, which wipes the user message we are about to
    // echo locally (the prompt appears to "vanish" while the server still
    // streams a reply against it). Hold the prompt and let
    // `process_remote_followups` dispatch it once history is loaded - the same
    // gating that startup auto-submit already relies on.
    if !remote.has_loaded_history() {
        crate::logging::info(
            "Deferring manually submitted prompt until remote history loads (avoids first-prompt clobber)",
        );
        app.pending_prompt_before_history = Some(prepared);
        app.set_status_notice("Loading session...");
        return Ok(());
    }

    if let Some(command) = input::extract_input_shell_command(&prepared.expanded) {
        submit_remote_input_shell(app, remote, prepared.raw_input, command.to_string()).await?;
        return Ok(());
    }

    app.commit_pending_streaming_assistant_message();
    // A manually submitted prompt supersedes any armed post-error fallback
    // offer (and its staged resend): the user chose to continue differently.
    app.clear_pending_fallback_offer();
    // Remember the typed prompt so we can restore it to the input box if this turn
    // fails (e.g. "token refresh needed"), instead of dropping it.
    app.last_submitted_input = Some(prepared.raw_input.clone());
    app.push_display_message(DisplayMessage {
        role: "user".to_string(),
        content: prepared.raw_input,
        tool_calls: vec![],
        duration_secs: None,
        title: None,
        tool_data: None,
        tool_duration_ms: None,
    });
    let _ = app
        .begin_remote_send(remote, prepared.expanded, prepared.images, false)
        .await;
    Ok(())
}

/// Route a slash input through the remote client instead of the local
/// `submit_input` path. Built-in slash commands still belong to the client,
/// but a skill invocation with a trailing prompt must become a remote turn.
/// Calling `App::submit_input` directly for that case sets `pending_turn`,
/// which only the local run loop consumes, leaving remote sessions stuck in
/// the sending state.
pub(in crate::tui::app) async fn submit_remote_slash_input(
    app: &mut App,
    remote: &mut RemoteConnection,
    prepared: input::PreparedInput,
) -> Result<()> {
    let raw_input = prepared.raw_input.clone();

    if crate::tui::is_ssh_remote() {
        // Box the shared dispatcher because its non-SSH fallback can call
        // this entry point. The SSH branch terminates in a wire send instead.
        return Box::pin(super::key_handling::submit_prepared_remote_enter_input(
            app, remote, prepared,
        ))
        .await;
    }

    // Text that merely starts with `/` is not necessarily a command. A terminal
    // file drop (`/tmp/shot.png`) or a bare path (`/home/me/notes`) is ordinary
    // user input. Routing those through `App::submit_input` stages a *local*
    // turn via `pending_turn`, which no remote run loop consumes, so the client
    // parks in "Sending" forever. Send them as a normal remote turn instead.
    //
    // `/?` is the one builtin whose token is not identifier-shaped, so it is
    // allowed through explicitly.
    // Resolve registered multi-word skill names before falling back to the
    // existing single-token command handling.
    let snapshot = app.current_skills_snapshot();
    let trimmed = raw_input.trim();
    let is_command_shaped = trimmed == "/?"
        || (input::parse_dropped_paths(&raw_input).is_none()
            && (snapshot.resolve_invocation(&raw_input).is_some()
                || app_mod::commands_dispatch::contains_registered_slash_command(trimmed)));
    if !is_command_shaped {
        return submit_prepared_remote_input(app, remote, prepared).await;
    }

    let Some(invocation) = snapshot.resolve_invocation(&raw_input) else {
        app.input = raw_input;
        app.cursor_pos = app.input.len();
        app.submit_input();
        return Ok(());
    };

    let Some(trailing_prompt) = invocation.prompt else {
        app.input = raw_input;
        app.cursor_pos = app.input.len();
        app.submit_input();
        return Ok(());
    };

    let skill_name = invocation.name.to_string();
    let mut skill = snapshot.get(&skill_name).cloned();
    if skill.is_none() {
        app.refresh_skills_snapshot();
        skill = app.current_skills_snapshot().get(&skill_name).cloned();
    }
    if skill.is_none() {
        // Preserve the existing unknown-skill and built-in slash-command
        // handling, including the helpful endorsed-skill installation hint.
        app.input = raw_input;
        app.cursor_pos = app.input.len();
        app.submit_input();
        return Ok(());
    }

    // Reuse the normal bare invocation path to update active_skill and show
    // the activation notice, then prepare only the trailing prompt for the
    // remote request. This avoids duplicating slash-command presentation and
    // keeps pasted images attached to the same user turn.
    app.input = format!("/{}", skill_name);
    app.cursor_pos = app.input.len();
    app.pending_images.clear();
    app.submit_input();

    let expanded_prompt = app
        .current_skills_snapshot()
        .resolve_invocation(&prepared.expanded)
        .and_then(|invocation| invocation.prompt)
        .unwrap_or(trailing_prompt)
        .to_string();
    submit_prepared_remote_input(
        app,
        remote,
        input::PreparedInput {
            raw_input: prepared.raw_input,
            expanded: expanded_prompt,
            images: prepared.images,
        },
    )
    .await
}

pub(in crate::tui::app) async fn route_prepared_input_to_new_remote_session(
    app: &mut App,
    remote: &mut RemoteConnection,
    prepared: input::PreparedInput,
) -> Result<()> {
    app.route_next_prompt_to_new_session = false;
    app.pending_split_startup_message = None;
    app.pending_split_prompt = Some(PendingSplitPrompt {
        content: prepared.expanded,
        images: prepared.images,
    });
    app.pending_split_model_override = None;
    app.pending_split_provider_key_override = None;
    app.pending_split_label = Some("Prompt".to_string());
    app.pending_split_started_at = Some(Instant::now());

    app.pending_split_request = false;
    if app.is_processing {
        app.set_status_notice("Prompt launching in new session");
        if let Err(error) = remote.split().await {
            let pending = app
                .pending_split_prompt
                .take()
                .map(|prompt| input::PreparedInput {
                    raw_input: prepared.raw_input,
                    expanded: prompt.content,
                    images: prompt.images,
                });
            app.pending_split_model_override = None;
            app.pending_split_provider_key_override = None;
            app.pending_split_label = None;
            if let Some(prepared) = pending {
                restore_prepared_remote_input(app, prepared);
            }
            return Err(error);
        }
        return Ok(());
    }

    begin_remote_split_launch(app, "Prompt");
    if let Err(error) = remote.split().await {
        finish_remote_split_launch(app);
        let pending = app
            .pending_split_prompt
            .take()
            .map(|prompt| input::PreparedInput {
                raw_input: prepared.raw_input,
                expanded: prompt.content,
                images: prompt.images,
            });
        app.pending_split_model_override = None;
        app.pending_split_provider_key_override = None;
        app.pending_split_label = None;
        if let Some(prepared) = pending {
            restore_prepared_remote_input(app, prepared);
        }
        return Err(error);
    }
    Ok(())
}

pub(in crate::tui::app) fn begin_remote_split_launch(app: &mut App, label: &str) {
    app.is_processing = true;
    app.status = ProcessingStatus::Sending;
    app.status_detail = None;
    let started_at = Instant::now();
    app.pending_split_started_at = Some(started_at);
    app.processing_started = Some(started_at);
    app.last_stream_activity = Some(started_at);
    app.remote_resume_activity = None;
    app.reset_streaming_tps();
    app.thought_line_inserted = false;
    app.thinking_prefix_emitted = false;
    app.thinking_buffer.clear();
    app.current_message_id = None;
    app.set_status_notice(format!("{} launching", label));
}

pub(in crate::tui::app) fn finish_remote_split_launch(app: &mut App) {
    if !app.is_processing || app.current_message_id.is_some() {
        return;
    }
    if !matches!(app.status, ProcessingStatus::Sending) {
        return;
    }
    app.is_processing = false;
    app.status = ProcessingStatus::Idle;
    app.stream_message_ended = false;
    app.processing_started = None;
    app.clear_visible_turn_started();
    app.last_stream_activity = None;
    app.reset_streaming_tps();
    app.current_message_id = None;
}

fn set_transcript_input(app: &mut App, text: String) {
    app.input_typing_undo = None;
    app.input = text;
    app.cursor_pos = app.input.len();
    app.reset_tab_completion();
    app.sync_model_picker_preview_from_input();
}

fn transcript_send_text(text: &str) -> String {
    const TRANSCRIPTION_PREFIX: &str = "[transcription]";

    let trimmed_start = text.trim_start();
    if trimmed_start.is_empty()
        || trimmed_start.starts_with(TRANSCRIPTION_PREFIX)
        || trimmed_start.starts_with('/')
        || trimmed_start.starts_with('!')
    {
        return text.to_string();
    }

    format!("{} {}", TRANSCRIPTION_PREFIX, trimmed_start)
}

fn queue_transcript_input(app: &mut App) {
    input::queue_message(app);
    let count = app.queued_messages.len();
    app.set_status_notice(format!(
        "Transcript queued ({} message{})",
        count,
        if count == 1 { "" } else { "s" }
    ));
}

fn submit_transcript_input(app: &mut App) {
    match app.send_action(false) {
        SendAction::Submit => app.submit_input(),
        SendAction::Queue => queue_transcript_input(app),
        SendAction::Interleave => {
            let prepared = input::take_prepared_input(app);
            input::stage_local_interleave(app, prepared.expanded, prepared.images);
        }
    }
}

async fn submit_remote_transcript_input(
    app: &mut App,
    remote: &mut RemoteConnection,
) -> Result<()> {
    input::promote_dropped_images(app);
    let trimmed = app.input.trim().to_string();
    if trimmed.is_empty() {
        app.set_status_notice("Transcript was empty");
        return Ok(());
    }

    if trimmed.starts_with('/') {
        let prepared = input::take_prepared_input(app);
        submit_remote_slash_input(app, remote, prepared).await?;
        return Ok(());
    }

    if let Some(command) = input::extract_input_shell_command(&trimmed) {
        let raw_input = std::mem::take(&mut app.input);
        app.cursor_pos = 0;
        app.clear_input_undo_history();
        submit_remote_input_shell(app, remote, raw_input, command.to_string()).await?;
        return Ok(());
    }

    match app.send_action(false) {
        SendAction::Submit => {
            let prepared = input::take_prepared_input(app);
            app.push_display_message(DisplayMessage {
                role: "user".to_string(),
                content: prepared.raw_input,
                tool_calls: vec![],
                duration_secs: None,
                title: None,
                tool_data: None,
                tool_duration_ms: None,
            });
            app.begin_remote_send(remote, prepared.expanded, prepared.images, false)
                .await?;
        }
        SendAction::Queue => queue_transcript_input(app),
        SendAction::Interleave => {
            let prepared = input::take_prepared_input(app);
            app.send_interleave_now(prepared.expanded, prepared.images, remote)
                .await;
        }
    }

    Ok(())
}

pub(super) async fn submit_remote_input_shell(
    app: &mut App,
    remote: &mut RemoteConnection,
    raw_input: String,
    command: String,
) -> Result<()> {
    app.commit_pending_streaming_assistant_message();
    app.push_display_message(DisplayMessage::user(raw_input));

    if command.trim().is_empty() {
        app.push_display_message(DisplayMessage::system(
            "Shell command cannot be empty after !.",
        ));
        app.set_status_notice("Shell command is empty");
        return Ok(());
    }

    let request_id = remote.send_input_shell(command.clone()).await?;
    app.current_message_id = Some(request_id);
    app.is_processing = true;
    app.status = ProcessingStatus::Sending;
    app.status_detail = None;
    app.processing_started = Some(Instant::now());
    app.visible_turn_started = Some(Instant::now());
    app.last_stream_activity = Some(Instant::now());
    app.remote_resume_activity = None;
    app.reset_streaming_tps();
    app.thought_line_inserted = false;
    app.thinking_prefix_emitted = false;
    app.thinking_buffer.clear();
    app.rate_limit_pending_message = None;
    remote.reset_call_output_tokens_seen();
    app.set_status_notice(format!(
        "Running remote shell: {}",
        crate::util::truncate_str(&command, 48)
    ));
    Ok(())
}

pub(in crate::tui::app) fn apply_transcript_event(
    app: &mut App,
    text: String,
    mode: TranscriptMode,
) {
    if text.trim().is_empty() {
        app.set_status_notice("Transcript was empty");
        return;
    }

    match mode {
        TranscriptMode::Insert => {
            input::insert_input_text(app, &text);
            app.set_status_notice("Transcript inserted");
        }
        TranscriptMode::Append => {
            let mut combined = app.input.clone();
            combined.push_str(&text);
            set_transcript_input(app, combined);
            app.set_status_notice("Transcript appended");
        }
        TranscriptMode::Replace => {
            set_transcript_input(app, text);
            app.set_status_notice("Transcript replaced input");
        }
        TranscriptMode::Send => {
            let text = transcript_send_text(&text);
            input::insert_input_text(app, &text);
            submit_transcript_input(app);
        }
    }

    app.follow_chat_bottom_for_typing();
}

/// Composer state set aside while a voice transcript is sent as its own
/// prompt, so text the user already typed is neither sent nor lost.
struct StashedDraft {
    input: String,
    cursor_pos: usize,
    pasted_contents: Vec<String>,
    pending_images: Vec<(String, String)>,
    undo: Vec<(String, usize)>,
}

fn stash_draft_for_voice(app: &mut App, transcript: &str) -> StashedDraft {
    let stash = StashedDraft {
        input: std::mem::take(&mut app.input),
        cursor_pos: std::mem::take(&mut app.cursor_pos),
        pasted_contents: std::mem::take(&mut app.pasted_contents),
        pending_images: std::mem::take(&mut app.pending_images),
        undo: std::mem::take(&mut app.input_undo_stack),
    };
    app.input = jcode_session_types::wrap_transcription(transcript);
    app.cursor_pos = app.input.len();
    stash
}

fn restore_draft_after_voice(app: &mut App, stash: StashedDraft) {
    // Submission consumes the composer. Anything left means it was kept (for
    // example an oversized prompt), which must not be silently replaced.
    if !app.input.is_empty() {
        return;
    }
    app.input = stash.input;
    app.cursor_pos = stash.cursor_pos.min(app.input.len());
    app.pasted_contents = stash.pasted_contents;
    app.pending_images = stash.pending_images;
    app.input_undo_stack = stash.undo;
    app.reset_tab_completion();
    app.sync_model_picker_preview_from_input();
}

/// Send a built-in voice transcript from a local (in-process) session. It is
/// wrapped in `<transcription>` tags, sent as soon as possible like Enter
/// (steering an active turn), and the typed draft is left untouched.
pub(in crate::tui::app) fn submit_voice_transcript(app: &mut App, transcript: &str) {
    let stash = stash_draft_for_voice(app, transcript);
    submit_transcript_input(app);
    restore_draft_after_voice(app, stash);
    app.follow_chat_bottom_for_typing();
}

/// Remote-session counterpart of [`submit_voice_transcript`].
pub(in crate::tui::app) async fn submit_remote_voice_transcript(
    app: &mut App,
    remote: &mut RemoteConnection,
    transcript: &str,
) -> Result<()> {
    let stash = stash_draft_for_voice(app, transcript);
    let result = submit_remote_transcript_input(app, remote).await;
    restore_draft_after_voice(app, stash);
    app.follow_chat_bottom_for_typing();
    result
}

pub(in crate::tui::app) async fn apply_remote_transcript_event(
    app: &mut App,
    remote: &mut RemoteConnection,
    text: String,
    mode: TranscriptMode,
) -> Result<()> {
    if text.trim().is_empty() {
        app.set_status_notice("Transcript was empty");
        return Ok(());
    }

    match mode {
        TranscriptMode::Send => {
            let text = transcript_send_text(&text);
            input::insert_input_text(app, &text);
            submit_remote_transcript_input(app, remote).await?;
        }
        _ => apply_transcript_event(app, text, mode),
    }

    app.follow_chat_bottom_for_typing();
    Ok(())
}

/// Stage a submitted turn for the remote tick loop when the app is attached to
/// a remote session, returning true when it took ownership of the turn.
///
/// Only the LOCAL run loop consumes `App::pending_turn`. Any path that reaches
/// `App::submit_input` while remote (a slash command that turned out not to be
/// one, an unknown skill fallback, a staged prompt) would otherwise set a flag
/// nobody dispatches, freezing the client in "Sending" forever. Queueing hands
/// the turn to `process_remote_followups`, which also echoes the user message.
pub(in crate::tui::app) fn stage_turn_for_remote_tick_loop(app: &mut App, input: &str) -> bool {
    if !app.is_remote {
        return false;
    }
    if app.is_processing && !app.queue_mode {
        let images = app.take_pending_image_payloads();
        input::stage_local_interleave(app, input.to_string(), images);
        return true;
    }
    app.queued_messages.push(input.to_string());
    app.pending_images.clear();
    true
}

#[cfg(test)]
mod ssh_slash_tests {
    use super::*;

    #[test]
    fn ssh_slash_capability_matrix_covers_entire_registry() {
        let matrix = include_str!("../../../../../../docs/ssh-slash-commands.md");
        for command in app_mod::registered_command_names() {
            assert!(
                matrix.contains(&format!("`{command}`")),
                "missing {command}"
            );
        }
    }

    #[test]
    fn ssh_alternate_slash_input_uses_wire_dispatch_not_agent_prompt() {
        if app_mod::commands_dispatch::ssh_test_runs_in_child(
            "ssh_alternate_slash_input_uses_wire_dispatch_not_agent_prompt",
        ) {
            return;
        }
        let mut app = app_mod::tests::create_test_app();
        app.is_remote = true;
        let runtime = tokio::runtime::Runtime::new().unwrap();
        runtime.block_on(async {
            use tokio::io::AsyncBufReadExt;
            let mut remote = RemoteConnection::dummy();
            remote.mark_history_loaded();
            let mut peer = tokio::io::BufReader::new(remote.take_dummy_peer().unwrap());
            for (command, expected) in [
                ("/compact", "compact"),
                ("/rename remote title", "rename_session"),
                ("/memory on", "set_feature"),
                ("/model refresh", "refresh_models"),
                ("/git status", "input_shell"),
                ("/auth", "runtime_command"),
                ("/refresh-model-list", "runtime_command"),
                ("/logout openai", "runtime_command"),
                ("/context", "runtime_command"),
                ("/overnight status", "runtime_command"),
            ] {
                submit_remote_slash_input(
                    &mut app,
                    &mut remote,
                    input::PreparedInput {
                        raw_input: command.to_string(),
                        expanded: command.to_string(),
                        images: Vec::new(),
                    },
                )
                .await
                .unwrap();
                let mut line = String::new();
                tokio::time::timeout(std::time::Duration::from_secs(1), peer.read_line(&mut line))
                    .await
                    .unwrap()
                    .unwrap();
                let request: serde_json::Value = serde_json::from_str(&line).unwrap();
                assert_eq!(request["type"], expected, "{command}: {line}");
                if command == "/git status" {
                    assert_eq!(request["command"], "git status --short --branch");
                }
                if command == "/logout openai" {
                    assert_eq!(request["command"], "logout");
                    assert_eq!(request["args"], serde_json::json!(["openai"]));
                }
                assert!(!app.pending_turn);
            }
            let next_id = remote.next_request_id_for_test();
            let images = vec![("image/png".to_string(), "attachment".to_string())];
            submit_remote_slash_input(
                &mut app,
                &mut remote,
                input::PreparedInput {
                    raw_input: "/local".to_string(),
                    expanded: "/local".to_string(),
                    images: images.clone(),
                },
            )
            .await
            .unwrap();
            assert_eq!(app.input, "/local");
            assert_eq!(app.pending_images, images);
            assert_eq!(remote.next_request_id_for_test(), next_id);
        });
    }

    #[test]
    fn ssh_host_loop_and_usage_commands_send_structured_requests() {
        if app_mod::commands_dispatch::ssh_test_runs_in_child(
            "ssh_host_loop_and_usage_commands_send_structured_requests",
        ) {
            return;
        }
        let mut app = app_mod::tests::create_test_app();
        app.is_remote = true;
        let runtime = tokio::runtime::Runtime::new().unwrap();
        runtime.block_on(async {
            use tokio::io::{AsyncBufReadExt, BufReader};
            let mut remote = RemoteConnection::dummy();
            let peer = remote.take_dummy_peer().unwrap();
            let (reader, _writer) = peer.into_split();
            let mut reader = BufReader::new(reader);
            for (input, command, args) in [
                ("/config view", "config", vec!["view"]),
                ("/fast default off", "fast_default", vec!["off"]),
                ("/open README.md", "open", vec!["README.md"]),
                ("/poke status", "poke", vec!["status"]),
                ("/improve plan", "improve", vec!["plan"]),
                ("/refactor stop", "refactor", vec!["stop"]),
                ("/fix", "fix", vec![]),
                ("/usage", "ssh_account", vec!["usage"]),
                (
                    "/reset usage limits openai",
                    "ssh_account",
                    vec!["reset", "usage", "limits", "openai"],
                ),
            ] {
                submit_remote_slash_input(
                    &mut app,
                    &mut remote,
                    input::PreparedInput {
                        raw_input: input.into(),
                        expanded: input.into(),
                        images: Vec::new(),
                    },
                )
                .await
                .unwrap();
                let mut line = String::new();
                tokio::time::timeout(Duration::from_secs(1), reader.read_line(&mut line))
                    .await
                    .unwrap()
                    .unwrap();
                let request: serde_json::Value = serde_json::from_str(&line).unwrap();
                assert_eq!(request["type"], "runtime_command", "{input}");
                assert_eq!(request["command"], command, "{input}");
                assert_eq!(request["args"], serde_json::json!(args), "{input}");
                assert!(app.pending_prompt_before_history.is_none(), "{input}");
                assert!(!app.pending_turn, "{input}");
            }
        });
    }

    #[test]
    fn ssh_registered_unimplemented_command_fails_closed_but_skills_reach_server() {
        if app_mod::commands_dispatch::ssh_test_runs_in_child(
            "ssh_registered_unimplemented_command_fails_closed_but_skills_reach_server",
        ) {
            return;
        }
        let mut app = app_mod::tests::create_test_app();
        app.is_remote = true;
        let runtime = tokio::runtime::Runtime::new().unwrap();
        runtime.block_on(async {
            let mut remote = RemoteConnection::dummy();
            // No history makes normal remote prompts queue without a wire
            // send, providing a direct distinction from rejected commands.
            for command in [
                "/desktop",
                "/workspace",
                "/ssh other-host",
                "/onboarding-sim",
                "/update-rehearsal",
            ] {
                let next_id = remote.next_request_id_for_test();
                submit_remote_slash_input(
                    &mut app,
                    &mut remote,
                    input::PreparedInput {
                        raw_input: command.to_string(),
                        expanded: command.to_string(),
                        images: Vec::new(),
                    },
                )
                .await
                .unwrap();
                assert!(app.pending_prompt_before_history.is_none(), "{command}");
                assert_eq!(remote.next_request_id_for_test(), next_id);
                assert!(
                    app.display_messages()
                        .last()
                        .unwrap()
                        .content
                        .contains("SSH")
                );
            }
            for command in [
                "/future remote skill /context",
                "/home/vm/notes",
                "/improve-custom-skill",
                "/plan-custom-skill",
                "/refactor-custom-skill",
            ] {
                submit_remote_slash_input(
                    &mut app,
                    &mut remote,
                    input::PreparedInput {
                        raw_input: command.to_string(),
                        expanded: command.to_string(),
                        images: Vec::new(),
                    },
                )
                .await
                .unwrap();
                assert_eq!(
                    app.pending_prompt_before_history.take().unwrap().expanded,
                    command
                );
                assert!(!app.pending_turn);
            }
        });
    }
}
