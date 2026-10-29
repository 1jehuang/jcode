use super::*;

#[tokio::test]
async fn owner_is_notified_once_of_idle_headless_workers_stopped_by_restart() {
    // Issue #1651: an idle headless worker loads as stopped after a restart
    // and has no re-attach path. Its owner must hear about it once.
    let note = crate::server::swarm_persistence::IDLE_WORKER_NOT_RESTORED_NOTE;
    let (coord, mut coord_rx) = swarm_member("coord", "coordinator", false);
    let (mut butterfly, _rx1) = swarm_member("butterfly", "agent", true);
    butterfly.status = "stopped".to_string();
    butterfly.detail = Some(note.to_string());
    butterfly.report_back_to_session_id = Some("coord".to_string());
    let (mut stranger, _rx2) = swarm_member("stranger", "agent", true);
    stranger.status = "stopped".to_string();
    stranger.detail = Some(note.to_string());
    stranger.report_back_to_session_id = Some("someone-else".to_string());
    let (mut tui_client, _rx3) = swarm_member("tui", "agent", false);
    tui_client.status = "stopped".to_string();
    tui_client.detail = Some("client not attached after server restart".to_string());
    tui_client.report_back_to_session_id = Some("coord".to_string());
    let swarm_state = crate::server::SwarmState::new(
        HashMap::from([
            ("coord".to_string(), coord),
            ("butterfly".to_string(), butterfly),
            ("stranger".to_string(), stranger),
            ("tui".to_string(), tui_client),
        ]),
        HashMap::from([(
            "swarm-1".to_string(),
            HashSet::from([
                "coord".to_string(),
                "butterfly".to_string(),
                "stranger".to_string(),
                "tui".to_string(),
            ]),
        )]),
        HashMap::new(),
        HashMap::from([("swarm-1".to_string(), "coord".to_string())]),
    );

    let sessions = Arc::new(RwLock::new(HashMap::new()));
    let queue: jcode_agent_runtime::SoftInterruptQueue = Default::default();
    let queues = Arc::new(RwLock::new(HashMap::from([(
        "coord".to_string(),
        queue.clone(),
    )])));
    assert_eq!(
        super::super::notify_owner_of_unrestored_idle_workers(
            "coord",
            &swarm_state,
            &sessions,
            &queues
        )
        .await,
        1
    );
    {
        let pending = queue.lock().unwrap();
        assert_eq!(pending.len(), 1);
        assert!(pending[0].content.contains("butterfly"));
        assert!(!pending[0].content.contains("stranger"));
        assert_eq!(
            pending[0].source,
            jcode_agent_runtime::SoftInterruptSource::System
        );
    }
    let notices: Vec<String> = std::iter::from_fn(|| coord_rx.try_recv().ok())
        .filter_map(|event| match event {
            ServerEvent::Notification { message, .. } => Some(message),
            _ => None,
        })
        .collect();
    assert_eq!(notices.len(), 1, "{notices:?}");
    assert!(notices[0].contains("butterfly"), "{}", notices[0]);
    assert!(!notices[0].contains("stranger"), "{}", notices[0]);

    // Delivered once: a later reattach does not repeat it.
    assert_eq!(
        super::super::notify_owner_of_unrestored_idle_workers(
            "coord",
            &swarm_state,
            &sessions,
            &queues
        )
        .await,
        0
    );
    let members = swarm_state.members.read().await;
    assert_eq!(members.get("butterfly").unwrap().status, "stopped");
    assert!(
        members
            .get("stranger")
            .and_then(|m| m.detail.as_deref())
            .is_some_and(|d| !d.contains("owner notified"))
    );
}

#[tokio::test]
async fn unrestored_notice_retries_model_delivery_despite_available_ui() {
    let owner_id = "unrestored-notice-retry-owner";
    let (state, mut ui_rx) = unrestored_notice_state(owner_id);
    let sessions = Default::default();
    let queues = Default::default();
    assert_eq!(
        super::super::notify_owner_of_unrestored_idle_workers(owner_id, &state, &sessions, &queues)
            .await,
        0
    );
    assert!(ui_rx.try_recv().is_err());
    assert!(super::super::member_awaits_unrestored_notice(
        &state.members.read().await["recovery-worker"]
    ));

    // Reconnect registers the model queue. The earlier failed delivery
    // must not have consumed the durable recovery marker.
    let queue: jcode_agent_runtime::SoftInterruptQueue = Default::default();
    queues
        .write()
        .await
        .insert(owner_id.to_string(), queue.clone());
    assert_eq!(
        super::super::notify_owner_of_unrestored_idle_workers(owner_id, &state, &sessions, &queues)
            .await,
        1
    );
    assert_eq!(queue.lock().unwrap().len(), 1);
}

