/// EDGE CASES of the assign/replay layer that the sequential test
/// (`assign_next_pinned_replay.rs`) explicitly left unchecked.
///
/// # STATE OF THE TREE WHEN THIS WAS WRITTEN -- read this first
///
/// These tests were written against the PRE-FIX pinned path, where
/// `handle_comm_assign_next` forwarded `task_id = None` and the dedup key's
/// third component collapsed to the constant `"__next_runnable__"`
/// (comm_control.rs:1481-1494). A sibling node landed a fix in
/// `comm_control.rs` WHILE this file was being written: the pinned branch now
/// resolves the concrete task first (comm_control.rs:2062,
/// `next_unassigned_runnable_task_id`) and forwards it at :2067, so the key
/// component varies per call and the pinned path no longer replays.
///
/// Three of my original tests were written against the pre-fix behaviour and
/// were CORRECTLY RED when run against the fixed tree. That is the useful
/// result, and it is recorded as such rather than being quietly rewritten to
/// pass:
///   * `pinned_assign_next_replays_inside_the_final_state_ttl` (EDGE 2 control)
///     failed with "returned different task_ids (task-a then task-b)". The
///     pinned replay the TTL was supposed to bound NO LONGER HAPPENS.
///   * `pinned_assign_next_advances_once_the_persisted_state_ages_past_the_ttl`
///     failed with "no persisted mutation state at ...": with a concrete task
///     id in the key, the pinned path writes a DIFFERENT state file per call,
///     so the fixed-key TTL file my helper looked for no longer exists.
///   * `concurrent_pinned_assign_next_coalesces_onto_one_execution` failed for
///     the same reason: it waited on the pre-fix fixed key.
/// The versions below are the POST-FIX re-framings, and each says which
/// question it can and cannot answer any more.
///
/// # What is still genuinely open, and is tested here
///
/// The fix moved the pick EARLIER, out of `handle_comm_assign_task_with_mode`.
/// That is exactly the shape that makes the three original edges worth
/// re-asking, because the guard moved relative to the work:
///
///   * EDGE 1 -- the pick is now at :2062, BEFORE `begin_or_replay` (:1495).
///     Two concurrent pinned calls can therefore resolve the SAME task id and
///     only then contend on the mutation key. Coalescing still has to hold.
///   * EDGE 2 -- the TTL now gates a per-call key, so a caller that retries a
///     pinned assign_next within 30s of a SUCCESSFUL one on the same task no
///     longer replays, and no longer collapses. Verified both halves.
///   * EDGE 3 -- UNCHANGED by the fix and still a real race: the pick and the
///     write are not atomic with respect to each other.
///
/// # On the clock (EDGE 2)
///
/// `now_unix_ms` (durable_state.rs:7-12) reads `SystemTime::now()` and is NOT
/// injectable. The staleness predicate reads `created_at_unix_ms` from the
/// PERSISTED state file, so these tests rewrite that one field to simulate
/// elapsed time. No test sleeps 30s. What that does and does not measure is
/// stated per-assertion.

/// Duplicates `SWARM_MUTATION_DIR` (swarm_mutation_state.rs:11), which is a
/// private const. Used only to locate the on-disk state file the production
/// code reads back.
const MUTATION_DIR: &str = "jcode-swarm-mutations";

/// Shared fixture. Every dependency the handler needs, ownable so a test can
/// move it into a spawned task (the handler takes `&Arc<..>` everywhere, which
/// cannot cross a `tokio::spawn` boundary directly).
struct EdgeFixture {
    swarm_id: String,
    sessions: Arc<RwLock<HashMap<String, Arc<Mutex<Agent>>>>>,
    global_session_id: Arc<RwLock<String>>,
    provider: Arc<dyn Provider>,
    soft_interrupt_queues: crate::server::SessionInterruptQueues,
    client_connections: Arc<RwLock<HashMap<String, super::ClientConnectionInfo>>>,
    swarm_members: Arc<RwLock<HashMap<String, SwarmMember>>>,
    swarms_by_id: Arc<RwLock<HashMap<String, HashSet<String>>>>,
    swarm_plans: Arc<RwLock<HashMap<String, VersionedPlan>>>,
    swarm_coordinators: Arc<RwLock<HashMap<String, String>>>,
    event_history: Arc<RwLock<VecDeque<SwarmEvent>>>,
    event_counter: Arc<AtomicU64>,
    swarm_event_tx: broadcast::Sender<SwarmEvent>,
    mcp_pool: Arc<crate::mcp::SharedMcpPool>,
    mutation_runtime: SwarmMutationRuntime,
}

