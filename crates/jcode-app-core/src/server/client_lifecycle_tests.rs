use super::*;
use crate::message::{Message, StreamEvent, ToolDefinition};
use crate::provider::{EventStream, Provider};
use async_trait::async_trait;
use futures::stream;
use std::sync::atomic::{AtomicBool, Ordering};
use tokio::io::{AsyncBufReadExt, AsyncWriteExt, BufReader};

struct IsolatedRuntimeDir {
    _prev_runtime: Option<std::ffi::OsString>,
    _temp: tempfile::TempDir,
}

struct IsolatedReloadRecoveryEnv {
    prev_home: Option<std::ffi::OsString>,
    prev_runtime: Option<std::ffi::OsString>,
    _home: tempfile::TempDir,
    _runtime: tempfile::TempDir,
}

#[tokio::test]
async fn session_control_handle_does_not_wait_for_busy_agent_lock() {
    let provider: Arc<dyn Provider> = Arc::new(PanicOnForkProvider {
        forked: Arc::new(AtomicBool::new(false)),
    });
    let registry = Registry::new(Arc::clone(&provider)).await;
    let agent = Arc::new(Mutex::new(Agent::new(provider, registry)));

    let queue = Arc::new(std::sync::Mutex::new(Vec::new()));
    let background_signal = InterruptSignal::new();
    let stop_signal = InterruptSignal::new();
    let control = SessionControlHandle::new(
        "session_control_test",
        Arc::clone(&queue),
        background_signal.clone(),
        stop_signal.clone(),
    );

    let _busy_agent_lock = agent.lock().await;

    tokio::time::timeout(Duration::from_millis(100), async {
        assert!(control.queue_soft_interrupt(
            "please stop".to_string(),
            Vec::new(),
            true,
            SoftInterruptSource::User,
        ));
        control.request_cancel();
        assert!(control.request_background_current_tool());
        control.clear_soft_interrupts();
    })
    .await
    .expect("lock-free control operations should not wait for the agent mutex");

    assert!(stop_signal.is_set());
    assert!(background_signal.is_set());
    assert!(queue.lock().expect("queue lock").is_empty());
}

#[tokio::test]
async fn refreshed_session_control_handle_does_not_wait_for_busy_agent_lock() {
    let provider: Arc<dyn Provider> = Arc::new(PanicOnForkProvider {
        forked: Arc::new(AtomicBool::new(false)),
    });
    let registry = Registry::new(Arc::clone(&provider)).await;
    let mut session = crate::session::Session::create_with_id(
        "session_busy_control_refresh".to_string(),
        None,
        None,
    );
    session.model = Some("panic-on-fork".to_string());
    let agent = Arc::new(Mutex::new(Agent::new_with_session(
        provider, registry, session, None,
    )));

    let stop_signal = InterruptSignal::new();
    let soft_interrupt_queue = Arc::new(std::sync::Mutex::new(Vec::new()));
    let shutdown_signals = Arc::new(RwLock::new(HashMap::from([(
        "session_busy_control_refresh".to_string(),
        stop_signal.clone(),
    )])));
    let soft_interrupt_queues: SessionInterruptQueues = Arc::new(RwLock::new(HashMap::from([(
        "session_busy_control_refresh".to_string(),
        soft_interrupt_queue,
    )])));

    let _busy_agent_lock = agent.lock().await;

    tokio::time::timeout(Duration::from_millis(100), async {
        let control = refresh_session_control_handle(
            "session_busy_control_refresh",
            &agent,
            &shutdown_signals,
            &soft_interrupt_queues,
        )
        .await;
        control.request_cancel();
    })
    .await
    .expect("refreshing a session control handle must not wait for the busy agent mutex");

    assert!(stop_signal.is_set());
}

#[tokio::test]
async fn busy_session_background_tool_signal_fires_via_registry_fallback() {
    // Regression: pressing Alt+B/Ctrl+B while a turn owns the agent mutex (e.g.
    // running `await_members`) used to silently no-op because the lock-free
    // `cancel_only` control handle dropped the background-tool signal
    // (BACKGROUND_TOOL_SIGNAL_FIRE result=no_signal_handle). Building a full
    // SessionControlHandle now registers the signal in a process-global registry
    // so the cancel-only fallback can still fire it without the agent lock.
    let provider: Arc<dyn Provider> = Arc::new(PanicOnForkProvider {
        forked: Arc::new(AtomicBool::new(false)),
    });
    let registry = Registry::new(Arc::clone(&provider)).await;
    let session_id = "session_busy_background_signal_registry";
    let mut session = crate::session::Session::create_with_id(session_id.to_string(), None, None);
    session.model = Some("panic-on-fork".to_string());
    let agent = Arc::new(Mutex::new(Agent::new_with_session(
        provider, registry, session, None,
    )));

    let background_signal = {
        let agent_guard = agent.lock().await;
        agent_guard.background_tool_signal()
    };

    // Build a full control handle once (registers the background signal), then
    // simulate the busy-turn reconnect path which yields a cancel-only handle.
    let stop_signal = InterruptSignal::new();
    let soft_interrupt_queue = Arc::new(std::sync::Mutex::new(Vec::new()));
    let _full = SessionControlHandle::new(
        session_id,
        Arc::clone(&soft_interrupt_queue),
        background_signal.clone(),
        stop_signal.clone(),
    );

    let cancel_only =
        SessionControlHandle::cancel_only(session_id, soft_interrupt_queue, stop_signal);

    // The cancel-only handle has no directly-held background signal, yet it must
    // still fire the registered one.
    assert!(cancel_only.request_background_current_tool());
    assert!(background_signal.is_set());

    // Cleanup so the global registry does not leak across tests.
    crate::server::state::remove_background_tool_signal(session_id);
}

