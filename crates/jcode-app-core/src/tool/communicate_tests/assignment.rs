#[tokio::test]
async fn communicate_assign_task_can_spawn_fallback_agent() {
    let _env_lock = crate::storage::lock_test_env();
    let runtime_dir = tempfile::TempDir::new().expect("runtime tempdir");
    let repo_dir = std::env::current_dir().expect("repo cwd");
    let socket_path = runtime_dir.path().join("jcode.sock");
    let _runtime = EnvGuard::set("JCODE_RUNTIME_DIR", runtime_dir.path());
    // Stand in for a developer config that pins spawns to an unrelated model.
    // The isolation guard below must hide it, or this test fails.
    let outer_home = tempfile::TempDir::new().expect("outer home tempdir");
    std::fs::write(
        outer_home.path().join("config.toml"),
        "[agents]\nswarm_model = \"openai-api:isolation-probe-model\"\n",
    )
    .expect("write conflicting outer config");
    let _outer_home = EnvGuard::set("JCODE_HOME", outer_home.path());
    // Keep the developer's real config (agents.swarm_model, providers) out of spawns.
    let _home = EnvGuard::set("JCODE_HOME", runtime_dir.path());
    // The env override would set agents.swarm_model regardless of JCODE_HOME.
    let _swarm_model = EnvGuard::remove("JCODE_SWARM_MODEL");
    let _socket = EnvGuard::set("JCODE_SOCKET", &socket_path);
    let _debug = EnvGuard::set("JCODE_DEBUG_CONTROL", "1");
    crate::config::invalidate_config_cache();
    assert_eq!(
        crate::config::config().agents.swarm_model,
        None,
        "spawn config must come from the isolated JCODE_HOME"
    );

    let provider: Arc<dyn Provider> = Arc::new(DelayedTestProvider {
        delay: Duration::from_millis(100),
    });
    let server = Arc::new(Server::new(provider));
    let mut server_task = {
        let server = Arc::clone(&server);
        tokio::spawn(async move { server.run().await })
    };

    wait_for_server_socket(&socket_path, &mut server_task)
        .await
        .expect("server socket should be ready");

    let mut watcher = RawClient::connect(&socket_path)
        .await
        .expect("watcher should connect");
    watcher
        .subscribe(&repo_dir)
        .await
        .expect("watcher subscribe");

    let watcher_session = watcher.session_id().await.expect("watcher session id");
    let tool = CommunicateTool::new();
    let ctx = test_ctx(&watcher_session, &repo_dir);

    tool.execute(
        json!({
            "action": "assign_role",
            "target_session": watcher_session,
            "role": "coordinator"
        }),
        ctx.clone(),
    )
    .await
    .expect("self-promotion to coordinator should succeed");

    tool.execute(
        json!({
            "action": "propose_plan",
            "plan_items": [{
                "id": "task-a",
                "content": "Implement planner follow-up",
                "status": "queued",
                "priority": "high"
            }]
        }),
        ctx.clone(),
    )
    .await
    .expect("plan proposal should succeed");

    let assign_output = tool
        .execute(
            json!({
                "action": "assign_task",
                "spawn_if_needed": true
            }),
            ctx,
        )
        .await
        .expect("assign_task should spawn a fallback worker");

    assert!(
        assign_output.output.contains("spawned automatically"),
        "expected fallback spawn in output, got: {}",
        assign_output.output
    );
    assert!(
        assign_output.output.contains("task-a"),
        "expected selected task id in output, got: {}",
        assign_output.output
    );

    let spawned_session = assign_output
        .output
        .strip_prefix("Task 'task-a' assigned to ")
        .and_then(|rest| rest.strip_suffix(" (spawned automatically)"))
        .expect("assign output should include spawned session id")
        .trim()
        .to_string();

    assert!(
        !spawned_session.is_empty(),
        "spawned session id should not be empty"
    );

    wait_for_member_presence(&mut watcher, &watcher_session, &spawned_session)
        .await
        .expect("spawned fallback worker should appear in swarm");

    let members = watcher
        .comm_list(&watcher_session)
        .await
        .expect("comm_list should succeed");
    let spawned_member = members
        .iter()
        .find(|member| member.session_id == spawned_session)
        .expect("spawned worker should be listed");
    assert_eq!(spawned_member.role.as_deref(), Some("agent"));

    server_task.abort();
}

