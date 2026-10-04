#![cfg_attr(test, allow(clippy::await_holding_lock))]
#![cfg_attr(test, allow(clippy::await_holding_lock))]

use super::*;
#[tokio::test]
async fn notify_session_runs_scheduled_task_immediately_for_idle_live_session() {
    let provider = Arc::new(StreamingMockProvider::default());
    provider.queue_response(vec![
        StreamEvent::TextDelta("Working on scheduled task.".to_string()),
        StreamEvent::MessageEnd { stop_reason: None },
    ]);
    let provider_dyn: Arc<dyn Provider> = provider.clone();
    let registry = Registry::new(provider_dyn.clone()).await;
    let agent = Arc::new(Mutex::new(Agent::new(provider_dyn, registry)));
    let session_id = agent.lock().await.session_id().to_string();
    let sessions = Arc::new(RwLock::new(HashMap::<String, Arc<Mutex<Agent>>>::from([(
        session_id.clone(),
        agent.clone(),
    )])));
    let soft_interrupt_queues = Arc::new(RwLock::new(HashMap::new()));
    let client_connections = Arc::new(RwLock::new(HashMap::from([(
        "client-1".to_string(),
        ClientConnectionInfo {
            client_id: "client-1".to_string(),
            session_id: session_id.clone(),
            client_instance_id: None,
            debug_client_id: Some("debug-1".to_string()),
            connected_at: Instant::now(),
            last_seen: Instant::now(),
            is_processing: false,
            current_tool_name: None,
            terminal_env: Vec::new(),
            disconnect_tx: mpsc::unbounded_channel().0,
        },
    )])));
    let (member_event_tx, mut member_event_rx) = mpsc::unbounded_channel();
    let swarm_members = Arc::new(RwLock::new(HashMap::from([(
        session_id.clone(),
        SwarmMember {
            session_id: session_id.clone(),
            event_tx: member_event_tx,
            event_txs: HashMap::new(),
            working_dir: None,
            swarm_id: None,
            swarm_enabled: false,
            status: "ready".to_string(),
            detail: None,
            task_label: None,
            friendly_name: Some("otter".to_string()),
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
    let (client_event_tx, mut client_event_rx) = mpsc::unbounded_channel();

    let (swarms_by_id, event_history, event_counter, swarm_event_tx) = empty_swarm_status_state();
    handle_notify_session(
        77,
        session_id.clone(),
        "[Scheduled task]\nTask: Follow up".to_string(),
        NotifySessionContext {
            sessions: &sessions,
            soft_interrupt_queues: &soft_interrupt_queues,
            client_connections: &client_connections,
            swarm_members: &swarm_members,
            swarms_by_id: &swarms_by_id,
            event_history: &event_history,
            event_counter: &event_counter,
            swarm_event_tx: &swarm_event_tx,
            client_event_tx: &client_event_tx,
        },
    )
    .await;

    let streamed_event = timeout(Duration::from_secs(2), async {
        loop {
            match member_event_rx.recv().await {
                Some(ServerEvent::TextDelta { text })
                    if text.contains("Working on scheduled task.") =>
                {
                    return text;
                }
                Some(_) => continue,
                None => panic!("live member stream closed before scheduled task ran"),
            }
        }
    })
    .await
    .expect("scheduled task should start streaming promptly");
    assert!(streamed_event.contains("Working on scheduled task."));

    let client_events: Vec<_> = std::iter::from_fn(|| client_event_rx.try_recv().ok()).collect();
    assert!(
        client_events
            .iter()
            .any(|event| matches!(event, ServerEvent::Done { id } if *id == 77))
    );

    let guard = agent.lock().await;
    assert!(guard.messages().iter().any(|message| {
        message.role == Role::User
            && message.display_role == Some(crate::session::StoredDisplayRole::System)
            && message
                .content_preview()
                .contains("[Scheduled task] Task: Follow up")
    }));
    assert!(guard.messages().iter().any(|message| {
        message.role == Role::Assistant
            && message
                .content_preview()
                .contains("Working on scheduled task.")
    }));
}

#[tokio::test]
async fn notify_session_queues_soft_interrupt_when_live_session_is_busy() {
    let provider: Arc<dyn Provider> = Arc::new(MockProvider);
    let registry = Registry::new(provider.clone()).await;
    let agent = Arc::new(Mutex::new(Agent::new(provider, registry)));
    let session_id = agent.lock().await.session_id().to_string();
    let queue = agent.lock().await.soft_interrupt_queue();

    let sessions = Arc::new(RwLock::new(HashMap::<String, Arc<Mutex<Agent>>>::from([(
        session_id.clone(),
        agent.clone(),
    )])));
    let soft_interrupt_queues = Arc::new(RwLock::new(HashMap::from([(
        session_id.clone(),
        queue.clone(),
    )])));
    let client_connections = Arc::new(RwLock::new(HashMap::from([(
        "client-1".to_string(),
        ClientConnectionInfo {
            client_id: "client-1".to_string(),
            session_id: session_id.clone(),
            client_instance_id: None,
            debug_client_id: Some("debug-1".to_string()),
            connected_at: Instant::now(),
            last_seen: Instant::now(),
            is_processing: false,
            current_tool_name: None,
            terminal_env: Vec::new(),
            disconnect_tx: mpsc::unbounded_channel().0,
        },
    )])));
    let (member_event_tx, mut member_event_rx) = mpsc::unbounded_channel();
    let swarm_members = Arc::new(RwLock::new(HashMap::from([(
        session_id.clone(),
        SwarmMember {
            session_id: session_id.clone(),
            event_tx: member_event_tx,
            event_txs: HashMap::new(),
            working_dir: None,
            swarm_id: None,
            swarm_enabled: false,
            status: "running".to_string(),
            detail: None,
            task_label: None,
            friendly_name: Some("otter".to_string()),
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
    let (client_event_tx, mut client_event_rx) = mpsc::unbounded_channel();

    let _busy_guard = agent.lock().await;

    let (swarms_by_id, event_history, event_counter, swarm_event_tx) = empty_swarm_status_state();
    handle_notify_session(
        88,
        session_id.clone(),
        "[Scheduled task]\nTask: Follow up while busy".to_string(),
        NotifySessionContext {
            sessions: &sessions,
            soft_interrupt_queues: &soft_interrupt_queues,
            client_connections: &client_connections,
            swarm_members: &swarm_members,
            swarms_by_id: &swarms_by_id,
            event_history: &event_history,
            event_counter: &event_counter,
            swarm_event_tx: &swarm_event_tx,
            client_event_tx: &client_event_tx,
        },
    )
    .await;

    let member_event = timeout(Duration::from_secs(2), member_event_rx.recv())
        .await
        .expect("notification should arrive promptly")
        .expect("live member should receive notification");
    match member_event {
        ServerEvent::Notification {
            from_session,
            from_name,
            message,
            ..
        } => {
            assert_eq!(from_session, "schedule");
            assert_eq!(from_name.as_deref(), Some("scheduled task"));
            assert!(message.contains("Task: Follow up while busy"));
        }
        other => panic!("expected notification event, got {other:?}"),
    }

    let queued = queue.lock().unwrap();
    assert_eq!(
        queued.len(),
        1,
        "scheduled task should queue as soft interrupt"
    );
    assert!(queued[0].content.contains("Task: Follow up while busy"));
    drop(queued);

    let client_events: Vec<_> = std::iter::from_fn(|| client_event_rx.try_recv().ok()).collect();
    assert!(
        client_events
            .iter()
            .any(|event| matches!(event, ServerEvent::Done { id } if *id == 88))
    );
}
