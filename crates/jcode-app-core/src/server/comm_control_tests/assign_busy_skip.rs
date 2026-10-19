// Worker-stacking regression tests for auto-assignment target selection.
//
// Observed live: three `assign_task spawn_if_needed=true` calls within ~100ms
// all auto-picked the SAME reusable worker, queueing three large tasks
// serially on one agent while the swarm had spawn capacity. Two invariants
// pin the fix:
//  1. a member that already holds an incomplete plan assignment is busy, not
//     reusable, so auto-pick skips it (and the caller falls back to spawning);
//  2. the pick itself is an in-process claim, so concurrent picks that race
//     ahead of the plan write cannot select the same member twice.
//
// Included into the `comm_control::tests` module, so the parent's private
// selection helpers are in scope.

use super::{
    member_has_active_assignment, release_auto_assign_claim, select_and_claim_auto_target,
};

#[test]
fn member_with_nonzero_plan_load_is_busy() {
    let loads = HashMap::from([("busy".to_string(), 1usize)]);
    assert!(member_has_active_assignment("busy", &loads));
    assert!(!member_has_active_assignment("idle", &loads));
}

#[test]
fn auto_pick_skips_busy_worker_and_selects_idle() {
    let swarm_id = "swarm-busy-skip";
    let busy = owned_member("busy-worker", swarm_id, "ready", "coord");
    let idle = owned_member("idle-worker", swarm_id, "ready", "coord");
    // Caller-ranked order puts the busy worker first; selection must still
    // land on the idle one.
    let candidates = vec![&busy, &idle];
    let loads = HashMap::from([("busy-worker".to_string(), 2usize)]);

    let picked = select_and_claim_auto_target(swarm_id, &candidates, &loads).expect("pick idle");
    assert_eq!(picked, "idle-worker");
    release_auto_assign_claim(swarm_id, &picked);
}

#[test]
fn auto_pick_with_only_busy_workers_reports_no_target_for_spawn_fallback() {
    let swarm_id = "swarm-busy-only";
    let busy = owned_member("busy-worker", swarm_id, "ready", "coord");
    let candidates = vec![&busy];
    let loads = HashMap::from([("busy-worker".to_string(), 1usize)]);

    let err = select_and_claim_auto_target(swarm_id, &candidates, &loads).unwrap_err();
    // The leading sentence is the stable contract that spawn_if_needed /
    // run_plan match on to spawn a fresh agent instead of stacking.
    assert!(
        err.starts_with("No ready or completed swarm agents are available"),
        "unexpected error: {err}"
    );
    assert!(err.contains("Skipped 1 worker(s)"), "unexpected error: {err}");
}

#[test]
fn concurrent_auto_picks_do_not_stack_on_one_member() {
    let swarm_id = "swarm-race-claim";
    let a = owned_member("worker-a", swarm_id, "ready", "coord");
    let b = owned_member("worker-b", swarm_id, "ready", "coord");
    let candidates = vec![&a, &b];
    // No plan write has landed yet, so the plan-derived loads see everyone as
    // idle. This models the observed race: back-to-back picks resolving inside
    // the window before the first assignment is recorded.
    let loads = HashMap::new();

    let first = select_and_claim_auto_target(swarm_id, &candidates, &loads).expect("first pick");
    let second = select_and_claim_auto_target(swarm_id, &candidates, &loads).expect("second pick");
    assert_ne!(first, second, "two racing picks must not share a member");

    let third = select_and_claim_auto_target(swarm_id, &candidates, &loads).unwrap_err();
    assert!(
        third.starts_with("No ready or completed swarm agents are available"),
        "third racing pick should demand a spawn, got: {third}"
    );

    release_auto_assign_claim(swarm_id, &first);
    release_auto_assign_claim(swarm_id, &second);
}

#[test]
fn released_claim_makes_member_pickable_again() {
    let swarm_id = "swarm-claim-release";
    let a = owned_member("worker-a", swarm_id, "ready", "coord");
    let candidates = vec![&a];
    let loads = HashMap::new();

    let first = select_and_claim_auto_target(swarm_id, &candidates, &loads).expect("first pick");
    assert_eq!(first, "worker-a");
    // The assign path releases the claim once the plan write records (or
    // abandons) the assignment; with the plan still showing zero load, the
    // member is genuinely reusable again.
    release_auto_assign_claim(swarm_id, &first);
    let again = select_and_claim_auto_target(swarm_id, &candidates, &loads).expect("re-pick");
    assert_eq!(again, "worker-a");
    release_auto_assign_claim(swarm_id, &again);
}

/// Handler-level regression: when the only worker already holds an incomplete
/// assignment, an auto `assign_task` must refuse (so `spawn_if_needed` spawns)
/// instead of stacking a second task onto it.
#[tokio::test]
async fn assign_task_does_not_stack_on_busy_worker() {
    let (_env, _runtime) = RuntimeEnvGuard::new();
    let swarm_id = "swarm-no-stack";
    let requester = "coord";
    let busy_worker = "worker-busy-solo";
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
        (
            busy_worker.to_string(),
            // "ready" lifecycle status but with an incomplete plan assignment:
            // exactly the state the stacked worker was in.
            owned_member(busy_worker, swarm_id, "ready", requester),
        ),
    ])));
    let swarms_by_id = Arc::new(RwLock::new(HashMap::from([(
        swarm_id.to_string(),
        HashSet::from([requester.to_string(), busy_worker.to_string()]),
    )])));
    let mut in_flight = plan_item("in-flight", "queued", "high", &[]);
    in_flight.assigned_to = Some(busy_worker.to_string());
    let swarm_plans = Arc::new(RwLock::new(HashMap::from([(
        swarm_id.to_string(),
        VersionedPlan {
            items: vec![in_flight, plan_item("next", "queued", "high", &[])],
            version: 1,
            participants: HashSet::from([requester.to_string(), busy_worker.to_string()]),
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
        104,
        requester.to_string(),
        None,
        None,
        Some("Do not stack this onto the busy worker".to_string()),
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
                message.starts_with("No ready or completed swarm agents are available"),
                "expected the spawn-fallback trigger, got: {message}"
            );
        }
        other => panic!("expected Error refusing to stack, got {other:?}"),
    }

    // The busy worker must still hold exactly its original assignment.
    let plans = swarm_plans.read().await;
    let assigned: Vec<&str> = plans[swarm_id]
        .items
        .iter()
        .filter(|item| item.assigned_to.as_deref() == Some(busy_worker))
        .map(|item| item.id.as_str())
        .collect();
    assert_eq!(assigned, vec!["in-flight"]);
}
