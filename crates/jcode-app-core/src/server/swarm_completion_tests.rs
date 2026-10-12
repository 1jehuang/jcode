//! Actual terminal publishers must not turn a cancelled descendant into success.
#![allow(clippy::await_holding_lock)]

use super::super::{client_lifecycle, comm_session, live_turn};
use super::{TurnPublication, begin_turn_publication, cancel_spawned_descendant_turns};
use crate::protocol::{ServerEvent, TurnStopReason};
use std::collections::{HashMap, HashSet, VecDeque};
use std::sync::Arc;
use std::sync::atomic::{AtomicU64, Ordering};
use tokio::sync::{RwLock, broadcast, mpsc, oneshot};

#[derive(Clone, Copy, Debug)]
enum CompletionPath {
    Headless,
    Live,
    Client,
}

struct Fixture {
    worker: String,
    coordinator: String,
    stranger: String,
    swarm: live_turn::LiveTurnSwarmContext,
    worker_rx: mpsc::UnboundedReceiver<ServerEvent>,
    coordinator_rx: mpsc::UnboundedReceiver<ServerEvent>,
    stranger_rx: mpsc::UnboundedReceiver<ServerEvent>,
}

impl Fixture {
    fn new() -> Self {
        static NEXT: AtomicU64 = AtomicU64::new(1);
        let prefix = format!("publication-{}", NEXT.fetch_add(1, Ordering::Relaxed));
        let coordinator = format!("{prefix}-coordinator");
        let worker = format!("{prefix}-worker");
        let stranger = format!("{prefix}-stranger");
        let (coordinator_member, coordinator_rx) =
            super::swarm_cascade_tests::member(&coordinator, None);
        let (worker_member, worker_rx) =
            super::swarm_cascade_tests::member(&worker, Some(&coordinator));
        let (stranger_member, stranger_rx) = super::swarm_cascade_tests::member(&stranger, None);
        let members = Arc::new(RwLock::new(HashMap::from([
            (coordinator.clone(), coordinator_member),
            (worker.clone(), worker_member),
            (stranger.clone(), stranger_member),
        ])));
        let swarms = Arc::new(RwLock::new(HashMap::from([(
            "swarm-1".to_string(),
            HashSet::from([coordinator.clone(), worker.clone(), stranger.clone()]),
        )])));
        let (event_tx, _) = broadcast::channel(32);
        Self {
            worker,
            coordinator,
            stranger,
            swarm: live_turn::LiveTurnSwarmContext::new(
                &members,
                &swarms,
                &Arc::new(RwLock::new(VecDeque::new())),
                &Arc::new(AtomicU64::new(0)),
                &event_tx,
            ),
            worker_rx,
            coordinator_rx,
            stranger_rx,
        }
    }

    async fn cancel(&self) {
        let stopped = cancel_spawned_descendant_turns(
            &self.coordinator,
            &self.swarm.members,
            &self.swarm.swarms_by_id,
            Some(&self.swarm.event_history),
            Some(&self.swarm.event_counter),
            Some(&self.swarm.event_tx),
        )
        .await;
        assert_eq!(stopped, vec![self.worker.clone()]);
    }

    async fn worker_tx(&self) -> mpsc::UnboundedSender<ServerEvent> {
        self.swarm.members.read().await[&self.worker]
            .event_tx
            .clone()
    }
}

async fn complete(
    path: CompletionPath,
    session_id: &str,
    publication: &TurnPublication,
    failed: bool,
    tx: &mpsc::UnboundedSender<ServerEvent>,
    swarm: &live_turn::LiveTurnSwarmContext,
) {
    let result = if failed {
        Err(anyhow::anyhow!("late provider failure"))
    } else {
        Ok(())
    };
    let report = Some("partial output must not count as success".to_string());
    match path {
        CompletionPath::Headless => {
            comm_session::complete_headless_turn(session_id, publication, result, report, swarm)
                .await
        }
        CompletionPath::Live => {
            live_turn::complete_live_turn(
                session_id,
                publication,
                result.map_err(|error| (error, None)),
                report,
                TurnStopReason::Failure,
                tx,
                swarm,
            )
            .await
        }
        CompletionPath::Client => {
            client_lifecycle::complete_client_turn(
                17,
                session_id,
                publication,
                &result,
                report,
                TurnStopReason::Failure,
                tx,
                swarm,
            )
            .await
        }
    }
}

