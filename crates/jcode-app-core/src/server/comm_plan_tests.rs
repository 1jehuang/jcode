//! Tests for plan proposal/approval cycle validation.
//!
//! `dag::seed`/`expand` already reject cyclic task graphs, but `propose_plan`
//! (coordinator direct update) and `approve_plan` used to write `plan.items`
//! verbatim. A cycle entering there parks its nodes in `blocked_ids` forever
//! and silently wedges dependent work, so these handlers must validate
//! acyclicity too.

use super::{handle_comm_approve_plan, handle_comm_propose_plan, plan_cycle_error};
use crate::plan::PlanItem;
use crate::protocol::ServerEvent;
use crate::server::{SharedContext, SwarmEvent, SwarmMember, SwarmMutationRuntime, VersionedPlan};
use std::collections::{HashMap, HashSet, VecDeque};
use std::sync::Arc;
use std::sync::atomic::AtomicU64;
use std::time::Instant;
use tokio::sync::{RwLock, broadcast, mpsc};

struct RuntimeEnvGuard {
    _guard: jcode_base::storage::TestEnvGuard,
    prev_runtime: Option<std::ffi::OsString>,
}

impl RuntimeEnvGuard {
    fn new() -> (Self, tempfile::TempDir) {
        let guard = crate::storage::lock_test_env();
        let temp = tempfile::TempDir::new().expect("create runtime dir");
        let prev_runtime = std::env::var_os("JCODE_RUNTIME_DIR");
        crate::env::set_var("JCODE_RUNTIME_DIR", temp.path());
        (
            Self {
                _guard: guard,
                prev_runtime,
            },
            temp,
        )
    }
}

impl Drop for RuntimeEnvGuard {
    fn drop(&mut self) {
        if let Some(prev_runtime) = self.prev_runtime.take() {
            crate::env::set_var("JCODE_RUNTIME_DIR", prev_runtime);
        } else {
            crate::env::remove_var("JCODE_RUNTIME_DIR");
        }
    }
}

fn member(session_id: &str, swarm_id: &str, role: &str) -> SwarmMember {
    let (event_tx, _event_rx) = mpsc::unbounded_channel();
    SwarmMember {
        session_id: session_id.to_string(),
        event_tx,
        event_txs: HashMap::new(),
        working_dir: None,
        swarm_id: Some(swarm_id.to_string()),
        swarm_enabled: true,
        status: "ready".to_string(),
        detail: None,
        friendly_name: Some(session_id.to_string()),
        report_back_to_session_id: None,
        latest_completion_report: None,
        role: role.to_string(),
        joined_at: Instant::now(),
        last_status_change: Instant::now(),
        is_headless: false,
        output_tail: None,
        todo_progress: None,
        todo_items: Vec::new(),
        runtime: crate::protocol::SwarmMemberRuntime::default(),
        task_label: None,
    }
}

fn plan_item(id: &str, blocked_by: &[&str]) -> PlanItem {
    PlanItem {
        content: format!("task {id}"),
        status: "pending".to_string(),
        priority: "medium".to_string(),
        id: id.to_string(),
        subsystem: None,
        file_scope: Vec::new(),
        blocked_by: blocked_by.iter().map(|value| value.to_string()).collect(),
        assigned_to: None,
    }
}

/// Fixture: coordinator + worker swarm with all the runtime handles the plan
/// handlers thread through.
struct PlanFixture {
    swarm_id: String,
    coord: String,
    worker: String,
    client_tx: mpsc::UnboundedSender<ServerEvent>,
    client_rx: mpsc::UnboundedReceiver<ServerEvent>,
    sessions: crate::server::SessionAgents,
    soft_interrupt_queues: crate::server::SessionInterruptQueues,
    swarm_members: Arc<RwLock<HashMap<String, SwarmMember>>>,
    swarms_by_id: Arc<RwLock<HashMap<String, HashSet<String>>>>,
    shared_context: Arc<RwLock<HashMap<String, HashMap<String, SharedContext>>>>,
    swarm_plans: Arc<RwLock<HashMap<String, VersionedPlan>>>,
    swarm_coordinators: Arc<RwLock<HashMap<String, String>>>,
    event_history: Arc<RwLock<VecDeque<SwarmEvent>>>,
    event_counter: Arc<AtomicU64>,
    swarm_event_tx: broadcast::Sender<SwarmEvent>,
    mutation_runtime: SwarmMutationRuntime,
}

