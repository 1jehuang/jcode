use super::{
    PersistedSwarmMutationResponse, SwarmMutationRuntime, begin_or_join_in_flight, begin_or_replay,
    finish_request, request_key,
};
use crate::protocol::ServerEvent;

struct RuntimeEnvGuard {
    _guard: std::sync::MutexGuard<'static, ()>,
    prev_runtime: Option<std::ffi::OsString>,
}

impl RuntimeEnvGuard {
    fn new() -> (Self, tempfile::TempDir) {
        let guard = crate::storage::lock_test_env();
        let temp = tempfile::TempDir::new().expect("create runtime dir");
        let prev_runtime = std::env::var_os("JCODE_RUNTIME_DIR");
        crate::env::set_var("JCODE_RUNTIME_DIR", temp.path());
        (
            Self {
                _guard: guard,
                prev_runtime,
            },
            temp,
        )
    }
}

impl Drop for RuntimeEnvGuard {
    fn drop(&mut self) {
        if let Some(prev_runtime) = self.prev_runtime.take() {
            crate::env::set_var("JCODE_RUNTIME_DIR", prev_runtime);
        } else {
            crate::env::remove_var("JCODE_RUNTIME_DIR");
        }
    }
}

#[tokio::test]
async fn swarm_mutation_replays_persisted_spawn_response() {
    let (_env, _runtime_dir) = RuntimeEnvGuard::new();
    let runtime = SwarmMutationRuntime::default();
    let (client_tx, mut client_rx) = tokio::sync::mpsc::unbounded_channel();
    let key = request_key(
        "coord",
        "spawn",
        &[
            "swarm-1".to_string(),
            "/repo".to_string(),
            "hello".to_string(),
        ],
    );

    let state = begin_or_replay(&runtime, &key, "spawn", "coord", 1, &client_tx)
        .await
        .expect("first request should start execution");
    finish_request(
        &runtime,
        &state,
        PersistedSwarmMutationResponse::Spawn {
            new_session_id: "child-1".to_string(),
        },
    )
    .await;

    let (retry_tx, mut retry_rx) = tokio::sync::mpsc::unbounded_channel();
    let replay = begin_or_replay(&runtime, &key, "spawn", "coord", 2, &retry_tx).await;
    assert!(replay.is_none(), "retry should replay persisted response");

    match client_rx.recv().await.expect("initial response") {
        ServerEvent::CommSpawnResponse { new_session_id, .. } => {
            assert_eq!(new_session_id, "child-1")
        }
        other => panic!("expected spawn response, got {other:?}"),
    }

    match retry_rx.recv().await.expect("replayed response") {
        ServerEvent::CommSpawnResponse {
            id, new_session_id, ..
        } => {
            assert_eq!(id, 2);
            assert_eq!(new_session_id, "child-1");
        }
        other => panic!("expected spawn replay, got {other:?}"),
    }
}

#[tokio::test]
async fn swarm_mutation_concurrent_duplicates_share_final_done_response() {
    let (_env, _runtime_dir) = RuntimeEnvGuard::new();
    let runtime = SwarmMutationRuntime::default();
    let key = request_key(
        "coord",
        "assign_task",
        &[
            "swarm-1".to_string(),
            "worker-1".to_string(),
            "task-1".to_string(),
            "extra".to_string(),
        ],
    );
    let (first_tx, mut first_rx) = tokio::sync::mpsc::unbounded_channel();
    let (retry_tx, mut retry_rx) = tokio::sync::mpsc::unbounded_channel();

    let state = begin_or_replay(&runtime, &key, "assign_task", "coord", 1, &first_tx)
        .await
        .expect("first request should start execution");
    let replay = begin_or_replay(&runtime, &key, "assign_task", "coord", 2, &retry_tx).await;
    assert!(
        replay.is_none(),
        "second in-flight duplicate should wait for original completion"
    );

    finish_request(&runtime, &state, PersistedSwarmMutationResponse::Done).await;

    match first_rx.recv().await.expect("first response") {
        ServerEvent::Done { id } => assert_eq!(id, 1),
        other => panic!("expected done, got {other:?}"),
    }
    match retry_rx.recv().await.expect("retry response") {
        ServerEvent::Done { id } => assert_eq!(id, 2),
        other => panic!("expected done, got {other:?}"),
    }
}

