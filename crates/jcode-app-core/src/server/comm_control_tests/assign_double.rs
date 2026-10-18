// Double-assignment guard: a direct assign_task naming a node that is already
// assigned and actively worked must be rejected with an error naming the
// current assignee. Incident: run_plan dispatched a node to a spawned worker,
// and 16 seconds later an explicit `assign_task task_id=<same node>` silently
// re-assigned it to a fresh worker; both edited the same files for ~7 minutes.

/// Pure guard predicate: assigned+fresh -> conflict, assigned+stale -> allow,
/// unassigned -> allow, stale/terminal statuses -> allow.
#[test]
fn active_assignment_conflict_detects_only_assigned_and_fresh_items() {
    let now = 1_000_000_u64;
    let window = 45_000_u64;
    let progress_with_heartbeat = |heartbeat: Option<u64>| crate::server::SwarmTaskProgress {
        assigned_session_id: Some("snail".to_string()),
        last_heartbeat_unix_ms: heartbeat,
        ..Default::default()
    };

    // Unassigned -> allow, regardless of progress freshness.
    assert!(
        super::active_assignment_conflict(
            "queued",
            None,
            Some(&progress_with_heartbeat(Some(now - 1_000))),
            now,
            window,
        )
        .is_none(),
        "unassigned items are always assignable"
    );

    // Assigned + fresh heartbeat -> reject, naming the assignee and age.
    let conflict = super::active_assignment_conflict(
        "running",
        Some("snail"),
        Some(&progress_with_heartbeat(Some(now - 12_000))),
        now,
        window,
    )
    .expect("assigned + fresh heartbeat must conflict");
    assert_eq!(conflict.assignee, "snail");
    assert_eq!(conflict.active_ago_ms, 12_000);
    let message = super::active_assignment_error("mem-impl-attribution", &conflict);
    assert!(
        message.contains("'mem-impl-attribution'")
            && message.contains("'snail'")
            && message.contains("12s ago")
            && message.contains("reassign"),
        "error must name the task, assignee, activity age, and takeover path: {message}"
    );

    // Assigned + queued (dispatch pending) also counts as actively worked.
    assert!(
        super::active_assignment_conflict(
            "queued",
            Some("snail"),
            Some(&progress_with_heartbeat(Some(now - 1_000))),
            now,
            window,
        )
        .is_some(),
        "a freshly queued assignment is in-flight, not reassignable"
    );

    // Assigned + heartbeat at/over the stale window -> allow (stale path).
    assert!(
        super::active_assignment_conflict(
            "running",
            Some("snail"),
            Some(&progress_with_heartbeat(Some(now - window))),
            now,
            window,
        )
        .is_none(),
        "stale assignments stay reassignable"
    );

    // Assigned with no progress record or no timestamps -> allow (treated
    // stale, mirroring refresh_swarm_task_staleness).
    assert!(super::active_assignment_conflict("running", Some("snail"), None, now, window).is_none());
    assert!(
        super::active_assignment_conflict(
            "running",
            Some("snail"),
            Some(&progress_with_heartbeat(None)),
            now,
            window,
        )
        .is_none()
    );

    // started_at / assigned_at count as activity when no heartbeat landed yet.
    let just_assigned = crate::server::SwarmTaskProgress {
        assigned_session_id: Some("snail".to_string()),
        assigned_at_unix_ms: Some(now - 5_000),
        ..Default::default()
    };
    assert!(
        super::active_assignment_conflict("queued", Some("snail"), Some(&just_assigned), now, window)
            .is_some(),
        "an assignment made moments ago is active even before its first heartbeat"
    );

    // running_stale and terminal statuses -> allow (existing recovery paths).
    for status in ["running_stale", "failed", "stopped", "crashed", "completed", "done"] {
        assert!(
            super::active_assignment_conflict(
                status,
                Some("snail"),
                Some(&progress_with_heartbeat(Some(now - 1_000))),
                now,
                window,
            )
            .is_none(),
            "status '{status}' must not trigger the double-assignment guard"
        );
    }
}

