use super::*;
use crate::session::{Session, StoredCompactionState, StoredDisplayRole};
use jcode_session_types::RewindTargetKind;

fn text(value: &str) -> ContentBlock {
    ContentBlock::Text {
        text: value.into(),
        cache_control: None,
    }
}

fn ids(session: &Session) -> Vec<String> {
    session
        .messages
        .iter()
        .map(|message| message.id.clone())
        .collect()
}

fn compacted(count: usize) -> StoredCompactionState {
    StoredCompactionState {
        summary_text: "future summary".into(),
        openai_encrypted_content: Some("future reasoning".into()),
        covers_up_to_turn: 1,
        original_turn_count: 1,
        compacted_count: count,
    }
}

#[test]
fn assistant_targets_share_rendered_identity_and_deduplicate_split_entries() {
    let _env = crate::storage::lock_test_env();
    let (_home, _work) = fixture();
    let mut session = Session::create(None, None);
    let input = session.add_message(Role::User, vec![text("human")]);
    let tool_only = session.add_message(
        Role::Assistant,
        vec![ContentBlock::ToolUse {
            id: "call".into(),
            name: "read".into(),
            input: serde_json::json!({}),
            thought_signature: None,
        }],
    );
    session.add_message(
        Role::User,
        vec![ContentBlock::ToolResult {
            tool_use_id: "call".into(),
            content: "result".into(),
            is_error: None,
        }],
    );
    let split = session.add_message(
        Role::Assistant,
        vec![
            text("before"),
            ContentBlock::ToolResult {
                tool_use_id: "native".into(),
                content: "inline result".into(),
                is_error: None,
            },
            text("after"),
        ],
    );
    let hidden = session.add_message_with_display_role(
        Role::Assistant,
        vec![text("hidden answer")],
        Some(StoredDisplayRole::System),
    );
    let background = session.add_message_with_display_role(
        Role::Assistant,
        vec![text("background")],
        Some(StoredDisplayRole::BackgroundTask),
    );
    let targets = session.rewind_checkpoints().unwrap();
    assert_eq!(targets.len(), 2);
    assert_eq!(targets[0].id, format!("message:{input}"));
    assert_eq!(targets[0].target_kind, RewindTargetKind::UserPrompt);
    assert_eq!(targets[1].id, format!("assistant:{split}"));
    assert_eq!(targets[1].target_kind, RewindTargetKind::AssistantMessage);
    assert!(!targets[1].can_restore_files);
    assert_eq!(targets[1].changed_file_count, 0);
    assert_eq!(session.rewind_target_stored_indices(), vec![0, 3, 3]);
    assert_eq!(session.rewind_stored_len_for_message(2), Some(4));
    assert_eq!(session.rewind_stored_len_for_message(3), Some(4));
    assert_eq!(session.rewind_stored_len_for_assistant(&split), Some(4));
    assert_eq!(session.rewind_stored_len_for_message(0), None);
    assert_eq!(session.rewind_stored_len_for_message(4), None);
    for id in [input, tool_only, hidden, background] {
        assert_eq!(session.rewind_stored_len_for_assistant(&id), None);
    }
}