fn unrestored_notice_state(
    owner_id: &str,
) -> (
    crate::server::SwarmState,
    tokio::sync::mpsc::UnboundedReceiver<ServerEvent>,
) {
    let swarm_id = format!("swarm-{owner_id}");
    let (mut owner, owner_rx) = swarm_member(owner_id, "coordinator", false);
    owner.swarm_id = Some(swarm_id.clone());
    let (mut worker, _worker_rx) = swarm_member("recovery-worker", "agent", true);
    worker.swarm_id = Some(swarm_id.clone());
    worker.status = "stopped".to_string();
    worker.detail =
        Some(crate::server::swarm_persistence::IDLE_WORKER_NOT_RESTORED_NOTE.to_string());
    // Exercise the coordinator fallback when no explicit report-back
    // owner survived in a legacy snapshot.
    worker.report_back_to_session_id = None;
    (
        crate::server::SwarmState::new(
            HashMap::from([
                (owner_id.to_string(), owner),
                ("recovery-worker".to_string(), worker),
            ]),
            HashMap::from([(
                swarm_id.clone(),
                HashSet::from([owner_id.to_string(), "recovery-worker".to_string()]),
            )]),
            HashMap::new(),
            HashMap::from([(swarm_id, owner_id.to_string())]),
        ),
        owner_rx,
    )
}

#[tokio::test]
async fn unrestored_notice_concurrent_reconnects_queue_once_without_ui() {
    let owner_id = "unrestored-notice-concurrent-owner";
    let (state, ui_rx) = unrestored_notice_state(owner_id);
    drop(ui_rx);
    let sessions = Default::default();
    let queue: jcode_agent_runtime::SoftInterruptQueue = Default::default();
    let queues = Arc::new(RwLock::new(HashMap::from([(
        owner_id.to_string(),
        queue.clone(),
    )])));
    let (first, second) = tokio::join!(
        super::super::notify_owner_of_unrestored_idle_workers(owner_id, &state, &sessions, &queues),
        super::super::notify_owner_of_unrestored_idle_workers(owner_id, &state, &sessions, &queues),
    );
    assert_eq!(first + second, 1);
    assert_eq!(queue.lock().unwrap().len(), 1);
    assert!(!super::super::member_awaits_unrestored_notice(
        &state.members.read().await["recovery-worker"]
    ));
}

#[tokio::test]
#[allow(clippy::await_holding_lock)]
async fn unrestored_notice_ignores_worker_with_pending_recovery_intent() {
    let _guard = crate::storage::lock_test_env();
    let temp = tempfile::TempDir::new().unwrap();
    let previous_home = std::env::var_os("JCODE_HOME");
    crate::env::set_var("JCODE_HOME", temp.path());
    let owner_id = "unrestored-notice-pending-owner";
    let (state, mut ui_rx) = unrestored_notice_state(owner_id);
    let worker_id = "pending-recovery-worker";
    {
        let mut members = state.members.write().await;
        let mut worker = members.remove("recovery-worker").unwrap();
        worker.session_id = worker_id.to_string();
        members.insert(worker_id.to_string(), worker);
    }
    let sessions = Default::default();
    let queue: jcode_agent_runtime::SoftInterruptQueue = Default::default();
    let queues = Arc::new(RwLock::new(HashMap::from([(
        owner_id.to_string(),
        queue.clone(),
    )])));
    crate::server::reload_recovery::persist_intent(
        "reload-notice-pending",
        worker_id,
        crate::server::reload_recovery::ReloadRecoveryRole::Headless,
        crate::tool::selfdev::ReloadRecoveryDirective {
            reconnect_notice: None,
            continuation_message: "Continue interrupted work".to_string(),
        },
        "work in flight before shutdown",
    )
    .unwrap();
    assert_eq!(
        super::super::notify_owner_of_unrestored_idle_workers(owner_id, &state, &sessions, &queues)
            .await,
        0
    );
    assert!(queue.lock().unwrap().is_empty());
    assert!(ui_rx.try_recv().is_err());
    let members = state.members.read().await;
    assert_eq!(
        members[worker_id].detail.as_deref(),
        Some(crate::server::swarm_persistence::IDLE_WORKER_NOT_RESTORED_NOTE)
    );
    assert!(crate::server::reload_recovery::has_pending_for_session(
        worker_id
    ));
    if let Some(home) = previous_home {
        crate::env::set_var("JCODE_HOME", home);
    } else {
        crate::env::remove_var("JCODE_HOME");
    }
}