fn plan_fixture(swarm_id: &str, coord: &str, worker: &str) -> PlanFixture {
    let swarm_id = swarm_id.to_string();
    let coord = coord.to_string();
    let worker = worker.to_string();
    let (client_tx, client_rx) = mpsc::unbounded_channel();
    let swarm_members = Arc::new(RwLock::new(HashMap::from([
        (coord.clone(), member(&coord, &swarm_id, "coordinator")),
        (worker.clone(), member(&worker, &swarm_id, "agent")),
    ])));
    let swarms_by_id = Arc::new(RwLock::new(HashMap::from([(
        swarm_id.clone(),
        HashSet::from([coord.clone(), worker.clone()]),
    )])));
    let swarm_plans = Arc::new(RwLock::new(HashMap::from([(
        swarm_id.clone(),
        VersionedPlan::new(),
    )])));
    let swarm_coordinators = Arc::new(RwLock::new(HashMap::from([(
        swarm_id.clone(),
        coord.clone(),
    )])));
    PlanFixture {
        swarm_id,
        coord,
        worker,
        client_tx,
        client_rx,
        sessions: Arc::new(RwLock::new(HashMap::new())),
        soft_interrupt_queues: Arc::new(RwLock::new(HashMap::new())),
        swarm_members,
        swarms_by_id,
        shared_context: Arc::new(RwLock::new(HashMap::new())),
        swarm_plans,
        swarm_coordinators,
        event_history: Arc::new(RwLock::new(VecDeque::new())),
        event_counter: Arc::new(AtomicU64::new(1)),
        swarm_event_tx: broadcast::channel(64).0,
        mutation_runtime: SwarmMutationRuntime::default(),
    }
}

impl PlanFixture {
    async fn propose(&self, from: &str, items: Vec<PlanItem>) {
        handle_comm_propose_plan(
            1,
            from.to_string(),
            items,
            &self.client_tx,
            &self.swarm_members,
            &self.swarms_by_id,
            &self.shared_context,
            &self.swarm_plans,
            &self.swarm_coordinators,
            &self.sessions,
            &self.soft_interrupt_queues,
            &self.event_history,
            &self.event_counter,
            &self.swarm_event_tx,
            &self.mutation_runtime,
        )
        .await;
    }

    async fn approve(&self, proposer: &str) {
        handle_comm_approve_plan(
            2,
            self.coord.clone(),
            proposer.to_string(),
            &self.client_tx,
            &self.swarm_members,
            &self.swarms_by_id,
            &self.shared_context,
            &self.swarm_plans,
            &self.swarm_coordinators,
            &self.sessions,
            &self.soft_interrupt_queues,
            &self.event_history,
            &self.event_counter,
            &self.swarm_event_tx,
            &self.mutation_runtime,
        )
        .await;
    }

    async fn plan_item_ids(&self) -> Vec<String> {
        let plans = self.swarm_plans.read().await;
        plans[&self.swarm_id]
            .items
            .iter()
            .map(|item| item.id.clone())
            .collect()
    }

    fn drain_events(&mut self) -> Vec<ServerEvent> {
        let mut events = Vec::new();
        while let Ok(event) = self.client_rx.try_recv() {
            events.push(event);
        }
        events
    }
}

fn error_messages(events: &[ServerEvent]) -> Vec<String> {
    events
        .iter()
        .filter_map(|event| match event {
            ServerEvent::Error { message, .. } => Some(message.clone()),
            _ => None,
        })
        .collect()
}

fn saw_done(events: &[ServerEvent]) -> bool {
    events
        .iter()
        .any(|event| matches!(event, ServerEvent::Done { .. }))
}