impl EdgeFixture {
    /// One coordinator (`coordinator`), `workers.len()` owned workers, and
    /// `item_ids.len()` ready unassigned plan items.
    fn new(swarm_id: &str, coordinator: &str, workers: &[&str], item_ids: &[&str], mode: &str) -> Arc<Self> {
        let participants: HashSet<String> = std::iter::once(coordinator.to_string())
            .chain(workers.iter().map(|w| (*w).to_string()))
            .collect();

        let mut members = HashMap::new();
        members.insert(
            coordinator.to_string(),
            {
                let mut m = member(coordinator, swarm_id, "ready");
                m.role = "coordinator".to_string();
                m
            },
        );
        for worker in workers {
            members.insert(
                (*worker).to_string(),
                owned_member(worker, swarm_id, "ready", coordinator),
            );
        }

        Arc::new(Self {
            swarm_id: swarm_id.to_string(),
            sessions: Arc::new(RwLock::new(HashMap::new())),
            global_session_id: Arc::new(RwLock::new(String::new())),
            provider: Arc::new(TestProvider),
            soft_interrupt_queues: Arc::new(RwLock::new(HashMap::new())),
            client_connections: Arc::new(RwLock::new(HashMap::new())),
            swarm_members: Arc::new(RwLock::new(members)),
            swarms_by_id: Arc::new(RwLock::new(HashMap::from([(
                swarm_id.to_string(),
                participants.clone(),
            )]))),
            swarm_plans: Arc::new(RwLock::new(HashMap::from([(
                swarm_id.to_string(),
                VersionedPlan {
                    items: item_ids
                        .iter()
                        .map(|id| plan_item(id, "queued", "high", &[]))
                        .collect(),
                    version: 1,
                    participants,
                    task_progress: HashMap::new(),
                    mode: mode.to_string(),
                    node_meta: HashMap::new(),
                },
            )]))),
            swarm_coordinators: Arc::new(RwLock::new(HashMap::from([(
                swarm_id.to_string(),
                coordinator.to_string(),
            )]))),
            event_history: Arc::new(RwLock::new(VecDeque::new())),
            event_counter: Arc::new(AtomicU64::new(1)),
            swarm_event_tx: broadcast::channel(32).0,
            mcp_pool: Arc::new(crate::mcp::SharedMcpPool::from_default_config()),
            mutation_runtime: SwarmMutationRuntime::default(),
        })
    }

    /// Spawn a pinned `assign_next` as an independent task, so two of them can
    /// genuinely overlap. Cloning every dependency into the task is what lets
    /// the handler's `&Arc<..>` parameters satisfy `'static`.
    fn spawn_pinned(
        self: &Arc<Self>,
        id: u64,
        requester: &str,
        target: &str,
        client_event_tx: mpsc::UnboundedSender<ServerEvent>,
    ) -> tokio::task::JoinHandle<()> {
        let fx = Arc::clone(self);
        let requester = requester.to_string();
        let target = target.to_string();
        tokio::spawn(async move {
            handle_comm_assign_next(
                id,
                requester,
                Some(target),
                None,
                None,
                None,
                None,
                None,
                None,
                &client_event_tx,
                &fx.sessions,
                &fx.global_session_id,
                &fx.provider,
                &fx.soft_interrupt_queues,
                &fx.client_connections,
                &fx.swarm_members,
                &fx.swarms_by_id,
                &fx.swarm_plans,
                &fx.swarm_coordinators,
                &fx.event_history,
                &fx.event_counter,
                &fx.swarm_event_tx,
                &fx.mcp_pool,
                &fx.mutation_runtime,
            )
            .await;
        })
    }

    /// Spawn an explicit `assign_task` naming `task_id` directly. Used to hold
    /// the mutation key CONSTANT across calls, which the pinned path no longer
    /// does now that it resolves the task first (comm_control.rs:2062-2067).
    /// This is how the in-flight coalescing path is still reachable at all.
    fn spawn_explicit_task(
        self: &Arc<Self>,
        id: u64,
        requester: &str,
        target: &str,
        task_id: &str,
        client_event_tx: mpsc::UnboundedSender<ServerEvent>,
    ) -> tokio::task::JoinHandle<()> {
        let fx = Arc::clone(self);
        let requester = requester.to_string();
        let target = target.to_string();
        let task_id = task_id.to_string();
        tokio::spawn(async move {
            handle_comm_assign_task(
                id,
                requester,
                Some(target),
                Some(task_id),
                None,
                &client_event_tx,
                &fx.sessions,
                &fx.soft_interrupt_queues,
                &fx.client_connections,
                &fx.swarm_members,
                &fx.swarms_by_id,
                &fx.swarm_plans,
                &fx.swarm_coordinators,
                &fx.event_history,
                &fx.event_counter,
                &fx.swarm_event_tx,
                &fx.mutation_runtime,
            )
            .await;
        })
    }

    /// The mutation key for an EXPLICIT `assign_task` naming `task_id`
    /// (comm_control.rs:1481-1494): `[swarm_id, target, task_id, message]`.
    /// Stable across calls, which is what makes the in-flight path testable.
    fn explicit_key(&self, requester: &str, target: &str, task_id: &str) -> String {
        super::swarm_mutation_request_key(
            requester,
            "assign_task",
            &[
                self.swarm_id.clone(),
                target.to_string(),
                task_id.to_string(),
                String::new(),
            ],
        )
    }

    fn state_path(&self, key: &str) -> std::path::PathBuf {
        super::super::durable_state::state_dir(MUTATION_DIR).join(format!("{key}.json"))
    }

