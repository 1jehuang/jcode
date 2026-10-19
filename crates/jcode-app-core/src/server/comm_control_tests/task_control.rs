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

/// Regression: resuming a plain-'running' task whose agent is busy must be
/// rejected BEFORE any plan mutation. The old ordering requeued the live task
/// first (flipping it to 'queued' and rewriting its progress record) and only
/// then discovered the agent was busy, so a rejected resume still mangled the
/// running task's state and run history.
#[tokio::test]
async fn task_control_resume_busy_agent_rejects_without_mutating_plan() {
    let (_env, _runtime) = RuntimeEnvGuard::new();
    let swarm_id = "swarm-task-control-busy";
    let requester = "coord";
    let worker = "worker";
    let (client_tx, mut client_rx) = mpsc::unbounded_channel();
    let worker_agent = test_agent().await;
    let sessions = Arc::new(RwLock::new(HashMap::from([(
        worker.to_string(),
        Arc::clone(&worker_agent),
    )])));
    let soft_interrupt_queues = Arc::new(RwLock::new(HashMap::new()));
    let client_connections = Arc::new(RwLock::new(HashMap::new()));
    let swarm_members = Arc::new(RwLock::new(HashMap::from([
        (requester.to_string(), {
            let mut member = member(requester, swarm_id, "ready");
            member.role = "coordinator".to_string();
            member
        }),
        (worker.to_string(), member(worker, swarm_id, "running")),
    ])));
    let swarms_by_id = Arc::new(RwLock::new(HashMap::from([(
        swarm_id.to_string(),
        HashSet::from([requester.to_string(), worker.to_string()]),
    )])));
    let mut assigned = plan_item("busy-task", "running", "high", &[]);
    assigned.assigned_to = Some(worker.to_string());
    let prior_progress = crate::server::SwarmTaskProgress {
        assigned_session_id: Some(worker.to_string()),
        assignment_summary: Some("original assignment".to_string()),
        assigned_at_unix_ms: Some(1_000),
        started_at_unix_ms: Some(2_000),
        last_heartbeat_unix_ms: Some(3_000),
        heartbeat_count: Some(7),
        checkpoint_count: Some(2),
        checkpoint_summary: Some("halfway".to_string()),
        ..crate::server::SwarmTaskProgress::default()
    };
    let swarm_plans = Arc::new(RwLock::new(HashMap::from([(
        swarm_id.to_string(),
        VersionedPlan {
            items: vec![assigned],
            version: 1,
            participants: HashSet::from([requester.to_string(), worker.to_string()]),
            task_progress: HashMap::from([("busy-task".to_string(), prior_progress.clone())]),
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

    // Hold the agent lock so the resume path sees the worker as busy.
    let _busy_guard = worker_agent.lock().await;

    handle_comm_task_control(
        104,
        requester.to_string(),
        "resume".to_string(),
        "busy-task".to_string(),
        None,
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
            assert_eq!(id, 104);
            assert!(message.contains("currently busy"), "{message}");
        }
        other => panic!("expected busy Error, got {other:?}"),
    }

    let plans = swarm_plans.read().await;
    let plan = plans.get(swarm_id).expect("plan exists");
    let item = plan
        .items
        .iter()
        .find(|item| item.id == "busy-task")
        .expect("task exists");
    assert_eq!(
        item.status, "running",
        "rejected resume must not flip a live task back to queued"
    );
    assert_eq!(plan.version, 1, "rejected resume must not bump the plan");
    assert_eq!(
        plan.task_progress.get("busy-task"),
        Some(&prior_progress),
        "rejected resume must not touch the task's progress record"
    );
}

/// Regression: requeueing an existing assignment (resume of a running/stale
/// task) must preserve the prior run's history instead of replacing the
/// progress record. Wiping started_at/heartbeats/checkpoints blinded
/// staleness monitors and salvage flows to everything the previous run did.
#[tokio::test]
async fn requeue_existing_assignment_preserves_prior_progress_history() {
    let (_env, _runtime) = RuntimeEnvGuard::new();
    let swarm_id = "swarm-requeue-preserve";
    let requester = "coord";
    let worker = "worker";
    let mut assigned = plan_item("requeue-me", "running_stale", "high", &[]);
    assigned.assigned_to = Some(worker.to_string());
    let swarm_plans = Arc::new(RwLock::new(HashMap::from([(
        swarm_id.to_string(),
        VersionedPlan {
            items: vec![assigned],
            version: 1,
            participants: HashSet::from([requester.to_string(), worker.to_string()]),
            task_progress: HashMap::from([(
                "requeue-me".to_string(),
                crate::server::SwarmTaskProgress {
                    assigned_session_id: Some(worker.to_string()),
                    assignment_summary: Some("original assignment".to_string()),
                    assigned_at_unix_ms: Some(1_000),
                    started_at_unix_ms: Some(2_000),
                    last_heartbeat_unix_ms: Some(3_000),
                    last_detail: Some("was working".to_string()),
                    last_checkpoint_unix_ms: Some(3_500),
                    checkpoint_summary: Some("halfway".to_string()),
                    heartbeat_count: Some(7),
                    checkpoint_count: Some(2),
                    stale_since_unix_ms: Some(4_000),
                    completed_at_unix_ms: None,
                    dead_assignee_reclaims: None,
                },
            )]),
        },
    )])));

    let result = super::requeue_existing_assignment(
        swarm_id,
        requester,
        worker,
        "requeue-me",
        "resume this work".to_string(),
        &swarm_plans,
    )
    .await;
    assert!(result.is_some(), "requeue should succeed");

    let plans = swarm_plans.read().await;
    let plan = plans.get(swarm_id).expect("plan exists");
    let item = plan
        .items
        .iter()
        .find(|item| item.id == "requeue-me")
        .expect("task exists");
    assert_eq!(item.status, "queued");
    let progress = plan
        .task_progress
        .get("requeue-me")
        .expect("progress exists");
    assert_eq!(
        progress.started_at_unix_ms,
        Some(2_000),
        "prior run's start time must survive the requeue"
    );
    assert_eq!(progress.last_heartbeat_unix_ms, Some(3_000));
    assert_eq!(progress.heartbeat_count, Some(7));
    assert_eq!(progress.checkpoint_count, Some(2));
    assert_eq!(progress.checkpoint_summary.as_deref(), Some("halfway"));
    assert_eq!(progress.last_detail.as_deref(), Some("was working"));
    assert_eq!(
        progress.stale_since_unix_ms, None,
        "requeued task is no longer stale"
    );
    assert_eq!(
        progress.assignment_summary.as_deref(),
        Some("resume this work"),
        "assignment-scoped fields refresh for the new attempt"
    );
    assert!(
        progress.assigned_at_unix_ms.unwrap_or(0) > 1_000,
        "assigned_at refreshes for the new attempt"
    );
}

/// Regression: an identical coordinator retry issued shortly after a
/// previous retry succeeded must actually re-dispatch the task. The durable
/// mutation layer used to replay the persisted assign_task success for any
/// identical request within the final-state TTL (30s), so a worker that
/// failed quickly made the second retry a silent no-op: the coordinator got
/// a success response while the task stayed failed.
#[tokio::test]
async fn task_control_retry_re_dispatches_after_recent_identical_retry() {
    let (_env, _runtime) = RuntimeEnvGuard::new();
    let swarm_id = "swarm-retry-replay";
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
    let mut assigned = plan_item("flaky-task", "failed", "high", &[]);
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

    let run_retry = |id: u64| {
        let requester = requester.to_string();
        let client_tx = client_tx.clone();
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
        let mutation_runtime = mutation_runtime.clone();
        async move {
            handle_comm_task_control(
                id,
                requester,
                "retry".to_string(),
                "flaky-task".to_string(),
                None,
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
        }
    };

    let wait_for_status_leaving_failed = || {
        let swarm_plans = Arc::clone(&swarm_plans);
        async move {
            for _ in 0..200 {
                {
                    let plans = swarm_plans.read().await;
                    let status = plans
                        .get(swarm_id)
                        .and_then(|plan| plan.items.iter().find(|item| item.id == "flaky-task"))
                        .map(|item| item.status.clone())
                        .unwrap_or_default();
                    if status != "failed" {
                        return status;
                    }
                }
                tokio::time::sleep(Duration::from_millis(10)).await;
            }
            "failed".to_string()
        }
    };

    // First retry dispatches the task.
    run_retry(201).await;
    match client_rx.recv().await.expect("first retry response") {
        ServerEvent::CommAssignTaskResponse { id, task_id, .. } => {
            assert_eq!(id, 201);
            assert_eq!(task_id, "flaky-task");
        }
        other => panic!("expected CommAssignTaskResponse, got {other:?}"),
    }
    let status = wait_for_status_leaving_failed().await;
    assert_ne!(status, "failed", "first retry should dispatch the task");

    // Simulate the worker failing quickly, well within the final-state TTL.
    {
        let mut plans = swarm_plans.write().await;
        let plan = plans.get_mut(swarm_id).expect("plan exists");
        let item = plan
            .items
            .iter_mut()
            .find(|item| item.id == "flaky-task")
            .expect("task exists");
        item.status = "failed".to_string();
    }

    // Identical second retry must re-dispatch instead of replaying the
    // persisted success from the first retry.
    run_retry(202).await;
    match client_rx.recv().await.expect("second retry response") {
        ServerEvent::CommAssignTaskResponse { id, task_id, .. } => {
            assert_eq!(id, 202);
            assert_eq!(task_id, "flaky-task");
        }
        other => panic!("expected CommAssignTaskResponse, got {other:?}"),
    }
    let status = wait_for_status_leaving_failed().await;
    assert_ne!(
        status, "failed",
        "second identical retry must actually re-dispatch the task"
    );
}

/// Regression: `swarm wake` on a worker spawned without a plan used to fail
/// with "No swarm plan exists for this swarm", leaving the coordinator with no
/// way to rouse an idle worker. It must wake the session directly instead.
#[tokio::test]
async fn task_control_wake_without_plan_wakes_target_session_by_friendly_name() {
    let (_env, _runtime) = RuntimeEnvGuard::new();
    let swarm_id = "swarm-no-plan";
    let requester = "coord";
    let worker = "worker-session";
    let (client_tx, mut client_rx) = mpsc::unbounded_channel();
    let worker_agent = test_agent().await;
    // Hold the agent so it reads as busy: the wake must queue, not error.
    let _busy = worker_agent.clone().lock_owned().await;
    let sessions = Arc::new(RwLock::new(HashMap::from([(
        worker.to_string(),
        worker_agent,
    )])));
    let soft_interrupt_queues = Arc::new(RwLock::new(HashMap::new()));
    let client_connections = Arc::new(RwLock::new(HashMap::new()));
    let swarm_members = Arc::new(RwLock::new(HashMap::from([
        (requester.to_string(), member(requester, swarm_id, "ready")),
        (worker.to_string(), {
            let mut member = member(worker, swarm_id, "ready");
            member.friendly_name = Some("mizaru".to_string());
            member.is_headless = true;
            member
        }),
    ])));
    let swarms_by_id = Arc::new(RwLock::new(HashMap::from([(
        swarm_id.to_string(),
        HashSet::from([requester.to_string(), worker.to_string()]),
    )])));
    let swarm_plans = Arc::new(RwLock::new(HashMap::new()));
    let swarm_coordinators = Arc::new(RwLock::new(HashMap::from([(
        swarm_id.to_string(),
        requester.to_string(),
    )])));
    let event_history = Arc::new(RwLock::new(VecDeque::new()));
    let event_counter = Arc::new(AtomicU64::new(1));
    let (swarm_event_tx, _swarm_event_rx) = broadcast::channel(32);
    let mutation_runtime = SwarmMutationRuntime::default();

    handle_comm_task_control(
        111,
        requester.to_string(),
        "wake".to_string(),
        String::new(),
        Some("mizaru".to_string()),
        Some("keep going".to_string()),
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
            target_session,
            status,
            ..
        } => {
            assert_eq!(id, 111);
            assert_eq!(action, "wake");
            assert_eq!(target_session.as_deref(), Some(worker));
            assert_eq!(status, "queued");
        }
        other => panic!("expected CommTaskControlResponse, got {other:?}"),
    }
}