#[test]
fn plan_cycle_error_names_cyclic_ids_and_accepts_dags() {
    // b <-> c cycle: both ids named, deterministic order.
    let cyclic = vec![
        plan_item("a", &[]),
        plan_item("b", &["c"]),
        plan_item("c", &["b"]),
    ];
    let message = plan_cycle_error(&cyclic).expect("cycle must be rejected");
    assert!(message.contains("b, c"), "message: {message}");

    // Self-dependency is a one-node cycle.
    let self_dep = vec![plan_item("solo", &["solo"])];
    let message = plan_cycle_error(&self_dep).expect("self-dependency must be rejected");
    assert!(message.contains("solo"), "message: {message}");

    // A valid DAG (including deps on unknown/external ids) passes.
    let dag = vec![
        plan_item("a", &[]),
        plan_item("b", &["a"]),
        plan_item("c", &["a", "b", "external-id"]),
    ];
    assert_eq!(plan_cycle_error(&dag), None);
}

#[tokio::test]
async fn coordinator_direct_update_rejects_cyclic_plan_without_mutating_it() {
    let (_env, _runtime) = RuntimeEnvGuard::new();
    let mut fx = plan_fixture("swarm-plan-cycle", "coord-pc", "worker-pc");

    // Pre-existing valid plan content must survive the rejected update.
    {
        let mut plans = fx.swarm_plans.write().await;
        let plan = plans.get_mut(&fx.swarm_id).unwrap();
        plan.items = vec![plan_item("keep", &[])];
        plan.version = 3;
    }

    let coord = fx.coord.clone();
    fx.propose(
        &coord,
        vec![
            plan_item("a", &[]),
            plan_item("b", &["c"]),
            plan_item("c", &["b"]),
        ],
    )
    .await;

    assert_eq!(fx.plan_item_ids().await, vec!["keep".to_string()]);
    let plans = fx.swarm_plans.read().await;
    assert_eq!(plans[&fx.swarm_id].version, 3, "version must not bump");
    drop(plans);

    let events = fx.drain_events();
    let errors = error_messages(&events);
    assert!(
        errors
            .iter()
            .any(|m| m.contains("cycle") && m.contains("b, c")),
        "expected cycle error naming b and c, got: {errors:?}"
    );
    assert!(!saw_done(&events), "rejected update must not ack Done");
}

#[tokio::test]
async fn coordinator_direct_update_rejects_self_dependency() {
    let (_env, _runtime) = RuntimeEnvGuard::new();
    let mut fx = plan_fixture("swarm-plan-selfdep", "coord-sd", "worker-sd");

    let coord = fx.coord.clone();
    fx.propose(&coord, vec![plan_item("loop", &["loop"])]).await;

    assert!(fx.plan_item_ids().await.is_empty());
    let events = fx.drain_events();
    let errors = error_messages(&events);
    assert!(
        errors
            .iter()
            .any(|m| m.contains("cycle") && m.contains("loop")),
        "expected self-dependency rejection, got: {errors:?}"
    );
}

#[tokio::test]
async fn coordinator_direct_update_accepts_valid_dag() {
    let (_env, _runtime) = RuntimeEnvGuard::new();
    let mut fx = plan_fixture("swarm-plan-dag", "coord-dag", "worker-dag");

    let coord = fx.coord.clone();
    fx.propose(
        &coord,
        vec![
            plan_item("a", &[]),
            plan_item("b", &["a"]),
            plan_item("c", &["a", "b"]),
        ],
    )
    .await;

    assert_eq!(
        fx.plan_item_ids().await,
        vec!["a".to_string(), "b".to_string(), "c".to_string()]
    );
    let plans = fx.swarm_plans.read().await;
    assert_eq!(plans[&fx.swarm_id].version, 1);
    drop(plans);

    let events = fx.drain_events();
    assert!(saw_done(&events), "valid DAG update must ack Done");
    assert!(error_messages(&events).is_empty());
}

