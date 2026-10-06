//! Interrupting a coordinator stops the workers it spawned.

use super::{cancel_spawned_descendant_turns, swarm_descendants};
use crate::protocol::ServerEvent;
use crate::server::SwarmMember;
use jcode_agent_runtime::InterruptSignal;
use std::collections::HashMap;
use std::sync::Arc;
use std::time::Instant;
use tokio::sync::{RwLock, mpsc};

fn member(
    session_id: &str,
    parent: Option<&str>,
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
            status: "running".to_string(),
            detail: None,
            task_label: None,
            friendly_name: Some(session_id.to_string()),
            report_back_to_session_id: parent.map(str::to_string),
            latest_completion_report: None,
            role: "agent".to_string(),
            joined_at: Instant::now(),
            last_status_change: Instant::now(),
            is_headless: true,
            output_tail: None,
            todo_progress: None,
            todo_items: Vec::new(),
            runtime: crate::protocol::SwarmMemberRuntime::default(),
        },
        event_rx,
    )
}

#[test]
fn swarm_descendants_follow_the_spawn_tree_only() {
    let mut members: HashMap<String, SwarmMember> = HashMap::new();
    for (id, parent) in [
        ("root", None),
        ("a", Some("root")),
        ("b", Some("a")),
        ("c", Some("root")),
        ("other-root", None),
        ("other-child", Some("other-root")),
    ] {
        members.insert(id.to_string(), member(id, parent).0);
    }
    assert_eq!(swarm_descendants(&members, "root"), vec!["a", "b", "c"]);
    assert_eq!(swarm_descendants(&members, "a"), vec!["b"]);
    assert!(swarm_descendants(&members, "b").is_empty());
    // A cycle must not loop or count the session as its own descendant.
    members.insert("x".to_string(), member("x", Some("y")).0);
    members.insert("y".to_string(), member("y", Some("x")).0);
    assert_eq!(swarm_descendants(&members, "x"), vec!["y"]);
}

/// Cancelling a coordinator must stop the running turns of every worker it
/// spawned (and theirs), and leave idle workers and unrelated sessions alone.
#[tokio::test]
async fn cancelling_a_coordinator_stops_its_running_workers() {
    let tag = format!("cascade-{}", std::process::id());
    let id = |name: &str| format!("{tag}-{name}");
    let mut members: HashMap<String, SwarmMember> = HashMap::new();
    let mut worker_rx = None;
    for (name, parent) in [
        ("coord", None),
        ("worker", Some("coord")),
        ("grandchild", Some("worker")),
        ("idle", Some("coord")),
        ("stranger", None),
    ] {
        let parent = parent.map(id);
        let (member, rx) = member(&id(name), parent.as_deref());
        if name == "worker" {
            worker_rx = Some(rx);
        }
        members.insert(id(name), member);
    }
    let swarm_members = Arc::new(RwLock::new(members));
    let swarms_by_id = Arc::new(RwLock::new(HashMap::new()));

    let worker_signal = InterruptSignal::new();
    let grandchild_signal = InterruptSignal::new();
    let stranger_signal = InterruptSignal::new();
    let _worker_turn =
        crate::turn_cancel_registry::register_active_turn(&id("worker"), worker_signal.clone());
    let _grandchild_turn = crate::turn_cancel_registry::register_active_turn(
        &id("grandchild"),
        grandchild_signal.clone(),
    );
    let _stranger_turn =
        crate::turn_cancel_registry::register_active_turn(&id("stranger"), stranger_signal.clone());

    let stopped = cancel_spawned_descendant_turns(
        &id("coord"),
        &swarm_members,
        &swarms_by_id,
        None,
        None,
        None,
    )
    .await;

    assert_eq!(stopped, vec![id("grandchild"), id("worker")]);
    assert!(worker_signal.is_set(), "spawned worker turn must stop");
    assert!(grandchild_signal.is_set(), "nested worker turn must stop");
    assert!(!stranger_signal.is_set(), "unrelated session untouched");
    let members = swarm_members.read().await;
    assert_eq!(members[&id("worker")].status, "stopped");
    assert_eq!(members[&id("grandchild")].status, "stopped");
    assert_eq!(members[&id("idle")].status, "running", "no turn, no change");
    assert_eq!(members[&id("stranger")].status, "running");
    drop(members);
    let mut worker_rx = worker_rx.expect("worker receiver");
    let mut events = Vec::new();
    while let Ok(event) = worker_rx.try_recv() {
        events.push(event);
    }
    assert!(
        events.iter().any(|event| matches!(
            event,
            ServerEvent::TurnStopped {
                reason: crate::protocol::TurnStopReason::Interrupted,
                ..
            }
        )),
        "an attached worker client is told its turn stopped: {events:?}"
    );
}