fn terminal_events(rx: &mut mpsc::UnboundedReceiver<ServerEvent>) -> Vec<ServerEvent> {
    let mut events = Vec::new();
    while let Ok(event) = rx.try_recv() {
        if matches!(
            event,
            ServerEvent::TurnStopped { .. } | ServerEvent::Done { .. } | ServerEvent::Error { .. }
        ) {
            events.push(event);
        }
    }
    events
}

async fn cancellation_then_completion(path: CompletionPath, failed: bool) {
    let mut fixture = Fixture::new();
    let publication = begin_turn_publication(&fixture.worker).await;
    let signal = jcode_agent_runtime::InterruptSignal::new();
    let active = crate::turn_cancel_registry::register_active_turn(&fixture.worker, signal.clone());
    let stranger_signal = jcode_agent_runtime::InterruptSignal::new();
    let _stranger_active = crate::turn_cancel_registry::register_active_turn(
        &fixture.stranger,
        stranger_signal.clone(),
    );
    let stranger_publication = begin_turn_publication(&fixture.stranger).await;
    let tx = fixture.worker_tx().await;
    let worker = fixture.worker.clone();
    let swarm = fixture.swarm.clone();
    let (release_tx, release_rx) = oneshot::channel();
    let completion = tokio::spawn(async move {
        release_rx.await.expect("completion barrier released");
        complete(path, &worker, &publication, failed, &tx, &swarm).await;
    });

    fixture.cancel().await;
    assert!(signal.is_set());
    // Model the real agent's RAII cleanup, which resets its interrupt signal
    // before the server completion callback. Cancellation must remain latched.
    drop(active);
    assert!(!signal.is_set());
    release_tx.send(()).unwrap();
    completion.await.unwrap();

    let members = fixture.swarm.members.read().await;
    assert_eq!(members[&fixture.worker].status, "stopped", "{path:?}");
    assert_eq!(members[&fixture.worker].latest_completion_report, None);
    assert_eq!(members[&fixture.stranger].status, "running");
    assert!(!stranger_signal.is_set());
    drop(members);
    let terminals = terminal_events(&mut fixture.worker_rx);
    assert!(
        matches!(
            terminals.as_slice(),
            [ServerEvent::TurnStopped {
                reason: TurnStopReason::Interrupted,
                ..
            }]
        ),
        "{path:?}: {terminals:?}"
    );
    while let Ok(event) = fixture.coordinator_rx.try_recv() {
        if let ServerEvent::Notification { message, .. } = event {
            assert!(!message.contains("partial output"));
            assert!(!message.contains("is ready for more"));
        }
    }
    assert!(terminal_events(&mut fixture.stranger_rx).is_empty());
    // An unrelated turn still owns its completion, not just its signal.
    let stranger_tx = fixture.swarm.members.read().await[&fixture.stranger]
        .event_tx
        .clone();
    complete(
        path,
        &fixture.stranger,
        &stranger_publication,
        false,
        &stranger_tx,
        &fixture.swarm,
    )
    .await;
    assert_eq!(
        fixture.swarm.members.read().await[&fixture.stranger].status,
        "ready"
    );
}

#[tokio::test]
async fn cancelled_headless_success_stays_stopped() {
    cancellation_then_completion(CompletionPath::Headless, false).await;
}
#[tokio::test]
async fn cancelled_live_success_has_only_interrupted_terminal() {
    cancellation_then_completion(CompletionPath::Live, false).await;
}
#[tokio::test]
async fn cancelled_client_success_has_only_interrupted_terminal() {
    cancellation_then_completion(CompletionPath::Client, false).await;
}
#[tokio::test]
async fn cancelled_headless_error_stays_stopped() {
    cancellation_then_completion(CompletionPath::Headless, true).await;
}
#[tokio::test]
async fn cancelled_live_error_has_only_interrupted_terminal() {
    cancellation_then_completion(CompletionPath::Live, true).await;
}
#[tokio::test]
async fn cancelled_client_error_has_only_interrupted_terminal() {
    cancellation_then_completion(CompletionPath::Client, true).await;
}

