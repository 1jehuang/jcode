#[tokio::test]
async fn cancelled_task_progress_retains_turn_until_scoped_events_drain() {
    let agent = test_agent().await;
    let lease = agent.lock().await.claim_server_turn().unwrap();
    let members = Arc::new(RwLock::new(HashMap::new()));
    let blocked = members.write().await;
    let (fallback, mut received) = mpsc::unbounded_channel();
    let events = Arc::new(crate::server::state::TurnEventChannel::new(
        "progress-test".to_string(),
        Arc::clone(&members),
        fallback,
        &lease,
    ));
    let (bus, _) = broadcast::channel(16);
    let progress = super::task_progress_event_sender(
        "progress-test".to_string(),
        "swarm".to_string(),
        "task".to_string(),
        Arc::clone(&members),
        Arc::new(RwLock::new(HashMap::new())),
        Arc::new(RwLock::new(HashMap::new())),
        Arc::new(RwLock::new(HashMap::new())),
        Arc::new(RwLock::new(VecDeque::new())),
        Arc::new(AtomicU64::new(0)),
        bus,
        Arc::clone(&events),
        lease.publication_pin(),
    );
    progress
        .sender()
        .send(ServerEvent::TextDelta {
            text: "accepted".to_string(),
        })
        .unwrap();
    // Simulate the task owner being cancelled before either queue has drained.
    drop(progress);
    drop(events);
    drop(lease);
    assert!(agent.lock().await.claim_server_turn().is_err());
    drop(blocked);
    let started = tokio::time::timeout(Duration::from_secs(1), received.recv())
        .await
        .unwrap()
        .unwrap();
    assert!(matches!(started, ServerEvent::TurnStarted { .. }));
    let event = tokio::time::timeout(Duration::from_secs(1), received.recv())
        .await
        .unwrap()
        .unwrap();
    assert!(
        matches!(event, ServerEvent::TurnEvent { event, .. } if matches!(*event, ServerEvent::TextDelta { ref text } if text == "accepted"))
    );
    tokio::time::timeout(Duration::from_secs(1), async {
        loop {
            if let Ok(next) = agent.lock().await.claim_server_turn() {
                drop(next);
                break;
            }
            tokio::task::yield_now().await;
        }
    })
    .await
    .expect("publication reservation released after drain");
}
