use super::*;
use crate::protocol::AwaitedMemberStatus;
use crate::server::await_members_state::{
    PersistedAwaitMembersResult, PersistedAwaitMembersState, load_state, request_key, save_state,
};
use crate::server::comm_await::mode_summary;

const CACHE_REQUESTER: &str = "cache-owner";
const CACHE_SWARM: &str = "cache-freshness";

fn cached_row(id: &str, status: &str, report: &str) -> AwaitedMemberStatus {
    AwaitedMemberStatus {
        session_id: id.into(),
        friendly_name: Some(id.into()),
        status: status.into(),
        done: true,
        completion_report: Some(report.into()),
    }
}

fn cached_wait(
    rows: Vec<AwaitedMemberStatus>,
    requested_ids: Vec<String>,
    mode: Option<&str>,
) -> PersistedAwaitMembersState {
    let now = SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .unwrap()
        .as_millis() as u64;
    let target_status = vec!["completed".into(), "failed".into()];
    PersistedAwaitMembersState {
        key: request_key(
            CACHE_REQUESTER,
            CACHE_SWARM,
            &requested_ids,
            &target_status,
            mode,
        ),
        session_id: CACHE_REQUESTER.into(),
        swarm_id: CACHE_SWARM.into(),
        target_status,
        requested_ids,
        mode: mode.map(str::to_string),
        created_at_unix_ms: now,
        deadline_unix_ms: now + 60_000,
        background: false,
        notify: false,
        wake: false,
        final_response: Some(PersistedAwaitMembersResult {
            completed: true,
            summary: mode_summary(&rows, mode),
            members: rows,
            resolved_at_unix_ms: now,
        }),
    }
}

fn current_worker(id: &str, status: &str, report: &str) -> SwarmMember {
    let mut worker = member(id, CACHE_SWARM, status);
    worker.latest_completion_report = Some(report.into());
    worker
}

async fn issue_cached_wait(
    state: &PersistedAwaitMembersState,
    workers: Vec<SwarmMember>,
    background: bool,
) -> (bool, Vec<AwaitedMemberStatus>, String, bool) {
    save_state(state);
    let mut members = HashMap::from([(
        CACHE_REQUESTER.into(),
        member(CACHE_REQUESTER, CACHE_SWARM, "ready"),
    )]);
    for worker in workers {
        members.insert(worker.session_id.clone(), worker);
    }
    let original_members = members.clone();
    let swarms_by_id = Arc::new(RwLock::new(HashMap::from([(
        CACHE_SWARM.into(),
        members.keys().cloned().collect(),
    )])));
    let swarm_members = Arc::new(RwLock::new(members));
    let runtime = AwaitMembersRuntime::default();
    let (client_tx, mut client_rx) = mpsc::unbounded_channel();
    let (swarm_event_tx, _swarm_event_rx) = broadcast::channel(32);
    handle_comm_await_members(
        91,
        CACHE_REQUESTER.into(),
        state.target_status.clone(),
        state.requested_ids.clone(),
        state.mode.clone(),
        Some(3600),
        background,
        false,
        false,
        CommAwaitMembersContext {
            client_event_tx: &client_tx,
            swarm_members: &swarm_members,
            swarms_by_id: &swarms_by_id,
            swarm_event_tx: &swarm_event_tx,
            await_members_runtime: &runtime,
        },
    )
    .await;
    let response = tokio::time::timeout(Duration::from_secs(1), client_rx.recv())
        .await
        .expect("cached or expired await should answer immediately")
        .expect("response channel should stay open");
    let members = swarm_members.read().await;
    for (id, original) in original_members {
        let current = &members[&id];
        assert_eq!(current.status, original.status);
        assert_eq!(
            current.latest_completion_report,
            original.latest_completion_report
        );
        assert_eq!(current.joined_at, original.joined_at);
    }
    match response {
        ServerEvent::CommAwaitMembersResponse {
            id,
            completed,
            members,
            summary,
            background_started,
        } => {
            assert_eq!(id, 91);
            (completed, members, summary, background_started)
        }
        other => panic!("expected await response, got {other:?}"),
    }
}

#[tokio::test]
async fn await_cache_refreshes_recovered_workers_status_and_reports() {
    let (_env, _runtime_dir) = RuntimeEnvGuard::new();
    let cached = (0..7)
        .map(|index| {
            cached_row(
                &format!("worker-{index}"),
                if index >= 5 { "failed" } else { "completed" },
                &format!("original report {index}"),
            )
        })
        .collect();
    let state = cached_wait(cached, vec![], None);
    let current = (0..7)
        .map(|index| {
            current_worker(
                &format!("worker-{index}"),
                "completed",
                &format!("current report {index}"),
            )
        })
        .collect();
    let (completed, rows, _, background_started) = issue_cached_wait(&state, current, true).await;
    assert!(completed);
    assert!(!background_started);
    assert_eq!(rows.len(), 7);
    for (index, row) in rows.iter().enumerate() {
        assert_eq!(row.status, "completed");
        assert_eq!(
            row.completion_report,
            Some(format!("current report {index}"))
        );
    }
}

