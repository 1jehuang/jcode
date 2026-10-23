use super::*;

#[path = "checkpoint_input.rs"]
mod checkpoint_input;

struct AssistantRewindHome {
    _dir: tempfile::TempDir,
    previous: Option<std::ffi::OsString>,
}

impl AssistantRewindHome {
    fn new() -> Self {
        let dir = tempfile::tempdir().unwrap();
        let previous = std::env::var_os("JCODE_HOME");
        crate::env::set_var("JCODE_HOME", dir.path());
        crate::config::Config::invalidate_cache();
        Self {
            _dir: dir,
            previous,
        }
    }
}

impl Drop for AssistantRewindHome {
    fn drop(&mut self) {
        match self.previous.take() {
            Some(home) => crate::env::set_var("JCODE_HOME", home),
            None => crate::env::remove_var("JCODE_HOME"),
        }
        crate::config::Config::invalidate_cache();
    }
}

fn rewind_text(value: &str) -> ContentBlock {
    ContentBlock::Text {
        text: value.into(),
        cache_control: None,
    }
}

fn rewind_message_ids(agent: &Agent) -> Vec<String> {
    agent
        .session
        .messages
        .iter()
        .map(|message| message.id.clone())
        .collect()
}

#[tokio::test]
async fn assistant_picker_and_numeric_rewind_share_prefix_and_one_durable_undo_slot() {
    let _env = crate::storage::lock_test_env();
    let _home = AssistantRewindHome::new();
    let mut agent = Agent::new(Arc::new(BoundSavedAccountProvider), Registry::empty());
    let mut answers = Vec::new();
    for index in 0..3 {
        agent.add_message(Role::User, vec![rewind_text(&format!("prompt {index}"))]);
        answers.push(agent.add_message(
            Role::Assistant,
            vec![rewind_text(&format!("answer {index}"))],
        ));
    }
    agent.session.save().unwrap();
    let original = rewind_message_ids(&agent);
    let first_boundary = agent.session.rewind_stored_len_for_message(2).unwrap();
    assert_eq!(agent.rewind_to_message(2).unwrap(), 4);
    assert_eq!(rewind_message_ids(&agent), original[..first_boundary]);
    assert_eq!(agent.undo_rewind().unwrap(), 4);
    assert_eq!(rewind_message_ids(&agent), original);
    assert!(agent.undo_rewind().is_err());
    let second_boundary = agent.session.rewind_stored_len_for_message(4).unwrap();
    let result = agent
        .rewind_checkpoint(
            &format!("assistant:{}", answers[1]),
            jcode_session_types::RewindMode::ConversationOnly,
        )
        .await
        .unwrap();
    assert_eq!(
        result.target_kind,
        jcode_session_types::RewindTargetKind::AssistantMessage
    );
    assert!(result.prompt.is_empty());
    assert!(result.images.is_empty());
    assert_eq!(rewind_message_ids(&agent), original[..second_boundary]);
    assert!(agent.rewind_undo_snapshot.is_some());
    assert_eq!(agent.undo_rewind().unwrap(), 2);
    assert_eq!(rewind_message_ids(&agent), original);
    assert!(agent.provider_session_id.is_none());
    assert!(agent.session.provider_session_id.is_none());
    assert!(agent.provider_conversation_reset_pending);
    assert_eq!(
        Session::load(agent.session_id())
            .unwrap()
            .messages
            .iter()
            .map(|message| message.id.clone())
            .collect::<Vec<_>>(),
        original
    );
    assert_eq!(agent.rewind_to_message(5).unwrap(), 1);
    assert_eq!(
        agent.session.rewind_target_count(),
        5,
        "new numeric target after undo is recomputed"
    );
    assert_eq!(agent.undo_rewind().unwrap(), 1);
    assert_eq!(rewind_message_ids(&agent), original);
}