#[tokio::test]
#[allow(clippy::await_holding_lock)]
async fn unrestored_notice_persists_for_owner_without_live_agent() {
    let _guard = crate::storage::lock_test_env();
    let temp = tempfile::TempDir::new().unwrap();
    let previous_home = std::env::var_os("JCODE_HOME");
    crate::env::set_var("JCODE_HOME", temp.path());
    let owner_id = "unrestored-notice-durable-owner";
    crate::session::Session::create_with_id(owner_id.to_string(), None, None)
        .save_prepared()
        .unwrap();
    let (state, ui_rx) = unrestored_notice_state(owner_id);
    drop(ui_rx);
    let sessions = Default::default();
    let queues = Default::default();
    assert_eq!(
        super::super::notify_owner_of_unrestored_idle_workers(owner_id, &state, &sessions, &queues)
            .await,
        1
    );
    let pending = crate::soft_interrupt_store::load(owner_id).unwrap();
    assert_eq!(pending.len(), 1);
    assert!(pending[0].content.contains("recovery-worker"));
    assert_eq!(
        pending[0].source,
        jcode_agent_runtime::SoftInterruptSource::System
    );
    // Rebuild the worker from the persisted swarm snapshot as a second
    // restart would. A reconnect must not append another deferred notice.
    let snapshot =
        crate::server::swarm_persistence::read_transfer_snapshot(&format!("swarm-{owner_id}"))
            .unwrap()
            .unwrap();
    let worker = snapshot
        .members
        .into_iter()
        .find(|member| member.record.session_id == "recovery-worker")
        .unwrap();
    let recovered = crate::server::swarm_persistence::from_persisted_member(
        worker,
        snapshot.updated_at_unix_ms,
        snapshot.updated_at_unix_ms,
        Duration::from_secs(3600),
    )
    .unwrap();
    assert!(!super::super::member_awaits_unrestored_notice(&recovered));
    state
        .members
        .write()
        .await
        .insert("recovery-worker".to_string(), recovered);
    assert_eq!(
        super::super::notify_owner_of_unrestored_idle_workers(owner_id, &state, &sessions, &queues)
            .await,
        0
    );
    assert_eq!(
        crate::soft_interrupt_store::load(owner_id).unwrap().len(),
        1
    );
    if let Some(home) = previous_home {
        crate::env::set_var("JCODE_HOME", home);
    } else {
        crate::env::remove_var("JCODE_HOME");
    }
}

#[tokio::test]
async fn update_member_status_notifies_coordinator_when_headless_worker_returns_ready() {
    let swarm_members = Arc::new(RwLock::new(HashMap::new()));
    let swarms_by_id = Arc::new(RwLock::new(HashMap::from([(
        "swarm-1".to_string(),
        HashSet::from(["coord".to_string(), "worker".to_string()]),
    )])));

    let (coord, mut coord_rx) = swarm_member("coord", "coordinator", false);
    let (mut worker, _worker_rx) = swarm_member("worker", "agent", true);
    worker.status = "running".to_string();
    worker.detail = Some("doing task".to_string());
    worker.report_back_to_session_id = Some("coord".to_string());
    {
        let mut members = swarm_members.write().await;
        members.insert("coord".to_string(), coord);
        members.insert("worker".to_string(), worker);
    }

    update_member_status(
        "worker",
        "ready",
        None,
        &swarm_members,
        &swarms_by_id,
        None,
        None,
        None,
    )
    .await;

    let events: Vec<_> = std::iter::from_fn(|| coord_rx.try_recv().ok()).collect();
    assert!(events.iter().any(|event| {
        matches!(
            event,
            ServerEvent::Notification {
                notification_type: NotificationType::Message { .. },
                message,
                ..
            } if message.contains("finished their work and is ready for more")
        )
    }));
}

#[tokio::test]
async fn member_elapsed_time_runs_only_while_active_and_freezes_afterward() {
    let swarm_members = Arc::new(RwLock::new(HashMap::new()));
    let swarms_by_id = Arc::new(RwLock::new(HashMap::from([(
        "swarm-1".to_string(),
        HashSet::from(["worker".to_string()]),
    )])));
    let (mut worker, _worker_rx) = swarm_member("worker", "agent", true);
    worker.runtime.elapsed_secs = Some(12);
    swarm_members
        .write()
        .await
        .insert("worker".to_string(), worker);

    update_member_status(
        "worker",
        "running",
        None,
        &swarm_members,
        &swarms_by_id,
        None,
        None,
        None,
    )
    .await;
    assert_eq!(
        swarm_members
            .read()
            .await
            .get("worker")
            .and_then(|member| member.runtime.elapsed_secs),
        None,
        "active members should derive elapsed time from joined_at"
    );

    {
        let mut members = swarm_members.write().await;
        members.get_mut("worker").unwrap().joined_at = Instant::now() - Duration::from_secs(37);
    }
    update_member_status(
        "worker",
        "completed",
        None,
        &swarm_members,
        &swarms_by_id,
        None,
        None,
        None,
    )
    .await;

    let frozen = swarm_members
        .read()
        .await
        .get("worker")
        .and_then(|member| member.runtime.elapsed_secs)
        .expect("terminal member should retain frozen elapsed time");
    assert!((37..=38).contains(&frozen), "frozen={frozen}");
    tokio::time::sleep(Duration::from_millis(20)).await;
    assert_eq!(
        swarm_members
            .read()
            .await
            .get("worker")
            .and_then(|member| member.runtime.elapsed_secs),
        Some(frozen)
    );
}

