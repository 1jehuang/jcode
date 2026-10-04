/// DECISIVE discriminator for the pinned `assign_next` replay bug (upstream
/// #1209).
///
/// The audit's central claim is that the client-side dedup added in
/// `communicate.rs` is necessary because the SERVER replays a stale
/// `task_id` when `assign_next` is called repeatedly with a pinned
/// `target_session`. The mechanism, read from source:
///
/// 1. `handle_comm_assign_next` takes the `target_session.is_none()` branch only
///    for the unpinned case. A pinned call falls straight through to
///    `handle_comm_assign_task(.., task_id = None, ..)`
///    (comm_control.rs:2018-2037).
/// 2. That becomes `handle_comm_assign_task_with_mode` with
///    `AssignDedupMode::ReplayFinal` (comm_control.rs:1416).
/// 3. The dedup key is
///    `[swarm_id, requested_target, requested_task_id or "__next_runnable__", message]`
///    (comm_control.rs:1481-1494). On the pinned path `requested_task_id` is
///    always `None`, so it collapses to the constant `"__next_runnable__"`. The
///    key does NOT include the request id. Two consecutive pinned `assign_next`
///    calls to the SAME worker therefore produce an IDENTICAL key.
/// 4. `begin_with_mode` (swarm_mutation_state.rs:219-224) sees a persisted
///    final response under that key and REPLAYS it, returning `None`, so
///    `handle_comm_assign_task_with_mode` returns at comm_control.rs:1519-1521
///    without touching the plan at all.
///
/// If that is what happens, the second call reports the first call's
/// `task_id`, the plan frontier does not advance, and a `fill_slots` loop sees
/// the same node handed back every iteration -- exactly the reported symptom of
/// "8 identical assignments of catalog-evaluate to session_kangaroo".
///
/// The unpinned path cannot exhibit this: it picks its task id BEFORE building
/// the key and passes that concrete id into `handle_comm_assign_task`, so each
/// iteration gets a distinct key. That asymmetry is why the pre-existing
/// unpinned top-up test passes and the pinned case is the one that needs
/// proof.
///
/// DISCRIMINATING POWER: if this test FAILS (second task_id equals the first),
/// the pinned replay bug is CONFIRMED at the server layer and the client-side
/// dedup in `communicate.rs` is load-bearing. If it PASSES, the audit's central
/// claim is WRONG and the client-side dedup is not covering what it claims to.
/// There is no third outcome: the assertion is a direct comparison of the two
/// responses, and the plan-state assertion below independently checks that the
/// frontier actually advanced.
#[tokio::test]
async fn assign_next_pinned_target_advances_the_frontier_instead_of_replaying() {
    let (_env, _runtime) = RuntimeEnvGuard::new();
    let swarm_id = "swarm-pinned-replay";
    let requester = "coord";
    let pinned_worker = "worker-pinned";
    let (client_tx, mut client_rx) = mpsc::unbounded_channel();
    let sessions = Arc::new(RwLock::new(HashMap::new()));
    let soft_interrupt_queues = Arc::new(RwLock::new(HashMap::new()));
    let client_connections = Arc::new(RwLock::new(HashMap::new()));
    let swarm_members = Arc::new(RwLock::new(HashMap::from([
        (requester.to_string(), {
            let mut m = member(requester, swarm_id, "ready");
            m.role = "coordinator".to_string();
            m
        }),
        (
            pinned_worker.to_string(),
            owned_member(pinned_worker, swarm_id, "ready", requester),
        ),
    ])));
    let swarms_by_id = Arc::new(RwLock::new(HashMap::from([(
        swarm_id.to_string(),
        HashSet::from([requester.to_string(), pinned_worker.to_string()]),
    )])));
    // Three READY, UNASSIGNED nodes. A wide ready frontier is the precondition
    // of the reported symptom (one node, many slots).
    let swarm_plans = Arc::new(RwLock::new(HashMap::from([(
        swarm_id.to_string(),
        VersionedPlan {
            items: vec![
                plan_item("task-a", "queued", "high", &[]),
                plan_item("task-b", "queued", "high", &[]),
                plan_item("task-c", "queued", "high", &[]),
            ],
            version: 1,
            participants: HashSet::from([requester.to_string(), pinned_worker.to_string()]),
            task_progress: HashMap::new(),
            mode: "light".to_string(),
            node_meta: HashMap::new(),
        },
    )])));
    let swarm_coordinators = Arc::new(RwLock::new(HashMap::from([(
        swarm_id.to_string(),
        requester.to_string(),
    )])));
    let event_history = Arc::new(RwLock::new(VecDeque::new()));
    let event_counter = Arc::new(AtomicU64::new(1));
    let (swarm_event_tx, _swarm_event_rx) = broadcast::channel(32);
    let mutation_runtime = SwarmMutationRuntime::default();
    let provider: Arc<dyn Provider> = Arc::new(TestProvider);
    let global_session_id = Arc::new(RwLock::new(String::new()));
    let mcp_pool = Arc::new(crate::mcp::SharedMcpPool::from_default_config());

    // ---- Call 1: pinned assign_next. -------------------------------------------------
    handle_comm_assign_next(
        9001,
        requester.to_string(),
        Some(pinned_worker.to_string()),
        None,
        None,
        None,
        None,
        None,
        None,
        &client_tx,
        &sessions,
        &global_session_id,
        &provider,
        &soft_interrupt_queues,
        &client_connections,
        &swarm_members,
        &swarms_by_id,
        &swarm_plans,
        &swarm_coordinators,
        &event_history,
        &event_counter,
        &swarm_event_tx,
        &mcp_pool,
        &mutation_runtime,
    )
    .await;

    let first_task_id = match client_rx.recv().await.expect("first response") {
        ServerEvent::CommAssignTaskResponse {
            id,
            task_id,
            target_session,
        } => {
            assert_eq!(id, 9001, "first response must carry the first request id");
            assert_eq!(
                target_session, pinned_worker,
                "first pinned assignment must land on the pinned worker"
            );
            task_id
        }
        other => panic!("expected CommAssignTaskResponse on call 1, got {other:?}"),
    };

    // ---- Call 2: IDENTICAL pinned assign_next. --------------------------------------
    // Same coordinator, same pinned target, no explicit task id, no message. The only
    // difference is the request id, which the dedup key deliberately excludes.
    handle_comm_assign_next(
        9002,
        requester.to_string(),
        Some(pinned_worker.to_string()),
        None,
        None,
        None,
        None,
        None,
        None,
        &client_tx,
        &sessions,
        &global_session_id,
        &provider,
        &soft_interrupt_queues,
        &client_connections,
        &swarm_members,
        &swarms_by_id,
        &swarm_plans,
        &swarm_coordinators,
        &event_history,
        &event_counter,
        &swarm_event_tx,
        &mcp_pool,
        &mutation_runtime,
    )
    .await;

    let second_task_id = match client_rx.recv().await.expect("second response") {
        ServerEvent::CommAssignTaskResponse {
            id,
            task_id,
            target_session,
        } => {
            assert_eq!(id, 9002, "second response must carry the second request id");
            assert_eq!(
                target_session, pinned_worker,
                "second pinned assignment must also land on the pinned worker"
            );
            task_id
        }
        other => panic!("expected CommAssignTaskResponse on call 2, got {other:?}"),
    };

    // Plan state, read independently of the responses above.
    let plans = swarm_plans.read().await;
    let assigned: Vec<String> = plans
        .get(swarm_id)
        .expect("plan still present")
        .items
        .iter()
        .filter(|item| item.assigned_to.as_deref() == Some(pinned_worker))
        .map(|item| item.id.clone())
        .collect();
    drop(plans);

    assert_ne!(
        second_task_id, first_task_id,
        "CONFIRMED pinned assign_next replay: two consecutive pinned assign_next calls \
         to the same worker returned the SAME task_id ({second_task_id:?}). The dedup key \
         collapses the pinned path to a constant \"__next_runnable__\" component \
         (comm_control.rs:1481-1494), so AssignDedupMode::ReplayFinal replays the first \
         response within FINAL_STATE_TTL instead of advancing the frontier. Plan items \
         now assigned to {pinned_worker}: {assigned:?}."
    );

    assert_eq!(
        assigned.len(),
        2,
        "two pinned assign_next calls must advance the ready frontier by two distinct \
         nodes, but only {:?} ended up assigned to {pinned_worker} (first response said \
         {first_task_id:?}, second said {second_task_id:?})",
        assigned
    );
}
/// POSITIVE CONTROL for the test above, and the reason that test's failure is
/// attributable to the dedup key rather than to a fixture that could never
/// advance.
///
/// Identical in every respect to
/// `assign_next_pinned_target_advances_the_frontier_instead_of_replaying`
/// -- same coordinator, same three ready unassigned nodes, same plan, same
/// runtime -- EXCEPT that the second call is pinned to a DIFFERENT worker.
///
/// The dedup key is `[swarm_id, requested_target, requested_task_id or
/// "__next_runnable__", message]` (comm_control.rs:1481-1494), so a different
/// target yields a different key, the persisted final response under the first
/// key is not consulted, and the frontier is expected to advance. That makes
/// this the exact one-variable experiment the failing test needs: the only
/// difference between a run that advances and a run that replays is whether
/// the pinned target is the SAME worker twice.
///
/// If THIS test also failed to advance, the failure of its sibling would be a
/// property of the fixture (no runnable node, a busy-member filter, a
/// coordinator-permission error) rather than proof of the replay bug.
#[tokio::test]
async fn assign_next_pinned_target_change_does_advance_the_frontier() {
    let (_env, _runtime) = RuntimeEnvGuard::new();
    let swarm_id = "swarm-pinned-control";
    let requester = "coord";
    let first_worker = "worker-first";
    let second_worker = "worker-second";
    let (client_tx, mut client_rx) = mpsc::unbounded_channel();
    let sessions = Arc::new(RwLock::new(HashMap::new()));
    let soft_interrupt_queues = Arc::new(RwLock::new(HashMap::new()));
    let client_connections = Arc::new(RwLock::new(HashMap::new()));
    let swarm_members = Arc::new(RwLock::new(HashMap::from([
        (requester.to_string(), {
            let mut m = member(requester, swarm_id, "ready");
            m.role = "coordinator".to_string();
            m
        }),
        (
            first_worker.to_string(),
            owned_member(first_worker, swarm_id, "ready", requester),
        ),
        (
            second_worker.to_string(),
            owned_member(second_worker, swarm_id, "ready", requester),
        ),
    ])));
    let swarms_by_id = Arc::new(RwLock::new(HashMap::from([(
        swarm_id.to_string(),
        HashSet::from([
            requester.to_string(),
            first_worker.to_string(),
            second_worker.to_string(),
        ]),
    )])));
    let swarm_plans = Arc::new(RwLock::new(HashMap::from([(
        swarm_id.to_string(),
        VersionedPlan {
            items: vec![
                plan_item("task-a", "queued", "high", &[]),
                plan_item("task-b", "queued", "high", &[]),
                plan_item("task-c", "queued", "high", &[]),
            ],
            version: 1,
            participants: HashSet::from([
                requester.to_string(),
                first_worker.to_string(),
                second_worker.to_string(),
            ]),
            task_progress: HashMap::new(),
            mode: "light".to_string(),
            node_meta: HashMap::new(),
        },
    )])));
    let swarm_coordinators = Arc::new(RwLock::new(HashMap::from([(
        swarm_id.to_string(),
        requester.to_string(),
    )])));
    let event_history = Arc::new(RwLock::new(VecDeque::new()));
    let event_counter = Arc::new(AtomicU64::new(1));
    let (swarm_event_tx, _swarm_event_rx) = broadcast::channel(32);
    let mutation_runtime = SwarmMutationRuntime::default();
    let provider: Arc<dyn Provider> = Arc::new(TestProvider);
    let global_session_id = Arc::new(RwLock::new(String::new()));
    let mcp_pool = Arc::new(crate::mcp::SharedMcpPool::from_default_config());

    handle_comm_assign_next(
        9101,
        requester.to_string(),
        Some(first_worker.to_string()),
        None,
        None,
        None,
        None,
        None,
        None,
        &client_tx,
        &sessions,
        &global_session_id,
        &provider,
        &soft_interrupt_queues,
        &client_connections,
        &swarm_members,
        &swarms_by_id,
        &swarm_plans,
        &swarm_coordinators,
        &event_history,
        &event_counter,
        &swarm_event_tx,
        &mcp_pool,
        &mutation_runtime,
    )
    .await;

    let first_task_id = match client_rx.recv().await.expect("first response") {
        ServerEvent::CommAssignTaskResponse {
            task_id,
            target_session,
            ..
        } => {
            assert_eq!(
                target_session, first_worker,
                "control: first call must land on the first pinned worker"
            );
            task_id
        }
        other => panic!("expected CommAssignTaskResponse on call 1, got {other:?}"),
    };

    // Same fixture, same coordinator, same ready frontier. Only the pinned
    // target changes.
    handle_comm_assign_next(
        9102,
        requester.to_string(),
        Some(second_worker.to_string()),
        None,
        None,
        None,
        None,
        None,
        None,
        &client_tx,
        &sessions,
        &global_session_id,
        &provider,
        &soft_interrupt_queues,
        &client_connections,
        &swarm_members,
        &swarms_by_id,
        &swarm_plans,
        &swarm_coordinators,
        &event_history,
        &event_counter,
        &swarm_event_tx,
        &mcp_pool,
        &mutation_runtime,
    )
    .await;

    let second_task_id = match client_rx.recv().await.expect("second response") {
        ServerEvent::CommAssignTaskResponse {
            task_id,
            target_session,
            ..
        } => {
            assert_eq!(
                target_session, second_worker,
                "control: second call must land on the second pinned worker"
            );
            task_id
        }
        other => panic!("expected CommAssignTaskResponse on call 2, got {other:?}"),
    };

    let plans = swarm_plans.read().await;
    let assigned: Vec<String> = plans
        .get(swarm_id)
        .expect("plan still present")
        .items
        .iter()
        .filter(|item| item.assigned_to.is_some())
        .map(|item| item.id.clone())
        .collect();
    drop(plans);

    // This is the property the failing sibling cannot reach. It must hold here,
    // otherwise the fixture is incapable of advancing and proves nothing.
    assert_ne!(
        second_task_id, first_task_id,
        "CONTROL FAILED: pinning to a DIFFERENT worker did not advance the frontier \
         (both calls returned {first_task_id:?}). The fixture cannot demonstrate \
         advancement at all, so the replay failure of its sibling would be a \
         fixture artifact rather than proof of the dedup bug."
    );

    assert_eq!(
        assigned.len(),
        2,
        "control: two pinned calls to two different workers must assign two distinct \
         nodes, but {assigned:?} are assigned"
    );
}

