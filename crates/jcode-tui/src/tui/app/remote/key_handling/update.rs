use super::*;

pub(in crate::tui::app) fn handle_remote_char_input(app: &mut App, c: char) {
    input::handle_text_input(app, &c.to_string());
}

pub(in crate::tui::app) async fn send_interleave_now(
    app: &mut App,
    content: String,
    images: Vec<(String, String)>,
    remote: &mut RemoteConnection,
) {
    if content.trim().is_empty() {
        return;
    }
    let msg_clone = content.clone();
    let images_clone = images.clone();
    match remote.soft_interrupt(content, images, false).await {
        Err(e) => {
            app.push_display_message(DisplayMessage::error(format!(
                "Failed to send interleave: {}",
                e
            )));
        }
        Ok(request_id) => {
            app.track_pending_soft_interrupt_with_images(request_id, msg_clone, images_clone);
            app.set_status_notice("⏭ Interleave sent");
        }
    }
}

pub(in crate::tui::app) async fn handle_remote_update_command(
    app: &mut App,
    remote: &mut RemoteConnection,
    explicit_channel: Option<crate::config::UpdateChannel>,
) -> Result<()> {
    if crate::tui::is_ssh_remote() {
        app.set_status_notice("Update the client and SSH server separately, then reconnect");
        return Ok(());
    }
    if explicit_channel.is_none() {
        reload_stale_remote_server_before_update(app, remote).await?;
    }

    let session_id = app
        .remote_session_id
        .clone()
        .unwrap_or_else(|| crate::id::new_id("ses"));
    app.start_background_client_update_with_channel(session_id, explicit_channel);
    Ok(())
}

pub(in crate::tui::app) async fn reload_stale_remote_server_before_update(
    app: &mut App,
    remote: &mut RemoteConnection,
) -> Result<bool> {
    if app.remote_server_has_update != Some(true) {
        return Ok(false);
    }

    app.append_reload_message("Reloading stale server before checking for client updates...");
    remote.reload().await?;
    Ok(true)
}
