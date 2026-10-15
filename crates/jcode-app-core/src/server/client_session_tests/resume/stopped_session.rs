use super::*;
use crate::server::live_turn::admit_client_turn;
use crate::server::state::{begin_session_interrupt_delivery, begin_session_interrupt_stop};

struct ResumeEnv {
    home: tempfile::TempDir,
    _runtime: tempfile::TempDir,
    previous: Vec<(&'static str, Option<std::ffi::OsString>)>,
}

impl ResumeEnv {
    fn new() -> Result<Self> {
        let home = tempfile::tempdir()?;
        let (runtime, previous_runtime) = setup_runtime_dir()?;
        let previous = vec![
            ("JCODE_RUNTIME_DIR", previous_runtime),
            ("JCODE_HOME", std::env::var_os("JCODE_HOME")),
            ("JCODE_NO_TELEMETRY", std::env::var_os("JCODE_NO_TELEMETRY")),
        ];
        crate::env::set_var("JCODE_HOME", home.path());
        crate::env::set_var("JCODE_NO_TELEMETRY", "1");
        Ok(Self {
            home,
            _runtime: runtime,
            previous,
        })
    }
}

impl Drop for ResumeEnv {
    fn drop(&mut self) {
        for (key, value) in self.previous.drain(..) {
            match value {
                Some(value) => crate::env::set_var(key, value),
                None => crate::env::remove_var(key),
            }
        }
    }
}

#[derive(Clone, Copy, PartialEq)]
enum ResumeCase {
    Stopped,
    MissingDisk,
    Stopping,
    StopDuringResume,
}

#[tokio::test]
async fn stopped_same_client_disk_resume_admits_client_turn() -> Result<()> {
    check_disk_resume(ResumeCase::Stopped).await
}

#[tokio::test]
async fn failed_same_client_disk_resume_keeps_admission_closed() -> Result<()> {
    check_disk_resume(ResumeCase::MissingDisk).await
}

#[tokio::test]
async fn stopping_same_client_disk_resume_keeps_admission_closed() -> Result<()> {
    check_disk_resume(ResumeCase::Stopping).await
}

#[tokio::test]
async fn stop_between_restore_and_registration_keeps_admission_closed() -> Result<()> {
    check_disk_resume(ResumeCase::StopDuringResume).await
}

async fn check_disk_resume(case: ResumeCase) -> Result<()> {
    let _lock = crate::storage::lock_test_env();
    let env = ResumeEnv::new()?;
    let session_id = match case {
        ResumeCase::Stopped => "session_resume_stopped",
        ResumeCase::MissingDisk => "session_resume_failed",
        ResumeCase::Stopping => "session_resume_stopping",
        ResumeCase::StopDuringResume => "session_resume_stop_race",
    };
    let mut persisted = crate::session::Session::create_with_id(session_id.into(), None, None);
    persisted.save_prepared()?;
    assert!(crate::session::Session::load(session_id).is_ok());
    let provider: Arc<dyn Provider> = Arc::new(MockProvider);
    let registry = Registry::new(provider.clone()).await;
    let agent = Arc::new(Mutex::new(build_test_agent_with_id(
        provider.clone(),
        registry.clone(),
        session_id,
        Vec::new(),
    )));
    let sessions = Arc::new(RwLock::new(HashMap::from([(
        session_id.into(),
        agent.clone(),
    )])));
    let queues: SessionInterruptQueues = Arc::new(RwLock::new(HashMap::new()));
    crate::server::register_session_interrupt_queue(
        &queues,
        session_id,
        agent.lock().await.soft_interrupt_queue(),
    )
    .await;
    let members = Arc::new(RwLock::new(HashMap::from([
        (session_id.into(), test_swarm_member(session_id, "ready")),
        ("coord".into(), test_swarm_member("coord", "ready")),
    ])));
    let swarms = Arc::new(RwLock::new(HashMap::from([(
        "swarm-test".into(),
        HashSet::from([session_id.into(), "coord".into()]),
    )])));
    let coordinators = Arc::new(RwLock::new(HashMap::from([(
        "swarm-test".into(),
        "coord".into(),
    )])));
    let plans = Arc::new(RwLock::new(HashMap::new()));
    let subscriptions = Arc::new(RwLock::new(HashMap::new()));
    let subscriptions_by_session = Arc::new(RwLock::new(HashMap::new()));
    let history = Arc::new(RwLock::new(VecDeque::new()));
    let counter = Arc::new(std::sync::atomic::AtomicU64::new(0));
    let (swarm_event_tx, _swarm_event_rx) = broadcast::channel(8);
    let now = Instant::now();
    let connections = Arc::new(RwLock::new(HashMap::from([(
        "conn".into(),
        ClientConnectionInfo {
            client_id: "conn".into(),
            session_id: session_id.into(),
            client_instance_id: Some("same-client".into()),
            debug_client_id: None,
            connected_at: now,
            last_seen: now,
            is_processing: false,
            current_tool_name: None,
            terminal_env: Vec::new(),
            disconnect_tx: mpsc::unbounded_channel().0,
        },
    )])));

    let stop = tokio::task::unconstrained(async {
        let (stop_tx, mut stop_rx) = mpsc::unbounded_channel();
        crate::server::comm_session::handle_comm_stop(
            1,
            "coord".into(),
            session_id.into(),
            false,
            &stop_tx,
            &sessions,
            &members,
            &swarms,
            &coordinators,
            &plans,
            &subscriptions,
            &subscriptions_by_session,
            &history,
            &counter,
            &swarm_event_tx,
            &queues,
            &crate::server::swarm_mutation_state::SwarmMutationRuntime::default(),
        )
        .await;
        assert!(matches!(
            stop_rx.try_recv(),
            Ok(ServerEvent::Done { id: 1 })
        ));
        assert!(!sessions.read().await.contains_key(session_id));
        assert!(!members.read().await.contains_key(session_id));
        assert!(!queues.read().await.contains_key(session_id));
    });
    tokio::pin!(stop);
    if case == ResumeCase::Stopping {
        // An incomplete stop must stay closed even when disk restore succeeds.
        begin_session_interrupt_stop(session_id).await;
        members.write().await.get_mut(session_id).unwrap().status = "stopping".into();
    } else if case != ResumeCase::StopDuringResume {
        stop.as_mut().await;
        assert!(agent.lock().await.is_closed());
    }
    if case != ResumeCase::StopDuringResume {
        assert!(begin_session_interrupt_delivery(session_id).is_none());
        assert!(admit_client_turn(&agent, None, 2).await.is_err());
    }
    assert_eq!(connections.read().await["conn"].session_id, session_id);

    if case == ResumeCase::MissingDisk {
        // Move only this test's isolated snapshots and backups out of discovery.
        std::fs::rename(
            env.home.path().join("sessions"),
            env.home.path().join("saved-sessions"),
        )?;
        assert!(crate::session::Session::load(session_id).is_err());
    }
    let (writer, _peer) = test_writer()?;
    let (event_tx, mut event_rx) = mpsc::unbounded_channel();
    let mut client_selfdev = false;
    let mut client_session_id = session_id.to_string();
    let restored = {
        let resume = tokio::task::unconstrained(async {
            handle_resume_session(
                3,
                session_id.into(),
                None,
                Some("same-client"),
                false,
                false,
                &mut client_selfdev,
                &mut client_session_id,
                "conn",
                &agent,
                &provider,
                &registry,
                &sessions,
                &Arc::new(RwLock::new(HashMap::new())),
                &queues,
                &connections,
                &Arc::new(RwLock::new(ClientDebugState::default())),
                &members,
                &swarms,
                &FileTouchService::new(),
                &subscriptions,
                &subscriptions_by_session,
                &plans,
                &coordinators,
                &Arc::new(RwLock::new(1)),
                &writer,
                "test-server",
                "test",
                &event_tx,
                &Arc::new(crate::mcp::SharedMcpPool::from_default_config()),
                &history,
                &counter,
                &swarm_event_tx,
                false,
            )
            .await
        });
        tokio::pin!(resume);
        if case == ResumeCase::StopDuringResume {
            // Queue stop's snapshot read before resume's rename write. Both
            // futures run only when polled here, with cooperative yields disabled.
            let queues_guard = queues.write().await;
            assert!(futures::poll!(stop.as_mut()).is_pending());
            assert!(futures::poll!(resume.as_mut()).is_pending());
            assert!(!agent.lock().await.is_closed());
            assert!(Arc::ptr_eq(&sessions.read().await[session_id], &agent));
            drop(queues_guard);

            // Stop now owns the terminal Agent, waiting for queue removal behind
            // resume's rename. Resume completes rename, then waits for that Agent.
            assert!(futures::poll!(stop.as_mut()).is_pending());
            assert!(agent.try_lock().is_err());
            assert!(!sessions.read().await.contains_key(session_id));
            assert!(futures::poll!(resume.as_mut()).is_pending());
            stop.as_mut().await;
            assert!(begin_session_interrupt_delivery(session_id).is_none());
        }
        resume.await?
    };
    assert!(Arc::ptr_eq(&agent, &restored));
    assert_eq!(client_session_id, session_id);

    if case == ResumeCase::MissingDisk {
        assert!(
            matches!(event_rx.try_recv(), Ok(ServerEvent::Error { id: 3, message, .. })
            if message.contains("Failed to restore session"))
        );
        assert!(
            event_rx.try_recv().is_err(),
            "failed restore must not acknowledge Done"
        );
        assert!(agent.lock().await.is_closed());
        assert!(!sessions.read().await.contains_key(session_id));
        assert!(!queues.read().await.contains_key(session_id));
    } else {
        let events = collect_events_until_done(&mut event_rx, 3).await;
        assert!(
            events
                .iter()
                .any(|event| matches!(event, ServerEvent::Done { id: 3 }))
        );
        assert!(
            !events
                .iter()
                .any(|event| matches!(event, ServerEvent::Error { .. }))
        );
        if case == ResumeCase::StopDuringResume {
            assert!(agent.lock().await.is_closed());
            assert!(
                begin_session_interrupt_delivery(session_id).is_none(),
                "late resume registration must not reopen a completed stop"
            );
            assert!(!sessions.read().await.contains_key(session_id));
            assert!(!queues.read().await.contains_key(session_id));
        } else {
            assert!(!agent.lock().await.is_closed());
            assert!(Arc::ptr_eq(&sessions.read().await[session_id], &agent));
        }
    }

    let admitted = admit_client_turn(&restored, None, 4).await;
    if case == ResumeCase::Stopped {
        let reservation =
            admitted.expect("successful explicit disk resume must admit the next client turn");
        assert_eq!(reservation.session_id(), session_id);
        assert!(
            agent.try_lock().is_err(),
            "actual admission must reserve the Agent"
        );
        let queue = queues
            .read()
            .await
            .get(session_id)
            .cloned()
            .expect("restored interrupt queue");
        assert!(Arc::ptr_eq(&queue, &reservation.soft_interrupt_queue()));
    } else {
        assert!(
            matches!(admitted, Err(ServerEvent::Error { id: 4, message, .. })
            if message.contains("stopped"))
        );
        assert!(begin_session_interrupt_delivery(session_id).is_none());
        if case == ResumeCase::Stopping {
            assert_eq!(members.read().await[session_id].status, "stopping");
        }
    }
    Ok(())
}
