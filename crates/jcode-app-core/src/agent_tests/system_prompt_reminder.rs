use super::*;

type CapturedSystemPromptRequests = Arc<std::sync::Mutex<Vec<(String, Vec<Message>)>>>;

#[derive(Clone, Default)]
struct SystemPromptCaptureProvider {
    requests: CapturedSystemPromptRequests,
}

#[async_trait]
impl Provider for SystemPromptCaptureProvider {
    async fn complete(
        &self,
        messages: &[Message],
        _tools: &[ToolDefinition],
        system: &str,
        _resume_session_id: Option<&str>,
    ) -> Result<EventStream> {
        self.requests
            .lock()
            .unwrap()
            .push((system.to_string(), messages.to_vec()));
        let (tx, rx) = tokio_mpsc::channel::<Result<StreamEvent>>(4);
        tokio::spawn(async move {
            let _ = tx.send(Ok(StreamEvent::TextDelta("ok".to_string()))).await;
            let _ = tx
                .send(Ok(StreamEvent::MessageEnd {
                    stop_reason: Some("end_turn".to_string()),
                }))
                .await;
        });
        Ok(Box::pin(ReceiverStream::new(rx)))
    }

    fn name(&self) -> &str {
        "mock"
    }

    fn fork(&self) -> Arc<dyn Provider> {
        Arc::new(self.clone())
    }
}

/// A swarm DM wakes the recipient with a per-turn system reminder. Putting it
/// in the system prompt changed the prefix of every cached message, so each DM
/// cost a full KV cache miss, and the next turn missed again when it vanished.
/// The reminder must ride in the transcript and leave the system prompt alone.
#[tokio::test]
async fn turn_system_reminder_does_not_change_the_system_prompt() {
    let _guard = crate::storage::lock_test_env();
    let capture = SystemPromptCaptureProvider::default();
    let requests = capture.requests.clone();
    let provider: Arc<dyn Provider> = Arc::new(capture);
    let registry = Registry::new(provider.clone()).await;
    let mut agent = Agent::new(provider, registry);

    let (tx, mut rx) = tokio::sync::mpsc::unbounded_channel();
    agent
        .run_once_streaming_mpsc("first", Vec::new(), None, tx.clone())
        .await
        .expect("plain turn");
    agent
        .run_once_streaming_mpsc(
            "DM from fox: hello",
            Vec::new(),
            Some("You just received a direct swarm message from fox.".to_string()),
            tx.clone(),
        )
        .await
        .expect("reminder turn");
    agent
        .run_once_streaming_mpsc("third", Vec::new(), None, tx)
        .await
        .expect("follow-up turn");

    let injections: Vec<_> = std::iter::from_fn(|| rx.try_recv().ok())
        .filter_map(|event| match event {
            ServerEvent::SoftInterruptInjected {
                content,
                display_role,
                ..
            } => {
                assert_eq!(display_role.as_deref(), Some("system"));
                Some(content)
            }
            _ => None,
        })
        .collect();
    assert_eq!(injections.len(), 1, "ordinary prompts must not be echoed");
    assert!(injections[0].contains("direct swarm message from fox"));

    let requests = requests.lock().unwrap();
    assert_eq!(requests.len(), 3);
    assert!(
        requests.iter().all(|(system, _)| system == &requests[0].0),
        "the system prompt must be identical across reminder and plain turns"
    );
    assert!(!requests[1].0.contains("direct swarm message"));

    let reminder_turn = &requests[1].1;
    let last = reminder_turn.last().map(message_text).unwrap_or_default();
    assert!(last.starts_with("<system-reminder>"), "got {last:?}");
    assert!(last.contains("direct swarm message from fox"));

    // The follow-up turn replays the reminder turn unchanged as its prefix.
    let follow_up = &requests[2].1;
    assert!(follow_up.len() > reminder_turn.len());
    for (before, after) in reminder_turn.iter().zip(follow_up.iter()) {
        assert_eq!(message_text(before), message_text(after));
    }
}

#[tokio::test]
async fn pending_alerts_and_reminder_only_turn_publish_context_entries() {
    let _guard = crate::storage::lock_test_env();
    let provider: Arc<dyn Provider> = Arc::new(SystemPromptCaptureProvider::default());
    let mut agent = Agent::new(provider, Registry::empty());
    agent.push_alert("peer alert: inspect the shared file".to_string());
    let (tx, mut rx) = tokio::sync::mpsc::unbounded_channel();
    agent
        .run_once_streaming_mpsc("", vec![], Some("Resume after reload".into()), tx)
        .await
        .unwrap();
    let entries: Vec<_> = std::iter::from_fn(|| rx.try_recv().ok())
        .filter_map(|event| match event {
            ServerEvent::SoftInterruptInjected {
                content,
                display_role,
                client_ids,
                ..
            } => {
                assert_eq!(display_role.as_deref(), Some("system"));
                assert!(client_ids.is_empty());
                Some(content)
            }
            _ => None,
        })
        .collect();
    assert_eq!(entries.len(), 2);
    assert!(entries[0].contains("peer alert: inspect the shared file"));
    assert_eq!(entries[1], "[auto] Resume after reload");
    let persisted = crate::session::Session::load(&agent.session.id).unwrap();
    for entry in entries {
        assert!(
            persisted
                .messages
                .iter()
                .any(|message| content_text(&message.content) == entry
                    && message.display_role == Some(crate::session::StoredDisplayRole::System))
        );
    }
}

#[tokio::test]
async fn late_skill_announcement_publishes_one_new_context_entry() {
    let _guard = crate::storage::lock_test_env();
    let project = tempfile::tempdir().unwrap();
    let provider: Arc<dyn Provider> = Arc::new(SystemPromptCaptureProvider::default());
    let mut agent = Agent::new_with_initial_working_dir(
        provider,
        Registry::empty(),
        Some(project.path().to_str().unwrap()),
    );
    // Freeze the session's initial skill snapshot before installing a skill.
    agent.build_system_prompt_split(None);
    let skill_dir = project.path().join(".jcode/skills/context-entry-test");
    std::fs::create_dir_all(&skill_dir).unwrap();
    std::fs::write(
        skill_dir.join("SKILL.md"),
        "---\nname: context-entry-test\ndescription: Late announcement test\n---\n\nTest body.\n",
    )
    .unwrap();
    let (tx, mut rx) = tokio::sync::mpsc::unbounded_channel();
    for prompt in ["first", "second"] {
        agent
            .run_once_streaming_mpsc(prompt, vec![], None, tx.clone())
            .await
            .unwrap();
    }
    let entries: Vec<_> = std::iter::from_fn(|| rx.try_recv().ok())
        .filter_map(|event| match event {
            ServerEvent::SoftInterruptInjected { content, .. } => Some(content),
            _ => None,
        })
        .collect();
    assert_eq!(entries.len(), 1);
    assert!(entries[0].contains("New skills were installed."));
    assert!(entries[0].contains("context-entry-test"));
}