    /// Wait until the persisted mutation state for `key` exists, which is the
    /// observable proof that `begin_or_replay` claimed the key and wrote its
    /// pending state. Bounded, so a failure is a clear assert rather than a hang.
    async fn await_claimed(&self, key: &str, context: &str) {
        let path = self.state_path(key);
        for _ in 0..400 {
            if path.exists() {
                return;
            }
            tokio::time::sleep(Duration::from_millis(5)).await;
        }
        panic!(
            "{context}: no pending mutation state at {path:?} after 2s. The call never \
             reached `begin_with_mode`, so it cannot be proven in flight."
        );
    }

    /// Plan items currently assigned to `session`, in plan order.
    async fn assigned_to(&self, session: &str) -> Vec<String> {
        let plans = self.swarm_plans.read().await;
        plans
            .get(&self.swarm_id)
            .expect("plan still present")
            .items
            .iter()
            .filter(|item| item.assigned_to.as_deref() == Some(session))
            .map(|item| item.id.clone())
            .collect()
    }
}

/// Register `session` as a second driver in the fixture's swarm: a member with
/// `role = "coordinator"`, present in `swarms_by_id`, and present in
/// `plan.participants` (the last is what deep mode's driver check reads).
async fn add_second_driver(fx: &Arc<EdgeFixture>, session: &str) {
    {
        let mut members = fx.swarm_members.write().await;
        members.insert(
            session.to_string(),
            {
                let mut m = member(session, &fx.swarm_id, "ready");
                m.role = "coordinator".to_string();
                m
            },
        );
        let mut swarms = fx.swarms_by_id.write().await;
        swarms
            .get_mut(&fx.swarm_id)
            .expect("swarm")
            .insert(session.to_string());
    }
    let mut plans = fx.swarm_plans.write().await;
    plans
        .get_mut(&fx.swarm_id)
        .expect("plan")
        .participants
        .insert(session.to_string());
}

async fn assigned_event(
    rx: &mut mpsc::UnboundedReceiver<ServerEvent>,
    context: &str,
) -> (u64, String, String) {
    match tokio::time::timeout(Duration::from_secs(5), rx.recv())
        .await
        .unwrap_or_else(|_| panic!("{context}: timed out after 5s waiting for a response"))
    {
        Some(ServerEvent::CommAssignTaskResponse {
            id,
            task_id,
            target_session,
        }) => (id, task_id, target_session),
        Some(other) => panic!("{context}: expected CommAssignTaskResponse, got {other:?}"),
        None => panic!("{context}: channel closed without a response"),
    }
}

/// EDGE 1, positive case: two CONCURRENT requests that resolve to the SAME
/// mutation key must COALESCE onto one execution.
///
/// IMPORTANT SCOPE CHANGE caused by the sibling fix. The pinned `assign_next`
/// path now resolves its task before claiming (comm_control.rs:2062), so two
/// concurrent pinned calls get DIFFERENT keys and no longer contend. This test
/// therefore uses EXPLICIT `assign_task` naming the same task, which is the
/// shape that still produces a constant key -- and it is the only shape where
/// "two concurrent calls, one execution" is the correct expectation at all.
/// The pinned path's own behaviour post-fix is covered separately by
/// `concurrent_pinned_assign_next_advances_two_distinct_nodes`.
///
/// Concurrency is forced deterministically rather than hoped for: the plan
/// write lock is held, so the first call parks at comm_control.rs:1549 -- AFTER
/// `begin_or_replay` has claimed the key and written the pending state. The
/// presence of that file is the observable proving the claim, so the second
/// call is only spawned once the first is provably in flight. Without the lock
/// holder, `#[tokio::test]`'s current-thread runtime would most likely run call
/// A to completion before call B is ever polled, silently re-testing the
/// SEQUENTIAL path.
///
/// Expected (swarm_mutation_state.rs:225-234 + :264-276): call B finds the key
/// active, registers as a waiter, returns `None`, and the handler exits at
/// comm_control.rs:1519-1521 without sending. Both callers are answered by
/// `finish_request`'s drain, with their OWN request ids and the SAME task_id.
/// The plan must end with exactly ONE assigned node.
#[tokio::test]
async fn concurrent_same_key_assign_task_coalesces_onto_one_execution() {
    let (_env, _runtime) = RuntimeEnvGuard::new();
    let fx = EdgeFixture::new(
        "swarm-edge1-coalesce",
        "coord",
        &["worker-pinned"],
        &["task-a", "task-b", "task-c"],
        "light",
    );
    let (tx, mut rx) = mpsc::unbounded_channel();
    let key = fx.explicit_key("coord", "worker-pinned", "task-a");

    // Park the first call inside the handler, past its mutation claim.
    let plans_guard = fx.swarm_plans.write().await;
    let first = fx.spawn_explicit_task(9001, "coord", "worker-pinned", "task-a", tx.clone());

    fx.await_claimed(&key, "first call").await;
    assert!(
        !first.is_finished(),
        "first call finished before the plan lock was released, so it cannot still \
         hold the mutation claim; the concurrency this test claims to exercise \
         would silently degrade to the sequential case."
    );

    // Only now is the second call genuinely concurrent with the first.
    let second = fx.spawn_explicit_task(9002, "coord", "worker-pinned", "task-a", tx.clone());
    drop(plans_guard);

    first.await.expect("first call task panicked");
    second.await.expect("second call task panicked");

    let (id_a, task_a, target_a) = assigned_event(&mut rx, "first caller").await;
    let (id_b, task_b, target_b) = assigned_event(&mut rx, "second caller").await;

    assert_eq!(id_a, 9001, "first caller must keep its own request id");
    assert_eq!(id_b, 9002, "second caller must keep its own request id");
    assert_eq!(target_a, "worker-pinned");
    assert_eq!(target_b, "worker-pinned");
    assert_eq!(
        task_a, "task-a",
        "the executor must assign the requested task, got {task_a:?}"
    );
    assert_eq!(
        task_b, task_a,
        "the two concurrent callers did NOT coalesce: they were handed different \
         task_ids ({task_a:?} and {task_b:?}). A second execution means the active \
         claim in `begin_with_mode` (swarm_mutation_state.rs:232-234) failed to \
         exclude the duplicate."
    );

    // Nothing extra: the coalesced second caller must not also produce an Error
    // or a Done. A third event would mean the waiter was answered twice.
    assert!(
        rx.try_recv().is_err(),
        "a third event was queued; the waiter was answered more than once"
    );

    let assigned = fx.assigned_to("worker-pinned").await;
    assert_eq!(
        assigned,
        vec!["task-a"],
        "exactly one node must end up assigned to the pinned worker, found {assigned:?}"
    );
}

