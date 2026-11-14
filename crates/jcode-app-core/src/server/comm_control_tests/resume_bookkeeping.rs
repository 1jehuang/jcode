// Actual assigned-task runner regression: stall bookkeeping after the resume
// finishes, then attempt the reservation a concurrent successor would need.
#[derive(Clone)]
struct GatedBookkeepingResumeProvider {
    first: LimitedOnceProvider,
    resumed: Arc<tokio::sync::Notify>,
    release: Arc<tokio::sync::Semaphore>,
    fail_resume: bool,
}

#[async_trait]
impl Provider for GatedBookkeepingResumeProvider {
    async fn complete(
        &self,
        messages: &[Message],
        tools: &[ToolDefinition],
        system: &str,
        resume_session_id: Option<&str>,
    ) -> Result<EventStream> {
        if self.first.calls.load(std::sync::atomic::Ordering::SeqCst) == 0 {
            return self
                .first
                .complete(messages, tools, system, resume_session_id)
                .await;
        }
        self.resumed.notify_one();
        self.release
            .acquire()
            .await
            .expect("release resume")
            .forget();
        if self.fail_resume {
            return Ok(Box::pin(stream::iter(vec![Err(anyhow::anyhow!(
                "permanent resumed provider failure"
            ))])));
        }
        self.first
            .complete(messages, tools, system, resume_session_id)
            .await
    }

    fn name(&self) -> &str {
        "test"
    }
    fn fork(&self) -> Arc<dyn Provider> {
        Arc::new(self.clone())
    }
}

async fn assigned_resume_reserves_agent_during_bookkeeping(fail_resume: bool) -> bool {
    let (_env, _runtime) = RuntimeEnvGuard::new();
    let home = tempfile::TempDir::new().expect("jcode home");
    let prev_home = std::env::var_os("JCODE_HOME");
    crate::env::set_var("JCODE_HOME", home.path());
    let swarm_id = "swarm-resume-bookkeeping";
    let coord = "coord-resume-bookkeeping";
    let worker = "worker-resume-bookkeeping";
    let resumed = Arc::new(tokio::sync::Notify::new());
    let release = Arc::new(tokio::sync::Semaphore::new(0));
    let provider: Arc<dyn Provider> = Arc::new(GatedBookkeepingResumeProvider {
        first: LimitedOnceProvider {
            calls: Arc::new(std::sync::atomic::AtomicUsize::new(0)),
        },
        resumed: resumed.clone(),
        release: release.clone(),
        fail_resume,
    });
    let registry = Registry::new(provider.clone()).await;
    let agent = Arc::new(Mutex::new(Agent::new(provider, registry)));
    let agent_session = agent.lock().await.session_id().to_string();
    let swarm_members = Arc::new(RwLock::new(HashMap::from([
        (coord.to_string(), member(coord, swarm_id, "ready")),
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
    let runner = super::spawn_assigned_task_run(
        agent.clone(),
        worker.into(),
        swarm_id.into(),
        "task-1".into(),
        "summarize logs".into(),
        swarm_members.clone(),
        swarms_by_id,
        swarm_plans.clone(),
        Arc::new(RwLock::new(HashMap::from([(
            swarm_id.into(),
            coord.into(),
        )]))),
        Arc::new(RwLock::new(VecDeque::new())),
        Arc::new(AtomicU64::new(1)),
        broadcast::channel(256).0,
    );
    // Paused Tokio automatically advances through the reset until the second
    // request is gated. The real resumed agent turn now owns its reservation.
    resumed.notified().await;
    assert!(!crate::turn_cancel_registry::active_turn_signals(&agent_session).is_empty());
    let blocked_bookkeeping = swarm_plans.write().await;
    release.add_permits(1);
    let mut turn_finished = false;
    for _ in 0..20_000 {
        if crate::turn_cancel_registry::active_turn_signals(&agent_session).is_empty() {
            turn_finished = true;
            break;
        }
        tokio::task::yield_now().await;
    }
    assert!(
        turn_finished,
        "actual resumed turn must finish before probing bookkeeping reservation"
    );
    // The runner has returned from the actual agent turn, but cannot finish
    // heartbeat/plan bookkeeping until the plan write lock above is released.
    let successor_blocked = agent.clone().try_lock_owned().is_err();
    drop(blocked_bookkeeping);
    runner.await.expect("assigned task runner finished");
    assert!(
        agent.try_lock().is_ok(),
        "reservation released after bookkeeping"
    );
    assert_eq!(
        swarm_plans.read().await[swarm_id].items[0].status,
        if fail_resume { "failed" } else { "done" }
    );
    match prev_home {
        Some(prev) => crate::env::set_var("JCODE_HOME", prev),
        None => crate::env::remove_var("JCODE_HOME"),
    }
    successor_blocked
}

#[tokio::test(start_paused = true)]
async fn completed_assigned_resume_blocks_successor_until_bookkeeping_finishes() {
    assert!(assigned_resume_reserves_agent_during_bookkeeping(false).await);
}

#[tokio::test(start_paused = true)]
async fn failed_assigned_resume_blocks_successor_until_bookkeeping_finishes() {
    assert!(assigned_resume_reserves_agent_during_bookkeeping(true).await);
}
