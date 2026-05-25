use super::{
    graceful_shutdown_sessions, graceful_shutdown_sessions_with_timeout,
    persist_reload_recovery_intents, receive_reload_signal,
};
use crate::server::{ReloadSignal, SwarmEvent, SwarmEventType, SwarmMember};
use jcode_agent_runtime::InterruptSignal;
use std::collections::HashMap;
use std::sync::Arc;
use std::time::Instant;
use tokio::sync::{RwLock, broadcast, mpsc, watch};

fn set_member_status(members: &mut HashMap<String, SwarmMember>, session_id: &str, status: &str) {
    assert!(
        members.contains_key(session_id),
        "missing test member {session_id}"
    );
    if let Some(member) = members.get_mut(session_id) {
        member.status = status.to_string();
    }
}

fn member(session_id: &str, status: &str) -> SwarmMember {
    let (event_tx, _event_rx) = mpsc::unbounded_channel();
    SwarmMember {
        session_id: session_id.to_string(),
        event_tx,
        event_txs: HashMap::new(),
        working_dir: None,
        swarm_id: None,
        swarm_enabled: false,
        status: status.to_string(),
        detail: None,
        friendly_name: None,
        report_back_to_session_id: None,
        latest_completion_report: None,
        role: "agent".to_string(),
        joined_at: Instant::now(),
        last_status_change: Instant::now(),
        is_headless: false,
    }
}

#[tokio::test]
async fn receive_reload_signal_consumes_already_pending_value() {
    let (tx, mut rx) = watch::channel(None::<ReloadSignal>);
    tx.send(Some(ReloadSignal {
        hash: "abc1234".to_string(),
        triggering_session: Some("sess-1".to_string()),
        prefer_selfdev_binary: true,
        request_id: "reload-1".to_string(),
    }))
    .expect("send pending reload signal");

    let signal = tokio::time::timeout(
        std::time::Duration::from_millis(100),
        receive_reload_signal(&mut rx),
    )
    .await
    .expect("pending signal should be observed immediately")
    .expect("channel should still be open");

    assert_eq!(signal.hash, "abc1234");
    assert_eq!(signal.triggering_session.as_deref(), Some("sess-1"));
    assert!(signal.prefer_selfdev_binary);
    assert_eq!(signal.request_id, "reload-1");
}

#[tokio::test]
async fn receive_reload_signal_waits_for_future_value_when_initially_empty() {
    let (tx, mut rx) = watch::channel(None::<ReloadSignal>);

    let waiter = tokio::spawn(async move { receive_reload_signal(&mut rx).await });
    tokio::time::sleep(std::time::Duration::from_millis(20)).await;

    tx.send(Some(ReloadSignal {
        hash: "def5678".to_string(),
        triggering_session: Some("sess-2".to_string()),
        prefer_selfdev_binary: false,
        request_id: "reload-2".to_string(),
    }))
    .expect("send future reload signal");

    let signal = tokio::time::timeout(std::time::Duration::from_millis(100), waiter)
        .await
        .expect("future signal should wake waiter")
        .expect("waiter task should succeed")
        .expect("channel should still be open");

    assert_eq!(signal.hash, "def5678");
    assert_eq!(signal.triggering_session.as_deref(), Some("sess-2"));
    assert!(!signal.prefer_selfdev_binary);
    assert_eq!(signal.request_id, "reload-2");
}

#[test]
fn persist_reload_recovery_intents_records_running_peer_recovery() -> anyhow::Result<()> {
    let _guard = crate::storage::lock_test_env();
    let temp_home = tempfile::TempDir::new()?;
    let prev_home = std::env::var_os("JCODE_HOME");
    crate::env::set_var("JCODE_HOME", temp_home.path());

    let runtime = tokio::runtime::Builder::new_current_thread()
        .enable_all()
        .build()?;

    let swarm_members = Arc::new(RwLock::new(HashMap::from([
        ("initiator".to_string(), member("initiator", "running")),
        ("peer".to_string(), member("peer", "running")),
        ("idle".to_string(), member("idle", "ready")),
    ])));

    runtime.block_on(persist_reload_recovery_intents(
        "reload-store-test",
        &swarm_members,
        Some("initiator"),
    ));

    let peer_directive = crate::server::reload_recovery::claim_pending_for_session("peer")
        .expect("claim peer recovery")
        .expect("peer recovery intent should exist");
    assert!(
        peer_directive
            .continuation_message
            .contains("interrupted by a server reload")
    );
    assert!(
        crate::server::reload_recovery::claim_pending_for_session("idle")
            .expect("claim idle recovery")
            .is_none(),
        "idle sessions should not get reload recovery intents"
    );
    assert!(
        crate::server::reload_recovery::claim_pending_for_session("initiator")
            .expect("claim initiator recovery")
            .is_none(),
        "initiator without selfdev reload context should not get a generic interrupted-peer intent"
    );

    if let Some(prev_home) = prev_home {
        crate::env::set_var("JCODE_HOME", prev_home);
    } else {
        crate::env::remove_var("JCODE_HOME");
    }
    Ok(())
}

