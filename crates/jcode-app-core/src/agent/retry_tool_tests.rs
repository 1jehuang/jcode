//! Exercise the real agent and tool executor across a provider-local replay.
use super::*;
use async_trait::async_trait;
use std::sync::atomic::{AtomicUsize, Ordering};

#[derive(Clone)]
struct RetryToolProvider {
    calls: Arc<AtomicUsize>,
    fixture: PathBuf,
}

#[async_trait]
impl Provider for RetryToolProvider {
    async fn complete(
        &self,
        _: &[Message],
        _: &[ToolDefinition],
        _: &str,
        _: Option<&str>,
    ) -> Result<crate::provider::EventStream> {
        let call = self.calls.fetch_add(1, Ordering::SeqCst);
        let mut events = Vec::new();
        if call == 0 {
            let tool = || {
                vec![
                    StreamEvent::ToolUseStart {
                        id: "read-fixture".into(),
                        name: "read".into(),
                    },
                    StreamEvent::ToolInputDelta(
                        serde_json::json!({
                            "file_path": self.fixture,
                            "intent": "Read retry regression fixture"
                        })
                        .to_string(),
                    ),
                    StreamEvent::ToolUseEnd,
                ]
            };
            events.extend(tool());
            events.push(StreamEvent::RetryRollback { attempt: 2, max: 3 });
            events.extend(tool());
            events.push(StreamEvent::MessageEnd {
                stop_reason: Some("tool_use".into()),
            });
        } else {
            events.push(StreamEvent::TextDelta("Read the fixture once.".into()));
            events.push(StreamEvent::MessageEnd {
                stop_reason: Some("end_turn".into()),
            });
        }
        Ok(Box::pin(futures::stream::iter(events.into_iter().map(Ok))))
    }

    fn name(&self) -> &str {
        "retry-tool-test"
    }
    fn model(&self) -> String {
        "retry-tool-test".into()
    }
    fn fork(&self) -> Arc<dyn Provider> {
        Arc::new(self.clone())
    }
}

#[tokio::test]
async fn provider_retry_rollback_executes_tool_only_once() {
    let _home = crate::auth::test_sandbox::AuthTestSandbox::new().unwrap();
    let directory = tempfile::tempdir().unwrap();
    let fixture = directory.path().join("fixture.txt");
    std::fs::write(&fixture, "retry fixture content").unwrap();
    let calls = Arc::new(AtomicUsize::new(0));
    let provider: Arc<dyn Provider> = Arc::new(RetryToolProvider {
        calls: calls.clone(),
        fixture,
    });
    let registry = Registry::new(provider.clone()).await;
    let mut agent = Agent::new(provider, registry);
    agent.add_message(
        Role::User,
        vec![ContentBlock::Text {
            text: "Read the fixture.".into(),
            cache_control: None,
        }],
    );
    let (tx, mut rx) = mpsc::unbounded_channel();
    agent.run_turn_streaming_mpsc(tx).await.unwrap();
    let mut executions = 0;
    let mut completions = 0;
    let mut rollbacks = 0;
    while let Ok(event) = rx.try_recv() {
        match event {
            ServerEvent::RetryRollback { .. } => {
                rollbacks += 1;
                // ToolExec is a provisional display phase emitted when tool
                // arguments finish streaming, not a Registry execution. The
                // consumer discards it with the failed attempt.
                executions = 0;
            }
            ServerEvent::ToolExec { id, name } => {
                assert_eq!(id, "read-fixture");
                assert_eq!(name, "read");
                executions += 1;
            }
            ServerEvent::ToolDone {
                id,
                name,
                output,
                error,
            } => {
                assert_eq!(id, "read-fixture");
                assert_eq!(name, "read");
                assert!(error.is_none(), "real read tool must succeed: {error:?}");
                assert!(output.contains("retry fixture content"));
                completions += 1;
            }
            _ => {}
        }
    }
    assert_eq!(rollbacks, 1);
    assert_eq!(executions, 1);
    assert_eq!(completions, 1);
    assert_eq!(calls.load(Ordering::SeqCst), 2);
}
