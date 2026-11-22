use super::*;
use crate::network_retry::test_hooks::{TestNetworkHooks, with_test_network_hooks};

#[derive(Clone)]
enum MockCallScript {
    ConnectError(String),
    StreamErrorAfterPartial { partial: String, error: String },
    StreamEventError { message: String },
    SuccessText(String),
}

struct ScriptedNetworkProvider {
    scripts: Arc<std::sync::Mutex<std::collections::VecDeque<MockCallScript>>>,
    calls: Arc<std::sync::Mutex<usize>>,
}

impl ScriptedNetworkProvider {
    fn new(scripts: Vec<MockCallScript>) -> Self {
        Self {
            scripts: Arc::new(std::sync::Mutex::new(scripts.into())),
            calls: Arc::new(std::sync::Mutex::new(0)),
        }
    }
}

#[async_trait]
impl Provider for ScriptedNetworkProvider {
    async fn complete(
        &self,
        _messages: &[Message],
        _tools: &[ToolDefinition],
        _system: &str,
        _resume_session_id: Option<&str>,
    ) -> Result<EventStream> {
        let mut calls = self.calls.lock().unwrap();
        *calls += 1;
        drop(calls);

        let script = self
            .scripts
            .lock()
            .unwrap()
            .pop_front()
            .unwrap_or_else(|| MockCallScript::SuccessText("fallback done".to_string()));

        match script {
            MockCallScript::ConnectError(err) => Err(anyhow::anyhow!(err)),
            MockCallScript::StreamErrorAfterPartial { partial, error } => {
                let events: Vec<Result<StreamEvent>> = vec![
                    Ok(StreamEvent::TextDelta(partial)),
                    Err(anyhow::anyhow!(error)),
                ];
                Ok(Box::pin(futures::stream::iter(events)))
            }
            MockCallScript::StreamEventError { message } => {
                let events: Vec<Result<StreamEvent>> = vec![Ok(StreamEvent::Error {
                    message,
                    retry_after_secs: None,
                })];
                Ok(Box::pin(futures::stream::iter(events)))
            }
            MockCallScript::SuccessText(text) => {
                let events: Vec<Result<StreamEvent>> = vec![
                    Ok(StreamEvent::TextDelta(text)),
                    Ok(StreamEvent::MessageEnd {
                        stop_reason: Some("end_turn".to_string()),
                    }),
                ];
                Ok(Box::pin(futures::stream::iter(events)))
            }
        }
    }

    fn name(&self) -> &'static str {
        "mock-network"
    }

    fn fork(&self) -> Arc<dyn Provider> {
        Arc::new(Self {
            scripts: self.scripts.clone(),
            calls: self.calls.clone(),
        })
    }
}

#[tokio::test]
async fn streaming_turn_waits_for_network_on_connect_error_and_resumes() {
    let _guard = crate::storage::lock_test_env();
    let provider_impl = ScriptedNetworkProvider::new(vec![
        MockCallScript::ConnectError(
            "Failed to send Antigravity generateContent request".to_string(),
        ),
        MockCallScript::SuccessText("recovered after offline disconnect".to_string()),
    ]);
    let calls = provider_impl.calls.clone();
    let provider: Arc<dyn Provider> = Arc::new(provider_impl);
    let registry = Registry::new(provider.clone()).await;
    let mut agent = Agent::new(provider, registry);

    let hooks = TestNetworkHooks::from_probe_sequence(vec![false, false, true]);
    let (tx, mut rx) = tokio::sync::mpsc::unbounded_channel();

    with_test_network_hooks(hooks, async {
        agent
            .run_once_streaming_mpsc("keep going", Vec::new(), None, tx)
            .await
            .expect("turn should recover automatically once connectivity returns");
    })
    .await;

    let mut saw_waiting_phase = false;
    let mut saw_connecting_after_wait = false;
    let mut text = String::new();
    while let Ok(event) = rx.try_recv() {
        match event {
            ServerEvent::ConnectionPhase { phase } => {
                if crate::network_retry::parse_waiting_for_network_connection_phase(&phase)
                    .is_some()
                {
                    saw_waiting_phase = true;
                } else if saw_waiting_phase && phase == "connecting" {
                    saw_connecting_after_wait = true;
                }
            }
            ServerEvent::TextDelta { text: delta } => text.push_str(&delta),
            _ => {}
        }
    }

    assert_eq!(*calls.lock().unwrap(), 2);
    assert!(
        saw_waiting_phase,
        "server must emit WaitingForNetwork ConnectionPhase while offline"
    );
    assert!(
        saw_connecting_after_wait,
        "server must emit Connecting phase when resuming after connectivity returns"
    );
    assert_eq!(text, "recovered after offline disconnect");
}

