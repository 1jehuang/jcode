use super::{
    broadcast_swarm_plan, broadcast_swarm_plan_with_previous, broadcast_swarm_status,
    broadcast_swarm_status_now, member_in_status_broadcast, member_status_is_dead,
    monitor_wait_status, now_unix_ms, parse_swarm_tasks, refresh_swarm_task_staleness,
    remove_session_from_swarm, salvage_assignments_of_dead_member, swarm_ancestors,
    swarm_is_self_or_ancestor, swarm_spawn_depth, touch_swarm_task_progress, update_member_status,
    update_member_status_with_report,
};
use crate::plan::PlanItem;
use crate::protocol::{NotificationType, ServerEvent};
use crate::server::{SwarmMember, VersionedPlan};
use jcode_swarm_core::{summarize_plan_items, truncate_detail};
use std::collections::{HashMap, HashSet};
use std::sync::Arc;
use std::time::{Duration, Instant};
use tokio::sync::{RwLock, mpsc};

#[test]
fn inline_worker_environment_is_removed_when_its_scope_ends() {
    let parent = "inline-env-cleanup-parent";
    let child = "inline-env-cleanup-child";
    let env = HashMap::from([("INLINE_WORKER_SECRET".into(), "test-only".into())]);
    crate::session_env::set_session_env(parent, env.clone()).unwrap();
    {
        crate::session_env::inherit_session_env(parent, child);
        let _guard = super::InlineWorkerEnv(child.into());
        assert_eq!(crate::session_env::session_env(child), env);
    }
    assert!(crate::session_env::session_env(child).is_empty());
    assert_eq!(crate::session_env::session_env(parent), env);
    crate::session_env::remove_session_env(parent);
}

fn plan_item(id: &str, content: &str) -> PlanItem {
    PlanItem {
        content: content.to_string(),
        status: "pending".to_string(),
        priority: "medium".to_string(),
        id: id.to_string(),
        subsystem: None,
        file_scope: Vec::new(),
        blocked_by: Vec::new(),
        assigned_to: None,
    }
}

#[test]
fn truncate_detail_collapses_whitespace_and_ellipsizes() {
    assert_eq!(truncate_detail("hello   there\nworld", 11), "hello th...");
}

#[test]
fn summarize_plan_items_limits_output() {
    let items = vec![
        plan_item("1", "inspect"),
        plan_item("2", "refactor"),
        plan_item("3", "test"),
    ];

    assert_eq!(
        summarize_plan_items(&items, 2),
        "inspect; refactor (+1 more)"
    );
}

#[test]
fn parse_swarm_tasks_accepts_wrapped_json() {
    let text = "Plan:\n[{\"description\":\"A\",\"prompt\":\"B\",\"subagent_type\":\"general\"}]";
    let tasks = parse_swarm_tasks(text);

    assert_eq!(tasks.len(), 1);
    assert_eq!(tasks[0].description, "A");
    assert_eq!(tasks[0].prompt, "B");
    assert_eq!(tasks[0].subagent_type.as_deref(), Some("general"));
}

fn swarm_member(
    session_id: &str,
    role: &str,
    is_headless: bool,
) -> (SwarmMember, mpsc::UnboundedReceiver<ServerEvent>) {
    let (event_tx, event_rx) = mpsc::unbounded_channel();
    (
        SwarmMember {
            session_id: session_id.to_string(),
            event_tx,
            event_txs: HashMap::new(),
            working_dir: None,
            swarm_id: Some("swarm-1".to_string()),
            swarm_enabled: true,
            status: "ready".to_string(),
            detail: None,
            task_label: None,
            friendly_name: Some(session_id.to_string()),
            report_back_to_session_id: None,
            latest_completion_report: None,
            role: role.to_string(),
            joined_at: Instant::now(),
            last_status_change: Instant::now(),
            is_headless,
            output_tail: None,
            todo_progress: None,
            todo_items: Vec::new(),
            runtime: crate::protocol::SwarmMemberRuntime::default(),
        },
        event_rx,
    )
}

#[test]
fn event_fanout_moves_payload_to_final_live_receiver() {
    let (first, mut first_rx) = mpsc::unbounded_channel();
    let (closed, closed_rx) = mpsc::unbounded_channel();
    let (last, mut last_rx) = mpsc::unbounded_channel();
    drop(closed_rx);
    let detail = "a large event payload".repeat(1000);
    let original = detail.as_ptr();
    assert_eq!(
        super::super::state::send_event_to_targets(
            [&first, &closed, &last],
            ServerEvent::StatusDetail { detail },
        ),
        2
    );
    let ServerEvent::StatusDetail { detail: first } = first_rx.try_recv().unwrap() else {
        panic!("first receiver got wrong event");
    };
    let ServerEvent::StatusDetail { detail: last } = last_rx.try_recv().unwrap() else {
        panic!("last receiver got wrong event");
    };
    assert_eq!(first, last);
    assert_ne!(first.as_ptr(), original);
    assert_eq!(last.as_ptr(), original, "last receiver must not clone");
    assert!(first_rx.try_recv().is_err());
    assert!(last_rx.try_recv().is_err());
}