#[tokio::test]
async fn assistant_restore_matches_legacy_inclusive_prefix_and_is_durable() {
    let _env = crate::storage::lock_test_env();
    let (_home, work) = fixture();
    let path = work.path().join("tracked");
    let mut session = Session::create(None, None);
    user(&mut session, "first");
    let answer = session.add_message(Role::Assistant, vec![text("kept answer")]);
    let prefix = ids(&session);
    user(&mut session, "removed prompt");
    session.add_message(Role::Assistant, vec![text("discarded answer")]);
    mutate(&session.id, &path, Some(b"keep filesystem edits")).unwrap();
    session.save().unwrap();
    let original = session.messages.clone();
    let numeric_boundary = session.rewind_stored_len_for_message(2).unwrap();
    session
        .rewind_conversation_to_len(numeric_boundary)
        .unwrap();
    assert_eq!(ids(&session), prefix);
    session.restore_rewind_messages(original).unwrap();
    session.provider_session_id = Some("discarded provider".into());
    session.model_usage_turn_id = Some("discarded usage turn".into());
    session.compaction = Some(compacted(1));
    let result = session
        .rewind_checkpoint(&format!("assistant:{answer}"), RewindMode::ConversationOnly)
        .await
        .unwrap();
    assert_eq!(result.target_kind, RewindTargetKind::AssistantMessage);
    assert_eq!(result.removed_messages, 2);
    assert_eq!(result.restored_files, 0);
    assert!(result.prompt.is_empty());
    assert!(result.images.is_empty());
    assert_eq!(ids(&session), prefix);
    assert!(session.compaction.is_none());
    assert!(session.provider_session_id.is_none());
    assert!(session.model_usage_turn_id.is_none());
    assert_eq!(fs::read(path).unwrap(), b"keep filesystem edits");
    let loaded = Session::load(&session.id).unwrap();
    assert_eq!(ids(&loaded), prefix);
    assert!(loaded.compaction.is_none());
    assert!(loaded.provider_session_id.is_none());
    let snapshot = crate::session::session_path(&session.id).unwrap();
    assert!(!snapshot.with_extension("rewind-pending.json").exists());
    assert!(!crate::session::session_journal_path_from_snapshot(&snapshot).exists());
}

#[tokio::test]
async fn assistant_file_modes_and_removed_branch_ids_refuse_without_mutation() {
    let _env = crate::storage::lock_test_env();
    let (_home, work) = fixture();
    let path = work.path().join("tracked");
    let mut session = Session::create(None, None);
    let first = user(&mut session, "first");
    let answer = session.add_message(Role::Assistant, vec![text("same answer")]);
    user(&mut session, "future");
    mutate(&session.id, &path, Some(b"unchanged")).unwrap();
    let target = format!("assistant:{answer}");
    let before = serde_json::to_value(&session).unwrap();
    for mode in [RewindMode::FilesOnly, RewindMode::ConversationAndFiles] {
        let error = session.rewind_checkpoint(&target, mode).await.unwrap_err();
        assert!(error.contains("conversation-only"));
        assert_eq!(serde_json::to_value(&session).unwrap(), before);
        assert_eq!(fs::read(&path).unwrap(), b"unchanged");
    }
    session
        .rewind_checkpoint(&first.id, RewindMode::ConversationOnly)
        .await
        .unwrap();
    user(&mut session, "first");
    let replacement = session.add_message(Role::Assistant, vec![text("same answer")]);
    assert_ne!(replacement, answer);
    let before = ids(&session);
    assert!(
        session
            .rewind_checkpoint(&target, RewindMode::ConversationOnly)
            .await
            .is_err()
    );
    assert_eq!(ids(&session), before);
    assert_eq!(fs::read(path).unwrap(), b"unchanged");
}

#[tokio::test]
async fn assistant_transcript_undo_preserves_hidden_user_boundary_and_file_capability() {
    let _env = crate::storage::lock_test_env();
    let (_home, work) = fixture();
    let path = work.path().join("tracked");
    fs::write(&path, b"original").unwrap();
    let mut session = Session::create(None, None);
    user(&mut session, "first");
    mutate(&session.id, &path, Some(b"kept edit")).unwrap();
    let answer = session.add_message(Role::Assistant, vec![text("kept answer")]);
    let checkpoint = session.create_rewind_checkpoint("future").unwrap();
    let boundary = session.messages.len();
    session.add_message_with_display_role(
        Role::User,
        vec![text("late hidden context")],
        Some(StoredDisplayRole::System),
    );
    let input = session.add_message(Role::User, vec![text("future")]);
    session
        .bind_rewind_checkpoint(&checkpoint.id, &input)
        .unwrap();
    mutate(&session.id, &path, Some(b"future edit")).unwrap();
    session.save().unwrap();
    let original = session.messages.clone();
    session
        .rewind_checkpoint(&format!("assistant:{answer}"), RewindMode::ConversationOnly)
        .await
        .unwrap();
    assert!(
        !session
            .rewind_checkpoints()
            .unwrap()
            .iter()
            .any(|cp| cp.id == format!("message:{input}"))
    );
    assert!(
        session
            .rewind_checkpoint(&checkpoint.id, RewindMode::FilesOnly)
            .await
            .is_err()
    );
    assert_eq!(fs::read(&path).unwrap(), b"future edit");
    session.restore_rewind_messages(original).unwrap();
    assert_eq!(
        fs::read(&path).unwrap(),
        b"future edit",
        "transcript undo never undoes files"
    );
    let future = session
        .rewind_checkpoints()
        .unwrap()
        .into_iter()
        .find(|cp| cp.id == format!("message:{input}"))
        .unwrap();
    assert_eq!(future.message_index, boundary);
    assert!(future.can_restore_files);
    session
        .rewind_checkpoint(&future.id, RewindMode::FilesOnly)
        .await
        .unwrap();
    assert_eq!(fs::read(path).unwrap(), b"kept edit");
    let result = session
        .rewind_checkpoint(&future.id, RewindMode::ConversationOnly)
        .await
        .unwrap();
    assert_eq!(result.target_kind, RewindTargetKind::UserPrompt);
    assert_eq!(result.prompt, "future");
    assert_eq!(
        session.messages.len(),
        boundary,
        "original hidden context is removed too"
    );
}