#[tokio::test]
async fn worker_proposal_with_internal_cycle_is_rejected_before_storage() {
    let (_env, _runtime) = RuntimeEnvGuard::new();
    let mut fx = plan_fixture("swarm-prop-cycle", "coord-wp", "worker-wp");

    let worker = fx.worker.clone();
    fx.propose(
        &worker,
        vec![plan_item("b", &["c"]), plan_item("c", &["b"])],
    )
    .await;

    // No proposal stored for the coordinator to approve.
    let context = fx.shared_context.read().await;
    let stored = context
        .get(&fx.swarm_id)
        .and_then(|swarm_context| swarm_context.get(&format!("plan_proposal:{worker}")));
    assert!(stored.is_none(), "cyclic proposal must not be stored");
    drop(context);

    let events = fx.drain_events();
    let errors = error_messages(&events);
    assert!(
        errors.iter().any(|m| m.contains("cycle")),
        "expected proposer-side cycle rejection, got: {errors:?}"
    );
}

#[tokio::test]
async fn approve_plan_rejects_proposal_that_forms_cycle_with_existing_plan() {
    let (_env, _runtime) = RuntimeEnvGuard::new();
    let mut fx = plan_fixture("swarm-approve-cycle", "coord-ac", "worker-ac");

    // Existing plan: a depends on b (b not yet defined, so no cycle yet).
    {
        let mut plans = fx.swarm_plans.write().await;
        let plan = plans.get_mut(&fx.swarm_id).unwrap();
        plan.items = vec![plan_item("a", &["b"])];
        plan.version = 1;
    }

    // Proposal alone is acyclic (its dep 'a' is external to the proposal), so it
    // passes the proposer-side check and is stored for approval.
    let worker = fx.worker.clone();
    fx.propose(&worker, vec![plan_item("b", &["a"])]).await;
    {
        let context = fx.shared_context.read().await;
        assert!(
            context
                .get(&fx.swarm_id)
                .and_then(|c| c.get(&format!("plan_proposal:{worker}")))
                .is_some(),
            "acyclic-in-isolation proposal should be stored"
        );
    }

    // Approval must reject: merged graph has a <-> b.
    fx.approve(&worker).await;

    assert_eq!(fx.plan_item_ids().await, vec!["a".to_string()]);
    let plans = fx.swarm_plans.read().await;
    assert_eq!(plans[&fx.swarm_id].version, 1, "version must not bump");
    drop(plans);

    // The proposal stays pending so the proposer can fix and re-propose.
    let context = fx.shared_context.read().await;
    assert!(
        context
            .get(&fx.swarm_id)
            .and_then(|c| c.get(&format!("plan_proposal:{worker}")))
            .is_some(),
        "rejected proposal should remain pending"
    );
    drop(context);

    let events = fx.drain_events();
    let errors = error_messages(&events);
    assert!(
        errors
            .iter()
            .any(|m| m.contains("cycle") && m.contains('a') && m.contains('b')),
        "expected merged-cycle rejection naming a and b, got: {errors:?}"
    );
}