async fn new_generation_owns_completion(path: CompletionPath) {
    let mut fixture = Fixture::new();
    let old = begin_turn_publication(&fixture.worker).await;
    let active = crate::turn_cancel_registry::register_active_turn(
        &fixture.worker,
        jcode_agent_runtime::InterruptSignal::new(),
    );
    fixture.cancel().await;
    drop(active);
    let new = begin_turn_publication(&fixture.worker).await;
    super::update_member_status(
        &fixture.worker,
        "running",
        None,
        &fixture.swarm.members,
        &fixture.swarm.swarms_by_id,
        None,
        None,
        None,
    )
    .await;
    let tx = fixture.worker_tx().await;
    let _ = terminal_events(&mut fixture.worker_rx);
    complete(path, &fixture.worker, &old, false, &tx, &fixture.swarm).await;
    assert_eq!(
        fixture.swarm.members.read().await[&fixture.worker].status,
        "running"
    );
    assert!(terminal_events(&mut fixture.worker_rx).is_empty());
    complete(path, &fixture.worker, &new, false, &tx, &fixture.swarm).await;
    assert_eq!(
        fixture.swarm.members.read().await[&fixture.worker].status,
        "ready"
    );
    assert!(
        fixture.swarm.members.read().await[&fixture.worker]
            .latest_completion_report
            .is_some()
    );
    let terminals = terminal_events(&mut fixture.worker_rx);
    match path {
        CompletionPath::Headless => assert!(terminals.is_empty()),
        CompletionPath::Live => assert!(matches!(
            terminals.as_slice(),
            [ServerEvent::Done { id: 0 }]
        )),
        CompletionPath::Client => assert!(matches!(
            terminals.as_slice(),
            [ServerEvent::Done { id: 17 }]
        )),
    }
    complete(path, &fixture.worker, &new, true, &tx, &fixture.swarm).await;
    assert_eq!(
        fixture.swarm.members.read().await[&fixture.worker].status,
        "ready"
    );
    assert!(
        terminal_events(&mut fixture.worker_rx).is_empty(),
        "completion is single-owner"
    );
}

#[tokio::test]
async fn headless_successor_owns_completion() {
    new_generation_owns_completion(CompletionPath::Headless).await;
}
#[tokio::test]
async fn live_successor_owns_completion() {
    new_generation_owns_completion(CompletionPath::Live).await;
}
#[tokio::test]
async fn client_successor_owns_completion() {
    new_generation_owns_completion(CompletionPath::Client).await;
}

#[tokio::test]
async fn uncancelled_superseded_turn_cannot_publish() {
    let mut fixture = Fixture::new();
    let old = begin_turn_publication(&fixture.worker).await;
    let _new = begin_turn_publication(&fixture.worker).await;
    let tx = fixture.worker_tx().await;
    complete(
        CompletionPath::Live,
        &fixture.worker,
        &old,
        false,
        &tx,
        &fixture.swarm,
    )
    .await;
    assert_eq!(
        fixture.swarm.members.read().await[&fixture.worker].status,
        "running"
    );
    assert!(terminal_events(&mut fixture.worker_rx).is_empty());
}

#[tokio::test]
async fn uncancelled_failure_keeps_each_actual_error_path() {
    for path in [
        CompletionPath::Headless,
        CompletionPath::Live,
        CompletionPath::Client,
    ] {
        let mut fixture = Fixture::new();
        let publication = begin_turn_publication(&fixture.worker).await;
        let tx = fixture.worker_tx().await;
        complete(
            path,
            &fixture.worker,
            &publication,
            true,
            &tx,
            &fixture.swarm,
        )
        .await;
        let members = fixture.swarm.members.read().await;
        assert_eq!(members[&fixture.worker].status, "failed", "{path:?}");
        assert_eq!(members[&fixture.worker].latest_completion_report, None);
        drop(members);
        let events = terminal_events(&mut fixture.worker_rx);
        match path {
            CompletionPath::Headless => assert!(events.is_empty()),
            CompletionPath::Live | CompletionPath::Client => assert!(
                matches!(
                    events.as_slice(),
                    [
                        ServerEvent::TurnStopped {
                            reason: TurnStopReason::Failure,
                            ..
                        },
                        ServerEvent::Error { .. }
                    ]
                ),
                "{path:?}: {events:?}"
            ),
        }
    }
}

