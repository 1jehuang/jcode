#[tokio::test]
async fn await_members_returns_persisted_final_response_after_reload_retry() {
    let (_env, _runtime_dir) = RuntimeEnvGuard::new();
    let swarm_id = "swarm-d";
    let requester = "req";
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
            deadline_unix_ms: now_ms + 60_000,
            final_response: Some(
                crate::server::await_members_state::PersistedAwaitMembersResult {
                    completed: true,
                    members: vec![crate::protocol::AwaitedMemberStatus {
                        session_id: "peer-1".to_string(),
                        friendly_name: Some("peer-1".to_string()),
                        status: "completed".to_string(),
                        done: true,
                        completion_report: None,
                    }],
                    summary: "All 1 members are done: peer-1".to_string(),
                    resolved_at_unix_ms: now_ms,
                },
            ),
        },
    );

    let await_runtime = AwaitMembersRuntime::default();
    let (client_tx, mut client_rx) = mpsc::unbounded_channel();
    let swarm_members = Arc::new(RwLock::new(HashMap::from([(
        requester.to_string(),
        member(requester, swarm_id, "ready"),
    )])));
    let swarms_by_id = Arc::new(RwLock::new(HashMap::from([(
        swarm_id.to_string(),
        HashSet::from([requester.to_string()]),
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

    match client_rx.recv().await.expect("response should arrive") {
        ServerEvent::CommAwaitMembersResponse {
            completed,
            summary,
            members,
            ..
        } => {
            assert!(completed);
            assert_eq!(summary, "All 1 members are done: peer-1");
            assert_eq!(members.len(), 1);
            assert_eq!(members[0].session_id, "peer-1");
        }
        other => panic!("expected CommAwaitMembersResponse, got {other:?}"),
    }
}