#[tokio::test]
async fn wait_monitor_publishes_entry_age_and_cleanup_without_tool_events() {
    let sid = "wait_monitor_worker";
    let swarm_id = "wait_monitor_swarm";
    let (mut worker, mut rx) = swarm_member(sid, "agent", true);
    worker.swarm_id = Some(swarm_id.into());
    worker.status = "running".into();
    let members = Arc::new(RwLock::new(HashMap::from([(sid.into(), worker)])));
    let swarms = Arc::new(RwLock::new(HashMap::from([(
        swarm_id.into(),
        HashSet::from([sid.into()]),
    )])));
    let monitor = tokio::spawn(monitor_wait_status(members, swarms));
    let guard = crate::wait_registry::register(sid, "Waiting for build");
    tokio::time::timeout(Duration::from_secs(5), async {
        loop {
            if let Some(ServerEvent::SwarmStatus { members }) = rx.recv().await
                && members.iter().any(|member| {
                    member.runtime.wait_reason.as_deref() == Some("Waiting for build")
                })
            {
                break;
            }
        }
    })
    .await
    .unwrap();
    tokio::time::timeout(Duration::from_secs(5), async {
        loop {
            if let Some(ServerEvent::SwarmStatus { members }) = rx.recv().await
                && members
                    .iter()
                    .any(|member| member.runtime.wait_age_secs.is_some_and(|age| age >= 1))
            {
                break;
            }
        }
    })
    .await
    .unwrap();
    drop(guard);
    tokio::time::timeout(Duration::from_secs(5), async {
        loop {
            if let Some(ServerEvent::SwarmStatus { members }) = rx.recv().await
                && members.iter().all(|member| {
                    member.runtime.wait_reason.is_none() && member.runtime.wait_age_secs.is_none()
                })
            {
                break;
            }
        }
    })
    .await
    .unwrap();
    monitor.abort();
    let _ = monitor.await;
}

#[tokio::test]
async fn wait_metadata_snapshots_are_live_and_clear_after_completion() {
    let sid = "snapshot_wait_metadata";
    let (mut worker, mut rx) = swarm_member(sid, "agent", true);
    worker.runtime.wait_reason = Some("stale persisted wait".into());
    worker.runtime.wait_age_secs = Some(999);
    let members = Arc::new(RwLock::new(HashMap::from([(sid.to_string(), worker)])));
    let guard = crate::wait_registry::register(sid, "Waiting for background task");
    broadcast_swarm_status_now(vec![sid.into()], &members).await;
    let ServerEvent::SwarmStatus { members: snapshot } = rx.try_recv().unwrap() else {
        panic!("expected swarm status");
    };
    assert_eq!(
        snapshot[0].runtime.wait_reason.as_deref(),
        Some("Waiting for background task")
    );
    assert_eq!(snapshot[0].runtime.wait_age_secs, Some(0));
    drop(guard);
    broadcast_swarm_status_now(vec![sid.into()], &members).await;
    let ServerEvent::SwarmStatus { members: snapshot } = rx.try_recv().unwrap() else {
        panic!("expected swarm status");
    };
    assert_eq!(snapshot[0].runtime.wait_reason, None);
    assert_eq!(snapshot[0].runtime.wait_age_secs, None);
}

