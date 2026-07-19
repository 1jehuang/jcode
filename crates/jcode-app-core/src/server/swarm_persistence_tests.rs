use super::*;
use std::time::{Duration, Instant};

struct EnvGuard {
    _lock: std::sync::MutexGuard<'static, ()>,
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
    let lock = storage::lock_test_env();
    let previous = std::env::var_os("JCODE_RUNTIME_DIR");
    crate::env::set_var("JCODE_RUNTIME_DIR", dir.path());
    EnvGuard {
        _lock: lock,
        runtime: previous,
    }
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
                    no_artifact_requeues: None,
                    dead_assignee_reclaims: None,
                },
            )]),
            mode: "light".to_string(),
            node_meta: HashMap::new(),
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
        output_tail: None,
        todo_progress: None,
        todo_items: Vec::new(),
        runtime: crate::protocol::SwarmMemberRuntime::default(),
        task_label: None,
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
fn ready_headless_member_with_report_stops_without_losing_report() {
    // A headless worker that finished its task has no process after restart.
    // Preserve its report, but do not eagerly reconstruct the full Agent just
    // to keep an idle worker reusable indefinitely.
    let dir = tempfile::TempDir::new().expect("tempdir");
    let _env = test_env(&dir);

    let (event_tx, _event_rx) = tokio::sync::mpsc::unbounded_channel();
    let members = vec![SwarmMember {
        session_id: "session-ready".to_string(),
        event_tx,
        event_txs: HashMap::new(),
        working_dir: Some(PathBuf::from("/tmp/swarm-gamma")),
        swarm_id: Some("swarm-gamma".to_string()),
        swarm_enabled: true,
        status: "ready".to_string(),
        detail: None,
        friendly_name: Some("pig".to_string()),
        report_back_to_session_id: Some("session-coordinator".to_string()),
        latest_completion_report: Some("Done. Built the worker; all tests pass.".to_string()),
        role: "agent".to_string(),
        joined_at: Instant::now(),
        last_status_change: Instant::now(),
        is_headless: true,
        output_tail: None,
        todo_progress: None,
        todo_items: Vec::new(),
        runtime: crate::protocol::SwarmMemberRuntime::default(),
        task_label: None,
    }];

    persist_swarm_state("swarm-gamma", None, None, &members);
    let loaded = load_runtime_state();

    let recovered = loaded.members.get("session-ready").expect("member");
    assert_eq!(recovered.status, "stopped");
    assert_eq!(
        recovered.detail.as_deref(),
        Some("idle worker not restored after server restart")
    );
    assert_eq!(
        recovered.latest_completion_report.as_deref(),
        Some("Done. Built the worker; all tests pass.")
    );
}

#[test]
fn ready_detached_client_stops_on_reload_until_it_reattaches() {
    let dir = tempfile::TempDir::new().expect("tempdir");
    let _env = test_env(&dir);

    let (event_tx, _event_rx) = tokio::sync::mpsc::unbounded_channel();
    let members = vec![SwarmMember {
        session_id: "session-detached".to_string(),
        event_tx,
        event_txs: HashMap::new(),
        working_dir: Some(PathBuf::from("/tmp/swarm-client")),
        swarm_id: Some("swarm-client".to_string()),
        swarm_enabled: true,
        status: "ready".to_string(),
        detail: None,
        friendly_name: Some("finch".to_string()),
        report_back_to_session_id: None,
        latest_completion_report: None,
        role: "agent".to_string(),
        joined_at: Instant::now(),
        last_status_change: Instant::now(),
        is_headless: false,
        output_tail: None,
        todo_progress: None,
        todo_items: Vec::new(),
        runtime: crate::protocol::SwarmMemberRuntime::default(),
        task_label: None,
    }];

    persist_swarm_state("swarm-client", None, None, &members);
    let loaded = load_runtime_state();
    let recovered = loaded.members.get("session-detached").expect("member");
    assert_eq!(recovered.status, "stopped");
    assert_eq!(
        recovered.detail.as_deref(),
        Some("client not attached after server restart")
    );
}

#[test]
fn dormant_plan_expiry_preserves_active_work_and_prunes_old_unassigned_graphs() {
    let item = |status: &str, assigned_to: Option<&str>| crate::plan::PlanItem {
        content: "task".to_string(),
        status: status.to_string(),
        priority: "medium".to_string(),
        id: format!("{status}-{}", assigned_to.unwrap_or("none")),
        subsystem: None,
        file_scope: Vec::new(),
        blocked_by: Vec::new(),
        assigned_to: assigned_to.map(str::to_string),
    };
    let plan = |items| PersistedVersionedPlan {
        items,
        version: 1,
        participants: Vec::new(),
        task_progress: HashMap::new(),
        mode: "light".to_string(),
        node_meta: HashMap::new(),
    };
    let now = 10_000_000u64;
    let retention = Duration::from_secs(60);
    let old = now - retention.as_millis() as u64;

    assert!(persisted_plan_is_expired(
        &plan(Vec::new()),
        now,
        now,
        retention
    ));
    assert!(persisted_plan_is_expired(
        &plan(vec![item("queued", None), item("completed", None)]),
        old,
        now,
        retention
    ));
    assert!(!persisted_plan_is_expired(
        &plan(vec![item("running", Some("worker"))]),
        old,
        now,
        retention
    ));
    assert!(!persisted_plan_is_expired(
        &plan(vec![item("queued", None)]),
        now,
        now,
        retention
    ));
}

