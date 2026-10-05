//! Tests for session control handle behavior while the agent lock is busy.
use super::*;

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