#[tokio::test]
async fn busy_agent_request_rejection_does_not_wait_for_agent_lock() {
    let provider: Arc<dyn Provider> = Arc::new(PanicOnForkProvider {
        forked: Arc::new(AtomicBool::new(false)),
    });
    let registry = Registry::new(Arc::clone(&provider)).await;
    let agent = Arc::new(Mutex::new(Agent::new(provider, registry)));
    let (client_event_tx, mut client_event_rx) = mpsc::unbounded_channel::<ServerEvent>();

    let busy_agent_lock = agent.lock().await;
    let rejected = tokio::time::timeout(Duration::from_millis(100), async {
        reject_if_agent_busy_for_request(
            17,
            "rename_session",
            "session_busy_reject",
            true,
            &agent,
            &client_event_tx,
        )
    })
    .await
    .expect("busy-agent request rejection must not wait for the agent mutex");
    assert!(rejected);
    assert!(matches!(
        client_event_rx.recv().await,
        Some(ServerEvent::Error {
            id: 17,
            retry_after_secs: Some(1),
            ..
        })
    ));

    drop(busy_agent_lock);
    assert!(!reject_if_agent_busy_for_request(
        18,
        "rename_session",
        "session_busy_reject",
        false,
        &agent,
        &client_event_tx,
    ));
    assert!(client_event_rx.try_recv().is_err());
}

#[tokio::test]
async fn cancel_without_local_task_still_signals_session_control() {
    let soft_interrupt_queue = Arc::new(std::sync::Mutex::new(Vec::new()));
    let stop_signal = InterruptSignal::new();
    let control = SessionControlHandle::cancel_only(
        "session_detached_cancel",
        soft_interrupt_queue,
        stop_signal.clone(),
    );
    let (client_event_tx, mut client_event_rx) = mpsc::unbounded_channel::<ServerEvent>();
    let swarm_members = Arc::new(RwLock::new(HashMap::new()));
    let swarms_by_id = Arc::new(RwLock::new(HashMap::new()));
    let event_history = Arc::new(RwLock::new(std::collections::VecDeque::new()));
    let event_counter = Arc::new(std::sync::atomic::AtomicU64::new(0));
    let (swarm_event_tx, _) = broadcast::channel(8);
    let mut client_is_processing = true;
    let mut message_id = Some(99);
    let mut session_id = Some("session_detached_cancel".to_string());
    let mut task = None;

    cancel_processing_message(
        &mut ProcessingState {
            client_is_processing: &mut client_is_processing,
            message_id: &mut message_id,
            session_id: &mut session_id,
            task: &mut task,
        },
        &control,
        &client_event_tx,
        &SwarmStatusRefs {
            members: &swarm_members,
            swarms_by_id: &swarms_by_id,
            event_history: &event_history,
            event_counter: &event_counter,
            event_tx: &swarm_event_tx,
        },
        Some(99),
        None,
    )
    .await;

    assert!(stop_signal.is_set());
    assert!(!client_is_processing);
    assert!(message_id.is_none());
    assert!(session_id.is_none());
    assert!(matches!(
        client_event_rx.recv().await,
        Some(ServerEvent::Interrupted)
    ));
    assert!(matches!(
        client_event_rx.recv().await,
        Some(ServerEvent::Done { id: 99 })
    ));
}

/// Regression for issue #428: the detached-turn cancel path schedules a
/// deferred reset of the shared stop signal. That reset must be epoch-guarded:
/// if a newer cancel fires during the reset window (rapid repeated Esc), the
/// stale timer must not clear it, otherwise the running turn never observes
/// the interrupt and keeps generating.
#[tokio::test]
async fn deferred_cancel_reset_does_not_erase_newer_cancel() {
    let soft_interrupt_queue = Arc::new(std::sync::Mutex::new(Vec::new()));
    let stop_signal = InterruptSignal::new();
    let control = SessionControlHandle::cancel_only(
        "session_detached_cancel_race",
        Arc::clone(&soft_interrupt_queue),
        stop_signal.clone(),
    );
    let (client_event_tx, _client_event_rx) = mpsc::unbounded_channel::<ServerEvent>();
    let swarm_members = Arc::new(RwLock::new(HashMap::new()));
    let swarms_by_id = Arc::new(RwLock::new(HashMap::new()));
    let event_history = Arc::new(RwLock::new(std::collections::VecDeque::new()));
    let event_counter = Arc::new(std::sync::atomic::AtomicU64::new(0));
    let (swarm_event_tx, _) = broadcast::channel(8);

    let cancel_via_no_task_path = async |request_id: u64| {
        let mut client_is_processing = true;
        let mut message_id = Some(request_id);
        let mut session_id = Some("session_detached_cancel_race".to_string());
        let mut task = None;
        cancel_processing_message(
            &mut ProcessingState {
                client_is_processing: &mut client_is_processing,
                message_id: &mut message_id,
                session_id: &mut session_id,
                task: &mut task,
            },
            &control,
            &client_event_tx,
            &SwarmStatusRefs {
                members: &swarm_members,
                swarms_by_id: &swarms_by_id,
                event_history: &event_history,
                event_counter: &event_counter,
                event_tx: &swarm_event_tx,
            },
            Some(request_id),
            None,
        )
        .await;
    };

    // First Esc: fires the signal and schedules a 500ms deferred reset.
    cancel_via_no_task_path(1).await;
    assert!(stop_signal.is_set());

    // 400ms later the user presses Esc again (turn still hasn't stopped).
    tokio::time::sleep(Duration::from_millis(400)).await;
    cancel_via_no_task_path(2).await;
    assert!(stop_signal.is_set());

    // The first press's timer expires now. It must NOT clear the second
    // press's still-unobserved cancel.
    tokio::time::sleep(Duration::from_millis(200)).await;
    assert!(
        stop_signal.is_set(),
        "stale deferred reset erased a newer cancel (issue #428)"
    );

    // The second press's own timer may still clear it afterwards.
    tokio::time::sleep(Duration::from_millis(600)).await;
    assert!(
        !stop_signal.is_set(),
        "the newest cancel's deferred reset should eventually clear the flag"
    );
}

