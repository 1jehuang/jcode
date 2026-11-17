use super::*;

#[test]
fn reload_handoff_active_when_server_flag_is_set() {
    let state = RemoteRunState {
        server_reload_in_progress: true,
        ..RemoteRunState::default()
    };

    assert!(reconnect::reload_handoff_active(&state));
}

#[test]
fn client_focus_defaults_to_true() {
    let app = create_test_app();
    assert!(
        app.client_focused(),
        "a freshly created client should start focused so terminals that never \
         report focus events still animate/redraw normally"
    );
}

#[test]
fn presumed_focus_expires_only_while_unconfirmed() {
    let timeout = crate::tui::app::App::PRESUMED_FOCUS_TIMEOUT + std::time::Duration::from_secs(1);
    let old = std::time::Instant::now() - timeout;

    // Fresh client: focus is presumed and stays so inside the grace period.
    let mut app = create_test_app();
    app.expire_presumed_focus();
    assert!(app.client_focused());

    // Never touched and never told about focus: stop presuming.
    app.app_started = old;
    app.expire_presumed_focus();
    assert!(!app.client_focused());

    // Input proves focus and restores it immediately, for good.
    app.note_client_interaction();
    assert!(app.client_focused());
    app.expire_presumed_focus();
    assert!(app.client_focused());

    // A focus report alone also confirms, so a reporting terminal that says
    // "focused" is trusted however long the user only watches.
    let mut app = create_test_app();
    app.app_started = old;
    app.set_client_focused(true);
    app.expire_presumed_focus();
    assert!(app.client_focused());
}
