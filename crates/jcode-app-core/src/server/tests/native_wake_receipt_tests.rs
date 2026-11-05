//! Native wake receipt coverage: admission, replay, and delivery.
//!
//! Split out of `server/tests.rs`, which had grown past the oversized test-file
//! budget; these two cases are the only ones that exercise the receipt flow.

use super::*;

// These tests exercise `server::wake_receipts` and `server::live_turn`, which
// are two module levels up from this child of `server::tests`.

#[tokio::test]
async fn native_receipt_remains_queued_until_exact_target_turn_saves_message() {
    let _env_guard = crate::storage::lock_test_env();
    let home = tempfile::TempDir::new().expect("isolated home");
    let old_home = std::env::var_os("JCODE_HOME");
    crate::env::set_var("JCODE_HOME", home.path());

    let provider = Arc::new(StreamingMockProvider::default());
    provider.queue_response(vec![
        StreamEvent::TextDelta("receipt delivered".to_string()),
        StreamEvent::MessageEnd { stop_reason: None },
    ]);
    let agent = test_agent(provider).await;
    let session_id = agent.lock().await.session_id().to_string();
    let receipt_id = "synthetic:receipt:1";
    let receipt = super::super::wake_receipts::submit(receipt_id, &session_id, "synthetic wake")
        .expect("durable queue");
    assert_eq!(
        receipt.state,
        super::super::wake_receipts::WakeState::Queued
    );

    let sessions = Arc::new(super::super::session_manager::SessionManager::new());
    sessions
        .register_new(session_id.clone(), agent.clone())
        .await
        .expect("register live agent");
    let (member_event_tx, mut member_event_rx) = mpsc::unbounded_channel();
    let member = attached_swarm_member(&session_id, member_event_tx);
    let swarm_members = Arc::new(RwLock::new(HashMap::from([(session_id.clone(), member)])));
    let (swarms_by_id, event_history, event_counter, swarm_event_tx) = empty_swarm_status_state();
    let ctx = super::super::live_turn::LiveTurnSwarmContext::new(
        &swarm_members,
        &swarms_by_id,
        &event_history,
        &event_counter,
        &swarm_event_tx,
    );
    let reservation =
        super::super::live_turn::idle_live_agent(&session_id, &sessions, &swarm_members)
            .await
            .expect("reserve synthetic target");
    assert!(
        !super::super::live_turn::run_receipt_turn_if_idle(
            &session_id,
            "synthetic wake",
            receipt_id,
            &sessions,
            ctx.clone(),
        )
        .await
    );
    assert_eq!(
        super::super::wake_receipts::get(receipt_id)
            .expect("status")
            .expect("receipt")
            .state,
        super::super::wake_receipts::WakeState::Queued
    );
    drop(reservation);
    assert!(
        super::super::live_turn::run_receipt_turn_if_idle(
            &session_id,
            "synthetic wake",
            receipt_id,
            &sessions,
            ctx.clone(),
        )
        .await
    );
    // The tracked live turn fans its events out through the server's turn
    // channel, not the swarm member's private receiver, so completion is
    // observed through the durable ledger this feature is about.
    timeout(Duration::from_secs(5), async {
        loop {
            let state = super::super::wake_receipts::get(receipt_id)
                .expect("status")
                .expect("receipt")
                .state;
            if state == super::super::wake_receipts::WakeState::Awakened {
                break;
            }
            tokio::time::sleep(Duration::from_millis(20)).await;
        }
    })
    .await
    .expect("receipt turn finishes");
    assert!(crate::session::session_exists(&session_id));
    assert_eq!(
        super::super::wake_receipts::get(receipt_id)
            .expect("status")
            .expect("receipt")
            .state,
        super::super::wake_receipts::WakeState::Awakened
    );
    assert!(agent.lock().await.messages().iter().any(|message| {
        message.role == Role::User && message.content_preview().contains("synthetic wake")
    }));
    let persisted = crate::session::Session::load(&session_id).expect("persisted transcript");
    assert!(persisted.messages.iter().any(|message| {
        message.role == Role::User && message.content_preview().contains("synthetic wake")
    }));
    // A delayed duplicate still carries its old queued submission snapshot.
    // Once it reserves the now-idle agent, it must observe the durable final
    // state and leave the transcript unchanged.
    let message_count = agent.lock().await.messages().len();
    assert!(
        !super::super::live_turn::run_receipt_turn_if_idle(
            &session_id,
            "synthetic wake",
            receipt_id,
            &sessions,
            ctx,
        )
        .await
    );
    assert_eq!(agent.lock().await.messages().len(), message_count);

    if let Some(old_home) = old_home {
        crate::env::set_var("JCODE_HOME", old_home);
    } else {
        crate::env::remove_var("JCODE_HOME");
    }
}

