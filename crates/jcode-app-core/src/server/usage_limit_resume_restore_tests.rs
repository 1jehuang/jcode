// Driver-level recovery regressions. Included in usage_limit_resume::tests.

struct ResumeTestHome {
    previous: Option<std::ffi::OsString>,
    _directory: tempfile::TempDir,
}

impl ResumeTestHome {
    fn new() -> Self {
        let directory = tempfile::tempdir().unwrap();
        let previous = std::env::var_os("JCODE_HOME");
        crate::env::set_var("JCODE_HOME", directory.path());
        Self {
            previous,
            _directory: directory,
        }
    }
}

impl Drop for ResumeTestHome {
    fn drop(&mut self) {
        if let Some(previous) = self.previous.take() {
            crate::env::set_var("JCODE_HOME", previous);
        } else {
            crate::env::remove_var("JCODE_HOME");
        }
    }
}

#[derive(Clone)]
struct RestoreTestProvider {
    calls: Arc<std::sync::atomic::AtomicUsize>,
}

#[async_trait::async_trait]
impl crate::provider::Provider for RestoreTestProvider {
    async fn complete(
        &self,
        _: &[crate::message::Message],
        _: &[crate::message::ToolDefinition],
        _: &str,
        _: Option<&str>,
    ) -> anyhow::Result<crate::provider::EventStream> {
        self.calls.fetch_add(1, Ordering::SeqCst);
        Ok(Box::pin(futures::stream::iter(vec![
            Ok(crate::message::StreamEvent::TextDelta(
                "continued original turn".into(),
            )),
            Ok(crate::message::StreamEvent::MessageEnd { stop_reason: None }),
        ])))
    }

    fn name(&self) -> &str {
        "test"
    }
    fn fork(&self) -> Arc<dyn crate::provider::Provider> {
        Arc::new(self.clone())
    }
}

#[tokio::test(start_paused = true)]
async fn disk_only_wait_restores_deadline_and_original_transcript() {
    let _env = crate::storage::lock_test_env();
    let _home = ResumeTestHome::new();
    let calls = Arc::new(std::sync::atomic::AtomicUsize::new(0));
    let provider: Arc<dyn crate::provider::Provider> = Arc::new(RestoreTestProvider {
        calls: calls.clone(),
    });
    let registry = crate::tool::Registry::new(provider.clone()).await;
    let mut original = Agent::new(provider, registry);
    original
        .append_user_context_message("original assignment", vec![])
        .unwrap();
    let session_id = original.session_id().to_string();
    let original_messages = original.message_count();
    let agent = Arc::new(AsyncMutex::new(original));
    let state = ResumeState {
        session_id: session_id.clone(),
        generation: "before-restart".into(),
        resume_at: Utc::now() + chrono::Duration::hours(2),
        quota_resumes_used: 2,
        overload_resumes_used: 1,
        kind: ResumeKind::UsageLimit,
        system_reminder: Some("preserved reminder".into()),
        in_flight: false,
        terminal: false,
    };
    assert!(durable::save(&state, None).unwrap());
    // Losing a task/ticket, as on daemon reload, must not delete its wait.
    drop(register_pending_resume(&session_id));
    assert!(has_pending_resume(&session_id));
    let restored = pending_state(&session_id).unwrap().unwrap();
    let swarm = LiveTurnSwarmContext {
        recovered_assignment: None,
        recovery_reload_id: None,
        recovery_swarm: None,
        members: Arc::new(tokio::sync::RwLock::new(HashMap::new())),
        swarms_by_id: Arc::new(tokio::sync::RwLock::new(HashMap::new())),
        event_history: Arc::new(tokio::sync::RwLock::new(std::collections::VecDeque::new())),
        event_counter: Arc::new(AtomicU64::new(0)),
        event_tx: tokio::sync::broadcast::channel(16).0,
    };
    let (event_tx, mut events) = mpsc::unbounded_channel();
    let held = agent.clone().lock_owned().await;
    let run_agent = agent.clone();
    let task = tokio::spawn(async move {
        restore_turn_after_provider_wait(
            run_agent,
            Some(held),
            restored,
            &event_tx,
            &swarm,
            &no_wait_hook,
            None,
        )
        .await
    });
    let event = events.recv().await.unwrap();
    assert!(matches!(
        event,
        ServerEvent::Error {
            server_resumes: true,
            ..
        }
    ));
    assert_eq!(calls.load(Ordering::SeqCst), 0);
    tokio::time::advance(Duration::from_secs(7190)).await;
    tokio::task::yield_now().await;
    assert_eq!(
        calls.load(Ordering::SeqCst),
        0,
        "restore must not request before original reset"
    );
    // A harmless status inspection holding the mutex exactly at reset must
    // not cause the durable wait to be discarded as superseded.
    let inspection = agent.clone().lock_owned().await;
    tokio::time::advance(Duration::from_secs(20)).await;
    tokio::task::yield_now().await;
    assert_eq!(calls.load(Ordering::SeqCst), 0);
    assert!(
        !task.is_finished(),
        "transient mutex contention must not abandon the wait"
    );
    assert!(pending_state(&session_id).unwrap().is_some());
    drop(inspection);
    let ResumeOutcome::Completed { guard, .. } = task.await.unwrap() else {
        panic!("restored wait should complete");
    };
    assert_eq!(calls.load(Ordering::SeqCst), 1);
    assert_eq!(
        guard.message_count(),
        original_messages + 1,
        "restore must not append a synthetic user turn"
    );
    assert!(pending_state(&session_id).unwrap().is_none());
    assert!(!has_pending_resume(&session_id));
}

