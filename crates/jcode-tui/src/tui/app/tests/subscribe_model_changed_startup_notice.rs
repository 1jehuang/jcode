// Regression coverage for the subscribe-time `ServerEvent::ModelChanged`
// introduced by 48de3e706.
//
// Before that commit only `handle_resume_session` sent `ModelChanged`, so a
// brand-new session never received one and the client handler
// (crates/jcode-tui/src/tui/app/remote/server_events.rs:2299-2351) never ran on
// the startup path. 48de3e706 added an unconditional `ModelChanged` to
// `handle_subscribe` (crates/jcode-app-core/src/server/client_session.rs:898-922),
// so EVERY subscribe now replays the startup sequence
// SessionId -> ModelChanged -> Done.
//
// The handler treats any non-error `ModelChanged` as proof that a switch just
// happened: it pushes a "Switched to model: ..." transcript message and
// unconditionally sets a "Model -> ..." status notice. On a fresh session no
// switch happened, so both are spurious.

/// Feed the exact event sequence a brand-new session's `Subscribe` produces
/// (48de3e706, client_session.rs:898-922) into a fresh remote App and assert
/// nothing is announced to the user.
#[test]
fn test_new_session_subscribe_model_changed_announces_no_model_switch() {
    with_temp_jcode_home(|| {
        // `None` resume id = the launcher starting a brand-new session.
        let mut app = App::new_for_remote(None);
        let rt = tokio::runtime::Runtime::new().unwrap();
        let _guard = rt.enter();
        let mut remote = crate::tui::backend::RemoteConnection::dummy();

        // Precondition: the user never asked for a model switch.
        assert!(
            !app.remote_model_switch_in_flight,
            "a fresh session must not start with a switch in flight"
        );
        assert!(
            !app.auth_catalog_refresh_pending,
            "a fresh session is not in the post-login catalog refresh path"
        );

        app.handle_server_event(
            crate::protocol::ServerEvent::SessionId {
                session_id: "ses_launcher_fresh_01".to_string(),
            },
            &mut remote,
        );
        app.handle_server_event(
            crate::protocol::ServerEvent::ModelChanged {
                id: 1,
                model: "stealth/space-bunny-alpha".to_string(),
                provider_name: Some("Stealth".to_string()),
                context_window: Some(1_000_000),
                error: None,
                resolved_credential: None,
                reasoning_effort: Some("high".to_string()),
            },
            &mut remote,
        );
        app.handle_server_event(crate::protocol::ServerEvent::Done { id: 1 }, &mut remote);

        // The subscribe-time ModelChanged still carries the route the fix
        // wanted: the context window is what a launcher session previously
        // got wrong (200K instead of the real window).
        assert_eq!(
            app.remote_provider_model.as_deref(),
            Some("stealth/space-bunny-alpha")
        );

        let announced = app
            .display_messages()
            .iter()
            .map(|m| m.content.clone())
            .collect::<Vec<_>>();
        let notice = app.status_notice();
        // One assertion over both user-visible artifacts, so a failure reports
        // exactly what a brand-new session wrongly shows the user.
        let spurious = announced
            .iter()
            .any(|c| c.contains("Switched to model"))
            || notice
                .as_deref()
                .is_some_and(|text| text.starts_with("Model \u{2192}"));
        assert!(
            !spurious,
            "subscribe-time ModelChanged must not announce a switch; \
             transcript: {announced:?}, status notice: {notice:?}"
        );
    });
}

/// Companion guard: a switch the user actually asked for must keep its notice.
/// Any fix for the regression has to preserve this path.
#[test]
fn test_requested_model_switch_still_announces_the_switch() {
    with_temp_jcode_home(|| {
        let mut app = App::new_for_remote(None);
        let rt = tokio::runtime::Runtime::new().unwrap();
        let _guard = rt.enter();
        let mut remote = crate::tui::backend::RemoteConnection::dummy();

        app.remote_model_switch_in_flight = true;

        app.handle_server_event(
            crate::protocol::ServerEvent::ModelChanged {
                id: 2,
                model: "gpt-5.6-luna".to_string(),
                provider_name: Some("OpenAI".to_string()),
                context_window: Some(400_000),
                error: None,
                resolved_credential: None,
                reasoning_effort: None,
            },
            &mut remote,
        );

        assert!(
            app.display_messages()
                .iter()
                .any(|m| m.content.contains("Switched to model")),
            "a real switch must still announce itself in the transcript"
        );
        assert_eq!(
            app.status_notice(),
            Some("Model \u{2192} gpt-5.6-luna".to_string()),
            "a real switch must still set its status notice"
        );
        assert!(
            !app.remote_model_switch_in_flight,
            "the switch must be marked complete"
        );
    });
}