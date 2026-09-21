#![allow(clippy::await_holding_lock)]

use super::super::{handle_comm_stop, process_message_streaming_mpsc};
use super::{member, test_agent_with_working_dir};

/// Isolates `JCODE_HOME` for one test and restores the previous value on drop.
struct EnvVarGuard {
    key: &'static str,
    previous: Option<std::ffi::OsString>,
}

impl EnvVarGuard {
    fn set_path(key: &'static str, value: &std::path::Path) -> Self {
        let previous = std::env::var_os(key);
        crate::env::set_var(key, value);
        Self { key, previous }
    }
}

impl Drop for EnvVarGuard {
    fn drop(&mut self) {
        if let Some(value) = self.previous.take() {
            crate::env::set_var(self.key, value);
        } else {
            crate::env::remove_var(self.key);
        }
    }
}
use crate::agent::Agent;
use crate::message::{Message, StreamEvent, ToolDefinition};
use crate::protocol::ServerEvent;
use crate::provider::{EventStream, Provider};
use crate::server::live_turn::{LiveTurnSwarmContext, idle_live_agent, spawn_tracked_live_turn};
use crate::server::state::restore_session_interrupt_delivery;
use crate::server::swarm_mutation_state::SwarmMutationRuntime;
use crate::server::{
    ChannelSubscriptions, SessionAgents, SessionControlHandle, SessionInterruptQueues, SwarmEvent,
    SwarmMember, VersionedPlan, begin_session_interrupt_delivery, queue_soft_interrupt_for_session,
    register_session_interrupt_queue,
};
use crate::tool::Registry;
use anyhow::Result;
use async_trait::async_trait;
use futures::{StreamExt, stream};
use jcode_agent_runtime::{InterruptSignal, SoftInterruptMessage, SoftInterruptSource};
use std::collections::{HashMap, HashSet, VecDeque};
use std::sync::Arc;
use std::sync::atomic::{AtomicU64, AtomicUsize, Ordering};
use std::time::Duration;
use tokio::sync::{Mutex, RwLock, broadcast, mpsc};

#[derive(Clone)]
struct BlockingStreamProvider {
    calls: Arc<AtomicUsize>,
}

#[async_trait]
impl Provider for BlockingStreamProvider {
    async fn complete(
        &self,
        _messages: &[Message],
        _tools: &[ToolDefinition],
        _system: &str,
        _resume_session_id: Option<&str>,
    ) -> Result<EventStream> {
        self.calls.fetch_add(1, Ordering::SeqCst);
        let started = stream::iter([Ok(StreamEvent::TextDelta("started".to_string()))]);
        Ok(Box::pin(started.chain(stream::pending())))
    }

    fn name(&self) -> &str {
        "blocking-stream"
    }

    fn fork(&self) -> Arc<dyn Provider> {
        Arc::new(self.clone())
    }
}

async fn blocking_agent(session_id: &str, calls: &Arc<AtomicUsize>) -> Arc<Mutex<Agent>> {
    let provider: Arc<dyn Provider> = Arc::new(BlockingStreamProvider {
        calls: Arc::clone(calls),
    });
    let registry = Registry::new(Arc::clone(&provider)).await;
    let mut session = crate::session::Session::create_with_id(session_id.to_string(), None, None);
    session.model = Some("blocking-stream".to_string());
    Arc::new(Mutex::new(Agent::new_with_session(
        provider, registry, session, None,
    )))
}

struct StopFixture {
    worker_id: String,
    coordinator_id: String,
    sessions: SessionAgents,
    swarm_members: Arc<RwLock<HashMap<String, SwarmMember>>>,
    swarms_by_id: Arc<RwLock<HashMap<String, HashSet<String>>>>,
    swarm_coordinators: Arc<RwLock<HashMap<String, String>>>,
    swarm_plans: Arc<RwLock<HashMap<String, VersionedPlan>>>,
    channel_subscriptions: ChannelSubscriptions,
    channel_subscriptions_by_session: ChannelSubscriptions,
    event_history: Arc<RwLock<VecDeque<SwarmEvent>>>,
    event_counter: Arc<AtomicU64>,
    swarm_event_tx: broadcast::Sender<SwarmEvent>,
    soft_interrupt_queues: SessionInterruptQueues,
    mutation_runtime: SwarmMutationRuntime,
    queue: jcode_agent_runtime::SoftInterruptQueue,
    unrelated_id: String,
    _worker_events: mpsc::UnboundedReceiver<ServerEvent>,
}