#[tokio::test]
async fn swarm_status_reaches_attachments_and_headless_fallback_once() {
    let (mut attached, mut stale_rx) = swarm_member("attached", "agent", false);
    stale_rx.close();
    let (one, mut one_rx) = mpsc::unbounded_channel();
    let (two, mut two_rx) = mpsc::unbounded_channel();
    let (closed, closed_rx) = mpsc::unbounded_channel();
    drop(closed_rx);
    attached.event_txs = HashMap::from([
        ("one".into(), one),
        ("two".into(), two),
        ("closed".into(), closed),
    ]);
    let (headless, mut headless_rx) = swarm_member("headless", "agent", true);
    let (dead, dead_rx) = swarm_member("dead", "agent", true);
    drop(dead_rx);
    let members = Arc::new(RwLock::new(HashMap::from([
        ("attached".into(), attached),
        ("headless".into(), headless),
        ("dead".into(), dead),
    ])));
    super::broadcast_swarm_status_now(
        vec!["attached".into(), "headless".into(), "dead".into()],
        &members,
    )
    .await;
    for rx in [&mut one_rx, &mut two_rx, &mut headless_rx] {
        let ServerEvent::SwarmStatus { members } = rx.try_recv().unwrap() else {
            panic!("expected swarm status");
        };
        assert_eq!(members.len(), 3, "disconnected members stay in snapshot");
        let attached = members.iter().find(|m| m.session_id == "attached").unwrap();
        assert_eq!(attached.live_attachments, Some(2));
        assert!(rx.try_recv().is_err(), "exactly one event per queue");
    }
    assert!(stale_rx.try_recv().is_err());
    assert_eq!(members.read().await["attached"].event_txs.len(), 2);
}

#[tokio::test]
#[ignore = "comparative swarm payload benchmark, run with --ignored --nocapture"]
async fn benchmark_swarm_payload_fanout() {
    use std::hint::black_box;
    let (mut member, mut rx) = swarm_member("payload", "agent", true);
    member.output_tail = Some("recent streamed output\n".repeat(180));
    let mut fixture = HashMap::new();
    for index in 0..32 {
        let mut member = member.clone();
        member.session_id = format!("payload-{index}");
        fixture.insert(member.session_id.clone(), member);
    }
    let ids = fixture.keys().cloned().collect();
    let fixture = Arc::new(RwLock::new(fixture));
    super::broadcast_swarm_status_now(ids, &fixture).await;
    let payload = rx.recv().await.unwrap();
    // Every fixture member shares this queue, so discard the other copies.
    while rx.try_recv().is_ok() {}
    let bytes = serde_json::to_vec(&payload).unwrap().len();
    for live in [1, 4] {
        let mut targets = Vec::new();
        let mut receivers = Vec::new();
        for index in 0..64 {
            let (tx, rx) = mpsc::unbounded_channel();
            targets.push(tx);
            if index < live {
                receivers.push(rx);
            }
        }
        let mut times = Vec::new();
        for optimized in [false, true] {
            let start = Instant::now();
            for _ in 0..100 {
                // Both paths pay the same cost to supply an owned input.
                let event = payload.clone();
                let delivered = if optimized {
                    super::super::state::send_event_to_targets(&targets, event)
                } else {
                    let mut delivered = 0;
                    for tx in &targets {
                        // Previous broadcast cloned once per session, then
                        // generic fanout cloned again into its single queue.
                        let session_event = event.clone();
                        delivered += usize::from(tx.send(session_event.clone()).is_ok());
                    }
                    delivered
                };
                assert_eq!(delivered, live);
                for rx in &mut receivers {
                    let event = rx.try_recv().unwrap();
                    assert!(
                        matches!(&event, ServerEvent::SwarmStatus { members } if members.len() == 32)
                    );
                    black_box(event);
                    assert!(rx.try_recv().is_err());
                }
            }
            times.push(start.elapsed());
        }
        eprintln!(
            "swarm delivery: payload_bytes={bytes} sessions=64 live={live} iterations=100 baseline={:?} optimized={:?} speedup={:.2}x (queue/payload work only)",
            times[0],
            times[1],
            times[0].as_secs_f64() / times[1].as_secs_f64()
        );
    }
}

