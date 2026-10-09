#[tokio::test]
async fn resume_background_awaits_finalizes_states_expired_while_down() {
    let (_env, _runtime_dir) = RuntimeEnvGuard::new();
    let swarm_id = "swarm-resume-expired";
    let requester = "req-resume-expired";
    let peer = "peer-1";
    let key = crate::server::await_members_state::request_key(
        requester,
        swarm_id,
        &[],
        &["completed".to_string()],
        None,
    );
    let now_ms = SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .unwrap_or_default()
        .as_millis() as u64;
    // A background await whose deadline passed "while the server was down".
    crate::server::await_members_state::save_state(
        &crate::server::await_members_state::PersistedAwaitMembersState {
            key: key.clone(),
            session_id: requester.to_string(),
            swarm_id: swarm_id.to_string(),
            target_status: vec!["completed".to_string()],
            requested_ids: vec![],
            mode: None,
            created_at_unix_ms: now_ms.saturating_sub(120_000),
            deadline_unix_ms: now_ms.saturating_sub(60_000),
            background: true,
            notify: true,
            wake: true,
            auto_rearm_count: 0,
            base_timeout_secs: None,
            last_progress_ms: None,
            final_response: None,
        },
    );

    let await_runtime = AwaitMembersRuntime::default();
    let swarm_members = Arc::new(RwLock::new(HashMap::from([
        (requester.to_string(), member(requester, swarm_id, "ready")),
        (peer.to_string(), member(peer, swarm_id, "running")),
    ])));
    let swarms_by_id = Arc::new(RwLock::new(HashMap::from([(
        swarm_id.to_string(),
        HashSet::from([requester.to_string(), peer.to_string()]),
    )])));
    let (swarm_event_tx, _swarm_event_rx) = broadcast::channel(32);

    let mut bus_rx = crate::bus::Bus::global().subscribe();

    crate::server::comm_await::resume_background_awaits(
        &swarm_members,
        &swarms_by_id,
        &swarm_event_tx,
        &await_runtime,
    )
    .await;

    // The expired state must be finalized as a timeout so the promised
    // notify/wake fires.
    let event = tokio::time::timeout(Duration::from_secs(2), async {
        loop {
            match bus_rx.recv().await {
                Ok(crate::bus::BusEvent::SwarmAwaitCompleted(event))
                    if event.session_id == requester =>
                {
                    return event;
                }
                Ok(_) => continue,
                Err(tokio::sync::broadcast::error::RecvError::Lagged(_)) => continue,
                Err(tokio::sync::broadcast::error::RecvError::Closed) => {
                    panic!("bus closed before SwarmAwaitCompleted arrived")
                }
            }
        }
    })
    .await
    .expect("expired background await should publish SwarmAwaitCompleted on resume");

    assert!(!event.completed, "expired await should finalize as timeout");
    assert!(event.summary.contains("Timed out"), "summary: {}", event.summary);
    assert!(event.notify);
    assert!(event.wake);

    let final_state = crate::server::await_members_state::load_state(&key)
        .expect("state should still be persisted");
    let final_response = final_state
        .final_response
        .expect("expired await should have a persisted final response");
    assert!(!final_response.completed);
    assert!(final_response.summary.contains("Timed out"));
}
