use super::*;
use std::time::Instant;

struct EnvGuard {
    runtime: Option<std::ffi::OsString>,
}

impl Drop for EnvGuard {
    fn drop(&mut self) {
        if let Some(value) = self.runtime.take() {
            crate::env::set_var("JCODE_RUNTIME_DIR", value);
        } else {
            crate::env::remove_var("JCODE_RUNTIME_DIR");
        }
    }
}

fn test_env(dir: &tempfile::TempDir) -> EnvGuard {
    let _guard = storage::lock_test_env();
    let previous = std::env::var_os("JCODE_RUNTIME_DIR");
    crate::env::set_var("JCODE_RUNTIME_DIR", dir.path());
    EnvGuard { runtime: previous }
}

#[test]
fn persisted_swarm_state_round_trips_and_marks_running_stale() {
    let dir = tempfile::TempDir::new().expect("tempdir");
    let _env = test_env(&dir);

    let mut plans = HashMap::new();
    plans.insert(
        "swarm-alpha".to_string(),
        VersionedPlan {
            items: vec![crate::plan::PlanItem {
                content: "do thing".to_string(),
                status: "running".to_string(),
                priority: "high".to_string(),
                id: "task-1".to_string(),
                subsystem: None,
                file_scope: Vec::new(),
                blocked_by: Vec::new(),
                assigned_to: Some("session-1".to_string()),
            }],
            version: 3,
            participants: ["session-1".to_string(), "session-2".to_string()]
                .into_iter()
                .collect(),
            task_progress: HashMap::from([(
                "task-1".to_string(),
                SwarmTaskProgress {
                    assigned_session_id: Some("session-1".to_string()),
                    assignment_summary: Some("do thing".to_string()),
                    assigned_at_unix_ms: Some(10),
                    started_at_unix_ms: Some(20),
                    last_heartbeat_unix_ms: Some(30),
                    last_detail: Some("tool start: read".to_string()),
                    last_checkpoint_unix_ms: Some(40),
                    checkpoint_summary: Some("tool done: read".to_string()),
                    completed_at_unix_ms: None,
                    stale_since_unix_ms: None,
                    heartbeat_count: Some(2),
                    checkpoint_count: Some(1),
                },
            )]),
        },
    );
    let coordinators = HashMap::from([("swarm-alpha".to_string(), "session-2".to_string())]);
    let (event_tx, _event_rx) = tokio::sync::mpsc::unbounded_channel();
    let members = vec![SwarmMember {
        session_id: "session-1".to_string(),
        event_tx,
        event_txs: HashMap::new(),
        working_dir: Some(PathBuf::from("/tmp/swarm-alpha")),
        swarm_id: Some("swarm-alpha".to_string()),
        swarm_enabled: true,
        status: "running".to_string(),
        detail: Some("writing tests".to_string()),
        friendly_name: Some("fox".to_string()),
        report_back_to_session_id: Some("session-2".to_string()),
        latest_completion_report: None,
        role: "agent".to_string(),
        joined_at: Instant::now(),
        last_status_change: Instant::now(),
        is_headless: true,
    }];

    persist_swarm_state(
        "swarm-alpha",
        plans.get("swarm-alpha"),
        coordinators.get("swarm-alpha").map(String::as_str),
        &members,
    );
    let loaded = load_runtime_state();

    let loaded_plan = loaded.plans.get("swarm-alpha").expect("loaded plan");
    assert_eq!(loaded_plan.version, 3);
    assert_eq!(loaded_plan.items.len(), 1);
    assert_eq!(loaded_plan.items[0].status, "running_stale");
    let progress = loaded_plan
        .task_progress
        .get("task-1")
        .expect("task progress");
    assert_eq!(progress.assigned_session_id.as_deref(), Some("session-1"));
    assert_eq!(
        progress.checkpoint_summary.as_deref(),
        Some("tool done: read")
    );
    assert!(progress.stale_since_unix_ms.is_some());
    assert_eq!(
        loaded.coordinators.get("swarm-alpha"),
        Some(&"session-2".to_string())
    );
    let recovered_member = loaded.members.get("session-1").expect("recovered member");
    assert_eq!(recovered_member.role, "agent");
    assert_eq!(
        recovered_member.report_back_to_session_id.as_deref(),
        Some("session-2")
    );
    assert_eq!(recovered_member.status, "crashed");
    assert_eq!(
        recovered_member.detail.as_deref(),
        Some("writing tests (recovered after reload while running)")
    );
    assert_eq!(
        loaded.swarms_by_id.get("swarm-alpha"),
        Some(&HashSet::from(["session-1".to_string()]))
    );
}

#[test]
fn remove_swarm_state_deletes_persisted_snapshot() {
    let dir = tempfile::TempDir::new().expect("tempdir");
    let _env = test_env(&dir);

    let plans = HashMap::from([(
        "swarm-beta".to_string(),
        VersionedPlan {
            items: Vec::new(),
            version: 1,
            participants: Default::default(),
            task_progress: HashMap::new(),
        },
    )]);
    persist_swarm_state("swarm-beta", plans.get("swarm-beta"), None, &[]);
    assert!(state_path("swarm-beta").exists());

    remove_swarm_state("swarm-beta");
    assert!(!state_path("swarm-beta").exists());
}

#[test]
fn persisted_swarm_state_without_plan_still_restores_coordinator_and_members() {
    let dir = tempfile::TempDir::new().expect("tempdir");
    let _env = test_env(&dir);

    let (event_tx, _event_rx) = tokio::sync::mpsc::unbounded_channel();
    let members = vec![SwarmMember {
        session_id: "coord-1".to_string(),
        event_tx,
        event_txs: HashMap::new(),
        working_dir: Some(PathBuf::from("/tmp/swarm-gamma")),
        swarm_id: Some("swarm-gamma".to_string()),
        swarm_enabled: true,
        status: "ready".to_string(),
        detail: None,
        friendly_name: Some("owl".to_string()),
        report_back_to_session_id: None,
        latest_completion_report: None,
        role: "coordinator".to_string(),
        joined_at: Instant::now(),
        last_status_change: Instant::now(),
        is_headless: false,
    }];

    persist_swarm_state("swarm-gamma", None, Some("coord-1"), &members);

    let loaded = load_runtime_state();
    assert!(!loaded.plans.contains_key("swarm-gamma"));
    assert_eq!(
        loaded.coordinators.get("swarm-gamma"),
        Some(&"coord-1".to_string())
    );
    assert_eq!(
        loaded
            .members
            .get("coord-1")
            .and_then(|member| member.friendly_name.as_deref()),
        Some("owl")
    );
    assert_eq!(
        loaded.swarms_by_id.get("swarm-gamma"),
        Some(&HashSet::from(["coord-1".to_string()]))
    );
}