/// EDGE 1, post-fix counterpart: two concurrent PINNED `assign_next` calls to
/// the same worker. Post-fix these are expected NOT to coalesce, because the
/// pinned branch resolves the task before building the key
/// (comm_control.rs:2062-2067) and so produces two distinct keys.
///
/// This is the sibling's fix under test, and it is also the check that the fix
/// did not introduce a double-assign while moving the pick earlier: both calls
/// resolve task-a only if they read the plan before either writes, and if they
/// do contend on the same key the mutation layer must still collapse them.
#[tokio::test]
async fn concurrent_pinned_assign_next_advances_two_distinct_nodes() {
    let (_env, _runtime) = RuntimeEnvGuard::new();
    let fx = EdgeFixture::new(
        "swarm-edge1-pinned-postfix",
        "coord",
        &["worker-pinned"],
        &["task-a", "task-b", "task-c"],
        "light",
    );
    let (tx, mut rx) = mpsc::unbounded_channel();

    let first = fx.spawn_pinned(9151, "coord", "worker-pinned", tx.clone());
    let second = fx.spawn_pinned(9152, "coord", "worker-pinned", tx.clone());
    first.await.expect("first call panicked");
    second.await.expect("second call panicked");

    let (id_a, task_a, _) = assigned_event(&mut rx, "first caller").await;
    let (id_b, task_b, _) = assigned_event(&mut rx, "second caller").await;
    assert_eq!((id_a, id_b), (9151, 9152), "each caller keeps its own id");

    assert_ne!(
        task_a, task_b,
        "CONFIRMED the sibling fix regressed or is absent: two concurrent pinned \
         assign_next calls were both handed {task_a:?}. Post-fix the pinned path \
         resolves the task before building the key (comm_control.rs:2062), so the \
         frontier must advance."
    );

    let assigned = fx.assigned_to("worker-pinned").await;
    assert_eq!(
        assigned.len(),
        2,
        "two concurrent pinned calls must advance the frontier by two distinct \
         nodes, but {assigned:?} are assigned"
    );
}

