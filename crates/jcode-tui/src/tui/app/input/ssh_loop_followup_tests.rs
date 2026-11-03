#[test]
fn ssh_loop_turn_end_never_reads_or_acknowledges_laptop_todo_state() {
    if super::super::commands_dispatch::ssh_test_runs_in_child(
        "ssh_loop_turn_end_never_reads_or_acknowledges_laptop_todo_state",
    ) {
        return;
    }
    let mut app = super::super::tests::create_test_app();
    app.is_remote = true;
    app.remote_session_id = Some("session_server_owned_loop".into());
    app.auto_poke_default_on = true;
    app.auto_poke_incomplete_todos = true;
    let observation = crate::todo::GateObservation {
        kind: crate::todo::GateObservationKind::IntentUnderstanding,
        group: None,
        state: Some("partial".into()),
    };
    crate::todo::save_gate_observations(
        "session_server_owned_loop",
        std::slice::from_ref(&observation),
    )
    .unwrap();
    let local_id = app.session.id.clone();
    let messages = app.session.messages.clone();
    assert!(!app.schedule_turn_end_followups_after_response(true));
    assert!(!app.schedule_auto_poke_followup_if_needed());
    assert_eq!(app.session.id, local_id);
    assert_eq!(app.session.messages.len(), messages.len());
    assert!(!app.has_queued_followups());
    assert_eq!(
        crate::todo::load_gate_observations("session_server_owned_loop").unwrap(),
        vec![observation]
    );
}
