use super::*;
use async_trait::async_trait;
use std::sync::atomic::{AtomicUsize, Ordering};

#[derive(Clone)]
struct CompactionCacheProvider {
    calls: Arc<AtomicUsize>,
    compaction_after_usage: bool,
    discard_compaction: bool,
}

#[async_trait]
impl Provider for CompactionCacheProvider {
    async fn complete(
        &self,
        _: &[Message],
        _: &[ToolDefinition],
        _: &str,
        _: Option<&str>,
    ) -> Result<crate::provider::EventStream> {
        let call = self.calls.fetch_add(1, Ordering::SeqCst);
        let mut events = vec![Ok(StreamEvent::TokenUsage {
            input_tokens: Some(if call == 0 { 216_517 } else { 204_393 }),
            output_tokens: Some(5),
            cache_read_input_tokens: Some(if call == 0 { 216_064 } else { 0 }),
            cache_creation_input_tokens: None,
        })];
        if call == 1 {
            let compaction = Ok(StreamEvent::Compaction {
                trigger: "openai_native_auto".into(),
                pre_tokens: None,
                openai_encrypted_content: Some("test-encrypted-compaction".into()),
            });
            if self.compaction_after_usage {
                events.push(compaction);
            } else {
                events.insert(0, compaction);
            }
            if self.discard_compaction {
                events.push(Ok(StreamEvent::RetryRollback { attempt: 1, max: 2 }));
                events.push(Ok(StreamEvent::TokenUsage {
                    input_tokens: Some(204_393),
                    output_tokens: Some(5),
                    cache_read_input_tokens: Some(0),
                    cache_creation_input_tokens: None,
                }));
            }
        }
        events.push(Ok(StreamEvent::TextDelta("answer".into())));
        events.push(Ok(StreamEvent::MessageEnd {
            stop_reason: Some("end_turn".into()),
        }));
        Ok(Box::pin(futures::stream::iter(events)))
    }

    fn name(&self) -> &str {
        "openai"
    }

    fn model(&self) -> String {
        "cache-compaction-test".into()
    }

    fn fork(&self) -> Arc<dyn Provider> {
        Arc::new(self.clone())
    }
}

#[tokio::test]
async fn native_compaction_cache_refresh_is_classified_before_compaction_is_announced() {
    let _home = crate::auth::test_sandbox::AuthTestSandbox::new().unwrap();
    for (compaction_after_usage, discard_compaction) in
        [(false, false), (true, false), (true, true)]
    {
        let provider: Arc<dyn Provider> = Arc::new(CompactionCacheProvider {
            calls: Arc::new(AtomicUsize::new(0)),
            compaction_after_usage,
            discard_compaction,
        });
        let registry = Registry::new(provider.clone()).await;
        let mut agent = Agent::new(provider, registry);
        agent.add_message(
            Role::User,
            vec![ContentBlock::Text {
                text: "warm cache".into(),
                cache_control: None,
            }],
        );
        let (tx, mut rx) = mpsc::unbounded_channel();
        agent.run_turn_streaming_mpsc(tx.clone()).await.unwrap();
        while rx.try_recv().is_ok() {}
        agent.add_message(
            Role::User,
            vec![ContentBlock::Text {
                text: "continue".into(),
                cache_control: None,
            }],
        );
        agent.run_turn_streaming_mpsc(tx).await.unwrap();
        let mut refresh_seen = false;
        let mut compaction_seen = false;
        while let Ok(event) = rx.try_recv() {
            match event {
                ServerEvent::KvCacheMiss {
                    reason,
                    harness_caused,
                    expected_tokens,
                    read_tokens,
                    documented_cause,
                    message,
                    ..
                } => {
                    assert_eq!(
                        reason,
                        if discard_compaction {
                            "zero_read"
                        } else {
                            "compaction"
                        }
                    );
                    assert!(!harness_caused);
                    assert_eq!(expected_tokens, 216_517);
                    assert_eq!(read_tokens, 0);
                    if discard_compaction {
                        assert_eq!(documented_cause, None);
                        assert!(message.starts_with("KV cache miss"));
                    } else {
                        assert_eq!(
                            documented_cause.as_deref(),
                            Some("provider_auto_compaction")
                        );
                        assert!(message.starts_with("KV cache refresh"));
                    }
                    refresh_seen = true;
                }
                ServerEvent::Compaction {
                    trigger,
                    pre_tokens,
                    post_tokens,
                    tokens_saved,
                    summary_chars,
                    ..
                } => {
                    assert_eq!(trigger, "openai_native_auto");
                    assert_eq!(
                        pre_tokens, None,
                        "response input is not a pre-compaction count"
                    );
                    assert_eq!(
                        post_tokens, None,
                        "opaque payload length is not a token count"
                    );
                    assert_eq!(tokens_saved, None);
                    assert_eq!(summary_chars, None);
                    assert!(
                        refresh_seen,
                        "usage classification precedes compaction application"
                    );
                    compaction_seen = true;
                }
                _ => {}
            }
        }
        assert!(
            refresh_seen,
            "completed request must classify its zero read"
        );
        assert_eq!(compaction_seen, !discard_compaction);
    }
}
