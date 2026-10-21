// A swarm worker's task turn that hits a subscription usage limit stays in
// progress (member `rate_limited`, plan item still `running`) and resumes the
// same turn after the reset instead of failing the task.

#[derive(Clone)]
struct CascadeBlockedTaskProvider {
    calls: Arc<std::sync::atomic::AtomicUsize>,
    limited: bool,
}

#[async_trait]
impl Provider for CascadeBlockedTaskProvider {
    async fn complete(
        &self,
        _messages: &[Message],
        _tools: &[ToolDefinition],
        _system: &str,
        _resume_session_id: Option<&str>,
    ) -> Result<EventStream> {
        self.calls.fetch_add(1, std::sync::atomic::Ordering::SeqCst);
        if self.limited {
            return Ok(Box::pin(stream::iter(vec![Err(
                jcode_provider_core::usage_limit_resume::with_usage_limit_reset(
                    anyhow::anyhow!("Usage limit reached for the assigned worker"),
                    Some(Duration::from_secs(20)),
                ),
            )])));
        }
        futures::future::pending().await
    }
    fn name(&self) -> &str {
        "test"
    }
    fn fork(&self) -> Arc<dyn Provider> {
        Arc::new(self.clone())
    }
}

async fn assigned_task_cascade_stays_stopped(limited: bool) {
    let (_env, _runtime) = RuntimeEnvGuard::new();
    let home = tempfile::TempDir::new().expect("isolated home");
    let prev_home = std::env::var_os("JCODE_HOME");
    crate::env::set_var("JCODE_HOME", home.path());
    let swarm_id = "assigned-cascade-swarm";
    let coord = "assigned-cascade-owner";
    let worker = "assigned-cascade-worker";
    let calls = Arc::new(std::sync::atomic::AtomicUsize::new(0));
    let provider: Arc<dyn Provider> = Arc::new(CascadeBlockedTaskProvider {
        calls: calls.clone(),
        limited,
    });
    let registry = Registry::new(provider.clone()).await;
    let agent = Arc::new(Mutex::new(Agent::new(provider, registry)));
    let mut worker_member = owned_member(worker, swarm_id, "queued", coord);
    worker_member.is_headless = true;
    let (tx, mut rx) = mpsc::unbounded_channel();
    worker_member.event_tx = tx;
    let members = Arc::new(RwLock::new(HashMap::from([
        (coord.to_string(), member(coord, swarm_id, "ready")),
        (worker.to_string(), worker_member),
    ])));
    let swarms = Arc::new(RwLock::new(HashMap::from([(
        swarm_id.to_string(),
        HashSet::from([coord.to_string(), worker.to_string()]),
    )])));
    let mut plan = VersionedPlan::new();
    let mut item = plan_item("assigned-cascade-task", "queued", "high", &[]);
    item.assigned_to = Some(worker.to_string());
    plan.items.push(item);
    let plans = Arc::new(RwLock::new(HashMap::from([(swarm_id.to_string(), plan)])));
    let runner = super::spawn_assigned_task_run(
        agent.clone(),
        worker.to_string(),
        swarm_id.to_string(),
        "assigned-cascade-task".to_string(),
        "original assigned request".to_string(),
        members.clone(),
        swarms.clone(),
        plans.clone(),
        Arc::new(RwLock::new(HashMap::from([(
            swarm_id.to_string(),
            coord.to_string(),
        )]))),
        Arc::new(RwLock::new(VecDeque::new())),
        Arc::new(AtomicU64::new(1)),
        broadcast::channel(256).0,
    );
    let mut reached_barrier = false;
    for _ in 0..20_000 {
        let requested = calls.load(std::sync::atomic::Ordering::SeqCst) == 1;
        let waiting = !limited
            || (members.read().await[worker].status == "rate_limited"
                && crate::server::usage_limit_resume::has_pending_resume(worker));
        if requested && waiting {
            reached_barrier = true;
            break;
        }
        tokio::task::yield_now().await;
    }
    assert!(
        reached_barrier,
        "assigned task must reach its provider/wait barrier"
    );
    let stopped = crate::server::swarm::cancel_spawned_descendant_turns(
        coord, &members, &swarms, None, None, None,
    )
    .await;
    assert_eq!(stopped, vec![worker.to_string()]);
    tokio::time::timeout(Duration::from_secs(3), runner)
        .await
        .expect("cascade must terminate the assigned runner")
        .unwrap();
    tokio::time::sleep(Duration::from_secs(100)).await;
    assert_eq!(
        calls.load(std::sync::atomic::Ordering::SeqCst),
        1,
        "cancelled initial requests and quota timers must not call the provider again"
    );
    let state = members.read().await;
    assert_eq!(state[worker].status, "stopped");
    assert!(state[worker].latest_completion_report.is_none());
    drop(state);
    assert_ne!(plans.read().await[swarm_id].items[0].status, "done");
    assert!(!crate::server::usage_limit_resume::has_pending_resume(
        worker
    ));
    assert!(
        agent.try_lock().is_ok(),
        "cancelled runner must release its reservation"
    );
    let events: Vec<_> = std::iter::from_fn(|| rx.try_recv().ok()).collect();
    assert!(
        !events
            .iter()
            .any(|event| matches!(event, ServerEvent::Done { .. })),
        "cancelled assignment must not publish terminal success: {events:?}"
    );
    match prev_home {
        Some(value) => crate::env::set_var("JCODE_HOME", value),
        None => crate::env::remove_var("JCODE_HOME"),
    }
}

