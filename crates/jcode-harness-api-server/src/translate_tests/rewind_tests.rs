use super::*;

#[test]
fn rewind_checkpoint_translation_keeps_history_before_typed_completion() {
    let mut state = BridgeState {
        session_id: Some("s1".into()),
        ..Default::default()
    };
    let output = state.api_request_to_legacy(
        &json!({"id": 12, "req": "list_rewind_checkpoints", "session_id": "s1"}),
    );
    let [Outbound::Legacy(list)] = &output[..] else {
        panic!("expected list request")
    };
    assert_eq!(list["type"], "list_rewind_checkpoints");
    let replies = state.legacy_event_to_api(&json!({"type": "rewind_checkpoints", "id": list["id"], "session_id": "s1", "checkpoints": []}));
    assert!(
        matches!(&replies[..], [ServerFrame { reply_to: Some(12), event: ApiEvent::RewindCheckpoints { checkpoints, .. }, .. }] if checkpoints.is_empty())
    );

    let output = state.api_request_to_legacy(&json!({"id": 13, "req": "rewind_checkpoint", "session_id": "s1", "checkpoint_id": "cp1", "mode": "conversation_only"}));
    let [Outbound::Legacy(rewind)] = &output[..] else {
        panic!("expected rewind request")
    };
    assert_eq!(rewind["checkpoint_id"], "cp1");
    assert_eq!(rewind["mode"], "conversation_only");
    let history = state.legacy_event_to_api(&json!({"type": "history", "id": rewind["id"], "session_id": "s1", "messages": [{"role": "user", "content": "before"}]}));
    assert!(
        matches!(history.first(), Some(ServerFrame { reply_to: None, event: ApiEvent::History { messages, .. }, .. }) if messages.len() == 1)
    );
    let complete = json!({"type": "rewind_complete", "id": rewind["id"], "session_id": "s1", "result": {"mode": "conversation_only", "prompt": "retry", "removed_messages": 2, "restored_files": 0}});
    let replies = state.legacy_event_to_api(&complete);
    assert!(
        matches!(&replies[..], [ServerFrame { reply_to: Some(13), event: ApiEvent::RewindComplete { result, .. }, .. }] if result.prompt == "retry")
    );
    assert!(state.legacy_event_to_api(&complete).is_empty());
}

#[test]
fn rewind_checkpoint_requests_require_the_target_attachment() {
    for req in ["list_rewind_checkpoints", "rewind_checkpoint"] {
        for attached in [None, Some("other".to_string())] {
            let mut state = BridgeState {
                session_id: attached,
                ..Default::default()
            };
            let output = state.api_request_to_legacy(&json!({"id": 1, "req": req, "session_id": "s1", "checkpoint_id": "cp", "mode": "files_only"}));
            assert!(matches!(
                &output[..],
                [Outbound::Reply(ServerFrame {
                    event: ApiEvent::Error { .. },
                    ..
                })]
            ));
        }
    }
}
