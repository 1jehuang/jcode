// Reproduce the screenshot: many historical, overlapping waits recovered
// after their workers have disappeared from the daemon's runtime registry.
#[tokio::test]
async fn resume_background_awaits_coalesces_missing_workers_without_false_success_or_wake() {
    use crate::server::await_members_state::*;
    let (_env, runtime_dir) = RuntimeEnvGuard::new();
    let requester = "recovery-spam-owner";
    let now = crate::todo::await_dependency::now_unix_ms();
    let peers = ["session_duck_123_abc", "session_butterfly_456_def"];
    let mut states = Vec::new();
    for index in 0..20 {
        let state = PersistedAwaitMembersState {
            key: format!("spam-recovery-{index}"),
            session_id: requester.into(),
            swarm_id: "recovery-spam-swarm".into(),
            target_status: vec!["completed".into(), "stopped".into()],
            requested_ids: peers.iter().map(|id| (*id).into()).collect(),
            mode: Some(if index % 2 == 0 { "all" } else { "any" }.into()),
            created_at_unix_ms: now - 120_000,
            deadline_unix_ms: now - 60_000,
            background: true,
            notify: true,
            wake: true,
            final_response: None,
        };
        save_state(&state);
        states.push(state);
    }
    let members = Arc::new(RwLock::new(HashMap::new()));
    let swarms = Arc::new(RwLock::new(HashMap::new()));
    let (events, _) = broadcast::channel(32);
    let mut bus = crate::bus::Bus::global().subscribe();
    for _ in 0..2 {
        // A second daemon recovery must not redeliver terminal generations.
        crate::server::comm_await::resume_background_awaits(
            &members,
            &swarms,
            &events,
            &AwaitMembersRuntime::default(),
        )
        .await;
    }
    let mut delivered = Vec::new();
    while let Ok(event) = bus.try_recv() {
        if let crate::bus::BusEvent::SwarmAwaitCompleted(event) = event
            && event.session_id == requester
        {
            delivered.push(event);
        }
    }
    assert_eq!(
        delivered.len(),
        1,
        "one notice per requester, not per old wait"
    );
    let event = &delivered[0];
    assert!(!event.completed);
    assert!(event.notify);
    assert!(
        !event.wake,
        "missing historical workers must not revive an idle agent"
    );
    assert!(event.summary.contains("Recovered 20 swarm wait(s)"));
    assert!(event.summary.contains("20 timed out"));
    assert!(event.notification.contains("session_duck_123_abc (unknown)"));
    assert_eq!(
        event.notification.matches("(unknown)").count(),
        2,
        "overlapping workers are listed once"
    );
    for state in states {
        let result = load_state(&state.key).unwrap().final_response.unwrap();
        assert!(!result.completed);
        assert!(result.members.iter().all(|member| !member.done));
        assert!(!crate::todo::await_dependency::running(
            runtime_dir.path(),
            &state.dependency_id(),
            requester
        ));
    }
}

#[tokio::test]
async fn resume_background_awaits_empty_registry_is_timeout_not_vacuous_success() {
    use crate::server::await_members_state::*;
    let (_env, _runtime_dir) = RuntimeEnvGuard::new();
    let now = crate::todo::await_dependency::now_unix_ms();
    let state = PersistedAwaitMembersState {
        key: "recovery-empty".into(),
        session_id: "recovery-empty-owner".into(),
        swarm_id: "empty-swarm".into(),
        target_status: vec!["completed".into()],
        requested_ids: vec![],
        mode: None,
        created_at_unix_ms: now - 120_000,
        deadline_unix_ms: now - 60_000,
        background: true,
        notify: true,
        wake: true,
        final_response: None,
    };
    save_state(&state);
    let (events, _) = broadcast::channel(16);
    crate::server::comm_await::resume_background_awaits(
        &Arc::new(RwLock::new(HashMap::new())),
        &Arc::new(RwLock::new(HashMap::new())),
        &events,
        &AwaitMembersRuntime::default(),
    )
    .await;
    let result = load_state(&state.key).unwrap().final_response.unwrap();
    assert!(!result.completed);
    assert!(result.summary.contains("No watched members"));
}

#[tokio::test]
async fn resume_background_awaits_missing_peer_waits_for_real_status_after_reload() {
    use crate::server::await_members_state::*;
    let (_env, _runtime_dir) = RuntimeEnvGuard::new();
    let requester = "recovery-hydration-owner";
    let peer = "recovery-hydration-peer";
    let swarm_id = "recovery-hydration-swarm";
    let state = ensure_pending_state(
        "recovery-hydration",
        requester,
        swarm_id,
        &[peer.into()],
        &["completed".into(), "stopped".into()],
        None,
        crate::todo::await_dependency::now_unix_ms() + 60_000,
        true,
        true,
        true,
    );
    let members = Arc::new(RwLock::new(HashMap::new()));
    let swarms = Arc::new(RwLock::new(HashMap::new()));
    let (events, _) = broadcast::channel(16);
    let runtime = AwaitMembersRuntime::default();
    crate::server::comm_await::resume_background_awaits(&members, &swarms, &events, &runtime).await;
    // Yield to the detached watcher while the registry is still unhydrated.
    tokio::time::sleep(Duration::from_millis(20)).await;
    assert!(load_state(&state.key).unwrap().is_pending());
    let mut bus = crate::bus::Bus::global().subscribe();
    members
        .write()
        .await
        .insert(peer.into(), member(peer, swarm_id, "completed"));
    events
        .send(swarm_event(
            peer,
            swarm_id,
            SwarmEventType::StatusChange {
                old_status: "running".into(),
                new_status: "completed".into(),
            },
        ))
        .unwrap();
    let event = tokio::time::timeout(Duration::from_secs(2), async {
        loop {
            if let Ok(crate::bus::BusEvent::SwarmAwaitCompleted(event)) = bus.recv().await
                && event.session_id == requester
            {
                break event;
            }
        }
    })
    .await
    .unwrap();
    assert!(event.completed && event.notify && event.wake);
    assert!(!load_state(&state.key).unwrap().is_pending());
}

#[tokio::test]
async fn awaited_member_unknown_never_satisfies_terminal_targets() {
    let members = Arc::new(RwLock::new(HashMap::new()));
    let swarms = Arc::new(RwLock::new(HashMap::new()));
    let statuses = crate::server::comm_await::awaited_member_statuses(
        "owner",
        "swarm",
        &["missing".into()],
        &["completed".into(), "stopped".into(), "unknown".into()],
        &members,
        &swarms,
    )
    .await;
    assert_eq!(statuses[0].status, "unknown");
    assert!(!statuses[0].done);
    assert!(!crate::server::comm_await::mode_satisfied(
        &statuses,
        Some("any")
    ));
    assert!(!crate::server::comm_await::mode_satisfied(
        &statuses,
        Some("all")
    ));
}