#[tokio::test(start_paused = true)]
async fn assigned_task_initial_request_cascade_does_not_publish_completion() {
    assigned_task_cascade_stays_stopped(false).await;
}

#[tokio::test(start_paused = true)]
async fn assigned_task_quota_wait_cascade_never_retries_or_resurrects_worker() {
    assigned_task_cascade_stays_stopped(true).await;
}

#[tokio::test(start_paused = true)]
async fn stale_assigned_runner_preserves_successor_durable_wait() {
    let (_env, _runtime) = RuntimeEnvGuard::new();
    let home = tempfile::TempDir::new().unwrap();
    let prev_home = std::env::var_os("JCODE_HOME");
    crate::env::set_var("JCODE_HOME", home.path());
    let (swarm_id, coord, worker) = ("stale-assignment-swarm", "stale-owner", "stale-worker");
    let calls = Arc::new(std::sync::atomic::AtomicUsize::new(0));
    let provider: Arc<dyn Provider> = Arc::new(LimitedOnceProvider {
        calls: calls.clone(),
    });
    let registry = Registry::new(provider.clone()).await;
    let agent = Arc::new(Mutex::new(Agent::new(provider, registry)));
    let mut worker_member = owned_member(worker, swarm_id, "ready", coord);
    worker_member.is_headless = true;
    let (tx, _rx) = mpsc::unbounded_channel();
    worker_member.event_tx = tx;
    let members = Arc::new(RwLock::new(HashMap::from([
        (coord.to_string(), member(coord, swarm_id, "ready")),
        (worker.to_string(), worker_member),
    ])));
    let swarms = Arc::new(RwLock::new(HashMap::from([(
        swarm_id.to_string(),
        HashSet::from([coord.to_string(), worker.to_string()]),
    )])));
    let mut plan = VersionedPlan::new();
    for (id, status, assignee) in [
        ("successor", "queued", worker),
        ("old-completed", "done", worker),
        ("old-reassigned", "running", "different-worker"),
    ] {
        let mut item = plan_item(id, status, "high", &[]);
        item.assigned_to = Some(assignee.to_string());
        plan.items.push(item);
    }
    let plans = Arc::new(RwLock::new(HashMap::from([(swarm_id.to_string(), plan)])));
    let coordinators = Arc::new(RwLock::new(HashMap::from([(
        swarm_id.to_string(),
        coord.to_string(),
    )])));
    let history = Arc::new(RwLock::new(VecDeque::new()));
    let counter = Arc::new(AtomicU64::new(1));
    let events = broadcast::channel(256).0;
    let run = |task: &str| {
        super::spawn_assigned_task_run(
            agent.clone(),
            worker.to_string(),
            swarm_id.to_string(),
            task.to_string(),
            format!("assignment for {task}"),
            members.clone(),
            swarms.clone(),
            plans.clone(),
            coordinators.clone(),
            history.clone(),
            counter.clone(),
            events.clone(),
        )
    };
    let successor = run("successor");
    let mut state = None;
    for _ in 0..20_000 {
        state = crate::server::usage_limit_resume::pending_state(worker).unwrap();
        if state.is_some() && members.read().await[worker].status == "rate_limited" {
            break;
        }
        tokio::task::yield_now().await;
    }
    let before = state.expect("successor must register a durable provider wait");
    for stale_task in ["old-completed", "old-reassigned"] {
        run(stale_task).await.unwrap();
        assert!(crate::server::usage_limit_resume::has_pending_resume(
            worker
        ));
        let after = crate::server::usage_limit_resume::pending_state(worker)
            .unwrap()
            .expect("stale assignment must preserve successor's disk record");
        assert_eq!(before.generation, after.generation);
        assert_eq!(before.resume_at, after.resume_at);
        assert_eq!(calls.load(std::sync::atomic::Ordering::SeqCst), 1);
        assert_eq!(members.read().await[worker].status, "rate_limited");
    }
    tokio::time::timeout(Duration::from_secs(100), successor)
        .await
        .unwrap()
        .unwrap();
    assert_eq!(calls.load(std::sync::atomic::Ordering::SeqCst), 2);
    assert_eq!(plans.read().await[swarm_id].items[0].status, "done");
    assert!(!crate::server::usage_limit_resume::has_pending_resume(
        worker
    ));
    match prev_home {
        Some(value) => crate::env::set_var("JCODE_HOME", value),
        None => crate::env::remove_var("JCODE_HOME"),
    }
}