#[tokio::test]
async fn repeated_cascade_has_one_interrupted_terminal() {
    let mut fixture = Fixture::new();
    let _publication = begin_turn_publication(&fixture.worker).await;
    let _active = crate::turn_cancel_registry::register_active_turn(
        &fixture.worker,
        jcode_agent_runtime::InterruptSignal::new(),
    );
    fixture.cancel().await;
    let stopped = cancel_spawned_descendant_turns(
        &fixture.coordinator,
        &fixture.swarm.members,
        &fixture.swarm.swarms_by_id,
        None,
        None,
        None,
    )
    .await;
    assert!(stopped.is_empty());
    assert!(matches!(
        terminal_events(&mut fixture.worker_rx).as_slice(),
        [ServerEvent::TurnStopped { .. }]
    ));
}

#[tokio::test]
async fn completed_generation_cannot_be_retroactively_cancelled() {
    let mut fixture = Fixture::new();
    let publication = begin_turn_publication(&fixture.worker).await;
    let signal = jcode_agent_runtime::InterruptSignal::new();
    let _active =
        crate::turn_cancel_registry::register_active_turn(&fixture.worker, signal.clone());
    let tx = fixture.worker_tx().await;
    complete(
        CompletionPath::Live,
        &fixture.worker,
        &publication,
        false,
        &tx,
        &fixture.swarm,
    )
    .await;
    let stopped = cancel_spawned_descendant_turns(
        &fixture.coordinator,
        &fixture.swarm.members,
        &fixture.swarm.swarms_by_id,
        None,
        None,
        None,
    )
    .await;
    assert!(stopped.is_empty());
    assert!(!signal.is_set());
    assert_eq!(
        fixture.swarm.members.read().await[&fixture.worker].status,
        "ready"
    );
    assert!(matches!(
        terminal_events(&mut fixture.worker_rx).as_slice(),
        [ServerEvent::Done { id: 0 }]
    ));
}

#[tokio::test]
async fn completion_waits_for_inflight_cascade_publication() {
    let mut fixture = Fixture::new();
    let publication = begin_turn_publication(&fixture.worker).await;
    let signal = jcode_agent_runtime::InterruptSignal::new();
    let active = crate::turn_cancel_registry::register_active_turn(&fixture.worker, signal.clone());
    let tx = fixture.worker_tx().await;
    // Stop publication reaches the swarm broadcast lock after latching the
    // lease and emitting TurnStopped. Poll explicitly, without sleep/timing.
    let broadcast_gate = fixture.swarm.swarms_by_id.write().await;
    let mut cancel = Box::pin(fixture.cancel());
    assert!(futures::poll!(cancel.as_mut()).is_pending());
    assert!(signal.is_set());
    drop(active);
    let mut completion = Box::pin(complete(
        CompletionPath::Live,
        &fixture.worker,
        &publication,
        false,
        &tx,
        &fixture.swarm,
    ));
    assert!(futures::poll!(completion.as_mut()).is_pending());
    drop(broadcast_gate);
    cancel.await;
    completion.await;
    assert_eq!(
        fixture.swarm.members.read().await[&fixture.worker].status,
        "stopped"
    );
    assert!(matches!(
        terminal_events(&mut fixture.worker_rx).as_slice(),
        [ServerEvent::TurnStopped {
            reason: TurnStopReason::Interrupted,
            ..
        }]
    ));
}

#[tokio::test]
async fn cancellation_before_signal_registration_prevents_start() {
    for path in [
        CompletionPath::Headless,
        CompletionPath::Live,
        CompletionPath::Client,
    ] {
        let mut fixture = Fixture::new();
        let publication = begin_turn_publication(&fixture.worker).await;
        assert!(!crate::turn_cancel_registry::has_active_turn(
            &fixture.worker
        ));
        fixture.cancel().await;
        publication
            .publish_start(super::update_member_status(
                &fixture.worker,
                "running",
                None,
                &fixture.swarm.members,
                &fixture.swarm.swarms_by_id,
                None,
                None,
                None,
            ))
            .await;
        let polled = std::sync::atomic::AtomicBool::new(false);
        let result = publication
            .run(jcode_agent_runtime::InterruptSignal::new(), async {
                polled.store(true, Ordering::SeqCst);
                Ok::<(), anyhow::Error>(())
            })
            .await;
        assert!(result.is_none());
        assert!(
            !polled.load(Ordering::SeqCst),
            "cancelled turn must never start"
        );
        let tx = fixture.worker_tx().await;
        complete(
            path,
            &fixture.worker,
            &publication,
            false,
            &tx,
            &fixture.swarm,
        )
        .await;
        assert_eq!(
            fixture.swarm.members.read().await[&fixture.worker].status,
            "stopped"
        );
        assert!(matches!(
            terminal_events(&mut fixture.worker_rx).as_slice(),
            [ServerEvent::TurnStopped {
                reason: TurnStopReason::Interrupted,
                ..
            }]
        ));
    }
}

