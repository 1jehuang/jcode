use super::*;

#[test]
fn thousand_snapshots_avoid_single_recipient_heap_storage() {
    // Structural accounting, not a global allocator measurement: every nonempty
    // Vec owns heap storage, whereas Empty and One have no recipient allocation.
    // This deliberately excludes channel internals and payload allocations.
    for count in [0, 1, 4] {
        let channels: Vec<_> = (0..count)
            .map(|_| mpsc::unbounded_channel::<ServerEvent>())
            .collect();
        let senders: Vec<_> = channels.iter().map(|(tx, _)| tx.clone()).collect();
        let mut vec_heap_owners = 0;
        let mut snapshot_heap_owners = 0;
        for _ in 0..1000 {
            let original = std::hint::black_box(senders.to_vec());
            vec_heap_owners += usize::from(original.capacity() > 0);
            let snapshot =
                std::hint::black_box(senders.iter().cloned().collect::<RecipientSnapshot>());
            snapshot_heap_owners += usize::from(matches!(&snapshot, RecipientSnapshot::Many(_)));
            assert_eq!(original.len(), snapshot.as_slice().len());
        }
        assert_eq!(vec_heap_owners, if count == 0 { 0 } else { 1000 });
        assert_eq!(snapshot_heap_owners, if count > 1 { 1000 } else { 0 });
        eprintln!(
            "structural recipient storage: attachments={count} snapshots=1000 original_heap_owners={vec_heap_owners} new_heap_owners={snapshot_heap_owners}"
        );
    }
}

// Exact pre-optimization session fanout, retained only as a timing baseline.
async fn original_session_fanout(members: &Members, session_id: &str, event: ServerEvent) -> usize {
    super::super::live_status::observe(session_id, &event);
    let targets = {
        let mut members = members.write().await;
        let Some(member) = members.get_mut(session_id) else {
            return 0;
        };
        member.event_txs.retain(|_, tx| !tx.is_closed());
        crate::stream_batching::note_session_targets(session_id, member.event_txs.keys());
        if member.event_txs.is_empty() {
            vec![member.event_tx.clone()]
        } else {
            if let Some((_, tx)) = member.event_txs.iter().next() {
                member.event_tx = tx.clone();
            }
            member.event_txs.values().cloned().collect::<Vec<_>>()
        }
    };
    send_event_to_targets(&targets, event)
}

#[tokio::test]
#[ignore = "manual comparative timing probe, not a timing assertion"]
async fn original_vs_new_full_session_fanout_microprobe() {
    const EVENTS: usize = 10_000;
    const ROUNDS: usize = 8;
    for count in [1, 4] {
        let session_id = format!("fanout-microprobe-{count}");
        let (mut member, _primary_rx) = swarm_member(&session_id);
        let mut receivers = Vec::new();
        for index in 0..count {
            let (tx, rx) = mpsc::unbounded_channel();
            member
                .event_txs
                .insert(format!("microprobe-{count}-{index}"), tx);
            receivers.push(rx);
        }
        member.event_tx = member.event_txs.values().next().unwrap().clone();
        let members = members(member);
        // Warm both code paths and channel blocks before measuring.
        for original in [true, false] {
            for _ in 0..100 {
                if original {
                    assert_eq!(
                        original_session_fanout(&members, &session_id, event("delta")).await,
                        count
                    );
                } else {
                    assert_eq!(
                        fanout_session_event(&members, &session_id, event("delta")).await,
                        count
                    );
                }
                for rx in &mut receivers {
                    assert_text(rx, "delta");
                }
            }
        }
        let mut original_samples = Vec::new();
        let mut new_samples = Vec::new();
        for round in 0..ROUNDS {
            // Alternate which implementation gets the first measurement.
            for original in [round % 2 == 0, round % 2 != 0] {
                let started = Instant::now();
                for _ in 0..EVENTS {
                    let delivered = if original {
                        original_session_fanout(&members, &session_id, event("delta")).await
                    } else {
                        fanout_session_event(&members, &session_id, event("delta")).await
                    };
                    assert_eq!(delivered, count);
                    // Match bounded steady-state delivery, never queue a burst.
                    for rx in &mut receivers {
                        assert_text(rx, "delta");
                    }
                }
                let elapsed = started.elapsed();
                if original {
                    original_samples.push(elapsed);
                } else {
                    new_samples.push(elapsed);
                }
            }
        }
        original_samples.sort_unstable();
        new_samples.sort_unstable();
        let original_ns = original_samples[ROUNDS / 2].as_nanos() / EVENTS as u128;
        let new_ns = new_samples[ROUNDS / 2].as_nanos() / EVENTS as u128;
        eprintln!(
            "full session fanout microprobe attachments={count} events_per_sample={EVENTS} rounds={ROUNDS} upper_median_original_ns_per_event={original_ns} upper_median_new_ns_per_event={new_ns} original_samples={original_samples:?} new_samples={new_samples:?}"
        );
    }
}

