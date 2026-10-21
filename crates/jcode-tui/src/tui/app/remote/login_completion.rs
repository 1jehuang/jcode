use super::*;

pub(super) async fn handle_saved_account_login_completed_event(
    app: &mut App,
    remote: &mut RemoteConnection,
    completion: crate::bus::SavedAccountLoginCompleted,
) -> bool {
    if crate::tui::is_ssh_remote() {
        return false;
    }
    let success = completion.login.success;
    let provider_hint = auth_provider_hint_for_login_provider(&completion.login.provider);
    let auth = auth_changed_event_for_login_provider(&completion.login.provider);
    let prefer_strongest = success && app.onboarding_should_prefer_strongest_model();
    // Capture before the handler consumes the event. Only a successful
    // Replace login migrates the server-side session pin/home.
    let migration = (success && completion.replace).then(|| {
        (
            completion.login.provider.clone(),
            completion.label.clone(),
            completion.old_identity.clone(),
            completion.new_identity.clone(),
        )
    });
    let handled = app.handle_saved_account_login_completed(completion);
    // Handled means origin window, attempt and verifier all matched. The
    // stored-identity check skips the move if a later replacement won.
    // The server request runs BEFORE the auth refresh notification below.
    if handled
        && let Some((provider_id, label, old_identity, new_identity)) = migration
        && App::saved_login_snapshot(&provider_id, &label).ok()
            == Some((true, new_identity.clone()))
        && let Err(error) = remote
            .replace_session_account_identity(
                &provider_id,
                &label,
                old_identity.as_deref(),
                new_identity.as_deref(),
            )
            .await
    {
        crate::logging::warn(&format!(
            "Login replaced but session pins could not be migrated on the server: {error}"
        ));
    }
    if handled
        && success
        && let Err(error) = remote
            .notify_auth_changed_event(provider_hint, auth, prefer_strongest)
            .await
    {
        crate::logging::warn(&format!(
            "Failed to notify server about refreshed auth: {error}"
        ));
        app.finish_auth_catalog_refresh();
        app.set_status_notice("Model setup will retry after reconnect");
    }
    handled
}

pub(super) async fn handle_login_completed_event(
    app: &mut App,
    remote: &mut RemoteConnection,
    login: crate::bus::LoginCompleted,
) -> bool {
    if crate::tui::is_ssh_remote() {
        app.set_status_notice("Local login does not change SSH server credentials");
        return true;
    }
    let success = login.success && login.provider != "copilot_code";
    let provider_hint = auth_provider_hint_for_login_provider(&login.provider);
    let auth = auth_changed_event_for_login_provider(&login.provider);
    let prefer_strongest = success && app.onboarding_should_prefer_strongest_model();
    app.handle_login_completed(login);
    if success
        && let Err(error) = remote
            .notify_auth_changed_event(provider_hint, auth, prefer_strongest)
            .await
    {
        crate::logging::warn(&format!(
            "Failed to notify server about refreshed auth: {error}"
        ));
        app.finish_auth_catalog_refresh();
        app.set_status_notice("Model setup will retry after reconnect");
    }
    true
}
