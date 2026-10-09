/// Client-side guards for `run_swarm_plan_loop`'s dispatch loop.
///
/// # What the server does, verified in this repo
///
/// `handle_comm_assign_next` folds the pinned target and (on the unpinned path)
/// the server's own node pick into the swarm-mutation dedup key
/// (`comm_control.rs:1481-1494`). With a pinned `target_session` that key is
/// byte-identical on every iteration of the loop, so every call after the first
/// is answered inside the 30s `FINAL_STATE_TTL` by re-emitting the stored
/// `CommAssignTaskResponse` under a fresh request id
/// (`swarm_mutation_state.rs:36-43`, `:219-224`). The sibling server-side
/// control test `pinned_assign_next_replays_inside_the_final_state_ttl`
/// (`comm_control_tests/assign_next_replay_edges.rs`) pins that two sequential
/// pinned `assign_next` calls inside the TTL return the SAME `task_id`.
/// The response is therefore shape-identical to a real dispatch, which is why
/// the client cannot tell them apart except by remembering what it already
/// accepted. That memory is the guard under test here.

/// The replay itself: an identical `(task, worker)` response must be reported
/// as a repeat, because that is the only way a single dispatch can otherwise be
/// counted once per open slot (upstream issue #1209's "8 identical
/// assignments").
#[test]
fn run_plan_dispatch_repeats_flags_an_identical_task_and_worker_pair() {
    let accepted = vec![("catalog-evaluate".to_string(), "session_kangaroo".to_string())];

    assert!(
        super::run_plan_dispatch_repeats(
            &accepted,
            "catalog-evaluate",
            "session_kangaroo"
        ),
        "the server replayed the stored response verbatim, which is exactly the \
         shape of a real dispatch; the loop must not count it twice"
    );
}

/// The stranded-node reclaim, which `gap-stranded-reclaim-underfill` flagged as
/// the reason NOT to copy `fill_slots`' unconditional task-id guard here.
///
/// `next_runnable_task_id_reclaiming_stranded` re-dispatches a node stranded
/// on a dead assignee, and the re-dispatch lands on a DIFFERENT worker: auto-pick
/// only considers ready, non-busy members (`filter_swarm_agent_candidates`),
/// and a dead assignee is failed/stopped/crashed or no longer a member. So the
/// same task id arriving with a new worker is new progress and must not stop
/// the wave. A task-id-only guard would break here and under-fill.
#[test]
fn run_plan_dispatch_repeats_exempts_a_stranded_reclaim_to_a_different_worker() {
    let accepted = vec![("catalog-evaluate".to_string(), "session_corpse".to_string())];

    assert!(
        !super::run_plan_dispatch_repeats(&accepted, "catalog-evaluate", "session_wallace"),
        "re-dispatching a reclaimed node to a DIFFERENT worker is real progress; \
         stopping the wave here would under-fill exactly the case the stranded \
         reclaim exists to serve"
    );
}

/// The other direction: the same worker receiving a DIFFERENT node is also real
/// progress. This is the unpinned path's normal case (one busy worker, several
/// ready nodes) and must not be mistaken for a replay.
#[test]
fn run_plan_dispatch_repeats_exempts_a_different_node_to_the_same_worker() {
    let accepted = vec![("catalog-evaluate".to_string(), "session_kangaroo".to_string())];

    assert!(
        !super::run_plan_dispatch_repeats(&accepted, "pricing-rollout", "session_kangaroo"),
        "a different node dispatched to the same worker is a distinct dispatch, \
         not a replayed response"
    );
}

/// Empty state: the first dispatch of a wave is never a repeat.
#[test]
fn run_plan_dispatch_repeats_is_false_before_anything_is_accepted() {
    assert!(
        !super::run_plan_dispatch_repeats(&[], "catalog-evaluate", "session_kangaroo"),
        "the first assignment of a coordination loop is always real progress"
    );
}