#[test]
fn terminal_member_retention_preserves_recent_reports_and_prunes_expired_records() {
    let (event_tx, _event_rx) = tokio::sync::mpsc::unbounded_channel();
    let member = SwarmMember {
        session_id: "session-terminal".to_string(),
        event_tx,
        event_txs: HashMap::new(),
        working_dir: Some(PathBuf::from("/tmp/swarm-terminal")),
        swarm_id: Some("swarm-terminal".to_string()),
        swarm_enabled: true,
        status: "completed".to_string(),
        detail: Some("done".to_string()),
        friendly_name: Some("otter".to_string()),
        report_back_to_session_id: Some("session-coordinator".to_string()),
        latest_completion_report: Some("All targeted tests passed.".to_string()),
        role: "agent".to_string(),
        joined_at: Instant::now(),
        last_status_change: Instant::now(),
        is_headless: true,
        output_tail: None,
        todo_progress: None,
        todo_items: Vec::new(),
        runtime: crate::protocol::SwarmMemberRuntime::default(),
        task_label: Some("retention test".to_string()),
    };
    let loaded_at = 10_000_000;
    let mut persisted = to_persisted_member(&member, loaded_at);
    persisted.terminal_since_unix_ms = Some(loaded_at - 30_000);

    let recent = from_persisted_member(
        persisted.clone(),
        loaded_at,
        loaded_at,
        Duration::from_secs(60),
    )
    .expect("recent terminal member remains inspectable");
    assert_eq!(
        recent.latest_completion_report.as_deref(),
        Some("All targeted tests passed.")
    );
    assert!(recent.last_status_change.elapsed() >= Duration::from_secs(30));

    assert!(
        from_persisted_member(persisted, loaded_at, loaded_at, Duration::from_secs(10),).is_none(),
        "expired terminal member should be pruned"
    );
}

#[test]
fn legacy_terminal_member_uses_snapshot_time_as_retention_fallback() {
    let (event_tx, _event_rx) = tokio::sync::mpsc::unbounded_channel();
    let member = SwarmMember {
        session_id: "session-legacy-terminal".to_string(),
        event_tx,
        event_txs: HashMap::new(),
        working_dir: None,
        swarm_id: Some("swarm-legacy".to_string()),
        swarm_enabled: true,
        status: "failed".to_string(),
        detail: Some("old failure".to_string()),
        friendly_name: Some("badger".to_string()),
        report_back_to_session_id: None,
        latest_completion_report: Some("legacy report".to_string()),
        role: "agent".to_string(),
        joined_at: Instant::now(),
        last_status_change: Instant::now(),
        is_headless: true,
        output_tail: None,
        todo_progress: None,
        todo_items: Vec::new(),
        runtime: crate::protocol::SwarmMemberRuntime::default(),
        task_label: None,
    };
    let loaded_at = 20_000_000;
    let mut persisted = to_persisted_member(&member, loaded_at);
    persisted.terminal_since_unix_ms = None;

    assert!(
        from_persisted_member(
            persisted,
            loaded_at - 20_000,
            loaded_at,
            Duration::from_secs(10),
        )
        .is_none(),
        "legacy records should age from their containing snapshot"
    );
}

#[test]
fn recovery_induced_terminal_status_starts_retention_at_load_time() {
    let (event_tx, _event_rx) = tokio::sync::mpsc::unbounded_channel();
    let member = SwarmMember {
        session_id: "session-ready-recovery".to_string(),
        event_tx,
        event_txs: HashMap::new(),
        working_dir: None,
        swarm_id: Some("swarm-recovery".to_string()),
        swarm_enabled: true,
        status: "ready".to_string(),
        detail: None,
        friendly_name: Some("hare".to_string()),
        report_back_to_session_id: None,
        latest_completion_report: Some("finished just before restart".to_string()),
        role: "agent".to_string(),
        joined_at: Instant::now(),
        last_status_change: Instant::now(),
        is_headless: true,
        output_tail: None,
        todo_progress: None,
        todo_items: Vec::new(),
        runtime: crate::protocol::SwarmMemberRuntime::default(),
        task_label: None,
    };
    let loaded_at = 300_000_000;
    let mut persisted = to_persisted_member(&member, loaded_at);
    persisted.terminal_since_unix_ms = None;

    let recovered = from_persisted_member(
        persisted,
        loaded_at - Duration::from_secs(48 * 60 * 60).as_millis() as u64,
        loaded_at,
        Duration::from_secs(24 * 60 * 60),
    )
    .expect("recovery-induced terminal status should receive a fresh retention window");
    assert_eq!(recovered.status, "stopped");
    assert!(recovered.last_status_change.elapsed() < Duration::from_secs(1));
}