impl IsolatedRuntimeDir {
    fn new() -> Self {
        let temp = tempfile::TempDir::new().expect("runtime dir");
        let prev_runtime = std::env::var_os("JCODE_RUNTIME_DIR");
        crate::env::set_var("JCODE_RUNTIME_DIR", temp.path());
        crate::server::clear_reload_marker();
        Self {
            _prev_runtime: prev_runtime,
            _temp: temp,
        }
    }
}

impl IsolatedReloadRecoveryEnv {
    fn new() -> Self {
        let home = tempfile::TempDir::new().expect("jcode home");
        let runtime = tempfile::TempDir::new().expect("runtime dir");
        let prev_home = std::env::var_os("JCODE_HOME");
        let prev_runtime = std::env::var_os("JCODE_RUNTIME_DIR");
        crate::env::set_var("JCODE_HOME", home.path());
        crate::env::set_var("JCODE_RUNTIME_DIR", runtime.path());
        crate::server::clear_reload_marker();
        Self {
            prev_home,
            prev_runtime,
            _home: home,
            _runtime: runtime,
        }
    }
}

impl Drop for IsolatedReloadRecoveryEnv {
    fn drop(&mut self) {
        crate::server::clear_reload_marker();
        if let Some(prev_home) = self.prev_home.take() {
            crate::env::set_var("JCODE_HOME", prev_home);
        } else {
            crate::env::remove_var("JCODE_HOME");
        }
        if let Some(prev_runtime) = self.prev_runtime.take() {
            crate::env::set_var("JCODE_RUNTIME_DIR", prev_runtime);
        } else {
            crate::env::remove_var("JCODE_RUNTIME_DIR");
        }
    }
}

impl Drop for IsolatedRuntimeDir {
    fn drop(&mut self) {
        crate::server::clear_reload_marker();
        if let Some(prev_runtime) = self._prev_runtime.take() {
            crate::env::set_var("JCODE_RUNTIME_DIR", prev_runtime);
        } else {
            crate::env::remove_var("JCODE_RUNTIME_DIR");
        }
    }
}

/// Regression for issue #428: a turn actively streaming in this session but
/// NOT owned by the cancelling connection (no local task handle: post-reload
/// reattach, server-initiated wake turns, headless recovery) must abort
/// promptly even when the control handle's stop signal is a *different
/// instance* from the streaming agent's own `graceful_shutdown` signal.
///
/// Before the fix, `cancel_processing_message` hit the NO_LOCAL_TASK branch,
/// fired the stale handle-local signal (which nothing was listening to),
/// emitted `Interrupted` immediately, and the provider stream kept generating
/// for minutes ("Interrupting..." disappears, model keeps going, eventually
/// "Interrupted [x66]").
#[test]
fn cancel_aborts_detached_streaming_turn_with_stale_stop_signal() -> anyhow::Result<()> {
    let _lock = crate::storage::lock_test_env();
    let _env = IsolatedReloadRecoveryEnv::new();
    let session_id = "session_detached_streaming_cancel_428";

    let rt = tokio::runtime::Runtime::new().expect("runtime");
    rt.block_on(async {
        let provider: Arc<dyn Provider> = Arc::new(NeverEndingStreamProvider);
        let registry = Registry::new(Arc::clone(&provider)).await;
        let mut session =
            crate::session::Session::create_with_id(session_id.to_string(), None, None);
        session.model = Some("never-ending-stream".to_string());
        let agent = Arc::new(Mutex::new(Agent::new_with_session(
            provider, registry, session, None,
        )));

        let (event_tx, mut event_rx) = mpsc::unbounded_channel::<ServerEvent>();

        // Start the turn the way server-initiated paths do: no entry in any
        // connection's processing-task map.
        let turn_agent = Arc::clone(&agent);
        let turn = tokio::spawn(async move {
            process_message_streaming_mpsc(turn_agent, "stream forever", Vec::new(), None, event_tx)
                .await
        });

        // Wait until the provider stream is actively producing output.
        loop {
            match tokio::time::timeout(Duration::from_secs(5), event_rx.recv()).await {
                Ok(Some(ServerEvent::TextDelta { .. })) => break,
                Ok(Some(_)) => continue,
                Ok(None) => panic!("event channel closed before streaming started"),
                Err(_) => panic!("turn never started streaming"),
            }
        }

        // Esc arrives on a connection that does not own the task. Its control
        // handle holds a stop signal instance that is NOT the streaming
        // agent's graceful_shutdown signal (stale/lost registration).
        let stale_stop_signal = InterruptSignal::new();
        let control = SessionControlHandle::cancel_only(
            session_id,
            Arc::new(std::sync::Mutex::new(Vec::new())),
            stale_stop_signal.clone(),
        );
        let (client_event_tx, _client_event_rx) = mpsc::unbounded_channel::<ServerEvent>();
        let swarm_members = Arc::new(RwLock::new(HashMap::new()));
        let swarms_by_id = Arc::new(RwLock::new(HashMap::new()));
        let event_history = Arc::new(RwLock::new(std::collections::VecDeque::new()));
        let event_counter = Arc::new(std::sync::atomic::AtomicU64::new(0));
        let (swarm_event_tx, _) = broadcast::channel(8);
        let mut client_is_processing = false;
        let mut message_id = None;
        let mut cancel_session_id = None;
        let mut task = None;

        cancel_processing_message(
            &mut ProcessingState {
                client_is_processing: &mut client_is_processing,
                message_id: &mut message_id,
                session_id: &mut cancel_session_id,
                task: &mut task,
            },
            &control,
            &client_event_tx,
            &SwarmStatusRefs {
                members: &swarm_members,
                swarms_by_id: &swarms_by_id,
                event_history: &event_history,
                event_counter: &event_counter,
                event_tx: &swarm_event_tx,
            },
            Some(1),
            None,
        )
        .await;

        // The streaming turn must observe the cancel and stop promptly, not
        // minutes later when the provider happens to finish (issue #428).
        let result = tokio::time::timeout(Duration::from_secs(2), turn)
            .await
            .expect("streaming turn must abort promptly after cancel (issue #428)")
            .expect("turn task join");
        result.expect("cancelled turn should checkpoint cleanly");

        // The turn is over, so its cancel registration must be gone and the
        // agent's own signal must be reset so the *next* turn is not aborted
        // by the consumed cancel.
        assert!(
            crate::turn_cancel_registry::active_turn_signals(session_id).is_empty(),
            "finished turn must unregister its cancel signal"
        );
        let agent_signal = {
            let agent_guard = agent.lock().await;
            agent_guard.graceful_shutdown_signal()
        };
        assert!(
            !agent_signal.is_set(),
            "consumed cancel must not leak into the next turn"
        );
    });
    Ok(())
}

