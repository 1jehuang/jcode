use super::super::swarm_persistence::{PersistedSwarmMember, PersistedVersionedPlan};
use super::*;
use crate::session::Session;
use jcode_swarm_core::{SwarmLifecycleStatus, SwarmMemberRecord, SwarmRole};
use std::collections::HashMap;
use std::sync::Arc;
use tokio::sync::RwLock;

struct TestHome {
    _lock: storage::TestEnvGuard,
    _dir: tempfile::TempDir,
    home: Option<std::ffi::OsString>,
    runtime: Option<std::ffi::OsString>,
}
impl TestHome {
    fn new() -> Self {
        let lock = storage::lock_test_env();
        let dir = tempfile::tempdir().unwrap();
        let home = std::env::var_os("JCODE_HOME");
        let runtime = std::env::var_os("JCODE_RUNTIME_DIR");
        crate::env::set_var("JCODE_HOME", dir.path());
        crate::env::set_var("JCODE_RUNTIME_DIR", dir.path().join("runtime"));
        Self {
            _lock: lock,
            _dir: dir,
            home,
            runtime,
        }
    }
}
impl Drop for TestHome {
    fn drop(&mut self) {
        for (key, value) in [
            ("JCODE_HOME", self.home.take()),
            ("JCODE_RUNTIME_DIR", self.runtime.take()),
        ] {
            if let Some(value) = value {
                crate::env::set_var(key, value);
            } else {
                crate::env::remove_var(key);
            }
        }
    }
}
fn state() -> SwarmState {
    SwarmState::new(
        HashMap::new(),
        HashMap::new(),
        HashMap::new(),
        HashMap::new(),
    )
}
fn agents() -> SessionAgents {
    Arc::new(RwLock::new(HashMap::new()))
}
fn member(id: &str, swarm: &str) -> PersistedSwarmMember {
    PersistedSwarmMember {
        record: SwarmMemberRecord {
            session_id: id.into(),
            working_dir: None,
            swarm_id: Some(swarm.into()),
            swarm_enabled: true,
            status: SwarmLifecycleStatus::Ready,
            detail: None,
            task_label: Some("owned lane".into()),
            friendly_name: Some(id.into()),
            report_back_to_session_id: None,
            latest_completion_report: Some("checkpoint".into()),
            role: SwarmRole::Agent,
            is_headless: true,
        },
        terminal_since_unix_ms: None,
    }
}
fn swarm(id: &str, ids: &[&str]) -> PersistedSwarmState {
    PersistedSwarmState {
        swarm_id: id.into(),
        plan: None,
        coordinator_session_id: ids.first().map(|s| s.to_string()),
        members: ids.iter().map(|id_| member(id_, id)).collect(),
        updated_at_unix_ms: 1_000_000,
    }
}
fn bundle(swarms: Vec<PersistedSwarmState>, ids: &[&str]) -> GroupTransferBundle {
    GroupTransferBundle {
        version: 1,
        transfer_id: "test-transfer".into(),
        destination_host: "cloud".into(),
        sessions: ids
            .iter()
            .map(|id| GroupTransferSession {
                session_id: id.to_string(),
                epoch: 1,
                previous_lease: None,
            })
            .collect(),
        swarms,
    }
}
fn save_session(id: &str, epoch: u64) -> Session {
    let mut session = Session::create_with_id(id.into(), None, None);
    session.migration_epoch = epoch;
    session.save_prepared().unwrap();
    session
}
fn inbound_lease(id: &str, epoch: u64) {
    storage::write_session_lease(&SessionLease {
        session_id: id.into(),
        epoch,
        away_host: Some(INBOUND_HOST.into()),
        repo_root: None,
        last_sync_snapshot: None,
        last_sync_head: None,
        updated_at: "now".into(),
    })
    .unwrap();
}

#[test]
fn closure_includes_entire_swarm_and_external_report_owner_but_not_unrelated() {
    let mut a = swarm("a", &["leader", "worker"]);
    a.members[1].record.report_back_to_session_id = Some("outer".into());
    let b = swarm("b", &["outer", "sibling"]);
    let c = swarm("unrelated", &["untouched"]);
    let (sessions, swarms) = expand_selection(
        &GroupTransferSelection {
            session_ids: vec!["worker".into(), "independent".into()],
            swarm_ids: vec![],
        },
        &[a, b, c],
    )
    .unwrap();
    assert_eq!(
        sessions,
        ["independent", "leader", "outer", "sibling", "worker"]
            .map(str::to_string)
            .into()
    );
    assert_eq!(swarms, ["a", "b"].map(str::to_string).into());
}