/// EDGE 1, negative case: coalescing must never leave a caller with NO
/// response.
///
/// The mechanism that would produce a permanent hang is a claimed mutation
/// whose executor never reaches `finish_request`. If that happens mid-flight,
///
/// 1. `active_keys` never loses the key (`swarm_mutation_state.rs:267` is the
///    only remover, and it runs inside `finish_request`), so the key stays
///    claimed for the life of the process, and
/// 2. every later duplicate registers as a waiter and returns `None`, so the
///    handler exits silently at comm_control.rs:1519-1521 and its caller waits
///    forever. That is strictly worse than a duplicate: the duplicate costs a
///    repeated assignment, this costs a permanently silent coordinator.
///
/// This test reproduces (1)+(2) with `JoinHandle::abort`, which drops the
/// future at its await point. It uses an explicit `assign_task` because only
/// that shape still yields a constant key post-fix.
///
/// REACHABILITY -- stated honestly, and it is WEAKER than the mechanism:
/// this proves the MECHANISM is real, not that production can trigger it.
/// Every ordinary path out of `handle_comm_assign_task_with_mode` after the
/// claim (comm_control.rs:1519-1845) calls `finish_swarm_mutation_request`
/// first -- the four early returns are at 1520, 1543, 1675, 1690 -- and there
/// is no `panic!`/`unwrap`/`expect` in that span. The per-connection client
/// loop awaits each handler INLINE (client_lifecycle.rs:805-1012), so a client
/// disconnect does NOT drop an in-flight handler. The cancellation token in
/// `run_client_stream`'s select! (runtime.rs:376-379) comes from
/// `RuntimeTaskScope` (runtime.rs:40-59), cancelled only by
/// `ServerRuntime::shutdown` at teardown, and `active_keys` is in-memory so it
/// dies with the process anyway. Remaining candidates are an unproven panic in
/// the post-claim side-effect sequence (comm_control.rs:1699-1832) and any
/// future early return added between claim and finish.
///
/// NOTE ON STATUS: this test PASSES when the wedge is present. That is
/// intentional. The bug is the ABSENCE of a response, which cannot be asserted
/// red without hanging the suite forever, so the negative space is asserted
/// directly (the timeout fires) and the verdict lives in the artifact.
#[tokio::test]
async fn dropped_executor_wedges_the_key_and_starves_every_later_caller() {
    let (_env, _runtime) = RuntimeEnvGuard::new();
    let fx = EdgeFixture::new(
        "swarm-edge1-wedge",
        "coord",
        &["worker-pinned"],
        &["task-a", "task-b", "task-c"],
        "light",
    );
    let (tx, mut rx) = mpsc::unbounded_channel();
    let key = fx.explicit_key("coord", "worker-pinned", "task-a");

    let plans_guard = fx.swarm_plans.write().await;
    let doomed = fx.spawn_explicit_task(9101, "coord", "worker-pinned", "task-a", tx.clone());

    fx.await_claimed(&key, "doomed call").await;
    assert!(!doomed.is_finished(), "doomed call must still be in flight");

    // Exactly what a cancelled client connection would do: drop the handler
    // future at its await point. `finish_request` never runs.
    doomed.abort();
    let _ = doomed.await;
    drop(plans_guard);

    // A later caller with the same key must be starved. The timeout firing IS
    // the finding, so a fired timeout is success and a received event is the
    // failure.
    let later = fx.spawn_explicit_task(9102, "coord", "worker-pinned", "task-a", tx.clone());
    later.await.expect("later call task panicked");

    match tokio::time::timeout(Duration::from_millis(500), rx.recv()).await {
        // HANG REPRODUCED. Nothing to assert: the absence of a response is the
        // defect, and this test passes precisely because it reproduced.
        Err(_elapsed) => {}
        Ok(Some(event)) => panic!(
            "HANG NOT REPRODUCED: the later caller received {event:?}. Either the \
             active claim is now released outside `finish_request`, or the pending \
             state is already stale enough to replay. Re-verify before treating \
             the wedge as fixed."
        ),
        Ok(None) => panic!("channel closed without a response"),
    }
}

/// NEGATIVE CONTROL. This test's whole job is to prove the coalescing test
/// above discriminates. It runs the SAME two concurrent same-key calls and
/// asserts they did NOT coalesce, by giving the two calls keys that differ in
/// the one component the dedup key actually hashes.
///
/// If this test cannot be made to fail while the real coalescing test passes,
/// then the coalescing test proves nothing. It fails by construction whenever
/// the key really is load-bearing: differing only in `task_id` yields two
/// different mutation keys, so `begin_with_mode` does not dedup, and both
/// callers execute independently.
///
/// Without this, `concurrent_same_key_assign_task_coalesces_onto_one_execution`
/// could be passing merely because the two calls are sequential on a
/// current-thread runtime -- in which case it would be re-testing the path the
/// sequential suite already covers, and EDGE 1 would still be untested.
#[tokio::test]
async fn concurrent_distinct_key_assign_task_does_not_coalesce() {
    let (_env, _runtime) = RuntimeEnvGuard::new();
    let fx = EdgeFixture::new(
        "swarm-edge1-negative-control",
        "coord",
        &["worker-pinned"],
        &["task-a", "task-b", "task-c"],
        "light",
    );
    let (tx, mut rx) = mpsc::unbounded_channel();

    // Park the first call past its claim, exactly as the coalescing test does,
    // so the only difference between the two tests is the key.
    let plans_guard = fx.swarm_plans.write().await;
    let first = fx.spawn_explicit_task(9201, "coord", "worker-pinned", "task-a", tx.clone());
    fx.await_claimed(
        &fx.explicit_key("coord", "worker-pinned", "task-a"),
        "first call",
    )
    .await;
    assert!(!first.is_finished(), "first call must still be in flight");

    let second = fx.spawn_explicit_task(9202, "coord", "worker-pinned", "task-b", tx.clone());
    drop(plans_guard);
    first.await.expect("first call task panicked");
    second.await.expect("second call task panicked");

    let (_, task_a, _) = assigned_event(&mut rx, "first caller").await;
    let (_, task_b, _) = assigned_event(&mut rx, "second caller").await;

    assert_ne!(
        task_a, task_b,
        "NEGATIVE CONTROL FAILED: calls with DIFFERENT mutation keys were collapsed \
         onto {task_a:?}. If distinct keys coalesce, the dedup key is not actually \
         load-bearing and the coalescing test proves nothing."
    );
    assert_eq!(
        fx.assigned_to("worker-pinned").await,
        vec![task_a, task_b],
        "distinct keys must produce two independent assignments"
    );
}