/// Regression: a control-driven mutation (retry/reassign/replace/salvage)
/// must re-dispatch even when an identical mutation persisted a success
/// within the final-state TTL. Replaying the stale success would turn the
/// coordinator's deliberate retry into a silent no-op whenever the worker
/// failed in under the TTL.
#[tokio::test]
async fn control_driven_begin_re_dispatches_despite_persisted_final_state() {
    let (_env, _runtime_dir) = RuntimeEnvGuard::new();
    let runtime = SwarmMutationRuntime::default();
    let key = request_key(
        "coord",
        "assign_task",
        &[
            "swarm-1".to_string(),
            "worker-1".to_string(),
            "task-1".to_string(),
            "Retry this assignment.".to_string(),
        ],
    );

    let (first_tx, mut first_rx) = tokio::sync::mpsc::unbounded_channel();
    let state = begin_or_join_in_flight(&runtime, &key, "assign_task", "coord", 1, &first_tx)
        .await
        .expect("first attempt should start execution");
    finish_request(
        &runtime,
        &state,
        PersistedSwarmMutationResponse::AssignTask {
            task_id: "task-1".to_string(),
            target_session: "worker-1".to_string(),
        },
    )
    .await;
    match first_rx.recv().await.expect("first response") {
        ServerEvent::CommAssignTaskResponse { id, .. } => assert_eq!(id, 1),
        other => panic!("expected assign response, got {other:?}"),
    }

    // Identical retry within the final-state TTL: must start a fresh attempt.
    let (retry_tx, _retry_rx) = tokio::sync::mpsc::unbounded_channel();
    let retry_state = begin_or_join_in_flight(&runtime, &key, "assign_task", "coord", 2, &retry_tx)
        .await
        .expect("control-driven retry must re-dispatch instead of replaying the stale success");
    assert!(
        retry_state.final_response.is_none(),
        "fresh attempt must not carry the previous attempt's final response"
    );
}

/// Concurrent in-flight duplicates of a control-driven mutation still
/// coalesce onto the active execution instead of double-dispatching.
#[tokio::test]
async fn control_driven_begin_coalesces_in_flight_duplicates() {
    let (_env, _runtime_dir) = RuntimeEnvGuard::new();
    let runtime = SwarmMutationRuntime::default();
    let key = request_key(
        "coord",
        "assign_task",
        &["swarm-1".to_string(), "worker-1".to_string()],
    );

    let (first_tx, mut first_rx) = tokio::sync::mpsc::unbounded_channel();
    let (dup_tx, mut dup_rx) = tokio::sync::mpsc::unbounded_channel();
    let state = begin_or_join_in_flight(&runtime, &key, "assign_task", "coord", 1, &first_tx)
        .await
        .expect("first attempt should start execution");
    let dup = begin_or_join_in_flight(&runtime, &key, "assign_task", "coord", 2, &dup_tx).await;
    assert!(
        dup.is_none(),
        "in-flight duplicate should join the active execution as a waiter"
    );

    finish_request(&runtime, &state, PersistedSwarmMutationResponse::Done).await;
    match first_rx.recv().await.expect("first response") {
        ServerEvent::Done { id } => assert_eq!(id, 1),
        other => panic!("expected done, got {other:?}"),
    }
    match dup_rx.recv().await.expect("duplicate response") {
        ServerEvent::Done { id } => assert_eq!(id, 2),
        other => panic!("expected done, got {other:?}"),
    }
}