#[tokio::test]
async fn communicate_assign_next_assigns_next_runnable_task() {
    let _env_lock = crate::storage::lock_test_env();
    let runtime_dir = tempfile::TempDir::new().expect("runtime tempdir");
    let repo_dir = std::env::current_dir().expect("repo cwd");
    let socket_path = runtime_dir.path().join("jcode.sock");
    let _runtime = EnvGuard::set("JCODE_RUNTIME_DIR", runtime_dir.path());
    // Keep the developer's real config (agents.swarm_model, providers) out of spawns.
    let _home = EnvGuard::set("JCODE_HOME", runtime_dir.path());
    let _socket = EnvGuard::set("JCODE_SOCKET", &socket_path);
    let _debug = EnvGuard::set("JCODE_DEBUG_CONTROL", "1");

    let provider: Arc<dyn Provider> = Arc::new(DelayedTestProvider {
        delay: Duration::from_millis(100),
    });
    let server = Arc::new(Server::new(provider));
    let mut server_task = {
        let server = Arc::clone(&server);
        tokio::spawn(async move { server.run().await })
    };

    wait_for_server_socket(&socket_path, &mut server_task)
        .await
        .expect("server socket should be ready");

    let mut watcher = RawClient::connect(&socket_path)
        .await
        .expect("watcher should connect");
    watcher
        .subscribe(&repo_dir)
        .await
        .expect("watcher subscribe");

    let watcher_session = watcher.session_id().await.expect("watcher session id");
    let tool = CommunicateTool::new();
    let ctx = test_ctx(&watcher_session, &repo_dir);

    tool.execute(
        json!({
            "action": "assign_role",
            "target_session": watcher_session,
            "role": "coordinator"
        }),
        ctx.clone(),
    )
    .await
    .expect("self-promotion to coordinator should succeed");

    let spawn_output = tool
        .execute(
            json!({
                "action": "spawn",
                "label": "next-task worker"
            }),
            ctx.clone(),
        )
        .await
        .expect("worker spawn should succeed");
    let worker_session = spawn_output
        .output
        .strip_prefix("Spawned new agent: ")
        .expect("spawn output should include session id")
        .trim()
        .to_string();

    wait_for_member_presence(&mut watcher, &watcher_session, &worker_session)
        .await
        .expect("spawned worker should appear in swarm");

    tool.execute(
        json!({
            "action": "propose_plan",
            "plan_items": [{
                "id": "setup",
                "content": "setup",
                "status": "completed",
                "priority": "high"
            }, {
                "id": "next",
                "content": "Take the next task",
                "status": "queued",
                "priority": "high",
                "blocked_by": ["setup"]
            }]
        }),
        ctx.clone(),
    )
    .await
    .expect("plan proposal should succeed");

    let assign_output = tool
        .execute(
            json!({
                "action": "assign_next",
                "target_session": worker_session
            }),
            ctx,
        )
        .await
        .expect("assign_next should succeed");

    assert!(
        assign_output.output.contains("Task 'next' assigned to"),
        "unexpected assign_next output: {}",
        assign_output.output
    );

    server_task.abort();
}

