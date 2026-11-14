//! Exercise assistant targets through the same daemon socket as user checkpoints.
use super::*;
use crate::protocol::{RewindCheckpoint, RewindTargetKind};

pub(super) async fn restore_and_undo(
    reader: &mut BufReader<crate::transport::ReadHalf>,
    writer: &mut crate::transport::WriteHalf,
    session_id: &str,
    checkpoints: &[RewindCheckpoint],
    file: &std::path::Path,
) {
    let assistants = checkpoints
        .iter()
        .filter(|point| point.target_kind == RewindTargetKind::AssistantMessage)
        .collect::<Vec<_>>();
    assert_eq!(assistants.len(), 2);
    let target = assistants[0];
    assert!(!target.can_restore_files);
    let original = Session::load(session_id).unwrap();
    let legacy_index = original
        .rewind_target_stored_indices()
        .into_iter()
        .find(|&index| format!("assistant:{}", original.messages[index].id) == target.id)
        .unwrap();
    let expected = original.messages[..=legacy_index]
        .iter()
        .map(|message| message.id.clone())
        .collect::<Vec<_>>();

    for (id, mode) in [(30, "files_only"), (31, "conversation_and_files")] {
        send_checkpoint_request(
            writer,
            json!({
                "type":"rewind_checkpoint", "id":id,
                "checkpoint_id":target.id, "mode":mode,
            }),
        )
        .await;
        checkpoint_event(
            reader,
            |event| matches!(event, ServerEvent::Error { id: reply, .. } if *reply == id),
        )
        .await;
        assert_eq!(
            Session::load(session_id).unwrap().messages.len(),
            original.messages.len()
        );
        assert_eq!(std::fs::read_to_string(file).unwrap(), "version two\n");
    }

    send_checkpoint_request(
        writer,
        json!({
            "type":"rewind_checkpoint", "id":32,
            "checkpoint_id":target.id, "mode":"conversation_only",
        }),
    )
    .await;
    let first = checkpoint_event(reader, |event| {
        matches!(
            event,
            ServerEvent::History { id: 32, .. }
                | ServerEvent::RewindComplete { id: 32, .. }
                | ServerEvent::Error { id: 32, .. }
        )
    })
    .await;
    assert!(
        matches!(first, ServerEvent::History { .. }),
        "history must precede assistant completion: {first:?}"
    );
    let ServerEvent::RewindComplete { result, .. } = checkpoint_event(reader, |event| {
        matches!(event, ServerEvent::RewindComplete { id: 32, .. })
    })
    .await
    else {
        unreachable!()
    };
    assert_eq!(result.target_kind, RewindTargetKind::AssistantMessage);
    assert!(
        result.prompt.is_empty(),
        "assistant prose is not a user draft"
    );
    assert!(result.images.is_empty());
    assert_eq!(result.restored_files, 0);
    let restored = Session::load(session_id).unwrap();
    assert_eq!(
        restored
            .messages
            .iter()
            .map(|message| message.id.clone())
            .collect::<Vec<_>>(),
        expected
    );
    assert_eq!(
        result.removed_messages,
        original.messages.len() - expected.len()
    );
    assert_eq!(std::fs::read_to_string(file).unwrap(), "version two\n");

    send_checkpoint_request(
        writer,
        json!({
            "type":"rewind_checkpoint", "id":33,
            "checkpoint_id":assistants[1].id, "mode":"conversation_only",
        }),
    )
    .await;
    checkpoint_event(reader, |event| {
        matches!(event, ServerEvent::Error { id: 33, .. })
    })
    .await;

    send_checkpoint_request(writer, json!({"type":"rewind_undo", "id":34})).await;
    let event = checkpoint_event(reader, |event| {
        matches!(
            event,
            ServerEvent::History { id: 34, .. } | ServerEvent::Error { id: 34, .. }
        )
    })
    .await;
    assert!(
        matches!(event, ServerEvent::History { .. }),
        "existing rewind undo must restore the picker rewind: {event:?}"
    );
    let undone = Session::load(session_id).unwrap();
    assert_eq!(
        undone
            .messages
            .iter()
            .map(|message| &message.id)
            .collect::<Vec<_>>(),
        original
            .messages
            .iter()
            .map(|message| &message.id)
            .collect::<Vec<_>>()
    );
    assert_eq!(std::fs::read_to_string(file).unwrap(), "version two\n");
}