/// EDGE 2, post-fix control: does the pinned path still replay inside the TTL?
///
/// Run against the FIXED tree this expects the frontier to ADVANCE. That is
/// the point: it is the executable statement that the sibling's fix removed the
/// pinned replay. It was written as the opposite assertion (expecting a replay)
/// and failed with `("task-a", "task-b")`, which is how the fix was detected.
#[tokio::test]
async fn pinned_assign_next_does_not_replay_inside_the_final_state_ttl() {
    let (_env, _runtime) = RuntimeEnvGuard::new();
    let fx = EdgeFixture::new(
        "swarm-edge2-inside",
        "coord",
        &["worker-pinned"],
        &["task-a", "task-b", "task-c"],
        "light",
    );
    let (tx, mut rx) = mpsc::unbounded_channel();

    fx.spawn_pinned(9201, "coord", "worker-pinned", tx.clone())
        .await
        .expect("call 1 panicked");
    fx.spawn_pinned(9202, "coord", "worker-pinned", tx.clone())
        .await
        .expect("call 2 panicked");

    let (_, first, _) = assigned_event(&mut rx, "call 1").await;
    let (_, second, _) = assigned_event(&mut rx, "call 2").await;

    assert_ne!(
        first, second,
        "REGRESSION: two sequential pinned assign_next calls inside \
         FINAL_STATE_TTL returned the same task_id ({first:?}). The pinned replay \
         bug is back -- most likely comm_control.rs:2062 stopped resolving the \
         task before building the key."
    );
    assert_eq!(
        fx.assigned_to("worker-pinned").await,
        vec![first, second],
        "inside the TTL the pinned frontier must still advance, one node per call"
    );
}

/// EDGE 2, the TTL boundary itself, on the path where a key IS stable.
///
/// The pinned path no longer yields a constant key, so the TTL is exercised
/// with an EXPLICIT `assign_task` naming the same task twice. That is the same
/// `is_stale` / `FINAL_STATE_TTL` machinery (swarm_mutation_state.rs:12,
/// :99-105 -> durable_state.rs:73-75), reached by a shape whose key the fix
/// did not change.
///
/// The clock is controlled by rewriting `created_at_unix_ms` in the persisted
/// state file, which is the exact value `is_stale` compares against. Nothing
/// sleeps.
///
/// What this measures: that once the recorded timestamp is older than the TTL,
/// `load_state` treats the file as stale and DELETES it (durable_state.rs:56-59),
/// `begin_or_replay` then starts a fresh attempt, and the caller is told the
/// real state of the world instead of a cached one.
///
/// What this does NOT measure: real wall-clock passage, and the specific
/// caller-visible consequence of expiry (which for a re-assign of an actively
/// worked task is the double-assignment refusal, asserted here).
#[tokio::test]
async fn explicit_assign_task_stops_replaying_once_the_persisted_state_ages_past_the_ttl() {
    let (_env, _runtime) = RuntimeEnvGuard::new();
    let fx = EdgeFixture::new(
        "swarm-edge2-expired",
        "coord",
        &["worker-pinned"],
        &["task-a", "task-b", "task-c"],
        "light",
    );
    let (tx, mut rx) = mpsc::unbounded_channel();

    // Call 1 assigns task-a for real.
    fx.spawn_explicit_task(9301, "coord", "worker-pinned", "task-a", tx.clone())
        .await
        .expect("call 1 panicked");
    let (_, first, _) = assigned_event(&mut rx, "call 1").await;
    assert_eq!(first, "task-a");

    // Call 2, same key, inside the TTL: must REPLAY, silently re-reporting
    // task-a without doing anything. This is the idempotency ReplayFinal buys.
    fx.spawn_explicit_task(9302, "coord", "worker-pinned", "task-a", tx.clone())
        .await
        .expect("call 2 panicked");
    let (_, replayed, _) = assigned_event(&mut rx, "call 2").await;
    assert_eq!(
        replayed, first,
        "inside the TTL the second call must replay the persisted response \
         (idempotency), not re-execute"
    );

    let key = fx.explicit_key("coord", "worker-pinned", "task-a");
    let path = fx.state_path(&key);
    assert!(path.exists(), "no persisted mutation state at {path:?}");

    // Age the persisted state past FINAL_STATE_TTL (30s) by rewriting the one
    // field the staleness predicate reads.
    backdate_state(&path, 31_000);

    // Call 3, same key, after expiry: must NOT replay. task-a is already
    // assigned and actively worked, so the double-assignment guard
    // (comm_control.rs:1564-1582) is what the caller should now see. That is
    // the concrete answer to "what does a caller see in the gap": inside the
    // TTL a misleading success, after it the truth.
    fx.spawn_explicit_task(9303, "coord", "worker-pinned", "task-a", tx.clone())
        .await
        .expect("call 3 panicked");

    match tokio::time::timeout(Duration::from_secs(5), rx.recv())
        .await
        .expect("timed out waiting for call 3's response")
    {
        Some(ServerEvent::Error { message, .. }) => assert!(
            message.contains("already assigned to 'worker-pinned'"),
            "after TTL expiry the caller must be told task-a is already assigned, \
             got: {message}"
        ),
        Some(other) => panic!(
            "CONFIRMED the TTL does not expire for this key: call 3 after ageing \
             the persisted state past FINAL_STATE_TTL returned {other:?} instead \
             of re-executing."
        ),
        None => panic!("channel closed without a response"),
    }

    // The stale file must have been replaced, not merely ignored, so the NEXT
    // call starts clean instead of re-ageing the same record.
    let refreshed: serde_json::Value =
        serde_json::from_str(&std::fs::read_to_string(&path).expect("state file rewritten"))
            .expect("state file is valid json");
    let refreshed_at = refreshed["created_at_unix_ms"]
        .as_u64()
        .expect("created_at_unix_ms present");
    assert!(
        refreshed_at > super::super::swarm::now_unix_ms().saturating_sub(5_000),
        "the aged state was not replaced with a fresh one (created_at_unix_ms \
         stayed at {refreshed_at}); a stale record left on disk would be aged \
         again on every subsequent call"
    );
}

