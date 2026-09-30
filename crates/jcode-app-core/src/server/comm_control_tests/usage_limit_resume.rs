// A swarm worker's task turn that hits a subscription usage limit stays in
// progress (member `rate_limited`, plan item still `running`) and resumes the
// same turn after the reset instead of failing the task.

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
        let call = self
            .calls
            .fetch_add(1, std::sync::atomic::Ordering::SeqCst);
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
            Ok(StreamEvent::TextDelta("task finished after the reset".to_string())),
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
        (worker.to_string(), owned_member(worker, swarm_id, "queued", coord)),
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

#[tokio::test(start_paused = true)]
async fn swarm_task_hitting_a_usage_limit_is_requeued_when_the_resume_is_superseded() {
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
        (worker.to_string(), owned_member(worker, swarm_id, "queued", coord)),
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

    // A user message takes the worker over; the takeover turn does not
    // finish the task.
    assert!(crate::server::usage_limit_resume::cancel_pending_resume(worker));

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
        if item(&*swarm_plans.read().await).status == "queued" {
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

    match prev_home {
        Some(prev) => crate::env::set_var("JCODE_HOME", prev),
        None => crate::env::remove_var("JCODE_HOME"),
    }
}