#[tokio::test]
async fn run_registers_signal_before_polling_turn_future() {
    let fixture = Fixture::new();
    let publication = begin_turn_publication(&fixture.worker).await;
    let signal = jcode_agent_runtime::InterruptSignal::new();
    let mut turn = Box::pin(publication.run(signal.clone(), async {
        assert!(
            crate::turn_cancel_registry::active_turn_signals(&fixture.worker)
                .iter()
                .any(|registered| registered.same_instance(&signal))
        );
        signal.notified().await;
    }));
    assert!(futures::poll!(turn.as_mut()).is_pending());
    fixture.cancel().await;
    assert!(turn.await.is_some());
    assert!(!signal.is_set(), "run guard resets its signal on exit");
    assert!(!crate::turn_cancel_registry::has_active_turn(
        &fixture.worker
    ));
}

#[tokio::test]
async fn captured_old_publication_cannot_cancel_successor() {
    let mut fixture = Fixture::new();
    let old = begin_turn_publication(&fixture.worker).await;
    let old_signal = jcode_agent_runtime::InterruptSignal::new();
    let _old_active =
        crate::turn_cancel_registry::register_active_turn(&fixture.worker, old_signal.clone());
    let state_gate = old.state.lock().await;
    let mut begin = Box::pin(begin_turn_publication(&fixture.worker));
    assert!(futures::poll!(begin.as_mut()).is_pending());
    let mut cancel = Box::pin(cancel_spawned_descendant_turns(
        &fixture.coordinator,
        &fixture.swarm.members,
        &fixture.swarm.swarms_by_id,
        None,
        None,
        None,
    ));
    // Captures the old Arc and queues behind begin at its state lock.
    assert!(futures::poll!(cancel.as_mut()).is_pending());
    drop(state_gate);
    let successor = begin.await;
    let successor_signal = jcode_agent_runtime::InterruptSignal::new();
    let _successor_active = crate::turn_cancel_registry::register_active_turn(
        &fixture.worker,
        successor_signal.clone(),
    );
    assert!(cancel.await.is_empty());
    assert!(!old_signal.is_set());
    assert!(!successor_signal.is_set());
    assert_eq!(
        fixture.swarm.members.read().await[&fixture.worker].status,
        "running"
    );
    assert!(terminal_events(&mut fixture.worker_rx).is_empty());
    let tx = fixture.worker_tx().await;
    complete(
        CompletionPath::Live,
        &fixture.worker,
        &successor,
        false,
        &tx,
        &fixture.swarm,
    )
    .await;
    assert_eq!(
        fixture.swarm.members.read().await[&fixture.worker].status,
        "ready"
    );
}

#[tokio::test]
async fn waiting_replacement_does_not_lock_unrelated_registry_entries() {
    let fixture = Fixture::new();
    let old = begin_turn_publication(&fixture.worker).await;
    let state_gate = old.state.lock().await;
    let mut replacement = Box::pin(begin_turn_publication(&fixture.worker));
    assert!(futures::poll!(replacement.as_mut()).is_pending());
    let mut unrelated = Box::pin(begin_turn_publication(&fixture.stranger));
    assert!(
        futures::poll!(unrelated.as_mut()).is_ready(),
        "registry lock must not span the per-turn wait"
    );
    drop(state_gate);
    replacement.await;
}

#[tokio::test]
async fn simultaneous_replacements_leave_one_terminal_owner() {
    let mut fixture = Fixture::new();
    let old = begin_turn_publication(&fixture.worker).await;
    let state_gate = old.state.lock().await;
    let mut first = Box::pin(begin_turn_publication(&fixture.worker));
    let mut second = Box::pin(begin_turn_publication(&fixture.worker));
    assert!(futures::poll!(first.as_mut()).is_pending());
    assert!(futures::poll!(second.as_mut()).is_pending());
    drop(state_gate);
    let first = first.await;
    let second = second.await;
    let tx = fixture.worker_tx().await;
    complete(
        CompletionPath::Live,
        &fixture.worker,
        &old,
        false,
        &tx,
        &fixture.swarm,
    )
    .await;
    complete(
        CompletionPath::Live,
        &fixture.worker,
        &first,
        false,
        &tx,
        &fixture.swarm,
    )
    .await;
    assert!(terminal_events(&mut fixture.worker_rx).is_empty());
    complete(
        CompletionPath::Live,
        &fixture.worker,
        &second,
        false,
        &tx,
        &fixture.swarm,
    )
    .await;
    assert!(matches!(
        terminal_events(&mut fixture.worker_rx).as_slice(),
        [ServerEvent::Done { id: 0 }]
    ));
}

