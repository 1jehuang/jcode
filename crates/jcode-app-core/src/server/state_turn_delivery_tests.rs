#![allow(clippy::await_holding_lock)]
use super::*;

// The pre-recovery helper, not the compatibility wrapper around the new
// Result-returning helper. Keeping this separate exposes large-Result return
// overhead rather than accidentally charging it to both measurements.
fn legacy_send_for_probe<'a>(
    targets: impl IntoIterator<Item = &'a mpsc::UnboundedSender<ServerEvent>>,
    event: ServerEvent,
) -> usize {
    let mut targets = targets.into_iter().filter(|tx| !tx.is_closed()).peekable();
    let mut delivered = 0;
    while let Some(tx) = targets.next() {
        if targets.peek().is_none() {
            delivered += usize::from(tx.send(event).is_ok());
            return delivered;
        }
        delivered += usize::from(tx.send(event.clone()).is_ok());
    }
    delivered
}

async fn legacy_session_fanout_for_probe(
    members: &Arc<RwLock<HashMap<String, SwarmMember>>>,
    session_id: &str,
    event: ServerEvent,
) -> usize {
    super::super::live_status::observe(session_id, &event);
    let targets = {
        let mut members = members.write().await;
        let Some(member) = members.get_mut(session_id) else {
            return 0;
        };
        member.event_txs.retain(|_, tx| !tx.is_closed());
        crate::stream_batching::note_session_targets(session_id, member.event_txs.keys());
        if member.event_txs.is_empty() {
            RecipientSnapshot::One(member.event_tx.clone())
        } else {
            if let Some((_, tx)) = member.event_txs.iter().next() {
                member.event_tx = tx.clone();
            }
            member
                .event_txs
                .values()
                .cloned()
                .collect::<RecipientSnapshot>()
        }
    };
    legacy_send_for_probe(targets.as_slice(), event)
}

#[tokio::test]
#[ignore = "manual alternating timing probe, not a timing assertion"]
async fn legacy_clone_fallback_vs_recover_fallback_microprobe() {
    const EVENTS: usize = 4_000;
    const ROUNDS: usize = 8;
    for bytes in [24, 64 * 1024] {
        for full_session in [false, true] {
            let session_id = format!("turn-forward-probe-{bytes}-{full_session}");
            let (target, mut receiver) = mpsc::unbounded_channel();
            let (fallback, mut fallback_receiver) = mpsc::unbounded_channel();
            let mut member = member(&session_id, target.clone());
            member
                .event_txs
                .insert("probe-attachment".into(), target.clone());
            let members = Arc::new(RwLock::new(HashMap::from([(session_id.clone(), member)])));
            let template = wrapped("x".repeat(bytes));
            let mut legacy_samples = Vec::new();
            let mut recover_samples = Vec::new();
            // Include a complete discarded warmup pair. Alternate order each
            // round so allocator/cache/host drift does not favor one variant.
            for round in 0..=ROUNDS {
                for legacy in [round % 2 == 0, round % 2 != 0] {
                    // Producer payload construction is outside the interval:
                    // measure forwarding, its necessary clones, and queue drain.
                    let events: Vec<_> = (0..EVENTS).map(|_| template.clone()).collect();
                    let started = Instant::now();
                    for event in events {
                        if legacy {
                            let count = if full_session {
                                legacy_session_fanout_for_probe(
                                    &members,
                                    &session_id,
                                    event.clone(),
                                )
                                .await
                            } else {
                                legacy_send_for_probe([&target], event.clone())
                            };
                            if count == 0 {
                                fallback.send(event).unwrap();
                            }
                        } else {
                            let result = if full_session {
                                fanout_session_event_recover(&members, &session_id, event).await
                            } else {
                                send_event_to_targets_recover([&target], event)
                            };
                            if let Err(event) = result {
                                fallback.send(*event).unwrap();
                            }
                        }
                        // Identical drain for both variants, with no unbounded
                        // accumulation or task scheduling in the timed region.
                        std::hint::black_box(receiver.try_recv().unwrap());
                    }
                    let elapsed = started.elapsed();
                    assert!(fallback_receiver.try_recv().is_err());
                    if round != 0 {
                        if legacy {
                            legacy_samples.push(elapsed);
                        } else {
                            recover_samples.push(elapsed);
                        }
                    }
                }
            }
            legacy_samples.sort_unstable();
            recover_samples.sort_unstable();
            let legacy_ns = legacy_samples[ROUNDS / 2].as_nanos() / EVENTS as u128;
            let recover_ns = recover_samples[ROUNDS / 2].as_nanos() / EVENTS as u128;
            eprintln!(
                "turn normal-forward microprobe profile=test payload_bytes={bytes} full_session={full_session} recipients=1 events_per_sample={EVENTS} rounds={ROUNDS} upper_median_legacy_ns={legacy_ns} upper_median_recover_ns={recover_ns} recover_over_legacy={:.3} legacy_samples={legacy_samples:?} recover_samples={recover_samples:?}",
                recover_ns as f64 / legacy_ns as f64,
            );
        }
    }
}

