#[test]
fn side_panel_state_hydration_requires_correlated_attachment() {
    let mut state = BridgeState::default();
    let (_, mut reply) = recovery_attach(&mut state, Some("recover"));
    reply["side_panel"] = markdown_panel();
    let mut wrong = reply.clone();
    wrong["session_id"] = json!("other");
    assert!(state.legacy_event_to_api(&wrong).is_empty());
    wrong = reply.clone();
    wrong["id"] = json!(u64::MAX);
    assert!(state.legacy_event_to_api(&wrong).is_empty());
    assert_panel(
        &state.legacy_event_to_api(&reply),
        "recover",
        markdown_panel(),
    );
}

#[test]
fn side_panel_snapshot_identity_cannot_cross_the_attached_session() {
    let mut state = state_with_session();
    let mut snapshot = markdown_panel();
    snapshot["session_id"] = json!("s1");
    assert_panel(&state.legacy_event_to_api(&json!({"type":"side_panel_state", "snapshot":snapshot.clone()})), "s1", snapshot.clone());
    snapshot["session_id"] = json!("different-session");
    let frames = state.legacy_event_to_api(&json!({"type":"side_panel_state", "snapshot":snapshot}));
    assert!(!frames.iter().any(|frame| matches!(frame.event, ApiEvent::SidePanelState { .. })));
}
