use super::{FileAccess, latest_peer_touches};
use crate::bus::FileOp;
use std::collections::HashSet;
use std::time::{Duration, Instant, SystemTime};

fn access(session_id: &str, op: FileOp, age_ms: u64) -> FileAccess {
    let now = Instant::now();
    FileAccess {
        session_id: session_id.to_string(),
        op,
        timestamp: now
            .checked_sub(Duration::from_millis(age_ms))
            .unwrap_or(now),
        absolute_time: SystemTime::now(),
        intent: None,
        summary: None,
        detail: None,
    }
}

#[test]
fn latest_peer_touches_excludes_previous_readers_from_modification_alerts() {
    let swarm_session_ids = HashSet::from([
        "current".to_string(),
        "reader".to_string(),
        "writer".to_string(),
    ]);
    let accesses = vec![
        access("reader", FileOp::Read, 20),
        access("current", FileOp::Edit, 10),
        access("writer", FileOp::Write, 5),
    ];

    let latest = latest_peer_touches(&accesses, "current", &swarm_session_ids);

    assert_eq!(latest.len(), 1);
    assert!(!latest.iter().any(|entry| entry.session_id == "reader"));
    assert!(
        latest
            .iter()
            .any(|entry| entry.session_id == "writer" && entry.op == FileOp::Write)
    );
}

#[test]
fn latest_peer_touches_deduplicates_to_most_recent_touch_per_peer() {
    let swarm_session_ids = HashSet::from(["current".to_string(), "peer".to_string()]);
    let accesses = vec![
        access("peer", FileOp::Read, 30),
        access("peer", FileOp::Edit, 5),
        access("current", FileOp::Write, 1),
    ];

    let latest = latest_peer_touches(&accesses, "current", &swarm_session_ids);

    assert_eq!(latest.len(), 1);
    assert_eq!(latest[0].session_id, "peer");
    assert_eq!(latest[0].op, FileOp::Edit);
}