#[tokio::test(start_paused = true)]
async fn reassignment_during_publication_contention_preserves_successor_wait() {
    let (_env, _runtime) = RuntimeEnvGuard::new();
    let home = tempfile::TempDir::new().unwrap();
    let prev_home = std::env::var_os("JCODE_HOME");
    crate::env::set_var("JCODE_HOME", home.path());
    let (swarm_id, worker, task) = ("claim-race-swarm", "claim-race-worker", "claim-race-task");
    let mut item = plan_item(task, "running", "high", &[]);
    item.assigned_to = Some(worker.to_string());
    let mut plan = VersionedPlan::new();
    plan.items.push(item);
    let plans = Arc::new(RwLock::new(HashMap::from([(swarm_id.to_string(), plan)])));
    // Seed both pieces of a successor wait: a live cancellation ticket and
    // the exact durable record consulted/removed by production reset code.
    let state = crate::server::usage_limit_resume::ResumeState {
        session_id: worker.to_string(),
        generation: "successor-generation".to_string(),
        resume_at: chrono::Utc::now() + chrono::Duration::minutes(10),
        quota_resumes_used: 1,
        overload_resumes_used: 0,
        kind: crate::server::usage_limit_resume::ResumeKind::UsageLimit,
        system_reminder: None,
        in_flight: false,
        terminal: false,
    };
    let wait_dir = crate::storage::jcode_dir().unwrap().join("provider-waits");
    std::fs::create_dir_all(&wait_dir).unwrap();
    let encoded: String = worker
        .as_bytes()
        .iter()
        .map(|b| format!("{b:02x}"))
        .collect();
    std::fs::write(
        wait_dir.join(format!("{encoded}.json")),
        serde_json::to_vec(&state).unwrap(),
    )
    .unwrap();
    let ticket = crate::server::usage_limit_resume::register_pending_resume(worker);
    let predecessor = crate::server::swarm::begin_turn_publication(worker).await;
    let held = predecessor.publish_start(futures::future::pending());
    tokio::pin!(held);
    assert!(
        futures::poll!(&mut held).is_pending(),
        "hold predecessor publication state"
    );
    let candidate =
        super::assigned::begin_assigned_turn_publication(worker, swarm_id, task, &plans);
    tokio::pin!(candidate);
    // A single explicit poll reaches the contended replacement and its retry
    // delay. This is an enforced interleave, not a scheduling/sleep guess.
    assert!(futures::poll!(&mut candidate).is_pending());
    {
        let mut locked = plans
            .try_write()
            .expect("candidate must release the plan lock before backing off");
        locked.get_mut(swarm_id).unwrap().items[0].assigned_to = Some("new-assignee".to_string());
    }
    // The old publication remains locked here: revalidation must reject the
    // now-stale candidate without waiting for, superseding, or resetting it.
    tokio::time::advance(Duration::from_millis(1)).await;
    assert!(candidate.await.is_none());
    assert!(
        !predecessor.cancelled.is_cancelled(),
        "stale work cannot supersede the successor publication"
    );
    assert!(crate::server::usage_limit_resume::has_pending_resume(
        worker
    ));
    assert!(
        ticket.wait(Duration::ZERO).await,
        "successor RAM ticket must remain uncancelled"
    );
    let preserved = crate::server::usage_limit_resume::pending_state(worker)
        .unwrap()
        .unwrap();
    assert_eq!(preserved.generation, state.generation);
    assert_eq!(preserved.resume_at, state.resume_at);
    assert_eq!(
        plans.read().await[swarm_id].items[0].assigned_to.as_deref(),
        Some("new-assignee")
    );
    crate::server::usage_limit_resume::cancel_pending_resume(worker);
    match prev_home {
        Some(value) => crate::env::set_var("JCODE_HOME", value),
        None => crate::env::remove_var("JCODE_HOME"),
    }
}

