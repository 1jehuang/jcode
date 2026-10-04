#![cfg_attr(test, allow(clippy::await_holding_lock))]
#![cfg_attr(test, allow(clippy::await_holding_lock))]

use super::*;
#[test]
fn clone_split_session_uses_persisted_session_state() {
    let _guard = crate::storage::lock_test_env();
    let temp = tempfile::tempdir().expect("tempdir");
    let prev_home = std::env::var_os("JCODE_HOME");
    crate::env::set_var("JCODE_HOME", temp.path());

    let mut parent = crate::session::Session::create_with_id(
        "session_parent_split_test".to_string(),
        None,
        None,
    );
    parent.working_dir = Some("/tmp/jcode-split-test".to_string());
    parent.model = Some("gpt-test".to_string());
    parent.system_prompt = Some("forked system prompt".into());
    parent.add_message(
        Role::User,
        vec![ContentBlock::Text {
            text: "hello from parent".to_string(),
            cache_control: None,
        }],
    );
    parent.compaction = Some(crate::session::StoredCompactionState {
        summary_text: "summary".to_string(),
        openai_encrypted_content: None,
        covers_up_to_turn: 1,
        original_turn_count: 1,
        compacted_count: 1,
    });
    parent.save().expect("save parent");

    let mut unsaved_parent = parent.clone();
    unsaved_parent.model = Some("unsaved-model".into());
    unsaved_parent.system_prompt = Some("unsaved prompt".into());
    unsaved_parent.add_message(
        Role::Assistant,
        vec![ContentBlock::Text {
            text: "unfinished turn".into(),
            cache_control: None,
        }],
    );
    let (child_id, _child_name) =
        clone_split_session(&parent.id, Some(&unsaved_parent)).expect("clone split");
    let child = crate::session::Session::load(&child_id).expect("load child");

    assert_eq!(child.parent_id.as_deref(), Some(parent.id.as_str()));
    assert_eq!(child.system_prompt, parent.system_prompt);
    assert_eq!(
        child.messages.len(),
        parent.messages.len() + 1,
        "fork should inherit the transcript plus one fork notice"
    );
    assert_eq!(
        child.messages[0].content_preview(),
        parent.messages[0].content_preview()
    );
    let fork_notice = child.messages.last().expect("fork notice message");
    assert_eq!(
        fork_notice.display_role,
        Some(crate::session::StoredDisplayRole::System),
        "fork notice must be hidden from the visible transcript"
    );
    let fork_notice_text = fork_notice.content_preview();
    assert!(
        fork_notice_text.contains("forked") && fork_notice_text.contains(parent.id.as_str()),
        "fork notice should mention the parent session: {fork_notice_text}"
    );
    assert_eq!(child.compaction, parent.compaction);
    assert_eq!(child.working_dir, parent.working_dir);
    assert_eq!(child.model, parent.model);
    assert_eq!(child.status, crate::session::SessionStatus::Closed);
    assert_ne!(child.id, parent.id);

    if let Some(prev_home) = prev_home {
        crate::env::set_var("JCODE_HOME", prev_home);
    } else {
        crate::env::remove_var("JCODE_HOME");
    }
}

#[tokio::test]
async fn split_empty_live_session_without_persisted_parent() {
    let _guard = crate::storage::lock_test_env();
    let _home = SplitTestHome::new();
    let agent = new_split_test_agent().await;
    let parent = agent.lock().await.session_for_split().clone();
    assert_eq!(parent.visible_conversation_message_count(), 0);
    assert!(
        !crate::session::session_exists(&parent.id),
        "regression requires an unsaved parent"
    );
    let (tx, mut rx) = mpsc::unbounded_channel();

    handle_split(17, &parent.id, &agent, &tx).await;
    let child = split_response(&mut rx, 17);
    assert_ne!(child.id, parent.id);
    assert_eq!(child.parent_id.as_deref(), Some(parent.id.as_str()));
    assert_eq!(child.working_dir, parent.working_dir);
    assert_eq!(child.model, parent.model);
    assert_eq!(child.status, crate::session::SessionStatus::Closed);
    assert_eq!(child.messages.len(), parent.messages.len() + 1);
    let notice = child.messages.last().unwrap();
    assert_eq!(
        notice.display_role,
        Some(crate::session::StoredDisplayRole::System)
    );
    assert!(notice.content_preview().contains(&parent.id));
    assert_eq!(agent.lock().await.session_id(), parent.id);
    assert!(
        !crate::session::session_exists(&parent.id),
        "fork must not mutate/persist its parent"
    );
}

