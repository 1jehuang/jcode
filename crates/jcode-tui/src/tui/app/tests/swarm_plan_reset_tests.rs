#[test]
fn test_history_session_change_clears_swarm_plan_state_and_plan_graph_does_not_reappear() {
    // Wiring-audit claim 3: the History server event clears swarm_plan_items
    // (server_events.rs ~1637) on session change and the plan-graph chat
    // message does not reappear from the restored history.
    let mut app = create_test_app();
    let rt = tokio::runtime::Runtime::new().unwrap();
    let _guard = rt.enter();
    let mut remote = crate::tui::backend::RemoteConnection::dummy();
    remote.mark_history_loaded();

    app.remote_session_id = Some("session_same".to_string());
    app.handle_server_event(
        swarm_plan_event(3, vec![swarm_plan_graph_item("haiku-1", "write a haiku")]),
        &mut remote,
    );
    assert!(!app.swarm_plan_items.is_empty());
    assert_eq!(plan_graph_titles(&app).len(), 1);

    // Same-session history refresh does NOT clear the plan snapshot or the
    // inline diagram (the clearing block is scoped to session_changed).
    app.handle_server_event(history_event_for_session("session_same"), &mut remote);
    assert!(
        !app.swarm_plan_items.is_empty(),
        "same-session history refresh keeps swarm_plan_items"
    );
    assert_eq!(
        plan_graph_titles(&app).len(),
        1,
        "same-session history refresh keeps the inline plan graph message"
    );

    // Session-changing history clears the plan snapshot and the diagram does
    // not come back from the (empty) restored history.
    app.handle_server_event(history_event_for_session("session_other"), &mut remote);
    assert!(
        app.swarm_plan_items.is_empty(),
        "session-change history must clear swarm_plan_items"
    );
    assert_eq!(app.swarm_plan_version, None);
    assert_eq!(app.swarm_plan_swarm_id, None);
    assert!(
        plan_graph_titles(&app).is_empty(),
        "plan graph message must not reappear after history restore: {:?}",
        plan_graph_titles(&app)
    );
}

/// Path 1: local `/clear` (commands.rs -> reset_current_session at
/// commands_review.rs). A full transcript discard: it now also clears the
/// process-global ACTIVE_DIAGRAMS registry, so neither the pinned pane nor
/// the Margin info widget can keep showing a diagram from the old
/// transcript, including its swarm plan snapshot.
#[test]
fn test_local_clear_command_clears_active_diagrams_and_swarm_plan_state() {
    let _render_lock = scroll_render_test_lock();
    let _mode_guard = DiagramModeOverrideGuard::pinned();
    let mut app = create_test_app();
    app.diagram_mode = crate::config::DiagramDisplayMode::Pinned;
    app.diagram_pane_enabled = true;
    let rt = tokio::runtime::Runtime::new().unwrap();
    let _guard = rt.enter();
    let mut remote = crate::tui::backend::RemoteConnection::dummy();
    remote.mark_history_loaded();

    let _stale_hash = seed_rendered_plan_graph(&mut app, &mut remote);

    assert!(super::commands::handle_session_command(&mut app, "/clear"));

    assert_full_discard_clears_diagrams_and_plan_state(
        &mut app,
        "local /clear (reset_current_session)",
    );

    crate::tui::mermaid::clear_active_diagrams();
}

/// Path 4: remote `/clear` (remote/key_handling.rs). A full transcript
/// discard like path 1: the server session is cleared, so the registry is
/// re-scoped too, along with the old session's swarm plan snapshot.
#[test]
fn test_remote_clear_command_clears_active_diagrams_and_swarm_plan_state() {
    let _render_lock = scroll_render_test_lock();
    let _mode_guard = DiagramModeOverrideGuard::pinned();
    let mut app = create_test_app();
    app.diagram_mode = crate::config::DiagramDisplayMode::Pinned;
    app.diagram_pane_enabled = true;
    app.is_remote = true;
    let rt = tokio::runtime::Runtime::new().unwrap();
    let _guard = rt.enter();
    let mut remote = crate::tui::backend::RemoteConnection::dummy();
    remote.mark_history_loaded();

    let _stale_hash = seed_rendered_plan_graph(&mut app, &mut remote);

    app.input = "/clear".to_string();
    app.cursor_pos = app.input.len();
    rt.block_on(app.handle_remote_key(KeyCode::Enter, KeyModifiers::empty(), &mut remote))
        .expect("remote /clear should succeed");
    assert_eq!(
        crate::tui::TuiState::status_notice(&app).as_deref(),
        Some("Session cleared"),
        "remote /clear path executed"
    );

    assert_full_discard_clears_diagrams_and_plan_state(&mut app, "remote /clear");

    crate::tui::mermaid::clear_active_diagrams();
}
