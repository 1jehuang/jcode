use super::*;

pub(super) async fn send_pending_route(app: &mut App, remote: &mut RemoteConnection) {
    let verification = match app.claude_cli_route_verification() {
        Ok(verification) => verification,
        Err(error) => {
            app.pending_route_selection = None;
            app.fail_claude_cli_route_send(&error.to_string());
            return;
        }
    };
    if let Some(selection) = app.pending_route_selection.take() {
        app.cancel_pending_provider_failover("Provider auto-switch canceled");
        app.pending_model_switch = None;
        match remote
            .set_route_selection_verified(selection.clone(), verification)
            .await
        {
            Ok(id) => {
                // Manual fallback offers use unrestricted SetRoute,
                // but need the same exact acknowledgement fence as
                // automatic failover before releasing their resend.
                if app.pending_fallback_resend.is_some() {
                    app.pending_automatic_failover_route = Some(selection);
                    app.pending_automatic_failover_request_id = Some(id);
                    app.pending_automatic_failover_started = Some(Instant::now());
                }
                app.note_claude_cli_route_sent(id);
                forward_pending_reasoning_effort(app, remote).await;
            }
            Err(error) => {
                app.pending_reasoning_effort = None;
                if let Some(payload) = app.pending_fallback_resend.take() {
                    app.restore_failover_payload(payload);
                }
                app.push_display_message(DisplayMessage::error(format!(
                    "Failed to request model switch: {}",
                    error
                )));
                app.fail_claude_cli_route_send(&error.to_string());
            }
        }
    }
}

pub(super) async fn handle_cli_login(
    app: &mut App,
    remote: &mut RemoteConnection,
    mut login: crate::bus::LoginCompleted,
) {
    if crate::tui::is_ssh_remote() {
        app.set_status_notice("Local login does not change SSH server credentials");
        return;
    }
    if !app.accept_claude_cli_completion(&mut login) {
        if app.claude_cli_route_queued() {
            send_pending_route(app, remote).await;
        }
        return;
    }
    let success = login.success;
    let prefer_strongest = success && app.onboarding_should_prefer_strongest_model();
    app.handle_login_completed(login);
    if success
        && let Err(error) = remote
            .notify_auth_changed_event(
                Some("claude".into()),
                auth_changed_event_for_login_provider("claude"),
                prefer_strongest,
            )
            .await
    {
        crate::logging::warn(&format!(
            "Failed to notify server about completed Claude login: {error}"
        ));
        app.finish_auth_catalog_refresh();
        app.set_status_notice("Model setup will retry after reconnect");
    }
}