/// EDGE 2, boundary: which side of 30s is stale, and by how much.
///
/// `elapsed_exceeds` (durable_state.rs:73-75) uses a STRICT `>`, so a record
/// exactly `FINAL_STATE_TTL` old is still fresh and one millisecond more makes
/// it stale. `now_unix_ms` cannot be frozen, so the two deterministic sides
/// are pinned here -- elapsed ~0 (fresh) and elapsed >= 30_001ms (stale) -- and
/// the exact 30_000ms millisecond is reported as un-pinnable rather than
/// asserted with a flaky bound.
#[tokio::test]
async fn final_state_staleness_boundary_is_one_millisecond_past_the_ttl() {
    let (_env, _runtime) = RuntimeEnvGuard::new();
    use super::super::swarm_mutation_state as sms;

    let key = "swarm-edge2-boundary".to_string();
    let path = super::super::durable_state::state_dir(MUTATION_DIR).join(format!("{key}.json"));

    // Fresh: created "now", so elapsed is ~0 and the record must survive.
    sms::save_state(&sms::PersistedSwarmMutationState {
        key: key.clone(),
        action: "assign_task".to_string(),
        session_id: "coord".to_string(),
        created_at_unix_ms: super::super::swarm::now_unix_ms(),
        final_response: Some(sms::PersistedSwarmMutationResponse::AssignTask {
            task_id: "task-a".to_string(),
            target_session: "worker-pinned".to_string(),
        }),
    });
    assert!(
        sms::load_state(&key).is_some(),
        "a just-created final state must not be stale"
    );

    // Just past the TTL: elapsed >= 30_001ms, so the strict `>` must trip and
    // `load_state` must delete the record.
    sms::save_state(&sms::PersistedSwarmMutationState {
        key: key.clone(),
        action: "assign_task".to_string(),
        session_id: "coord".to_string(),
        created_at_unix_ms: super::super::swarm::now_unix_ms().saturating_sub(30_001),
        final_response: Some(sms::PersistedSwarmMutationResponse::AssignTask {
            task_id: "task-a".to_string(),
            target_session: "worker-pinned".to_string(),
        }),
    });
    assert!(
        sms::load_state(&key).is_none(),
        "a final state 30_001ms old must be stale"
    );
    assert!(
        !path.exists(),
        "`load_json_state` must delete the stale record, not just skip it"
    );

    // UNPinnable, recorded rather than asserted: exactly 30_000ms old. Whether
    // that reads as stale depends on how many milliseconds elapse between the
    // `now_unix_ms()` call above and the one inside `elapsed_exceeds`, so any
    // assertion here would be a coin flip rather than a result.
}

/// EDGE 3, guard: in light mode the second "coordinator" cannot drive at all.
///
/// `swarm_coordinators` maps a swarm id to exactly ONE coordinator session, so
/// light mode structurally has one driver. This pins that, because it is what
/// makes EDGE 3 a deep-mode question rather than a light-mode one.
#[tokio::test]
async fn light_mode_refuses_a_second_driver_before_touching_the_plan() {
    let (_env, _runtime) = RuntimeEnvGuard::new();
    let fx = EdgeFixture::new(
        "swarm-edge3-light",
        "coord-a",
        &["worker-a", "worker-b"],
        &["task-a", "task-b", "task-c"],
        "light",
    );
    // A second session in the same swarm, claiming to be a coordinator. In light
    // mode it is NOT a plan participant, so the deep-mode branch of
    // `require_plan_driver_swarm` cannot rescue it.
    add_second_driver(&fx, "coord-b").await;
    let (tx, mut rx) = mpsc::unbounded_channel();

    fx.spawn_pinned(9401, "coord-b", "worker-b", tx.clone())
        .await
        .expect("call panicked");

    match tokio::time::timeout(Duration::from_secs(5), rx.recv())
        .await
        .expect("timed out waiting for the permission error")
    {
        Some(ServerEvent::Error { message, .. }) => assert_eq!(
            message, "Only the coordinator can assign tasks.",
            "light mode must refuse a non-coordinator driver with the documented message"
        ),
        Some(other) => panic!("expected the permission Error, got {other:?}"),
        None => panic!("channel closed without a response"),
    }
    assert_eq!(
        fx.assigned_to("worker-b").await,
        Vec::<String>::new(),
        "the refused driver must not have assigned anything"
    );
}

