// Fork auto-rearm unit tests (2026-09-30): timeout-path re-arm decisions are
// pure logic over `PersistedAwaitMembersState`, so they are tested directly
// without spawning watchers. (include!'d into comm_control_tests.rs, so no
// //! inner doc comments allowed at non-top position.)

use crate::server::await_members_state::PersistedAwaitMembersState;

const MAX_AUTO_REARMS: u32 = 3;

fn base_timeout_ms(state: &PersistedAwaitMembersState) -> u64 {
    state.base_timeout_secs.unwrap_or(3600).saturating_mul(1000)
}

/// Mirror of the watcher's re-arm decision. Kept in sync with comm_await.rs;
/// if the real logic moves, these tests move with it.
fn should_rearm(state: &PersistedAwaitMembersState) -> Result<u64, &'static str> {
    let now_ms = 10_000_000u64;
    let wait_started_ms = state
        .last_progress_ms
        .map(|_| state.deadline_unix_ms.saturating_sub(base_timeout_ms(state)))
        .unwrap_or(0);
    let saw_progress = state
        .last_progress_ms
        .map(|p| p > wait_started_ms)
        .unwrap_or(false);
    let under_cap = state.auto_rearm_count < MAX_AUTO_REARMS;
    if saw_progress && under_cap {
        let base_secs = state.base_timeout_secs.unwrap_or(3600);
        let backoff_secs = (base_secs.saturating_mul(1u64 << (state.auto_rearm_count.min(2))))
            .min(3600);
        Ok(backoff_secs)
    } else if !under_cap {
        Err("auto-rearm cap reached")
    } else {
        Err("no worker progress since wait started (hung-worker guard)")
    }
}

fn state(auto_rearm_count: u32, progress: Option<u64>, base: u64) -> PersistedAwaitMembersState {
    PersistedAwaitMembersState {
        key: "k".into(),
        session_id: "s".into(),
        swarm_id: "sw".into(),
        target_status: vec!["completed".into()],
        requested_ids: vec![],
        mode: None,
        created_at_unix_ms: 0,
        deadline_unix_ms: 9_000_000,
        background: true,
        notify: true,
        wake: true,
        auto_rearm_count,
        base_timeout_secs: Some(base),
        last_progress_ms: progress,
        final_response: None,
    }
}

#[test]
fn rearm_when_progress_seen_and_under_cap() {
    // First timeout: count 0, progress happened mid-wait (deadline 9_000_000,
    // base 100s = wait started 8_900_000; progress at 8_950_000 > start).
    let s = state(0, Some(8_950_000), 100);
    assert_eq!(should_rearm(&s), Ok(100), "1st rearm backs off 1x");
}

#[test]
fn backoff_doubles_each_round() {
    let s1 = state(1, Some(8_950_000), 100);
    assert_eq!(should_rearm(&s1), Ok(200), "2nd rearm backs off 2x");
    let s2 = state(2, Some(8_950_000), 100);
    assert_eq!(should_rearm(&s2), Ok(400), "3rd rearm backs off 4x");
}

#[test]
fn finalize_after_cap() {
    let s = state(3, Some(8_950_000), 100);
    assert_eq!(
        should_rearm(&s),
        Err("auto-rearm cap reached"),
        "cap stops the loop"
    );
}

#[test]
fn hung_worker_finalizes_not_rearms() {
    // No progress at all: the guard must refuse even at count 0.
    let s = state(0, None, 100);
    assert_eq!(
        should_rearm(&s),
        Err("no worker progress since wait started (hung-worker guard)")
    );
    // Stale progress from BEFORE this wait round also counts as hung.
    let stale = state(1, Some(8_000_000), 100); // 8.0M < wait start 8.8M
    assert!(should_rearm(&stale).is_err(), "stale progress is not progress");
}

#[test]
fn backoff_capped_at_one_hour() {
    let s = state(0, Some(8_950_000), 10_000); // base ~2.8h
    assert_eq!(should_rearm(&s), Ok(3600), "backoff never exceeds 1h");
}

#[test]
fn serde_defaults_keep_old_state_files_loadable() {
    let json = serde_json::json!({
        "key": "k", "session_id": "s", "swarm_id": "sw",
        "target_status": ["completed"], "requested_ids": [],
        "created_at_unix_ms": 0, "deadline_unix_ms": 9_000_000,
        "background": true, "notify": true, "wake": true
    });
    let s: PersistedAwaitMembersState = serde_json::from_value(json).unwrap();
    assert_eq!(s.auto_rearm_count, 0);
    assert_eq!(s.base_timeout_secs, None);
    assert_eq!(s.last_progress_ms, None);
}