#[test]
fn rejects_incomplete_or_forged_bundle() {
    let mut transfer = bundle(vec![swarm("a", &["leader", "worker"])], &["leader"]);
    assert!(validate_bundle(&transfer).is_err());
    transfer.sessions.push(GroupTransferSession {
        session_id: "../escape".into(),
        epoch: 1,
        previous_lease: None,
    });
    assert!(validate_bundle(&transfer).is_err());
    let mut transfer = bundle(vec![], &["leader"]);
    local_lease_value(&mut transfer);
    assert!(validate_bundle(&transfer).is_err());
}
fn local_lease_value(transfer: &mut GroupTransferBundle) {
    transfer.sessions[0].previous_lease = Some(SessionLease {
        session_id: "victim".into(),
        epoch: 0,
        away_host: None,
        repo_root: None,
        last_sync_snapshot: None,
        last_sync_head: None,
        updated_at: "now".into(),
    });
}

#[tokio::test]
async fn prepare_rejects_busy_gate_then_fences_and_abort_restores() {
    let _home = TestHome::new();
    let mut stale = save_session("independent", 0);
    let agents = agents();
    let state = state();
    let gate = crate::runtime_activity::ActivityGate::default();
    let inspected = handle_with_gate(
        "group_transfer:inspect:{\"session_ids\":[\"independent\"]}",
        &agents,
        &state,
        &gate,
    )
    .await
    .unwrap()
    .unwrap();
    let mut expected: GroupTransferBundle = serde_json::from_str(&inspected).unwrap();
    expected.destination_host = "cloud".into();
    let command = format!(
        "group_transfer:prepare:{}",
        serde_json::json!({"session_ids":["independent"],"destination_host":"cloud","expected":expected})
    );
    let active = gate.try_enter().unwrap();
    assert!(
        handle_with_gate(&command, &agents, &state, &gate)
            .await
            .unwrap_err()
            .to_string()
            .contains("busy")
    );
    assert!(storage::read_session_lease("independent").is_none());
    drop(active);
    let result = handle_with_gate(&command, &agents, &state, &gate)
        .await
        .unwrap()
        .unwrap();
    let transfer: GroupTransferBundle = serde_json::from_str(&result).unwrap();
    assert_eq!(transfer.sessions[0].epoch, 1);
    assert!(matches!(
        stale.migration_lease_block(),
        Some(storage::SessionLeaseBlock::MovedAway { .. })
    ));
    stale.title = Some("must not persist".into());
    stale.save_prepared().unwrap();
    assert_ne!(Session::load("independent").unwrap().title, stale.title);
    handle_with_gate(
        &format!("group_transfer:abort:{result}"),
        &agents,
        &state,
        &gate,
    )
    .await
    .unwrap();
    assert!(stale.migration_lease_block().is_none());
    handle_with_gate(
        &format!("group_transfer:abort:{result}"),
        &agents,
        &state,
        &gate,
    )
    .await
    .unwrap();
    assert!(
        handle_with_gate(&command, &agents, &state, &gate)
            .await
            .unwrap_err()
            .to_string()
            .contains("cancelled")
    );
}