#[test]
fn cancel_disk_only_wait_and_stale_generation_cannot_replace_successor() {
    let _env = crate::storage::lock_test_env();
    let _home = ResumeTestHome::new();
    let mut state = ResumeState {
        session_id: "disk-only-cancel".into(),
        generation: "first".into(),
        resume_at: Utc::now() + chrono::Duration::hours(2),
        quota_resumes_used: 2,
        overload_resumes_used: 0,
        kind: ResumeKind::UsageLimit,
        system_reminder: None,
        in_flight: false,
        terminal: false,
    };
    durable::save(&state, None).unwrap();
    state.generation = "successor".into();
    let successor = register_durable_resume(&state, None).unwrap().unwrap();
    let mut stale = state.clone();
    stale.generation = "first".into();
    assert!(
        register_durable_resume(&stale, Some("first"))
            .unwrap()
            .is_none()
    );
    assert!(!successor.token.is_cancelled());
    assert!(!clear_current_wait(&state.session_id, "first").unwrap());
    drop(successor);
    assert!(has_pending_resume(&state.session_id));
    assert!(cancel_pending_resume(&state.session_id));
    assert!(pending_state(&state.session_id).unwrap().is_none());
    assert!(!has_pending_resume(&state.session_id));
}

#[tokio::test]
async fn stop_before_first_registration_cannot_leave_durable_orphan() {
    let _env = crate::storage::lock_test_env();
    let _home = ResumeTestHome::new();
    let session_id = format!("stop-before-save-{}", uuid::Uuid::new_v4());
    let owner = "registration-stop-owner";
    let publication = super::super::swarm::begin_turn_publication(&session_id).await;
    let (tx, _rx) = mpsc::unbounded_channel();
    let member = super::super::SwarmMember::from_record(
        jcode_swarm_core::SwarmMemberRecord {
            session_id: session_id.clone(),
            working_dir: None,
            swarm_id: None,
            swarm_enabled: false,
            status: jcode_swarm_core::SwarmLifecycleStatus::Running,
            detail: None,
            task_label: None,
            friendly_name: None,
            report_back_to_session_id: Some(owner.into()),
            latest_completion_report: None,
            role: jcode_swarm_core::SwarmRole::Agent,
            is_headless: true,
        },
        tx,
    );
    let members = Arc::new(tokio::sync::RwLock::new(HashMap::from([(
        session_id.clone(),
        member,
    )])));
    let swarms = Arc::new(tokio::sync::RwLock::new(HashMap::new()));
    // Deterministically place Stop between provider failure and the very
    // first registration. No RAM ticket or durable record exists yet.
    let stopped = super::super::swarm::cancel_spawned_descendant_turns(
        owner, &members, &swarms, None, None, None,
    )
    .await;
    assert_eq!(stopped, vec![session_id.clone()]);
    let state = ResumeState {
        session_id: session_id.clone(),
        generation: "stopped-turn".into(),
        resume_at: Utc::now() + chrono::Duration::hours(2),
        quota_resumes_used: 0,
        overload_resumes_used: 0,
        kind: ResumeKind::UsageLimit,
        system_reminder: None,
        in_flight: false,
        terminal: false,
    };
    assert!(
        register_owned_durable_resume(&state, None, Some(&publication))
            .await
            .unwrap()
            .is_none()
    );
    drop(publication);
    assert!(!has_pending_resume(&session_id));
    assert!(
        pending_state(&session_id).unwrap().is_none(),
        "a daemon restart must not discover an orphan written after Stop"
    );
    assert_eq!(members.read().await[&session_id].status, "stopped");
}