#[allow(clippy::type_complexity)]
fn double_assign_fixture(
    swarm_id: &str,
    requester: &str,
    holder: &str,
    intruder: &str,
    contested: PlanItem,
    progress: crate::server::SwarmTaskProgress,
) -> (
    Arc<RwLock<HashMap<String, Arc<Mutex<Agent>>>>>,
    Arc<RwLock<HashMap<String, SwarmMember>>>,
    Arc<RwLock<HashMap<String, HashSet<String>>>>,
    Arc<RwLock<HashMap<String, VersionedPlan>>>,
    Arc<RwLock<HashMap<String, String>>>,
) {
    let swarm_members = Arc::new(RwLock::new(HashMap::from([
        (requester.to_string(), {
            let mut member = member(requester, swarm_id, "ready");
            member.role = "coordinator".to_string();
            member
        }),
        (holder.to_string(), member(holder, swarm_id, "running")),
        (intruder.to_string(), member(intruder, swarm_id, "ready")),
    ])));
    let swarms_by_id = Arc::new(RwLock::new(HashMap::from([(
        swarm_id.to_string(),
        HashSet::from([
            requester.to_string(),
            holder.to_string(),
            intruder.to_string(),
        ]),
    )])));
    let task_id = contested.id.clone();
    let swarm_plans = Arc::new(RwLock::new(HashMap::from([(
        swarm_id.to_string(),
        VersionedPlan {
            items: vec![contested],
            version: 1,
            participants: HashSet::from([requester.to_string(), holder.to_string()]),
            task_progress: HashMap::from([(task_id, progress)]),
        },
    )])));
    let swarm_coordinators = Arc::new(RwLock::new(HashMap::from([(
        swarm_id.to_string(),
        requester.to_string(),
    )])));
    let sessions = Arc::new(RwLock::new(HashMap::new()));
    (
        sessions,
        swarm_members,
        swarms_by_id,
        swarm_plans,
        swarm_coordinators,
    )
}

fn unix_now_ms() -> u64 {
    SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .expect("clock after epoch")
        .as_millis() as u64
}

/// Live path: explicit assign_task against an assigned-and-active node is
/// rejected and the plan keeps the original assignee.
#[tokio::test]
async fn assign_task_rejects_double_assignment_of_actively_worked_task() {
    let (_env, _runtime) = RuntimeEnvGuard::new();
    let swarm_id = "swarm-double-assign";
    let (requester, holder, intruder) = ("coord", "snail", "penguin");
    let mut contested = plan_item("contested", "running", "high", &[]);
    contested.assigned_to = Some(holder.to_string());
    let (sessions, swarm_members, swarms_by_id, swarm_plans, swarm_coordinators) =
        double_assign_fixture(
            swarm_id,
            requester,
            holder,
            intruder,
            contested,
            crate::server::SwarmTaskProgress {
                assigned_session_id: Some(holder.to_string()),
                last_heartbeat_unix_ms: Some(unix_now_ms()),
                ..Default::default()
            },
        );
    sessions
        .write()
        .await
        .insert(intruder.to_string(), test_agent().await);
    let (client_tx, mut client_rx) = mpsc::unbounded_channel();
    let soft_interrupt_queues = Arc::new(RwLock::new(HashMap::new()));
    let client_connections = Arc::new(RwLock::new(HashMap::new()));
    let event_history = Arc::new(RwLock::new(VecDeque::new()));
    let event_counter = Arc::new(AtomicU64::new(1));
    let (swarm_event_tx, _swarm_event_rx) = broadcast::channel(32);
    let mutation_runtime = SwarmMutationRuntime::default();

    handle_comm_assign_task(
        91,
        requester.to_string(),
        Some(intruder.to_string()),
        Some("contested".to_string()),
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
        ServerEvent::Error { message, .. } => {
            assert!(
                message.contains("already assigned to 'snail'"),
                "error must name the current assignee: {message}"
            );
            assert!(
                message.contains("reassign"),
                "error must point at the explicit takeover path: {message}"
            );
        }
        other => panic!("expected double-assignment rejection, got {other:?}"),
    }

    let plans = swarm_plans.read().await;
    let item = plans[swarm_id]
        .items
        .iter()
        .find(|item| item.id == "contested")
        .expect("contested task exists");
    assert_eq!(
        item.assigned_to.as_deref(),
        Some(holder),
        "rejected double assignment must not steal the task"
    );
    assert_eq!(item.status, "running", "lifecycle status untouched");
}

/// Live path: an assignment whose heartbeat is far past the stale window is
/// legitimately reassignable (dead-assignee recovery must keep working).
#[tokio::test]
async fn assign_task_allows_taking_over_stale_assignment() {
    let (_env, _runtime) = RuntimeEnvGuard::new();
    let swarm_id = "swarm-double-assign-stale";
    let (requester, holder, intruder) = ("coord", "snail", "penguin");
    let mut stalled = plan_item("stalled", "queued", "high", &[]);
    stalled.assigned_to = Some(holder.to_string());
    let (sessions, swarm_members, swarms_by_id, swarm_plans, swarm_coordinators) =
        double_assign_fixture(
            swarm_id,
            requester,
            holder,
            intruder,
            stalled,
            crate::server::SwarmTaskProgress {
                assigned_session_id: Some(holder.to_string()),
                // Far beyond any configured stale window.
                last_heartbeat_unix_ms: Some(unix_now_ms().saturating_sub(3_600_000)),
                ..Default::default()
            },
        );
    sessions
        .write()
        .await
        .insert(intruder.to_string(), test_agent().await);
    let (client_tx, mut client_rx) = mpsc::unbounded_channel();
    let soft_interrupt_queues = Arc::new(RwLock::new(HashMap::new()));
    let client_connections = Arc::new(RwLock::new(HashMap::new()));
    let event_history = Arc::new(RwLock::new(VecDeque::new()));
    let event_counter = Arc::new(AtomicU64::new(1));
    let (swarm_event_tx, _swarm_event_rx) = broadcast::channel(32);
    let mutation_runtime = SwarmMutationRuntime::default();

    handle_comm_assign_task(
        92,
        requester.to_string(),
        Some(intruder.to_string()),
        Some("stalled".to_string()),
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
            task_id,
            target_session,
            ..
        } => {
            assert_eq!(task_id, "stalled");
            assert_eq!(target_session, intruder);
        }
        other => panic!("stale assignment takeover should succeed, got {other:?}"),
    }

    let plans = swarm_plans.read().await;
    let item = plans[swarm_id]
        .items
        .iter()
        .find(|item| item.id == "stalled")
        .expect("stalled task exists");
    assert_eq!(item.assigned_to.as_deref(), Some(intruder));
}