#[tokio::test]
async fn update_member_status_prefers_explicit_report_back_owner_over_coordinator() {
    let swarm_members = Arc::new(RwLock::new(HashMap::new()));
    let swarms_by_id = Arc::new(RwLock::new(HashMap::from([(
        "swarm-1".to_string(),
        HashSet::from([
            "coord".to_string(),
            "owner".to_string(),
            "worker".to_string(),
        ]),
    )])));

    let (coord, mut coord_rx) = swarm_member("coord", "coordinator", false);
    let (owner, mut owner_rx) = swarm_member("owner", "agent", false);
    let (mut worker, _worker_rx) = swarm_member("worker", "agent", true);
    worker.status = "running".to_string();
    worker.detail = Some("doing task".to_string());
    worker.report_back_to_session_id = Some("owner".to_string());
    {
        let mut members = swarm_members.write().await;
        members.insert("coord".to_string(), coord);
        members.insert("owner".to_string(), owner);
        members.insert("worker".to_string(), worker);
    }

    update_member_status(
        "worker",
        "ready",
        None,
        &swarm_members,
        &swarms_by_id,
        None,
        None,
        None,
    )
    .await;

    let owner_events: Vec<_> = std::iter::from_fn(|| owner_rx.try_recv().ok()).collect();
    assert!(owner_events.iter().any(|event| {
        matches!(
            event,
            ServerEvent::Notification {
                notification_type: NotificationType::Message { .. },
                message,
                ..
            } if message.contains("finished their work and is ready for more")
        )
    }));
    let coord_events: Vec<_> = std::iter::from_fn(|| coord_rx.try_recv().ok()).collect();
    assert!(coord_events.iter().all(|event| {
        !matches!(
            event,
            ServerEvent::Notification {
                notification_type: NotificationType::Message { .. },
                message,
                ..
            } if message.contains("finished their work and is ready for more")
        )
    }));
}

#[tokio::test]
async fn update_member_status_includes_completion_report_in_owner_notification() {
    let swarm_members = Arc::new(RwLock::new(HashMap::new()));
    let swarms_by_id = Arc::new(RwLock::new(HashMap::from([(
        "swarm-1".to_string(),
        HashSet::from(["coord".to_string(), "worker".to_string()]),
    )])));

    let (coord, mut coord_rx) = swarm_member("coord", "coordinator", false);
    let (mut worker, _worker_rx) = swarm_member("worker", "agent", true);
    worker.status = "running".to_string();
    worker.report_back_to_session_id = Some("coord".to_string());
    {
        let mut members = swarm_members.write().await;
        members.insert("coord".to_string(), coord);
        members.insert("worker".to_string(), worker);
    }

    update_member_status_with_report(
        "worker",
        "ready",
        None,
        Some("Validated the parser and all tests passed.".to_string()),
        &swarm_members,
        &swarms_by_id,
        None,
        None,
        None,
    )
    .await;

    let events: Vec<_> = std::iter::from_fn(|| coord_rx.try_recv().ok()).collect();
    assert!(events.iter().any(|event| {
        matches!(
            event,
            ServerEvent::Notification {
                notification_type: NotificationType::Message { .. },
                message,
                ..
            } if message.contains("Recent output (automatically captured):\nValidated the parser")
                && !message.contains("No recent assistant output")
        )
    }));
}

#[tokio::test]
async fn automatic_output_tail_survives_forced_stop_and_is_delivered_once() {
    for status in ["stopped", "failed", "crashed"] {
        let members = Arc::new(RwLock::new(HashMap::new()));
        let swarms = Arc::new(RwLock::new(HashMap::from([(
            "swarm-1".to_string(),
            HashSet::from(["coord".to_string(), "worker".to_string()]),
        )])));
        let (coord, mut events) = swarm_member("coord", "coordinator", false);
        let (mut worker, _rx) = swarm_member("worker", "agent", true);
        worker.status = "running".into();
        worker.report_back_to_session_id = Some("coord".into());
        worker.output_tail = Some("Applied fix, running validation".into());
        worker.latest_completion_report = Some("stale previous turn".into());
        members
            .write()
            .await
            .extend([("coord".into(), coord), ("worker".into(), worker)]);
        for _ in 0..2 {
            update_member_status("worker", status, None, &members, &swarms, None, None, None).await;
        }
        let members = members.read().await;
        assert_eq!(
            members["worker"].latest_completion_report.as_deref(),
            Some("Applied fix, running validation")
        );
        assert!(members["worker"].output_tail.is_none());
        let notifications: Vec<_> = std::iter::from_fn(|| events.try_recv().ok())
            .filter_map(|event| match event {
                ServerEvent::Notification { message, .. } => Some(message),
                _ => None,
            })
            .collect();
        assert_eq!(notifications.len(), 1, "{status}: {notifications:?}");
        assert!(notifications[0].contains("Applied fix, running validation"));
        assert!(!notifications[0].contains("stale previous turn"));
    }
}

#[tokio::test]
async fn update_member_status_skips_noop_broadcasts() {
    let swarm_members = Arc::new(RwLock::new(HashMap::new()));
    let swarms_by_id = Arc::new(RwLock::new(HashMap::from([(
        "swarm-1".to_string(),
        HashSet::from(["worker".to_string()]),
    )])));

    let (worker, mut worker_rx) = swarm_member("worker", "agent", false);
    swarm_members
        .write()
        .await
        .insert("worker".to_string(), worker);

    update_member_status(
        "worker",
        "ready",
        None,
        &swarm_members,
        &swarms_by_id,
        None,
        None,
        None,
    )
    .await;

    assert!(worker_rx.try_recv().is_err());

    update_member_status(
        "worker",
        "busy",
        Some("working".to_string()),
        &swarm_members,
        &swarms_by_id,
        None,
        None,
        None,
    )
    .await;

    assert!(matches!(
        worker_rx.try_recv(),
        Ok(ServerEvent::SwarmStatus { members }) if members.len() == 1
            && members[0].session_id == "worker"
            && members[0].status == "busy"
            && members[0].detail.as_deref() == Some("working")
    ));
}

