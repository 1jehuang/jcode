use super::*;

#[test]
fn deliberately_paused_headless_status_survives_restart() {
    for value in ["suspended", "paused"] {
        let status = SwarmLifecycleStatus::Other(value.to_string());
        let (recovered, detail) =
            recover_member_status(status.clone(), Some("user paused".into()), true);
        assert_eq!(recovered, status);
        assert_eq!(detail.as_deref(), Some("user paused"));
    }
}

struct RuntimeEnv(Option<std::ffi::OsString>);

impl Drop for RuntimeEnv {
    fn drop(&mut self) {
        if let Some(value) = self.0.take() {
            crate::env::set_var("JCODE_RUNTIME_DIR", value);
        } else {
            crate::env::remove_var("JCODE_RUNTIME_DIR");
        }
    }
}

#[tokio::test]
#[expect(
    clippy::await_holding_lock,
    reason = "serializes the test storage environment"
)]
async fn stopped_backup_only_worker_cannot_resurrect_and_keeps_its_plan() {
    let _lock = storage::lock_test_env();
    let directory = tempfile::tempdir().unwrap();
    let _env = RuntimeEnv(std::env::var_os("JCODE_RUNTIME_DIR"));
    crate::env::set_var("JCODE_RUNTIME_DIR", directory.path());
    let session_id = "backup-only-worker";
    let swarm_id = "backup-only-swarm";
    let (event_tx, _event_rx) = tokio::sync::mpsc::unbounded_channel();
    let member = SwarmMember {
        session_id: session_id.into(),
        event_tx,
        event_txs: HashMap::new(),
        working_dir: None,
        swarm_id: Some(swarm_id.into()),
        swarm_enabled: true,
        status: "rate_limited".into(),
        detail: Some("waiting for provider".into()),
        friendly_name: None,
        report_back_to_session_id: Some("owner".into()),
        latest_completion_report: None,
        role: "agent".into(),
        joined_at: std::time::Instant::now(),
        last_status_change: std::time::Instant::now(),
        is_headless: true,
        output_tail: None,
        todo_progress: None,
        todo_items: Vec::new(),
        runtime: crate::protocol::SwarmMemberRuntime::default(),
        task_label: None,
    };
    let plan = VersionedPlan {
        items: vec![crate::plan::PlanItem {
            content: "preserve original assignment".into(),
            status: "running".into(),
            priority: "high".into(),
            id: "original-task".into(),
            subsystem: None,
            file_scope: Vec::new(),
            blocked_by: Vec::new(),
            assigned_to: Some(session_id.into()),
        }],
        version: 7,
        participants: HashSet::from([session_id.into()]),
        task_progress: HashMap::new(),
        mode: "deep".into(),
        node_meta: HashMap::new(),
    };
    persist_swarm_state(swarm_id, Some(&plan), Some("owner"), &[member]);
    let path = state_path(swarm_id);
    std::fs::rename(&path, path.with_extension("bak")).unwrap();
    let mut recovered = load_runtime_state();
    assert_eq!(recovered.members[session_id].status, "crashed");
    recovered.members.get_mut(session_id).unwrap().status = "stopped".into();
    let members = Arc::new(tokio::sync::RwLock::new(recovered.members));
    persist_member_lifecycle(session_id, &members).await;
    assert!(
        path.exists(),
        "terminal transition must replace the lost primary"
    );
    let restarted = load_runtime_state();
    assert_eq!(restarted.members[session_id].status, "stopped");
    let saved_plan = &restarted.plans[swarm_id];
    assert_eq!(saved_plan.version, 7);
    assert_eq!(saved_plan.mode, "deep");
    assert_eq!(restarted.coordinators[swarm_id], "owner");
}
