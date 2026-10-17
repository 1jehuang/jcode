use super::*;
use serde_json::{Value, json};
use std::sync::Mutex;

type InputLog = Arc<Mutex<Vec<Value>>>;

#[derive(Clone)]
struct ChannelProvider(Arc<Mutex<Option<tokio_mpsc::Receiver<Result<StreamEvent>>>>>);

#[async_trait]
impl Provider for ChannelProvider {
    async fn complete(
        &self,
        _: &[Message],
        _: &[ToolDefinition],
        _: &str,
        _: Option<&str>,
    ) -> Result<EventStream> {
        if let Some(rx) = self.0.lock().unwrap().take() {
            Ok(Box::pin(ReceiverStream::new(rx)))
        } else {
            Ok(Box::pin(futures::stream::iter([
                Ok(StreamEvent::TextDelta("done".into())),
                Ok(StreamEvent::MessageEnd {
                    stop_reason: Some("end_turn".into()),
                }),
            ])))
        }
    }
    fn name(&self) -> &str {
        "keyed-stream-test"
    }
    fn supports_compaction(&self) -> bool {
        false
    }
    fn fork(&self) -> Arc<dyn Provider> {
        Arc::new(self.clone())
    }
}

struct CaptureTool(InputLog);
#[async_trait]
impl crate::tool::Tool for CaptureTool {
    fn name(&self) -> &str {
        "capture"
    }
    fn description(&self) -> &str {
        "Capture test input"
    }
    fn parameters_schema(&self) -> Value {
        json!({"type":"object"})
    }
    async fn execute(&self, input: Value, _: crate::tool::ToolContext) -> Result<ToolOutput> {
        self.0.lock().unwrap().push(input);
        Ok(ToolOutput::new("ok"))
    }
}

async fn setup() -> (Agent, tokio_mpsc::Sender<Result<StreamEvent>>, InputLog) {
    let (tx, rx) = tokio_mpsc::channel(32);
    let provider = Arc::new(ChannelProvider(Arc::new(Mutex::new(Some(rx)))));
    let log = Arc::new(Mutex::new(Vec::new()));
    let registry = Registry::empty();
    registry
        .register("capture".into(), Arc::new(CaptureTool(log.clone())))
        .await;
    let mut agent = Agent::new(provider, registry);
    agent.add_message(
        Role::User,
        vec![ContentBlock::Text {
            text: "capture both".into(),
            cache_control: None,
        }],
    );
    (agent, tx, log)
}

fn start(id: &str) -> StreamEvent {
    StreamEvent::ToolUseStart {
        id: id.into(),
        name: "capture".into(),
    }
}
fn delta(id: &str, delta: &str) -> StreamEvent {
    StreamEvent::ToolInputDeltaFor {
        id: id.into(),
        delta: delta.into(),
    }
}
fn end(id: &str) -> StreamEvent {
    StreamEvent::ToolUseEndFor { id: id.into() }
}
fn finish() -> StreamEvent {
    StreamEvent::MessageEnd {
        stop_reason: Some("tool_use".into()),
    }
}

async fn next_tool_event(rx: &mut tokio_mpsc::UnboundedReceiver<ServerEvent>) -> ServerEvent {
    tokio::time::timeout(Duration::from_secs(10), async {
        loop {
            let event = rx.recv().await.expect("turn remains open");
            if matches!(
                event,
                ServerEvent::ToolStart { .. }
                    | ServerEvent::ToolInput { .. }
                    | ServerEvent::ToolExec { .. }
            ) {
                return event;
            }
        }
    })
    .await
    .expect("tool event must arrive without more provider output")
}