#[tokio::test]
async fn assistant_reset_failure_preserves_committed_prefix_and_transcript_undo() {
    let _env = crate::storage::lock_test_env();
    let _home = AssistantRewindHome::new();
    let mut agent = Agent::new(Arc::new(FailingConversationReset), Registry::empty());
    agent.add_message(Role::User, vec![rewind_text("prompt")]);
    let answer = agent.add_message(Role::Assistant, vec![rewind_text("kept")]);
    let retained = rewind_message_ids(&agent);
    agent.add_message(Role::User, vec![rewind_text("removed future")]);
    let original = rewind_message_ids(&agent);
    agent.session.save().unwrap();
    let error = agent
        .rewind_checkpoint(
            &format!("assistant:{answer}"),
            jcode_session_types::RewindMode::ConversationOnly,
        )
        .await
        .unwrap_err();
    assert!(error.contains("reset unavailable"));
    assert_eq!(rewind_message_ids(&agent), retained);
    assert!(agent.provider_conversation_reset_pending);
    assert!(agent.rewind_undo_snapshot.is_some());
    assert!(
        agent
            .run_turn(false)
            .await
            .unwrap_err()
            .to_string()
            .contains("reset unavailable")
    );
    agent.undo_rewind().unwrap();
    assert_eq!(rewind_message_ids(&agent), original);
    assert!(agent.provider_conversation_reset_pending);
    assert!(agent.rewind_undo_snapshot.is_none());
}

#[tokio::test]
async fn assistant_mixed_tool_boundary_uses_existing_missing_result_repair_once() {
    let _env = crate::storage::lock_test_env();
    let _home = AssistantRewindHome::new();
    let mut agent = Agent::new(Arc::new(BoundSavedAccountProvider), Registry::empty());
    agent.add_message(Role::User, vec![rewind_text("prompt")]);
    let answer = agent.add_message(
        Role::Assistant,
        vec![
            rewind_text("visible text before a tool"),
            ContentBlock::ToolUse {
                id: "rewind-paired-call".into(),
                name: "read".into(),
                input: serde_json::json!({}),
                thought_signature: None,
            },
        ],
    );
    let prefix = rewind_message_ids(&agent);
    agent.add_message(
        Role::User,
        vec![ContentBlock::ToolResult {
            tool_use_id: "rewind-paired-call".into(),
            content: "old result".into(),
            is_error: None,
        }],
    );
    agent.add_message(Role::Assistant, vec![rewind_text("final answer")]);
    agent.session.save().unwrap();
    agent
        .rewind_checkpoint(
            &format!("assistant:{answer}"),
            jcode_session_types::RewindMode::ConversationOnly,
        )
        .await
        .unwrap();
    assert_eq!(
        rewind_message_ids(&agent),
        prefix,
        "rewind keeps the exact legacy stored prefix"
    );
    assert_eq!(agent.repair_missing_tool_outputs(), 1);
    assert_eq!(agent.repair_missing_tool_outputs(), 0);
    let results: Vec<_> = agent
        .session
        .messages
        .iter()
        .flat_map(|message| &message.content)
        .filter_map(|block| match block {
            ContentBlock::ToolResult {
                tool_use_id,
                is_error,
                content,
            } if tool_use_id == "rewind-paired-call" => Some((is_error, content)),
            _ => None,
        })
        .collect();
    assert_eq!(results.len(), 1);
    assert_eq!(*results[0].0, Some(true));
    assert_eq!(results[0].1, TOOL_OUTPUT_MISSING_TEXT);
}

#[tokio::test]
async fn assistant_noop_and_file_mode_refusal_do_not_replace_existing_undo() {
    let _env = crate::storage::lock_test_env();
    let _home = AssistantRewindHome::new();
    let mut agent = Agent::new(Arc::new(BoundSavedAccountProvider), Registry::empty());
    agent.add_message(Role::User, vec![rewind_text("prompt")]);
    let answer = agent.add_message(Role::Assistant, vec![rewind_text("answer")]);
    agent.add_message(Role::User, vec![rewind_text("future")]);
    agent.session.save().unwrap();
    agent.rewind_to_message(2).unwrap();
    let snapshot_ids = agent
        .rewind_undo_snapshot
        .as_ref()
        .unwrap()
        .messages
        .iter()
        .map(|message| message.id.clone())
        .collect::<Vec<_>>();
    let result = agent
        .rewind_checkpoint(
            &format!("assistant:{answer}"),
            jcode_session_types::RewindMode::ConversationOnly,
        )
        .await
        .unwrap();
    assert_eq!(result.removed_messages, 0);
    for mode in [
        jcode_session_types::RewindMode::FilesOnly,
        jcode_session_types::RewindMode::ConversationAndFiles,
    ] {
        assert!(
            agent
                .rewind_checkpoint(&format!("assistant:{answer}"), mode)
                .await
                .is_err()
        );
    }
    assert_eq!(
        agent
            .rewind_undo_snapshot
            .as_ref()
            .unwrap()
            .messages
            .iter()
            .map(|message| message.id.clone())
            .collect::<Vec<_>>(),
        snapshot_ids
    );
}

