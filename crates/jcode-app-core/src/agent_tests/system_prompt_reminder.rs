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

    let (tx, _rx) = tokio::sync::mpsc::unbounded_channel();
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