impl StopFixture {
    async fn new(worker_id: &str, worker: Arc<Mutex<Agent>>) -> Self {
        let coordinator_id = format!("{worker_id}-coord");
        let unrelated_id = format!("{worker_id}-unrelated");
        let swarm_id = format!("{worker_id}-swarm");
        let sessions = Arc::new(RwLock::new(HashMap::new()));
        let unrelated = test_agent_with_working_dir(&unrelated_id, "/tmp/unrelated").await;
        {
            let mut live_sessions = sessions.write().await;
            live_sessions.insert(worker_id.to_string(), Arc::clone(&worker));
            live_sessions.insert(unrelated_id.clone(), unrelated);
        }

        let swarm_members = Arc::new(RwLock::new(HashMap::new()));
        let (coordinator, _coordinator_events) =
            member(&coordinator_id, Some(&swarm_id), "coordinator");
        let (mut worker_member, worker_events) = member(worker_id, Some(&swarm_id), "agent");
        let (unrelated_member, _unrelated_events) = member(&unrelated_id, Some(&swarm_id), "agent");
        worker_member.report_back_to_session_id = Some(coordinator_id.clone());
        {
            let mut members = swarm_members.write().await;
            members.insert(coordinator_id.clone(), coordinator);
            members.insert(worker_id.to_string(), worker_member);
            members.insert(unrelated_id.clone(), unrelated_member);
        }

        let swarms_by_id = Arc::new(RwLock::new(HashMap::from([(
            swarm_id.clone(),
            HashSet::from([
                coordinator_id.clone(),
                worker_id.to_string(),
                unrelated_id.clone(),
            ]),
        )])));
        let swarm_coordinators = Arc::new(RwLock::new(HashMap::from([(
            swarm_id,
            coordinator_id.clone(),
        )])));
        let swarm_plans = Arc::new(RwLock::new(HashMap::new()));
        let channel_subscriptions = Arc::new(RwLock::new(HashMap::new()));
        let channel_subscriptions_by_session = Arc::new(RwLock::new(HashMap::new()));
        let event_history = Arc::new(RwLock::new(VecDeque::new()));
        let event_counter = Arc::new(AtomicU64::new(0));
        let (swarm_event_tx, _swarm_event_rx) = broadcast::channel(8);
        let soft_interrupt_queues = Arc::new(RwLock::new(HashMap::new()));
        let queue = worker.lock().await.soft_interrupt_queue();
        register_session_interrupt_queue(&soft_interrupt_queues, worker_id, queue.clone()).await;

        Self {
            worker_id: worker_id.to_string(),
            coordinator_id,
            sessions,
            swarm_members,
            swarms_by_id,
            swarm_coordinators,
            swarm_plans,
            channel_subscriptions,
            channel_subscriptions_by_session,
            event_history,
            event_counter,
            swarm_event_tx,
            soft_interrupt_queues,
            mutation_runtime: SwarmMutationRuntime::default(),
            queue,
            unrelated_id,
            _worker_events: worker_events,
        }
    }

