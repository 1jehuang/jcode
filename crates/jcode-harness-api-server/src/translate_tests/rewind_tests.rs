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

#[test]
fn assistant_rewind_translation_keeps_kind_and_completion_correlation() {
    let mut state = BridgeState {
        session_id: Some("s1".into()),
        ..Default::default()
    };
    let output = state.api_request_to_legacy(
        &json!({"id": 21, "req": "list_rewind_checkpoints", "session_id": "s1"}),
    );
    let [Outbound::Legacy(list)] = &output[..] else {
        panic!("expected list request")
    };
    let checkpoint = json!({"id": "assistant-cp", "target_kind": "assistant_message", "message_index": 3, "prompt": "assistant answer", "can_restore_files": true, "changed_file_count": 1});
    let replies = state.legacy_event_to_api(&json!({"type": "rewind_checkpoints", "id": list["id"], "session_id": "s1", "checkpoints": [checkpoint.clone()]}));
    let [frame] = &replies[..] else {
        panic!("expected checkpoint list reply")
    };
    assert_eq!(frame.reply_to, Some(21));
    assert_eq!(
        serde_json::to_value(frame).unwrap()["checkpoints"][0],
        checkpoint
    );

    let output = state.api_request_to_legacy(&json!({"id": 22, "req": "rewind_checkpoint", "session_id": "s1", "checkpoint_id": "assistant-cp", "mode": "conversation_only"}));
    let [Outbound::Legacy(rewind)] = &output[..] else {
        panic!("expected rewind request")
    };
    assert_eq!(rewind["checkpoint_id"], "assistant-cp");
    let history = state.legacy_event_to_api(&json!({"type": "history", "id": rewind["id"], "session_id": "s1", "messages": [{"role": "assistant", "content": "assistant answer"}]}));
    assert!(
        matches!(&history[..], [ServerFrame { reply_to: None, event: ApiEvent::History { messages, .. }, .. }] if messages.len() == 1 && messages[0].role == "assistant")
    );
    let result = json!({"target_kind": "assistant_message", "mode": "conversation_only", "prompt": "", "removed_messages": 2, "restored_files": 0});
    let complete = json!({"type": "rewind_complete", "id": rewind["id"], "session_id": "s1", "result": result.clone()});
    let replies = state.legacy_event_to_api(&complete);
    let [frame] = &replies[..] else {
        panic!("expected typed completion reply")
    };
    assert_eq!(frame.reply_to, Some(22));
    assert!(matches!(&frame.event, ApiEvent::RewindComplete { .. }));
    assert_eq!(serde_json::to_value(frame).unwrap()["result"], result);
    assert!(state.legacy_event_to_api(&complete).is_empty());
}