#[tokio::test(start_paused = true)]
async fn assigned_start_blocked_on_plan_never_revives_stopped_or_terminal_member() {
    let (_env, _runtime) = RuntimeEnvGuard::new();
    let home = tempfile::TempDir::new().unwrap();
    let prev_home = std::env::var_os("JCODE_HOME");
    crate::env::set_var("JCODE_HOME", home.path());
    for terminal in [
        "stopped",
        "completed",
        "done",
        "suspended",
        "paused",
        "failed",
        "crashed",
        "removed",
    ] {
        let swarm_id = "blocked-start-swarm";
        let worker = format!("blocked-start-{terminal}");
        let calls = Arc::new(std::sync::atomic::AtomicUsize::new(0));
        let provider: Arc<dyn Provider> = Arc::new(CascadeBlockedTaskProvider {
            calls: calls.clone(),
            limited: false,
        });
        let registry = Registry::new(provider.clone()).await;
        let agent = Arc::new(Mutex::new(Agent::new(provider, registry)));
        let mut worker_member = owned_member(&worker, swarm_id, "ready", "blocked-start-owner");
        worker_member.is_headless = true;
        let (tx, mut rx) = mpsc::unbounded_channel();
        worker_member.event_tx = tx;
        let members = Arc::new(RwLock::new(HashMap::from([(
            worker.clone(),
            worker_member,
        )])));
        let swarms = Arc::new(RwLock::new(HashMap::from([(
            swarm_id.to_string(),
            HashSet::from([worker.clone()]),
        )])));
        let mut item = plan_item("blocked-task", "queued", "high", &[]);
        item.assigned_to = Some(worker.clone());
        let mut plan = VersionedPlan::new();
        plan.items.push(item);
        let plans = Arc::new(RwLock::new(HashMap::from([(swarm_id.to_string(), plan)])));
        let blocked = plans.write().await;
        let predecessor = crate::server::swarm::begin_turn_publication(&worker).await;
        let runner = super::spawn_assigned_task_run(
            agent.clone(),
            worker.clone(),
            swarm_id.to_string(),
            "blocked-task".to_string(),
            "must not start after Stop".to_string(),
            members.clone(),
            swarms,
            plans.clone(),
            Arc::new(RwLock::new(HashMap::new())),
            Arc::new(RwLock::new(VecDeque::new())),
            Arc::new(AtomicU64::new(1)),
            broadcast::channel(256).0,
        );
        // The current-thread executor polls the runner until its first blocked
        // await. With the member lock free, owning Agent proves it has reached
        // the held plan lock after the original ready-status observation.
        let mut owns_agent = false;
        for _ in 0..20_000 {
            if agent.try_lock().is_err() {
                owns_agent = true;
                break;
            }
            tokio::task::yield_now().await;
        }
        assert!(owns_agent, "runner must reach the blocked assignment start");
        crate::server::swarm::cancel_session_turn_with(&worker, |_| async {
            let mut state = members.write().await;
            if terminal == "removed" {
                state.remove(&worker);
            } else {
                state.get_mut(&worker).unwrap().status = terminal.to_string();
            }
        })
        .await;
        assert!(
            predecessor.cancelled.is_cancelled(),
            "actual Stop must own the old publication"
        );
        drop(blocked);
        tokio::time::timeout(Duration::from_secs(3), runner)
            .await
            .expect("stopped assigned start must finish without reaching its blocked provider")
            .unwrap();
        assert_eq!(
            calls.load(std::sync::atomic::Ordering::SeqCst),
            0,
            "{terminal} member must never call provider"
        );
        assert_eq!(plans.read().await[swarm_id].items[0].status, "queued");
        assert!(plans.read().await[swarm_id].task_progress.is_empty());
        if terminal == "removed" {
            assert!(!members.read().await.contains_key(&worker));
        } else {
            assert_eq!(members.read().await[&worker].status, terminal);
        }
        let events: Vec<_> = std::iter::from_fn(|| rx.try_recv().ok()).collect();
        assert!(
            !events
                .iter()
                .any(|event| matches!(event, ServerEvent::Done { .. })),
            "no completion publication: {events:?}"
        );
    }
    match prev_home {
        Some(value) => crate::env::set_var("JCODE_HOME", value),
        None => crate::env::remove_var("JCODE_HOME"),
    }
}

#[derive(Clone)]
struct LimitedOnceProvider {
    calls: Arc<std::sync::atomic::AtomicUsize>,
}

#[async_trait]
impl Provider for LimitedOnceProvider {
    async fn complete(
        &self,
        _messages: &[Message],
        _tools: &[ToolDefinition],
        _system: &str,
        _resume_session_id: Option<&str>,
    ) -> Result<EventStream> {
        let call = self.calls.fetch_add(1, std::sync::atomic::Ordering::SeqCst);
        if call == 0 {
            let error = jcode_provider_core::usage_limit_resume::with_usage_limit_reset(
                anyhow::anyhow!(
                    "Anthropic API error (429 Too Many Requests): rate_limit_error Usage limit reached for this Claude account; resets in 1m (2026-09-30 13:30 UTC)."
                ),
                Some(Duration::from_secs(20)),
            );
            return Ok(Box::pin(stream::iter(vec![Err(error)])));
        }
        Ok(Box::pin(stream::iter(vec![
            Ok(StreamEvent::TextDelta(
                "task finished after the reset".to_string(),
            )),
            Ok(StreamEvent::MessageEnd { stop_reason: None }),
        ])))
    }