#[tokio::test]
async fn import_preserves_unrelated_state_and_plan_without_starting_agents() {
    let _home = TestHome::new();
    let agents = agents();
    let state = state();
    let unrelated = bundle(vec![swarm("unrelated", &["untouched"])], &["untouched"]);
    save_session("untouched", 1);
    inbound_lease("untouched", 1);
    import_bundle(&unrelated, &agents, &state).await.unwrap();
    let before =
        serde_json::to_value(swarm_persistence::read_transfer_snapshot("unrelated").unwrap())
            .unwrap();
    let mut selected = swarm("selected", &["leader", "worker"]);
    selected.plan = Some(PersistedVersionedPlan {
        version: 7,
        participants: vec!["leader".into(), "worker".into()],
        task_progress: HashMap::new(),
        items: vec![crate::plan::PlanItem {
            content: "work".into(),
            status: "running".into(),
            priority: "high".into(),
            id: "task".into(),
            subsystem: None,
            file_scope: vec![],
            blocked_by: vec![],
            assigned_to: Some("worker".into()),
        }],
    });
    let selected = bundle(vec![selected], &["leader", "worker", "independent"]);
    for session in &selected.sessions {
        save_session(&session.session_id, 1);
        inbound_lease(&session.session_id, 1);
    }
    import_bundle(&selected, &agents, &state).await.unwrap();
    assert!(
        !storage::read_session_lease("worker").unwrap().is_here(),
        "import must not activate before transport commits"
    );
    assert!(
        agents.read().await.is_empty(),
        "import must not auto-resume any session"
    );
    assert_eq!(state.members.read().await["worker"].status, "stopped");
    assert_eq!(
        state.members.read().await["worker"].task_label.as_deref(),
        Some("owned lane")
    );
    assert_eq!(state.coordinators.read().await["selected"], "leader");
    let plans = state.plans.read().await;
    assert_eq!(plans["selected"].version, 7);
    assert_eq!(
        plans["selected"].items[0].assigned_to.as_deref(),
        Some("worker")
    );
    assert_eq!(plans["selected"].items[0].status, "running_stale");
    drop(plans);
    let after =
        serde_json::to_value(swarm_persistence::read_transfer_snapshot("unrelated").unwrap())
            .unwrap();
    assert_eq!(before, after);
    let conflict = bundle(vec![swarm("unrelated", &["leader"])], &["leader"]);
    assert!(
        validate_destination(&conflict, &agents, &state)
            .await
            .is_err()
    );
}

#[tokio::test]
async fn import_rejects_stale_destination_before_mutating_swarm_state() {
    let _home = TestHome::new();
    save_session("worker", 0);
    let state = state();
    let transfer = bundle(vec![swarm("selected", &["worker"])], &["worker"]);
    assert!(import_bundle(&transfer, &agents(), &state).await.is_err());
    assert!(state.members.read().await.is_empty());
}

#[tokio::test]
async fn abort_before_late_prepare_is_idempotent_and_durably_cancels() {
    let _home = TestHome::new();
    save_session("worker", 0);
    let sessions = agents();
    let state = state();
    let gate = crate::runtime_activity::ActivityGate::default();
    let expected = bundle(vec![], &["worker"]);
    let abort = format!(
        "group_transfer:abort:{}",
        serde_json::to_string(&expected).unwrap()
    );
    handle_with_gate(&abort, &sessions, &state, &gate)
        .await
        .unwrap();
    handle_with_gate(&abort, &sessions, &state, &gate)
        .await
        .unwrap();
    let prepare = format!(
        "group_transfer:prepare:{}",
        serde_json::json!({"session_ids":["worker"],"destination_host":"cloud","expected":expected})
    );
    assert!(
        handle_with_gate(&prepare, &sessions, &state, &gate)
            .await
            .unwrap_err()
            .to_string()
            .contains("cancelled")
    );
    assert!(storage::read_session_lease("worker").is_none());
}

#[tokio::test]
async fn changed_expected_epoch_never_partially_fences_group() {
    let _home = TestHome::new();
    save_session("one", 0);
    save_session("two", 0);
    let sessions = agents();
    let state = state();
    let gate = crate::runtime_activity::ActivityGate::default();
    let mut expected = bundle(vec![], &["one", "two"]);
    expected.sessions[1].epoch = 55;
    let prepare = format!(
        "group_transfer:prepare:{}",
        serde_json::json!({"session_ids":["one","two"],"destination_host":"cloud","expected":expected})
    );
    assert!(
        handle_with_gate(&prepare, &sessions, &state, &gate)
            .await
            .unwrap_err()
            .to_string()
            .contains("changed")
    );
    assert!(storage::read_session_lease("one").is_none());
    assert!(storage::read_session_lease("two").is_none());
}

