use super::*;

#[test]
fn streaming_hash_default_fallback_invalidates_same_length_frame() {
    let mut state = TestState {
        status: ProcessingStatus::Streaming,
        streaming_text: "stream-alpha".to_string(),
        ..Default::default()
    };
    let first_hash = state.streaming_text_hash();
    assert_eq!(first_hash, hash_text_for_cache(state.streaming_text()));
    let first = prepare::prepare_messages(&state, 110, 30);
    state.streaming_text = "stream-bravo".to_string();
    assert_ne!(first_hash, state.streaming_text_hash());
    assert_eq!(
        state.streaming_text_hash(),
        hash_text_for_cache(state.streaming_text())
    );
    let second = prepare::prepare_messages(&state, 110, 30);
    assert!(!Arc::ptr_eq(&first, &second));
    let rendered = second
        .materialize_all_lines()
        .iter()
        .map(extract_line_text)
        .collect::<Vec<_>>()
        .join("\n");
    assert!(rendered.contains("stream-bravo"), "{rendered}");
    assert!(!rendered.contains("stream-alpha"), "{rendered}");
}

#[test]
fn test_prepare_messages_places_live_swarm_card_beneath_matching_spawn_tool_call() {
    let session_id = "spawned-session-123";
    let state = TestState {
        display_messages: vec![DisplayMessage {
            role: "tool".to_string(),
            content: format!("Spawned new agent: {session_id}"),
            tool_calls: Vec::new(),
            duration_secs: None,
            title: None,
            tool_data: Some(ToolCall {
                id: "call-spawn".to_string(),
                name: "swarm".to_string(),
                input: serde_json::json!({
                    "action": "spawn",
                    "label": "API reviewer",
                    "prompt": "Review the authentication flow"
                }),
                intent: Some("Spawn an authentication reviewer".to_string()),
                thought_signature: None,
            }),
        }],
        swarm_members: vec![chat_swarm_member(session_id)],
        anim_elapsed: 0.16,
        ..Default::default()
    };

    let prepared = prepare::prepare_messages(&state, 110, 30);
    let rendered = prepared
        .materialize_all_lines()
        .iter()
        .map(extract_line_text)
        .collect::<Vec<_>>();
    let tool_row = rendered
        .iter()
        .position(|line| line.contains("Spawn an authentication reviewer"))
        .expect("missing swarm tool row");
    let card_row = rendered
        .iter()
        .position(|line| line.contains("🐄 ● API reviewer"))
        .expect("missing live member card");
    let all = rendered.join("\n");

    assert_eq!(
        card_row,
        tool_row + 1,
        "card must directly follow tool: {all}"
    );
    // Transcript cards are deliberately stable one-liners: no elapsed time,
    // todos, or tool progress (those live on the dedicated swarm page), so
    // old chat rows do not move while the user is reading them.
    assert!(
        rendered[card_row].contains("🐄 ● API reviewer · Working"),
        "stable card line missing: {all}"
    );
    assert!(
        !all.contains("00:18"),
        "elapsed time must not render in transcript cards: {all}"
    );
    assert!(
        !all.contains("Test token refresh flow"),
        "todos must not render in transcript cards: {all}"
    );
    assert!(
        !all.contains("Run targeted authentication tests"),
        "tool progress must not render in transcript cards: {all}"
    );
}
