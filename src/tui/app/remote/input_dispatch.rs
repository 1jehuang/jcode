use super::super::{PendingRemoteMessage, PendingSplitPrompt};
use super::*;

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
    let msg_id = remote
        .send_message_with_images_and_reminder(
            content.clone(),
            images.clone(),
            system_reminder.clone(),
        )
        .await?;
    app.current_message_id = Some(msg_id);
    app.is_processing = true;
    app.status = ProcessingStatus::Sending;
    app.status_detail = None;
    app.processing_started = Some(Instant::now());
    app.last_stream_activity = Some(Instant::now());
    app.remote_resume_activity = None;
    app.reset_streaming_tps();
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

fn restore_prepared_remote_input(app: &mut App, prepared: input::PreparedInput) {
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

pub(in crate::tui::app) async fn submit_prepared_remote_input(
    app: &mut App,
    remote: &mut RemoteConnection,
    prepared: input::PreparedInput,
) -> Result<()> {
    if let Some(command) = input::extract_input_shell_command(&prepared.expanded) {
        submit_remote_input_shell(app, remote, prepared.raw_input, command.to_string()).await?;
        return Ok(());
    }

    app.commit_pending_streaming_assistant_message();
    app.push_display_message(DisplayMessage {
        role: "user".to_string(),
        content: prepared.raw_input,
        tool_calls: vec![],
        duration_secs: None,
        title: None,
        tool_data: None,
    });
    let _ = app
        .begin_remote_send(remote, prepared.expanded, prepared.images, false)
        .await;
    Ok(())
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

    if app.is_processing {
        app.pending_split_request = true;
        app.set_status_notice("Prompt queued for new session");
        return Ok(());
    }

    app.pending_split_request = false;
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
    app.last_stream_activity = None;
    app.reset_streaming_tps();
    app.current_message_id = None;
}

fn set_transcript_input(app: &mut App, text: String) {
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
            input::stage_local_interleave(app, prepared.expanded);
        }
    }
}

async fn submit_remote_transcript_input(
    app: &mut App,
    remote: &mut RemoteConnection,
) -> Result<()> {
    let trimmed = app.input.trim().to_string();
    if trimmed.is_empty() {
        app.set_status_notice("Transcript was empty");
        return Ok(());
    }

    if trimmed.starts_with('/') {
        app.submit_input();
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
            });
            app.begin_remote_send(remote, prepared.expanded, prepared.images, false)
                .await?;
        }
        SendAction::Queue => queue_transcript_input(app),
        SendAction::Interleave => {
            let prepared = input::take_prepared_input(app);
            app.send_interleave_now(prepared.expanded, remote).await;
        }
    }

    Ok(())
}

async fn submit_remote_input_shell(
    app: &mut App,
    remote: &mut RemoteConnection,
    raw_input: String,
    command: String,
) -> Result<()> {
    app.commit_pending_streaming_assistant_message();
    app.push_display_message(DisplayMessage::user(raw_input));

    if command.trim().is_empty() {
        app.push_display_message(DisplayMessage::system(
            "Shell command cannot be empty after `!`.",
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