#[tokio::test(start_paused = true)]
async fn live_status_fanout_projects_worker_events_into_broadcast() {
    let sid = "live-status-projection-worker";
    let (worker, mut rx) = swarm_member(sid, "agent", true);
    let members = Arc::new(RwLock::new(HashMap::from([(sid.to_string(), worker)])));
    let swarms = Arc::new(RwLock::new(HashMap::from([(
        "swarm-1".into(),
        HashSet::from([sid.to_string()]),
    )])));
    update_member_status(sid, "running", None, &members, &swarms, None, None, None).await;
    for event in [
        ServerEvent::ConnectionType {
            connection: "sse".into(),
        },
        ServerEvent::ConnectionPhase {
            phase: "streaming".into(),
        },
        ServerEvent::TokenUsage {
            input: 123,
            output: 45,
            cache_read_input: Some(999),
            cache_creation_input: None,
            effective_budget: None,
        },
        ServerEvent::ToolStart {
            id: "call".into(),
            name: "bash".into(),
        },
        ServerEvent::ToolInput {
            id: Some("call".into()),
            delta: r#"{"intent":"Check telemetry"}"#.into(),
        },
        ServerEvent::ToolExec {
            id: "call".into(),
            name: "bash".into(),
        },
    ] {
        super::super::fanout_session_event(&members, sid, event).await;
    }
    super::broadcast_swarm_status_now(vec![sid.into()], &members).await;
    let mut projected = None;
    while let Ok(event) = rx.try_recv() {
        if let ServerEvent::SwarmStatus { members } = event {
            projected = members[0].runtime.live_status.clone();
        }
    }
    let live = projected.expect("live status included in actual wire event");
    assert_eq!(live.phase, "running_tool");
    assert_eq!(live.input_tokens, Some(123));
    assert_eq!(live.output_tokens, Some(45));
    assert_eq!(live.connection_type.as_deref(), Some("sse"));
    assert_eq!(live.tool_intent.as_deref(), Some("Check telemetry"));
    let monitor = tokio::spawn(super::monitor_wait_status(members.clone(), swarms.clone()));
    // Consume the initial timer snapshot, then require a usage transition
    // to publish before the next one-second refresh.
    tokio::time::timeout(Duration::from_secs(2), rx.recv())
        .await
        .unwrap()
        .unwrap();
    super::super::fanout_session_event(
        &members,
        sid,
        ServerEvent::TokenUsage {
            input: 124,
            output: 46,
            cache_read_input: None,
            cache_creation_input: None,
            effective_budget: None,
        },
    )
    .await;
    tokio::time::timeout(Duration::from_millis(500), async {
        loop {
            if let Some(ServerEvent::SwarmStatus { members }) = rx.recv().await
                && members[0]
                    .runtime
                    .live_status
                    .as_ref()
                    .and_then(|s| s.input_tokens)
                    == Some(124)
            {
                break;
            }
        }
    })
    .await
    .expect("usage change broadcasts immediately rather than waiting for heartbeat");
    monitor.abort();
    update_member_status(
        sid,
        "stopped",
        Some("User requested stop".into()),
        &members,
        &swarms,
        None,
        None,
        None,
    )
    .await;
    let stopped = super::super::live_status::snapshot(sid).unwrap();
    assert_eq!(stopped.phase, "stopped");
    assert_eq!(stopped.stop_reason.as_deref(), Some("User requested stop"));
    assert!(stopped.tool_name.is_none());
    update_member_status(
        sid,
        "stopped",
        Some("Owner intervention needed".into()),
        &members,
        &swarms,
        None,
        None,
        None,
    )
    .await;
    assert_eq!(
        super::super::live_status::snapshot(sid)
            .unwrap()
            .stop_reason
            .as_deref(),
        Some("Owner intervention needed")
    );
    // Late queued events cannot revive the finished turn.
    super::super::fanout_session_event(
        &members,
        sid,
        ServerEvent::TextDelta {
            text: "late".into(),
        },
    )
    .await;
    assert_eq!(
        super::super::live_status::snapshot(sid).unwrap().phase,
        "stopped"
    );
    super::super::live_status::begin(sid);
    let restarted = super::super::live_status::snapshot(sid).unwrap();
    assert!(restarted.connection_type.is_none());
    assert!(restarted.input_tokens.is_none());
    let mut todos = crate::bus::TodoEvent {
        session_id: sid.into(),
        todos: vec![crate::todo::TodoItem {
            id: "login".into(),
            content: "Authenticate".into(),
            status: "pending".into(),
            completely_blocked: Some(crate::todo::TodoCompleteBlocker {
                reason: "Required data is permanently unavailable".into(),
            }),
            ..Default::default()
        }],
    };
    super::super::background_tasks::dispatch_swarm_todo_progress(&todos, &members, &swarms).await;
    update_member_status(sid, "ready", None, &members, &swarms, None, None, None).await;
    super::broadcast_swarm_status_now(vec![sid.into()], &members).await;
    let latest = std::iter::from_fn(|| rx.try_recv().ok())
        .filter_map(|event| match event {
            ServerEvent::SwarmStatus { members } => Some(members[0].runtime.clone()),
            _ => None,
        })
        .last()
        .unwrap();
    assert!(
        latest
            .wait_reason
            .as_deref()
            .unwrap()
            .contains("Required data is permanently unavailable")
    );
    assert_eq!(latest.todo_statuses, vec!["completely_blocked"]);
    todos.todos.push(crate::todo::TodoItem {
        id: "independent".into(),
        content: "Run tests".into(),
        status: "in_progress".into(),
        ..Default::default()
    });
    super::super::background_tasks::dispatch_swarm_todo_progress(&todos, &members, &swarms).await;
    update_member_status(sid, "running", None, &members, &swarms, None, None, None).await;
    super::broadcast_swarm_status_now(vec![sid.into()], &members).await;
    let latest = std::iter::from_fn(|| rx.try_recv().ok())
        .filter_map(|event| match event {
            ServerEvent::SwarmStatus { members } => Some(members[0].runtime.clone()),
            _ => None,
        })
        .last()
        .unwrap();
    assert!(latest.wait_reason.is_none());
    assert_eq!(latest.live_status.unwrap().phase, "sending");
    super::super::live_status::forget(sid);
}