/// EDGE 3: in DEEP mode two sessions may both drive the same plan. Does
/// anything stop them landing on the same `task_id`?
///
/// This is NOT the replay defect class. The two calls compute different
/// mutation keys (the key hashes the requesting session AND the pinned target,
/// comm_control.rs:1481-1494), so the dedup layer cannot help and does not
/// engage. The only thing standing between them is the plan write lock, which
/// spans both the pick (`next_unassigned_runnable_item_id`,
/// comm_control.rs:1553-1555) and the write (:1620-1637). If pick and write
/// were ever split, both callers would pick the same first unassigned node.
///
/// THE SIBLING FIX MAKES THIS SHARPER, NOT SAFER. Post-fix the pinned path
/// picks OUTSIDE that lock (comm_control.rs:2062, a bare `plans.read()`), so
/// two concurrent pinned calls can now both read `task-a` before either writes
/// it. This test exercises exactly that: both are released onto the plan
/// together, so the window is as wide as the fixture can make it. Correct
/// behaviour is still distinct tasks, because `handle_comm_assign_task_with_mode`
/// re-reads and writes under the write lock and the second caller then finds
/// task-a assigned.
#[tokio::test]
async fn deep_mode_two_drivers_cannot_both_pick_the_same_task() {
    let (_env, _runtime) = RuntimeEnvGuard::new();
    let fx = EdgeFixture::new(
        "swarm-edge3-deep",
        "coord-a",
        &["worker-a", "worker-b"],
        &["task-a", "task-b", "task-c"],
        "deep",
    );
    // Second driver: deep mode lets any plan participant drive
    // (comm_control.rs:2650-2663), so this session is admitted -- but ONLY
    // because it is in `plan.participants`. Registering it in the members map
    // alone would leave it refused, which is what the light-mode sibling proves.
    add_second_driver(&fx, "coord-b").await;
    let (tx, mut rx) = mpsc::unbounded_channel();

    let first = fx.spawn_pinned(9501, "coord-a", "worker-a", tx.clone());
    let second = fx.spawn_pinned(9502, "coord-b", "worker-b", tx.clone());
    first.await.expect("coord-a call panicked");
    second.await.expect("coord-b call panicked");

    let mut landed: Vec<(u64, String, String)> = vec![
        assigned_event(&mut rx, "coord-a").await,
        assigned_event(&mut rx, "coord-b").await,
    ];
    landed.sort_by_key(|(id, _, _)| *id);
    let ((id_a, task_a, target_a), (id_b, task_b, target_b)) =
        (landed[0].clone(), landed[1].clone());

    assert_eq!((id_a, id_b), (9501, 9502), "each caller keeps its own id");
    assert_eq!(
        (target_a.as_str(), target_b.as_str()),
        ("worker-a", "worker-b"),
        "each call honoured its pin"
    );

    assert_ne!(
        task_a, task_b,
        "CONFIRMED multi-coordinator double-pick: two deep-mode drivers were both \
         handed {task_a:?}. The pick now happens outside the plan write lock \
         (comm_control.rs:2062), so the pick and the write are not atomic."
    );

    assert_eq!(
        fx.assigned_to("worker-a").await,
        vec![task_a.clone()],
        "worker-a must hold exactly one assignment"
    );
    assert_eq!(
        fx.assigned_to("worker-b").await,
        vec![task_b.clone()],
        "worker-b must hold exactly one assignment"
    );
}

/// Rewrite `created_at_unix_ms` in a persisted mutation state file, leaving
/// every other field untouched. This is the controllable-clock stand-in for
/// `FINAL_STATE_TTL`: `is_stale` reads this field and nothing else
/// (swarm_mutation_state.rs:99-105).
fn backdate_state(path: &std::path::Path, by_ms: u64) {
    let raw = std::fs::read_to_string(path).expect("read mutation state");
    let mut value: serde_json::Value =
        serde_json::from_str(&raw).expect("mutation state is valid json");
    let created = value["created_at_unix_ms"]
        .as_u64()
        .expect("created_at_unix_ms present");
    value["created_at_unix_ms"] = serde_json::json!(created.saturating_sub(by_ms));
    std::fs::write(path, serde_json::to_string(&value).expect("serialize mutation state"))
        .expect("write mutation state");
}