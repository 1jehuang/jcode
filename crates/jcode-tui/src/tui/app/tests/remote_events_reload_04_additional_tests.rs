use super::*;

#[test]
fn test_remote_fast_status_tracks_history_tier_including_explicit_off() {
    with_temp_jcode_home(|| {
        let rt = tokio::runtime::Runtime::new().unwrap();
        let _guard = rt.enter();
        let mut app = create_test_app();
        let mut remote = crate::tui::backend::RemoteConnection::dummy();
        app.is_remote = true;
        app.remote_session_id = Some("session_fast_status".to_string());
        remote.mark_history_loaded();

        // Bootstrap, catalog refresh, explicit off, then on again. An absent
        // tier is authoritative Standard, not an invitation to use the saved
        // default or preserve a stale priority badge.
        for tier in [Some("priority"), Some("priority"), None, Some("priority")] {
            let history = serde_json::from_value(serde_json::json!({
                "type": "history",
                "id": 1,
                "session_id": "session_fast_status",
                "messages": [],
                "provider_name": "OpenAI",
                "provider_model": "gpt-future-model",
                "service_tier": tier
            }))
            .expect("history fixture");
            app.handle_server_event(history, &mut remote);
            assert_eq!(app.remote_service_tier.as_deref(), tier);
            // Until the provider reports the applied tier, the widget is honest
            // that the requested tier is unconfirmed.
            assert_eq!(
                crate::tui::TuiState::info_widget_data(&app)
                    .service_tier
                    .as_deref(),
                tier.map(|_| "Fast ?")
            );

            app.input = "/fast status".to_string();
            rt.block_on(app.handle_remote_key(KeyCode::Enter, KeyModifiers::empty(), &mut remote))
                .expect("fast status");
            let status = &app
                .display_messages()
                .last()
                .expect("status message")
                .content;
            let expected = if tier.is_some() {
                "Fast mode is on."
            } else {
                "Fast mode is off."
            };
            assert!(status.starts_with(expected), "{status}");
        }
    });
}
