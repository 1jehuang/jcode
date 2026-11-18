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
        added: 0,
        removed: 0,
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

use super::file_activity::{
    FILE_ACTIVITY_NOTICE_MAX_ENTRIES, FileActivityEntry, file_activity_display_path,
    format_file_activity_notice, merge_file_activity_entry,
};

fn entry(agent: &str, path: &str, added: u64, removed: u64) -> FileActivityEntry {
    FileActivityEntry {
        agent: agent.to_string(),
        operation: "edited".to_string(),
        previously: false,
        path: path.to_string(),
        added,
        removed,
        edits: 1,
        lines: None,
        overlap: false,
        intent: None,
    }
}

#[test]
fn notice_is_one_short_line_with_agent_path_counts_and_intent() {
    let mut single = entry("moss", "src/parser.rs", 3, 1);
    single.lines = Some((45, 60));
    single.overlap = true;
    single.intent = Some("  fix the\n parser  ".to_string());
    let notice = format_file_activity_notice(&[single]);
    assert_eq!(
        notice,
        "moss edited src/parser.rs +3-1 L45-60 overlap: fix the parser"
    );
    // The diff preview is never part of the model-facing notice.
    assert!(!notice.contains('\n'));

    let mut bare = entry("fox", "src/lib.rs", 10, 0);
    bare.previously = true;
    bare.operation = "wrote".to_string();
    assert_eq!(
        format_file_activity_notice(&[bare]),
        "fox wrote src/lib.rs +10-0 earlier"
    );
}

#[test]
fn long_intents_are_truncated() {
    let mut long = entry("moss", "a.rs", 1, 1);
    long.intent = Some("x".repeat(500));
    let notice = format_file_activity_notice(&[long]);
    assert!(notice.ends_with('…'));
    assert!(notice.chars().count() < 100, "{}", notice.chars().count());
}

#[test]
fn repeated_edits_by_one_agent_to_one_file_merge_into_one_entry() {
    let mut entries = Vec::new();
    let mut first = entry("moss", "src/a.rs", 3, 1);
    first.lines = Some((10, 20));
    first.intent = Some("first".to_string());
    let mut second = entry("moss", "src/a.rs", 2, 2);
    second.lines = Some((40, 45));
    second.overlap = true;
    second.intent = Some("second".to_string());
    merge_file_activity_entry(&mut entries, first);
    merge_file_activity_entry(&mut entries, second);
    merge_file_activity_entry(&mut entries, entry("fox", "src/a.rs", 1, 0));
    merge_file_activity_entry(&mut entries, entry("moss", "src/b.rs", 7, 0));

    assert_eq!(entries.len(), 3);
    assert!(entries.len() <= FILE_ACTIVITY_NOTICE_MAX_ENTRIES);
    assert_eq!(
        format_file_activity_notice(&entries),
        "moss edited src/a.rs +5-3 x2 L10-45 overlap: second\n\
         fox edited src/a.rs +1-0\n\
         moss edited src/b.rs +7-0"
    );
}

#[test]
fn display_path_is_relative_to_the_recipient_working_dir() {
    use std::path::Path;
    let path = Path::new("/home/me/repo/src/lib.rs");
    assert_eq!(
        file_activity_display_path(path, Some(Path::new("/home/me/repo"))),
        "src/lib.rs"
    );
    assert_eq!(
        file_activity_display_path(path, Some(Path::new("/elsewhere"))),
        "/home/me/repo/src/lib.rs"
    );
    assert_eq!(
        file_activity_display_path(path, None),
        "/home/me/repo/src/lib.rs"
    );
}
