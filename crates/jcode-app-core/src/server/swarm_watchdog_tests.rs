use super::*;
use std::collections::HashMap;
use std::time::{Duration, Instant};
use tokio::sync::mpsc;

fn member(session_id: &str, role: &str, status: &str, status_age: Duration) -> SwarmMember {
    let (event_tx, _event_rx) = mpsc::unbounded_channel();
    SwarmMember {
        session_id: session_id.to_string(),
        event_tx,
        event_txs: HashMap::new(),
        working_dir: None,
        swarm_id: Some("swarm-wd".to_string()),
        swarm_enabled: true,
        status: status.to_string(),
        detail: None,
        task_label: None,
        friendly_name: Some(format!("{session_id}-name")),
        report_back_to_session_id: None,
        latest_completion_report: None,
        role: role.to_string(),
        joined_at: Instant::now(),
        last_status_change: Instant::now()
            .checked_sub(status_age)
            .unwrap_or_else(Instant::now),
        is_headless: true,
        output_tail: None,
        todo_progress: None,
        todo_items: Vec::new(),
        runtime: crate::protocol::SwarmMemberRuntime::default(),
    }
}

fn inputs(status: &'static str, activity: Option<u64>, status_age: u64) -> StallInputs<'static> {
    StallInputs {
        status,
        role: "agent",
        activity_age_secs: activity,
        status_age_secs: status_age,
        tool_running: false,
    }
}

const WINDOW: Duration = Duration::from_secs(600);

#[test]
fn silent_running_worker_is_stalled() {
    assert_eq!(
        evaluate_stall(inputs("running", Some(700), 900), WINDOW),
        Some(700)
    );
    assert_eq!(
        evaluate_stall(inputs("running", None, 650), WINDOW),
        Some(650)
    );
}

#[test]
fn recent_activity_or_recent_turn_start_is_not_stalled() {
    assert_eq!(
        evaluate_stall(inputs("running", Some(30), 900), WINDOW),
        None
    );
    // Stale activity clock from an earlier turn, but the turn just started.
    assert_eq!(
        evaluate_stall(inputs("running", Some(5000), 20), WINDOW),
        None
    );
}

#[test]
fn running_tool_never_counts_as_stall() {
    let mut i = inputs("running", Some(5000), 5000);
    i.tool_running = true;
    assert_eq!(evaluate_stall(i, WINDOW), None);
}

#[test]
fn only_running_non_coordinator_members_can_stall() {
    for status in ["ready", "queued", "completed", "failed", "stopped"] {
        let i = StallInputs {
            status,
            ..inputs("running", Some(5000), 5000)
        };
        assert_eq!(evaluate_stall(i, WINDOW), None, "status {status}");
    }
    let mut i = inputs("running", Some(5000), 5000);
    i.role = "coordinator";
    assert_eq!(evaluate_stall(i, WINDOW), None);
}

#[test]
fn sweep_interval_is_clamped() {
    assert_eq!(
        sweep_interval(Duration::from_secs(600)),
        Duration::from_secs(60)
    );
    assert_eq!(
        sweep_interval(Duration::from_secs(120)),
        Duration::from_secs(30)
    );
    assert_eq!(
        sweep_interval(Duration::from_secs(1)),
        Duration::from_secs(5)
    );
}

#[test]
fn update_stalls_notifies_once_and_rearms_after_recovery() {
    let worker = "wd_test_worker_once";
    let owned = "wd_test_worker_owned";
    let coord = "wd_test_coord";
    clear_for_tests(&[worker, owned, coord]);

    let mut members = HashMap::new();
    members.insert(
        worker.to_string(),
        member(worker, "agent", "running", Duration::from_secs(900)),
    );
    let mut owned_member = member(owned, "agent", "running", Duration::from_secs(900));
    owned_member.report_back_to_session_id = Some("wd_test_owner".to_string());
    members.insert(owned.to_string(), owned_member);
    members.insert(
        coord.to_string(),
        member(coord, "coordinator", "running", Duration::from_secs(900)),
    );
    let coordinators = HashMap::from([("swarm-wd".to_string(), coord.to_string())]);

    let silent = |_: &str| Some(800);
    let no_tool = |_: &str| false;

    let first = update_stalls(&members, &coordinators, WINDOW, silent, no_tool);
    assert_eq!(first.len(), 2);
    let by_id: HashMap<_, _> = first.iter().map(|s| (s.session_id.as_str(), s)).collect();
    // Unowned worker falls back to the swarm coordinator.
    assert_eq!(by_id[worker].recipient_session_id.as_deref(), Some(coord));
    assert_eq!(by_id[worker].label, format!("{worker}-name"));
    assert_eq!(by_id[worker].silent_secs, 800);
    // Spawned worker reports to its explicit owner.
    assert_eq!(
        by_id[owned].recipient_session_id.as_deref(),
        Some("wd_test_owner")
    );
    assert!(stalled_for_secs(worker).is_some_and(|secs| secs >= 800));
    assert!(stalled_for_secs(coord).is_none());

    // Still stalled: no repeat notification.
    let second = update_stalls(&members, &coordinators, WINDOW, silent, no_tool);
    assert!(second.is_empty());

    // Activity resumes for one worker: it is cleared.
    let recovered = |sid: &str| if sid == worker { Some(1) } else { Some(800) };
    assert!(update_stalls(&members, &coordinators, WINDOW, recovered, no_tool).is_empty());
    assert!(stalled_for_secs(worker).is_none());
    assert!(stalled_for_secs(owned).is_some());

    // A fresh stall after recovery notifies again.
    let again = update_stalls(&members, &coordinators, WINDOW, silent, no_tool);
    assert_eq!(again.len(), 1);
    assert_eq!(again[0].session_id, worker);

    // A long-running tool clears the stall flag.
    let tool = |sid: &str| sid == owned;
    assert!(update_stalls(&members, &coordinators, WINDOW, silent, tool).is_empty());
    assert!(stalled_for_secs(owned).is_none());

    clear_for_tests(&[worker, owned, coord]);
}

#[test]
fn messages_mention_recovery_actions() {
    let stall = NewStall {
        session_id: "s".to_string(),
        label: "fox".to_string(),
        recipient_session_id: None,
        silent_secs: 660,
    };
    let msg = coordinator_message(&stall);
    assert!(msg.contains("fox"));
    assert!(msg.contains("11m"));
    assert!(msg.contains("swarm retry"));
    assert!(nudge_message(90).contains("1m"));
}
