use super::*;

#[test]
fn rewind_checkpoint_helpers_wait_for_completion_after_history() {
    use jcode_sdk::{RewindCheckpoint, RewindMode, RewindResult};
    let checkpoint = RewindCheckpoint {
        id: "cp1".into(),
        message_index: 0,
        prompt: "original".into(),
        timestamp: None,
        can_restore_files: true,
        changed_file_count: 1,
    };
    let expected = checkpoint.clone();
    let client = fake_harness(move |frame, mut writer| {
        let event = match &frame.request {
            ApiRequest::ListRewindCheckpoints { session_id } => ApiEvent::RewindCheckpoints {
                session_id: session_id.clone(),
                checkpoints: vec![checkpoint.clone()],
            },
            ApiRequest::RewindCheckpoint {
                session_id,
                checkpoint_id,
                mode,
            } => {
                assert_eq!(checkpoint_id, "cp1");
                assert_eq!(*mode, RewindMode::ConversationOnly);
                write_frame(
                    &mut writer,
                    &ServerFrame::event(ApiEvent::History {
                        session_id: session_id.clone(),
                        messages: vec![],
                        images: vec![],
                    }),
                )
                .unwrap();
                ApiEvent::RewindComplete {
                    session_id: session_id.clone(),
                    result: RewindResult {
                        mode: *mode,
                        prompt: "original".into(),
                        removed_messages: 2,
                        restored_files: 0,
                        skipped_files: vec![],
                    },
                }
            }
            other => panic!("unexpected request {other:?}"),
        };
        reply(frame, event, writer);
    });
    let events = client.events(Some("s1"));
    assert_eq!(
        client.list_rewind_checkpoints("s1").unwrap(),
        vec![expected]
    );
    let result = client
        .rewind_checkpoint("s1", "cp1", RewindMode::ConversationOnly)
        .unwrap();
    assert_eq!(result.prompt, "original");
    assert_eq!(result.removed_messages, 2);
    let mut saw_history = false;
    while let Some(event) = events.next_timeout(Duration::from_millis(10)) {
        saw_history |= matches!(event, ApiEvent::History { .. });
    }
    assert!(saw_history);
}
