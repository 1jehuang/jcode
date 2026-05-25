#[tokio::test]
async fn await_members_any_mode_returns_after_first_match() {
    let (_env, _runtime) = RuntimeEnvGuard::new();
    let swarm_id = "swarm-any";
    let requester = "req";
    let peer_a = "peer-a";
    let peer_b = "peer-b";
    let await_runtime = AwaitMembersRuntime::default();

    let (client_tx, mut client_rx) = mpsc::unbounded_channel();
    let swarm_members = Arc::new(RwLock::new(HashMap::from([
        (requester.to_string(), member(requester, swarm_id, "ready")),
        (peer_a.to_string(), member(peer_a, swarm_id, "running")),
        (peer_b.to_string(), member(peer_b, swarm_id, "running")),
    ])));
    let swarms_by_id = Arc::new(RwLock::new(HashMap::from([(
        swarm_id.to_string(),
        HashSet::from([
            requester.to_string(),
            peer_a.to_string(),
            peer_b.to_string(),
        ]),
    )])));
    let (swarm_event_tx, _swarm_event_rx) = broadcast::channel(32);

    handle_comm_await_members(
        1,
        requester.to_string(),
        vec!["completed".to_string()],
        vec![],
        Some("any".to_string()),
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

    {
        let mut members = swarm_members.write().await;
        members.get_mut(peer_a).expect("peer a exists").status = "completed".to_string();
    }
    let _ = swarm_event_tx.send(swarm_event(
        peer_a,
        swarm_id,
        SwarmEventType::StatusChange {
            old_status: "running".to_string(),
            new_status: "completed".to_string(),
        },
    ));

    let response = tokio::time::timeout(Duration::from_secs(1), client_rx.recv())
        .await
        .expect("response should arrive")
        .expect("channel should stay open");

    match response {
        ServerEvent::CommAwaitMembersResponse {
            completed,
            members,
            summary,
            ..
        } => {
            assert!(
                completed,
                "await any should complete after first member matches"
            );
            assert!(
                summary.contains("peer-a"),
                "summary should mention matched member"
            );
            let done_members: Vec<_> = members.into_iter().filter(|member| member.done).collect();
            assert_eq!(done_members.len(), 1);
            assert_eq!(done_members[0].session_id, peer_a);
        }
        other => panic!("expected CommAwaitMembersResponse, got {other:?}"),
    }
}