struct PanicOnForkProvider {
    forked: Arc<AtomicBool>,
}

/// Streams text deltas forever (one every 20ms) until dropped. Stands in for
/// a live provider stream that only stops when the turn observes a cancel.
struct NeverEndingStreamProvider;

#[async_trait]
impl Provider for NeverEndingStreamProvider {
    async fn complete(
        &self,
        _messages: &[Message],
        _tools: &[ToolDefinition],
        _system: &str,
        _resume_session_id: Option<&str>,
    ) -> Result<EventStream> {
        Ok(Box::pin(stream::unfold(0u64, |n| async move {
            tokio::time::sleep(Duration::from_millis(20)).await;
            Some((Ok(StreamEvent::TextDelta(format!("token{} ", n))), n + 1))
        })))
    }

    fn name(&self) -> &str {
        "never-ending-stream"
    }

    fn fork(&self) -> Arc<dyn Provider> {
        Arc::new(Self)
    }
}

#[derive(Clone, Default)]
struct CompleteImmediatelyProvider;

#[async_trait]
impl Provider for CompleteImmediatelyProvider {
    async fn complete(
        &self,
        _messages: &[Message],
        _tools: &[ToolDefinition],
        _system: &str,
        _resume_session_id: Option<&str>,
    ) -> Result<EventStream> {
        Ok(Box::pin(stream::iter(vec![Ok(StreamEvent::MessageEnd {
            stop_reason: None,
        })])))
    }

    fn name(&self) -> &str {
        "complete-immediately"
    }

    fn fork(&self) -> Arc<dyn Provider> {
        Arc::new(Self)
    }
}

#[derive(Clone, Default)]
struct FanoutStreamProvider;

#[async_trait]
impl Provider for FanoutStreamProvider {
    async fn complete(
        &self,
        _messages: &[Message],
        _tools: &[ToolDefinition],
        _system: &str,
        _resume_session_id: Option<&str>,
    ) -> Result<EventStream> {
        Ok(Box::pin(stream::unfold(0_u8, |step| async move {
            match step {
                0 => Some((Ok(StreamEvent::TextDelta("before attach".to_string())), 1)),
                1 => {
                    tokio::time::sleep(Duration::from_millis(100)).await;
                    Some((Ok(StreamEvent::TextDelta("after attach".to_string())), 2))
                }
                2 => Some((
                    Ok(StreamEvent::MessageEnd {
                        stop_reason: Some("end_turn".to_string()),
                    }),
                    3,
                )),
                _ => None,
            }
        })))
    }

    fn name(&self) -> &str {
        "fanout-stream"
    }

    fn fork(&self) -> Arc<dyn Provider> {
        Arc::new(Self)
    }
}

#[async_trait]
impl Provider for PanicOnForkProvider {
    async fn complete(
        &self,
        _messages: &[Message],
        _tools: &[ToolDefinition],
        _system: &str,
        _resume_session_id: Option<&str>,
    ) -> Result<EventStream> {
        panic!("complete should never run in lightweight control test")
    }

    fn name(&self) -> &str {
        "panic-on-fork"
    }

    fn fork(&self) -> Arc<dyn Provider> {
        self.forked.store(true, Ordering::SeqCst);
        panic!("fork should not run for lightweight control requests")
    }
}