#[tokio::test]
async fn communicate_assign_next_can_prefer_fresh_spawn_server_side() {
    let _env_lock = crate::storage::lock_test_env();
    let runtime_dir = tempfile::TempDir::new().expect("runtime tempdir");
    let repo_dir = std::env::current_dir().expect("repo cwd");
    let socket_path = runtime_dir.path().join("jcode.sock");
    let _runtime = EnvGuard::set("JCODE_RUNTIME_DIR", runtime_dir.path());
    // Keep the developer's real config (agents.swarm_model, providers) out of spawns.
    let _home = EnvGuard::set("JCODE_HOME", runtime_dir.path());
    let _socket = EnvGuard::set("JCODE_SOCKET", &socket_path);
    let _debug = EnvGuard::set("JCODE_DEBUG_CONTROL", "1");

    let provider: Arc<dyn Provider> = Arc::new(DelayedTestProvider {
        delay: Duration::from_millis(100),
    });
    let server = Arc::new(Server::new(provider));
    let mut server_task = {
        let server = Arc::clone(&server);
        tokio::spawn(async move { server.run().await })
    };

    wait_for_server_socket(&socket_path, &mut server_task)
        .await
        .expect("server socket should be ready");

    let mut watcher = RawClient::connect(&socket_path)
        .await
        .expect("watcher should connect");
    watcher
        .subscribe(&repo_dir)
        .await
        .expect("watcher subscribe");

    let watcher_session = watcher.session_id().await.expect("watcher session id");
    let tool = CommunicateTool::new();
    let ctx = test_ctx(&watcher_session, &repo_dir);

    tool.execute(
        json!({
            "action": "assign_role",
            "target_session": watcher_session,
            "role": "coordinator"
        }),
        ctx.clone(),
    )
    .await
    .expect("self-promotion to coordinator should succeed");

    let existing_output = tool
        .execute(
            json!({"action": "spawn", "label": "existing worker"}),
            ctx.clone(),
        )
        .await
        .expect("existing worker spawn should succeed");
    let existing_worker = existing_output
        .output
        .strip_prefix("Spawned new agent: ")
        .expect("spawn output should include session id")
        .trim()
        .to_string();
    wait_for_member_presence(&mut watcher, &watcher_session, &existing_worker)
        .await
        .expect("existing worker should appear in swarm");

    tool.execute(
        json!({
            "action": "propose_plan",
            "plan_items": [{
                "id": "task-c",
                "content": "Use a fresh worker",
                "status": "queued",
                "priority": "high"
            }]
        }),
        ctx.clone(),
    )
    .await
    .expect("plan proposal should succeed");

    let assign_output = tool
        .execute(
            json!({
                "action": "assign_next",
                "prefer_spawn": true
            }),
            ctx,
        )
        .await
        .expect("assign_next with prefer_spawn should succeed");

    let preferred_session = assign_output
        .output
        .strip_prefix("Task 'task-c' assigned to ")
        .expect("assign_next output should include session id")
        .trim()
        .to_string();

    assert_ne!(
        preferred_session, existing_worker,
        "server-side prefer_spawn should choose a fresh worker"
    );

    wait_for_member_presence(&mut watcher, &watcher_session, &preferred_session)
        .await
        .expect("preferred spawned worker should appear in swarm");

    server_task.abort();
}

#[tokio::test]
async fn communicate_assign_next_can_spawn_if_needed_server_side() {
    let _env_lock = crate::storage::lock_test_env();
    let runtime_dir = tempfile::TempDir::new().expect("runtime tempdir");
    let repo_dir = std::env::current_dir().expect("repo cwd");
    let socket_path = runtime_dir.path().join("jcode.sock");
    let _runtime = EnvGuard::set("JCODE_RUNTIME_DIR", runtime_dir.path());
    // Keep the developer's real config (agents.swarm_model, providers) out of spawns.
    let _home = EnvGuard::set("JCODE_HOME", runtime_dir.path());
    let _socket = EnvGuard::set("JCODE_SOCKET", &socket_path);
    let _debug = EnvGuard::set("JCODE_DEBUG_CONTROL", "1");

    let provider: Arc<dyn Provider> = Arc::new(DelayedTestProvider {
        delay: Duration::from_millis(100),
    });
    let server = Arc::new(Server::new(provider));
    let mut server_task = {
        let server = Arc::clone(&server);
        tokio::spawn(async move { server.run().await })
    };

    wait_for_server_socket(&socket_path, &mut server_task)
        .await
        .expect("server socket should be ready");

    let mut watcher = RawClient::connect(&socket_path)
        .await
        .expect("watcher should connect");
    watcher
        .subscribe(&repo_dir)
        .await
        .expect("watcher subscribe");

    let watcher_session = watcher.session_id().await.expect("watcher session id");
    let tool = CommunicateTool::new();
    let ctx = test_ctx(&watcher_session, &repo_dir);

    tool.execute(
        json!({
            "action": "assign_role",
            "target_session": watcher_session,
            "role": "coordinator"
        }),
        ctx.clone(),
    )
    .await
    .expect("self-promotion to coordinator should succeed");

    tool.execute(
        json!({
            "action": "propose_plan",
            "plan_items": [{
                "id": "task-d",
                "content": "Spawn if no worker exists",
                "status": "queued",
                "priority": "high"
            }]
        }),
        ctx.clone(),
    )
    .await
    .expect("plan proposal should succeed");

    let assign_output = tool
        .execute(
            json!({
                "action": "assign_next",
                "spawn_if_needed": true
            }),
            ctx,
        )
        .await
        .expect("assign_next with spawn_if_needed should succeed");

    let spawned_session = assign_output
        .output
        .strip_prefix("Task 'task-d' assigned to ")
        .expect("assign_next output should include session id")
        .trim()
        .to_string();
    assert!(
        !spawned_session.is_empty(),
        "server-side spawn_if_needed should assign a spawned worker"
    );

    wait_for_member_presence(&mut watcher, &watcher_session, &spawned_session)
        .await
        .expect("spawn_if_needed worker should appear in swarm");

    server_task.abort();
}