    fn spawn_stop(
        &self,
        id: u64,
    ) -> (
        tokio::task::JoinHandle<()>,
        mpsc::UnboundedReceiver<ServerEvent>,
    ) {
        let (client_event_tx, client_event_rx) = mpsc::unbounded_channel();
        let worker_id = self.worker_id.clone();
        let coordinator_id = self.coordinator_id.clone();
        let sessions = Arc::clone(&self.sessions);
        let swarm_members = Arc::clone(&self.swarm_members);
        let swarms_by_id = Arc::clone(&self.swarms_by_id);
        let swarm_coordinators = Arc::clone(&self.swarm_coordinators);
        let swarm_plans = Arc::clone(&self.swarm_plans);
        let channel_subscriptions = Arc::clone(&self.channel_subscriptions);
        let channel_subscriptions_by_session = Arc::clone(&self.channel_subscriptions_by_session);
        let event_history = Arc::clone(&self.event_history);
        let event_counter = Arc::clone(&self.event_counter);
        let swarm_event_tx = self.swarm_event_tx.clone();
        let soft_interrupt_queues = Arc::clone(&self.soft_interrupt_queues);
        let mutation_runtime = self.mutation_runtime.clone();
        let task = tokio::spawn(async move {
            handle_comm_stop(
                id,
                coordinator_id,
                worker_id,
                false,
                &client_event_tx,
                &sessions,
                &swarm_members,
                &swarms_by_id,
                &swarm_coordinators,
                &swarm_plans,
                &channel_subscriptions,
                &channel_subscriptions_by_session,
                &event_history,
                &event_counter,
                &swarm_event_tx,
                &soft_interrupt_queues,
                &mutation_runtime,
            )
            .await;
        });
        (task, client_event_rx)
    }

    async fn stop(&self, id: u64) -> ServerEvent {
        let (task, mut events) = self.spawn_stop(id);
        task.await.expect("stop task");
        events.recv().await.expect("stop response")
    }

    fn live_turn_context(&self) -> LiveTurnSwarmContext {
        LiveTurnSwarmContext::new(
            &self.swarm_members,
            &self.swarms_by_id,
            &self.event_history,
            &self.event_counter,
            &self.swarm_event_tx,
        )
    }
}

async fn wait_until_interrupt_delivery_stops(session_id: &str) {
    tokio::time::timeout(Duration::from_secs(2), async {
        loop {
            if begin_session_interrupt_delivery(session_id).is_none() {
                return;
            }
            tokio::task::yield_now().await;
        }
    })
    .await
    .expect("stop lifecycle gate was not set");
}

fn setup_test_home() -> (tempfile::TempDir, EnvVarGuard) {
    let temp_home = tempfile::TempDir::new().expect("temp home");
    let home = EnvVarGuard::set_path("JCODE_HOME", temp_home.path());
    (temp_home, home)
}

#[tokio::test]
async fn stopping_busy_owned_worker_cancels_turn_and_discards_queued_wake() {
    let _storage_guard = crate::storage::lock_test_env();
    let (_temp_home, _home) = setup_test_home();
    let provider_calls = Arc::new(AtomicUsize::new(0));
    let worker = blocking_agent("stop-running-worker", &provider_calls).await;
    let fixture = StopFixture::new("stop-running-worker", Arc::clone(&worker)).await;

    let (turn_event_tx, mut turn_event_rx) = mpsc::unbounded_channel();
    let running_worker = Arc::clone(&worker);
    let turn = tokio::spawn(async move {
        process_message_streaming_mpsc(
            running_worker,
            "initial work",
            Vec::new(),
            None,
            turn_event_tx,
        )
        .await
    });
    loop {
        match tokio::time::timeout(Duration::from_secs(2), turn_event_rx.recv()).await {
            Ok(Some(ServerEvent::TextDelta { .. })) => break,
            Ok(Some(_)) => continue,
            Ok(None) => panic!("worker event stream closed before the turn started"),
            Err(_) => panic!("worker turn did not start"),
        }
    }

    assert!(
        queue_soft_interrupt_for_session(
            &fixture.worker_id,
            "queued follow-up".to_string(),
            false,
            SoftInterruptSource::System,
            &fixture.soft_interrupt_queues,
            &fixture.sessions,
        )
        .await
    );
    crate::soft_interrupt_store::append(
        &fixture.worker_id,
        SoftInterruptMessage {
            content: "persisted queued follow-up".to_string(),
            images: Vec::new(),
            urgent: false,
            source: SoftInterruptSource::System,
        },
    )
    .expect("persist queued follow-up");

    assert!(matches!(fixture.stop(7).await, ServerEvent::Done { id: 7 }));
    assert!(
        crate::turn_cancel_registry::active_turn_signals(&fixture.worker_id).is_empty(),
        "stop acknowledgement must not leave an active worker turn"
    );
    tokio::time::timeout(Duration::from_secs(2), turn)
        .await
        .expect("stop must quiesce the running worker before acknowledging")
        .expect("worker turn task")
        .expect("cancelled worker turn should checkpoint cleanly");
    assert_eq!(provider_calls.load(Ordering::SeqCst), 1);
    assert!(
        fixture
            .queue
            .lock()
            .expect("soft interrupt queue")
            .is_empty()
    );
    assert!(
        crate::soft_interrupt_store::load(&fixture.worker_id)
            .expect("load persisted soft interrupts")
            .is_empty()
    );
    assert!(worker.lock().await.is_closed());
    assert!(
        !fixture
            .sessions
            .read()
            .await
            .contains_key(&fixture.worker_id)
    );
    assert!(
        fixture
            .sessions
            .read()
            .await
            .contains_key(&fixture.unrelated_id)
    );
    assert!(
        !fixture
            .swarm_members
            .read()
            .await
            .contains_key(&fixture.worker_id)
    );
    assert!(
        fixture
            .swarm_members
            .read()
            .await
            .contains_key(&fixture.unrelated_id)
    );
}