#[test]
fn ping_request_is_lightweight_control_request() {
    assert!((Request::Ping { id: 1 }).is_lightweight_control_request());
}

fn subscribe_request(working_dir: Option<&str>) -> Request {
    Request::Subscribe {
        id: 1,
        working_dir: working_dir.map(str::to_string),
        selfdev: None,
        target_session_id: None,
        client_instance_id: None,
        client_has_local_history: false,
        allow_session_takeover: false,
        terminal_env: Vec::new(),
    }
}

#[test]
fn initial_subscribe_requires_an_absolute_client_working_dir() {
    for invalid in [None, Some(""), Some("relative/project")] {
        let error = initial_subscribe_working_dir(&subscribe_request(invalid))
            .expect_err("invalid client cwd must be rejected before session creation");
        assert!(error.contains("working_dir") || error.contains("working directory"));
    }

    let absolute = std::env::temp_dir().join("jcode-client-project");
    assert_eq!(
        initial_subscribe_working_dir(&subscribe_request(absolute.to_str()))
            .expect("absolute client cwd"),
        absolute.to_string_lossy()
    );

    let error = initial_subscribe_working_dir(&Request::GetState { id: 2 })
        .expect_err("stateful requests must not create an unbound session");
    assert!(error.contains("must Subscribe"));
}

#[tokio::test]
async fn new_client_agent_stamps_client_cwd_into_initial_context() {
    let provider: Arc<dyn Provider> = Arc::new(CompleteImmediatelyProvider);
    let registry = Registry::new(Arc::clone(&provider)).await;
    let client_cwd = std::env::temp_dir().join("jcode-authoritative-client-project");
    let client_cwd = client_cwd.to_string_lossy();
    let agent = Agent::new_with_initial_working_dir(provider, registry, Some(&client_cwd));

    assert_eq!(agent.working_dir(), Some(client_cwd.as_ref()));
    let context = agent.messages()[0].content_preview();
    assert!(
        context.contains(&format!("Working directory: {client_cwd}")),
        "initial context must be created from the client cwd: {context}"
    );
}

#[test]
fn server_reload_starting_is_true_only_for_recent_starting_marker() {
    let _guard = crate::storage::lock_test_env();
    let _runtime = IsolatedRuntimeDir::new();

    assert!(!server_reload_starting());

    crate::server::write_reload_state(
        "reload-lifecycle-test",
        "test-hash",
        crate::server::ReloadPhase::Starting,
        Some("session_test_reload".to_string()),
    );
    assert!(server_reload_starting());

    crate::server::write_reload_state(
        "reload-lifecycle-test",
        "test-hash",
        crate::server::ReloadPhase::SocketReady,
        Some("session_test_reload".to_string()),
    );
    assert!(!server_reload_starting());
}

#[test]
fn reload_starting_rejects_new_turn_without_spawning_processing_task() {
    let _guard = crate::storage::lock_test_env();
    let _runtime = IsolatedRuntimeDir::new();
    crate::server::write_reload_state(
        "reload-lifecycle-starting",
        "test-hash",
        crate::server::ReloadPhase::Starting,
        Some("session_guard".to_string()),
    );

    let rt = tokio::runtime::Runtime::new().expect("runtime");
    rt.block_on(async {
        let forked = Arc::new(AtomicBool::new(false));
        let provider: Arc<dyn Provider> = Arc::new(PanicOnForkProvider {
            forked: Arc::clone(&forked),
        });
        let registry = Registry::new(Arc::clone(&provider)).await;
        let mut session =
            crate::session::Session::create_with_id("session_guard".to_string(), None, None);
        session.model = Some("panic-on-fork".to_string());
        let agent = Arc::new(Mutex::new(Agent::new_with_session(
            provider, registry, session, None,
        )));

        let (client_event_tx, mut client_event_rx) = mpsc::unbounded_channel::<ServerEvent>();
        let (processing_done_tx, mut processing_done_rx) = mpsc::unbounded_channel();
        let mut client_is_processing = false;
        let mut processing_message_id = None;
        let mut processing_session_id = None;
        let mut processing_task = None;
        let swarm_members = Arc::new(RwLock::new(HashMap::new()));
        let swarms_by_id = Arc::new(RwLock::new(HashMap::new()));
        let event_history = Arc::new(RwLock::new(std::collections::VecDeque::new()));
        let event_counter = Arc::new(std::sync::atomic::AtomicU64::new(0));
        let (swarm_event_tx, _) = broadcast::channel(8);

        start_processing_message(
            ProcessingMessage {
                id: 42,
                content: "do not start during reload".to_string(),
                images: Vec::new(),
                system_reminder: None,
            },
            "session_guard",
            &mut ProcessingState {
                client_is_processing: &mut client_is_processing,
                message_id: &mut processing_message_id,
                session_id: &mut processing_session_id,
                task: &mut processing_task,
            },
            &agent,
            &client_event_tx,
            &processing_done_tx,
            &SwarmStatusRefs {
                members: &swarm_members,
                swarms_by_id: &swarms_by_id,
                event_history: &event_history,
                event_counter: &event_counter,
                event_tx: &swarm_event_tx,
            },
        )
        .await;

        let event = client_event_rx
            .recv()
            .await
            .expect("reload event should be sent to client");
        assert!(matches!(event, ServerEvent::Reloading { new_socket: None }));
        assert!(
            client_event_rx.try_recv().is_err(),
            "reload guard should only emit the reload notification"
        );
        assert!(!client_is_processing);
        assert_eq!(processing_message_id, None);
        assert_eq!(processing_session_id, None);
        assert!(processing_task.is_none());
        assert!(processing_done_rx.try_recv().is_err());
        assert!(
            !forked.load(Ordering::SeqCst),
            "rejecting during reload should not fork or invoke provider work"
        );
    });
}