    fn name(&self) -> &str {
        "test"
    }

    fn fork(&self) -> Arc<dyn Provider> {
        Arc::new(self.clone())
    }
}

#[tokio::test(start_paused = true)]
async fn swarm_task_hitting_a_usage_limit_stays_in_progress_and_resumes() {
    let (_env, _runtime) = RuntimeEnvGuard::new();
    let home = tempfile::TempDir::new().expect("jcode home");
    let prev_home = std::env::var_os("JCODE_HOME");
    crate::env::set_var("JCODE_HOME", home.path());

    let swarm_id = "swarm-usage-limit";
    let coord = "coord-ul";
    let worker = "worker-ul";
    let calls = Arc::new(std::sync::atomic::AtomicUsize::new(0));
    let provider: Arc<dyn Provider> = Arc::new(LimitedOnceProvider {
        calls: Arc::clone(&calls),
    });
    let registry = Registry::new(provider.clone()).await;
    let agent = Arc::new(Mutex::new(Agent::new(provider, registry)));

    let swarm_members = Arc::new(RwLock::new(HashMap::from([
        (coord.to_string(), {
            let mut m = member(coord, swarm_id, "ready");
            m.role = "coordinator".to_string();
            m
        }),
        (
            worker.to_string(),
            owned_member(worker, swarm_id, "queued", coord),
        ),
    ])));
    let swarms_by_id = Arc::new(RwLock::new(HashMap::from([(
        swarm_id.to_string(),
        HashSet::from([coord.to_string(), worker.to_string()]),
    )])));
    let mut plan = VersionedPlan::new();
    plan.items.push(plan_item("task-1", "queued", "high", &[]));
    let swarm_plans = Arc::new(RwLock::new(HashMap::from([(swarm_id.to_string(), plan)])));
    let swarm_coordinators = Arc::new(RwLock::new(HashMap::from([(
        swarm_id.to_string(),
        coord.to_string(),
    )])));
    let event_history = Arc::new(RwLock::new(VecDeque::new()));
    let event_counter = Arc::new(AtomicU64::new(1));
    let swarm_event_tx = broadcast::channel(256).0;

    super::spawn_assigned_task_run(
        Arc::clone(&agent),
        worker.to_string(),
        swarm_id.to_string(),
        "task-1".to_string(),
        "summarize the logs".to_string(),
        Arc::clone(&swarm_members),
        Arc::clone(&swarms_by_id),
        Arc::clone(&swarm_plans),
        Arc::clone(&swarm_coordinators),
        Arc::clone(&event_history),
        Arc::clone(&event_counter),
        swarm_event_tx,
    );

    let item_status = |plans: &HashMap<String, VersionedPlan>| {
        plans[swarm_id]
            .items
            .iter()
            .find(|item| item.id == "task-1")
            .map(|item| item.status.clone())
            .unwrap()
    };

    // Wait until the first attempt hit the limit.
    let mut waited = false;
    for _ in 0..20_000 {
        let status = swarm_members.read().await[worker].status.clone();
        if status == "rate_limited" {
            waited = true;
            break;
        }
        assert_ne!(status, "failed", "a usage limit must not fail the worker");
        tokio::task::yield_now().await;
    }
    assert!(waited, "worker should wait for the usage-limit reset");
    assert_eq!(item_status(&*swarm_plans.read().await), "running");
    assert_eq!(calls.load(std::sync::atomic::Ordering::SeqCst), 1);

    // Past the reset (20 s + at most 30 s jitter) the task resumes and ends.
    tokio::time::sleep(Duration::from_secs(60)).await;
    let mut done = false;
    for _ in 0..20_000 {
        if item_status(&*swarm_plans.read().await) == "done" {
            done = true;
            break;
        }
        tokio::task::yield_now().await;
    }
    assert!(done, "task should complete after the resume");
    assert_eq!(calls.load(std::sync::atomic::Ordering::SeqCst), 2);
    let worker_member = swarm_members.read().await[worker].clone();
    assert_eq!(worker_member.status, "completed");
    assert!(
        worker_member
            .latest_completion_report
            .is_some_and(|report| report.contains("task finished after the reset"))
    );

    match prev_home {
        Some(prev) => crate::env::set_var("JCODE_HOME", prev),
        None => crate::env::remove_var("JCODE_HOME"),
    }
}

