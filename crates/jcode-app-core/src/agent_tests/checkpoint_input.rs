use super::*;

struct TestHome {
    previous: Option<std::ffi::OsString>,
    _directory: tempfile::TempDir,
}

impl TestHome {
    fn new() -> Self {
        let directory = tempfile::tempdir().unwrap();
        let previous = std::env::var_os("JCODE_HOME");
        crate::env::set_var("JCODE_HOME", directory.path());
        crate::config::Config::invalidate_cache();
        Self {
            previous,
            _directory: directory,
        }
    }
}

impl Drop for TestHome {
    fn drop(&mut self) {
        match self.previous.take() {
            Some(home) => crate::env::set_var("JCODE_HOME", home),
            None => crate::env::remove_var("JCODE_HOME"),
        }
        crate::config::Config::invalidate_cache();
    }
}

#[tokio::test]
async fn automatic_input_does_not_fail_checkpoint_binding() {
    let _env = crate::storage::lock_test_env();
    let _home = TestHome::new();
    let mut agent = Agent::new(Arc::new(BoundSavedAccountProvider), Registry::empty());
    let prompts = [
        crate::todo::build_auto_poke_message(2),
        crate::todo::TODO_LONG_SESSION_REVIEW_MESSAGE.to_string(),
        "<system-reminder>Continue after reload.</system-reminder>".into(),
        "[Scheduled task]\nRun the scheduled check.".into(),
        "[NOTIFICATION]\nYou received 1 notification(s) from other agents working in this codebase:\n\nReady.".into(),
    ];
    for prompt in prompts {
        agent
            .append_user_context_message(&prompt, vec![])
            .unwrap_or_else(|error| panic!("automatic input was rejected: {error}: {prompt}"));
    }
    assert!(agent.rewind_checkpoints().unwrap().is_empty());
    let boundary = agent.session.messages.len();
    agent
        .append_user_context_message("real human follow-up", vec![])
        .unwrap();
    let checkpoints = agent.rewind_checkpoints().unwrap();
    assert_eq!(checkpoints.len(), 1);
    assert_eq!(checkpoints[0].prompt, "real human follow-up");
    assert_eq!(checkpoints[0].message_index, boundary);
    assert!(checkpoints[0].can_restore_files);
}

struct ContinuationProvider(Arc<std::sync::atomic::AtomicUsize>);

#[async_trait]
impl Provider for ContinuationProvider {
    async fn complete(
        &self,
        _: &[Message],
        _: &[ToolDefinition],
        _: &str,
        _: Option<&str>,
    ) -> Result<EventStream> {
        self.0.fetch_add(1, std::sync::atomic::Ordering::SeqCst);
        Ok(Box::pin(futures::stream::iter([
            Ok(StreamEvent::TextDelta("continued".into())),
            Ok(StreamEvent::MessageEnd {
                stop_reason: Some("end_turn".into()),
            }),
        ])))
    }
    fn name(&self) -> &str {
        "checkpoint-continuation-fixture"
    }
    fn fork(&self) -> Arc<dyn Provider> {
        Arc::new(Self(self.0.clone()))
    }
}

#[tokio::test]
async fn automatic_continuations_reach_provider_through_all_turn_entry_points() {
    let _env = crate::storage::lock_test_env();
    let _home = TestHome::new();
    let calls = Arc::new(std::sync::atomic::AtomicUsize::new(0));
    let mut agent = Agent::new(
        Arc::new(ContinuationProvider(calls.clone())),
        Registry::empty(),
    );
    let prompt = crate::todo::build_auto_poke_message(2);
    agent.run_once(&prompt).await.unwrap();
    assert_eq!(agent.run_once_capture(&prompt).await.unwrap(), "continued");
    let (tx, mut rx) = tokio_mpsc::unbounded_channel();
    agent
        .run_once_streaming_mpsc(&prompt, vec![], None, tx)
        .await
        .unwrap();
    assert!(
        rx.try_recv().is_ok(),
        "streaming continuation must emit events"
    );
    assert_eq!(calls.load(std::sync::atomic::Ordering::SeqCst), 3);
    let targets = agent.rewind_checkpoints().unwrap();
    assert_eq!(
        targets.len(),
        3,
        "visible assistant replies remain targetable"
    );
    assert!(
        targets.iter().all(|target| {
            target.target_kind == jcode_session_types::RewindTargetKind::AssistantMessage
                && !target.can_restore_files
        }),
        "automatic inputs must not become human/file checkpoints"
    );
}

#[tokio::test]
async fn automatic_inputs_do_not_evict_human_snapshots_or_lose_image_boundaries() {
    let _env = crate::storage::lock_test_env();
    let _home = TestHome::new();
    let mut agent = Agent::new(Arc::new(BoundSavedAccountProvider), Registry::empty());
    agent
        .append_user_context_message("keep this checkpoint", vec![])
        .unwrap();
    let prompt = crate::todo::build_auto_poke_message(2);
    for _ in 0..105 {
        agent.append_user_context_message(&prompt, vec![]).unwrap();
    }
    let images = vec![("image/png".into(), "AA==".into())];
    let boundary = agent.session.messages.len();
    agent
        .append_user_context_message("", images.clone())
        .unwrap();
    let targets = agent.rewind_checkpoints().unwrap();
    assert_eq!(targets.len(), 2);
    assert!(
        targets
            .iter()
            .all(|checkpoint| checkpoint.can_restore_files)
    );
    assert_eq!(targets[1].message_index, boundary);
    let restored = agent
        .rewind_checkpoint(
            &targets[1].id,
            jcode_session_types::RewindMode::ConversationOnly,
        )
        .await
        .unwrap();
    assert_eq!(restored.images, images);
    assert_eq!(agent.session.messages.len(), boundary);
}
