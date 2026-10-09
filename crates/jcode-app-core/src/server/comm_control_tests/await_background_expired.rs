#[tokio::test]
async fn await_members_background_already_expired_answers_tool_call() {
    let (_env, _runtime_dir) = RuntimeEnvGuard::new();
    let swarm_id = "swarm-bg-expired";
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
    // A pending background state whose deadline already passed. A retry of the
    // same request must get an explicit timeout response instead of hanging
    // until the client-side socket timeout.
    crate::server::await_members_state::save_state(
        &crate::server::await_members_state::PersistedAwaitMembersState {
            key,
            session_id: requester.to_string(),
            swarm_id: swarm_id.to_string(),
            target_status: vec!["completed".to_string()],
            requested_ids: vec![],
            mode: None,
            created_at_unix_ms: now_ms.saturating_sub(120_000),
            deadline_unix_ms: now_ms.saturating_sub(1_000),
            background: true,
            notify: false,
            wake: false,
            auto_rearm_count: 0,
            base_timeout_secs: None,
            last_progress_ms: None,
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
        7,
        requester.to_string(),
        vec!["completed".to_string()],
        vec![],
        None,
        Some(60),
        true,
        false,
        false,
        CommAwaitMembersContext {
            client_event_tx: &client_tx,
            swarm_members: &swarm_members,
            swarms_by_id: &swarms_by_id,
            swarm_event_tx: &swarm_event_tx,
            await_members_runtime: &await_runtime,
        },
    )
    .await;

    let response = tokio::time::timeout(Duration::from_secs(1), client_rx.recv())
        .await
        .expect("expired background await must answer the tool call")
        .expect("channel should stay open");

    match response {
        ServerEvent::CommAwaitMembersResponse {
            id,
            completed,
            summary,
            background_started,
            ..
        } => {
            assert_eq!(id, 7);
            assert!(!completed, "expired await should report a timeout");
            assert!(summary.contains("Timed out"), "summary: {summary}");
            assert!(
                !background_started,
                "an already-expired wait should not claim a background watcher started"
            );
        }
        other => panic!("expected CommAwaitMembersResponse, got {other:?}"),
    }
}
