use super::*;

#[test]
fn cancel_soft_interrupts_by_id_reports_each_requested_id_exactly_once() {
    let queue = Arc::new(std::sync::Mutex::new(Vec::new()));
    let control = SessionControlHandle::cancel_only(
        "session_cancel_by_id_test",
        Arc::clone(&queue),
        InterruptSignal::new(),
    );
    let user = SoftInterruptSource::User;
    for (content, id) in [
        ("a", Some("f1")),
        ("b", None),
        ("c", Some("f2")),
        ("d", Some("f3")),
    ] {
        assert!(control.queue_soft_interrupt_with_id(
            content.into(),
            Vec::new(),
            false,
            user,
            id.map(str::to_string),
        ));
    }
    assert_eq!(control.pending_soft_interrupt_count(), 4);

    let requested: Vec<String> = ["f3", "gone", "f1", "f3"].map(String::from).to_vec();
    let (cancelled, not_queued) = control.cancel_soft_interrupts_by_id(&requested);
    assert_eq!(cancelled, vec!["f3".to_string(), "f1".to_string()]);
    assert_eq!(not_queued, vec!["gone".to_string()]);

    let remaining: Vec<String> = queue
        .lock()
        .expect("queue lock")
        .iter()
        .map(|message| message.content.clone())
        .collect();
    assert_eq!(remaining, vec!["b".to_string(), "c".to_string()]);

    // Once drained (injected), the same id can no longer be cancelled.
    queue.lock().expect("queue lock").clear();
    let (cancelled, not_queued) = control.cancel_soft_interrupts_by_id(&["f2".to_string()]);
    assert!(cancelled.is_empty());
    assert_eq!(not_queued, vec!["f2".to_string()]);
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
async fn context_message_persists_without_starting_turn() {
    let _guard = crate::storage::lock_test_env();
    let _env = IsolatedReloadRecoveryEnv::new();
    let session_id = "session_context_only_no_reply";
    let forked = Arc::new(AtomicBool::new(false));
    let provider: Arc<dyn Provider> = Arc::new(PanicOnForkProvider {
        forked: Arc::clone(&forked),
    });
    let registry = Registry::new(Arc::clone(&provider)).await;
    let mut session = crate::session::Session::create_with_id(session_id.to_string(), None, None);
    session.model = Some("panic-on-fork".to_string());
    let agent = Arc::new(Mutex::new(Agent::new_with_session(
        provider, registry, session, None,
    )));
    let (client_event_tx, mut client_event_rx) = mpsc::unbounded_channel::<ServerEvent>();
    let before = agent.lock().await.message_count();

    append_context_message(
        77,
        "remember this context",
        vec![("image/png".to_string(), "AAA".to_string())],
        (session_id, None),
        false,
        &agent,
        &client_event_tx,
    )
    .await;

    assert!(matches!(
        client_event_rx.recv().await,
        Some(ServerEvent::ContextMessageAdded { id: 77 })
    ));
    assert!(client_event_rx.try_recv().is_err());
    assert!(!forked.load(Ordering::SeqCst));

    let persisted = crate::session::Session::load(session_id).expect("persisted session");
    assert_eq!(persisted.messages.len(), before + 1);
    let message = persisted.messages.last().unwrap();
    assert_eq!(format!("{:?}", message.role), "User");
    assert!(matches!(
        &message.content[0],
        ContentBlock::Image { media_type, data }
            if media_type == "image/png" && data == "AAA"
    ));
    assert!(matches!(
        &message.content[1],
        ContentBlock::Text { text, .. } if text == "remember this context"
    ));
}

#[tokio::test]
async fn context_message_rejects_while_busy_without_waiting_for_agent_lock() {
    let provider: Arc<dyn Provider> = Arc::new(PanicOnForkProvider {
        forked: Arc::new(AtomicBool::new(false)),
    });
    let registry = Registry::new(Arc::clone(&provider)).await;
    let agent = Arc::new(Mutex::new(Agent::new(provider, registry)));
    let (client_event_tx, mut client_event_rx) = mpsc::unbounded_channel::<ServerEvent>();
    let _busy_agent_lock = agent.lock().await;

    tokio::time::timeout(Duration::from_millis(100), async {
        append_context_message(
            78,
            "too busy",
            Vec::new(),
            ("session_context_busy", None),
            true,
            &agent,
            &client_event_tx,
        )
        .await;
    })
    .await
    .expect("busy rejection must not wait for the agent mutex");

    assert!(matches!(
        client_event_rx.recv().await,
        Some(ServerEvent::Error {
            id: 78,
            retry_after_secs: Some(1),
            ..
        })
    ));
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
    // The point of this path is a turn this connection does not own (attach
    // after reload, server-initiated turn). Without a registered active turn
    // the cancel is a deliberate no-op, because arming the signal with nothing
    // running only kills the *next* message.
    let _active_turn = crate::turn_cancel_registry::register_active_turn(
        "session_detached_cancel",
        InterruptSignal::new(),
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
        None,
    )
    .await;

    assert!(stop_signal.is_set());
    assert!(!client_is_processing);
    assert!(message_id.is_none());
    assert!(session_id.is_none());
    assert!(matches!(
        client_event_rx.recv().await,
        Some(ServerEvent::TurnStopped {
            reason: crate::protocol::TurnStopReason::Interrupted,
            ..
        })
    ));
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
    // A turn owned by another connection is what makes this the signalling
    // path rather than the idle no-op; see the sibling test.
    let _active_turn = crate::turn_cancel_registry::register_active_turn(
        "session_detached_cancel_race",
        InterruptSignal::new(),
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
