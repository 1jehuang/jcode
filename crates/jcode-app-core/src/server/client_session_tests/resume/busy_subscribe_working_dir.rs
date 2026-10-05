// A busy session must not be re-keyed to the attaching client's project.
//
// One daemon serves sessions for many projects. `handle_subscribe` reads the
// session's working directory twice, once to decide whether the reported
// directory may replace it and once to re-key the swarm member, and it reads it
// through `agent.try_lock()`. `try_lock` failing is not the same fact as the
// session having no directory: it also means the agent is mid-turn. That
// collapse is invisible in the idle case, which is why the existing
// cross-project tests pass, and it is the ordinary case in production because
// the desktop attaches to live sessions while they work.
//
// Under contention the directory fell back to the *client's* report while
// `apply_or_defer_subscribe_working_dir` still refused to move the agent, so the
// swarm member and project-local MCP discovery landed in the attaching client's
// project while the session's own tools stayed put.


/// A live agent whose session is persisted in `working_dir`, so the lock-free
/// read has the same answer the locked read would.
async fn busy_agent_in_project(
    provider: Arc<dyn Provider>,
    registry: Registry,
    session_id: &str,
    working_dir: &str,
) -> Arc<Mutex<Agent>> {
    let mut session = Session::create_with_id(session_id.to_string(), None, None);
    session.working_dir = Some(working_dir.to_string());
    // Persisted so the lock-free read sees it. `save_prepared` rather than
    // `save`: this session has no visible message, and plain `save`
    // deliberately skips writing that case.
    session.save_prepared().expect("persist busy session");
    Arc::new(Mutex::new(Agent::new_with_session(
        provider,
        registry,
        session,
        None,
    )))
}


#[tokio::test]
async fn subscribe_to_a_busy_session_keeps_its_own_project() -> Result<()> {
    let _guard = crate::storage::lock_test_env();
    let (_runtime, prev_runtime) = setup_runtime_dir()?;
    let _ = prev_runtime;

    let target_session_id = "session_busy_cross_project_target";
    let subscriber_session_id = "session_busy_cross_project_subscriber";
    let project_b = "/home/tester/work/project-b";
    let project_a = "/home/tester/work/project-a";

    let provider: Arc<dyn Provider> = Arc::new(MockProvider);
    let target_registry = Registry::new(provider.clone()).await;
    let target_agent = busy_agent_in_project(
        provider.clone(),
        target_registry,
        target_session_id,
        project_b,
    )
    .await;
    let new_registry = Registry::new(provider.clone()).await;

    let (client_event_tx, _client_event_rx) = mpsc::unbounded_channel::<ServerEvent>();
    let (swarm_event_tx, _swarm_event_rx) = broadcast::channel::<SwarmEvent>(8);
    let event_history = Arc::new(RwLock::new(VecDeque::<SwarmEvent>::new()));
    let event_counter = Arc::new(std::sync::atomic::AtomicU64::new(0));
    let swarm_members = Arc::new(RwLock::new(HashMap::new()));
    let swarms_by_id = Arc::new(RwLock::new(HashMap::<String, HashSet<String>>::new()));
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
    let mcp_pool = Arc::new(crate::mcp::SharedMcpPool::from_default_config());
    let mut client_selfdev = false;

    // Checked before the lock is taken: `tokio::sync::Mutex` is not reentrant, so
    // a second `lock().await` while the busy guard is alive would block forever.
    // The agent is created with `project_b` above, and nothing below moves it.
    assert_eq!(
        Session::load(target_session_id)
            .expect("target session loadable")
            .working_dir
            .as_deref(),
        Some(project_b),
        "precondition: the busy session is persisted in its own project"
    );

    // Hold the lock the whole time, so every `try_lock` in `handle_subscribe`
    // reports contention. This is what a session mid-turn looks like.
    let _busy_guard = target_agent.lock().await;

    handle_subscribe(
        92,
        Some(project_a.to_string()),
        None,
        false,
        &mut client_selfdev,
        target_session_id,
        "conn_busy",
        &None,
        &target_agent,
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

    let member_dir = swarm_members
        .read()
        .await
        .get(target_session_id)
        .and_then(|member| member.working_dir.as_ref())
        .map(|path| path.to_string_lossy().into_owned());
    assert_eq!(
        member_dir.as_deref(),
        Some(project_b),
        "a busy session's swarm member was re-keyed to the attaching client's project"
    );

    Ok(())
}

/// The session's own directory wins over the client's, and that answer does not
/// depend on the agent lock being free.
///
/// This is the other half of the same distinction, and the direction that is
/// actually reachable. `handle_subscribe` already holds the live agent's
/// directory as the answer for a busy agent, so "the stub has no directory, so
/// the report wins" holds whichever way `current_working_dir` resolves. What the
/// lock-free stub read decides is the opposite: with the session's directory
/// known and the report disagreeing, the session must win rather than the
/// attaching client's project.
#[tokio::test]
async fn subscribe_to_a_busy_session_keeps_its_own_dir_over_a_differing_report() -> Result<()> {
    let _guard = crate::storage::lock_test_env();
    let (_runtime, prev_runtime) = setup_runtime_dir()?;
    let _ = prev_runtime;

    let target_session_id = "session_busy_report_disagrees";
    let session_dir = "/home/tester/work/project-b";
    let reported_dir = "/home/tester/work/project-a";

    let provider: Arc<dyn Provider> = Arc::new(MockProvider);
    let registry = Registry::new(provider.clone()).await;
    let target_agent = busy_agent_in_project(
        provider.clone(),
        registry.clone(),
        target_session_id,
        session_dir,
    )
    .await;

    let (client_event_tx, _client_event_rx) = mpsc::unbounded_channel::<ServerEvent>();
    let (swarm_event_tx, _swarm_event_rx) = broadcast::channel::<SwarmEvent>(8);
    let event_history = Arc::new(RwLock::new(VecDeque::<SwarmEvent>::new()));
    let event_counter = Arc::new(std::sync::atomic::AtomicU64::new(0));
    let swarm_members = Arc::new(RwLock::new(HashMap::new()));
    let swarms_by_id = Arc::new(RwLock::new(HashMap::<String, HashSet<String>>::new()));
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
    let mcp_pool = Arc::new(crate::mcp::SharedMcpPool::from_default_config());
    let mut client_selfdev = false;

    let _busy_guard = target_agent.lock().await;

    handle_subscribe(
        93,
        Some(reported_dir.to_string()),
        None,
        false,
        &mut client_selfdev,
        target_session_id,
        "conn_busy_report",
        &None,
        &target_agent,
        &registry,
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

    let member_dir = swarm_members
        .read()
        .await
        .get(target_session_id)
        .and_then(|member| member.working_dir.as_ref())
        .map(|path| path.to_string_lossy().into_owned());
    assert_eq!(
        member_dir.as_deref(),
        Some(session_dir),
        "a busy session's own directory must beat the attaching client's report"
    );

    Ok(())
}