#[tokio::test]
async fn client_initiated_turn_fans_out_stream_and_terminal_events_to_live_attachments() {
    let _guard = crate::storage::lock_test_env();
    let _runtime = IsolatedRuntimeDir::new();
    let session_id = "session_live_attachment_fanout";

    let provider: Arc<dyn Provider> = Arc::new(FanoutStreamProvider);
    let registry = Registry::new(Arc::clone(&provider)).await;
    let mut session = crate::session::Session::create_with_id(session_id.to_string(), None, None);
    session.model = Some("fanout-stream".to_string());
    let agent = Arc::new(Mutex::new(Agent::new_with_session(
        provider, registry, session, None,
    )));

    let (origin_tx, mut origin_rx) = mpsc::unbounded_channel::<ServerEvent>();
    let (attached_tx, mut attached_rx) = mpsc::unbounded_channel::<ServerEvent>();
    let swarm_members = Arc::new(RwLock::new(HashMap::from([(
        session_id.to_string(),
        SwarmMember {
            session_id: session_id.to_string(),
            event_tx: origin_tx.clone(),
            event_txs: HashMap::from([("origin".to_string(), origin_tx.clone())]),
            working_dir: None,
            swarm_id: None,
            swarm_enabled: false,
            status: "ready".to_string(),
            detail: None,
            task_label: None,
            friendly_name: None,
            report_back_to_session_id: None,
            latest_completion_report: None,
            role: "agent".to_string(),
            joined_at: Instant::now(),
            last_status_change: Instant::now(),
            is_headless: false,
            output_tail: None,
            todo_progress: None,
            todo_items: Vec::new(),
            runtime: crate::protocol::SwarmMemberRuntime::default(),
        },
    )])));
    let swarms_by_id = Arc::new(RwLock::new(HashMap::new()));
    let event_history = Arc::new(RwLock::new(std::collections::VecDeque::new()));
    let event_counter = Arc::new(std::sync::atomic::AtomicU64::new(0));
    let (swarm_event_tx, _) = broadcast::channel(8);
    let (processing_done_tx, mut processing_done_rx) = mpsc::unbounded_channel();
    let mut client_is_processing = false;
    let mut processing_message_id = None;
    let mut processing_session_id = None;
    let mut processing_task = None;

    start_processing_message(
        ProcessingMessage {
            id: 479,
            content: "stream to every attachment".to_string(),
            images: Vec::new(),
            system_reminder: None,
        },
        session_id,
        &mut ProcessingState {
            client_is_processing: &mut client_is_processing,
            message_id: &mut processing_message_id,
            session_id: &mut processing_session_id,
            task: &mut processing_task,
        },
        &agent,
        &origin_tx,
        &processing_done_tx,
        &SwarmStatusRefs {
            members: &swarm_members,
            swarms_by_id: &swarms_by_id,
            event_history: &event_history,
            event_counter: &event_counter,
            event_tx: &swarm_event_tx,
        },
    )
    .await;

    loop {
        let event = tokio::time::timeout(Duration::from_secs(2), origin_rx.recv())
            .await
            .expect("origin should receive the initial stream event promptly")
            .expect("origin event channel should remain open");
        if matches!(event, ServerEvent::TextDelta { ref text } if text == "before attach") {
            break;
        }
    }

    crate::server::register_session_event_sender(
        &swarm_members,
        session_id,
        "attached",
        attached_tx,
    )
    .await;

    for rx in [&mut origin_rx, &mut attached_rx] {
        let mut saw_post_attach_delta = false;
        let mut saw_message_end = false;
        let mut saw_done = false;
        while !saw_post_attach_delta || !saw_message_end || !saw_done {
            let event = tokio::time::timeout(Duration::from_secs(2), rx.recv())
                .await
                .expect("attachment should receive streamed event promptly")
                .expect("attachment event channel should remain open");
            saw_post_attach_delta |= matches!(
                event,
                ServerEvent::TextDelta { ref text } if text == "after attach"
            );
            if matches!(event, ServerEvent::MessageEnd { .. }) {
                assert!(!saw_done, "MessageEnd must precede the terminal Done event");
                saw_message_end = true;
            }
            saw_done |= matches!(event, ServerEvent::Done { id: 479 });
        }
    }

    let (done_id, result, _) =
        tokio::time::timeout(Duration::from_secs(2), processing_done_rx.recv())
            .await
            .expect("processing should complete promptly")
            .expect("processing completion channel should remain open");
    assert_eq!(done_id, 479);
    result.expect("turn should complete successfully");

    if let Some(handle) = processing_task.take() {
        handle.await.expect("processing task join");
    }
}