fn member_with_parent(session_id: &str, parent: Option<&str>) -> SwarmMember {
    let (mut member, _rx) = swarm_member(session_id, "agent", false);
    member.report_back_to_session_id = parent.map(str::to_string);
    member
}

#[test]
fn idle_spawned_worker_reap_selects_only_finished_idle_spawned_agents() {
    use super::idle_spawned_worker_reap_candidates;

    let idle_after = Duration::from_secs(60);
    let old = Instant::now() - Duration::from_secs(120);

    // Finished spawned worker, idle past the window: reapable.
    let mut reapable = member_with_parent("reapable", Some("coord"));
    reapable.status = "ready".to_string();
    reapable.last_status_change = old;

    // Terminal-status spawned worker: reapable.
    let mut stopped = member_with_parent("stopped", Some("coord"));
    stopped.status = "completed".to_string();
    stopped.last_status_change = old;

    // Same shape but user-created (no spawner): never reaped.
    let mut user_owned = member_with_parent("user-owned", None);
    user_owned.status = "ready".to_string();
    user_owned.last_status_change = old;

    // Spawned but still running: not reaped.
    let mut running = member_with_parent("running", Some("coord"));
    running.status = "running".to_string();
    running.last_status_change = old;

    // Spawned and finished, but recently: not reaped yet.
    let mut fresh = member_with_parent("fresh", Some("coord"));
    fresh.status = "ready".to_string();

    // Spawned coordinator (sub-swarm manager): never reaped by role.
    let mut sub_coordinator = member_with_parent("sub-coord", Some("coord"));
    sub_coordinator.role = "coordinator".to_string();
    sub_coordinator.status = "ready".to_string();
    sub_coordinator.last_status_change = old;

    let members: HashMap<String, SwarmMember> = [
        reapable,
        stopped,
        user_owned,
        running,
        fresh,
        sub_coordinator,
    ]
    .into_iter()
    .map(|member| (member.session_id.clone(), member))
    .collect();

    let mut candidates = idle_spawned_worker_reap_candidates(&members, idle_after);
    candidates.sort();
    assert_eq!(
        candidates,
        vec!["reapable".to_string(), "stopped".to_string()]
    );
}

#[test]
fn idle_worker_reap_window_env_zero_disables() {
    // Note: mutating the process env in tests is racy in general, but this
    // env var is read on every call (not cached), and no other test touches
    // it.
    unsafe {
        std::env::set_var("JCODE_SWARM_IDLE_WORKER_REAP_SECS", "0");
    }
    assert_eq!(super::swarm_idle_worker_reap_after(), None);
    unsafe {
        std::env::set_var("JCODE_SWARM_IDLE_WORKER_REAP_SECS", "90");
    }
    assert_eq!(
        super::swarm_idle_worker_reap_after(),
        Some(Duration::from_secs(90))
    );
    unsafe {
        std::env::remove_var("JCODE_SWARM_IDLE_WORKER_REAP_SECS");
    }
    assert!(super::swarm_idle_worker_reap_after().is_some());
}

#[test]
fn status_broadcast_keeps_live_and_recently_terminal_members_only() {
    let retention = Duration::from_secs(900);

    let (live, _rx) = swarm_member("live", "agent", false);
    assert!(member_in_status_broadcast(&live, retention));

    let (mut fresh_terminal, _rx) = swarm_member("fresh", "agent", false);
    fresh_terminal.status = "completed".to_string();
    assert!(member_in_status_broadcast(&fresh_terminal, retention));

    let (mut stale_terminal, _rx) = swarm_member("stale", "agent", false);
    stale_terminal.status = "stopped".to_string();
    stale_terminal.last_status_change = Instant::now() - Duration::from_secs(901);
    assert!(!member_in_status_broadcast(&stale_terminal, retention));

    // A stale *live* status is never filtered, no matter how old.
    let (mut old_live, _rx) = swarm_member("old-live", "agent", false);
    old_live.last_status_change = Instant::now() - Duration::from_secs(100_000);
    assert!(member_in_status_broadcast(&old_live, retention));
}