#[tokio::test]
async fn streaming_turn_rolls_back_partial_stream_and_resumes_after_disconnect() {
    let _guard = crate::storage::lock_test_env();
    let provider_impl = ScriptedNetworkProvider::new(vec![
        MockCallScript::StreamErrorAfterPartial {
            partial: "partial before drop ".to_string(),
            error: "connection reset by peer".to_string(),
        },
        MockCallScript::SuccessText("clean full answer".to_string()),
    ]);
    let calls = provider_impl.calls.clone();
    let provider: Arc<dyn Provider> = Arc::new(provider_impl);
    let registry = Registry::new(provider.clone()).await;
    let mut agent = Agent::new(provider, registry);

    let hooks = TestNetworkHooks::from_probe_sequence(vec![false, true]);
    let (tx, mut rx) = tokio::sync::mpsc::unbounded_channel();

    with_test_network_hooks(hooks, async {
        agent
            .run_once_streaming_mpsc("write response", Vec::new(), None, tx)
            .await
            .expect("turn should roll back partial stream and recover cleanly");
    })
    .await;

    let mut saw_rollback = false;
    let mut saw_waiting = false;
    let mut text_after_rollback = String::new();
    while let Ok(event) = rx.try_recv() {
        match event {
            ServerEvent::RetryRollback { .. } => saw_rollback = true,
            ServerEvent::ConnectionPhase { phase }
                if crate::network_retry::parse_waiting_for_network_connection_phase(&phase)
                    .is_some() =>
            {
                saw_waiting = true;
            }
            ServerEvent::TextDelta { text } if saw_rollback => text_after_rollback.push_str(&text),
            _ => {}
        }
    }

    assert_eq!(*calls.lock().unwrap(), 2);
    assert!(
        saw_rollback,
        "partial stream must emit RetryRollback so clients wipe truncated output"
    );
    assert!(saw_waiting);
    assert_eq!(text_after_rollback, "clean full answer");
    let assistant_messages: Vec<String> = agent
        .session
        .messages
        .iter()
        .filter(|m| m.role == Role::Assistant)
        .map(|m| {
            m.content
                .iter()
                .filter_map(|b| match b {
                    ContentBlock::Text { text, .. } => Some(text.as_str()),
                    _ => None,
                })
                .collect::<String>()
        })
        .collect();
    assert_eq!(assistant_messages, vec!["clean full answer".to_string()]);
}

#[tokio::test]
async fn streaming_turn_recovers_from_stream_event_error_mid_turn() {
    let _guard = crate::storage::lock_test_env();
    let provider_impl = ScriptedNetworkProvider::new(vec![
        MockCallScript::StreamEventError {
            message: "client error (SendRequest): http2 error: stream error received".to_string(),
        },
        MockCallScript::SuccessText("resumed after stream event error".to_string()),
    ]);
    let calls = provider_impl.calls.clone();
    let provider: Arc<dyn Provider> = Arc::new(provider_impl);
    let registry = Registry::new(provider.clone()).await;
    let mut agent = Agent::new(provider, registry);

    let hooks = TestNetworkHooks::from_probe_sequence(vec![false, true]);
    let (tx, mut rx) = tokio::sync::mpsc::unbounded_channel();

    with_test_network_hooks(hooks, async {
        agent
            .run_once_streaming_mpsc("continue", Vec::new(), None, tx)
            .await
            .expect("turn should recover from network StreamEvent::Error");
    })
    .await;

    let mut text = String::new();
    while let Ok(event) = rx.try_recv() {
        if let ServerEvent::TextDelta { text: delta } = event {
            text.push_str(&delta);
        }
    }
    assert_eq!(*calls.lock().unwrap(), 2);
    assert_eq!(text, "resumed after stream event error");
}

