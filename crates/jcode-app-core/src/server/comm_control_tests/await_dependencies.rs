#[tokio::test]
async fn await_dependency_id_survives_retry_and_releases_on_completion() {
    let (_env, runtime_dir) = RuntimeEnvGuard::new();
    let requester = "dependency-owner";
    let peer = "dependency-peer";
    let swarm_id = "dependency-swarm";
    let runtime = AwaitMembersRuntime::default();
    let members = Arc::new(RwLock::new(HashMap::from([
        (requester.into(), member(requester, swarm_id, "ready")),
        (peer.into(), member(peer, swarm_id, "running")),
    ])));
    let swarms = Arc::new(RwLock::new(HashMap::from([(
        swarm_id.into(),
        HashSet::from([requester.into(), peer.into()]),
    )])));
    let (events, _) = broadcast::channel(32);
    let (client, mut responses) = mpsc::unbounded_channel();
    let mut ids = Vec::new();
    for request_id in [1, 2] {
        handle_comm_await_members(
            request_id,
            requester.into(),
            vec!["completed".into()],
            vec![peer.into()],
            None,
            Some(60),
            true,
            false,
            false,
            CommAwaitMembersContext {
                swarm_members: &members,
                swarms_by_id: &swarms,
                swarm_event_tx: &events,
                await_members_runtime: &runtime,
                client_event_tx: &client,
            },
        )
        .await;
        let response = responses.recv().await.unwrap();
        let ServerEvent::CommAwaitMembersResponse {
            summary,
            background_started,
            ..
        } = response
        else {
            panic!("unexpected response: {response:?}");
        };
        assert!(background_started);
        let id = summary
            .split("Dependency ID: `")
            .nth(1)
            .unwrap()
            .split('`')
            .next()
            .unwrap()
            .to_string();
        assert!(summary.contains("todo.waiting_on"));
        assert!(crate::todo::await_dependency::running(
            runtime_dir.path(),
            &id,
            requester
        ));
        assert!(!crate::todo::await_dependency::running(
            runtime_dir.path(),
            &id,
            peer
        ));
        ids.push(id);
    }
    assert_eq!(ids[0], ids[1], "duplicate requests retain their generation");
    crate::server::subscribe_await_dependency(&ids[0], requester).unwrap();
    let mut bus = crate::bus::Bus::global().subscribe();
    members.write().await.get_mut(peer).unwrap().status = "completed".into();
    let _ = events.send(swarm_event(
        peer,
        swarm_id,
        SwarmEventType::StatusChange {
            old_status: "running".into(),
            new_status: "completed".into(),
        },
    ));
    let finished = tokio::time::timeout(Duration::from_secs(2), async {
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
    assert!(finished.completed && finished.notify && finished.wake);
    assert!(!crate::todo::await_dependency::running(
        runtime_dir.path(),
        &ids[0],
        requester
    ));
    let terminal = crate::todo::await_dependency::load(runtime_dir.path(), &ids[0]).unwrap();
    assert!(terminal.final_response.is_some());
    // Completion won the race with a late subscription: it must stay terminal.
    crate::server::subscribe_await_dependency(&ids[0], requester).unwrap();
    assert!(!crate::todo::await_dependency::running(
        runtime_dir.path(),
        &ids[0],
        requester
    ));
}

#[test]
fn await_dependency_generation_and_delivery_are_completion_race_safe() {
    use crate::server::await_members_state::*;
    let (_env, runtime_dir) = RuntimeEnvGuard::new();
    let now = crate::todo::await_dependency::now_unix_ms();
    let first = ensure_pending_state(
        "owner-race",
        "owner",
        "swarm",
        &[],
        &[],
        None,
        now + 60_000,
        true,
        false,
        false,
    );
    let id = first.dependency_id();
    assert!(subscribe_dependency(&id, "foreign").is_err());
    subscribe_dependency(&id, "owner").unwrap();
    // Finalization must use the subscribed prefs, not the watcher's old copy.
    let (final_state, newly_finalized) =
        persist_final_response(&first, true, vec![], "done".into());
    assert!(newly_finalized);
    assert!(final_state.notify && final_state.wake);
    let (_, newly_finalized) =
        persist_final_response(&first, false, vec![], "racing timeout".into());
    assert!(
        !newly_finalized,
        "duplicate finalization must not send a second wake"
    );
    save_state(&first); // A stale pending copy cannot resurrect a final wait.
    assert!(!crate::todo::await_dependency::running(
        runtime_dir.path(),
        &id,
        "owner"
    ));
    let second = ensure_pending_state(
        "owner-race",
        "owner",
        "swarm",
        &[],
        &[],
        None,
        now + 60_000,
        true,
        false,
        false,
    );
    assert_ne!(id, second.dependency_id());
    assert!(crate::todo::await_dependency::load(runtime_dir.path(), &id).is_none());
    subscribe_dependency(&id, "owner").unwrap();
    let second = load_state("owner-race").unwrap();
    assert!(
        !second.notify && !second.wake,
        "old subscription cannot affect a new generation"
    );
    persist_final_response(&first, false, vec![], "late timeout".into());
    let still_pending = load_state("owner-race").unwrap();
    assert_eq!(still_pending.dependency_id(), second.dependency_id());
    assert!(
        still_pending.is_pending(),
        "old finalizer cannot finish the new generation"
    );
}

#[tokio::test]
async fn await_dependency_restores_watcher_with_same_id_and_delivery_flags() {
    use crate::server::await_members_state::ensure_pending_state;
    let (_env, runtime_dir) = RuntimeEnvGuard::new();
    let requester = "restore-owner";
    let peer = "restore-peer";
    let swarm_id = "restore-swarm";
    let state = ensure_pending_state(
        "restore-key",
        requester,
        swarm_id,
        &[peer.into()],
        &["completed".into()],
        None,
        crate::todo::await_dependency::now_unix_ms() + 60_000,
        true,
        true,
        false,
    );
    let id = state.dependency_id();
    let members = Arc::new(RwLock::new(HashMap::from([
        (requester.into(), member(requester, swarm_id, "ready")),
        (peer.into(), member(peer, swarm_id, "running")),
    ])));
    let swarms = Arc::new(RwLock::new(HashMap::from([(
        swarm_id.into(),
        HashSet::from([requester.into(), peer.into()]),
    )])));
    let (events, _) = broadcast::channel(32);
    let restored_runtime = AwaitMembersRuntime::default();
    crate::server::comm_await::resume_background_awaits(
        &members,
        &swarms,
        &events,
        &restored_runtime,
    )
    .await;
    assert!(crate::todo::await_dependency::running(
        runtime_dir.path(),
        &id,
        requester
    ));
    let mut bus = crate::bus::Bus::global().subscribe();
    members.write().await.get_mut(peer).unwrap().status = "completed".into();
    let _ = events.send(swarm_event(
        peer,
        swarm_id,
        SwarmEventType::StatusChange {
            old_status: "running".into(),
            new_status: "completed".into(),
        },
    ));
    let finished = tokio::time::timeout(Duration::from_secs(2), async {
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
    assert!(finished.completed && finished.notify);
    assert!(!finished.wake, "reload must retain explicit wake=false");
    assert!(!crate::todo::await_dependency::running(
        runtime_dir.path(),
        &id,
        requester
    ));
    assert_eq!(
        crate::server::await_members_state::load_state("restore-key")
            .unwrap()
            .dependency_id(),
        id
    );
}