#[test]
fn accepted_reload_recovery_continuation_marks_intent_delivered() -> anyhow::Result<()> {
    let _lock = crate::storage::lock_test_env();
    let _env = IsolatedReloadRecoveryEnv::new();
    let session_id = "session_accepted_reload_recovery";
    let continuation = "stored continuation accepted by server";

    super::super::reload_recovery::persist_intent(
        "reload-accepted-continuation",
        session_id,
        super::super::reload_recovery::ReloadRecoveryRole::InterruptedPeer,
        crate::tool::selfdev::ReloadRecoveryDirective {
            reconnect_notice: Some("stored notice".to_string()),
            continuation_message: continuation.to_string(),
        },
        "synthetic accepted continuation test",
    )?;
    assert!(super::super::reload_recovery::has_pending_for_session(
        session_id
    ));

    let rt = tokio::runtime::Runtime::new().expect("runtime");
    rt.block_on(async {
        let provider: Arc<dyn Provider> = Arc::new(CompleteImmediatelyProvider);
        let registry = Registry::new(Arc::clone(&provider)).await;
        let mut session =
            crate::session::Session::create_with_id(session_id.to_string(), None, None);
        session.model = Some("complete-immediately".to_string());
        let agent = Arc::new(Mutex::new(Agent::new_with_session(
            provider, registry, session, None,
        )));

        let (client_event_tx, _client_event_rx) = mpsc::unbounded_channel::<ServerEvent>();
        let (processing_done_tx, mut processing_done_rx) = mpsc::unbounded_channel();
        let mut client_is_processing = false;
        let mut processing_message_id = None;
        let mut processing_session_id = None;
        let mut processing_task = None;
        let swarm_members = Arc::new(RwLock::new(HashMap::new()));
        let swarms_by_id = Arc::new(RwLock::new(HashMap::new()));
        let event_history = Arc::new(RwLock::new(std::collections::VecDeque::new()));
        let event_counter = Arc::new(std::sync::atomic::AtomicU64::new(0));
        let (swarm_event_tx, _) = broadcast::channel(8);

        start_processing_message(
            ProcessingMessage {
                id: 77,
                content: "continue after reload".to_string(),
                images: Vec::new(),
                system_reminder: Some(continuation.to_string()),
            },
            session_id,
            &mut ProcessingState {
                client_is_processing: &mut client_is_processing,
                message_id: &mut processing_message_id,
                session_id: &mut processing_session_id,
                task: &mut processing_task,
            },
            &agent,
            &client_event_tx,
            &processing_done_tx,
            &SwarmStatusRefs {
                members: &swarm_members,
                swarms_by_id: &swarms_by_id,
                event_history: &event_history,
                event_counter: &event_counter,
                event_tx: &swarm_event_tx,
            },
        )
        .await;

        assert!(client_is_processing);
        assert_eq!(processing_message_id, Some(77));
        assert_eq!(processing_session_id.as_deref(), Some(session_id));
        assert!(processing_task.is_some());
        assert!(
            !super::super::reload_recovery::has_pending_for_session(session_id),
            "server acceptance of the exact hidden continuation should consume the durable intent"
        );

        let (done_id, result, _report) =
            tokio::time::timeout(std::time::Duration::from_secs(5), processing_done_rx.recv())
                .await
                .expect("processing task should finish")
                .expect("processing task should report completion");
        assert_eq!(done_id, 77);
        result?;
        if let Some(handle) = processing_task.take() {
            handle.await.expect("processing task join");
        }
        Ok::<(), anyhow::Error>(())
    })?;

    Ok(())
}

#[test]
fn reload_starting_rejects_new_turns_for_multiple_sessions() {
    let _guard = crate::storage::lock_test_env();
    let _runtime = IsolatedRuntimeDir::new();
    crate::server::write_reload_state(
        "reload-lifecycle-multi-starting",
        "test-hash",
        crate::server::ReloadPhase::Starting,
        Some("session_alpha".to_string()),
    );

    let rt = tokio::runtime::Runtime::new().expect("runtime");
    rt.block_on(async {
        let forked = Arc::new(AtomicBool::new(false));
        let provider: Arc<dyn Provider> = Arc::new(PanicOnForkProvider {
            forked: Arc::clone(&forked),
        });
        let registry = Registry::new(Arc::clone(&provider)).await;
        let swarm_members = Arc::new(RwLock::new(HashMap::new()));
        let swarms_by_id = Arc::new(RwLock::new(HashMap::new()));
        let event_history = Arc::new(RwLock::new(std::collections::VecDeque::new()));
        let event_counter = Arc::new(std::sync::atomic::AtomicU64::new(0));
        let (swarm_event_tx, _) = broadcast::channel(8);

        for (message_id, session_id) in [
            (101, "session_alpha"),
            (102, "session_beta"),
            (103, "session_gamma"),
        ] {
            let mut session =
                crate::session::Session::create_with_id(session_id.to_string(), None, None);
            session.model = Some("panic-on-fork".to_string());
            let agent = Arc::new(Mutex::new(Agent::new_with_session(
                Arc::clone(&provider),
                registry.clone(),
                session,
                None,
            )));

            let (client_event_tx, mut client_event_rx) = mpsc::unbounded_channel::<ServerEvent>();
            let (processing_done_tx, mut processing_done_rx) = mpsc::unbounded_channel();
            let mut client_is_processing = false;
            let mut processing_message_id = None;
            let mut processing_session_id = None;
            let mut processing_task = None;

            start_processing_message(
                ProcessingMessage {
                    id: message_id,
                    content: format!("do not start {session_id} during reload"),
                    images: Vec::new(),
                    system_reminder: None,
                },
                session_id,
                &mut ProcessingState {
                    client_is_processing: &mut client_is_processing,
                    message_id: &mut processing_message_id,
                    session_id: &mut processing_session_id,
                    task: &mut processing_task,
                },
                &agent,
                &client_event_tx,
                &processing_done_tx,
                &SwarmStatusRefs {
                    members: &swarm_members,
                    swarms_by_id: &swarms_by_id,
                    event_history: &event_history,
                    event_counter: &event_counter,
                    event_tx: &swarm_event_tx,
                },
            )
            .await;

            let event = tokio::time::timeout(
                std::time::Duration::from_millis(250),
                client_event_rx.recv(),
            )
            .await
            .expect("reload guard should emit promptly for every session")
            .expect("reload event should be sent to client");
            assert!(
                matches!(event, ServerEvent::Reloading { new_socket: None }),
                "expected Reloading event for {session_id}, got {event:?}"
            );
            assert!(
                client_event_rx.try_recv().is_err(),
                "reload guard should only emit one reload notification for {session_id}"
            );
            assert!(
                !client_is_processing,
                "{session_id} should not enter processing during reload"
            );
            assert_eq!(processing_message_id, None);
            assert_eq!(processing_session_id, None);
            assert!(
                processing_task.is_none(),
                "{session_id} should not spawn a processing task during reload"
            );
            assert!(processing_done_rx.try_recv().is_err());
        }

        assert!(
            !forked.load(Ordering::SeqCst),
            "rejecting multiple sessions during reload should not fork or invoke provider work"
        );
    });
}