#[test]
fn swarm_depth_and_ancestry_follow_report_back_chain() {
    let mut members: HashMap<String, SwarmMember> = HashMap::new();
    for (id, parent) in [
        ("root", None),
        ("a", Some("root")),
        ("b", Some("a")),
        ("c", Some("b")),
    ] {
        members.insert(id.to_string(), member_with_parent(id, parent));
    }

    assert_eq!(swarm_spawn_depth(&members, "root"), 0);
    assert_eq!(swarm_spawn_depth(&members, "a"), 1);
    assert_eq!(swarm_spawn_depth(&members, "c"), 3);
    assert_eq!(swarm_ancestors(&members, "c"), vec!["b", "a", "root"]);

    // Ownership: an ancestor (or self) owns the subtree.
    assert!(swarm_is_self_or_ancestor(&members, "a", "c"));
    assert!(swarm_is_self_or_ancestor(&members, "root", "c"));
    assert!(swarm_is_self_or_ancestor(&members, "c", "c"));
    // A sibling/descendant is not an ancestor.
    assert!(!swarm_is_self_or_ancestor(&members, "c", "a"));
    assert!(!swarm_is_self_or_ancestor(&members, "b", "a"));
}

#[test]
fn swarm_ancestry_guards_against_cycles() {
    let mut members: HashMap<String, SwarmMember> = HashMap::new();
    // x -> y -> x is a (pathological) cycle; depth must terminate.
    members.insert("x".to_string(), member_with_parent("x", Some("y")));
    members.insert("y".to_string(), member_with_parent("y", Some("x")));
    assert_eq!(swarm_spawn_depth(&members, "x"), 1);
    assert_eq!(swarm_ancestors(&members, "x"), vec!["y"]);
}

#[tokio::test]
async fn broadcast_swarm_plan_with_previous_includes_newly_ready_ids() {
    let swarm_plans = Arc::new(RwLock::new(HashMap::from([(
        "swarm-1".to_string(),
        VersionedPlan {
            items: vec![
                PlanItem {
                    content: "setup".to_string(),
                    status: "completed".to_string(),
                    priority: "high".to_string(),
                    id: "setup".to_string(),
                    subsystem: None,
                    file_scope: Vec::new(),
                    blocked_by: Vec::new(),
                    assigned_to: None,
                },
                PlanItem {
                    content: "follow-up".to_string(),
                    status: "queued".to_string(),
                    priority: "high".to_string(),
                    id: "follow-up".to_string(),
                    subsystem: None,
                    file_scope: Vec::new(),
                    blocked_by: vec!["setup".to_string()],
                    assigned_to: None,
                },
            ],
            version: 2,
            participants: HashSet::from(["worker".to_string()]),
            task_progress: HashMap::new(),
        },
    )])));
    let (worker, mut worker_rx) = swarm_member("worker", "agent", false);
    let swarm_members = Arc::new(RwLock::new(HashMap::from([("worker".to_string(), worker)])));
    let swarms_by_id = Arc::new(RwLock::new(HashMap::from([(
        "swarm-1".to_string(),
        HashSet::from(["worker".to_string()]),
    )])));
    let previous_items = vec![
        PlanItem {
            content: "setup".to_string(),
            status: "running".to_string(),
            priority: "high".to_string(),
            id: "setup".to_string(),
            subsystem: None,
            file_scope: Vec::new(),
            blocked_by: Vec::new(),
            assigned_to: Some("worker".to_string()),
        },
        PlanItem {
            content: "follow-up".to_string(),
            status: "queued".to_string(),
            priority: "high".to_string(),
            id: "follow-up".to_string(),
            subsystem: None,
            file_scope: Vec::new(),
            blocked_by: vec!["setup".to_string()],
            assigned_to: None,
        },
    ];

    broadcast_swarm_plan_with_previous(
        "swarm-1",
        Some("task_completed".to_string()),
        Some(&previous_items),
        &swarm_plans,
        &swarm_members,
        &swarms_by_id,
    )
    .await;

    match worker_rx.recv().await.expect("swarm plan event") {
        ServerEvent::SwarmPlan {
            reason,
            summary: Some(summary),
            ..
        } => {
            assert_eq!(reason.as_deref(), Some("task_completed"));
            assert_eq!(summary.newly_ready_ids, vec!["follow-up".to_string()]);
            assert_eq!(summary.next_ready_ids, vec!["follow-up".to_string()]);
        }
        other => panic!("expected SwarmPlan event, got {other:?}"),
    }
}

