#[tokio::test]
async fn await_members_stops_when_requesting_client_disconnects() {
    let (_env, _runtime) = RuntimeEnvGuard::new();
    let swarm_id = "swarm-b";
    let requester = "req";
    let peer = "peer-1";
    let await_runtime = AwaitMembersRuntime::default();

    let (client_tx, client_rx) = mpsc::unbounded_channel();
    let swarm_members = Arc::new(RwLock::new(HashMap::from([
        (requester.to_string(), member(requester, swarm_id, "ready")),
        (peer.to_string(), member(peer, swarm_id, "running")),
    ])));
    let swarms_by_id = Arc::new(RwLock::new(HashMap::from([(
        swarm_id.to_string(),
        HashSet::from([requester.to_string(), peer.to_string()]),
    )])));
    let (swarm_event_tx, swarm_event_rx) = broadcast::channel(32);
    drop(swarm_event_rx);
    let baseline_receivers = swarm_event_tx.receiver_count();

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

    drop(client_rx);

    tokio::time::timeout(std::time::Duration::from_secs(1), async {
        loop {
            if swarm_event_tx.receiver_count() == baseline_receivers {
                break;
            }
            tokio::time::sleep(std::time::Duration::from_millis(10)).await;
        }
    })
    .await
    .expect("await task should unsubscribe promptly after client disconnect");
}
