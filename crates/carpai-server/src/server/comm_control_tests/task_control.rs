#[tokio::test]
async fn task_control_wake_returns_structured_response_with_plan_summary() {
    let (_env, _runtime) = RuntimeEnvGuard::new();
    let swarm_id = "swarm-task-control";
    let requester = "coord";
    let worker = "worker";
    let (client_tx, mut client_rx) = mpsc::unbounded_channel();
    let worker_agent = test_agent().await;
    let sessions = Arc::new(RwLock::new(HashMap::from([(
        worker.to_string(),
        worker_agent,
    )])));
    let soft_interrupt_queues = Arc::new(RwLock::new(HashMap::new()));
    let client_connections = Arc::new(RwLock::new(HashMap::new()));
    let swarm_members = Arc::new(RwLock::new(HashMap::from([
        (requester.to_string(), {
            let mut member = member(requester, swarm_id, "ready");
            member.role = "coordinator".to_string();
            member
        }),
        (worker.to_string(), member(worker, swarm_id, "ready")),
    ])));
    let swarms_by_id = Arc::new(RwLock::new(HashMap::from([(
        swarm_id.to_string(),
        HashSet::from([requester.to_string(), worker.to_string()]),
    )])));
    let mut assigned = plan_item("active-task", "queued", "high", &[]);
    assigned.assigned_to = Some(worker.to_string());
    let swarm_plans = Arc::new(RwLock::new(HashMap::from([(
        swarm_id.to_string(),
        VersionedPlan {
            items: vec![assigned, plan_item("next", "queued", "high", &[])],
            version: 1,
            participants: HashSet::from([requester.to_string(), worker.to_string()]),
            task_progress: HashMap::new(),
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

    handle_comm_task_control(
        101,
        requester.to_string(),
        "wake".to_string(),
        "active-task".to_string(),
        Some(worker.to_string()),
        Some("continue".to_string()),
        &client_tx,
        &sessions,
        &soft_interrupt_queues,
        &client_connections,
        &swarm_members,
        &swarms_by_id,
        &swarm_plans,
        &swarm_coordinators,
        &event_history,
        &event_counter,
        &swarm_event_tx,
        &mutation_runtime,
    )
    .await;

    match client_rx.recv().await.expect("response") {
        ServerEvent::CommTaskControlResponse {
            id,
            action,
            task_id,
            target_session,
            status,
            summary,
        } => {
            assert_eq!(id, 101);
            assert_eq!(action, "wake");
            assert_eq!(task_id, "active-task");
            assert_eq!(target_session.as_deref(), Some(worker));
            assert_eq!(status, "running");
            assert_eq!(summary.item_count, 2);
            assert!(summary.ready_ids.contains(&"next".to_string()));
        }
        other => panic!("expected CommTaskControlResponse, got {other:?}"),
    }
}

#[tokio::test]
async fn task_control_resume_without_task_id_uses_unique_target_assignment() {
    let (_env, _runtime) = RuntimeEnvGuard::new();
    let swarm_id = "swarm-task-control-target";
    let requester = "coord";
    let worker = "worker";
    let (client_tx, mut client_rx) = mpsc::unbounded_channel();
    let worker_agent = test_agent().await;
    let sessions = Arc::new(RwLock::new(HashMap::from([(
        worker.to_string(),
        worker_agent,
    )])));
    let soft_interrupt_queues = Arc::new(RwLock::new(HashMap::new()));
    let client_connections = Arc::new(RwLock::new(HashMap::new()));
    let swarm_members = Arc::new(RwLock::new(HashMap::from([
        (requester.to_string(), {
            let mut member = member(requester, swarm_id, "ready");
            member.role = "coordinator".to_string();
            member
        }),
        (worker.to_string(), member(worker, swarm_id, "stopped")),
    ])));
    let swarms_by_id = Arc::new(RwLock::new(HashMap::from([(
        swarm_id.to_string(),
        HashSet::from([requester.to_string(), worker.to_string()]),
    )])));
    let mut assigned = plan_item("resume-me", "queued", "high", &[]);
    assigned.assigned_to = Some(worker.to_string());
    let swarm_plans = Arc::new(RwLock::new(HashMap::from([(
        swarm_id.to_string(),
        VersionedPlan {
            items: vec![assigned],
            version: 1,
            participants: HashSet::from([requester.to_string(), worker.to_string()]),
            task_progress: HashMap::new(),
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

    handle_comm_task_control(
        102,
        requester.to_string(),
        "resume".to_string(),
        String::new(),
        Some(worker.to_string()),
        None,
        &client_tx,
        &sessions,
        &soft_interrupt_queues,
        &client_connections,
        &swarm_members,
        &swarms_by_id,
        &swarm_plans,
        &swarm_coordinators,
        &event_history,
        &event_counter,
        &swarm_event_tx,
        &mutation_runtime,
    )
    .await;

    match client_rx.recv().await.expect("response") {
        ServerEvent::CommTaskControlResponse {
            id,
            action,
            task_id,
            target_session,
            status,
            ..
        } => {
            assert_eq!(id, 102);
            assert_eq!(action, "resume");
            assert_eq!(task_id, "resume-me");
            assert_eq!(target_session.as_deref(), Some(worker));
            assert_eq!(status, "running");
        }
        other => panic!("expected CommTaskControlResponse, got {other:?}"),
    }
}

#[tokio::test]
async fn task_control_without_task_id_rejects_ambiguous_target_assignments() {
    let (_env, _runtime) = RuntimeEnvGuard::new();
    let swarm_id = "swarm-task-control-ambiguous";
    let requester = "coord";
    let worker = "worker";
    let (client_tx, mut client_rx) = mpsc::unbounded_channel();
    let sessions = Arc::new(RwLock::new(HashMap::new()));
    let soft_interrupt_queues = Arc::new(RwLock::new(HashMap::new()));
    let client_connections = Arc::new(RwLock::new(HashMap::new()));
    let swarm_members = Arc::new(RwLock::new(HashMap::from([
        (requester.to_string(), {
            let mut member = member(requester, swarm_id, "ready");
            member.role = "coordinator".to_string();
            member
        }),
        (worker.to_string(), member(worker, swarm_id, "stopped")),
    ])));
    let swarms_by_id = Arc::new(RwLock::new(HashMap::from([(
        swarm_id.to_string(),
        HashSet::from([requester.to_string(), worker.to_string()]),
    )])));
    let mut first = plan_item("first", "queued", "high", &[]);
    first.assigned_to = Some(worker.to_string());
    let mut second = plan_item("second", "queued", "high", &[]);
    second.assigned_to = Some(worker.to_string());
    let swarm_plans = Arc::new(RwLock::new(HashMap::from([(
        swarm_id.to_string(),
        VersionedPlan {
            items: vec![first, second],
            version: 1,
            participants: HashSet::from([requester.to_string(), worker.to_string()]),
            task_progress: HashMap::new(),
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

    handle_comm_task_control(
        103,
        requester.to_string(),
        "resume".to_string(),
        String::new(),
        Some(worker.to_string()),
        None,
        &client_tx,
        &sessions,
        &soft_interrupt_queues,
        &client_connections,
        &swarm_members,
        &swarms_by_id,
        &swarm_plans,
        &swarm_coordinators,
        &event_history,
        &event_counter,
        &swarm_event_tx,
        &mutation_runtime,
    )
    .await;

    match client_rx.recv().await.expect("response") {
        ServerEvent::Error { id, message, .. } => {
            assert_eq!(id, 103);
            assert!(message.contains("Multiple tasks assigned"), "{message}");
            assert!(message.contains("first"), "{message}");
            assert!(message.contains("second"), "{message}");
        }
        other => panic!("expected Error, got {other:?}"),
    }
}
