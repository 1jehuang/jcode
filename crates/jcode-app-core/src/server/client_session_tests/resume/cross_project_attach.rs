// A cross-project attach must not re-pin the target session's working directory.
//
// One daemon serves sessions for many projects. When a client in project A
// attaches to a live session stored under project B, the subscriber's directory
// must not become the session's directory: that would move B's bash and file
// tools, project-local MCP config, memory scope, and swarm grouping into A.
//
// Both paths are covered. `handle_resume_session` is where the restore actually
// binds the directory, and `handle_subscribe` is the bookkeeping the desktop
// sends straight after a target-aware resume, which has its own overwrite path
// (`apply_or_defer_subscribe_working_dir`). Fixing only one would leave the
// other able to undo it.

use crate::session::Session;

/// Build a live target agent whose session is already bound to `working_dir`.
async fn target_agent_in_project(
    provider: Arc<dyn Provider>,
    registry: Registry,
    session_id: &str,
    working_dir: &str,
) -> Arc<Mutex<Agent>> {
    let mut session =
        crate::session::Session::create_with_id(session_id.to_string(), None, None);
    session.working_dir = Some(working_dir.to_string());
    // Persist it so `Session::load` inside the restore reads the same directory
    // back. The in-memory agent and the on-disk copy must agree, otherwise the
    // test would only prove that the disk copy won. `save_prepared` rather than
    // `save`: this session has no visible message yet, and plain `save`
    // deliberately skips writing that case.
    session.save_prepared().expect("persist target session");
    Arc::new(Mutex::new(Agent::new_with_session(
        provider,
        registry,
        session,
        None,
    )))
}

