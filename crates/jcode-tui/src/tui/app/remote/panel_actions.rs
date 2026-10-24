use super::*;

pub(super) async fn dispatch_pending(app: &mut App, remote: &mut RemoteConnection) -> bool {
    if !remote.has_loaded_history() {
        return false;
    }
    let Some((expected_session, action, page_id)) = app.pending_panel_action.take() else {
        return false;
    };
    if app.active_client_session_id() != Some(expected_session.as_str()) {
        app.set_status_notice("Panel action skipped: session changed");
        return true;
    }
    if let Err(error) = remote
        .panel_action(&expected_session, &action, &page_id)
        .await
    {
        app.push_display_message(DisplayMessage::error(format!(
            "Couldn't {action} panel: {error}. Open /panels to retry."
        )));
    }
    true
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn queued_panel_action_does_not_follow_a_session_switch() {
        let mut app = crate::tui::app::tests::create_test_app();
        app.is_remote = true;
        app.remote_session_id = Some("new-session".into());
        app.pending_panel_action = Some(("old-session".into(), "purge".into(), "plan".into()));
        let runtime = tokio::runtime::Runtime::new().unwrap();
        let _entered = runtime.enter();
        let mut remote = RemoteConnection::dummy();
        remote.mark_history_loaded();
        assert!(runtime.block_on(dispatch_pending(&mut app, &mut remote)));
        assert!(app.pending_panel_action.is_none());
    }
}