#[tokio::test]
async fn keyed_tool_streaming_emits_each_name_before_args_and_executes_once() {
    let _sandbox = crate::auth::test_sandbox::AuthTestSandbox::new().unwrap();
    let (mut agent, provider_tx, log) = setup().await;
    let (tx, mut rx) = tokio_mpsc::unbounded_channel();
    let task = tokio::spawn(async move {
        agent.run_turn_streaming_mpsc(tx).await.unwrap();
        agent
    });
    for id in ["a", "b"] {
        provider_tx.send(Ok(start(id))).await.unwrap();
        assert!(
            matches!(next_tool_event(&mut rx).await, ServerEvent::ToolStart { id: actual, name }
            if actual == id && name == "capture")
        );
        assert!(log.lock().unwrap().is_empty());
    }
    // Neither incomplete JSON nor another call's start may delay a fragment.
    for (id, fragment) in [
        ("b", "{\"value\":"),
        ("a", "{\"value\":\"a\"}"),
        ("b", "\"b\"}"),
    ] {
        provider_tx.send(Ok(delta(id, fragment))).await.unwrap();
        assert!(
            matches!(next_tool_event(&mut rx).await, ServerEvent::ToolInput { id: Some(actual), delta }
            if actual == id && delta == fragment)
        );
    }
    // A replayed start and duplicate/unknown ends must not execute twice.
    for event in [
        start("a"),
        end("a"),
        end("a"),
        end("missing"),
        end("b"),
        StreamEvent::ToolUseSignatureFor {
            id: "a".into(),
            signature: "signature-a".into(),
        },
        StreamEvent::ToolUseSignatureFor {
            id: "b".into(),
            signature: "signature-b".into(),
        },
        finish(),
    ] {
        provider_tx.send(Ok(event)).await.unwrap();
    }
    drop(provider_tx);
    let agent = tokio::time::timeout(Duration::from_secs(10), task)
        .await
        .unwrap()
        .unwrap();
    let mut inputs = log.lock().unwrap().clone();
    inputs.sort_by_key(Value::to_string);
    assert_eq!(inputs, vec![json!({"value":"a"}), json!({"value":"b"})]);
    let mut executions = Vec::new();
    while let Ok(event) = rx.try_recv() {
        if let ServerEvent::ToolExec { id, .. } = event {
            executions.push(id);
        }
    }
    assert_eq!(executions, ["a", "b"]);
    for id in ["a", "b"] {
        assert!(agent.session.messages.iter().flat_map(|m| &m.content).any(
            |block| matches!(block,
            ContentBlock::ToolUse { id: actual, thought_signature: Some(signature), .. }
                if actual == id && signature == &format!("signature-{id}"))
        ));
    }
}

#[tokio::test]
async fn keyed_tool_streaming_blocking_loop_isolates_inputs_and_legacy_fallback() {
    let _sandbox = crate::auth::test_sandbox::AuthTestSandbox::new().unwrap();
    let (mut agent, tx, log) = setup().await;
    for event in [
        start("a"),
        start("b"),
        delta("a", "{\"value\":1}"),
        delta("b", "{\"value\":2}"),
        end("b"),
        end("a"),
        end("a"),
        start("c"),
        StreamEvent::ToolInputDelta("{\"value\":3}".into()),
        StreamEvent::ToolUseEnd,
        finish(),
    ] {
        tx.send(Ok(event)).await.unwrap();
    }
    drop(tx);
    agent.run_turn(false).await.unwrap();
    let mut inputs = log.lock().unwrap().clone();
    inputs.sort_by_key(Value::to_string);
    assert_eq!(
        inputs,
        vec![json!({"value":1}), json!({"value":2}), json!({"value":3})]
    );
}

#[tokio::test]
async fn keyed_tool_streaming_rollback_discards_all_partial_calls() {
    let _sandbox = crate::auth::test_sandbox::AuthTestSandbox::new().unwrap();
    for streaming in [false, true] {
        let (mut agent, tx, log) = setup().await;
        for event in [
            start("a"),
            start("b"),
            delta("a", "{\"stale\":"),
            delta("b", "{\"stale\":"),
            StreamEvent::RetryRollback { attempt: 1, max: 2 },
            end("b"),
            start("a"),
            delta("a", "{\"fresh\":true}"),
            end("a"),
            finish(),
        ] {
            tx.send(Ok(event)).await.unwrap();
        }
        drop(tx);
        if streaming {
            let (events, _rx) = tokio_mpsc::unbounded_channel();
            agent.run_turn_streaming_mpsc(events).await.unwrap();
        } else {
            agent.run_turn(false).await.unwrap();
        }
        assert_eq!(*log.lock().unwrap(), vec![json!({"fresh":true})]);
    }
}