type Members = Arc<RwLock<HashMap<String, SwarmMember>>>;

// Keep this fixture local so these transport tests do not depend on swarm tests.
fn swarm_member(session_id: &str) -> (SwarmMember, mpsc::UnboundedReceiver<ServerEvent>) {
    let (event_tx, event_rx) = mpsc::unbounded_channel();
    (
        SwarmMember {
            session_id: session_id.to_string(),
            event_tx,
            event_txs: HashMap::new(),
            working_dir: None,
            swarm_id: Some("swarm-1".to_string()),
            swarm_enabled: true,
            status: "ready".to_string(),
            detail: None,
            task_label: None,
            friendly_name: Some(session_id.to_string()),
            report_back_to_session_id: None,
            latest_completion_report: None,
            role: "agent".to_string(),
            joined_at: Instant::now(),
            last_status_change: Instant::now(),
            is_headless: false,
            output_tail: None,
            todo_progress: None,
            todo_items: Vec::new(),
            runtime: crate::protocol::SwarmMemberRuntime::default(),
        },
        event_rx,
    )
}

fn members(member: SwarmMember) -> Members {
    Arc::new(RwLock::new(HashMap::from([(
        member.session_id.clone(),
        member,
    )])))
}

fn event(text: &str) -> ServerEvent {
    ServerEvent::TextDelta { text: text.into() }
}

fn assert_text(rx: &mut mpsc::UnboundedReceiver<ServerEvent>, expected: &str) {
    match rx.try_recv().expect("fanout must enqueue before returning") {
        ServerEvent::TextDelta { text } => assert_eq!(text, expected),
        other => panic!("unexpected event: {other:?}"),
    }
}

#[test]
fn recipient_snapshot_shapes_preserve_vec_delivery_equivalence() {
    for count in 0..=4 {
        let channels: Vec<_> = (0..count).map(|_| mpsc::unbounded_channel()).collect();
        let (senders, mut receivers): (Vec<_>, Vec<_>) = channels.into_iter().unzip();
        let snapshot: RecipientSnapshot = senders.iter().cloned().collect();
        match (&snapshot, count) {
            (RecipientSnapshot::Empty, 0) | (RecipientSnapshot::One(_), 1) => {}
            (RecipientSnapshot::Many(targets), n) if n > 1 => assert_eq!(targets.len(), n),
            _ => panic!("incorrect snapshot shape for {count} recipients"),
        }
        assert_eq!(snapshot.as_slice().len(), count);
        for (actual, expected) in snapshot.as_slice().iter().zip(&senders) {
            assert!(actual.same_channel(expected));
        }
        // Peer fanout filters connections, so its iterator has lower size hint
        // zero even when several recipients survive. Preserve shape and order.
        let filtered: RecipientSnapshot = senders.iter().filter(|_| true).cloned().collect();
        match (&filtered, count) {
            (RecipientSnapshot::Empty, 0) | (RecipientSnapshot::One(_), 1) => {}
            (RecipientSnapshot::Many(targets), n) if n > 1 => {
                assert_eq!(targets.len(), n);
                assert!(targets.capacity() >= 4);
            }
            _ => panic!("incorrect filtered snapshot shape for {count} recipients"),
        }
        for (actual, expected) in filtered.as_slice().iter().zip(&senders) {
            assert!(actual.same_channel(expected));
        }
        assert_eq!(send_event_to_targets(&senders, event("vec")), count);
        assert_eq!(
            send_event_to_targets(snapshot.as_slice(), event("snapshot")),
            count
        );
        for rx in &mut receivers {
            assert_text(rx, "vec");
            assert_text(rx, "snapshot");
            assert!(rx.try_recv().is_err());
        }
    }
}

