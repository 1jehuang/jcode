use super::*;
use std::sync::atomic::{AtomicBool, AtomicUsize, Ordering};
use tokio::sync::Notify;

struct ToolCancelProvider {
    calls: AtomicUsize,
}

#[async_trait]
impl Provider for ToolCancelProvider {
    async fn complete(
        &self,
        _: &[Message],
        _: &[ToolDefinition],
        _: &str,
        _: Option<&str>,
    ) -> Result<EventStream> {
        let call = self.calls.fetch_add(1, Ordering::SeqCst);
        if call == 0 {
            Ok(Box::pin(futures::stream::iter([
                Ok(StreamEvent::ToolUseStart {
                    id: "slow_1".into(),
                    name: "slow".into(),
                }),
                Ok(StreamEvent::ToolInputDeltaFor {
                    id: "slow_1".into(),
                    delta: "{}".into(),
                }),
                Ok(StreamEvent::ToolUseEndFor {
                    id: "slow_1".into(),
                }),
                Ok(StreamEvent::MessageEnd {
                    stop_reason: Some("tool_use".into()),
                }),
            ])))
        } else {
            Ok(Box::pin(futures::stream::iter([
                Ok(StreamEvent::TextDelta("should not run".into())),
                Ok(StreamEvent::MessageEnd {
                    stop_reason: Some("end_turn".into()),
                }),
            ])))
        }
    }

    fn name(&self) -> &str {
        "tool-cancel-test"
    }

    fn supports_compaction(&self) -> bool {
        false
    }

    fn fork(&self) -> Arc<dyn Provider> {
        unreachable!("single-agent cancellation test")
    }
}

struct DropFlag(Arc<AtomicBool>);

impl Drop for DropFlag {
    fn drop(&mut self) {
        self.0.store(true, Ordering::SeqCst);
    }
}

struct SlowTool {
    started: Arc<Notify>,
    finished: Arc<AtomicBool>,
    dropped: Arc<AtomicBool>,
}

#[async_trait]
impl crate::tool::Tool for SlowTool {
    fn name(&self) -> &str {
        "slow"
    }

    fn description(&self) -> &str {
        "Wait until cancelled"
    }

    fn parameters_schema(&self) -> serde_json::Value {
        serde_json::json!({"type":"object"})
    }

    async fn execute(
        &self,
        _: serde_json::Value,
        _: crate::tool::ToolContext,
    ) -> Result<ToolOutput> {
        let _drop_flag = DropFlag(Arc::clone(&self.dropped));
        self.started.notify_one();
        tokio::time::sleep(Duration::from_secs(30)).await;
        self.finished.store(true, Ordering::SeqCst);
        Ok(ToolOutput::new("finished"))
    }
}

#[tokio::test]
async fn cancellation_during_tool_execution_aborts_tool_and_continuation() {
    let _guard = crate::storage::lock_test_env();
    struct HomeGuard(Option<std::ffi::OsString>);
    impl Drop for HomeGuard {
        fn drop(&mut self) {
            match self.0.take() {
                Some(value) => crate::env::set_var("JCODE_HOME", value),
                None => crate::env::remove_var("JCODE_HOME"),
            }
            crate::config::Config::invalidate_cache();
        }
    }
    let _home = HomeGuard(std::env::var_os("JCODE_HOME"));
    let home = tempfile::tempdir().unwrap();
    crate::env::set_var("JCODE_HOME", home.path());
    crate::config::Config::invalidate_cache();

    let started = Arc::new(Notify::new());
    let finished = Arc::new(AtomicBool::new(false));
    let dropped = Arc::new(AtomicBool::new(false));
    let registry = Registry::empty();
    registry
        .register(
            "slow".into(),
            Arc::new(SlowTool {
                started: Arc::clone(&started),
                finished: Arc::clone(&finished),
                dropped: Arc::clone(&dropped),
            }),
        )
        .await;

    let provider = Arc::new(ToolCancelProvider {
        calls: AtomicUsize::new(0),
    });
    let mut agent = Agent::new(provider.clone(), registry);
    let cancel = agent.graceful_shutdown_signal();
    let (event_tx, _event_rx) = tokio_mpsc::unbounded_channel();
    let turn = agent.run_once_streaming_mpsc("start a slow tool", Vec::new(), None, event_tx);
    tokio::pin!(turn);

    tokio::select! {
        _ = started.notified() => cancel.fire(),
        result = &mut turn => panic!("turn finished before the tool started: {result:?}"),
    }

    let _ = tokio::time::timeout(Duration::from_secs(5), &mut turn)
        .await
        .expect("cancellation must finish the turn promptly");
    tokio::time::sleep(Duration::from_millis(100)).await;

    assert_eq!(
        provider.calls.load(Ordering::SeqCst),
        1,
        "cancel must stop before the continuation request"
    );
    assert!(
        dropped.load(Ordering::SeqCst),
        "tool future must be aborted"
    );
    assert!(!finished.load(Ordering::SeqCst), "tool must not complete");
}