#[tokio::test]
async fn streaming_turn_cancels_cleanly_while_waiting_for_network() {
    let _guard = crate::storage::lock_test_env();
    let provider_impl = ScriptedNetworkProvider::new(vec![MockCallScript::ConnectError(
        "dns error: temporary failure in name resolution".to_string(),
    )]);
    let calls = provider_impl.calls.clone();
    let provider: Arc<dyn Provider> = Arc::new(provider_impl);
    let registry = Registry::new(provider.clone()).await;
    let mut agent = Agent::new(provider, registry);
    let shutdown = agent.graceful_shutdown_signal();

    let wait_entered = Arc::new(tokio::sync::Notify::new());
    let wait_entered_for_hook = wait_entered.clone();
    let hooks = TestNetworkHooks {
        probe: Arc::new(|| false),
        wait: Arc::new(move |_| {
            let notify = wait_entered_for_hook.clone();
            Box::pin(async move {
                notify.notify_one();
                std::future::pending::<bool>().await
            })
        }),
        online_backoff: std::time::Duration::from_millis(1),
    };

    let (tx, _rx) = tokio::sync::mpsc::unbounded_channel();
    let cancel_task = tokio::spawn(async move {
        wait_entered.notified().await;
        shutdown.fire();
    });

    with_test_network_hooks(hooks, async {
        agent
            .run_once_streaming_mpsc("cancel me", Vec::new(), None, tx)
            .await
            .expect("cancelled wait should exit cleanly with Ok(())");
    })
    .await;
    cancel_task.await.unwrap();

    assert_eq!(
        *calls.lock().unwrap(),
        1,
        "cancelled turn must not start another API call"
    );
}

#[tokio::test]
async fn streaming_turn_gives_up_after_online_retry_budget_exhausted() {
    let _guard = crate::storage::lock_test_env();
    let provider_impl = ScriptedNetworkProvider::new(vec![
        MockCallScript::ConnectError("connection refused".to_string()),
        MockCallScript::ConnectError("connection refused".to_string()),
        MockCallScript::ConnectError("connection refused".to_string()),
        MockCallScript::ConnectError("connection refused".to_string()),
    ]);
    let calls = provider_impl.calls.clone();
    let provider: Arc<dyn Provider> = Arc::new(provider_impl);
    let registry = Registry::new(provider.clone()).await;
    let mut agent = Agent::new(provider, registry);

    let hooks = TestNetworkHooks::from_probe_sequence(vec![true]);
    let (tx, _rx) = tokio::sync::mpsc::unbounded_channel();

    let result = with_test_network_hooks(hooks, async {
        agent
            .run_once_streaming_mpsc("dead endpoint", Vec::new(), None, tx)
            .await
    })
    .await;

    assert!(result.is_err(), "exhausted online retries must return Err");
    assert_eq!(
        *calls.lock().unwrap(),
        (crate::network_retry::MAX_ONLINE_NETWORK_RETRIES + 1) as usize,
        "must try initial request + MAX_ONLINE_NETWORK_RETRIES and then stop"
    );
}

#[tokio::test]
async fn blocking_run_turn_waits_for_network_and_resumes() {
    let _guard = crate::storage::lock_test_env();
    let provider_impl = ScriptedNetworkProvider::new(vec![
        MockCallScript::ConnectError(
            "Failed to send request to Anthropic API: network is unreachable".to_string(),
        ),
        MockCallScript::SuccessText("blocking turn recovered".to_string()),
    ]);
    let calls = provider_impl.calls.clone();
    let provider: Arc<dyn Provider> = Arc::new(provider_impl);
    let registry = Registry::new(provider.clone()).await;
    let mut agent = Agent::new(provider, registry);

    let hooks = TestNetworkHooks::from_probe_sequence(vec![false, true]);
    let output = with_test_network_hooks(hooks, async {
        agent
            .run_once_capture("cli prompt")
            .await
            .expect("blocking run_turn should wait for connectivity and resume")
    })
    .await;

    assert_eq!(*calls.lock().unwrap(), 2);
    assert_eq!(output, "blocking turn recovered");
}