#[tokio::test]
async fn all_fanouts_handle_missing_sessions_and_zero_attachments() {
    let empty = Arc::new(RwLock::new(HashMap::new()));
    assert_eq!(
        fanout_session_event(&empty, "missing", event("session")).await,
        0
    );
    assert_eq!(
        fanout_live_client_event(&empty, "missing", event("live")).await,
        0
    );
    assert_eq!(
        fanout_peer_client_event(&empty, "missing", "origin", event("peer")).await,
        0
    );
    let (member, mut primary) = swarm_member("fanout-zero");
    let members = members(member);
    assert_eq!(
        fanout_live_client_event(&members, "fanout-zero", event("live")).await,
        0
    );
    assert_eq!(
        fanout_peer_client_event(&members, "fanout-zero", "origin", event("peer")).await,
        0
    );
    assert!(primary.try_recv().is_err());
    assert_eq!(
        fanout_session_event(&members, "fanout-zero", event("fallback")).await,
        1
    );
    assert_text(&mut primary, "fallback");
    drop(primary);
    assert_eq!(
        fanout_session_event(&members, "fanout-zero", event("closed")).await,
        0
    );
}

#[tokio::test]
async fn fanouts_preserve_order_counts_and_peer_exclusion() {
    for count in 1..=3 {
        let session_id = format!("fanout-order-{count}");
        let (mut member, mut stale_primary) = swarm_member(&session_id);
        let mut receivers = Vec::new();
        for index in 0..count {
            let (tx, rx) = mpsc::unbounded_channel();
            member.event_txs.insert(format!("client-{index}"), tx);
            receivers.push(rx);
        }
        let members = members(member);
        assert_eq!(
            fanout_session_event(&members, &session_id, event("session-1")).await,
            count
        );
        assert_eq!(
            fanout_live_client_event(&members, &session_id, event("live")).await,
            count
        );
        assert_eq!(
            fanout_peer_client_event(&members, &session_id, "client-0", event("peer")).await,
            count - 1
        );
        assert_eq!(
            fanout_peer_client_event(&members, &session_id, "not-attached", event("all-peers"))
                .await,
            count
        );
        assert_eq!(
            fanout_session_event(&members, &session_id, event("session-2")).await,
            count
        );
        for (index, rx) in receivers.iter_mut().enumerate() {
            assert_text(rx, "session-1");
            assert_text(rx, "live");
            if index != 0 {
                assert_text(rx, "peer");
            }
            assert_text(rx, "all-peers");
            assert_text(rx, "session-2");
            assert!(rx.try_recv().is_err());
        }
        assert!(
            stale_primary.try_recv().is_err(),
            "attachments replace primary, not duplicate it"
        );
        let guard = members.read().await;
        let member = &guard[&session_id];
        assert!(
            member
                .event_tx
                .same_channel(member.event_txs.values().next().unwrap())
        );
    }
}

#[tokio::test]
async fn maintenance_prunes_closed_targets_and_refreshes_stale_primary() {
    for kind in 0..3 {
        let session_id = format!("fanout-maintenance-{kind}");
        let (mut member, stale_primary) = swarm_member(&session_id);
        drop(stale_primary);
        let (closed, closed_rx) = mpsc::unbounded_channel();
        drop(closed_rx);
        let (live, mut live_rx) = mpsc::unbounded_channel();
        member.event_txs.insert("closed".into(), closed);
        member.event_txs.insert("live".into(), live.clone());
        let members = members(member);
        let delivered = match kind {
            0 => fanout_session_event(&members, &session_id, event("maintenance")).await,
            1 => fanout_live_client_event(&members, &session_id, event("maintenance")).await,
            _ => {
                fanout_peer_client_event(&members, &session_id, "closed", event("maintenance"))
                    .await
            }
        };
        assert_eq!(delivered, 1);
        assert_text(&mut live_rx, "maintenance");
        {
            let guard = members.read().await;
            assert_eq!(guard[&session_id].event_txs.len(), 1);
            assert!(!guard[&session_id].event_txs.contains_key("closed"));
        }
        assert_eq!(
            fanout_session_event(&members, &session_id, event("refresh")).await,
            1
        );
        assert_text(&mut live_rx, "refresh");
        assert!(
            members.read().await[&session_id]
                .event_tx
                .same_channel(&live)
        );
        members
            .write()
            .await
            .get_mut(&session_id)
            .unwrap()
            .event_txs
            .clear();
        assert_eq!(
            fanout_session_event(&members, &session_id, event("refreshed-fallback")).await,
            1
        );
        assert_text(&mut live_rx, "refreshed-fallback");
    }
}