#[tokio::test]
async fn communicate_fill_slots_tops_up_to_concurrency_limit() {
    let _env_lock = crate::storage::lock_test_env();
    let runtime_dir = tempfile::TempDir::new().expect("runtime tempdir");
    let repo_dir = std::env::current_dir().expect("repo cwd");
    let socket_path = runtime_dir.path().join("jcode.sock");
    let _runtime = EnvGuard::set("JCODE_RUNTIME_DIR", runtime_dir.path());
    // Keep the developer's real config (agents.swarm_model, providers) out of spawns.
    let _home = EnvGuard::set("JCODE_HOME", runtime_dir.path());
    let _socket = EnvGuard::set("JCODE_SOCKET", &socket_path);
    let _debug = EnvGuard::set("JCODE_DEBUG_CONTROL", "1");

    let provider: Arc<dyn Provider> = Arc::new(DelayedTestProvider {
        delay: Duration::from_millis(300),
    });
    let server = Arc::new(Server::new(provider));
    let mut server_task = {
        let server = Arc::clone(&server);
        tokio::spawn(async move { server.run().await })
    };

    wait_for_server_socket(&socket_path, &mut server_task)
        .await
        .expect("server socket should be ready");

    let mut watcher = RawClient::connect(&socket_path)
        .await
        .expect("watcher should connect");
    watcher
        .subscribe(&repo_dir)
        .await
        .expect("watcher subscribe");

    let watcher_session = watcher.session_id().await.expect("watcher session id");
    let tool = CommunicateTool::new();
    let ctx = test_ctx(&watcher_session, &repo_dir);

    tool.execute(
        json!({
            "action": "assign_role",
            "target_session": watcher_session,
            "role": "coordinator"
        }),
        ctx.clone(),
    )
    .await
    .expect("self-promotion to coordinator should succeed");

    tool.execute(
        json!({
            "action": "propose_plan",
            "plan_items": [{
                "id": "task-1",
                "content": "first task",
                "status": "queued",
                "priority": "high"
            }, {
                "id": "task-2",
                "content": "second task",
                "status": "queued",
                "priority": "high"
            }, {
                "id": "task-3",
                "content": "third task",
                "status": "queued",
                "priority": "high"
            }]
        }),
        ctx.clone(),
    )
    .await
    .expect("plan proposal should succeed");

    let output = tool
        .execute(
            json!({
                "action": "fill_slots",
                "concurrency_limit": 2,
                "spawn_if_needed": true
            }),
            ctx,
        )
        .await
        .expect("fill_slots should succeed");

    assert!(
        output.output.contains("Filled 2 slot(s):"),
        "unexpected fill_slots output: {}",
        output.output
    );

    // The count alone is not the property that matters. Upstream issue #1209 is
    // exactly this: "the same node is assigned multiple times to the same
    // session ... 8 identical assignments of catalog-evaluate to
    // session_kangaroo over a 13-wide frontier", caused by the fill loop
    // re-picking the head-of-queue node instead of deduplicating. A 3-node plan
    // with limit 2 must therefore hand out two DIFFERENT nodes; if the loop
    // double-assigns, this still says "Filled 2 slot(s)" and only this fails.
    let assigned: Vec<&str> = output
        .output
        .lines()
        .filter_map(|line| line.trim_start().strip_prefix("- "))
        .filter_map(|line| line.split(" -> ").next())
        .collect();
    assert_eq!(
        assigned.len(),
        2,
        "expected two assignment lines, got: {}",
        output.output
    );
    let mut distinct = assigned.clone();
    distinct.sort_unstable();
    distinct.dedup();
    assert_eq!(
        distinct.len(),
        assigned.len(),
        "fill_slots assigned the same node twice ({:?}), which wastes a ready \
         frontier slot on duplicate work - upstream issue #1209",
        assigned
    );

    server_task.abort();
}