async fn superseded_usage_limit_worker_status(successor_status: Option<&str>) -> String {
    let (_env, _runtime) = RuntimeEnvGuard::new();
    let home = tempfile::TempDir::new().expect("jcode home");
    let prev_home = std::env::var_os("JCODE_HOME");
    crate::env::set_var("JCODE_HOME", home.path());

    let swarm_id = "swarm-usage-limit-sup";
    let coord = "coord-ul-sup";
    let worker = "worker-ul-sup";
    let calls = Arc::new(std::sync::atomic::AtomicUsize::new(0));
    let provider: Arc<dyn Provider> = Arc::new(LimitedOnceProvider {
        calls: Arc::clone(&calls),
    });
    let registry = Registry::new(provider.clone()).await;
    let agent = Arc::new(Mutex::new(Agent::new(provider, registry)));

    let swarm_members = Arc::new(RwLock::new(HashMap::from([
        (coord.to_string(), {
            let mut m = member(coord, swarm_id, "ready");
            m.role = "coordinator".to_string();
            m
        }),
        (
            worker.to_string(),
            owned_member(worker, swarm_id, "queued", coord),
        ),
    ])));
    let swarms_by_id = Arc::new(RwLock::new(HashMap::from([(
        swarm_id.to_string(),
        HashSet::from([coord.to_string(), worker.to_string()]),
    )])));
    let mut plan = VersionedPlan::new();
    plan.items.push(plan_item("task-1", "queued", "high", &[]));
    let swarm_plans = Arc::new(RwLock::new(HashMap::from([(swarm_id.to_string(), plan)])));
    let swarm_coordinators = Arc::new(RwLock::new(HashMap::from([(
        swarm_id.to_string(),
        coord.to_string(),
    )])));

    let task_run = super::spawn_assigned_task_run(
        Arc::clone(&agent),
        worker.to_string(),
        swarm_id.to_string(),
        "task-1".to_string(),
        "summarize the logs".to_string(),
        Arc::clone(&swarm_members),
        Arc::clone(&swarms_by_id),
        Arc::clone(&swarm_plans),
        Arc::clone(&swarm_coordinators),
        Arc::new(RwLock::new(VecDeque::new())),
        Arc::new(AtomicU64::new(1)),
        broadcast::channel(256).0,
    );

    let mut waited = false;
    for _ in 0..20_000 {
        if swarm_members.read().await[worker].status == "rate_limited"
            && crate::server::usage_limit_resume::has_pending_resume(worker)
        {
            waited = true;
            break;
        }
        tokio::task::yield_now().await;
    }
    assert!(waited, "worker should wait for the usage-limit reset");

    // A user message takes the worker over; the takeover turn does not
    // finish the task.
    if let Some(status) = successor_status {
        crate::server::update_member_status(
            worker,
            status,
            None,
            &swarm_members,
            &swarms_by_id,
            None,
            None,
            None,
        )
        .await;
    }
    assert!(crate::server::usage_limit_resume::cancel_pending_resume(
        worker
    ));
    task_run.await.expect("assigned task runner finished");

    let item = |plans: &HashMap<String, VersionedPlan>| {
        plans[swarm_id]
            .items
            .iter()
            .find(|item| item.id == "task-1")
            .cloned()
            .unwrap()
    };
    let mut released = false;
    for _ in 0..20_000 {
        if item(&*swarm_plans.read().await).status == "queued"
            && swarm_members.read().await[worker].status != "rate_limited"
        {
            released = true;
            break;
        }
        tokio::task::yield_now().await;
    }
    let item = item(&*swarm_plans.read().await);
    assert!(
        released,
        "a superseded resume must hand the task back, got status {:?}",
        item.status
    );
    assert_eq!(item.assigned_to, None);
    assert_eq!(calls.load(std::sync::atomic::Ordering::SeqCst), 1);
    let worker_status = swarm_members.read().await[worker].status.clone();
    match prev_home {
        Some(prev) => crate::env::set_var("JCODE_HOME", prev),
        None => crate::env::remove_var("JCODE_HOME"),
    }
    worker_status
}

#[tokio::test(start_paused = true)]
async fn superseded_usage_limit_resume_restores_idle_worker_to_ready() {
    assert_eq!(superseded_usage_limit_worker_status(None).await, "ready");
}

#[tokio::test(start_paused = true)]
async fn superseded_usage_limit_resume_preserves_running_successor() {
    assert_eq!(
        superseded_usage_limit_worker_status(Some("running")).await,
        "running"
    );
}

#[tokio::test(start_paused = true)]
async fn superseded_usage_limit_resume_preserves_stopped_successor() {
    assert_eq!(
        superseded_usage_limit_worker_status(Some("stopped")).await,
        "stopped"
    );
}