/// Deterministic demonstration of the mutate->broadcast version-inversion
/// race (wiring-audit.plan-broadcast-ordering).
///
/// `broadcast_swarm_plan_with_previous` snapshots `(version, items)` under
/// `swarm_plans.read()`, releases the lock, and only later (after further
/// await points on `swarms_by_id.read()` / `swarm_members.read()`) sends
/// on `member.event_tx`. A second mutator can bump the version AND
/// complete its own broadcast inside that window, so a single ordered
/// mpsc channel can deliver v6 before v5.
///
/// This test parks broadcast A (snapshot v5, empty participants, so it
/// must await `swarms_by_id.read()`) behind a held `swarms_by_id.write()`
/// guard, lets mutator B bump to v6 and broadcast it, then releases A.
/// The worker receives [6, 5]: inverted versions on one channel.
///
/// If this test starts failing with versions == [6, 6] or [5, 6], the
/// race has been fixed (e.g. by holding the plan lock through send or by
/// stamping a send-order sequence); update the wiring audit and consider
/// whether the TUI-side monotonicity guard (server_events.rs SwarmPlan
/// handler currently overwrites `swarm_plan_version` unconditionally) is
/// still needed.
#[tokio::test]
async fn swarm_plan_broadcast_versions_can_invert_on_one_member_channel() {
    let swarm_plans = Arc::new(RwLock::new(HashMap::from([(
        "swarm-1".to_string(),
        VersionedPlan {
            items: vec![plan_item("t1", "task one")],
            version: 5,
            // Empty participants: broadcast A takes the swarms_by_id
            // fallback path, which is where we deterministically park it.
            participants: HashSet::new(),
            task_progress: HashMap::new(),
        },
    )])));
    let (worker, mut worker_rx) = swarm_member("worker", "agent", false);
    let swarm_members = Arc::new(RwLock::new(HashMap::from([("worker".to_string(), worker)])));
    let swarms_by_id = Arc::new(RwLock::new(HashMap::from([(
        "swarm-1".to_string(),
        HashSet::from(["worker".to_string()]),
    )])));

    // Hold a write guard on swarms_by_id so broadcast A parks after it
    // has already snapshotted version 5 from swarm_plans.
    let gate = swarms_by_id.write().await;

    let a = tokio::spawn({
        let swarm_plans = Arc::clone(&swarm_plans);
        let swarm_members = Arc::clone(&swarm_members);
        let swarms_by_id = Arc::clone(&swarms_by_id);
        async move {
            broadcast_swarm_plan(
                "swarm-1",
                Some("mutator_1".to_string()),
                &swarm_plans,
                &swarm_members,
                &swarms_by_id,
            )
            .await;
        }
    });
    // Current-thread test runtime: yielding runs A until it parks on the
    // contended swarms_by_id.read().await, past its v5 snapshot.
    for _ in 0..16 {
        tokio::task::yield_now().await;
    }

    // Mutator B: bump to v6 and register an explicit participant so B's
    // broadcast skips the swarms_by_id fallback and is not blocked by
    // the gate. This mirrors real mutators (write, release, broadcast).
    {
        let mut plans = swarm_plans.write().await;
        let vp = plans.get_mut("swarm-1").expect("plan");
        vp.version = 6;
        vp.participants.insert("worker".to_string());
    }
    broadcast_swarm_plan(
        "swarm-1",
        Some("mutator_2".to_string()),
        &swarm_plans,
        &swarm_members,
        &swarms_by_id,
    )
    .await;

    // Release A: it resumes with its stale v5 snapshot and sends it
    // after v6 on the same ordered channel.
    drop(gate);
    a.await.expect("broadcast task");

    let mut versions = Vec::new();
    while let Ok(event) = worker_rx.try_recv() {
        if let ServerEvent::SwarmPlan { version, .. } = event {
            versions.push(version);
        }
    }
    assert_eq!(
        versions,
        vec![6, 5],
        "expected version inversion on one member channel; if this fails \
         the mutate->broadcast race may have been fixed (update the \
         wiring audit)"
    );
}

/// Drain the cooperative budget at the old snapshot/send race boundary.
/// Publication now holds one member guard through synchronous queue delivery,
/// so a concurrent update cannot put an older snapshot behind the new one.
#[tokio::test]
async fn swarm_status_immediate_broadcasts_preserve_snapshot_order() {
    let (worker, mut worker_rx) = swarm_member("worker", "agent", false);
    let swarm_members = Arc::new(RwLock::new(HashMap::from([("worker".to_string(), worker)])));
    let swarms_by_id = Arc::new(RwLock::new(HashMap::from([(
        "swarm-1".to_string(),
        HashSet::from(["worker".to_string()]),
    )])));

    // Broadcast A must enqueue its snapshot before releasing the member lock.
    let a = tokio::spawn({
        let swarm_members = Arc::clone(&swarm_members);
        let swarms_by_id = Arc::clone(&swarms_by_id);
        async move {
            // Leave two acquisitions: the session list and atomic publication.
            for _ in 0..126 {
                tokio::task::coop::consume_budget().await;
            }
            broadcast_swarm_status("swarm-1", &swarm_members, &swarms_by_id).await;
        }
    });
    // Let A run at the old race boundary before changing the member.
    tokio::task::yield_now().await;

    // A concurrent mutation is serialized with snapshot publication.
    {
        let mut members = swarm_members.write().await;
        members.get_mut("worker").expect("worker member").status = "running".to_string();
    }
    broadcast_swarm_status("swarm-1", &swarm_members, &swarms_by_id).await;

    // Both publications must leave the consumer showing the newest status.
    a.await.expect("broadcast task");

    let mut statuses = Vec::new();
    while let Ok(event) = worker_rx.try_recv() {
        if let ServerEvent::SwarmStatus { members } = event {
            assert_eq!(members.len(), 1);
            assert_eq!(members[0].session_id, "worker");
            statuses.push(members[0].status.clone());
        }
    }
    assert_eq!(
        statuses,
        vec!["ready".to_string(), "running".to_string()],
        "snapshot publication must not invert newer and older statuses"
    );
}