#[tokio::test]
async fn reserve_fences_before_install_and_import_keeps_members_blocked() {
    let _home = TestHome::new();
    let sessions = agents();
    let state = state();
    let gate = crate::runtime_activity::ActivityGate::default();
    let mut expected = bundle(vec![swarm("selected", &["worker"])], &["worker"]);
    expected.swarms[0].members[0].record.working_dir = Some("/source/project".into());
    let payload = serde_json::to_string(&expected).unwrap();
    let reserve = format!("group_transfer:reserve:{payload}");
    handle_with_gate(&reserve, &sessions, &state, &gate)
        .await
        .unwrap();
    handle_with_gate(&reserve, &sessions, &state, &gate)
        .await
        .unwrap();
    let mut session = Session::create_with_id("worker".into(), None, None);
    session.migration_epoch = 1;
    session.working_dir = Some("/destination/project".into());
    assert!(session.migration_lease_block().is_some());
    // The transport installs the already-validated full snapshot, not a save
    // by a running agent (which the pending lease intentionally prohibits).
    storage::write_json_fast(&crate::session::session_path("worker").unwrap(), &session).unwrap();
    handle_with_gate(
        &format!("group_transfer:import:{payload}"),
        &sessions,
        &state,
        &gate,
    )
    .await
    .unwrap();
    assert!(session.migration_lease_block().is_some());
    assert!(sessions.read().await.is_empty());
    assert_eq!(state.members.read().await["worker"].status, "stopped");
    assert_eq!(
        state.members.read().await["worker"].working_dir.as_deref(),
        Some(std::path::Path::new("/destination/project"))
    );
}

struct IdleProvider;

#[async_trait::async_trait]
impl crate::provider::Provider for IdleProvider {
    async fn complete(
        &self,
        _: &[crate::message::Message],
        _: &[crate::message::ToolDefinition],
        _: &str,
        _: Option<&str>,
    ) -> anyhow::Result<crate::provider::EventStream> {
        anyhow::bail!("transfer must never call a model")
    }
    fn name(&self) -> &str {
        "mock"
    }
    fn fork(&self) -> Arc<dyn crate::provider::Provider> {
        Arc::new(Self)
    }
}

#[tokio::test]
async fn live_agents_are_reserved_together_then_only_selected_agents_evicted() {
    let _home = TestHome::new();
    let sessions = agents();
    let state = state();
    let gate = crate::runtime_activity::ActivityGate::default();
    let make_agent = |id: &str| {
        Arc::new(tokio::sync::Mutex::new(
            crate::agent::Agent::new_with_session(
                Arc::new(IdleProvider),
                crate::tool::Registry::empty(),
                save_session(id, 0),
                None,
            ),
        ))
    };
    let one = make_agent("one");
    let two = make_agent("two");
    let unrelated = make_agent("unrelated");
    sessions.write().await.extend([
        ("one".into(), one.clone()),
        ("two".into(), two.clone()),
        ("unrelated".into(), unrelated.clone()),
    ]);
    let raw = handle_with_gate(
        "group_transfer:inspect:{\"session_ids\":[\"one\",\"two\"]}",
        &sessions,
        &state,
        &gate,
    )
    .await
    .unwrap()
    .unwrap();
    let mut expected: GroupTransferBundle = serde_json::from_str(&raw).unwrap();
    expected.destination_host = "cloud".into();
    let command = format!(
        "group_transfer:prepare:{}",
        serde_json::json!({"session_ids":["one","two"],"destination_host":"cloud","expected":expected})
    );
    let busy = two.lock().await;
    assert!(
        handle_with_gate(&command, &sessions, &state, &gate)
            .await
            .unwrap_err()
            .to_string()
            .contains("busy")
    );
    assert!(
        storage::read_session_lease("one").is_none(),
        "a later busy worker must not leave earlier members fenced"
    );
    assert!(
        one.try_lock().is_ok(),
        "failed reservation must release earlier agents"
    );
    drop(busy);
    handle_with_gate(&command, &sessions, &state, &gate)
        .await
        .unwrap();
    assert_eq!(sessions.read().await.len(), 1);
    assert!(sessions.read().await.contains_key("unrelated"));
    assert!(one.lock().await.ensure_session_lease().is_err());
    assert!(two.lock().await.ensure_session_lease().is_err());
    assert!(unrelated.lock().await.ensure_session_lease().is_ok());
}