/// After a resumed turn completes, the task keeps the agent reserved until it
/// has finalized the plan and the member status. Otherwise a new turn can
/// start in the gap and then be reported `completed` by the old task.
#[tokio::test(start_paused = true)]
async fn resumed_swarm_task_keeps_the_agent_reserved_until_finalized() {
    let (_env, _runtime) = RuntimeEnvGuard::new();
    let home = tempfile::TempDir::new().expect("jcode home");
    let prev_home = std::env::var_os("JCODE_HOME");
    crate::env::set_var("JCODE_HOME", home.path());

    let swarm_id = "swarm-usage-limit-hold";
    let coord = "coord-ul-hold";
    let worker = "worker-ul-hold";
    let calls = Arc::new(std::sync::atomic::AtomicUsize::new(0));
    let provider: Arc<dyn Provider> = Arc::new(LimitedOnceProvider {
        calls: Arc::clone(&calls),
    });
    let registry = Registry::new(provider.clone()).await;
    let agent = Arc::new(Mutex::new(Agent::new(provider, registry)));

    let swarm_members = Arc::new(RwLock::new(HashMap::from([
        (coord.to_string(), {
            let mut m = member(coord, swarm_id, "ready");
            m.role = "coordinator".to_string();
            m
        }),
        (
            worker.to_string(),
            owned_member(worker, swarm_id, "queued", coord),
        ),
    ])));
    let swarms_by_id = Arc::new(RwLock::new(HashMap::from([(
        swarm_id.to_string(),
        HashSet::from([coord.to_string(), worker.to_string()]),
    )])));
    let mut plan = VersionedPlan::new();
    plan.items.push(plan_item("task-1", "queued", "high", &[]));
    let swarm_plans = Arc::new(RwLock::new(HashMap::from([(swarm_id.to_string(), plan)])));
    let swarm_coordinators = Arc::new(RwLock::new(HashMap::from([(
        swarm_id.to_string(),
        coord.to_string(),
    )])));

    super::spawn_assigned_task_run(
        Arc::clone(&agent),
        worker.to_string(),
        swarm_id.to_string(),
        "task-1".to_string(),
        "summarize the logs".to_string(),
        Arc::clone(&swarm_members),
        Arc::clone(&swarms_by_id),
        Arc::clone(&swarm_plans),
        Arc::clone(&swarm_coordinators),
        Arc::new(RwLock::new(VecDeque::new())),
        Arc::new(AtomicU64::new(1)),
        broadcast::channel(256).0,
    );

    let mut waited = false;
    for _ in 0..20_000 {
        if swarm_members.read().await[worker].status == "rate_limited"
            && crate::server::usage_limit_resume::has_pending_resume(worker)
        {
            waited = true;
            break;
        }
        tokio::task::yield_now().await;
    }
    assert!(waited, "worker should wait for the usage-limit reset");
    // Let the wait callback publish its progress before blocking the plan.
    for _ in 0..1_000 {
        tokio::task::yield_now().await;
    }

    // Block plan finalization so the window between the resumed turn ending
    // and the task publishing its result stays open.
    let plans_guard = swarm_plans.write().await;
    tokio::time::sleep(Duration::from_secs(60)).await;
    for _ in 0..20_000 {
        if calls.load(std::sync::atomic::Ordering::SeqCst) == 2 {
            break;
        }
        tokio::task::yield_now().await;
    }
    assert_eq!(calls.load(std::sync::atomic::Ordering::SeqCst), 2);
    for _ in 0..1_000 {
        tokio::task::yield_now().await;
    }
    assert!(
        agent.try_lock().is_err(),
        "the agent must stay reserved until the task is finalized"
    );
    drop(plans_guard);

    let mut finalized = false;
    for _ in 0..20_000 {
        if swarm_members.read().await[worker].status == "completed" {
            finalized = true;
            break;
        }
        tokio::task::yield_now().await;
    }
    assert!(finalized, "the task should finish after the plan unblocks");
    let mut released = false;
    for _ in 0..20_000 {
        if agent.try_lock().is_ok() {
            released = true;
            break;
        }
        tokio::task::yield_now().await;
    }
    assert!(released, "the agent is released once the task is finalized");

    match prev_home {
        Some(prev) => crate::env::set_var("JCODE_HOME", prev),
        None => crate::env::remove_var("JCODE_HOME"),
    }
}

// A swarm worker's turn whose provider is still overloaded after the
// runtime's own retries waits and resumes instead of failing the task.
#[derive(Clone)]
struct OverloadedUntilRecoveredProvider {
    calls: Arc<std::sync::atomic::AtomicUsize>,
    recovered: Arc<std::sync::atomic::AtomicBool>,
}

#[async_trait]
impl Provider for OverloadedUntilRecoveredProvider {
    async fn complete(
        &self,
        _messages: &[Message],
        _tools: &[ToolDefinition],
        _system: &str,
        _resume_session_id: Option<&str>,
    ) -> Result<EventStream> {
        self.calls.fetch_add(1, std::sync::atomic::Ordering::SeqCst);
        if !self.recovered.load(std::sync::atomic::Ordering::SeqCst) {
            let error = anyhow::anyhow!(
                "OpenAI-compatible chat request failed\n  endpoint: https://provider.example/v1/chat/completions\n  model: glm-5.3\n  status: 503 Service Unavailable\n  response: {{\"error\":{{\"message\":\"Service temporarily unavailable\",\"type\":\"api_error\"}}}}"
            );
            return Ok(Box::pin(stream::iter(vec![Err(error)])));
        }
        Ok(Box::pin(stream::iter(vec![
            Ok(StreamEvent::TextDelta(
                "task finished after the overload".to_string(),
            )),
            Ok(StreamEvent::MessageEnd { stop_reason: None }),
        ])))
    }