struct FailingConversationReset;

#[async_trait]
impl Provider for FailingConversationReset {
    async fn complete(
        &self,
        _: &[Message],
        _: &[ToolDefinition],
        _: &str,
        _: Option<&str>,
    ) -> Result<EventStream> {
        panic!("a failed reset must block provider continuation")
    }
    async fn reset_conversation_state(&self) -> Result<()> {
        anyhow::bail!("reset unavailable")
    }
    fn name(&self) -> &str {
        "reset-test"
    }
    fn fork(&self) -> Arc<dyn Provider> {
        Arc::new(Self)
    }
}

#[tokio::test]
async fn rewind_reset_failure_keeps_gate_closed_on_next_turn() {
    let _env = crate::storage::lock_test_env();
    let home = tempfile::tempdir().unwrap();
    struct RestoreHome(Option<std::ffi::OsString>);
    impl Drop for RestoreHome {
        fn drop(&mut self) {
            match self.0.take() {
                Some(home) => crate::env::set_var("JCODE_HOME", home),
                None => crate::env::remove_var("JCODE_HOME"),
            }
            crate::config::Config::invalidate_cache();
        }
    }
    let _restore = RestoreHome(std::env::var_os("JCODE_HOME"));
    crate::env::set_var("JCODE_HOME", home.path());
    crate::config::Config::invalidate_cache();
    let mut agent = Agent::new(Arc::new(FailingConversationReset), Registry::empty());
    let checkpoint = agent
        .session
        .create_rewind_checkpoint("removed future")
        .unwrap();
    let retained_ids: Vec<_> = agent
        .session
        .messages
        .iter()
        .map(|message| message.id.clone())
        .collect();
    let input_id = agent.add_message(
        Role::User,
        vec![ContentBlock::Text {
            text: "removed future".into(),
            cache_control: None,
        }],
    );
    agent
        .session
        .bind_rewind_checkpoint(&checkpoint.id, &input_id)
        .unwrap();
    agent.session.save().unwrap();
    let error = agent
        .rewind_checkpoint(
            &checkpoint.id,
            jcode_session_types::RewindMode::ConversationOnly,
        )
        .await
        .unwrap_err();
    assert!(error.contains("reset unavailable"));
    assert!(agent.provider_conversation_reset_pending);
    assert_eq!(
        agent
            .session
            .messages
            .iter()
            .map(|message| message.id.clone())
            .collect::<Vec<_>>(),
        retained_ids
    );
    let error = agent.run_turn(false).await.unwrap_err();
    assert!(error.to_string().contains("reset unavailable"));
    assert!(agent.provider_conversation_reset_pending);
}

#[tokio::test]
async fn rewind_waits_for_compaction_lock_and_discards_future_summary() {
    let _env = crate::storage::lock_test_env();
    let home = tempfile::tempdir().unwrap();
    struct RestoreHome(Option<std::ffi::OsString>);
    impl Drop for RestoreHome {
        fn drop(&mut self) {
            if let Some(home) = self.0.take() {
                crate::env::set_var("JCODE_HOME", home);
            } else {
                crate::env::remove_var("JCODE_HOME");
            }
            crate::config::Config::invalidate_cache();
        }
    }
    let _restore = RestoreHome(std::env::var_os("JCODE_HOME"));
    crate::env::set_var("JCODE_HOME", home.path());
    crate::config::Config::invalidate_cache();
    let mut agent = Agent::new(Arc::new(BoundSavedAccountProvider), Registry::empty());
    let checkpoint = agent
        .session
        .create_rewind_checkpoint("discard me")
        .unwrap();
    let retained_message_count = checkpoint.message_index;
    let retained_ids: Vec<_> = agent
        .session
        .messages
        .iter()
        .map(|message| message.id.clone())
        .collect();
    let input_id = agent.add_message(
        Role::User,
        vec![ContentBlock::Text {
            text: "discard me".into(),
            cache_control: None,
        }],
    );
    agent
        .session
        .bind_rewind_checkpoint(&checkpoint.id, &input_id)
        .unwrap();
    agent.session.save().unwrap();
    let state = crate::session::StoredCompactionState {
        summary_text: "future summary must disappear".into(),
        openai_encrypted_content: Some("future native reasoning".into()),
        covers_up_to_turn: 1,
        original_turn_count: 1,
        compacted_count: 1,
    };
    let compaction = agent.registry.compaction();
    compaction
        .write()
        .await
        .restore_persisted_stored_state_with(&state, &agent.session.messages);
    agent.session.compaction = Some(state);
    let reader = compaction.read().await;
    assert!(reader.persisted_state().is_some());
    let mut rewind = Box::pin(agent.rewind_checkpoint(
        &checkpoint.id,
        jcode_session_types::RewindMode::ConversationOnly,
    ));
    assert!(
        tokio::time::timeout(Duration::from_millis(25), &mut rewind)
            .await
            .is_err(),
        "rewind must await the held compaction reader rather than skip reset"
    );
    assert!(reader.persisted_state().is_some());
    drop(rewind);
    assert!(
        agent.provider_conversation_reset_pending,
        "aborting at the manager lock must preserve the retry gate"
    );
    assert_eq!(
        agent.session.messages.len(),
        retained_message_count,
        "durable conversation rewind already committed"
    );
    assert_eq!(
        agent
            .session
            .messages
            .iter()
            .map(|message| message.id.clone())
            .collect::<Vec<_>>(),
        retained_ids
    );
    drop(reader);
    agent.ensure_provider_conversation_reset().await.unwrap();
    assert!(!agent.provider_conversation_reset_pending);
    assert!(compaction.read().await.persisted_state().is_none());
    assert!(agent.session.compaction.is_none());
    assert!(agent.rewind_checkpoints().unwrap().is_empty());
}

