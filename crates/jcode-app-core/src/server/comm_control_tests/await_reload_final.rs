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

#[tokio::test]
async fn await_members_ignores_persisted_final_when_requested_member_is_queued_again() {
    let (_env, _runtime_dir) = RuntimeEnvGuard::new();
    let swarm_id = "swarm-requeue";
    let requester = "req";
    let peer = "peer-1";
    let target_status = vec!["completed".to_string()];
    let requested_ids = vec![peer.to_string()];
    let key = crate::server::await_members_state::request_key(
        requester,
        swarm_id,
        &requested_ids,
        &target_status,
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
            target_status: target_status.clone(),
            requested_ids: requested_ids.clone(),
            mode: None,
            created_at_unix_ms: now_ms,
            deadline_unix_ms: now_ms + 60_000,
            final_response: Some(
                crate::server::await_members_state::PersistedAwaitMembersResult {
                    completed: true,
                    members: vec![crate::protocol::AwaitedMemberStatus {
                        session_id: peer.to_string(),
                        friendly_name: Some(peer.to_string()),
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
    let swarm_members = Arc::new(RwLock::new(HashMap::from([
        (requester.to_string(), member(requester, swarm_id, "ready")),
        (peer.to_string(), member(peer, swarm_id, "queued")),
    ])));
    let swarms_by_id = Arc::new(RwLock::new(HashMap::from([(
        swarm_id.to_string(),
        HashSet::from([requester.to_string(), peer.to_string()]),
    )])));
    let (swarm_event_tx, _swarm_event_rx) = broadcast::channel(32);

    handle_comm_await_members(
        2,
        requester.to_string(),
        target_status,
        requested_ids,
        None,
        Some(5),
        CommAwaitMembersContext {
            client_event_tx: &client_tx,
            swarm_members: &swarm_members,
            swarms_by_id: &swarms_by_id,
            swarm_event_tx: &swarm_event_tx,
            await_members_runtime: &await_runtime,
        },
    )
    .await;

    assert!(
        tokio::time::timeout(Duration::from_millis(50), client_rx.recv())
            .await
            .is_err(),
        "stale persisted final response should not be replayed while the worker is queued again"
    );

    {
        let mut members = swarm_members.write().await;
        members.get_mut(peer).expect("peer exists").status = "completed".to_string();
    }
    let _ = swarm_event_tx.send(swarm_event(
        peer,
        swarm_id,
        SwarmEventType::StatusChange {
            old_status: "queued".to_string(),
            new_status: "completed".to_string(),
        },
    ));

    match tokio::time::timeout(Duration::from_secs(1), client_rx.recv())
        .await
        .expect("await response should arrive after completion")
        .expect("response should be sent")
    {
        ServerEvent::CommAwaitMembersResponse {
            completed, members, ..
        } => {
            assert!(completed);
            assert_eq!(members.len(), 1);
            assert_eq!(members[0].session_id, peer);
        }
        other => panic!("expected CommAwaitMembersResponse, got {other:?}"),
    }
}