    fn name(&self) -> &str {
        "test"
    }

    fn fork(&self) -> Arc<dyn Provider> {
        Arc::new(self.clone())
    }
}

#[tokio::test(start_paused = true)]
async fn swarm_task_hitting_a_provider_overload_stays_in_progress_and_resumes() {
    let (_env, _runtime) = RuntimeEnvGuard::new();
    let home = tempfile::TempDir::new().expect("jcode home");
    let prev_home = std::env::var_os("JCODE_HOME");
    crate::env::set_var("JCODE_HOME", home.path());

    let swarm_id = "swarm-overload";
    let coord = "coord-ol";
    let worker = "worker-ol";
    let calls = Arc::new(std::sync::atomic::AtomicUsize::new(0));
    let recovered = Arc::new(std::sync::atomic::AtomicBool::new(false));
    let provider: Arc<dyn Provider> = Arc::new(OverloadedUntilRecoveredProvider {
        calls: Arc::clone(&calls),
        recovered: Arc::clone(&recovered),
    });
    let registry = Registry::new(provider.clone()).await;
    let agent = Arc::new(Mutex::new(Agent::new(provider, registry)));

    let swarm_members = Arc::new(RwLock::new(HashMap::from([
        (coord.to_string(), {
            let mut m = member(coord, swarm_id, "ready");
            m.role = "coordinator".to_string();
            m
        }),
        (
            worker.to_string(),
            owned_member(worker, swarm_id, "queued", coord),
        ),
    ])));
    let swarms_by_id = Arc::new(RwLock::new(HashMap::from([(
        swarm_id.to_string(),
        HashSet::from([coord.to_string(), worker.to_string()]),
    )])));
    let mut plan = VersionedPlan::new();
    plan.items.push(plan_item("task-1", "queued", "high", &[]));
    let swarm_plans = Arc::new(RwLock::new(HashMap::from([(swarm_id.to_string(), plan)])));
    let swarm_coordinators = Arc::new(RwLock::new(HashMap::from([(
        swarm_id.to_string(),
        coord.to_string(),
    )])));
    let event_history = Arc::new(RwLock::new(VecDeque::new()));
    let event_counter = Arc::new(AtomicU64::new(1));
    let swarm_event_tx = broadcast::channel(256).0;

    super::spawn_assigned_task_run(
        Arc::clone(&agent),
        worker.to_string(),
        swarm_id.to_string(),
        "task-1".to_string(),
        "build the app".to_string(),
        Arc::clone(&swarm_members),
        Arc::clone(&swarms_by_id),
        Arc::clone(&swarm_plans),
        Arc::clone(&swarm_coordinators),
        Arc::clone(&event_history),
        Arc::clone(&event_counter),
        swarm_event_tx,
    );

    let item_status = |plans: &HashMap<String, VersionedPlan>| {
        plans[swarm_id]
            .items
            .iter()
            .find(|item| item.id == "task-1")
            .map(|item| item.status.clone())
            .unwrap()
    };

    let mut waited = false;
    for _ in 0..20_000 {
        let member = swarm_members.read().await[worker].clone();
        if member.status == "rate_limited" {
            assert!(
                member
                    .detail
                    .as_deref()
                    .is_some_and(|detail| detail.contains("provider overloaded")),
                "status detail names the overload: {:?}",
                member.detail
            );
            waited = true;
            break;
        }
        assert_ne!(
            member.status, "failed",
            "an overload must not fail the worker"
        );
        tokio::task::yield_now().await;
    }
    assert!(waited, "worker should wait out the overload");
    assert_eq!(item_status(&*swarm_plans.read().await), "running");

    // The provider recovers; the first resume runs 15 s later and finishes.
    recovered.store(true, std::sync::atomic::Ordering::SeqCst);
    tokio::time::sleep(Duration::from_secs(20)).await;
    let mut done = false;
    for _ in 0..20_000 {
        if item_status(&*swarm_plans.read().await) == "done" {
            done = true;
            break;
        }
        tokio::task::yield_now().await;
    }
    assert!(done, "task should complete after the overload resume");
    let worker_member = swarm_members.read().await[worker].clone();
    assert_eq!(worker_member.status, "completed");
    assert!(
        worker_member
            .latest_completion_report
            .is_some_and(|report| report.contains("task finished after the overload"))
    );

    match prev_home {
        Some(prev) => crate::env::set_var("JCODE_HOME", prev),
        None => crate::env::remove_var("JCODE_HOME"),
    }
}