#[tokio::test]
async fn native_receipt_admits_attached_session_and_replays_without_target() {
    let _env_guard = crate::storage::lock_test_env();
    let home = tempfile::TempDir::new().expect("isolated home");
    let old_home = std::env::var_os("JCODE_HOME");
    let old_allowlist = std::env::var_os("JCODE_NATIVE_WAKE_SESSION_ALLOWLIST");
    crate::env::set_var("JCODE_HOME", home.path());

    let provider = Arc::new(StreamingMockProvider::default());
    let agent = test_agent(provider).await;
    // Attached in memory but never saved to disk yet: the pre-attach-subscribe
    // shape that the disk-only existence guard used to reject.
    //
    // The published runtime keeps its own session identity: SessionManager
    // refuses to publish an agent under a reserved id it does not carry.
    let target = agent.lock().await.session_id().to_string();
    crate::env::set_var("JCODE_NATIVE_WAKE_SESSION_ALLOWLIST", target.as_str());
    assert!(!crate::session::session_exists(&target));

    let sessions = Arc::new(super::super::session_manager::SessionManager::new());
    sessions
        .register_new(target.clone(), agent.clone())
        .await
        .expect("register live agent");
    let soft_interrupt_queues: SessionInterruptQueues = Arc::new(RwLock::new(HashMap::new()));
    let client_connections: Arc<RwLock<HashMap<String, super::super::ClientConnectionInfo>>> =
        Arc::new(RwLock::new(HashMap::new()));
    let (member_event_tx, _member_event_rx) = mpsc::unbounded_channel();
    let member = attached_swarm_member(&target, member_event_tx);
    let swarm_members = Arc::new(RwLock::new(HashMap::from([(target.clone(), member)])));
    let (swarms_by_id, event_history, event_counter, swarm_event_tx) = empty_swarm_status_state();
    let (client_event_tx, mut client_event_rx) = mpsc::unbounded_channel();

    let receipt_id = "attached:receipt:1";
    super::super::client_actions::handle_notify_session_receipt(
        1,
        target.clone(),
        "wake attached".to_string(),
        receipt_id.to_string(),
        super::super::client_actions::NotifySessionContext {
            sessions: &sessions,
            soft_interrupt_queues: &soft_interrupt_queues,
            client_connections: &client_connections,
            swarm_members: &swarm_members,
            swarms_by_id: &swarms_by_id,
            event_history: &event_history,
            event_counter: &event_counter,
            swarm_event_tx: &swarm_event_tx,
            client_event_tx: &client_event_tx,
        },
    )
    .await;
    let first = client_event_rx.recv().await.expect("receipt event");
    assert!(
        matches!(&first, ServerEvent::WakeReceipt { status, .. } if status == "queued"),
        "an attached live session must be admitted before its first save, got {first:?}"
    );

    // Drop the target from memory and leave nothing on disk, then replay the
    // identical request. It must return the stored state, not "does not exist".
    sessions.remove(&target).await;
    assert!(!crate::session::session_exists(&target));
    super::super::client_actions::handle_notify_session_receipt(
        2,
        target.clone(),
        "wake attached".to_string(),
        receipt_id.to_string(),
        super::super::client_actions::NotifySessionContext {
            sessions: &sessions,
            soft_interrupt_queues: &soft_interrupt_queues,
            client_connections: &client_connections,
            swarm_members: &swarm_members,
            swarms_by_id: &swarms_by_id,
            event_history: &event_history,
            event_counter: &event_counter,
            swarm_event_tx: &swarm_event_tx,
            client_event_tx: &client_event_tx,
        },
    )
    .await;
    let second = client_event_rx.recv().await.expect("replay event");
    assert!(
        matches!(
            &second,
            ServerEvent::WakeReceipt { status, receipt_id: replayed, .. }
                if status == "queued" && replayed == receipt_id
        ),
        "an identical replay must return the stored state after target loss, got {second:?}"
    );

    if let Some(old_home) = old_home {
        crate::env::set_var("JCODE_HOME", old_home);
    } else {
        crate::env::remove_var("JCODE_HOME");
    }
    if let Some(old_allowlist) = old_allowlist {
        crate::env::set_var("JCODE_NATIVE_WAKE_SESSION_ALLOWLIST", old_allowlist);
    } else {
        crate::env::remove_var("JCODE_NATIVE_WAKE_SESSION_ALLOWLIST");
    }
}