/// The reproduction condition for upstream issue #1209 that the top-up test
/// above does not reach: #1209 is "the same node is assigned multiple times to
/// THE SAME SESSION", which needs the assignee pinned via `target_session`. With
/// no pinned target each `assign_next` can reuse or spawn freely and the server
/// advances the frontier on its own, so the plain top-up case passes.
///
/// Pinning one worker and offering a wider frontier is what a coordinator does
/// when it wants a specific worker to pick up work, and it is where a
/// non-deduplicating fill loop would hand the same node back repeatedly.
///
/// The pin MUST land on a session other than the coordinator. This test used to
/// build `ctx` from `watcher_session` AND pin `target_session` to that same
/// session, so `req_session_id == requested_target` and the server rejected the
/// call outright at `comm_control.rs:509-512` ("Coordinator cannot assign a swarm
/// task to itself."). That message matches neither fill-loop break arm
/// (`communicate.rs:3283-3288`), so it fell through to `ensure_success` and
/// returned `Err` on iteration 1 -- the test contributed ZERO signal about
/// #1209 and never reached its assertion.
#[tokio::test]
async fn fill_slots_pinned_target_does_not_double_assign_one_node() {
    let _env_lock = crate::storage::lock_test_env();
    let runtime_dir = tempfile::TempDir::new().expect("runtime tempdir");
    let repo_dir = std::env::current_dir().expect("repo cwd");
    let socket_path = runtime_dir.path().join("jcode.sock");
    let _runtime = EnvGuard::set("JCODE_RUNTIME_DIR", runtime_dir.path());
    // Two independent root sessions default to DISTINCT swarms
    // (`swarm_id_for_session` -> `session:{session_id}`, server/util.rs:349-365), so a
    // second client would land in its own swarm and never become assignable. This is the
    // documented opt-in to a shared swarm, and the same guard the sender/peer test uses
    // at end_to_end.rs:583.
    let _swarm = EnvGuard::set("JCODE_SWARM_ID", "pinned-fill-slots-shared-swarm");
    let _home = EnvGuard::set("JCODE_HOME", runtime_dir.path());
    let _socket = EnvGuard::set("JCODE_SOCKET", &socket_path);
    let _debug = EnvGuard::set("JCODE_DEBUG_CONTROL", "1");

    let provider: Arc<dyn Provider> = Arc::new(DelayedTestProvider {
        delay: Duration::from_millis(300),
    });
    let server = Arc::new(Server::new(provider));
    let mut server_task = {
        let server = Arc::clone(&server);
        tokio::spawn(async move { server.run().await })
    };

    wait_for_server_socket(&socket_path, &mut server_task)
        .await
        .expect("server socket should be ready");

    let mut watcher = RawClient::connect(&socket_path)
        .await
        .expect("watcher should connect");
    // A REAL second, non-coordinator worker session to pin. #1209 is "the same
    // node is assigned multiple times to THE SAME SESSION", so the pin has to
    // land on somebody other than the coordinator. Two RawClients on one socket,
    // both subscribed to the same working dir, land in the same swarm once
    // JCODE_SWARM_ID above pins it: the sender/peer pattern at
    // end_to_end.rs:598-613.
    let mut worker = RawClient::connect(&socket_path)
        .await
        .expect("pinned worker should connect");
    watcher
        .subscribe(&repo_dir)
        .await
        .expect("watcher subscribe");
    worker
        .subscribe(&repo_dir)
        .await
        .expect("pinned worker subscribe");

    let watcher_session = watcher.session_id().await.expect("watcher session id");
    let worker_session = worker.session_id().await.expect("pinned worker session id");
    wait_for_member_presence(&mut watcher, &watcher_session, &worker_session)
        .await
        .expect("pinned worker should join the swarm");

    let tool = CommunicateTool::new();
    // The coordinator is still `watcher_session`; only the PIN moved.
    let ctx = test_ctx(&watcher_session, &repo_dir);

    tool.execute(
        json!({
            "action": "assign_role",
            "target_session": watcher_session.clone(),
            "role": "coordinator"
        }),
        ctx.clone(),
    )
    .await
    .expect("self-promotion to coordinator should succeed");

    tool.execute(
        json!({
            "action": "propose_plan",
            "plan_items": [
                { "id": "task-a", "content": "a", "status": "queued", "priority": "high" },
                { "id": "task-b", "content": "b", "status": "queued", "priority": "high" },
                { "id": "task-c", "content": "c", "status": "queued", "priority": "high" }
            ]
        }),
        ctx.clone(),
    )
    .await
    .expect("plan proposal should succeed");

    let output = tool
        .execute(
            json!({
                "action": "fill_slots",
                "concurrency_limit": 3,
                "target_session": worker_session.clone(),
                "spawn_if_needed": false
            }),
            ctx,
        )
        .await
        .expect("fill_slots with a pinned target should succeed");

    let assigned: Vec<&str> = output
        .output
        .lines()
        .filter_map(|line| line.trim_start().strip_prefix("- "))
        .filter_map(|line| line.split(" -> ").next())
        .collect();

    let mut distinct = assigned.clone();
    distinct.sort_unstable();
    distinct.dedup();
    let distinct_len = distinct.len();
    // NOT `distinct.len() == assigned.len()`. Both sides are derived from the
    // SAME `assigned` vector, so that assertion is satisfied by 0, 1, 2 or 3
    // assignments and therefore proves nothing. With the client-side repeat
    // guard live (communicate.rs:3277 breaks the loop the instant a task id
    // repeats) a pinned fill_slots assigns task-a, sees task-a again on the next
    // identical pinned request, and stops: 1 == 1, GREEN, against a server that
    // is still replaying. Assert the OUTCOME the client is supposed to produce
    // instead -- a 3-node plan at concurrency_limit 3 must hand the pinned worker
    // three DISTINCT nodes.
    //
    // That expectation is invariant to whether the client guard exists:
    //   guard live   -> server replays task-a, loop breaks -> len 1, != 3 -> RED
    //   guard absent -> loop emits task-a three times        -> len 3, 1 distinct -> RED
    // Both worlds fail, so this assertion cannot be satisfied by masking the
    // server bug. See artifacts/gap-fixture-and-assertion-vacuous.md for why 3
    // is the intended pinned outcome rather than 1.
    assert_eq!(
        assigned.len(),
        3,
        "fill_slots at concurrency_limit 3 over a 3-node plan should hand pinned \
         worker {worker_session} three assignments, but got {assigned:?} \
         ({distinct_len} distinct). Fewer than three means the fill loop stopped \
         early, which is the #1209 symptom: the pinned CommAssignNext dedup key \
         collapses to the constant \"__next_runnable__\" \
         (comm_control.rs:1481-1494), AssignDedupMode::ReplayFinal \
         (comm_control.rs:1416) then replays the FIRST pinned response instead of \
         advancing the frontier (swarm_mutation_state.rs:219-224), and the client \
         repeat guard at communicate.rs:3277 then breaks the loop on the replayed \
         duplicate. Raw fill_slots output:\n{raw}",
        raw = output.output
    );
    assert_eq!(
        distinct_len,
        3,
        "fill_slots handed the same node back to one pinned session \
         ({assigned:?}, {distinct_len} distinct) - upstream issue #1209. Raw \
         fill_slots output:\n{raw}",
        raw = output.output
    );

    server_task.abort();
}

