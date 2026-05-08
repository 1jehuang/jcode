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
        false,
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

#[tokio::test]
async fn await_members_owned_only_snapshots_owned_non_terminal_workers() {
    let (_env, _runtime) = RuntimeEnvGuard::new();
    let swarm_id = "swarm-owned";
    let requester = "coord";
    let owned_running = "owned-running";
    let owned_ready = "owned-ready";
    let other_owned_running = "other-owned-running";
    let user_created_running = "user-created-running";
    let await_runtime = AwaitMembersRuntime::default();

    let (client_tx, mut client_rx) = mpsc::unbounded_channel();
    let mut owned_running_member = member(owned_running, swarm_id, "running");
    owned_running_member.report_back_to_session_id = Some(requester.to_string());
    let mut owned_ready_member = member(owned_ready, swarm_id, "ready");
    owned_ready_member.report_back_to_session_id = Some(requester.to_string());
    let mut other_owned_member = member(other_owned_running, swarm_id, "running");
    other_owned_member.report_back_to_session_id = Some("other-coord".to_string());

    let swarm_members = Arc::new(RwLock::new(HashMap::from([
        (requester.to_string(), member(requester, swarm_id, "ready")),
        (owned_running.to_string(), owned_running_member),
        (owned_ready.to_string(), owned_ready_member),
        (other_owned_running.to_string(), other_owned_member),
        (
            user_created_running.to_string(),
            member(user_created_running, swarm_id, "running"),
        ),
    ])));
    let swarms_by_id = Arc::new(RwLock::new(HashMap::from([(
        swarm_id.to_string(),
        HashSet::from([
            requester.to_string(),
            owned_running.to_string(),
            owned_ready.to_string(),
            other_owned_running.to_string(),
            user_created_running.to_string(),
        ]),
    )])));
    let (swarm_event_tx, _swarm_event_rx) = broadcast::channel(32);

    handle_comm_await_members(
        1,
        requester.to_string(),
        vec![
            "ready".to_string(),
            "completed".to_string(),
            "stopped".to_string(),
            "failed".to_string(),
        ],
        vec![],
        true,
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

    {
        let mut members = swarm_members.write().await;
        members
            .get_mut(owned_running)
            .expect("owned running exists")
            .status = "ready".to_string();
    }
    let _ = swarm_event_tx.send(swarm_event(
        owned_running,
        swarm_id,
        SwarmEventType::StatusChange {
            old_status: "running".to_string(),
            new_status: "ready".to_string(),
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
            assert!(completed, "owned worker should be enough to complete await");
            let watched = members
                .into_iter()
                .map(|member| member.session_id)
                .collect::<HashSet<_>>();
            assert_eq!(watched, HashSet::from([owned_running.to_string()]));
        }
        other => panic!("expected CommAwaitMembersResponse, got {other:?}"),
    }
}

#[tokio::test]
async fn await_members_owned_only_empty_snapshot_does_not_fall_back_to_whole_swarm() {
    let (_env, _runtime) = RuntimeEnvGuard::new();
    let swarm_id = "swarm-owned-empty";
    let requester = "coord";
    let owned_ready = "owned-ready";
    let owned_crashed = "owned-crashed";
    let other_running = "other-running";
    let await_runtime = AwaitMembersRuntime::default();

    let (client_tx, mut client_rx) = mpsc::unbounded_channel();
    let mut owned_ready_member = member(owned_ready, swarm_id, "ready");
    owned_ready_member.report_back_to_session_id = Some(requester.to_string());
    let mut owned_crashed_member = member(owned_crashed, swarm_id, "crashed");
    owned_crashed_member.report_back_to_session_id = Some(requester.to_string());

    let swarm_members = Arc::new(RwLock::new(HashMap::from([
        (requester.to_string(), member(requester, swarm_id, "ready")),
        (owned_ready.to_string(), owned_ready_member),
        (owned_crashed.to_string(), owned_crashed_member),
        (other_running.to_string(), member(other_running, swarm_id, "running")),
    ])));
    let swarms_by_id = Arc::new(RwLock::new(HashMap::from([(
        swarm_id.to_string(),
        HashSet::from([
            requester.to_string(),
            owned_ready.to_string(),
            owned_crashed.to_string(),
            other_running.to_string(),
        ]),
    )])));
    let (swarm_event_tx, _swarm_event_rx) = broadcast::channel(32);

    handle_comm_await_members(
        1,
        requester.to_string(),
        vec![
            "ready".to_string(),
            "completed".to_string(),
            "stopped".to_string(),
            "failed".to_string(),
        ],
        vec![],
        true,
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

    let response = tokio::time::timeout(std::time::Duration::from_secs(1), client_rx.recv())
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
            assert!(completed, "empty owned-only snapshot should finish immediately");
            assert!(members.is_empty(), "should not fall back to whole swarm");
            assert!(
                summary.contains("No scoped await_members candidates found"),
                "summary should explain scoped default, got {summary:?}"
            );
        }
        other => panic!("expected CommAwaitMembersResponse, got {other:?}"),
    }
}