fn wrapped(text: String) -> ServerEvent {
    ServerEvent::TurnEvent {
        identity: crate::protocol::TurnIdentity {
            runtime_generation: "turn-delivery-runtime".into(),
            turn_id: 42,
        },
        event: Box::new(ServerEvent::TextDelta { text }),
    }
}

fn payload(event: &ServerEvent) -> &str {
    match event {
        ServerEvent::TurnEvent { identity, event } => {
            assert_eq!(identity.turn_id, 42);
            assert_eq!(identity.runtime_generation, "turn-delivery-runtime");
            match event.as_ref() {
                ServerEvent::TextDelta { text } => text,
                other => panic!("expected wrapped text, got {other:?}"),
            }
        }
        other => panic!("expected turn wrapper, got {other:?}"),
    }
}

fn large_event() -> ServerEvent {
    wrapped("large turn payload ".repeat(16_384))
}

#[test]
fn recover_zero_and_closed_targets_returns_original_wrapped_payload() {
    for count in 0..=4 {
        let senders: Vec<_> = (0..count)
            .map(|_| {
                let (tx, rx) = mpsc::unbounded_channel();
                drop(rx);
                tx
            })
            .collect();
        let event = large_event();
        let original = payload(&event).as_ptr();
        let returned = send_event_to_targets_recover(&senders, event).unwrap_err();
        assert_eq!(payload(&returned).as_ptr(), original);
        assert_eq!(send_event_to_targets(&senders, large_event()), 0);
    }
}

#[test]
fn recover_live_targets_preserves_count_order_and_final_recipient_ownership() {
    for count in 1..=4 {
        let channels: Vec<_> = (0..count).map(|_| mpsc::unbounded_channel()).collect();
        let (senders, mut receivers): (Vec<_>, Vec<_>) = channels.into_iter().unzip();
        let (closed, closed_rx) = mpsc::unbounded_channel();
        drop(closed_rx);
        // Closed targets before, between and after live ones must not change
        // which live recipient gets the original backing allocation.
        let mut targets = vec![&closed];
        for tx in &senders {
            targets.extend([tx, &closed]);
        }
        for sequence in 0..3 {
            let event = wrapped(format!("{sequence}:{}", "payload".repeat(16_384)));
            let expected = payload(&event).to_owned();
            let original = payload(&event).as_ptr();
            assert_eq!(
                send_event_to_targets_recover(targets.iter().copied(), event).unwrap(),
                count
            );
            let received: Vec<_> = receivers
                .iter_mut()
                .map(|rx| rx.try_recv().unwrap())
                .collect();
            for (index, event) in received.iter().enumerate() {
                assert_eq!(payload(event), expected);
                if index + 1 == count {
                    assert_eq!(payload(event).as_ptr(), original);
                } else {
                    assert_ne!(payload(event).as_ptr(), original);
                }
            }
        }
        assert!(receivers.iter_mut().all(|rx| rx.try_recv().is_err()));
    }
}

#[test]
fn recover_returns_original_when_final_target_closes_after_liveness_check() {
    let (tx, rx) = mpsc::unbounded_channel();
    let mut receiver = Some(rx);
    let mut yielded = false;
    // peek() asks for the next target after is_closed() accepted this one.
    // Closing here deterministically exercises send failure, not prefiltering.
    let targets = std::iter::from_fn(|| {
        if !yielded {
            yielded = true;
            Some(&tx)
        } else {
            drop(receiver.take());
            None
        }
    });
    let event = large_event();
    let original = payload(&event).as_ptr();
    let returned = send_event_to_targets_recover(targets, event).unwrap_err();
    assert!(tx.is_closed());
    assert_eq!(payload(&returned).as_ptr(), original);
}