/// Restored (persisted) plan participants with dead channels starve live
/// swarm members of plan broadcasts: the fallback to swarms_by_id only
/// triggers when `participants` is EMPTY, so a participant set that only
/// contains stale sessions (e.g. restored after a server restart, where
/// `from_persisted_member` gives every member a closed event_tx) means
/// nobody receives the snapshot, not even live members of the swarm.
#[tokio::test]
async fn stale_participants_starve_live_members_of_plan_broadcasts() {
    let swarm_plans = Arc::new(RwLock::new(HashMap::from([(
        "swarm-1".to_string(),
        VersionedPlan {
            items: vec![plan_item("t1", "task one")],
            version: 7,
            // "ghost" is a participant restored from disk whose session
            // no longer exists in this server process.
            participants: HashSet::from(["ghost".to_string()]),
            task_progress: HashMap::new(),
        },
    )])));
    // Ghost member as produced by swarm_persistence restore: present in
    // the member map but with a closed event channel.
    let (ghost, ghost_rx) = swarm_member("ghost", "agent", true);
    drop(ghost_rx);
    let (live, mut live_rx) = swarm_member("live", "agent", false);
    let swarm_members = Arc::new(RwLock::new(HashMap::from([
        ("ghost".to_string(), ghost),
        ("live".to_string(), live),
    ])));
    let swarms_by_id = Arc::new(RwLock::new(HashMap::from([(
        "swarm-1".to_string(),
        HashSet::from(["ghost".to_string(), "live".to_string()]),
    )])));

    broadcast_swarm_plan(
        "swarm-1",
        Some("test".to_string()),
        &swarm_plans,
        &swarm_members,
        &swarms_by_id,
    )
    .await;

    assert!(
        live_rx.try_recv().is_err(),
        "live member unexpectedly received the plan broadcast; stale \
         participant starvation may have been fixed (update the wiring \
         audit)"
    );
}

#[tokio::test]
async fn remove_session_from_swarm_reassigns_to_non_headless_member() {
    let swarm_members = Arc::new(RwLock::new(HashMap::new()));
    let swarms_by_id = Arc::new(RwLock::new(HashMap::from([(
        "swarm-1".to_string(),
        HashSet::from([
            "coord".to_string(),
            "headless".to_string(),
            "worker".to_string(),
        ]),
    )])));
    let swarm_coordinators = Arc::new(RwLock::new(HashMap::from([(
        "swarm-1".to_string(),
        "coord".to_string(),
    )])));
    let swarm_plans = Arc::new(RwLock::new(HashMap::from([(
        "swarm-1".to_string(),
        VersionedPlan {
            items: vec![PlanItem {
                content: "task".to_string(),
                status: "pending".to_string(),
                priority: "medium".to_string(),
                id: "1".to_string(),
                subsystem: None,
                file_scope: Vec::new(),
                blocked_by: Vec::new(),
                assigned_to: Some("coord".to_string()),
            }],
            version: 1,
            participants: HashSet::from(["coord".to_string()]),
            task_progress: HashMap::new(),
        },
    )])));

    let (coord, _coord_rx) = swarm_member("coord", "coordinator", false);
    let (headless, mut headless_rx) = swarm_member("headless", "agent", true);
    let (worker, mut worker_rx) = swarm_member("worker", "agent", false);
    {
        let mut members = swarm_members.write().await;
        members.insert("coord".to_string(), coord);
        members.insert("headless".to_string(), headless);
        members.insert("worker".to_string(), worker);
        members.remove("coord");
    }

    remove_session_from_swarm(
        "coord",
        "swarm-1",
        &swarm_members,
        &swarms_by_id,
        &swarm_coordinators,
        &swarm_plans,
    )
    .await;

    assert_eq!(
        swarm_coordinators
            .read()
            .await
            .get("swarm-1")
            .map(String::as_str),
        Some("worker")
    );
    assert!(
        swarm_plans
            .read()
            .await
            .get("swarm-1")
            .is_some_and(|plan| plan.participants.contains("worker"))
    );
    assert_eq!(
        swarm_members
            .read()
            .await
            .get("worker")
            .map(|member| member.role.as_str()),
        Some("coordinator")
    );
    assert_eq!(
        swarm_members
            .read()
            .await
            .get("headless")
            .map(|member| member.role.as_str()),
        Some("agent")
    );

    let headless_events: Vec<_> = std::iter::from_fn(|| headless_rx.try_recv().ok()).collect();
    assert!(headless_events.iter().all(|event| {
        !matches!(
            event,
            ServerEvent::Notification {
                notification_type: NotificationType::Message { .. },
                message,
                ..
            } if message == "You are now the coordinator for this swarm."
        )
    }));

    let worker_events: Vec<_> = std::iter::from_fn(|| worker_rx.try_recv().ok()).collect();
    assert!(worker_events.iter().any(|event| {
        matches!(
            event,
            ServerEvent::Notification {
                notification_type: NotificationType::Message { .. },
                message,
                ..
            } if message == "You are now the coordinator for this swarm."
        )
    }));
}