#[test]
fn startup_gc_removes_expired_terminal_members_from_durable_snapshot() {
    let dir = tempfile::TempDir::new().expect("tempdir");
    let _env = test_env(&dir);
    let (event_tx, _event_rx) = tokio::sync::mpsc::unbounded_channel();
    let members = vec![SwarmMember {
        session_id: "session-expired".to_string(),
        event_tx,
        event_txs: HashMap::new(),
        working_dir: None,
        swarm_id: Some("swarm-expired".to_string()),
        swarm_enabled: true,
        status: "completed".to_string(),
        detail: None,
        friendly_name: Some("fox".to_string()),
        report_back_to_session_id: None,
        latest_completion_report: Some("report retained until expiry".to_string()),
        role: "agent".to_string(),
        joined_at: Instant::now(),
        last_status_change: Instant::now(),
        is_headless: true,
        output_tail: None,
        todo_progress: None,
        todo_items: Vec::new(),
        runtime: crate::protocol::SwarmMemberRuntime::default(),
        task_label: None,
    }];
    persist_swarm_state("swarm-expired", None, None, &members);

    let path = state_path("swarm-expired");
    let mut persisted = storage::read_json::<PersistedSwarmState>(&path).expect("snapshot");
    persisted.members[0].terminal_since_unix_ms =
        Some(now_unix_ms().saturating_sub(Duration::from_secs(48 * 60 * 60).as_millis() as u64));
    storage::write_json_fast(&path, &persisted).expect("age terminal member");

    let loaded = load_runtime_state();
    assert!(!loaded.members.contains_key("session-expired"));
    assert!(
        !path.exists(),
        "empty snapshot should be deleted after startup collection"
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
            mode: "light".to_string(),
            node_meta: HashMap::new(),
        },
    )]);
    persist_swarm_state("swarm-beta", plans.get("swarm-beta"), None, &[]);
    assert!(state_path("swarm-beta").exists());

    remove_swarm_state("swarm-beta");
    assert!(!state_path("swarm-beta").exists());
}