#[tokio::test]
async fn lightweight_comm_request_skips_full_session_initialization() {
    let (server_stream, client_stream) = crate::transport::Stream::pair().expect("socket pair");
    let forked = Arc::new(AtomicBool::new(false));
    let provider_template: Arc<dyn Provider> = Arc::new(PanicOnForkProvider {
        forked: Arc::clone(&forked),
    });

    let sessions: SessionAgents = Arc::new(RwLock::new(HashMap::new()));
    let global_session_id = Arc::new(RwLock::new(String::new()));
    let client_count = Arc::new(RwLock::new(0usize));
    let client_connections = Arc::new(RwLock::new(HashMap::new()));
    let swarm_members = Arc::new(RwLock::new(HashMap::new()));
    let swarms_by_id = Arc::new(RwLock::new(HashMap::new()));
    let shared_context = Arc::new(RwLock::new(HashMap::new()));
    let swarm_plans = Arc::new(RwLock::new(HashMap::new()));
    let swarm_coordinators = Arc::new(RwLock::new(HashMap::new()));
    let file_touch = FileTouchService::new();
    let channel_subscriptions = Arc::new(RwLock::new(HashMap::new()));
    let channel_subscriptions_by_session = Arc::new(RwLock::new(HashMap::new()));
    let client_debug_state = Arc::new(RwLock::new(ClientDebugState::default()));
    let (_debug_response_tx, _) = broadcast::channel(8);
    let event_history = Arc::new(RwLock::new(std::collections::VecDeque::new()));
    let event_counter = Arc::new(std::sync::atomic::AtomicU64::new(0));
    let (swarm_event_tx, _) = broadcast::channel(8);
    let (_global_event_tx, _) = broadcast::channel(8);
    let global_is_processing = Arc::new(RwLock::new(false));
    let shutdown_signals = Arc::new(RwLock::new(HashMap::new()));
    let soft_interrupt_queues: SessionInterruptQueues = Arc::new(RwLock::new(HashMap::new()));
    let mcp_pool = Arc::new(crate::mcp::SharedMcpPool::from_default_config());

    let server_task = tokio::spawn(handle_client(
        server_stream,
        Arc::clone(&sessions),
        _global_event_tx,
        provider_template,
        global_is_processing,
        global_session_id,
        client_count,
        Arc::clone(&client_connections),
        swarm_members,
        swarms_by_id,
        shared_context,
        swarm_plans,
        swarm_coordinators,
        file_touch,
        channel_subscriptions,
        channel_subscriptions_by_session,
        client_debug_state,
        _debug_response_tx,
        event_history,
        event_counter,
        swarm_event_tx,
        "jcode-test".to_string(),
        "🧪".to_string(),
        mcp_pool,
        shutdown_signals,
        soft_interrupt_queues,
        AwaitMembersRuntime::default(),
        SwarmMutationRuntime::default(),
    ));

    let (client_reader, mut client_writer) = client_stream.into_split();
    let mut client_reader = BufReader::new(client_reader);
    let request = Request::CommList {
        id: 7,
        session_id: "not-in-swarm".to_string(),
    };
    let payload = serde_json::to_string(&request).expect("serialize request") + "\n";
    client_writer
        .write_all(payload.as_bytes())
        .await
        .expect("write request");

    let mut line = String::new();
    client_reader
        .read_line(&mut line)
        .await
        .expect("read ack bytes");
    let ack = decode_request_or_event(&line);
    assert!(matches!(ack, ServerEvent::Ack { id: 7 }));

    line.clear();
    client_reader
        .read_line(&mut line)
        .await
        .expect("read terminal response");
    let response = decode_request_or_event(&line);
    match response {
        ServerEvent::Error { id, message, .. } => {
            assert_eq!(id, 7);
            assert!(message.contains("Not in a swarm"));
        }
        other => panic!("expected error response, got {other:?}"),
    }

    drop(client_writer);
    server_task
        .await
        .expect("server task join")
        .expect("server task result");

    assert!(
        !forked.load(Ordering::SeqCst),
        "lightweight control request should not fork a provider"
    );
    assert!(
        client_connections.read().await.is_empty(),
        "lightweight control request should not register a live client session"
    );
    assert!(
        sessions.read().await.is_empty(),
        "lightweight control request should not allocate a live agent session"
    );
}

fn decode_request_or_event(line: &str) -> ServerEvent {
    serde_json::from_str(line.trim()).expect("decode server event")
}
