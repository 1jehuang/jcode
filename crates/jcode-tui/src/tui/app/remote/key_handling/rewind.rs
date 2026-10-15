use super::*;

pub(super) fn remote_rewindable_messages(app: &App) -> Vec<&DisplayMessage> {
    app.display_messages()
        .iter()
        .filter(|message| matches!(message.role.as_str(), "user" | "assistant"))
        .collect()
}

pub(super) fn show_remote_rewind_history(app: &mut App) {
    let rewindable = remote_rewindable_messages(app);
    if rewindable.is_empty() {
        app.push_display_message(DisplayMessage::system(
            "No messages in conversation.".to_string(),
        ));
        return;
    }

    let mut history = String::from("Conversation history:\n\n");
    for (i, msg) in rewindable.iter().enumerate() {
        let role_str = match msg.role.as_str() {
            "user" => "👤 User",
            "assistant" => "🤖 Assistant",
            _ => "💬 Message",
        };
        let preview = crate::util::truncate_str(&msg.content, 80);
        history.push_str(&format!("  {} {} - {}\n", i + 1, role_str, preview));
    }
    history.push_str("\nUse /rewind N to rewind to message N (removes all messages after).");
    history.push_str(" After rewinding, use /rewind undo to restore the removed messages.");
    app.push_display_message(DisplayMessage::system(history));
}

pub(super) async fn handle_remote_rewind_command(
    app: &mut App,
    remote: &mut RemoteConnection,
    trimmed: &str,
) -> Result<bool> {
    if trimmed == "/rewind" {
        show_remote_rewind_history(app);
        return Ok(true);
    }

    if trimmed == "/rewind undo" {
        remote.rewind_undo().await?;
        app.pending_remote_rewind_notice = Some(PendingRemoteRewindNotice {
            undo: true,
            message_index: None,
            changed_messages: 0,
        });
        app.set_status_notice("Undoing rewind...");
        return Ok(true);
    }

    let Some(num_str) = trimmed.strip_prefix("/rewind ") else {
        return Ok(false);
    };

    let message_count = remote_rewindable_messages(app).len();
    if message_count == 0 {
        app.push_display_message(DisplayMessage::system(
            "No messages in conversation.".to_string(),
        ));
        return Ok(true);
    }

    match num_str.trim().parse::<usize>() {
        Ok(n) if n > 0 && n <= message_count => {
            remote.rewind(n).await?;
            app.pending_remote_rewind_notice = Some(PendingRemoteRewindNotice {
                undo: false,
                message_index: Some(n),
                changed_messages: message_count - n,
            });
            app.set_status_notice(format!("Rewinding to message {}...", n));
        }
        Ok(n) => {
            app.push_display_message(DisplayMessage::error(format!(
                "Invalid message number: {}. Valid range: 1-{}",
                n, message_count
            )));
        }
        Err(_) => {
            app.push_display_message(DisplayMessage::error(format!(
                "Usage: /rewind N where N is a message number (1-{})",
                message_count
            )));
        }
    }

    Ok(true)
}