#[tokio::test]
async fn await_cache_refreshes_report_without_status_change() {
    let (_env, _runtime_dir) = RuntimeEnvGuard::new();
    let state = cached_wait(
        vec![cached_row("peer", "completed", "old report")],
        vec!["peer".into()],
        None,
    );
    let mut current = current_worker("peer", "completed", "new report");
    current.friendly_name = Some("current worker name".into());
    let (completed, rows, summary, background_started) =
        issue_cached_wait(&state, vec![current], false).await;
    assert!(completed);
    assert!(!background_started);
    assert_eq!(rows[0].completion_report.as_deref(), Some("new report"));
    assert_eq!(
        rows[0].friendly_name.as_deref(),
        Some("current worker name")
    );
    assert!(summary.contains("current worker name"));
}

#[tokio::test]
async fn await_cache_unchanged_terminal_result_stays_inline() {
    let (_env, _runtime_dir) = RuntimeEnvGuard::new();
    let state = cached_wait(
        vec![cached_row("peer", "completed", "unchanged report")],
        vec!["peer".into()],
        None,
    );
    let saved = state.final_response.as_ref().unwrap();
    let (completed, rows, summary, background_started) = issue_cached_wait(
        &state,
        vec![current_worker("peer", "completed", "unchanged report")],
        true,
    )
    .await;
    assert!(completed);
    assert!(!background_started);
    assert_eq!(summary, saved.summary);
    assert_eq!(
        serde_json::to_value(rows).unwrap(),
        serde_json::to_value(&saved.members).unwrap(),
    );
}

#[tokio::test]
async fn await_cache_keeps_reload_fallback_when_members_are_unavailable() {
    let (_env, _runtime_dir) = RuntimeEnvGuard::new();
    for requested in [vec![], vec!["peer".into()]] {
        let state = cached_wait(
            vec![cached_row("peer", "completed", "durable report")],
            requested,
            None,
        );
        let saved = state.final_response.as_ref().unwrap();
        let (completed, rows, summary, background_started) =
            issue_cached_wait(&state, vec![], false).await;
        assert!(completed);
        assert!(!background_started);
        assert_eq!(summary, saved.summary);
        assert_eq!(
            serde_json::to_value(rows).unwrap(),
            serde_json::to_value(&saved.members).unwrap(),
        );
    }
}

#[tokio::test]
async fn await_cache_any_mode_uses_current_pending_rows_and_reports() {
    let (_env, _runtime_dir) = RuntimeEnvGuard::new();
    let state = cached_wait(
        vec![
            cached_row("done", "failed", "old failure"),
            cached_row("pending", "completed", "old completion"),
        ],
        vec![],
        Some("any"),
    );
    let (completed, rows, summary, background_started) = issue_cached_wait(
        &state,
        vec![
            current_worker("done", "completed", "recovered report"),
            current_worker("pending", "running", "current progress"),
        ],
        true,
    )
    .await;
    assert!(completed);
    assert!(!background_started);
    assert_eq!(rows[0].status, "completed");
    assert_eq!(
        rows[0].completion_report.as_deref(),
        Some("recovered report")
    );
    assert_eq!(rows[1].status, "running");
    assert!(!rows[1].done);
    assert_eq!(
        rows[1].completion_report.as_deref(),
        Some("current progress")
    );
    assert_eq!(summary, mode_summary(&rows, Some("any")));
}

#[tokio::test]
async fn await_cache_expired_pending_preserves_deadline_and_timeout() {
    let (_env, _runtime_dir) = RuntimeEnvGuard::new();
    for background in [false, true] {
        let mut state = cached_wait(vec![], vec!["peer".into()], None);
        state.final_response = None;
        state.deadline_unix_ms = state.created_at_unix_ms.saturating_sub(1000);
        state.background = background;
        let (completed, rows, summary, background_started) = issue_cached_wait(
            &state,
            vec![current_worker("peer", "running", "current progress")],
            background,
        )
        .await;
        assert!(!completed);
        assert!(!background_started);
        assert_eq!(rows[0].status, "running");
        assert_eq!(
            rows[0].completion_report.as_deref(),
            Some("current progress")
        );
        assert!(summary.contains("Timed out"));
        let saved = load_state(&state.key).expect("expired wait should remain durable");
        assert_eq!(saved.deadline_unix_ms, state.deadline_unix_ms);
        assert!(!saved.final_response.unwrap().completed);
    }
}