#[test]
fn recover_partial_success_does_not_trigger_fallback_after_final_send_races_closed() {
    let (first, mut first_rx) = mpsc::unbounded_channel();
    let (last, last_rx) = mpsc::unbounded_channel();
    let mut last_rx = Some(last_rx);
    let mut index = 0;
    let targets = std::iter::from_fn(|| {
        index += 1;
        match index {
            1 => Some(&first),
            2 => Some(&last),
            _ => {
                drop(last_rx.take());
                None
            }
        }
    });
    let event = large_event();
    let expected = payload(&event).to_owned();
    assert_eq!(send_event_to_targets_recover(targets, event).unwrap(), 1);
    assert_eq!(payload(&first_rx.try_recv().unwrap()), expected);
    assert!(first_rx.try_recv().is_err());
    assert!(last.is_closed());
}

#[tokio::test]
async fn missing_session_recover_returns_original_wrapped_payload() {
    let members = Arc::new(RwLock::new(HashMap::new()));
    let event = large_event();
    let original = payload(&event).as_ptr();
    let returned = fanout_session_event_recover(&members, "turn-delivery-missing", event)
        .await
        .unwrap_err();
    assert_eq!(payload(&returned).as_ptr(), original);
    assert!(members.read().await.is_empty());
}

#[test]
fn thousand_wrapped_events_move_original_backing_instead_of_legacy_clone() {
    let (tx, mut rx) = mpsc::unbounded_channel();
    let mut moved_originals = 0;
    let mut legacy_clones = 0;
    for _ in 0..1000 {
        let event = large_event();
        let original = payload(&event).as_ptr();
        // Retain the original exactly as the former fallback implementation
        // did, making pointer inequality deterministic rather than allocator reuse.
        assert_eq!(
            send_event_to_targets_recover([&tx], event.clone()).unwrap(),
            1
        );
        let legacy = rx.try_recv().unwrap();
        legacy_clones += usize::from(payload(&legacy).as_ptr() != original);
        assert_eq!(payload(&legacy), payload(&event));
        assert_eq!(send_event_to_targets_recover([&tx], event).unwrap(), 1);
        let moved = rx.try_recv().unwrap();
        moved_originals += usize::from(payload(&moved).as_ptr() == original);
        assert_eq!(payload(&moved), payload(&legacy));
    }
    // Structural allocation ownership evidence, not a timing or allocator claim.
    assert_eq!(legacy_clones, 1000);
    assert_eq!(moved_originals, 1000);
    assert!(rx.try_recv().is_err());
}

#[derive(Clone)]
struct UnusedProvider;

#[async_trait::async_trait]
impl Provider for UnusedProvider {
    async fn complete(
        &self,
        _messages: &[crate::message::Message],
        _tools: &[crate::message::ToolDefinition],
        _system: &str,
        _resume_session_id: Option<&str>,
    ) -> anyhow::Result<crate::provider::EventStream> {
        panic!("delivery tests must not invoke a provider")
    }

    fn name(&self) -> &str {
        "turn-delivery-test"
    }

    fn fork(&self) -> Arc<dyn Provider> {
        Arc::new(self.clone())
    }
}

// Match the client_lifecycle_tests lease fixture without accessing its private
// items or mutating its source. Restore the environment even after a panic.
struct IsolatedHome {
    previous: Option<std::ffi::OsString>,
    _directory: tempfile::TempDir,
}

impl IsolatedHome {
    fn new() -> Self {
        let directory = tempfile::TempDir::new().unwrap();
        let previous = std::env::var_os("JCODE_HOME");
        crate::env::set_var("JCODE_HOME", directory.path());
        Self {
            previous,
            _directory: directory,
        }
    }
}

impl Drop for IsolatedHome {
    fn drop(&mut self) {
        if let Some(previous) = self.previous.take() {
            crate::env::set_var("JCODE_HOME", previous);
        } else {
            crate::env::remove_var("JCODE_HOME");
        }
    }
}

fn member(session_id: &str, event_tx: mpsc::UnboundedSender<ServerEvent>) -> SwarmMember {
    SwarmMember {
        session_id: session_id.into(),
        event_tx,
        event_txs: HashMap::new(),
        working_dir: None,
        swarm_id: None,
        swarm_enabled: false,
        status: "ready".into(),
        detail: None,
        task_label: None,
        friendly_name: None,
        report_back_to_session_id: None,
        latest_completion_report: None,
        role: "agent".into(),
        joined_at: Instant::now(),
        last_status_change: Instant::now(),
        is_headless: false,
        output_tail: None,
        todo_progress: None,
        todo_items: Vec::new(),
        runtime: crate::protocol::SwarmMemberRuntime::default(),
    }
}