#[tokio::test]
async fn graceful_shutdown_sessions_signals_all_running_sessions_including_initiator() {
    let sessions = Arc::new(RwLock::new(HashMap::new()));
    let swarm_members = Arc::new(RwLock::new(HashMap::from([
        ("initiator".to_string(), member("initiator", "running")),
        ("peer".to_string(), member("peer", "running")),
    ])));
    let initiator_signal = InterruptSignal::new();
    let peer_signal = InterruptSignal::new();
    let shutdown_signals = Arc::new(RwLock::new(HashMap::from([
        ("initiator".to_string(), initiator_signal.clone()),
        ("peer".to_string(), peer_signal.clone()),
    ])));
    let (swarm_event_tx, _) = broadcast::channel(8);
    let swarm_members_for_task = swarm_members.clone();
    let swarm_event_tx_for_task = swarm_event_tx.clone();

    let checkpoint_task = tokio::spawn(async move {
        tokio::task::yield_now().await;
        {
            let mut members = swarm_members_for_task.write().await;
            set_member_status(&mut members, "initiator", "ready");
            set_member_status(&mut members, "peer", "ready");
        }
        let _ = swarm_event_tx_for_task.send(SwarmEvent {
            id: 1,
            session_id: "initiator".to_string(),
            session_name: None,
            swarm_id: None,
            event: SwarmEventType::StatusChange {
                old_status: "running".to_string(),
                new_status: "ready".to_string(),
            },
            timestamp: Instant::now(),
            absolute_time: std::time::SystemTime::now(),
        });
        let _ = swarm_event_tx_for_task.send(SwarmEvent {
            id: 2,
            session_id: "peer".to_string(),
            session_name: None,
            swarm_id: None,
            event: SwarmEventType::StatusChange {
                old_status: "running".to_string(),
                new_status: "ready".to_string(),
            },
            timestamp: Instant::now(),
            absolute_time: std::time::SystemTime::now(),
        });
    });

    graceful_shutdown_sessions(
        &sessions,
        &swarm_members,
        &shutdown_signals,
        &swarm_event_tx,
        None,
    )
    .await;
    checkpoint_task.await.expect("checkpoint task");

    assert!(
        initiator_signal.is_set(),
        "initiating selfdev session should also be interrupted so reload tool cannot hang"
    );
    assert!(
        peer_signal.is_set(),
        "other running sessions should be interrupted too"
    );
}

#[tokio::test]
async fn graceful_shutdown_sessions_does_not_wait_for_triggering_session_checkpoint() {
    let sessions = Arc::new(RwLock::new(HashMap::new()));
    let swarm_members = Arc::new(RwLock::new(HashMap::from([
        ("initiator".to_string(), member("initiator", "running")),
        ("peer".to_string(), member("peer", "running")),
    ])));
    let initiator_signal = InterruptSignal::new();
    let peer_signal = InterruptSignal::new();
    let shutdown_signals = Arc::new(RwLock::new(HashMap::from([
        ("initiator".to_string(), initiator_signal.clone()),
        ("peer".to_string(), peer_signal.clone()),
    ])));
    let (swarm_event_tx, _) = broadcast::channel(8);
    let swarm_members_for_task = swarm_members.clone();
    let swarm_event_tx_for_task = swarm_event_tx.clone();

    let checkpoint_task = tokio::spawn(async move {
        tokio::time::sleep(std::time::Duration::from_millis(50)).await;
        {
            let mut members = swarm_members_for_task.write().await;
            set_member_status(&mut members, "peer", "ready");
        }
        let _ = swarm_event_tx_for_task.send(SwarmEvent {
            id: 1,
            session_id: "peer".to_string(),
            session_name: None,
            swarm_id: None,
            event: SwarmEventType::StatusChange {
                old_status: "running".to_string(),
                new_status: "ready".to_string(),
            },
            timestamp: Instant::now(),
            absolute_time: std::time::SystemTime::now(),
        });
    });

    tokio::time::timeout(
        std::time::Duration::from_secs(1),
        graceful_shutdown_sessions(
            &sessions,
            &swarm_members,
            &shutdown_signals,
            &swarm_event_tx,
            Some("initiator"),
        ),
    )
    .await
    .expect("reload shutdown should not wait for triggering session");
    checkpoint_task.await.expect("checkpoint task");

    assert!(
        initiator_signal.is_set(),
        "triggering session should still receive graceful shutdown signal"
    );
    assert!(
        peer_signal.is_set(),
        "peer session should still receive graceful shutdown signal"
    );
    assert_eq!(
        swarm_members
            .read()
            .await
            .get("initiator")
            .expect("initiator")
            .status,
        "running",
        "initiator may remain running without blocking reload"
    );
}