#[tokio::test]
async fn refresh_swarm_task_staleness_marks_running_tasks_stale_and_heartbeat_revives() {
    let swarm_members = Arc::new(RwLock::new(HashMap::new()));
    let swarms_by_id = Arc::new(RwLock::new(HashMap::from([(
        "swarm-1".to_string(),
        HashSet::from(["worker".to_string()]),
    )])));
    let swarm_coordinators = Arc::new(RwLock::new(HashMap::new()));
    let now_ms = now_unix_ms();
    let stale_age_ms = super::super::swarm_task_stale_after().as_millis() as u64 + 5_000;
    let swarm_plans = Arc::new(RwLock::new(HashMap::from([(
        "swarm-1".to_string(),
        VersionedPlan {
            items: vec![PlanItem {
                content: "task".to_string(),
                status: "running".to_string(),
                priority: "medium".to_string(),
                id: "task-1".to_string(),
                subsystem: None,
                file_scope: Vec::new(),
                blocked_by: Vec::new(),
                assigned_to: Some("worker".to_string()),
            }],
            version: 1,
            participants: HashSet::from(["worker".to_string()]),
            task_progress: HashMap::from([(
                "task-1".to_string(),
                crate::server::SwarmTaskProgress {
                    assigned_session_id: Some("worker".to_string()),
                    started_at_unix_ms: Some(now_ms.saturating_sub(stale_age_ms)),
                    last_heartbeat_unix_ms: Some(now_ms.saturating_sub(stale_age_ms)),
                    ..Default::default()
                },
            )]),
        },
    )])));
    let (worker, _worker_rx) = swarm_member("worker", "agent", true);
    swarm_members
        .write()
        .await
        .insert("worker".to_string(), worker);

    refresh_swarm_task_staleness(
        &swarm_members,
        &swarms_by_id,
        &swarm_plans,
        &swarm_coordinators,
    )
    .await;

    {
        let plans = swarm_plans.read().await;
        let plan = plans.get("swarm-1").expect("plan");
        assert_eq!(plan.items[0].status, "running_stale");
        assert!(
            plan.task_progress
                .get("task-1")
                .and_then(|progress| progress.stale_since_unix_ms)
                .is_some()
        );
    }

    let revived = touch_swarm_task_progress(
        "swarm-1",
        "task-1",
        Some("worker"),
        Some("still working".to_string()),
        Some("checkpoint saved".to_string()),
        &swarm_members,
        &swarms_by_id,
        &swarm_plans,
        &swarm_coordinators,
    )
    .await;
    assert!(revived);

    let plans = swarm_plans.read().await;
    let plan = plans.get("swarm-1").expect("plan");
    assert_eq!(plan.items[0].status, "running");
    let progress = plan.task_progress.get("task-1").expect("progress");
    assert_eq!(
        progress.checkpoint_summary.as_deref(),
        Some("checkpoint saved")
    );
    assert!(progress.stale_since_unix_ms.is_none());
}

#[test]
fn member_status_is_dead_matches_terminal_non_success_states() {
    for status in ["failed", "stopped", "crashed"] {
        assert!(member_status_is_dead(status), "{status} should be dead");
    }
    for status in ["ready", "running", "running_stale", "queued", "completed"] {
        assert!(!member_status_is_dead(status), "{status} should be alive");
    }
}

fn running_plan_assigned_to(
    assignee: &str,
    reclaims: Option<u32>,
) -> Arc<RwLock<HashMap<String, VersionedPlan>>> {
    Arc::new(RwLock::new(HashMap::from([(
        "swarm-1".to_string(),
        VersionedPlan {
            items: vec![PlanItem {
                content: "task".to_string(),
                status: "running".to_string(),
                priority: "medium".to_string(),
                id: "task-1".to_string(),
                subsystem: None,
                file_scope: Vec::new(),
                blocked_by: Vec::new(),
                assigned_to: Some(assignee.to_string()),
            }],
            version: 1,
            participants: HashSet::from([assignee.to_string()]),
            task_progress: HashMap::from([(
                "task-1".to_string(),
                crate::server::SwarmTaskProgress {
                    assigned_session_id: Some(assignee.to_string()),
                    dead_assignee_reclaims: reclaims,
                    ..Default::default()
                },
            )]),
        },
    )])))
}