/// Socket-level check of the loop's accounting over a real dispatch wave.
///
/// HONEST SCOPE, stated because the plan has nodes dedicated to vacuous tests:
/// whether the server-side replay actually fires here is INTERMITTENT. Four runs
/// of this exact fixture on HEAD's `comm_control.rs` produced three distinct
/// node ids (node-a, node-b, node-c, all to the one pinned worker) with no
/// replay, and one run produced a replay on the second call, which the guard
/// caught and stopped:
///     assigned node-a -> session_badger_...
///     assign_next replayed an already-dispatched assignment (node-a -> ...); stopping this dispatch wave
/// So the replay is real (and independently pinned server-side by
/// `pinned_assign_next_replays_inside_the_final_state_ttl`), but this fixture
/// does not force it. What the assertions below pin unconditionally is the
/// client property the guard maintains: within one dispatch wave no
/// `(task, worker)` pair is accepted twice, and the wave never reports more
/// dispatches than the plan has nodes. Both fail loudly if the guard is
/// inverted, too aggressive (under-filling), or absent in a replay run.
#[tokio::test]
async fn run_plan_pinned_wave_never_accepts_the_same_task_and_worker_pair_twice() {
    let _env_lock = crate::storage::lock_test_env();
    let runtime_dir = tempfile::TempDir::new().expect("runtime tempdir");
    let repo_dir = std::env::current_dir().expect("repo cwd");
    let socket_path = runtime_dir.path().join("jcode.sock");
    let _runtime = EnvGuard::set("JCODE_RUNTIME_DIR", runtime_dir.path());
    // Keep the developer's real config (agents.swarm_model, providers) out of the run.
    let _home = EnvGuard::set("JCODE_HOME", runtime_dir.path());
    let _socket = EnvGuard::set("JCODE_SOCKET", &socket_path);
    let _debug = EnvGuard::set("JCODE_DEBUG_CONTROL", "1");
    // Independently created root sessions own separate swarms; opt both clients
    // into one shared swarm explicitly.
    let _swarm = EnvGuard::set("JCODE_SWARM_ID", "run-plan-replay-guard-shared-swarm");

    let provider: Arc<dyn Provider> = Arc::new(DelayedTestProvider {
        delay: Duration::from_millis(50),
    });
    let server = Arc::new(Server::new(provider));
    let mut server_task = {
        let server = Arc::clone(&server);
        tokio::spawn(async move { server.run().await })
    };

    let socket_path = runtime_dir.path().join("jcode.sock");
    wait_for_server_socket(&socket_path, &mut server_task)
        .await
        .expect("server socket should be ready");

    let mut coordinator = RawClient::connect(&socket_path)
        .await
        .expect("coordinator should connect");
    let mut worker = RawClient::connect(&socket_path)
        .await
        .expect("worker should connect");
    coordinator
        .subscribe(&repo_dir)
        .await
        .expect("coordinator subscribe");
    worker.subscribe(&repo_dir).await.expect("worker subscribe");

    let coordinator_session = coordinator
        .session_id()
        .await
        .expect("coordinator session id");
    let worker_session = worker.session_id().await.expect("worker session id");
    wait_for_member_presence(&mut coordinator, &coordinator_session, &worker_session)
        .await
        .expect("worker should join the coordinator's swarm");

    let tool = CommunicateTool::new();
    let ctx = test_ctx(&coordinator_session, &repo_dir);

    let node_ids = ["node-a", "node-b", "node-c"];
    tool.execute(
        json!({
            "action": "task_graph",
            "mode": "light",
            "nodes": node_ids.iter().map(|id| json!({
                "id": id,
                "content": "Reply with a short acknowledgement.",
                "kind": "verify",
                "depends_on": [],
                "priority": 10
            })).collect::<Vec<_>>()
        }),
        ctx.clone(),
    )
    .await
    .expect("task_graph seed should succeed");

    let started = tool
        .execute(
            json!({
                "action": "run_plan",
                "target_session": worker_session.clone(),
                "concurrency_limit": node_ids.len(),
                "timeout_minutes": 1,
                "retain_agents": true
            }),
            ctx.clone(),
        )
        .await
        .expect("run_plan should start a background driver");

    let output_file = started
        .metadata
        .as_ref()
        .and_then(|metadata| metadata.get("output_file"))
        .and_then(|path| path.as_str())
        .map(std::path::PathBuf::from)
        .unwrap_or_else(|| panic!("run_plan should report an output file: {}", started.output));

    // The reporter creates the file lazily on its first log write, so poll for
    // content instead of asserting existence up front. `finalize` appends the
    // "--- run log ---" separator on either exit path, so it is the signal that
    // the driver stopped logging.
    let deadline = tokio::time::Instant::now() + Duration::from_secs(120);
    let mut log = String::new();
    loop {
        log = tokio::fs::read_to_string(&output_file)
            .await
            .unwrap_or_default();
        if log.contains("--- run log ---") || tokio::time::Instant::now() >= deadline {
            break;
        }
        tokio::time::sleep(Duration::from_millis(100)).await;
    }
    println!("--- run_plan driver output ---\n{log}\n--- end ---");

    let accepted: Vec<(String, String)> = log
        .lines()
        .filter_map(|line| line.trim().strip_prefix("assigned "))
        .filter_map(|line| {
            let (task, target) = line.split_once(" -> ")?;
            Some((task.to_string(), target.to_string()))
        })
        .collect();

    let mut unique = accepted.clone();
    unique.sort();
    unique.dedup();
    assert_eq!(
        unique.len(),
        accepted.len(),
        "the dispatch loop accepted the same (task, worker) pair twice in one wave: {accepted:?}"
    );

    // Anti-inflation: one wave can never report more dispatches than the plan
    // has nodes. Before the guard, a replayed response was counted once per open
    // slot, which is upstream #1209's "8 identical assignments".
    assert!(
        accepted.len() <= node_ids.len(),
        "the wave reported {} dispatches for a {}-node plan: {accepted:?}",
        accepted.len(),
        node_ids.len()
    );

    // When the server replayed, the guard must have stopped the wave at the
    // replay instead of spending the remaining slots on the same response.
    if log.contains("assign_next replayed an already-dispatched assignment") {
        assert_eq!(
            accepted.len(),
            1,
            "the guard reported a replay but the wave still accepted {} dispatch(es): \
             {accepted:?}. It must break out of the dispatch loop at the replay.",
            accepted.len()
        );
    }

    server_task.abort();
}