#[tokio::test]
async fn turn_channel_fallback_and_attached_delivery_move_live_and_drained_payloads_once() {
    let _guard = crate::storage::lock_test_env();
    let _home = IsolatedHome::new();
    let provider: Arc<dyn Provider> = Arc::new(UnusedProvider);
    let registry = crate::tool::Registry::new(Arc::clone(&provider)).await;
    let agent = crate::agent::Agent::new(provider, registry);
    for attached in [false, true] {
        let session_id = format!("turn-delivery-channel-{attached}");
        let members = Arc::new(RwLock::new(HashMap::new()));
        let (fallback, mut fallback_rx) = mpsc::unbounded_channel();
        let (attached_tx, mut attached_rx) = mpsc::unbounded_channel();
        if attached {
            let mut member = member(&session_id, attached_tx.clone());
            member
                .event_txs
                .insert("attached".into(), attached_tx.clone());
            members.write().await.insert(session_id.clone(), member);
        }
        let lease = agent.claim_server_turn().unwrap();
        let identity = lease.identity().clone();
        let channel = TurnEventChannel::new(session_id, Arc::clone(&members), fallback, &lease);
        let sender = channel.sender();
        let output = if attached {
            &mut attached_rx
        } else {
            &mut fallback_rx
        };
        let started = tokio::time::timeout(std::time::Duration::from_secs(2), output.recv())
            .await
            .unwrap()
            .unwrap();
        assert!(
            matches!(started, ServerEvent::TurnStarted { identity: actual } if actual == identity)
        );

        let live = "live payload ".repeat(16_384);
        let live_pointer = live.as_ptr();
        sender.send(ServerEvent::TextDelta { text: live }).unwrap();
        let delivered = tokio::time::timeout(std::time::Duration::from_secs(2), output.recv())
            .await
            .unwrap()
            .unwrap();
        match delivered {
            ServerEvent::TurnEvent {
                identity: actual,
                event,
            } => {
                assert_eq!(actual, identity);
                assert!(
                    matches!(*event, ServerEvent::TextDelta { text } if text.as_ptr() == live_pointer)
                );
            }
            other => panic!("unexpected live event {other:?}"),
        }

        // Hold the fanout lock so finish() closes the producer queue before it
        // can drain. A retained producer sender must not keep finish blocked.
        let blocked = members.write().await;
        let mut pointers = Vec::new();
        for index in 0..8 {
            let text = format!("drain-{index}:{}", "payload".repeat(16_384));
            pointers.push(text.as_ptr());
            sender.send(ServerEvent::TextDelta { text }).unwrap();
        }
        sender.send(ServerEvent::Done { id: 99 }).unwrap();
        drop(lease);
        assert!(
            agent.claim_server_turn().is_err(),
            "publication pin must reserve runtime"
        );
        let finishing = tokio::spawn(channel.finish());
        tokio::task::yield_now().await;
        drop(blocked);
        tokio::time::timeout(std::time::Duration::from_secs(2), finishing)
            .await
            .unwrap()
            .unwrap();
        for (index, pointer) in pointers.into_iter().enumerate() {
            match output.try_recv().unwrap() {
                ServerEvent::TurnEvent {
                    identity: actual,
                    event,
                } => {
                    assert_eq!(actual, identity);
                    match *event {
                        ServerEvent::TextDelta { text } => {
                            assert!(text.starts_with(&format!("drain-{index}:")));
                            assert_eq!(text.as_ptr(), pointer);
                        }
                        other => panic!("unexpected drained payload {other:?}"),
                    }
                }
                other => panic!("unexpected drained event {other:?}"),
            }
        }
        assert!(
            matches!(output.try_recv().unwrap(), ServerEvent::TurnEvent { event, .. } if matches!(*event, ServerEvent::Done { id: 99 }))
        );
        assert!(output.try_recv().is_err());
        assert!(sender.send(ServerEvent::Done { id: 100 }).is_err());
        assert!(agent.claim_server_turn().is_ok());
        if attached {
            assert!(
                fallback_rx.try_recv().is_err(),
                "attached delivery must not duplicate to fallback"
            );
        }
    }
}
