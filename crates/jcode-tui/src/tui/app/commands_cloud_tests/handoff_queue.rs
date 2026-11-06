use super::*;

#[test]
fn queued_system_reminders_reach_agent_before_handoff() {
    let mut app = crate::tui::app::tests::create_test_app();
    let session = crate::tui::app::commands::active_session_id(&app);
    app.hidden_queued_system_messages
        .push("bg task done".into());
    handle_cloud_command(&mut app, "/local", &session);
    assert!(app.pending_cloud_handoff.is_some());
    // Reminders still dispatch while the handoff waits.
    assert!(!app.cloud_handoff_blocks_work());
    assert!(app.poll_cloud_job());
    assert!(app.pending_queued_dispatch);
    assert!(app.cloud_job.is_none());
    assert_eq!(
        app.hidden_queued_system_messages,
        vec!["bg task done".to_string()]
    );
    assert!(app.pending_cloud_handoff.is_some());

    // Once delivered, the handoff proceeds and blocks new work again.
    app.pending_queued_dispatch = false;
    app.hidden_queued_system_messages.clear();
    assert!(app.cloud_handoff_blocks_work());
    assert!(app.take_ready_cloud_handoff().is_some());
}

#[test]
fn prepared_handoff_interrupts_running_turn_once() {
    let mut app = crate::tui::app::tests::create_test_app();
    let session = crate::tui::app::commands::active_session_id(&app);
    let mut request = pending(CloudCommand::Move { host: None }, &session);
    app.is_processing = true;
    app.pending_cloud_handoff = Some(request.clone());
    app.poll_cloud_job_interrupt_only_for_test();
    assert!(!app.cancel_requested, "unprepared host must not interrupt");

    request.prepared = true;
    app.pending_cloud_handoff = Some(request);
    app.poll_cloud_job_interrupt_only_for_test();
    assert!(app.cancel_requested, "ready host interrupts the turn");
    assert!(app.take_cloud_remote_interrupt());
    assert!(!app.cancel_requested, "remote mode clears the local flag");

    app.poll_cloud_job_interrupt_only_for_test();
    assert!(!app.take_cloud_remote_interrupt(), "interrupts only once");
}
