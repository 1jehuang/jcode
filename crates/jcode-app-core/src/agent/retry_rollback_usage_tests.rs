use super::*;
use async_trait::async_trait;
use std::sync::atomic::{AtomicUsize, Ordering};

/// Use the same scripted boundary-provider pattern as response_recovery tests.
/// The real agent reducers consume the entire interrupted/replayed response.
#[derive(Clone)]
struct RollbackUsageProvider {
    events: Vec<StreamEvent>,
    calls: Arc<AtomicUsize>,
}

#[async_trait]
impl Provider for RollbackUsageProvider {
    async fn complete(
        &self,
        _: &[Message],
        _: &[ToolDefinition],
        _: &str,
        _: Option<&str>,
    ) -> Result<crate::provider::EventStream> {
        assert_eq!(self.calls.fetch_add(1, Ordering::SeqCst), 0);
        Ok(Box::pin(futures::stream::iter(
            self.events.clone().into_iter().map(Ok),
        )))
    }

    fn name(&self) -> &str {
        "rollback-usage-test"
    }

    fn model(&self) -> String {
        "rollback-usage-test".into()
    }

    fn supports_compaction(&self) -> bool {
        false
    }

    fn fork(&self) -> Arc<dyn Provider> {
        Arc::new(self.clone())
    }
}

async fn assert_replacement_usage(streaming: bool, replacement_usage: bool) {
    let _home = crate::auth::test_sandbox::AuthTestSandbox::new().unwrap();
    let calls = Arc::new(AtomicUsize::new(0));
    let mut events = vec![
        StreamEvent::TextDelta("discarded answer".into()),
        StreamEvent::ThinkingStart,
        StreamEvent::ThinkingDelta("discarded reasoning".into()),
        StreamEvent::ThinkingSignatureDelta("discarded thinking signature".into()),
        StreamEvent::ThinkingEnd,
        StreamEvent::ToolUseStart {
            id: "discarded-tool".into(),
            name: "must-not-execute".into(),
        },
        StreamEvent::ToolInputDeltaFor {
            id: "discarded-tool".into(),
            delta: "{}".into(),
        },
        StreamEvent::ToolUseEndFor {
            id: "discarded-tool".into(),
        },
        StreamEvent::ToolUseSignatureFor {
            id: "discarded-tool".into(),
            signature: "discarded tool signature".into(),
        },
        StreamEvent::TokenUsage {
            input_tokens: Some(101),
            output_tokens: Some(103),
            cache_read_input_tokens: Some(107),
            cache_creation_input_tokens: Some(109),
        },
        StreamEvent::RetryRollback { attempt: 1, max: 2 },
        StreamEvent::TextDelta("replacement answer".into()),
    ];
    if replacement_usage {
        // None fields must not inherit cache counts from the discarded attempt.
        // Separate updates still accumulate within the replacement attempt.
        events.extend([
            StreamEvent::TokenUsage {
                input_tokens: Some(11),
                output_tokens: None,
                cache_read_input_tokens: None,
                cache_creation_input_tokens: None,
            },
            StreamEvent::TokenUsage {
                input_tokens: None,
                output_tokens: Some(13),
                cache_read_input_tokens: None,
                cache_creation_input_tokens: None,
            },
        ]);
    }
    events.push(StreamEvent::MessageEnd {
        stop_reason: Some("end_turn".into()),
    });
    let provider: Arc<dyn Provider> = Arc::new(RollbackUsageProvider {
        events,
        calls: Arc::clone(&calls),
    });
    let mut agent = Agent::new(provider, Registry::empty());
    agent.add_message(
        Role::User,
        vec![ContentBlock::Text {
            text: "test".into(),
            cache_control: None,
        }],
    );
    let (tx, mut rx) = mpsc::unbounded_channel();
    if streaming {
        agent.run_turn_streaming_mpsc(tx).await.unwrap();
    } else {
        assert_eq!(agent.run_turn(false).await.unwrap(), "replacement answer");
        drop(tx);
    }

    let expected = if replacement_usage { (11, 13) } else { (0, 0) };
    assert_eq!(calls.load(Ordering::SeqCst), 1);
    assert_eq!(agent.last_usage.input_tokens, expected.0);
    assert_eq!(agent.last_usage.output_tokens, expected.1);
    assert_eq!(agent.last_usage.cache_read_input_tokens, None);
    assert_eq!(agent.last_usage.cache_creation_input_tokens, None);
    let assistant_messages: Vec<_> = agent
        .session
        .messages
        .iter()
        .filter(|message| message.role == Role::Assistant)
        .collect();
    assert_eq!(assistant_messages.len(), 1);
    assert!(matches!(
        assistant_messages[0].content.as_slice(),
        [ContentBlock::Text { text, .. }] if text == "replacement answer"
    ));
    let usage = assistant_messages[0].token_usage.as_ref().unwrap();
    assert_eq!(usage.input_tokens, expected.0);
    assert_eq!(usage.output_tokens, expected.1);
    assert_eq!(usage.cache_read_input_tokens, None);
    assert_eq!(usage.cache_creation_input_tokens, None);

    if streaming {
        let mut rollback_count = 0;
        let mut reported_usage = Vec::new();
        while let Ok(event) = rx.try_recv() {
            match event {
                ServerEvent::RetryRollback { attempt: 1, max: 2 } => rollback_count += 1,
                ServerEvent::TokenUsage {
                    input,
                    output,
                    cache_read_input,
                    cache_creation_input,
                } => reported_usage.push((input, output, cache_read_input, cache_creation_input)),
                ServerEvent::ToolDone { .. } => panic!("discarded tool must not complete"),
                _ => {}
            }
        }
        assert_eq!(rollback_count, 1);
        assert_eq!(
            reported_usage,
            if replacement_usage {
                vec![(11, 13, None, None)]
            } else {
                vec![]
            }
        );
    }
}

#[tokio::test]
async fn capture_rollback_discards_usage_when_replacement_omits_usage() {
    assert_replacement_usage(false, false).await;
}

#[tokio::test]
async fn streaming_rollback_discards_usage_when_replacement_omits_usage() {
    assert_replacement_usage(true, false).await;
}

#[tokio::test]
async fn capture_rollback_does_not_inherit_omitted_cache_fields() {
    assert_replacement_usage(false, true).await;
}

#[tokio::test]
async fn streaming_rollback_does_not_inherit_omitted_cache_fields() {
    assert_replacement_usage(true, true).await;
}