#[tokio::test]
async fn salvage_requeues_dead_members_tasks_and_notifies_coordinator() {
    let swarm_members = Arc::new(RwLock::new(HashMap::new()));
    let swarms_by_id = Arc::new(RwLock::new(HashMap::from([(
        "swarm-1".to_string(),
        HashSet::from(["coord".to_string(), "worker".to_string()]),
    )])));
    let swarm_coordinators = Arc::new(RwLock::new(HashMap::from([(
        "swarm-1".to_string(),
        "coord".to_string(),
    )])));
    let swarm_plans = running_plan_assigned_to("worker", None);
    let (coord, mut coord_rx) = swarm_member("coord", "coordinator", false);
    let (worker, _worker_rx) = swarm_member("worker", "agent", true);
    {
        let mut members = swarm_members.write().await;
        members.insert("coord".to_string(), coord);
        members.insert("worker".to_string(), worker);
    }

    let outcome = salvage_assignments_of_dead_member(
        "worker",
        "swarm-1",
        &swarm_members,
        &swarms_by_id,
        &swarm_plans,
        &swarm_coordinators,
    )
    .await;

    assert_eq!(outcome.requeued_task_ids, vec!["task-1".to_string()]);
    assert!(outcome.failed_task_ids.is_empty());
    {
        let plans = swarm_plans.read().await;
        let plan = plans.get("swarm-1").expect("plan");
        assert_eq!(plan.items[0].status, "queued");
        assert_eq!(plan.items[0].assigned_to, None);
        let progress = plan.task_progress.get("task-1").expect("progress");
        assert_eq!(progress.assigned_session_id, None);
        assert_eq!(progress.dead_assignee_reclaims, Some(1));
    }

    let coord_events: Vec<_> = std::iter::from_fn(|| coord_rx.try_recv().ok()).collect();
    assert!(
        coord_events.iter().any(|event| matches!(
            event,
            ServerEvent::Notification { message, .. }
                if message.contains("died") && message.contains("task-1")
        )),
        "coordinator should be told about the salvage, got {coord_events:?}"
    );
}

#[tokio::test]
async fn salvage_fails_task_once_reclaim_cap_is_reached() {
    let swarm_members = Arc::new(RwLock::new(HashMap::new()));
    let swarms_by_id = Arc::new(RwLock::new(HashMap::from([(
        "swarm-1".to_string(),
        HashSet::from(["worker".to_string()]),
    )])));
    let swarm_coordinators = Arc::new(RwLock::new(HashMap::new()));
    let swarm_plans =
        running_plan_assigned_to("worker", Some(crate::plan::MAX_DEAD_ASSIGNEE_RECLAIMS));
    let (worker, _worker_rx) = swarm_member("worker", "agent", true);
    swarm_members
        .write()
        .await
        .insert("worker".to_string(), worker);

    let outcome = salvage_assignments_of_dead_member(
        "worker",
        "swarm-1",
        &swarm_members,
        &swarms_by_id,
        &swarm_plans,
        &swarm_coordinators,
    )
    .await;

    assert!(outcome.requeued_task_ids.is_empty());
    assert_eq!(outcome.failed_task_ids, vec!["task-1".to_string()]);
    let plans = swarm_plans.read().await;
    let plan = plans.get("swarm-1").expect("plan");
    assert_eq!(plan.items[0].status, "failed");
    assert_eq!(plan.items[0].assigned_to, None);
}

#[tokio::test]
async fn remove_session_from_swarm_salvages_running_assignments() {
    let swarm_members = Arc::new(RwLock::new(HashMap::new()));
    let swarms_by_id = Arc::new(RwLock::new(HashMap::from([(
        "swarm-1".to_string(),
        HashSet::from(["coord".to_string(), "worker".to_string()]),
    )])));
    let swarm_coordinators = Arc::new(RwLock::new(HashMap::from([(
        "swarm-1".to_string(),
        "coord".to_string(),
    )])));
    let swarm_plans = running_plan_assigned_to("worker", None);
    let (coord, _coord_rx) = swarm_member("coord", "coordinator", false);
    let (worker, _worker_rx) = swarm_member("worker", "agent", true);
    {
        let mut members = swarm_members.write().await;
        members.insert("coord".to_string(), coord);
        members.insert("worker".to_string(), worker);
    }

    remove_session_from_swarm(
        "worker",
        "swarm-1",
        &swarm_members,
        &swarms_by_id,
        &swarm_coordinators,
        &swarm_plans,
    )
    .await;

    let plans = swarm_plans.read().await;
    let plan = plans.get("swarm-1").expect("plan");
    assert_eq!(plan.items[0].status, "queued");
    assert_eq!(plan.items[0].assigned_to, None);
}