#[tokio::test]
async fn graceful_shutdown_sessions_skips_idle_sessions() {
    let sessions = Arc::new(RwLock::new(HashMap::new()));
    let swarm_members = Arc::new(RwLock::new(HashMap::from([(
        "idle".to_string(),
        member("idle", "ready"),
    )])));
    let idle_signal = InterruptSignal::new();
    let shutdown_signals = Arc::new(RwLock::new(HashMap::from([(
        "idle".to_string(),
        idle_signal.clone(),
    )])));
    let (swarm_event_tx, _) = broadcast::channel(8);

    graceful_shutdown_sessions(
        &sessions,
        &swarm_members,
        &shutdown_signals,
        &swarm_event_tx,
        None,
    )
    .await;

    assert!(
        !idle_signal.is_set(),
        "idle sessions should not be interrupted during reload"
    );
}

#[tokio::test]
async fn graceful_shutdown_sessions_does_not_wait_on_running_sessions_without_signal() {
    let sessions = Arc::new(RwLock::new(HashMap::new()));
    let swarm_members = Arc::new(RwLock::new(HashMap::from([(
        "orphan_running".to_string(),
        member("orphan_running", "running"),
    )])));
    let shutdown_signals = Arc::new(RwLock::new(HashMap::new()));
    let (swarm_event_tx, _) = broadcast::channel(8);

    let started = Instant::now();
    graceful_shutdown_sessions(
        &sessions,
        &swarm_members,
        &shutdown_signals,
        &swarm_event_tx,
        None,
    )
    .await;

    assert!(
        started.elapsed() < std::time::Duration::from_millis(100),
        "running sessions without shutdown signals should not consume the reload grace period"
    );
}

#[tokio::test]
async fn graceful_shutdown_sessions_waits_until_target_status_change_arrives() {
    let sessions = Arc::new(RwLock::new(HashMap::new()));
    let swarm_members = Arc::new(RwLock::new(HashMap::from([(
        "target".to_string(),
        member("target", "running"),
    )])));
    let signal = InterruptSignal::new();
    let shutdown_signals = Arc::new(RwLock::new(HashMap::from([(
        "target".to_string(),
        signal.clone(),
    )])));
    let (swarm_event_tx, _) = broadcast::channel(8);

    let mut waiter = tokio::spawn({
        let sessions = sessions.clone();
        let swarm_members = swarm_members.clone();
        let shutdown_signals = shutdown_signals.clone();
        let swarm_event_tx = swarm_event_tx.clone();
        async move {
            graceful_shutdown_sessions(
                &sessions,
                &swarm_members,
                &shutdown_signals,
                &swarm_event_tx,
                None,
            )
            .await;
        }
    });

    tokio::time::sleep(std::time::Duration::from_millis(50)).await;
    assert!(
        signal.is_set(),
        "running target should be interrupted promptly"
    );
    assert!(
        tokio::time::timeout(std::time::Duration::from_millis(50), &mut waiter)
            .await
            .is_err(),
        "reload shutdown should stay pending until target leaves running"
    );

    {
        let mut members = swarm_members.write().await;
        set_member_status(&mut members, "target", "ready");
    }
    let _ = swarm_event_tx.send(SwarmEvent {
        id: 1,
        session_id: "target".to_string(),
        session_name: None,
        swarm_id: None,
        event: SwarmEventType::StatusChange {
            old_status: "running".to_string(),
            new_status: "ready".to_string(),
        },
        timestamp: Instant::now(),
        absolute_time: std::time::SystemTime::now(),
    });

    tokio::time::timeout(std::time::Duration::from_secs(1), waiter)
        .await
        .expect("waiter should complete after target checkpoint")
        .expect("waiter task should succeed");
}