#[test]
fn deep_plan_mode_and_node_meta_round_trip() {
    let dir = tempfile::TempDir::new().expect("tempdir");
    let _env = test_env(&dir);

    let mut node_meta = HashMap::new();
    node_meta.insert(
        "root".to_string(),
        crate::plan::NodeMeta {
            kind: Some("explore".to_string()),
            parent: None,
            expanded: true,
            is_gate: false,
            planner: Some("session-1".to_string()),
            artifact_json: Some(r#"{"findings":"found it","confidence":"high"}"#.to_string()),
            origin: Some("seed".to_string()),
        },
    );
    node_meta.insert(
        "root.gate".to_string(),
        crate::plan::NodeMeta {
            kind: Some("critique".to_string()),
            parent: Some("root".to_string()),
            expanded: false,
            is_gate: true,
            planner: None,
            artifact_json: None,
            origin: Some("gate".to_string()),
        },
    );

    let plan = VersionedPlan {
        items: vec![
            crate::plan::PlanItem {
                content: "explore X".to_string(),
                status: "completed".to_string(),
                priority: "high".to_string(),
                id: "root".to_string(),
                subsystem: None,
                file_scope: Vec::new(),
                blocked_by: Vec::new(),
                assigned_to: Some("session-1".to_string()),
            },
            crate::plan::PlanItem {
                content: "gate".to_string(),
                status: "queued".to_string(),
                priority: "medium".to_string(),
                id: "root.gate".to_string(),
                subsystem: None,
                file_scope: Vec::new(),
                blocked_by: vec!["root".to_string()],
                assigned_to: None,
            },
        ],
        version: 7,
        participants: ["session-1".to_string()].into_iter().collect(),
        task_progress: HashMap::new(),
        mode: "deep".to_string(),
        node_meta,
    };

    persist_swarm_state("swarm-deep", Some(&plan), None, &[]);
    let loaded = load_runtime_state();

    let loaded_plan = loaded.plans.get("swarm-deep").expect("loaded plan");
    assert_eq!(loaded_plan.mode, "deep");
    assert_eq!(loaded_plan.version, 7);

    // Edges survive on the item itself.
    let gate_item = loaded_plan
        .items
        .iter()
        .find(|item| item.id == "root.gate")
        .expect("gate item");
    assert_eq!(gate_item.blocked_by, vec!["root".to_string()]);

    // Node kinds, gate flags, expansion, planner, and artifacts survive in node_meta.
    let root_meta = loaded_plan.node_meta.get("root").expect("root meta");
    assert_eq!(root_meta.kind.as_deref(), Some("explore"));
    assert!(root_meta.expanded);
    assert!(!root_meta.is_gate);
    assert_eq!(root_meta.planner.as_deref(), Some("session-1"));
    assert!(
        root_meta
            .artifact_json
            .as_deref()
            .is_some_and(|json| json.contains("found it"))
    );
    let gate_meta = loaded_plan.node_meta.get("root.gate").expect("gate meta");
    assert_eq!(gate_meta.kind.as_deref(), Some("critique"));
    assert!(gate_meta.is_gate);
    assert_eq!(gate_meta.parent.as_deref(), Some("root"));
}

/// The behavioral counterpart of `deep_plan_mode_and_node_meta_round_trip`:
/// after a persist -> load cycle (server restart), the reloaded plan must still
/// drive the deep-mode machinery that reads `node_meta`:
///
/// 1. `low_confidence_completed_ids` still reports completed nodes whose stored
///    artifact self-reported low confidence (gate confidence-debt tracking).
/// 2. `hydrate_assignment` still injects completed upstream artifacts
///    (forward dataflow) into assignment content.
/// 3. Lifting the reloaded plan into the DAG engine still enforces the gate
///    debt rule: a gate cannot rubber-stamp past an unaddressed low-confidence
///    sibling, but passes once it addresses that sibling by id.
#[test]
fn gate_debt_and_artifact_hydration_survive_reload() {
    use crate::plan::dag::{DagError, HandoffArtifact, complete_node, dispatch};

    let dir = tempfile::TempDir::new().expect("tempdir");
    let _env = test_env(&dir);

    let solid_artifact = serde_json::to_string(&HandoffArtifact {
        findings: "solid scope fully mapped".to_string(),
        evidence: vec!["crates/foo/api.rs:12".to_string()],
        confidence: Some("high".to_string()),
        what_i_did_not_check: vec!["nothing, fully covered".to_string()],
        ..HandoffArtifact::default()
    })
    .unwrap();
    let shaky_artifact = serde_json::to_string(&HandoffArtifact {
        findings: "unsure about the edge cases here".to_string(),
        confidence: Some("low".to_string()),
        what_i_did_not_check: vec!["error paths".to_string()],
        ..HandoffArtifact::default()
    })
    .unwrap();

    let item = |id: &str, status: &str, blocked_by: Vec<String>| crate::plan::PlanItem {
        content: format!("work on {id}"),
        status: status.to_string(),
        priority: "medium".to_string(),
        id: id.to_string(),
        subsystem: None,
        file_scope: Vec::new(),
        blocked_by,
        assigned_to: None,
    };
    let meta = |kind: &str, parent: Option<&str>, is_gate: bool, artifact: Option<&str>| {
        crate::plan::NodeMeta {
            kind: Some(kind.to_string()),
            parent: parent.map(str::to_string),
            expanded: false,
            is_gate,
            planner: None,
            artifact_json: artifact.map(str::to_string),
            origin: None,
        }
    };

    let mut plan = VersionedPlan::new();
    plan.mode = "deep".to_string();
    plan.version = 4;
    plan.items = vec![
        {
            let mut root = item("root", "running", Vec::new());
            root.assigned_to = Some("planner-1".to_string());
            root
        },
        item("root.solid", "completed", Vec::new()),
        item("root.shaky", "completed", Vec::new()),
        item(
            "root.gate",
            "queued",
            vec!["root.solid".to_string(), "root.shaky".to_string()],
        ),
    ];
    plan.node_meta = HashMap::from([
        ("root".to_string(), {
            let mut m = meta("explore", None, false, None);
            m.expanded = true;
            m.planner = Some("planner-1".to_string());
            m
        }),
        (
            "root.solid".to_string(),
            meta("explore", Some("root"), false, Some(&solid_artifact)),
        ),
        (
            "root.shaky".to_string(),
            meta("explore", Some("root"), false, Some(&shaky_artifact)),
        ),
        (
            "root.gate".to_string(),
            meta("critique", Some("root"), true, None),
        ),
    ]);

    persist_swarm_state("swarm-debt", Some(&plan), None, &[]);
    let loaded = load_runtime_state();
    let loaded_plan = loaded.plans.get("swarm-debt").expect("loaded plan");

    // 1. Confidence-debt tracking: the reloaded plan still flags the shaky node.
    assert_eq!(
        crate::plan::bridge::low_confidence_completed_ids(loaded_plan),
        vec!["root.shaky".to_string()]
    );

    // 2. Upstream artifact hydration: the gate's assignment content still gets
    // both completed dependency artifacts, including what_i_did_not_check.
    let hydrated = crate::plan::bridge::hydrate_assignment(loaded_plan, "root.gate", "gate prompt");
    assert!(hydrated.contains("gate prompt"));
    assert!(hydrated.contains("Inputs from completed dependencies"));
    assert!(hydrated.contains("solid scope fully mapped"));
    assert!(hydrated.contains("crates/foo/api.rs:12"));
    assert!(hydrated.contains("unsure about the edge cases here"));
    assert!(hydrated.contains("error paths"));

    // 3. The DAG engine, lifted from the reloaded plan, still enforces the gate
    // debt rule end to end.
    let mut graph = crate::plan::bridge::to_task_graph(loaded_plan);
    assert!(dispatch(&mut graph, "root.gate", "gate-worker"));
    let err = complete_node(
        &mut graph,
        "root.gate",
        "gate-worker",
        HandoffArtifact::brief("all good, no gaps"),
    )
    .unwrap_err();
    match &err {
        DagError::UnaddressedLowConfidence { gate, nodes } => {
            assert_eq!(gate, "root.gate");
            assert_eq!(nodes, &vec!["root.shaky".to_string()]);
        }
        other => panic!("expected UnaddressedLowConfidence after reload, got {other:?}"),
    }
    complete_node(
        &mut graph,
        "root.gate",
        "gate-worker",
        HandoffArtifact::brief(
            "root.shaky's low confidence is acceptable: its scope was re-derived and \
             cross-checked; root.solid audited clean",
        ),
    )
    .expect("gate passes once every audited node is addressed by id");
}

#[test]
fn load_migrates_legacy_runtime_dir_state() {
    let dir = tempfile::TempDir::new().expect("tempdir");
    let _env = test_env(&dir);

    let legacy = serde_json::json!({
        "swarm_id": "swarm-migrate",
        "coordinator_session_id": "coord-legacy",
        "updated_at_unix_ms": 1u64
    });
    std::fs::create_dir_all(legacy_state_dir()).expect("legacy state dir");
    std::fs::write(
        legacy_state_dir().join("swarm-migrate.json"),
        serde_json::to_vec(&legacy).unwrap(),
    )
    .expect("write legacy snapshot");

    let loaded = load_runtime_state();
    assert_eq!(
        loaded.coordinators.get("swarm-migrate"),
        Some(&"coord-legacy".to_string())
    );
    // Migrated copy lives in the durable dir now.
    assert!(state_path("swarm-migrate").exists());
}

#[test]
fn migration_does_not_clobber_existing_durable_state() {
    let dir = tempfile::TempDir::new().expect("tempdir");
    let _env = test_env(&dir);

    // Durable dir already has state for this swarm.
    persist_swarm_state("swarm-both", None, Some("coord-new"), &[]);

    // Legacy dir has a stale snapshot for the same swarm.
    let legacy = serde_json::json!({
        "swarm_id": "swarm-both",
        "coordinator_session_id": "coord-old",
        "updated_at_unix_ms": 1u64
    });
    std::fs::create_dir_all(legacy_state_dir()).expect("legacy state dir");
    std::fs::write(
        legacy_state_dir().join("swarm-both.json"),
        serde_json::to_vec(&legacy).unwrap(),
    )
    .expect("write legacy snapshot");

    let loaded = load_runtime_state();
    assert_eq!(
        loaded.coordinators.get("swarm-both"),
        Some(&"coord-new".to_string())
    );
}

#[test]
fn state_dir_is_durable_not_runtime() {
    let dir = tempfile::TempDir::new().expect("tempdir");
    let _env = test_env(&dir);

    // With JCODE_RUNTIME_DIR pinned, the state dir stays sandboxed but must
    // not be the legacy runtime-dir location.
    assert_ne!(state_dir(), legacy_state_dir());
    assert!(state_dir().starts_with(dir.path()));
}

#[test]
fn legacy_snapshot_without_mode_defaults_to_light() {
    let dir = tempfile::TempDir::new().expect("tempdir");
    let _env = test_env(&dir);

    // Simulate a pre-deep-mode snapshot on disk: no `mode`, no `node_meta`.
    let legacy = serde_json::json!({
        "swarm_id": "swarm-legacy",
        "plan": {
            "items": [{
                "content": "old task",
                "status": "queued",
                "priority": "medium",
                "id": "t1"
            }],
            "version": 2,
            "participants": ["session-1"]
        },
        "updated_at_unix_ms": 1u64
    });
    std::fs::create_dir_all(state_dir()).expect("state dir");
    std::fs::write(
        state_path("swarm-legacy"),
        serde_json::to_vec(&legacy).unwrap(),
    )
    .expect("write legacy snapshot");

    let loaded = load_runtime_state();
    let plan = loaded.plans.get("swarm-legacy").expect("legacy plan");
    assert_eq!(plan.mode, "light");
    assert!(plan.node_meta.is_empty());
    assert_eq!(plan.version, 2);
}

/// A persist that captured an older plan must not overwrite a newer durable
/// plan when the calls complete out of order.
///
/// This test parks persist A inside `load_runtime` at `members.read()`
/// (after A has already cloned the v5 plan) behind a held `members.write()`
/// gate, then performs mutator B's work while A is parked: bump the
/// in-memory plan to v6 and run B's persist half (`persist_swarm_state` with
/// the v6 runtime, exactly what B's unblocked `persist_swarm_state_for` does
/// on another worker thread, where its uncontended lock reads resolve
/// without suspending). v6 is then durably on disk. Releasing A regresses
/// the durable snapshot back to v5.
///
/// Post-restart impact: `Server::new` seeds `SwarmState` from
/// `load_runtime_state()` and `recover_headless_sessions_on_startup`
/// (server.rs:584-918) drives recovery from that state, so a regressed
/// snapshot silently restores the older plan: work completed between v5 and
/// v6 flips back to queued/running_stale and newer node_meta artifacts are
/// lost.
///
#[tokio::test]
#[allow(clippy::await_holding_lock)]
async fn stale_persist_cannot_regress_newer_plan_version() {
    let dir = tempfile::TempDir::new().expect("tempdir");
    let _env = test_env(&dir);

    let mut plan = VersionedPlan::new();
    plan.version = 5;
    plan.items = vec![crate::plan::PlanItem {
        content: "task one".to_string(),
        status: "queued".to_string(),
        priority: "medium".to_string(),
        id: "t1".to_string(),
        subsystem: None,
        file_scope: Vec::new(),
        blocked_by: Vec::new(),
        assigned_to: None,
    }];
    let swarm_state = crate::server::SwarmState::new(
        HashMap::new(),
        HashMap::new(),
        HashMap::from([("swarm-race".to_string(), plan)]),
        HashMap::new(),
    );

    // Gate: hold members.write() so persist A parks inside load_runtime at
    // the final members.read(), AFTER it has already cloned the v5 plan
    // under plans.read().
    let gate = swarm_state.members.write().await;

    let a = tokio::spawn({
        let swarm_state = swarm_state.clone();
        async move {
            crate::server::persist_swarm_state_for("swarm-race", &swarm_state).await;
        }
    });
    // Current-thread test runtime: yielding runs A until it parks on the
    // contended members.read().await, past its v5 plan clone.
    for _ in 0..16 {
        tokio::task::yield_now().await;
    }

    // Mutator B: bump the in-memory plan to v6 ...
    let v6_plan = {
        let mut plans = swarm_state.plans.write().await;
        let plan = plans.get_mut("swarm-race").expect("plan");
        plan.version = 6;
        plan.clone()
    };
    // ... and B's persist half runs to completion while A is parked. In
    // production this is B's own persist_swarm_state_for on another worker
    // thread: nothing gates B on A (there is no per-swarm persist lock), so
    // B's load_runtime observes v6 and its synchronous persist_swarm_state
    // lands v6 on disk before A's task is polled again.
    persist_swarm_state("swarm-race", Some(&v6_plan), None, &[]);
    assert_eq!(
        load_runtime_state()
            .plans
            .get("swarm-race")
            .expect("v6 snapshot")
            .version,
        6,
        "v6 must be durably on disk before A resumes"
    );

    // Release A: it resumes with its stale v5 runtime snapshot. The durable
    // version guard must reject that write.
    drop(gate);
    a.await.expect("persist task");

    let primary = storage::read_json::<PersistedSwarmState>(&state_path("swarm-race"))
        .expect("primary snapshot");
    assert_eq!(
        primary.plan.expect("plan").version,
        6,
        "a stale persist must not regress the durable plan version"
    );
}

#[tokio::test]
async fn persistence_operations_serialize_per_swarm_but_not_globally() {
    let alpha = swarm_operation_lock("swarm-lock-alpha");
    let same_alpha = swarm_operation_lock("swarm-lock-alpha");
    let beta = swarm_operation_lock("swarm-lock-beta");
    assert!(
        Arc::ptr_eq(&alpha, &same_alpha),
        "the same swarm must share one operation lock"
    );
    assert!(
        !Arc::ptr_eq(&alpha, &beta),
        "unrelated swarms must not share a global serialization lock"
    );

    let alpha_guard = alpha.lock().await;
    assert!(
        tokio::time::timeout(Duration::from_millis(20), same_alpha.lock())
            .await
            .is_err(),
        "a second operation for the same swarm must wait"
    );
    let _beta_guard = tokio::time::timeout(Duration::from_millis(100), beta.lock())
        .await
        .expect("an unrelated swarm operation was unnecessarily blocked");
    drop(alpha_guard);
}

/// Companion finding discovered while writing the regression test above:
/// `load_runtime_state` filters entries only with `path.is_file()`, with no
/// `.json` extension check (unlike `migrate_legacy_state`, which does check).
/// Since `storage::write_json_fast` leaves a `<swarm>.bak` hard link of the
/// PREVIOUS snapshot next to the primary, startup restore parses both files
/// and inserts them into the same maps keyed by `state.swarm_id`, so
/// whichever the directory iterator yields last wins. After a regressed
/// primary (v5) with a newer backup (v6), restart restore is therefore
/// nondeterministic between the two. This test pins the underlying behavior
/// deterministically: a `.bak` file with no primary at all is still loaded
/// as a live snapshot.
#[test]
fn load_runtime_state_reads_bak_files_as_snapshots() {
    let dir = tempfile::TempDir::new().expect("tempdir");
    let _env = test_env(&dir);

    let snapshot = serde_json::json!({
        "swarm_id": "swarm-bak-only",
        "coordinator_session_id": "coord-from-bak",
        "updated_at_unix_ms": 1u64
    });
    std::fs::create_dir_all(state_dir()).expect("state dir");
    std::fs::write(
        state_dir().join("swarm-bak-only.bak"),
        serde_json::to_vec(&snapshot).unwrap(),
    )
    .expect("write bak snapshot");

    let loaded = load_runtime_state();
    assert_eq!(
        loaded.coordinators.get("swarm-bak-only"),
        Some(&"coord-from-bak".to_string()),
        "load_runtime_state currently ingests .bak files as snapshots; if \
         this fails the loader gained a .json extension filter (update the \
         wiring audit and the primary-file assertions in \
         stale_persist_cannot_regress_newer_plan_version)"
    );
}

/// A `.bak` sibling must NOT be loaded when the primary `.json` exists:
/// the write path rotates the previous snapshot to `.bak`, so after an
/// intentional state drop (e.g. `swarm:clear_plan`) the `.bak` still holds
/// the dropped plan. Union-loading both would resurrect the cleared plan on
/// every server restart.
#[test]
fn load_runtime_state_ignores_bak_when_primary_json_exists() {
    let dir = tempfile::TempDir::new().expect("tempdir");
    let _env = test_env(&dir);

    std::fs::create_dir_all(state_dir()).expect("state dir");
    let stale_with_plan = serde_json::json!({
        "swarm_id": "swarm-cleared",
        "plan": {
            "items": [{
                "id": "stale-task",
                "content": "stale",
                "status": "queued",
                "assigned_to": null
            }],
            "version": 42u64,
            "participants": [],
            "task_progress": {},
            "mode": "light",
            "node_meta": {}
        },
        "coordinator_session_id": "coord-stale",
        "updated_at_unix_ms": 1u64
    });
    let current_without_plan = serde_json::json!({
        "swarm_id": "swarm-cleared",
        "coordinator_session_id": "coord-current",
        "updated_at_unix_ms": 2u64
    });
    std::fs::write(
        state_dir().join("swarm-cleared.bak"),
        serde_json::to_vec(&stale_with_plan).unwrap(),
    )
    .expect("write bak snapshot");
    std::fs::write(
        state_dir().join("swarm-cleared.json"),
        serde_json::to_vec(&current_without_plan).unwrap(),
    )
    .expect("write primary snapshot");

    let loaded = load_runtime_state();
    assert!(
        !loaded.plans.contains_key("swarm-cleared"),
        "plan cleared from the primary snapshot must not be resurrected from .bak"
    );
    assert_eq!(
        loaded.coordinators.get("swarm-cleared"),
        Some(&"coord-current".to_string()),
        "primary snapshot must win over its .bak sibling"
    );
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
        output_tail: None,
        todo_progress: None,
        todo_items: Vec::new(),
        runtime: crate::protocol::SwarmMemberRuntime::default(),
        task_label: None,
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

#[test]
fn remove_swarm_state_removes_backup_and_cannot_resurrect() {
    let dir = tempfile::TempDir::new().expect("tempdir");
    let _env = test_env(&dir);

    // First persist creates the primary; the second overwrite makes
    // write_json_fast hard-link the previous (coord-v1) snapshot to `.bak`.
    persist_swarm_state("swarm-zombie", None, Some("coord-v1"), &[]);
    persist_swarm_state("swarm-zombie", None, Some("coord-v2"), &[]);
    let bak_path = state_path("swarm-zombie").with_extension("bak");
    assert!(bak_path.exists(), "write_json_fast leaves a .bak hard link");

    remove_swarm_state("swarm-zombie");
    assert!(!state_path("swarm-zombie").exists());
    assert!(
        !bak_path.exists(),
        "logical deletion must remove the recovery backup too"
    );

    let loaded = load_runtime_state();
    assert!(
        !loaded.coordinators.contains_key("swarm-zombie"),
        "a deleted swarm must not be restored on the next load"
    );
}

#[test]
fn empty_persist_dissolution_removes_backup_and_cannot_resurrect() {
    let dir = tempfile::TempDir::new().expect("tempdir");
    let _env = test_env(&dir);

    persist_swarm_state("swarm-dissolve", None, Some("coord-v1"), &[]);
    persist_swarm_state("swarm-dissolve", None, Some("coord-v2"), &[]);
    let bak_path = state_path("swarm-dissolve").with_extension("bak");
    assert!(bak_path.exists(), "write_json_fast leaves a .bak hard link");

    // Dissolution: no plan, no coordinator, no members hits the
    // remove_file branch instead of writing a snapshot.
    persist_swarm_state("swarm-dissolve", None, None, &[]);
    assert!(!state_path("swarm-dissolve").exists());
    assert!(
        !bak_path.exists(),
        "empty-state persistence must remove the recovery backup too"
    );

    let loaded = load_runtime_state();
    assert!(
        !loaded.coordinators.contains_key("swarm-dissolve"),
        "a dissolved swarm must not be restored on the next load"
    );
}

/// Delete-vs-write interleaving between `remove_persisted_swarm_state_for`
/// and a concurrent persist (wiring-audit.bak-resurrection, part b).
///
/// `remove_persisted_swarm_state_for` (server.rs:120) is `load_runtime()
/// .await` followed by an unserialized `remove_swarm_state`. Like the
/// persist inversion race above, `load_runtime` observes the four state
/// maps across multiple await points, so a remover that saw an all-empty
/// (dissolved) runtime can park, lose the race to a swarm re-creation plus
/// persist, then resume and delete the FRESH snapshot the re-creation just
/// wrote. Two failures compound:
///   1. Orphaned live swarm: the recreated swarm (coordinator registered
///      in memory) has no primary snapshot, so a clean restart loses it.
///   2. Zombie resurrection: the persist that the remover clobbered
///      hard-linked the PRE-dissolution snapshot to `.bak`, and
///      `load_runtime_state` reads `.bak` files, so restart restores the
///      stale pre-dissolution state instead.
///
/// Same gate technique as
/// `stale_persist_cannot_regress_newer_plan_version`:
/// park A inside `load_runtime` at the contended `members.read()`, run
/// mutator B's re-creation and persist while A is parked, release A.
#[tokio::test]
#[allow(clippy::await_holding_lock)]
async fn stale_remove_cannot_delete_fresh_snapshot_or_restore_backup() {
    let dir = tempfile::TempDir::new().expect("tempdir");
    let _env = test_env(&dir);

    // The previous incarnation's snapshot is on disk; the swarm has since
    // been dissolved, so the in-memory runtime is empty.
    persist_swarm_state("swarm-del-race", None, Some("coord-stale"), &[]);
    let swarm_state = crate::server::SwarmState::new(
        HashMap::new(),
        HashMap::new(),
        HashMap::new(),
        HashMap::new(),
    );

    // Gate: hold members.write() so remover A parks inside load_runtime at
    // the final members.read(), AFTER it has already observed the
    // dissolved (all-empty) plans/coordinators/swarms_by_id state.
    let gate = swarm_state.members.write().await;

    let a = tokio::spawn({
        let swarm_state = swarm_state.clone();
        async move {
            crate::server::remove_persisted_swarm_state_for("swarm-del-race", &swarm_state).await;
        }
    });
    // Current-thread test runtime: yielding runs A until it parks on the
    // contended members.read().await.
    for _ in 0..16 {
        tokio::task::yield_now().await;
    }

    // Mutator B: the swarm is recreated while A is parked. B registers a
    // new coordinator in memory ...
    {
        let mut coordinators = swarm_state.coordinators.write().await;
        coordinators.insert("swarm-del-race".to_string(), "coord-new".to_string());
    }
    // ... and B's persist half runs to completion (in production this is
    // B's own persist_swarm_state_for on another worker thread, whose
    // uncontended lock reads resolve without suspending). This overwrite
    // also hard-links the stale pre-dissolution snapshot to `.bak`.
    persist_swarm_state("swarm-del-race", None, Some("coord-new"), &[]);
    let on_disk = storage::read_json::<PersistedSwarmState>(&state_path("swarm-del-race"))
        .expect("fresh snapshot");
    assert_eq!(
        on_disk.coordinator_session_id.as_deref(),
        Some("coord-new"),
        "fresh snapshot must be durably on disk before A resumes"
    );

    // Release A: its stale all-empty runtime passes has_any_state(), but the
    // compare-and-delete guard must notice that the durable snapshot changed.
    drop(gate);
    a.await.expect("remove task");

    assert!(
        state_path("swarm-del-race").exists(),
        "a stale remove must not delete a freshly persisted snapshot"
    );
    let loaded = load_runtime_state();
    assert_eq!(
        loaded.coordinators.get("swarm-del-race"),
        Some(&"coord-new".to_string()),
        "restart must restore the fresh incarnation, not its stale backup"
    );
}