#[derive(Clone)]
struct ResumeProvider(Arc<std::sync::atomic::AtomicUsize>);

#[async_trait::async_trait]
impl crate::provider::Provider for ResumeProvider {
    async fn complete(
        &self,
        _: &[crate::message::Message],
        _: &[crate::message::ToolDefinition],
        _: &str,
        _: Option<&str>,
    ) -> anyhow::Result<crate::provider::EventStream> {
        self.0.fetch_add(1, Ordering::SeqCst);
        Ok(Box::pin(futures::stream::iter(vec![
            Ok(crate::message::StreamEvent::TextDelta(
                "resumed successfully".into(),
            )),
            Ok(crate::message::StreamEvent::MessageEnd { stop_reason: None }),
        ])))
    }
    fn name(&self) -> &str {
        "resume-test"
    }
    fn fork(&self) -> Arc<dyn crate::provider::Provider> {
        Arc::new(self.clone())
    }
}

struct TestHome {
    previous: Option<std::ffi::OsString>,
    _home: tempfile::TempDir,
}

impl TestHome {
    fn new() -> Self {
        let home = tempfile::tempdir().unwrap();
        let previous = std::env::var_os("JCODE_HOME");
        crate::env::set_var("JCODE_HOME", home.path());
        Self {
            previous,
            _home: home,
        }
    }
}
impl Drop for TestHome {
    fn drop(&mut self) {
        if let Some(previous) = self.previous.take() {
            crate::env::set_var("JCODE_HOME", previous);
        } else {
            crate::env::remove_var("JCODE_HOME");
        }
    }
}

async fn resume_agent(
    session_id: &str,
) -> (
    Arc<tokio::sync::Mutex<crate::agent::Agent>>,
    Arc<std::sync::atomic::AtomicUsize>,
) {
    let calls = Arc::new(std::sync::atomic::AtomicUsize::new(0));
    let provider: Arc<dyn crate::provider::Provider> = Arc::new(ResumeProvider(calls.clone()));
    let registry = crate::tool::Registry::new(provider.clone()).await;
    let session = crate::session::Session::create_with_id(session_id.to_string(), None, None);
    let mut agent = crate::agent::Agent::new_with_session(provider, registry, session, None);
    agent
        .append_user_context_message("resume this turn", vec![])
        .unwrap();
    (Arc::new(tokio::sync::Mutex::new(agent)), calls)
}

fn usage_limit() -> anyhow::Error {
    jcode_provider_core::usage_limit_resume::with_usage_limit_reset(
        anyhow::anyhow!("Usage limit reached for this Claude account."),
        Some(std::time::Duration::from_secs(600)),
    )
}

#[tokio::test(start_paused = true)]
async fn cancelled_during_actual_usage_limit_wait_cannot_resume() {
    let _lock = crate::storage::lock_test_env();
    let _home = TestHome::new();
    let mut fixture = Fixture::new();
    let publication = begin_turn_publication(&fixture.worker).await;
    let (agent, calls) = resume_agent(&fixture.worker).await;
    let held = agent.clone().lock_owned().await;
    let tx = fixture.worker_tx().await;
    let mut resume = Box::pin(
        super::super::usage_limit_resume::resume_turn_after_usage_limit_with_publication(
            agent.clone(),
            Some(held),
            &fixture.worker,
            usage_limit(),
            None,
            &tx,
            &fixture.swarm,
            &super::super::usage_limit_resume::no_wait_hook,
            Some(&publication),
        ),
    );
    tokio::task::yield_now().await;
    assert!(futures::poll!(resume.as_mut()).is_pending());
    assert!(super::super::usage_limit_resume::has_pending_resume(
        &fixture.worker
    ));
    assert!(
        agent.try_lock().is_ok(),
        "reservation is released while waiting"
    );
    let _ = terminal_events(&mut fixture.worker_rx);
    fixture.cancel().await;
    assert!(matches!(
        resume.await,
        super::super::usage_limit_resume::ResumeOutcome::Superseded
    ));
    tokio::time::advance(std::time::Duration::from_secs(10_000)).await;
    assert_eq!(calls.load(Ordering::SeqCst), 0);
    assert!(!super::super::usage_limit_resume::has_pending_resume(
        &fixture.worker
    ));
    assert!(
        agent.try_lock().is_ok(),
        "cancelled retry releases its reservation"
    );
    assert_eq!(
        fixture.swarm.members.read().await[&fixture.worker].status,
        "stopped"
    );
    assert!(matches!(
        terminal_events(&mut fixture.worker_rx).as_slice(),
        [ServerEvent::TurnStopped {
            reason: TurnStopReason::Interrupted,
            ..
        }]
    ));
}