#[tokio::test]
async fn all_closed_attachments_use_only_session_primary_fallback() {
    let (mut member, mut primary) = swarm_member("fanout-closed-fallback");
    let (closed, closed_rx) = mpsc::unbounded_channel();
    drop(closed_rx);
    member.event_txs.insert("closed".into(), closed);
    let members = members(member);
    assert_eq!(
        fanout_session_event(&members, "fanout-closed-fallback", event("fallback")).await,
        1
    );
    assert_text(&mut primary, "fallback");
    assert!(
        members.read().await["fanout-closed-fallback"]
            .event_txs
            .is_empty()
    );
    assert_eq!(
        fanout_live_client_event(&members, "fanout-closed-fallback", event("live")).await,
        0
    );
    assert_eq!(
        fanout_peer_client_event(&members, "fanout-closed-fallback", "closed", event("peer")).await,
        0
    );
    assert!(primary.try_recv().is_err());
}

#[tokio::test]
async fn session_fanout_updates_focus_batching_from_live_targets_without_delaying_delivery() {
    use crate::stream_batching::{self, ConnectionFocusGuard};
    let session_id = "fanout-focus-batching";
    let (mut member, mut primary) = swarm_member(session_id);
    let first_id = "fanout-focus-hidden";
    let second_id = "fanout-focus-silent";
    let _first_focus = ConnectionFocusGuard(first_id.into());
    let _second_focus = ConnectionFocusGuard(second_id.into());
    let (first_tx, mut first_rx) = mpsc::unbounded_channel();
    let (second_tx, mut second_rx) = mpsc::unbounded_channel();
    member.event_txs.insert(first_id.into(), first_tx.clone());
    member.event_tx = first_tx;
    stream_batching::set_connection_focus(first_id, session_id, false, true);
    let members = members(member);
    assert_eq!(
        fanout_session_event(&members, session_id, event("hidden")).await,
        1
    );
    assert_text(&mut first_rx, "hidden");
    assert_eq!(
        stream_batching::session_batch_period(session_id),
        Some(stream_batching::HIDDEN_TEXT_BATCH)
    );
    members
        .write()
        .await
        .get_mut(session_id)
        .unwrap()
        .event_txs
        .insert(second_id.into(), second_tx);
    assert_eq!(
        fanout_session_event(&members, session_id, event("silent")).await,
        2
    );
    assert_text(&mut first_rx, "silent");
    assert_text(&mut second_rx, "silent");
    assert!(!stream_batching::session_batches(session_id));
    stream_batching::set_connection_focus(second_id, session_id, false, false);
    assert_eq!(
        fanout_session_event(&members, session_id, event("unfocused")).await,
        2
    );
    assert_text(&mut first_rx, "unfocused");
    assert_text(&mut second_rx, "unfocused");
    assert_eq!(
        stream_batching::session_batch_period(session_id),
        Some(stream_batching::UNFOCUSED_TEXT_BATCH)
    );
    drop(second_rx);
    assert_eq!(
        fanout_session_event(&members, session_id, event("pruned")).await,
        1
    );
    assert_text(&mut first_rx, "pruned");
    assert_eq!(
        stream_batching::session_batch_period(session_id),
        Some(stream_batching::HIDDEN_TEXT_BATCH)
    );
    stream_batching::set_connection_focus(first_id, session_id, true, false);
    assert_eq!(
        fanout_session_event(&members, session_id, event("focused")).await,
        1
    );
    assert_text(&mut first_rx, "focused");
    assert!(!stream_batching::session_batches(session_id));
    assert!(primary.try_recv().is_err());
}