/// Deterministic demonstration of SwarmPlanProposal delivery loss
/// (wiring-audit.status-proposal-ordering, bug 2).
///
/// The non-coordinator proposal path in `handle_comm_propose_plan` sends the
/// coordinator's `SwarmPlanProposal` (and its companion Notification) via the
/// raw cached `member.event_tx` with the send result discarded, instead of
/// `fanout_session_event`. The cached primary channel goes stale whenever the
/// connection that registered it drops (e.g. TUI reconnect): the member entry
/// keeps a live attachment in `event_txs`, but `event_tx` still points at the
/// closed channel. `fanout_session_event` handles exactly this by retaining
/// live `event_txs` and re-pointing `event_tx`, so a coordinator with a live
/// attachment silently loses the structured proposal event only because this
/// call site bypasses it.
///
/// The soft-interrupt fallback (queue_soft_interrupt_for_session) still fires,
/// so the coordinator gets a textual hint, but any UI driven by the
/// SwarmPlanProposal event never sees the proposal.
///
/// If this test starts failing because `live_rx` received the proposal, the
/// call site was switched to fanout delivery: update the wiring audit.
#[tokio::test]
async fn worker_proposal_is_lost_when_coordinator_cached_channel_is_closed() {
    let (_env, _runtime) = RuntimeEnvGuard::new();
    let mut fx = plan_fixture("swarm-prop-lost", "coord-pl", "worker-pl");
    let coord = fx.coord.clone();
    let worker = fx.worker.clone();

    // Stale primary channel: receiver dropped before the proposal arrives.
    let (closed_tx, closed_rx) = mpsc::unbounded_channel::<ServerEvent>();
    drop(closed_rx);
    // Live attachment, as fanout_session_event would use after a reconnect.
    let (live_tx, mut live_rx) = mpsc::unbounded_channel::<ServerEvent>();
    {
        let mut members = fx.swarm_members.write().await;
        let member = members.get_mut(&coord).expect("coordinator member");
        member.event_tx = closed_tx;
        member.event_txs.insert("conn-live".to_string(), live_tx);
    }

    // Register a live soft-interrupt queue for the coordinator so the
    // fallback path is observable.
    let coord_queue: jcode_agent_runtime::SoftInterruptQueue =
        Arc::new(std::sync::Mutex::new(Vec::new()));
    fx.soft_interrupt_queues
        .write()
        .await
        .insert(coord.clone(), coord_queue.clone());

    fx.propose(&worker, vec![plan_item("new-task", &[])]).await;

    // The proposal is stored and the proposer is acked, so nothing upstream
    // signals a failure...
    {
        let context = fx.shared_context.read().await;
        assert!(
            context
                .get(&fx.swarm_id)
                .and_then(|c| c.get(&format!("plan_proposal:{worker}")))
                .is_some(),
            "proposal should be stored for approval"
        );
    }
    let events = fx.drain_events();
    assert!(saw_done(&events), "proposer must be acked with Done");
    assert!(error_messages(&events).is_empty(), "no error surfaced");

    // ...but the coordinator's live attachment never receives the structured
    // proposal event (or its notification): both went to the closed cached
    // channel and the Err was discarded.
    let mut delivered = Vec::new();
    while let Ok(event) = live_rx.try_recv() {
        delivered.push(event);
    }
    assert!(
        !delivered
            .iter()
            .any(|event| matches!(event, ServerEvent::SwarmPlanProposal { .. })),
        "expected SwarmPlanProposal to be silently lost on the closed cached \
         channel; receiving it here means the call site now uses fanout \
         delivery (update the wiring audit): {delivered:?}"
    );
    assert!(
        delivered.is_empty(),
        "no event at all should reach the live attachment via this path: {delivered:?}"
    );

    // The soft-interrupt fallback still fires, so the coordinator gets a
    // textual hint about the proposal even though the event was lost.
    let pending = coord_queue.lock().expect("coordinator interrupt queue");
    assert_eq!(
        pending.len(),
        1,
        "soft-interrupt fallback should queue once"
    );
    assert!(
        pending[0].content.contains("Plan proposal from")
            && pending[0]
                .content
                .contains(&format!("plan_proposal:{worker}")),
        "fallback text should reference the stored proposal key: {}",
        pending[0].content
    );
}

#[tokio::test]
async fn approve_plan_accepts_valid_dag_proposal() {
    let (_env, _runtime) = RuntimeEnvGuard::new();
    let mut fx = plan_fixture("swarm-approve-dag", "coord-ad", "worker-ad");

    {
        let mut plans = fx.swarm_plans.write().await;
        let plan = plans.get_mut(&fx.swarm_id).unwrap();
        plan.items = vec![plan_item("a", &[])];
        plan.version = 1;
    }

    let worker = fx.worker.clone();
    fx.propose(&worker, vec![plan_item("b", &["a"])]).await;
    fx.approve(&worker).await;

    assert_eq!(
        fx.plan_item_ids().await,
        vec!["a".to_string(), "b".to_string()]
    );
    let plans = fx.swarm_plans.read().await;
    assert_eq!(plans[&fx.swarm_id].version, 2);
    drop(plans);

    // The consumed proposal is removed.
    let context = fx.shared_context.read().await;
    assert!(
        context
            .get(&fx.swarm_id)
            .and_then(|c| c.get(&format!("plan_proposal:{worker}")))
            .is_none(),
        "approved proposal should be consumed"
    );
    drop(context);

    let events = fx.drain_events();
    assert!(saw_done(&events), "approval must ack");
}