#[tokio::test]
async fn queued_human_steering_captures_separate_boundaries_and_keeps_prior_edits() {
    let _env = crate::storage::lock_test_env();
    let home = tempfile::tempdir().unwrap();
    struct RestoreHome(Option<std::ffi::OsString>);
    impl Drop for RestoreHome {
        fn drop(&mut self) {
            if let Some(home) = self.0.take() {
                crate::env::set_var("JCODE_HOME", home);
            } else {
                crate::env::remove_var("JCODE_HOME");
            }
            crate::config::Config::invalidate_cache();
        }
    }
    let _restore = RestoreHome(std::env::var_os("JCODE_HOME"));
    crate::env::set_var("JCODE_HOME", home.path());
    crate::config::Config::invalidate_cache();
    let mut agent = Agent::new(Arc::new(BoundSavedAccountProvider), Registry::empty());
    let path = home.path().join("tracked");
    std::fs::write(&path, b"original").unwrap();
    agent
        .append_user_context_message("initial work", vec![])
        .unwrap();
    crate::file_history::tracked_write(agent.session_id(), &path, b"wanted first edit")
        .await
        .unwrap();
    let image = ("image/png".to_string(), "aHVtYW4=".to_string());
    agent.queue_soft_interrupt_with_id(
        "steer".into(),
        vec![image.clone()],
        false,
        SoftInterruptSource::User,
        Some("human-one".into()),
    );
    agent.queue_soft_interrupt_with_id(
        "steer".into(),
        vec![],
        false,
        SoftInterruptSource::User,
        Some("human-two".into()),
    );
    agent.queue_soft_interrupt(
        "peer notification".into(),
        vec![],
        false,
        SoftInterruptSource::System,
    );
    let injected = agent.inject_soft_interrupts();
    assert_eq!(injected.len(), 3);
    assert_eq!(injected[0].client_ids, vec!["human-one"]);
    assert_eq!(injected[1].client_ids, vec!["human-two"]);
    let targets = agent.rewind_checkpoints().unwrap();
    assert_eq!(
        targets.len(),
        3,
        "initial prompt and each human steering input, no synthetic peer"
    );
    assert_eq!(targets[1].prompt, "steer");
    assert_eq!(targets[2].prompt, "steer");
    assert_ne!(targets[1].id, targets[2].id);
    crate::file_history::tracked_write(agent.session_id(), &path, b"unwanted later edit")
        .await
        .unwrap();
    let result = agent
        .rewind_checkpoint(
            &targets[1].id,
            jcode_session_types::RewindMode::ConversationAndFiles,
        )
        .await
        .unwrap();
    assert_eq!(result.prompt, "steer");
    assert_eq!(result.images, vec![image]);
    assert_eq!(std::fs::read(path).unwrap(), b"wanted first edit");
    let remaining = agent.rewind_checkpoints().unwrap();
    assert_eq!(remaining.len(), 1);
    assert_eq!(remaining[0].prompt, "initial work");
}
