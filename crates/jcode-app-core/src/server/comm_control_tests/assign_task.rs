#[tokio::test]
async fn assign_task_without_task_id_picks_highest_priority_runnable_task() {
    let (_env, _runtime) = RuntimeEnvGuard::new();
    let swarm_id = "swarm-assign";
    let requester = "coord";
    let worker = "worker";
    let (client_tx, mut client_rx) = mpsc::unbounded_channel();
    let sessions = Arc::new(crate::server::session_manager::SessionManager::from_agents(
        HashMap::new(),
    ));
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
    let swarm_plans = Arc::new(RwLock::new(HashMap::from([(
        swarm_id.to_string(),
        VersionedPlan {
            items: vec![
                plan_item("done", "completed", "high", &[]),
                plan_item("blocked", "queued", "high", &["high-ready"]),
                plan_item("low-ready", "queued", "low", &["done"]),
                plan_item("high-ready", "queued", "high", &["done"]),
            ],
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

    handle_comm_assign_task(
        77,
        requester.to_string(),
        Some(worker.to_string()),
        None,
        Some("Pick the next task".to_string()),
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

    let response = client_rx.recv().await.expect("response");
    match response {
        ServerEvent::CommAssignTaskResponse {
            id,
            task_id,
            target_session,
        } => {
            assert_eq!(id, 77);
            assert_eq!(task_id, "high-ready");
            assert_eq!(target_session, worker);
        }
        other => panic!("expected CommAssignTaskResponse, got {other:?}"),
    }

    let plans = swarm_plans.read().await;
    let plan = plans.get(swarm_id).expect("plan exists");
    let selected = plan
        .items
        .iter()
        .find(|item| item.id == "high-ready")
        .expect("selected task exists");
    assert_eq!(selected.assigned_to.as_deref(), Some(worker));
    assert_eq!(selected.status, "queued");

    let blocked = plan
        .items
        .iter()
        .find(|item| item.id == "blocked")
        .expect("blocked task exists");
    assert!(
        blocked.assigned_to.is_none(),
        "blocked task should not be auto-assigned"
    );

    let members = swarm_members.read().await;
    let worker_member = members.get(worker).expect("worker member exists");
    assert_eq!(
        worker_member.status, "queued",
        "assigned worker should stop looking completed/ready before async execution starts"
    );
}

#[tokio::test]
async fn assign_task_marks_completed_worker_queued_before_returning() {
    let (_env, _runtime) = RuntimeEnvGuard::new();
    let swarm_id = "swarm-assign-completed-worker";
    let requester = "coord";
    let worker = "worker-completed";
    let (client_tx, mut client_rx) = mpsc::unbounded_channel();
    let sessions = Arc::new(crate::server::session_manager::SessionManager::from_agents(
        HashMap::new(),
    ));
    let soft_interrupt_queues = Arc::new(RwLock::new(HashMap::new()));
    let client_connections = Arc::new(RwLock::new(HashMap::new()));
    let swarm_members = Arc::new(RwLock::new(HashMap::from([
        (requester.to_string(), {
            let mut member = member(requester, swarm_id, "ready");
            member.role = "coordinator".to_string();
            member
        }),
        (worker.to_string(), member(worker, swarm_id, "completed")),
    ])));
    let swarms_by_id = Arc::new(RwLock::new(HashMap::from([(
        swarm_id.to_string(),
        HashSet::from([requester.to_string(), worker.to_string()]),
    )])));
    let swarm_plans = Arc::new(RwLock::new(HashMap::from([(
        swarm_id.to_string(),
        VersionedPlan {
            items: vec![plan_item("next", "queued", "high", &[])],
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

    handle_comm_assign_task(
        78,
        requester.to_string(),
        Some(worker.to_string()),
        Some("next".to_string()),
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
        ServerEvent::CommAssignTaskResponse {
            id,
            task_id,
            target_session,
        } => {
            assert_eq!(id, 78);
            assert_eq!(task_id, "next");
            assert_eq!(target_session, worker);
        }
        other => panic!("expected CommAssignTaskResponse, got {other:?}"),
    }

    let members = swarm_members.read().await;
    let worker_member = members.get(worker).expect("worker member exists");
    assert_eq!(worker_member.status, "queued");
    assert!(
        worker_member
            .detail
            .as_deref()
            .is_some_and(|detail| detail.contains("task next")),
        "queued member should include the assigned task in its detail"
    );
}

#[tokio::test]
async fn assigned_turn_with_open_lane_todo_does_not_publish_completion() {
    check_assigned_lane_outcome("in_progress", "failed", "failed").await;
}

#[tokio::test]
async fn assigned_turn_with_finished_lane_todo_completes() {
    check_assigned_lane_outcome("completed", "done", "completed").await;
}

async fn check_assigned_lane_outcome(todo_status: &str, plan_status: &str, member_status: &str) {
    let (_guard, _runtime) = RuntimeEnvGuard::new();
    let worker = format!("lane_worker_{}", uuid::Uuid::new_v4().simple());
    let coordinator = format!("lane_owner_{}", uuid::Uuid::new_v4().simple());
    let swarm = "open-lane-at-turn-end";
    let lane = "lane:1:parser";
    let todo = crate::todo::TodoItem {
        id: "parse".into(),
        content: "Finish the parser".into(),
        status: todo_status.into(),
        stage: Some(1),
        group: Some("parser".into()),
        ..Default::default()
    };
    crate::todo::save_todos(&coordinator, &[todo]).unwrap();
    crate::todo::link_worker_lane(&coordinator, &worker, lane).unwrap();
    let mut item = plan_item(lane, "queued", "high", &[]);
    item.assigned_to = Some(worker.clone());
    let (_, members, swarms, plans, coordinators) = double_assign_fixture(
        swarm,
        &coordinator,
        &worker,
        "idle",
        item,
        Default::default(),
    );
    let (tx, mut rx) = mpsc::unbounded_channel();
    members.write().await.get_mut(&worker).unwrap().event_tx = tx;
    let (events, _) = broadcast::channel(32);
    super::spawn_assigned_task_run(
        named_assignment_test_agent(&worker).await,
        worker.clone(),
        swarm.into(),
        lane.into(),
        "Finish the parser".into(),
        members.clone(),
        swarms,
        plans.clone(),
        coordinators,
        Arc::new(RwLock::new(VecDeque::new())),
        Arc::new(AtomicU64::new(0)),
        events,
    );
    tokio::time::timeout(Duration::from_secs(10), async {
        loop {
            if let ServerEvent::TurnEvent { event, .. } = rx.recv().await.expect("worker event") {
                match *event {
                    ServerEvent::Done { .. } => {
                        assert_eq!(member_status, "completed");
                        break;
                    }
                    ServerEvent::Error { message, .. } => {
                        assert_eq!(member_status, "failed");
                        assert!(message.contains("Finish the parser"), "{message}");
                        break;
                    }
                    _ => {}
                }
            }
        }
    })
    .await
    .expect("worker should publish a terminal event");
    assert_eq!(plans.read().await[swarm].items[0].status, plan_status);
    assert_eq!(members.read().await[&worker].status, member_status);
    assert_eq!(
        crate::todo::load_todos(&coordinator).unwrap()[0].status,
        todo_status
    );
}