#[tokio::test]
async fn stopping_reserved_live_worker_discards_late_turn_before_provider_dispatch() {
    let _storage_guard = crate::storage::lock_test_env();
    let (_temp_home, _home) = setup_test_home();
    let provider_calls = Arc::new(AtomicUsize::new(0));
    let worker = blocking_agent("stop-reserved-worker", &provider_calls).await;
    let fixture = StopFixture::new("stop-reserved-worker", Arc::clone(&worker)).await;
    let reserved = idle_live_agent(
        &fixture.worker_id,
        &fixture.sessions,
        &fixture.swarm_members,
    )
    .await
    .expect("reserve idle live worker");

    let (stop_task, mut stop_events) = fixture.spawn_stop(11);
    wait_until_interrupt_delivery_stops(&fixture.worker_id).await;
    spawn_tracked_live_turn(
        &fixture.worker_id,
        reserved,
        "late queued wake".to_string(),
        None,
        None,
        Some("late queued wake".to_string()),
        fixture.live_turn_context(),
    )
    .await;
    stop_task.await.expect("stop task");

    assert!(matches!(
        stop_events.recv().await,
        Some(ServerEvent::Done { id: 11 })
    ));
    assert_eq!(
        provider_calls.load(Ordering::SeqCst),
        0,
        "a reserved wake released after stop begins must not dispatch the provider"
    );
    assert!(worker.lock().await.is_closed());
    assert!(
        !fixture
            .sessions
            .read()
            .await
            .contains_key(&fixture.worker_id)
    );
}

