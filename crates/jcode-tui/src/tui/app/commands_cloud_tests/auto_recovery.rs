use super::*;

#[test]
fn cloud_auto_recovery_explicit_cancel_and_retry_control_diagnosis() {
    let mut app = crate::tui::app::tests::create_test_app();
    let session = super::super::super::commands::active_session_id(&app);
    show_cloud_failure(&mut app, "/cloud box", &session, Some(1), "handoff failed");
    assert!(handle_cloud_command(&mut app, "/cloud cancel", &session));
    assert!(!app.has_pending_cloud_recovery());
    assert!(app.cloud_recovery_reminder().is_some());
    assert!(handle_cloud_command(&mut app, "/cloud box", &session));
    app.pending_cloud_handoff = None;
    show_cloud_failure(&mut app, "/cloud box", &session, Some(1), "retry failed");
    assert!(app.has_pending_cloud_recovery());
}

#[test]
fn cloud_auto_recovery_survives_offline_send_and_dispatches_after_reconnect() {
    let rt = tokio::runtime::Runtime::new().unwrap();
    let _guard = rt.enter();
    let mut app = crate::tui::app::tests::create_test_app();
    app.is_remote = true;
    let session = super::super::super::commands::active_session_id(&app);
    show_cloud_failure(
        &mut app,
        "/cloud box",
        &session,
        Some(1),
        "server disconnected",
    );
    let mut offline = crate::tui::backend::RemoteConnection::dummy();
    offline.mark_history_loaded();
    drop(offline.take_dummy_peer());
    rt.block_on(super::super::super::remote::process_remote_followups(
        &mut app,
        &mut offline,
    ));
    assert!(app.has_pending_cloud_recovery());
    assert!(!app.is_processing);
    assert!(app.hidden_queued_system_messages.is_empty());
    let mut reconnected = crate::tui::backend::RemoteConnection::dummy();
    reconnected.mark_history_loaded();
    app.is_processing = true;
    rt.block_on(super::super::super::remote::process_remote_followups(
        &mut app,
        &mut reconnected,
    ));
    assert!(
        app.has_pending_cloud_recovery(),
        "never interrupt a resumed turn"
    );
    app.is_processing = false;
    rt.block_on(super::super::super::remote::process_remote_followups(
        &mut app,
        &mut reconnected,
    ));
    assert!(app.is_processing);
    assert!(!app.has_pending_cloud_recovery());
}

#[test]
fn cloud_auto_recovery_waits_for_history_then_sends_hidden_diagnostics() {
    let rt = tokio::runtime::Runtime::new().unwrap();
    let _guard = rt.enter();
    let mut app = crate::tui::app::tests::create_test_app();
    app.is_remote = true;
    let session = super::super::super::commands::active_session_id(&app);
    app.input = "unsent draft".into();
    app.cursor_pos = 3;
    app.pending_images
        .push(("image/png".into(), "draft-image".into()));
    app.pasted_contents.push("draft paste".into());
    show_cloud_failure(&mut app, "/cloud box", &session, Some(1), "handoff failed");
    let mut remote = crate::tui::backend::RemoteConnection::dummy();
    rt.block_on(super::super::super::remote::process_remote_followups(
        &mut app,
        &mut remote,
    ));
    assert!(!app.is_processing);
    assert!(app.has_pending_cloud_recovery());
    remote.mark_history_loaded();
    rt.block_on(super::super::super::remote::process_remote_followups(
        &mut app,
        &mut remote,
    ));
    assert!(app.is_processing);
    assert!(!app.has_pending_cloud_recovery());
    assert!(app.cloud_recovery_context.is_none());
    let sent = app.rate_limit_pending_message.as_ref().unwrap();
    assert!(sent.is_system);
    assert!(sent.content.is_empty());
    assert!(sent.images.is_empty());
    assert!(
        sent.system_reminder
            .as_ref()
            .unwrap()
            .contains("handoff failed")
    );
    assert_eq!(app.input, "unsent draft");
    assert_eq!(app.cursor_pos, 3);
    assert_eq!(
        app.pending_images,
        vec![("image/png".into(), "draft-image".into())]
    );
    assert_eq!(app.pasted_contents, vec!["draft paste"]);
    assert!(
        !app.display_messages()
            .iter()
            .any(|message| message.role == "user")
    );
}

#[test]
fn cloud_auto_recovery_queued_user_turn_consumes_reminder_without_extra_turn() {
    let rt = tokio::runtime::Runtime::new().unwrap();
    let _guard = rt.enter();
    let mut app = crate::tui::app::tests::create_test_app();
    app.is_remote = true;
    let session = super::super::super::commands::active_session_id(&app);
    show_cloud_failure(&mut app, "/cloud box", &session, Some(1), "handoff failed");
    app.queued_messages.push("continue my work".into());
    let mut remote = crate::tui::backend::RemoteConnection::dummy();
    remote.mark_history_loaded();
    rt.block_on(super::super::super::remote::process_remote_followups(
        &mut app,
        &mut remote,
    ));
    let sent = app.rate_limit_pending_message.as_ref().unwrap();
    assert_eq!(sent.content, "continue my work");
    assert!(
        sent.system_reminder
            .as_ref()
            .unwrap()
            .contains("handoff failed")
    );
    assert!(!app.has_pending_cloud_recovery());
    let message_id = app.current_message_id;
    app.is_processing = false;
    rt.block_on(super::super::super::remote::process_remote_followups(
        &mut app,
        &mut remote,
    ));
    assert_eq!(app.current_message_id, message_id);
    assert!(!app.is_processing);
}

#[test]
fn cloud_auto_recovery_is_scoped_and_deduplicated() {
    let mut app = crate::tui::app::tests::create_test_app();
    let session = super::super::super::commands::active_session_id(&app);
    show_cloud_failure(&mut app, "/cloud box", &session, Some(1), "first failure");
    assert!(app.has_pending_cloud_recovery());
    show_cloud_failure(&mut app, "/cloud box", &session, Some(1), "updated failure");
    assert!(
        app.has_pending_cloud_recovery(),
        "duplicate must not disarm pending recovery"
    );
    assert!(
        app.take_cloud_recovery_reminder()
            .unwrap()
            .contains("updated failure")
    );
    show_cloud_failure(&mut app, "/cloud box", &session, Some(1), "same failure");
    assert!(
        !app.has_pending_cloud_recovery(),
        "never loop autonomous recovery"
    );
    assert!(
        app.cloud_recovery_reminder().is_some(),
        "manual turn still receives diagnostics"
    );
    show_cloud_failure(
        &mut app,
        "/cloud other",
        "another-session",
        Some(1),
        "other failure",
    );
    assert!(!app.has_pending_cloud_recovery());
}

#[test]
fn cloud_auto_recovery_local_tick_schedules_without_consuming_draft() {
    let mut app = crate::tui::app::tests::create_test_app();
    let session = super::super::super::commands::active_session_id(&app);
    show_cloud_failure(&mut app, "/cloud box", &session, Some(1), "handoff failed");
    app.input = "keep draft".into();
    app.pending_images
        .push(("image/png".into(), "keep image".into()));
    assert!(app.has_queued_followups());
    super::super::super::local::handle_tick(&mut app);
    assert!(app.pending_queued_dispatch);
    assert_eq!(app.input, "keep draft");
    assert_eq!(app.pending_images.len(), 1);
    assert!(!app.pending_turn);
}
