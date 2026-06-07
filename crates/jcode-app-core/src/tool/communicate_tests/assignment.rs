#[tokio::test]
async fn communicate_assign_task_can_spawn_fallback_agent() {
    let _env_lock = crate::storage::lock_test_env();
    let runtime_dir = tempfile::TempDir::new().expect("runtime tempdir");
    let repo_dir = std::env::current_dir().expect("repo cwd");
    let socket_path = runtime_dir.path().join("jcode.sock");
    let _runtime = EnvGuard::set("JCODE_RUNTIME_DIR", runtime_dir.path());
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
                "action": "spawn"
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
        .execute(json!({"action": "spawn"}), ctx.clone())
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

    server_task.abort();
}

#[tokio::test]
async fn communicate_assign_task_can_prefer_fresh_spawn_over_reuse() {
    let _env_lock = crate::storage::lock_test_env();
    let runtime_dir = tempfile::TempDir::new().expect("runtime tempdir");
    let repo_dir = std::env::current_dir().expect("repo cwd");
    let socket_path = runtime_dir.path().join("jcode.sock");
    let _runtime = EnvGuard::set("JCODE_RUNTIME_DIR", runtime_dir.path());
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
                "action": "spawn"
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