#[tokio::test]
async fn communicate_assign_task_can_prefer_fresh_spawn_over_reuse() {
    let _env_lock = crate::storage::lock_test_env();
    let runtime_dir = tempfile::TempDir::new().expect("runtime tempdir");
    let repo_dir = std::env::current_dir().expect("repo cwd");
    let socket_path = runtime_dir.path().join("jcode.sock");
    let _runtime = EnvGuard::set("JCODE_RUNTIME_DIR", runtime_dir.path());
    // Keep the developer's real config (agents.swarm_model, providers) out of spawns.
    let _home = EnvGuard::set("JCODE_HOME", runtime_dir.path());
    let _socket = EnvGuard::set("JCODE_SOCKET", &socket_path);
    let _debug = EnvGuard::set("JCODE_DEBUG_CONTROL", "1");

    let provider: Arc<dyn Provider> = Arc::new(DelayedTestProvider {
        delay: Duration::from_millis(100),
    });
    let server = Arc::new(Server::new(provider));
    let mut server_task = {
        let server = Arc::clone(&server);
        tokio::spawn(async move { server.run().await })
    };

    wait_for_server_socket(&socket_path, &mut server_task)
        .await
        .expect("server socket should be ready");

    let mut watcher = RawClient::connect(&socket_path)
        .await
        .expect("watcher should connect");
    watcher
        .subscribe(&repo_dir)
        .await
        .expect("watcher subscribe");

    let watcher_session = watcher.session_id().await.expect("watcher session id");
    let tool = CommunicateTool::new();
    let ctx = test_ctx(&watcher_session, &repo_dir);

    tool.execute(
        json!({
            "action": "assign_role",
            "target_session": watcher_session,
            "role": "coordinator"
        }),
        ctx.clone(),
    )
    .await
    .expect("self-promotion to coordinator should succeed");

    let existing_output = tool
        .execute(
            json!({
                "action": "spawn",
                "label": "reusable worker"
            }),
            ctx.clone(),
        )
        .await
        .expect("existing reusable worker should spawn");
    let existing_worker = existing_output
        .output
        .strip_prefix("Spawned new agent: ")
        .expect("spawn output should include session id")
        .trim()
        .to_string();
    wait_for_member_presence(&mut watcher, &watcher_session, &existing_worker)
        .await
        .expect("existing worker should appear in swarm");

    tool.execute(
        json!({
            "action": "propose_plan",
            "plan_items": [{
                "id": "task-b",
                "content": "Investigate a separate subsystem",
                "status": "queued",
                "priority": "high"
            }]
        }),
        ctx.clone(),
    )
    .await
    .expect("plan proposal should succeed");

    let assign_output = tool
        .execute(
            json!({
                "action": "assign_task",
                "prefer_spawn": true
            }),
            ctx,
        )
        .await
        .expect("assign_task with prefer_spawn should succeed");

    assert!(
        assign_output
            .output
            .contains("spawned by planner preference"),
        "expected planner-preference spawn in output, got: {}",
        assign_output.output
    );
    assert!(
        assign_output.output.contains("task-b"),
        "expected selected task id in output, got: {}",
        assign_output.output
    );

    let preferred_session = assign_output
        .output
        .strip_prefix("Task 'task-b' assigned to ")
        .and_then(|rest| rest.strip_suffix(" (spawned by planner preference)"))
        .expect("assign output should include preferred spawned session id")
        .trim()
        .to_string();

    assert_ne!(
        preferred_session, existing_worker,
        "prefer_spawn should choose a fresh worker instead of reusing the existing one"
    );

    wait_for_member_presence(&mut watcher, &watcher_session, &preferred_session)
        .await
        .expect("preferred spawned worker should appear in swarm");

    server_task.abort();
}
