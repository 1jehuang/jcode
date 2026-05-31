#[tokio::test]
async fn await_members_reuses_persisted_deadline_after_reload_retry() {
    let (_env, _runtime_dir) = RuntimeEnvGuard::new();
    let swarm_id = "swarm-c";
    let requester = "req";
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
    crate::server::await_members_state::save_state(
        &crate::server::await_members_state::PersistedAwaitMembersState {
            key,
            session_id: requester.to_string(),
            swarm_id: swarm_id.to_string(),
            target_status: vec!["completed".to_string()],
            requested_ids: vec![],
            mode: None,
            created_at_unix_ms: now_ms,
            deadline_unix_ms: now_ms + 150,
            final_response: None,
        },
    );

    let await_runtime = AwaitMembersRuntime::default();
    let (client_tx, mut client_rx) = mpsc::unbounded_channel();
    let swarm_members = Arc::new(RwLock::new(HashMap::from([
        (requester.to_string(), member(requester, swarm_id, "ready")),
        (peer.to_string(), member(peer, swarm_id, "running")),
    ])));
    let swarms_by_id = Arc::new(RwLock::new(HashMap::from([(
        swarm_id.to_string(),
        HashSet::from([requester.to_string(), peer.to_string()]),
    )])));
    let (swarm_event_tx, _swarm_event_rx) = broadcast::channel(32);

    handle_comm_await_members(
        1,
        requester.to_string(),
        vec!["completed".to_string()],
        vec![],
        None,
        Some(60),
        CommAwaitMembersContext {
            client_event_tx: &client_tx,
            swarm_members: &swarm_members,
            swarms_by_id: &swarms_by_id,
            swarm_event_tx: &swarm_event_tx,
            await_members_runtime: &await_runtime,
        },
    )
    .await;

    let started = Instant::now();
    let response = tokio::time::timeout(Duration::from_secs(1), client_rx.recv())
        .await
        .expect("response should arrive")
        .expect("channel should stay open");

    assert!(
        started.elapsed() < Duration::from_secs(1),
        "persisted deadline should win over new timeout"
    );

    match response {
        ServerEvent::CommAwaitMembersResponse {
            completed, summary, ..
        } => {
            assert!(!completed, "persisted expired wait should time out");
            assert!(summary.contains("Timed out"));
        }
        other => panic!("expected CommAwaitMembersResponse, got {other:?}"),
    }
}
