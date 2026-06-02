#[tokio::test]
async fn await_members_includes_late_joiners_when_watching_swarm() {
    let (_env, _runtime) = RuntimeEnvGuard::new();
    let swarm_id = "swarm-a";
    let requester = "req";
    let initial_peer = "peer-1";
    let late_peer = "peer-2";
    let await_runtime = AwaitMembersRuntime::default();

    let (client_tx, mut client_rx) = mpsc::unbounded_channel();
    let swarm_members = Arc::new(RwLock::new(HashMap::from([
        (requester.to_string(), member(requester, swarm_id, "ready")),
        (
            initial_peer.to_string(),
            member(initial_peer, swarm_id, "running"),
        ),
    ])));
    let swarms_by_id = Arc::new(RwLock::new(HashMap::from([(
        swarm_id.to_string(),
        HashSet::from([requester.to_string(), initial_peer.to_string()]),
    )])));
    let (swarm_event_tx, _swarm_event_rx) = broadcast::channel(32);

    handle_comm_await_members(
        1,
        requester.to_string(),
        vec!["completed".to_string()],
        vec![],
        None,
        Some(2),
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
        members.insert(
            late_peer.to_string(),
            member(late_peer, swarm_id, "running"),
        );
    }
    {
        let mut swarms = swarms_by_id.write().await;
        swarms
            .get_mut(swarm_id)
            .expect("swarm exists")
            .insert(late_peer.to_string());
    }
    let _ = swarm_event_tx.send(swarm_event(
        late_peer,
        swarm_id,
        SwarmEventType::MemberChange {
            action: "joined".to_string(),
        },
    ));

    {
        let mut members = swarm_members.write().await;
        members
            .get_mut(initial_peer)
            .expect("initial peer exists")
            .status = "completed".to_string();
    }
    let _ = swarm_event_tx.send(swarm_event(
        initial_peer,
        swarm_id,
        SwarmEventType::StatusChange {
            old_status: "running".to_string(),
            new_status: "completed".to_string(),
        },
    ));

    {
        let mut members = swarm_members.write().await;
        members.get_mut(late_peer).expect("late peer exists").status = "completed".to_string();
    }
    let _ = swarm_event_tx.send(swarm_event(
        late_peer,
        swarm_id,
        SwarmEventType::StatusChange {
            old_status: "running".to_string(),
            new_status: "completed".to_string(),
        },
    ));

    let response = tokio::time::timeout(std::time::Duration::from_secs(1), client_rx.recv())
        .await
        .expect("response should arrive")
        .expect("channel should stay open");

    match response {
        ServerEvent::CommAwaitMembersResponse {
            completed, members, ..
        } => {
            assert!(completed, "await should complete after both peers finish");
            let watched: HashSet<String> = members.into_iter().map(|m| m.session_id).collect();
            assert!(watched.contains(initial_peer));
            assert!(watched.contains(late_peer));
        }
        other => panic!("expected CommAwaitMembersResponse, got {other:?}"),
    }
}