#[tokio::test]
async fn staleness_sweep_salvages_tasks_of_vanished_assignee() {
    // The assignee is not a swarm member at all (zombie left over from a
    // previous process): no grace period applies and the sweep must
    // requeue its running task.
    let swarm_members = Arc::new(RwLock::new(HashMap::new()));
    let swarms_by_id = Arc::new(RwLock::new(HashMap::from([(
        "swarm-1".to_string(),
        HashSet::from(["coord".to_string()]),
    )])));
    let swarm_coordinators = Arc::new(RwLock::new(HashMap::from([(
        "swarm-1".to_string(),
        "coord".to_string(),
    )])));
    let swarm_plans = running_plan_assigned_to("ghost", None);
    // Give the task a fresh heartbeat so the first sweep phase does not
    // interfere; the salvage phase must still fire on the dead assignee.
    {
        let mut plans = swarm_plans.write().await;
        let plan = plans.get_mut("swarm-1").expect("plan");
        let progress = plan.task_progress.get_mut("task-1").expect("progress");
        progress.last_heartbeat_unix_ms = Some(now_unix_ms());
    }
    let (coord, _coord_rx) = swarm_member("coord", "coordinator", false);
    swarm_members
        .write()
        .await
        .insert("coord".to_string(), coord);

    refresh_swarm_task_staleness(
        &swarm_members,
        &swarms_by_id,
        &swarm_plans,
        &swarm_coordinators,
    )
    .await;

    let plans = swarm_plans.read().await;
    let plan = plans.get("swarm-1").expect("plan");
    assert_eq!(plan.items[0].status, "queued");
    assert_eq!(plan.items[0].assigned_to, None);
}

#[tokio::test]
async fn staleness_sweep_grants_grace_to_recently_crashed_member() {
    // A member marked crashed moments ago may be mid reload-recovery; the
    // sweep must not reclaim its work inside the grace window.
    let swarm_members = Arc::new(RwLock::new(HashMap::new()));
    let swarms_by_id = Arc::new(RwLock::new(HashMap::from([(
        "swarm-1".to_string(),
        HashSet::from(["worker".to_string()]),
    )])));
    let swarm_coordinators = Arc::new(RwLock::new(HashMap::new()));
    let swarm_plans = running_plan_assigned_to("worker", None);
    {
        let mut plans = swarm_plans.write().await;
        let plan = plans.get_mut("swarm-1").expect("plan");
        let progress = plan.task_progress.get_mut("task-1").expect("progress");
        progress.last_heartbeat_unix_ms = Some(now_unix_ms());
    }
    let (mut worker, _worker_rx) = swarm_member("worker", "agent", true);
    worker.status = "crashed".to_string();
    worker.last_status_change = Instant::now();
    swarm_members
        .write()
        .await
        .insert("worker".to_string(), worker);

    refresh_swarm_task_staleness(
        &swarm_members,
        &swarms_by_id,
        &swarm_plans,
        &swarm_coordinators,
    )
    .await;

    let plans = swarm_plans.read().await;
    let plan = plans.get("swarm-1").expect("plan");
    assert_eq!(plan.items[0].status, "running");
    assert_eq!(plan.items[0].assigned_to.as_deref(), Some("worker"));
}

#[tokio::test]
async fn update_member_status_notifies_owner_when_worker_crashes_mid_task() {
    let swarm_members = Arc::new(RwLock::new(HashMap::new()));
    let swarms_by_id = Arc::new(RwLock::new(HashMap::from([(
        "swarm-1".to_string(),
        HashSet::from(["owner".to_string(), "worker".to_string()]),
    )])));
    let (owner, mut owner_rx) = swarm_member("owner", "coordinator", false);
    let (mut worker, _worker_rx) = swarm_member("worker", "agent", true);
    worker.status = "running".to_string();
    worker.report_back_to_session_id = Some("owner".to_string());
    {
        let mut members = swarm_members.write().await;
        members.insert("owner".to_string(), owner);
        members.insert("worker".to_string(), worker);
    }

    update_member_status(
        "worker",
        "crashed",
        Some("client disconnected while processing".to_string()),
        &swarm_members,
        &swarms_by_id,
        None,
        None,
        None,
    )
    .await;

    let owner_events: Vec<_> = std::iter::from_fn(|| owner_rx.try_recv().ok()).collect();
    assert!(
        owner_events.iter().any(|event| matches!(
            event,
            ServerEvent::Notification { message, .. }
                if message.contains("crashed while working")
        )),
        "owner should be notified of the crash, got {owner_events:?}"
    );
}