/// Takeover path: task_control reassign moves the task AND tells the displaced
/// worker to stand down (soft interrupt + DM), so it stops editing the same
/// files as its replacement.
#[tokio::test]
async fn task_control_reassign_tells_displaced_worker_to_stand_down() {
    let (_env, _runtime) = RuntimeEnvGuard::new();
    let swarm_id = "swarm-reassign-stand-down";
    let (requester, holder, intruder) = ("coord", "snail", "penguin");
    let mut contested = plan_item("contested", "running_stale", "high", &[]);
    contested.assigned_to = Some(holder.to_string());
    let (sessions, swarm_members, swarms_by_id, swarm_plans, swarm_coordinators) =
        double_assign_fixture(
            swarm_id,
            requester,
            holder,
            intruder,
            contested,
            crate::server::SwarmTaskProgress {
                assigned_session_id: Some(holder.to_string()),
                last_heartbeat_unix_ms: Some(unix_now_ms().saturating_sub(3_600_000)),
                ..Default::default()
            },
        );
    // Capture the displaced worker's server-event stream to observe the DM.
    let (holder_tx, mut holder_rx) = mpsc::unbounded_channel();
    swarm_members
        .write()
        .await
        .get_mut(holder)
        .expect("holder member")
        .event_tx = holder_tx;
    sessions
        .write()
        .await
        .insert(holder.to_string(), test_agent().await);
    sessions
        .write()
        .await
        .insert(intruder.to_string(), test_agent().await);
    let (client_tx, mut client_rx) = mpsc::unbounded_channel();
    let soft_interrupt_queues = Arc::new(RwLock::new(HashMap::new()));
    let client_connections = Arc::new(RwLock::new(HashMap::new()));
    let event_history = Arc::new(RwLock::new(VecDeque::new()));
    let event_counter = Arc::new(AtomicU64::new(1));
    let (swarm_event_tx, _swarm_event_rx) = broadcast::channel(32);
    let mutation_runtime = SwarmMutationRuntime::default();

    handle_comm_task_control(
        93,
        requester.to_string(),
        "reassign".to_string(),
        "contested".to_string(),
        Some(intruder.to_string()),
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
            task_id,
            target_session,
            ..
        } => {
            assert_eq!(task_id, "contested");
            assert_eq!(target_session, intruder);
        }
        other => panic!("reassign should re-dispatch the task, got {other:?}"),
    }

    {
        let plans = swarm_plans.read().await;
        let item = plans[swarm_id]
            .items
            .iter()
            .find(|item| item.id == "contested")
            .expect("contested task exists");
        assert_eq!(item.assigned_to.as_deref(), Some(intruder));
    }

    // The displaced worker's soft-interrupt queue carries the stand-down order.
    let stand_down = {
        let queues = soft_interrupt_queues.read().await;
        queues.get(holder).and_then(|queue| {
            queue.lock().ok().and_then(|pending| {
                pending
                    .iter()
                    .map(|msg| msg.content.clone())
                    .find(|content| content.contains("handed off"))
            })
        })
    };
    let stand_down =
        stand_down.expect("displaced worker must receive a stand-down soft interrupt");
    assert!(
        stand_down.contains("'contested'") && stand_down.contains("'penguin'"),
        "stand-down order must name the task and the new assignee: {stand_down}"
    );
    assert!(
        stand_down.contains("Stop working"),
        "stand-down order must tell the worker to stop: {stand_down}"
    );

    // And the DM notification reaches its event stream.
    let mut saw_dm = false;
    while let Ok(event) = holder_rx.try_recv() {
        if let ServerEvent::Notification { message, .. } = event
            && message.contains("handed off")
        {
            saw_dm = true;
        }
    }
    assert!(saw_dm, "displaced worker must receive a stand-down DM");
}