#[tokio::test]
async fn stop_drains_inflight_queue_producers_and_rejects_late_live_or_persisted_work() {
    let _storage_guard = crate::storage::lock_test_env();
    let (_temp_home, _home) = setup_test_home();
    let worker = test_agent_with_working_dir("stop-queue-worker", "/tmp/stop-queue-worker").await;
    let fixture = StopFixture::new("stop-queue-worker", Arc::clone(&worker)).await;
    let producer = begin_session_interrupt_delivery(&fixture.worker_id)
        .expect("producer must enter before stop starts");
    let (stop_task, mut stop_events) = fixture.spawn_stop(13);
    wait_until_interrupt_delivery_stops(&fixture.worker_id).await;
    restore_session_interrupt_delivery(&fixture.worker_id);
    assert!(
        begin_session_interrupt_delivery(&fixture.worker_id).is_none(),
        "resume cannot reopen interrupt delivery while stop is in flight"
    );

    fixture
        .queue
        .lock()
        .expect("soft interrupt queue")
        .push(SoftInterruptMessage {
            content: "in-flight live producer".to_string(),
            images: Vec::new(),
            urgent: false,
            source: SoftInterruptSource::System,
        });
    crate::soft_interrupt_store::append(
        &fixture.worker_id,
        SoftInterruptMessage {
            content: "in-flight persisted producer".to_string(),
            images: Vec::new(),
            urgent: false,
            source: SoftInterruptSource::System,
        },
    )
    .expect("persist in-flight producer work");
    drop(producer);
    stop_task.await.expect("stop task");

    assert!(matches!(
        stop_events.recv().await,
        Some(ServerEvent::Done { id: 13 })
    ));
    assert!(
        fixture
            .queue
            .lock()
            .expect("soft interrupt queue")
            .is_empty()
    );
    assert!(
        crate::soft_interrupt_store::load(&fixture.worker_id)
            .expect("load persisted interrupts")
            .is_empty()
    );
    let control = SessionControlHandle::cancel_only(
        &fixture.worker_id,
        fixture.queue.clone(),
        InterruptSignal::new(),
    );
    assert!(!control.queue_soft_interrupt(
        "late live producer".to_string(),
        Vec::new(),
        false,
        SoftInterruptSource::System,
    ));
    assert!(
        !queue_soft_interrupt_for_session(
            &fixture.worker_id,
            "late persisted producer".to_string(),
            false,
            SoftInterruptSource::System,
            &fixture.soft_interrupt_queues,
            &fixture.sessions,
        )
        .await
    );
    worker.lock().await.queue_soft_interrupt(
        "late direct agent producer".to_string(),
        Vec::new(),
        false,
        SoftInterruptSource::System,
    );
    assert!(
        fixture
            .queue
            .lock()
            .expect("soft interrupt queue")
            .is_empty()
    );
    assert!(
        crate::soft_interrupt_store::load(&fixture.worker_id)
            .expect("load persisted interrupts")
            .is_empty()
    );
    restore_session_interrupt_delivery(&fixture.worker_id);
    assert!(control.queue_soft_interrupt(
        "explicitly resumed producer".to_string(),
        Vec::new(),
        false,
        SoftInterruptSource::System,
    ));
}

#[tokio::test]
async fn stop_timeout_keeps_target_resolvable_and_a_later_retry_can_finish() {
    let _storage_guard = crate::storage::lock_test_env();
    let (_temp_home, _home) = setup_test_home();
    let worker =
        test_agent_with_working_dir("stop-timeout-worker", "/tmp/stop-timeout-worker").await;
    let fixture = StopFixture::new("stop-timeout-worker", Arc::clone(&worker)).await;
    let busy_worker = Arc::clone(&worker);
    let busy = busy_worker.lock_owned().await;

    let first = fixture.stop(17).await;
    match first {
        ServerEvent::Error {
            id: 17, message, ..
        } => {
            assert!(message.contains("remains in stopping state"));
        }
        other => panic!("expected retryable stop timeout, got {other:?}"),
    }
    assert!(
        fixture
            .sessions
            .read()
            .await
            .contains_key(&fixture.worker_id)
    );
    assert!(
        fixture
            .swarm_members
            .read()
            .await
            .contains_key(&fixture.worker_id)
    );
    assert_eq!(
        fixture
            .swarm_members
            .read()
            .await
            .get(&fixture.worker_id)
            .map(|member| member.status.as_str()),
        Some("stopping")
    );

    drop(busy);
    assert!(matches!(
        fixture.stop(18).await,
        ServerEvent::Done { id: 18 }
    ));
    assert!(
        !fixture
            .sessions
            .read()
            .await
            .contains_key(&fixture.worker_id)
    );
    assert!(
        !fixture
            .swarm_members
            .read()
            .await
            .contains_key(&fixture.worker_id)
    );
    assert!(worker.lock().await.is_closed());
}