#[tokio::test(start_paused = true)]
async fn superseded_actual_usage_limit_wait_cannot_restart_successor() {
    let _lock = crate::storage::lock_test_env();
    let _home = TestHome::new();
    let fixture = Fixture::new();
    let old = begin_turn_publication(&fixture.worker).await;
    let (agent, calls) = resume_agent(&fixture.worker).await;
    let held = agent.clone().lock_owned().await;
    let tx = fixture.worker_tx().await;
    let mut resume = Box::pin(
        super::super::usage_limit_resume::resume_turn_after_usage_limit_with_publication(
            agent.clone(),
            Some(held),
            &fixture.worker,
            usage_limit(),
            None,
            &tx,
            &fixture.swarm,
            &super::super::usage_limit_resume::no_wait_hook,
            Some(&old),
        ),
    );
    tokio::task::yield_now().await;
    assert!(futures::poll!(resume.as_mut()).is_pending());
    assert!(super::super::usage_limit_resume::has_pending_resume(
        &fixture.worker
    ));
    let successor = begin_turn_publication(&fixture.worker).await;
    successor
        .publish_start(super::update_member_status(
            &fixture.worker,
            "running",
            None,
            &fixture.swarm.members,
            &fixture.swarm.swarms_by_id,
            None,
            None,
            None,
        ))
        .await;
    assert!(matches!(
        resume.await,
        super::super::usage_limit_resume::ResumeOutcome::Superseded
    ));
    tokio::time::advance(std::time::Duration::from_secs(10_000)).await;
    assert_eq!(calls.load(Ordering::SeqCst), 0);
    assert!(!super::super::usage_limit_resume::has_pending_resume(
        &fixture.worker
    ));
    assert_eq!(
        fixture.swarm.members.read().await[&fixture.worker].status,
        "running"
    );
}

#[tokio::test(start_paused = true)]
async fn uncancelled_actual_usage_limit_wait_resumes_with_reservation() {
    let _lock = crate::storage::lock_test_env();
    let _home = TestHome::new();
    let fixture = Fixture::new();
    let publication = begin_turn_publication(&fixture.worker).await;
    let (agent, calls) = resume_agent(&fixture.worker).await;
    let held = agent.clone().lock_owned().await;
    let tx = fixture.worker_tx().await;
    let mut resume = Box::pin(
        super::super::usage_limit_resume::resume_turn_after_usage_limit_with_publication(
            agent.clone(),
            Some(held),
            &fixture.worker,
            usage_limit(),
            None,
            &tx,
            &fixture.swarm,
            &super::super::usage_limit_resume::no_wait_hook,
            Some(&publication),
        ),
    );
    tokio::task::yield_now().await;
    assert!(futures::poll!(resume.as_mut()).is_pending());
    assert!(super::super::usage_limit_resume::has_pending_resume(
        &fixture.worker
    ));
    tokio::time::advance(std::time::Duration::from_secs(10_000)).await;
    let outcome = resume.await;
    let super::super::usage_limit_resume::ResumeOutcome::Completed {
        guard,
        completion_report,
    } = outcome
    else {
        panic!("uncancelled retry should complete");
    };
    assert_eq!(calls.load(Ordering::SeqCst), 1);
    assert_eq!(completion_report.as_deref(), Some("resumed successfully"));
    assert!(
        agent.try_lock().is_err(),
        "caller retains terminal-publication reservation"
    );
    drop(guard);
    assert!(agent.try_lock().is_ok());
    assert!(!super::super::usage_limit_resume::has_pending_resume(
        &fixture.worker
    ));
}
