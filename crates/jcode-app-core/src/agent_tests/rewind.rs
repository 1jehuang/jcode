use super::*;

#[path = "checkpoint_input.rs"]
mod checkpoint_input;

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
    drop(reader);
    rewind.await.unwrap();
    assert!(compaction.read().await.persisted_state().is_none());
    assert!(agent.session.compaction.is_none());
    assert!(agent.rewind_checkpoints().unwrap().is_empty());
}