#[test]
fn assistant_picker_includes_old_compacted_history_without_changing_numeric_window() {
    let _env = crate::storage::lock_test_env();
    let (_home, _work) = fixture();
    let mut session = Session::create(None, None);
    let mut first_answer = String::new();
    for index in 0..80 {
        session.add_message(Role::User, vec![text(&format!("prompt {index}"))]);
        let answer = session.add_message(Role::Assistant, vec![text(&format!("answer {index}"))]);
        if index == 0 {
            first_answer = answer;
        }
    }
    session.compaction = Some(compacted(140));
    let numeric = session.rewind_target_stored_indices();
    assert!(!numeric.contains(&1));
    assert_eq!(
        session.rewind_stored_len_for_assistant(&first_answer),
        Some(2)
    );
    let targets = session.rewind_checkpoints().unwrap();
    assert_eq!(targets.len(), 160);
    assert_eq!(targets[1].id, format!("assistant:{first_answer}"));
    assert_eq!(session.rewind_target_stored_indices(), numeric);
}

#[test]
fn transcriptless_stub_cannot_restore_undo_or_overwrite_durable_history() {
    let _env = crate::storage::lock_test_env();
    let (_home, _work) = fixture();
    let mut session = Session::create(None, None);
    user(&mut session, "prompt");
    session.add_message(Role::Assistant, vec![text("answer")]);
    session.save().unwrap();
    let original = session.messages.clone();
    let original_ids = ids(&session);
    session.strip_transcript_for_remote_client();
    let before = serde_json::to_value(&session).unwrap();
    assert!(
        session
            .rewind_conversation_to_len(session.messages.len())
            .unwrap_err()
            .to_string()
            .contains("transcript-less")
    );
    assert!(
        session
            .restore_rewind_messages(original)
            .unwrap_err()
            .to_string()
            .contains("transcript-less")
    );
    assert_eq!(serde_json::to_value(&session).unwrap(), before);
    assert_eq!(ids(&Session::load(&session.id).unwrap()), original_ids);
}

#[tokio::test]
async fn last_assistant_is_a_true_noop_and_does_not_drop_compaction() {
    let _env = crate::storage::lock_test_env();
    let (_home, _work) = fixture();
    let mut session = Session::create(None, None);
    user(&mut session, "prompt");
    let answer = session.add_message(Role::Assistant, vec![text("last answer")]);
    session.compaction = Some(compacted(1));
    session.provider_session_id = Some("still current".into());
    session.save().unwrap();
    let before = serde_json::to_value(&session).unwrap();
    let result = session
        .rewind_checkpoint(&format!("assistant:{answer}"), RewindMode::ConversationOnly)
        .await
        .unwrap();
    assert_eq!(result.removed_messages, 0);
    assert_eq!(result.target_kind, RewindTargetKind::AssistantMessage);
    assert_eq!(serde_json::to_value(&session).unwrap(), before);
}