#[tokio::test(start_paused = true)]
async fn superseded_driver_cleanup_preserves_successor_durable_record() {
    let _env = crate::storage::lock_test_env();
    let _home = ResumeTestHome::new();
    let calls = Arc::new(std::sync::atomic::AtomicUsize::new(0));
    let provider: Arc<dyn crate::provider::Provider> = Arc::new(RestoreTestProvider {
        calls: calls.clone(),
    });
    let registry = crate::tool::Registry::new(provider.clone()).await;
    let agent = Arc::new(AsyncMutex::new(Agent::new(provider, registry)));
    let held = agent.clone().lock_owned().await;
    let session_id = held.session_id().to_string();
    let members = Arc::new(tokio::sync::RwLock::new(HashMap::new()));
    let swarms = Arc::new(tokio::sync::RwLock::new(HashMap::new()));
    let (swarm_tx, _) = tokio::sync::broadcast::channel(16);
    let swarm = LiveTurnSwarmContext::new(
        &members,
        &swarms,
        &Arc::new(tokio::sync::RwLock::new(std::collections::VecDeque::new())),
        &Arc::new(AtomicU64::new(0)),
        &swarm_tx,
    );
    let (tx, _events) = mpsc::unbounded_channel();
    let old = super::super::swarm::begin_turn_publication(&session_id).await;
    let mut resume = Box::pin(resume_turn_after_usage_limit_with_publication(
        agent.clone(),
        Some(held),
        &session_id,
        limit(Some(Duration::from_secs(7200))),
        None,
        &tx,
        &swarm,
        &no_wait_hook,
        Some(&old),
    ));
    assert!(futures::poll!(resume.as_mut()).is_pending());
    let original = pending_state(&session_id).unwrap().unwrap();
    let successor = super::super::swarm::begin_turn_publication(&session_id).await;
    let mut replacement = original.clone();
    replacement.generation = uuid::Uuid::new_v4().to_string();
    replacement.resume_at += chrono::Duration::hours(1);
    replacement.quota_resumes_used = 1;
    let successor_ticket = register_owned_durable_resume(&replacement, None, Some(&successor))
        .await
        .unwrap()
        .unwrap();
    assert!(matches!(resume.await, ResumeOutcome::Superseded));
    let remaining = pending_state(&session_id).unwrap().unwrap();
    assert_eq!(remaining.generation, replacement.generation);
    assert_eq!(remaining.resume_at, replacement.resume_at);
    assert_eq!(remaining.quota_resumes_used, 1);
    assert!(!successor_ticket.token.is_cancelled());
    assert!(has_pending_resume(&session_id));
    assert_eq!(calls.load(Ordering::SeqCst), 0);
    drop(successor_ticket);
    assert!(cancel_pending_resume(&session_id));
}
