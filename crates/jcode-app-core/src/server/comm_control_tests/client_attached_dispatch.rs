// Regression pins for the client-attached (visible, live-client) assignee path.
//
// `handle_comm_assign_task` deliberately skips `spawn_assigned_task_run` when the
// target session has a live client connection (the client owns the turn loop).
// The assignment is delivered as a soft interrupt + DM instead, and the plan
// item stays `queued`. Critically, there is NO server-side turn-end hook for
// this path: when the client-driven turn finishes, `handle_client` /
// `CommReport` only update *member* status (`ready`/`failed`), never the plan
// item. Terminalizing the node is the assignee's own job (`complete_node`,
// which `claim_queued_node_for_actor` allows from `queued`), or the
// coordinator's via task control / reassignment. These tests document that
// contract so an accidental removal of the live-client skip (double-driving a
// session) or a silent behavior change in the no-auto-flip gap is caught.

#[tokio::test]
async fn assign_task_to_client_attached_session_skips_server_side_run() {
    let (_env, _runtime) = RuntimeEnvGuard::new();
    let swarm_id = "swarm-client-attached";
    let requester = "coord";
    let worker = "worker-attached";
    let (client_tx, mut client_rx) = mpsc::unbounded_channel();

    // The worker has a live server-side agent AND a live client connection:
    // the agent exists, so the only reason to skip the server-side run is the
    // client attachment.
    let worker_agent = test_agent().await;
    let sessions = Arc::new(RwLock::new(HashMap::from([(
        worker.to_string(),
        Arc::clone(&worker_agent),
    )])));
    let soft_interrupt_queues = Arc::new(RwLock::new(HashMap::new()));

    let (disconnect_tx, _disconnect_rx) = mpsc::unbounded_channel();
    let client_connections = Arc::new(RwLock::new(HashMap::from([(
        "conn-1".to_string(),
        crate::server::ClientConnectionInfo {
            client_id: "conn-1".to_string(),
            session_id: worker.to_string(),
            client_instance_id: None,
            debug_client_id: None,
            connected_at: Instant::now(),
            last_seen: Instant::now(),
            is_processing: false,
            current_tool_name: None,
            terminal_env: Vec::new(),
            disconnect_tx,
        },
    )])));

    let swarm_members = Arc::new(RwLock::new(HashMap::from([
        (requester.to_string(), {
            let mut member = member(requester, swarm_id, "ready");
            member.role = "coordinator".to_string();
            member
        }),
        // Owned visible worker: drivable for auto-pick, but client-attached.
        (
            worker.to_string(),
            owned_member(worker, swarm_id, "ready", requester),
        ),
    ])));
    let swarms_by_id = Arc::new(RwLock::new(HashMap::from([(
        swarm_id.to_string(),
        HashSet::from([requester.to_string(), worker.to_string()]),
    )])));
    let swarm_plans = Arc::new(RwLock::new(HashMap::from([(
        swarm_id.to_string(),
        VersionedPlan {
            items: vec![plan_item("solo", "queued", "high", &[])],
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
        91,
        requester.to_string(),
        Some(worker.to_string()),
        Some("solo".to_string()),
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
            assert_eq!(id, 91);
            assert_eq!(task_id, "solo");
            assert_eq!(target_session, worker);
        }
        other => panic!("expected CommAssignTaskResponse, got {other:?}"),
    }

    // Give any (incorrectly) spawned server-side run a chance to flip state.
    tokio::time::sleep(Duration::from_millis(50)).await;

    {
        let plans = swarm_plans.read().await;
        let item = &plans[swarm_id].items[0];
        assert_eq!(
            item.status, "queued",
            "client-attached dispatch must not start a server-side run (no running/done flip)"
        );
        assert_eq!(item.assigned_to.as_deref(), Some(worker));
    }

    // The assignment was handed to the live client as a soft interrupt.
    assert!(
        worker_agent.lock().await.has_soft_interrupts(),
        "assignment must be queued as a soft interrupt for the live client to inject"
    );

    // Now simulate the client-driven turn finishing: handle_client's done path
    // only calls update_member_status_with_report(..., "ready", ...). Pin that
    // this does NOT terminalize the plan item — there is no turn-end hook for
    // client-attached assignees; the node must be closed by the assignee
    // (complete_node) or the coordinator.
    crate::server::swarm::update_member_status_with_report(
        worker,
        "ready",
        None,
        Some("finished my turn".to_string()),
        &swarm_members,
        &swarms_by_id,
        Some(&event_history),
        Some(&event_counter),
        Some(&swarm_event_tx),
    )
    .await;

    {
        let plans = swarm_plans.read().await;
        let item = &plans[swarm_id].items[0];
        assert_eq!(
            item.status, "queued",
            "member-status turn-end flip must not (silently) complete the plan item; \
             if you add an auto-complete hook for client-attached assignees, update \
             run_plan's strand handling and this pin together"
        );
    }

    // Consequence check: the plan is now stranded from run_plan's perspective —
    // the task is runnable-but-assigned (so assign_next skips it) and the member
    // is no longer in flight. Pin the ingredients of that stall so the contract
    // stays visible.
    {
        let plans = swarm_plans.read().await;
        assert!(
            crate::plan::next_unassigned_runnable_item_id(&plans[swarm_id]).is_none(),
            "assigned queued task must not be offered to assign_next"
        );
        let summary = crate::plan::summarize_plan_graph(&plans[swarm_id].items);
        assert!(
            summary.terminal_ids.is_empty(),
            "plan must not be terminal while the assigned task is still queued"
        );
        let members = swarm_members.read().await;
        assert_eq!(members[worker].status, "ready");
    }
}