/// POLICYWATCH: the invariant that makes the pinned-path fix and concurrent-request
/// coalescing hold AT THE SAME TIME. This is the risk the fix introduces, stated
/// and then discharged.
///
/// `handle_comm_assign_next` now resolves a concrete task id on the pinned path
/// BEFORE delegating, so the `assign_task` mutation key's task component varies
/// per call instead of collapsing to the constant `"__next_runnable__"`. That is
/// what makes two SEQUENTIAL pinned calls advance the frontier, which
/// `assign_next_pinned_target_advances_the_frontier_instead_of_replaying` proves.
///
/// The obvious way to over-correct would be to let the key vary so much that two
/// CONCURRENT identical pinned requests stop coalescing and BOTH execute, handing
/// one worker two nodes from one logical request. That is precisely the
/// double-dispatch the dedup layer exists to prevent (`begin_with_mode`'s waiter
/// registration and `active_keys` claim, `swarm_mutation_state.rs:225-234`).
///
/// The property that satisfies both: the new pre-resolution is a plans **READ**
/// and the plan **WRITE** happens later, inside the handler. Two duplicates that
/// both resolve before either writes agree on the task id, so they compute the
/// SAME key and the mutation layer coalesces them exactly as it always did.
/// Sequential calls diverge because the first one's write already moved the
/// frontier before the second one resolves.
///
/// FORCING THE INTERLEAVING. Holding the plans write lock parks both calls at
/// their pre-resolution read. That is why this test cannot reuse the EDGE 1
/// strategy in `assign_next_replay_edges.rs`: that test waits for the pending
/// mutation-state file to appear, and the fix now writes that file only AFTER the
/// read this lock is holding, so the file never appears while the lock is held and
/// the wait times out. This test parks on the task's own progress instead.
///
/// The parking is airtight rather than lucky: for a light-mode coordinator
/// `require_plan_driver_swarm` returns at `comm_control.rs:2601` WITHOUT touching
/// the plans lock (it only reads members, then coordinators), so the new
/// pre-resolution read is the FIRST plan access on the pinned path. A call that
/// cannot acquire that read therefore cannot have reached the mutation claim, and
/// a call that cannot write therefore cannot have dispatched.
#[tokio::test]
async fn concurrent_pinned_assign_next_still_coalesces_onto_one_execution() {
    let (_env, _runtime) = RuntimeEnvGuard::new();
    let swarm_id = "swarm-pinned-coalesce";
    let requester = "coord";
    let pinned_worker = "worker-pinned";
    let (client_tx, mut client_rx) = mpsc::unbounded_channel();
    let sessions = Arc::new(RwLock::new(HashMap::new()));
    let soft_interrupt_queues = Arc::new(RwLock::new(HashMap::new()));
    let client_connections = Arc::new(RwLock::new(HashMap::new()));
    let swarm_members = Arc::new(RwLock::new(HashMap::from([
        (requester.to_string(), {
            let mut m = member(requester, swarm_id, "ready");
            m.role = "coordinator".to_string();
            m
        }),
        (
            pinned_worker.to_string(),
            owned_member(pinned_worker, swarm_id, "ready", requester),
        ),
    ])));
    let swarms_by_id = Arc::new(RwLock::new(HashMap::from([(
        swarm_id.to_string(),
        HashSet::from([requester.to_string(), pinned_worker.to_string()]),
    )])));
    // Wide ready frontier, exactly as in the sequential test, so a failure here
    // cannot be explained by "there was nothing left to pick".
    let swarm_plans = Arc::new(RwLock::new(HashMap::from([(
        swarm_id.to_string(),
        VersionedPlan {
            items: vec![
                plan_item("task-a", "queued", "high", &[]),
                plan_item("task-b", "queued", "high", &[]),
                plan_item("task-c", "queued", "high", &[]),
            ],
            version: 1,
            participants: HashSet::from([requester.to_string(), pinned_worker.to_string()]),
            task_progress: HashMap::new(),
            mode: "light".to_string(),
            node_meta: HashMap::new(),
        },
    )])));
    let swarm_coordinators = Arc::new(RwLock::new(HashMap::from([(
        swarm_id.to_string(),
        requester.to_string(),
    )])));
    let event_history = Arc::new(RwLock::new(VecDeque::new()));
    let event_counter = Arc::new(AtomicU64::new(1));
    let (swarm_event_tx, _swarm_event_rx) = broadcast::channel(32);
    // ONE runtime shared by both calls: `active_keys` and the waiter map live in
    // it, so a per-call runtime would make coalescing impossible to observe and
    // the test would measure nothing.
    let mutation_runtime = SwarmMutationRuntime::default();
    let provider: Arc<dyn Provider> = Arc::new(TestProvider);
    let global_session_id = Arc::new(RwLock::new(String::new()));
    let mcp_pool = Arc::new(crate::mcp::SharedMcpPool::from_default_config());

    // The handler takes `&Arc<..>` everywhere, which cannot cross a `tokio::spawn`
    // boundary, so every dependency is cloned into the task. `client_tx` is cloned
    // per call so both replies land on the one receiver.
    let spawn_pinned = {
        let sessions = Arc::clone(&sessions);
        let soft_interrupt_queues = Arc::clone(&soft_interrupt_queues);
        let client_connections = Arc::clone(&client_connections);
        let swarm_members = Arc::clone(&swarm_members);
        let swarms_by_id = Arc::clone(&swarms_by_id);
        let swarm_plans = Arc::clone(&swarm_plans);
        let swarm_coordinators = Arc::clone(&swarm_coordinators);
        let event_history = Arc::clone(&event_history);
        let event_counter = Arc::clone(&event_counter);
        let swarm_event_tx = swarm_event_tx.clone();
        let global_session_id = Arc::clone(&global_session_id);
        let provider = Arc::clone(&provider);
        let mcp_pool = Arc::clone(&mcp_pool);
        let mutation_runtime = mutation_runtime.clone();
        move |id: u64| {
            let sessions = Arc::clone(&sessions);
            let soft_interrupt_queues = Arc::clone(&soft_interrupt_queues);
            let client_connections = Arc::clone(&client_connections);
            let swarm_members = Arc::clone(&swarm_members);
            let swarms_by_id = Arc::clone(&swarms_by_id);
            let swarm_plans = Arc::clone(&swarm_plans);
            let swarm_coordinators = Arc::clone(&swarm_coordinators);
            let event_history = Arc::clone(&event_history);
            let event_counter = Arc::clone(&event_counter);
            let swarm_event_tx = swarm_event_tx.clone();
            let global_session_id = Arc::clone(&global_session_id);
            let provider = Arc::clone(&provider);
            let mcp_pool = Arc::clone(&mcp_pool);
            let mutation_runtime = mutation_runtime.clone();
            let client_tx = client_tx.clone();
            let requester = requester.to_string();
            let pinned_worker = pinned_worker.to_string();
            tokio::spawn(async move {
                handle_comm_assign_next(
                    id,
                    requester,
                    Some(pinned_worker),
                    None,
                    None,
                    None,
                    None,
                    None,
                    None,
                    &client_tx,
                    &sessions,
                    &global_session_id,
                    &provider,
                    &soft_interrupt_queues,
                    &client_connections,
                    &swarm_members,
                    &swarms_by_id,
                    &swarm_plans,
                    &swarm_coordinators,
                    &event_history,
                    &event_counter,
                    &swarm_event_tx,
                    &mcp_pool,
                    &mutation_runtime,
                )
                .await;
            })
        }
    };

    // Park the FIRST call on the plan lock. Yielding is required because
    // `#[tokio::test]` uses a current-thread runtime: `tokio::spawn` queues the
    // task and it does not run until this test yields.
    let plans_guard = swarm_plans.write().await;
    let first = spawn_pinned(9201);
    assert!(
        parked_on_plan_lock(&first).await,
        "the first call finished while the plans write lock was held, so it cannot \
         have been parked on the plan lock and the coalescing this test claims to \
         exercise would silently degrade to the sequential case."
    );

    // Only now is the SECOND call genuinely concurrent: it is spawned while the
    // first is still parked, so both resolve their task id before either writes.
    let second = spawn_pinned(9202);
    assert!(
        parked_on_plan_lock(&second).await,
        "the second call finished while the plans write lock was held, so the two \
         calls never overlapped and this is not a concurrency test."
    );

    drop(plans_guard);

    first.await.expect("first pinned call panicked");
    second.await.expect("second pinned call panicked");

    // Exactly two replies, each carrying its own request id and the same node.
    let mut replies = Vec::new();
    for _ in 0..2 {
        let event = tokio::time::timeout(Duration::from_secs(5), client_rx.recv())
            .await
            .expect("timed out after 5s waiting for a pinned assign_next reply")
            .expect("channel closed without a reply");
        match event {
            ServerEvent::CommAssignTaskResponse {
                id,
                task_id,
                target_session,
            } => {
                assert_eq!(
                    target_session, pinned_worker,
                    "both pinned calls must honour the pin"
                );
                replies.push((id, task_id));
            }
            other => panic!("expected CommAssignTaskResponse, got {other:?}"),
        }
    }
    replies.sort_by_key(|(id, _)| *id);
    let (id_a, task_a) = replies[0].clone();
    let (id_b, task_b) = replies[1].clone();

    assert_eq!(id_a, 9201, "the first caller keeps its own request id");
    assert_eq!(id_b, 9202, "the second caller keeps its own request id");
    assert_eq!(
        task_a, task_b,
        "COALESCING BROKEN BY THE PINNED-PATH FIX: two concurrent pinned assign_next \
         calls were handed different task_ids ({task_a:?} and {task_b:?}), so both \
         executed. Concurrent duplicates that resolve before either writes must \
         agree on the task id and therefore on the mutation key, which is what makes \
         `begin_with_mode` coalesce them."
    );
    assert!(
        client_rx.try_recv().is_err(),
        "a third event was queued; a coalesced waiter must be answered exactly once"
    );

    let plans = swarm_plans.read().await;
    let assigned: Vec<String> = plans
        .get(swarm_id)
        .expect("plan still present")
        .items
        .iter()
        .filter(|item| item.assigned_to.as_deref() == Some(pinned_worker))
        .map(|item| item.id.clone())
        .collect();
    drop(plans);

    assert_eq!(
        assigned,
        vec![task_a],
        "exactly one node must end up assigned to the pinned worker, found {assigned:?}"
    );
}

/// True if `handle` has not run to completion, i.e. it is parked.
///
/// The plans write lock is held by the caller for the whole of the park, so a
/// handle that is still pending after these yields is blocked on a plan lock and
/// has provably neither claimed a mutation nor dispatched. Works on both sides of
/// the fix: pre-fix a pinned call first blocks on the handler's plan write lock,
/// post-fix it first blocks on the pre-resolution plan read.
async fn parked_on_plan_lock(handle: &tokio::task::JoinHandle<()>) -> bool {
    for _ in 0..400 {
        if handle.is_finished() {
            return false;
        }
        tokio::time::sleep(Duration::from_millis(5)).await;
    }
    !handle.is_finished()
}