#[tokio::test]
async fn split_busy_session_uses_persisted_state_without_waiting_for_agent() {
    let _guard = crate::storage::lock_test_env();
    let _home = SplitTestHome::new();
    let agent = new_split_test_agent().await;
    let mut busy = agent.lock().await;
    let mut parent = busy.session_for_split().clone();
    parent.add_message(
        Role::User,
        vec![ContentBlock::Text {
            text: "persisted request".into(),
            cache_control: None,
        }],
    );
    parent.save().expect("save pre-turn snapshot");
    busy.add_message(
        Role::Assistant,
        vec![ContentBlock::Text {
            text: "unsaved streaming output".into(),
            cache_control: None,
        }],
    );
    let (tx, mut rx) = mpsc::unbounded_channel();

    timeout(
        Duration::from_millis(100),
        handle_split(18, &parent.id, &agent, &tx),
    )
    .await
    .expect("split must not wait on the held streaming Agent lock");
    let child = split_response(&mut rx, 18);
    assert_eq!(child.messages.len(), parent.messages.len() + 1);
    assert_eq!(
        child.messages[0].content_preview(),
        parent.messages[0].content_preview()
    );
    assert!(
        !child
            .messages
            .iter()
            .any(|m| m.content_preview().contains("unsaved streaming output"))
    );
    assert!(
        child
            .messages
            .last()
            .unwrap()
            .content_preview()
            .contains("forked")
    );
    assert!(
        agent.try_lock().is_err(),
        "parent lock is still owned by the busy turn"
    );
    drop(busy);
}

#[tokio::test]
async fn split_busy_unsaved_session_returns_error_without_waiting() {
    let _guard = crate::storage::lock_test_env();
    let _home = SplitTestHome::new();
    let agent = new_split_test_agent().await;
    let busy = agent.lock().await;
    let parent_id = busy.session_id().to_owned();
    assert!(!crate::session::session_exists(&parent_id));
    let (tx, mut rx) = mpsc::unbounded_channel();
    timeout(
        Duration::from_millis(100),
        handle_split(19, &parent_id, &agent, &tx),
    )
    .await
    .expect("missing snapshot must not block a busy session");
    assert!(matches!(
        rx.try_recv(),
        Ok(ServerEvent::Error { id: 19, .. })
    ));
    assert!(rx.try_recv().is_err());
    drop(busy);
}

#[test]
fn split_missing_parent_never_uses_another_live_session() {
    let _guard = crate::storage::lock_test_env();
    let _home = SplitTestHome::new();
    let other = crate::session::Session::create(None, None);
    assert!(clone_split_session("session_missing_parent", Some(&other)).is_err());
}

#[test]
fn split_corrupt_persisted_parent_is_not_hidden_by_live_fallback() {
    let _guard = crate::storage::lock_test_env();
    let _home = SplitTestHome::new();
    let mut parent = crate::session::Session::create(None, Some("persisted parent".into()));
    parent.save().expect("create snapshot");
    let path = crate::session::session_path(&parent.id).unwrap();
    std::fs::write(&path, b"invalid session JSON").unwrap();
    assert!(clone_split_session(&parent.id, Some(&parent)).is_err());
    assert_eq!(std::fs::read(&path).unwrap(), b"invalid session JSON");
}