#[tokio::test]
async fn remove_session_reparents_children_to_live_grandparent() {
    let swarm_members = Arc::new(RwLock::new(HashMap::new()));
    let swarms_by_id = Arc::new(RwLock::new(HashMap::from([(
        "swarm-1".to_string(),
        HashSet::from(["root".to_string(), "mid".to_string(), "leaf".to_string()]),
    )])));
    let swarm_coordinators = Arc::new(RwLock::new(HashMap::from([(
        "swarm-1".to_string(),
        "root".to_string(),
    )])));
    let swarm_plans = Arc::new(RwLock::new(HashMap::new()));

    let (root, _root_rx) = swarm_member("root", "coordinator", false);
    let (mut mid, _mid_rx) = swarm_member("mid", "agent", true);
    mid.report_back_to_session_id = Some("root".to_string());
    let (mut leaf, _leaf_rx) = swarm_member("leaf", "agent", true);
    leaf.report_back_to_session_id = Some("mid".to_string());
    {
        let mut members = swarm_members.write().await;
        members.insert("root".to_string(), root);
        members.insert("mid".to_string(), mid);
        members.insert("leaf".to_string(), leaf);
    }

    remove_session_from_swarm(
        "mid",
        "swarm-1",
        &swarm_members,
        &swarms_by_id,
        &swarm_coordinators,
        &swarm_plans,
    )
    .await;

    // Leaf follows the report-back chain up to its grandparent instead of
    // dangling on the removed session.
    let members = swarm_members.read().await;
    assert_eq!(
        members
            .get("leaf")
            .and_then(|member| member.report_back_to_session_id.as_deref()),
        Some("root")
    );
    assert!(swarm_is_self_or_ancestor(&members, "root", "leaf"));
}

#[tokio::test]
async fn remove_session_reparents_children_to_coordinator_when_no_grandparent() {
    let swarm_members = Arc::new(RwLock::new(HashMap::new()));
    let swarms_by_id = Arc::new(RwLock::new(HashMap::from([(
        "swarm-1".to_string(),
        HashSet::from([
            "coord".to_string(),
            "peer_root".to_string(),
            "child".to_string(),
        ]),
    )])));
    let swarm_coordinators = Arc::new(RwLock::new(HashMap::from([(
        "swarm-1".to_string(),
        "coord".to_string(),
    )])));
    let swarm_plans = Arc::new(RwLock::new(HashMap::new()));

    // peer_root is itself a root (no parent), so its children have no
    // grandparent to inherit; they should fall back to the coordinator.
    let (coord, _coord_rx) = swarm_member("coord", "coordinator", false);
    let (peer_root, _peer_rx) = swarm_member("peer_root", "agent", false);
    let (mut child, _child_rx) = swarm_member("child", "agent", true);
    child.report_back_to_session_id = Some("peer_root".to_string());
    {
        let mut members = swarm_members.write().await;
        members.insert("coord".to_string(), coord);
        members.insert("peer_root".to_string(), peer_root);
        members.insert("child".to_string(), child);
    }

    remove_session_from_swarm(
        "peer_root",
        "swarm-1",
        &swarm_members,
        &swarms_by_id,
        &swarm_coordinators,
        &swarm_plans,
    )
    .await;

    let members = swarm_members.read().await;
    assert_eq!(
        members
            .get("child")
            .and_then(|member| member.report_back_to_session_id.as_deref()),
        Some("coord")
    );
}

#[tokio::test]
async fn departing_sole_owner_keeps_its_workers_owned() {
    // A TUI owner briefly drops off during a server reload. With no live
    // fallback parent, its workers must keep pointing at it so it can still
    // resume them, and none of them may inherit the coordinator slot.
    let swarm_members = Arc::new(RwLock::new(HashMap::new()));
    let swarms_by_id = Arc::new(RwLock::new(HashMap::from([(
        "swarm-1".to_string(),
        HashSet::from(["owner".to_string(), "worker".to_string()]),
    )])));
    let swarm_coordinators = Arc::new(RwLock::new(HashMap::from([(
        "swarm-1".to_string(),
        "owner".to_string(),
    )])));
    let swarm_plans = Arc::new(RwLock::new(HashMap::new()));
    let (owner, _owner_rx) = swarm_member("owner", "coordinator", false);
    let (mut worker, _worker_rx) = swarm_member("worker", "agent", true);
    worker.report_back_to_session_id = Some("owner".to_string());
    {
        let mut members = swarm_members.write().await;
        members.insert("owner".to_string(), owner);
        members.insert("worker".to_string(), worker);
    }

    remove_session_from_swarm(
        "owner",
        "swarm-1",
        &swarm_members,
        &swarms_by_id,
        &swarm_coordinators,
        &swarm_plans,
    )
    .await;

    let members = swarm_members.read().await;
    assert_eq!(
        members
            .get("worker")
            .and_then(|member| member.report_back_to_session_id.as_deref()),
        Some("owner")
    );
    assert!(swarm_coordinators.read().await.get("swarm-1").is_none());
}

#[path = "swarm_owner_tests.rs"]
mod owner_notifications;