#[tokio::test]
async fn cross_project_attach_preserves_target_session_working_dir() -> Result<()> {
    let _guard = crate::storage::lock_test_env();
    let (_runtime, prev_runtime) = setup_runtime_dir()?;
    let _ = prev_runtime;

    let target_session_id = "session_cross_project_target";
    let temp_session_id = "session_cross_project_subscriber";
    let project_b = "/home/tester/work/project-b";
    let project_a = "/home/tester/work/project-a";

    let provider: Arc<dyn Provider> = Arc::new(MockProvider);
    let target_registry = Registry::new(provider.clone()).await;
    let existing_agent = target_agent_in_project(
        provider.clone(),
        target_registry,
        target_session_id,
        project_b,
    )
    .await;

    let new_registry = Registry::new(provider.clone()).await;
    let new_agent = Arc::new(Mutex::new(build_test_agent_with_id(
        provider.clone(),
        new_registry.clone(),
        temp_session_id,
        Vec::new(),
    )));

    let sessions = Arc::new(RwLock::new(HashMap::from([
        (target_session_id.to_string(), Arc::clone(&existing_agent)),
        (temp_session_id.to_string(), Arc::clone(&new_agent)),
    ])));
    let shutdown_signals = Arc::new(RwLock::new(HashMap::<String, InterruptSignal>::new()));
    let soft_interrupt_queues: SessionInterruptQueues = Arc::new(RwLock::new(HashMap::new()));
    let now = Instant::now();
    let client_connections = Arc::new(RwLock::new(HashMap::from([
        (
            "conn_existing".to_string(),
            ClientConnectionInfo {
                client_id: "conn_existing".to_string(),
                session_id: target_session_id.to_string(),
                client_instance_id: None,
                debug_client_id: None,
                connected_at: now,
                last_seen: now,
                is_processing: false,
                current_tool_name: None,
                terminal_env: Vec::new(),
                disconnect_tx: mpsc::unbounded_channel().0,
            },
        ),
        (
            "conn_new".to_string(),
            ClientConnectionInfo {
                client_id: "conn_new".to_string(),
                session_id: temp_session_id.to_string(),
                client_instance_id: None,
                debug_client_id: None,
                connected_at: now,
                last_seen: now,
                is_processing: false,
                current_tool_name: None,
                terminal_env: Vec::new(),
                disconnect_tx: mpsc::unbounded_channel().0,
            },
        ),
    ])));
    let swarm_members = Arc::new(RwLock::new(HashMap::<String, SwarmMember>::new()));
    let swarms_by_id = Arc::new(RwLock::new(HashMap::<String, HashSet<String>>::new()));
    let file_touch = FileTouchService::new();
    let channel_subscriptions = Arc::new(RwLock::new(HashMap::<
        String,
        HashMap<String, HashSet<String>>,
    >::new()));
    let channel_subscriptions_by_session = Arc::new(RwLock::new(HashMap::<
        String,
        HashMap<String, HashSet<String>>,
    >::new()));
    let swarm_plans = Arc::new(RwLock::new(HashMap::<String, VersionedPlan>::new()));
    let swarm_coordinators = Arc::new(RwLock::new(HashMap::<String, String>::new()));
    let client_count = Arc::new(RwLock::new(1usize));
    let (writer, _peer_stream) = test_writer()?;
    let (client_event_tx, _client_event_rx) = mpsc::unbounded_channel::<ServerEvent>();
    let event_history = Arc::new(RwLock::new(VecDeque::<SwarmEvent>::new()));
    let event_counter = Arc::new(std::sync::atomic::AtomicU64::new(0));
    let (swarm_event_tx, _swarm_event_rx) = broadcast::channel::<SwarmEvent>(8);
    let mcp_pool = Arc::new(crate::mcp::SharedMcpPool::from_default_config());

    let mut client_selfdev = false;
    let mut client_session_id = temp_session_id.to_string();

    // The subscriber reports project A while attaching to a session stored under
    // project B. This is the ordinary shape of a cross-project attach: the client
    // sends its own cwd, and the target's differs.
    let rebound_agent = handle_resume_session(
        91,
        target_session_id.to_string(),
        Some(project_a),
        None,
        false,
        false,
        &mut client_selfdev,
        &mut client_session_id,
        "conn_new",
        &new_agent,
        &provider,
        &new_registry,
        &sessions,
        &shutdown_signals,
        &soft_interrupt_queues,
        &client_connections,
        &Arc::new(RwLock::new(ClientDebugState::default())),
        &swarm_members,
        &swarms_by_id,
        &file_touch,
        &channel_subscriptions,
        &channel_subscriptions_by_session,
        &swarm_plans,
        &swarm_coordinators,
        &client_count,
        &writer,
        "test-server",
        "🌿",
        &client_event_tx,
        &mcp_pool,
        &event_history,
        &event_counter,
        &swarm_event_tx,
        false,
    )
    .await?;

    // A live target is adopted in place: `handle_resume_session` returns it without
    // calling `restore_session_with_working_dir`, so the directory it already had is
    // what must survive. Assert against the returned agent, not the entry the
    // subscriber came from -- the latter is never written on this path and made the
    // check pass for the wrong reason.
    let after_resume = rebound_agent.lock().await.working_dir().map(str::to_string);
    assert_eq!(
        after_resume.as_deref(),
        Some(project_b),
        "a cross-project attach re-pinned the target session into the subscriber's project"
    );

    // The desktop follows a target-aware resume with normal subscribe
    // bookkeeping, which has its own working-dir overwrite path. If that one
    // still honored the subscriber's directory it would undo the restore above,
    // so assert after it too rather than trusting the first check.
    handle_subscribe(
        91,
        Some(project_a.to_string()),
        None,
        false,
        &mut client_selfdev,
        target_session_id,
        "conn_new",
        &None,
        &rebound_agent,
        &new_registry,
        true,
        &swarm_members,
        &swarms_by_id,
        &channel_subscriptions,
        &channel_subscriptions_by_session,
        &swarm_plans,
        &swarm_coordinators,
        &client_event_tx,
        &mcp_pool,
        &event_history,
        &event_counter,
        &swarm_event_tx,
    )
    .await;

    let after_subscribe = rebound_agent.lock().await.working_dir().map(str::to_string);
    assert_eq!(
        after_subscribe.as_deref(),
        Some(project_b),
        "subscribe bookkeeping re-pinned the target session into the subscriber's project"
    );

    // The swarm member must agree with the session, or the session would be
    // grouped under the subscriber's project while its tools run in its own.
    let member_dir = swarm_members
        .read()
        .await
        .get(target_session_id)
        .and_then(|member| member.working_dir.as_ref())
        .map(|path| path.to_string_lossy().into_owned());
    if let Some(member_dir) = member_dir {
        assert_eq!(
            member_dir, project_b,
            "swarm membership was re-keyed to the subscriber's project"
        );
    }

    // And the on-disk copy must not have been rewritten either, or the next
    // daemon restart would resurrect the subscriber's directory.
    let persisted = Session::load(target_session_id)
        .expect("target session still loadable")
        .working_dir;
    assert_eq!(
        persisted.as_deref(),
        Some(project_b),
        "the persisted session was re-pinned, so a daemon restart would restore the wrong project"
    );

    Ok(())
}