/// Pins the delivery gap for the coordinator direct-update path
/// (wiring-audit.raw-event-tx-delivery-audit), the representative lossy site
/// OUTSIDE the proposal path: `handle_comm_propose_plan`'s
/// coordinator-direct-update branch sends the "Plan updated by ..."
/// Notification to each participant via the raw cached `member.event_tx`
/// (comm_plan.rs ~144), and `broadcast_swarm_plan` sends the SwarmPlan event
/// the same way (swarm.rs ~781). Neither routes through
/// `fanout_session_event` (state.rs), which retains live `event_txs` and
/// re-points the stale cached channel, so after a reattach rotates channels
/// both structured events are silently dropped even though a live attachment
/// exists. Only the soft-interrupt text fallback survives.
///
/// If a production fix routes these sites through fanout delivery, the
/// "lost" assertions below start failing: flip them to assert delivery.
#[tokio::test]
async fn direct_update_notification_is_lost_on_stale_cached_event_tx() {
    let (_env, _runtime) = RuntimeEnvGuard::new();
    let mut fx = plan_fixture("swarm-plan-stale-tx", "coord-st", "worker-st");
    let coord = fx.coord.clone();
    let worker = fx.worker.clone();

    // Rig the worker like a member whose cached channel went stale while a
    // live attachment exists: `event_tx` closed (receiver dropped), while
    // `event_txs` holds one live connection.
    let (live_tx, mut live_rx) = mpsc::unbounded_channel::<ServerEvent>();
    {
        let mut members = fx.swarm_members.write().await;
        let member = members.get_mut(&worker).expect("worker member");
        let (stale_tx, stale_rx) = mpsc::unbounded_channel::<ServerEvent>();
        drop(stale_rx);
        member.event_tx = stale_tx;
        member.event_txs.insert("conn-live".to_string(), live_tx);
    }

    // Register a soft-interrupt queue for the worker so the fallback this
    // site does have is observable.
    let interrupt_queue: jcode_agent_runtime::SoftInterruptQueue =
        Arc::new(std::sync::Mutex::new(Vec::new()));
    fx.soft_interrupt_queues
        .write()
        .await
        .insert(worker.clone(), interrupt_queue.clone());

    // A coordinator direct update assigning an item to the worker makes the
    // worker a plan participant, so the notify loop targets it.
    let mut item = plan_item("a", &[]);
    item.assigned_to = Some(worker.clone());
    fx.propose(&coord, vec![item]).await;

    let events = fx.drain_events();
    assert!(saw_done(&events), "direct update should ack: {events:?}");

    // Lossy pin: neither the "Plan updated" Notification (comm_plan.rs raw
    // send) nor the SwarmPlan broadcast (swarm.rs raw send) reached the live
    // attachment. Both went to the closed cached channel.
    let mut delivered = Vec::new();
    while let Ok(event) = live_rx.try_recv() {
        delivered.push(event);
    }
    assert!(
        delivered.is_empty(),
        "raw event_tx sends currently bypass live event_txs attachments; if \
         events now arrive here, the delivery gap was fixed - update this \
         test and the wiring audit: {delivered:?}"
    );

    // The fallback that does exist for this site: the plan-update text was
    // queued as a soft interrupt, so a live agent still learns of the change
    // even though the structured UI events were dropped.
    {
        let pending = interrupt_queue.lock().expect("queue lock");
        assert_eq!(pending.len(), 1, "soft-interrupt fallback should fire");
        assert!(
            pending[0].content.contains("Plan updated"),
            "unexpected fallback content: {}",
            pending[0].content
        );
    }

    // Contrast: identical member state routed through fanout_session_event
    // recovers from the stale cached channel and reaches the live attachment.
    let delivered = crate::server::fanout_session_event(
        &fx.swarm_members,
        &worker,
        ServerEvent::Done { id: 99 },
    )
    .await;
    assert_eq!(delivered, 1, "fanout must rotate onto the live attachment");
    assert!(
        matches!(live_rx.try_recv(), Ok(ServerEvent::Done { id: 99 })),
        "fanout-delivered event should arrive on the live attachment"
    );
}