#[tokio::test]
async fn graceful_shutdown_sessions_ignores_unrelated_events_until_target_leaves() {
    let sessions = Arc::new(RwLock::new(HashMap::new()));
    let swarm_members = Arc::new(RwLock::new(HashMap::from([
        ("target".to_string(), member("target", "running")),
        ("other".to_string(), member("other", "running")),
    ])));
    let signal = InterruptSignal::new();
    let shutdown_signals = Arc::new(RwLock::new(HashMap::from([("target".to_string(), signal)])));
    let (swarm_event_tx, _) = broadcast::channel(8);

    let mut waiter = tokio::spawn({
        let sessions = sessions.clone();
        let swarm_members = swarm_members.clone();
        let shutdown_signals = shutdown_signals.clone();
        let swarm_event_tx = swarm_event_tx.clone();
        async move {
            graceful_shutdown_sessions(
                &sessions,
                &swarm_members,
                &shutdown_signals,
                &swarm_event_tx,
                None,
            )
            .await;
        }
    });

    tokio::time::sleep(std::time::Duration::from_millis(20)).await;
    {
        let mut members = swarm_members.write().await;
        set_member_status(&mut members, "other", "ready");
    }
    let _ = swarm_event_tx.send(SwarmEvent {
        id: 1,
        session_id: "other".to_string(),
        session_name: None,
        swarm_id: None,
        event: SwarmEventType::StatusChange {
            old_status: "running".to_string(),
            new_status: "ready".to_string(),
        },
        timestamp: Instant::now(),
        absolute_time: std::time::SystemTime::now(),
    });

    assert!(
        tokio::time::timeout(std::time::Duration::from_millis(50), &mut waiter)
            .await
            .is_err(),
        "unrelated status changes should not unblock reload shutdown"
    );

    {
        let mut members = swarm_members.write().await;
        set_member_status(&mut members, "target", "stopped");
    }
    let _ = swarm_event_tx.send(SwarmEvent {
        id: 2,
        session_id: "target".to_string(),
        session_name: None,
        swarm_id: None,
        event: SwarmEventType::StatusChange {
            old_status: "running".to_string(),
            new_status: "stopped".to_string(),
        },
        timestamp: Instant::now(),
        absolute_time: std::time::SystemTime::now(),
    });

    tokio::time::timeout(std::time::Duration::from_secs(1), waiter)
        .await
        .expect("waiter should complete after target transition")
        .expect("waiter task should succeed");
}

#[tokio::test]
async fn graceful_shutdown_sessions_treats_member_left_as_unblocked() {
    let sessions = Arc::new(RwLock::new(HashMap::new()));
    let swarm_members = Arc::new(RwLock::new(HashMap::from([(
        "target".to_string(),
        member("target", "running"),
    )])));
    let signal = InterruptSignal::new();
    let shutdown_signals = Arc::new(RwLock::new(HashMap::from([("target".to_string(), signal)])));
    let (swarm_event_tx, _) = broadcast::channel(8);

    let waiter = tokio::spawn({
        let sessions = sessions.clone();
        let swarm_members = swarm_members.clone();
        let shutdown_signals = shutdown_signals.clone();
        let swarm_event_tx = swarm_event_tx.clone();
        async move {
            graceful_shutdown_sessions(
                &sessions,
                &swarm_members,
                &shutdown_signals,
                &swarm_event_tx,
                None,
            )
            .await;
        }
    });

    tokio::time::sleep(std::time::Duration::from_millis(20)).await;
    {
        let mut members = swarm_members.write().await;
        members.remove("target");
    }
    let _ = swarm_event_tx.send(SwarmEvent {
        id: 1,
        session_id: "target".to_string(),
        session_name: None,
        swarm_id: None,
        event: SwarmEventType::MemberChange {
            action: "left".to_string(),
        },
        timestamp: Instant::now(),
        absolute_time: std::time::SystemTime::now(),
    });

    tokio::time::timeout(std::time::Duration::from_secs(1), waiter)
        .await
        .expect("waiter should complete after member leaves")
        .expect("waiter task should succeed");
}

#[tokio::test]
async fn graceful_shutdown_sessions_times_out_and_proceeds() {
    let sessions = Arc::new(RwLock::new(HashMap::new()));
    let swarm_members = Arc::new(RwLock::new(HashMap::from([(
        "target".to_string(),
        member("target", "running"),
    )])));
    let signal = InterruptSignal::new();
    let shutdown_signals = Arc::new(RwLock::new(HashMap::from([(
        "target".to_string(),
        signal.clone(),
    )])));
    let (swarm_event_tx, _) = broadcast::channel(8);

    let started = Instant::now();
    graceful_shutdown_sessions_with_timeout(
        &sessions,
        &swarm_members,
        &shutdown_signals,
        &swarm_event_tx,
        std::time::Duration::from_millis(50),
        None,
    )
    .await;

    assert!(
        signal.is_set(),
        "running target should still be signaled promptly"
    );
    assert!(
        started.elapsed() >= std::time::Duration::from_millis(50)
            && started.elapsed() < std::time::Duration::from_millis(250),
        "graceful shutdown should honor the timeout instead of waiting indefinitely"
    );
}