/// The same attach, but against a session that is not live in memory.
///
/// This is the path the resume guard exists for. A live target is adopted in place
/// and never restored, so the directory check above passes whether or not the guard
/// runs. An offline session is loaded from disk by
/// `restore_session_with_working_dir`, which is the only code that would install the
/// subscriber's directory into the target's session.
#[tokio::test]
async fn cross_project_attach_of_offline_session_preserves_its_working_dir() -> Result<()> {
    let _guard = crate::storage::lock_test_env();
    let (_runtime, prev_runtime) = setup_runtime_dir()?;
    let _ = prev_runtime;

    let target_session_id = "session_cross_project_offline_target";
    let temp_session_id = "session_cross_project_offline_subscriber";
    let project_b = "/home/tester/work/project-offline-b";
    let project_a = "/home/tester/work/project-offline-a";

    // The target only exists on disk: it is not in `sessions`, so the resume takes
    // the restore-from-disk branch. Persist a directory under project B first.
    {
        let mut disk_session =
            crate::session::Session::create_with_id(target_session_id.to_string(), None, None);
        disk_session.working_dir = Some(project_b.to_string());
        disk_session.save_prepared().expect("persist offline target session");
    }

    let provider: Arc<dyn Provider> = Arc::new(MockProvider);
    let new_registry = Registry::new(provider.clone()).await;
    let new_agent = Arc::new(Mutex::new(build_test_agent_with_id(
        provider.clone(),
        new_registry.clone(),
        temp_session_id,
        Vec::new(),
    )));

    let sessions = Arc::new(RwLock::new(HashMap::from([(
        temp_session_id.to_string(),
        Arc::clone(&new_agent),
    )])));
    let shutdown_signals = Arc::new(RwLock::new(HashMap::<String, InterruptSignal>::new()));
    let soft_interrupt_queues: SessionInterruptQueues = Arc::new(RwLock::new(HashMap::new()));
    let client_connections = Arc::new(RwLock::new(HashMap::from([(
        "conn_new".to_string(),
        ClientConnectionInfo {
            client_id: "conn_new".to_string(),
            session_id: temp_session_id.to_string(),
            client_instance_id: None,
            debug_client_id: None,
            connected_at: Instant::now(),
            last_seen: Instant::now(),
            is_processing: false,
            current_tool_name: None,
            terminal_env: Vec::new(),
            disconnect_tx: mpsc::unbounded_channel().0,
        },
    )])));
    let swarm_members = Arc::new(RwLock::new(HashMap::<String, SwarmMember>::new()));
    let swarms_by_id = Arc::new(RwLock::new(HashMap::<String, HashSet<String>>::new()));
    let file_touch = FileTouchService::new();
    let channel_subscriptions = Arc::new(RwLock::new(HashMap::<
        String,
        HashMap<String, HashSet<String>>,
    >::new()));
    let channel_subscriptions_by_session = Arc::new(RwLock::new(HashMap::<
        String,
        HashMap<String, HashSet<String>>,
    >::new()));
    let swarm_plans = Arc::new(RwLock::new(HashMap::<String, VersionedPlan>::new()));
    let swarm_coordinators = Arc::new(RwLock::new(HashMap::<String, String>::new()));
    let client_count = Arc::new(RwLock::new(1usize));
    let (writer, _peer_stream) = test_writer()?;
    let (client_event_tx, _client_event_rx) = mpsc::unbounded_channel::<ServerEvent>();
    let event_history = Arc::new(RwLock::new(VecDeque::<SwarmEvent>::new()));
    let event_counter = Arc::new(std::sync::atomic::AtomicU64::new(0));
    let (swarm_event_tx, _swarm_event_rx) = broadcast::channel::<SwarmEvent>(8);
    let mcp_pool = Arc::new(crate::mcp::SharedMcpPool::from_default_config());

    let mut client_selfdev = false;
    let mut client_session_id = temp_session_id.to_string();

    let rebound_agent = handle_resume_session(
        92,
        target_session_id.to_string(),
        Some(project_a),
        None,
        false,
        false,
        &mut client_selfdev,
        &mut client_session_id,
        "conn_new",
        &new_agent,
        &provider,
        &new_registry,
        &sessions,
        &shutdown_signals,
        &soft_interrupt_queues,
        &client_connections,
        &Arc::new(RwLock::new(ClientDebugState::default())),
        &swarm_members,
        &swarms_by_id,
        &file_touch,
        &channel_subscriptions,
        &channel_subscriptions_by_session,
        &swarm_plans,
        &swarm_coordinators,
        &client_count,
        &writer,
        "test-server",
        "\u{1f33f}",
        &client_event_tx,
        &mcp_pool,
        &event_history,
        &event_counter,
        &swarm_event_tx,
        false,
    )
    .await?;

    let after_restore = rebound_agent.lock().await.working_dir().map(str::to_string);
    assert_eq!(
        after_restore.as_deref(),
        Some(project_b),
        "restoring an offline session from another project installed the subscriber's directory"
    );

    Ok(())
}
