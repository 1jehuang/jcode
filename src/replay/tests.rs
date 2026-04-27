use super::*;
use crate::plan::PlanItem;
use crate::protocol::SwarmMemberStatus;
use crate::session::{StoredReplayEvent, StoredReplayEventKind};
use chrono::{Duration, Utc};
use std::ffi::OsString;
use std::sync::{Mutex, OnceLock};

static ENV_LOCK: OnceLock<Mutex<()>> = OnceLock::new();

fn lock_env() -> std::sync::MutexGuard<'static, ()> {
    let mutex = ENV_LOCK.get_or_init(|| Mutex::new(()));
    match mutex.lock() {
        Ok(guard) => guard,
        Err(poisoned) => poisoned.into_inner(),
    }
}

struct EnvVarGuard {
    key: &'static str,
    prev: Option<OsString>,
}

impl EnvVarGuard {
    fn set(key: &'static str, value: impl AsRef<std::ffi::OsStr>) -> Self {
        let prev = std::env::var_os(key);
        crate::env::set_var(key, value);
        Self { key, prev }
    }
}

impl Drop for EnvVarGuard {
    fn drop(&mut self) {
        if let Some(prev) = &self.prev {
            crate::env::set_var(self.key, prev);
        } else {
            crate::env::remove_var(self.key);
        }
    }
}

#[test]
fn test_timeline_roundtrip() {
    let events = vec![
        TimelineEvent {
            t: 0,
            kind: TimelineEventKind::UserMessage {
                text: "hello".to_string(),
            },
        },
        TimelineEvent {
            t: 500,
            kind: TimelineEventKind::Thinking { duration: 1000 },
        },
        TimelineEvent {
            t: 1500,
            kind: TimelineEventKind::StreamText {
                text: "Hi there!".to_string(),
                speed: 80,
            },
        },
        TimelineEvent {
            t: 2000,
            kind: TimelineEventKind::Done,
        },
    ];

    // Serialize to JSON
    let json = serde_json::to_string_pretty(&events).unwrap();
    assert!(json.contains("user_message"));
    assert!(json.contains("stream_text"));

    // Deserialize back
    let parsed: Vec<TimelineEvent> = serde_json::from_str(&json).unwrap();
    assert_eq!(parsed.len(), 4);
    assert_eq!(parsed[0].t, 0);
    assert_eq!(parsed[2].t, 1500);
}

#[test]
fn test_timeline_to_replay_events() {
    let events = vec![
        TimelineEvent {
            t: 0,
            kind: TimelineEventKind::StreamText {
                text: "Hello world".to_string(),
                speed: 80,
            },
        },
        TimelineEvent {
            t: 500,
            kind: TimelineEventKind::Done,
        },
    ];

    let replay_events = timeline_to_replay_events(&events);
    assert!(!replay_events.is_empty());

    // First event should be a Server(TextDelta)
    match &replay_events[0].1 {
        ReplayEvent::Server(ServerEvent::TextDelta { text }) => assert!(!text.is_empty()),
        _ => panic!("Expected Server(TextDelta)"),
    }

    // Last event should be Server(Done)
    match &replay_events.last().unwrap().1 {
        ReplayEvent::Server(ServerEvent::Done { .. }) => {}
        _ => panic!("Expected Server(Done)"),
    }
}

#[test]
fn test_timeline_to_replay_events_caps_initial_idle() {
    let events = vec![
        TimelineEvent {
            t: 8_000,
            kind: TimelineEventKind::UserMessage {
                text: "hello".to_string(),
            },
        },
        TimelineEvent {
            t: 8_500,
            kind: TimelineEventKind::Thinking { duration: 800 },
        },
    ];

    let replay_events = timeline_to_replay_events(&events);
    assert_eq!(replay_events[0].0, 0);
    assert!(matches!(
        replay_events[0].1,
        ReplayEvent::UserMessage { .. }
    ));
}

#[test]
fn test_cap_initial_replay_idle_shifts_timeline_start() {
    let mut events = vec![
        TimelineEvent {
            t: 8_000,
            kind: TimelineEventKind::UserMessage {
                text: "hello".to_string(),
            },
        },
        TimelineEvent {
            t: 8_750,
            kind: TimelineEventKind::Thinking { duration: 800 },
        },
    ];

    cap_initial_replay_idle(&mut events);
    assert_eq!(events[0].t, 0);
    assert_eq!(events[1].t, 750);
}

#[test]
fn test_tool_events() {
    let events = vec![
        TimelineEvent {
            t: 0,
            kind: TimelineEventKind::ToolStart {
                name: "file_read".to_string(),
                input: serde_json::json!({"file_path": "/tmp/test.rs"}),
            },
        },
        TimelineEvent {
            t: 500,
            kind: TimelineEventKind::ToolDone {
                name: "file_read".to_string(),
                output: "fn main() {}".to_string(),
                is_error: false,
            },
        },
    ];

    let replay_events = timeline_to_replay_events(&events);
    let types: Vec<&str> = replay_events
        .iter()
        .filter_map(|(_, e)| match e {
            ReplayEvent::Server(se) => Some(match se {
                ServerEvent::ToolStart { .. } => "start",
                ServerEvent::ToolInput { .. } => "input",
                ServerEvent::ToolExec { .. } => "exec",
                ServerEvent::ToolDone { .. } => "done",
                _ => "other",
            }),
            _ => None,
        })
        .collect();
    assert!(types.contains(&"start"));
    assert!(types.contains(&"exec"));
    assert!(types.contains(&"done"));
}

#[test]
fn test_user_message_and_thinking() {
    let events = vec![
        TimelineEvent {
            t: 0,
            kind: TimelineEventKind::UserMessage {
                text: "hello".to_string(),
            },
        },
        TimelineEvent {
            t: 500,
            kind: TimelineEventKind::Thinking { duration: 800 },
        },
        TimelineEvent {
            t: 1300,
            kind: TimelineEventKind::StreamText {
                text: "Hi!".to_string(),
                speed: 80,
            },
        },
    ];

    let replay_events = timeline_to_replay_events(&events);

    // First should be UserMessage
    assert!(matches!(
        &replay_events[0].1,
        ReplayEvent::UserMessage { .. }
    ));

    // Second should be StartProcessing
    assert!(matches!(&replay_events[1].1, ReplayEvent::StartProcessing));

    // Third should be Server(TextDelta)
    assert!(matches!(
        &replay_events[2].1,
        ReplayEvent::Server(ServerEvent::TextDelta { .. })
    ));
}

#[test]
fn test_export_timeline_includes_persisted_swarm_replay_events() {
    let base = Utc::now();
    let mut session = Session::create_with_id("session_replay_swarm_test".to_string(), None, None);
    session.created_at = base;
    session.updated_at = base;
    session.replay_events = vec![
        StoredReplayEvent {
            timestamp: base + Duration::milliseconds(100),
            kind: StoredReplayEventKind::DisplayMessage {
                role: "swarm".to_string(),
                title: Some("DM from fox".to_string()),
                content: "Take parser tests".to_string(),
            },
        },
        StoredReplayEvent {
            timestamp: base + Duration::milliseconds(200),
            kind: StoredReplayEventKind::SwarmStatus {
                members: vec![SwarmMemberStatus {
                    session_id: "session_fox".to_string(),
                    friendly_name: Some("fox".to_string()),
                    status: "running".to_string(),
                    detail: Some("parser tests".to_string()),
                    role: Some("agent".to_string()),
                    is_headless: None,
                    live_attachments: None,
                    status_age_secs: None,
                }],
            },
        },
        StoredReplayEvent {
            timestamp: base + Duration::milliseconds(300),
            kind: StoredReplayEventKind::SwarmPlan {
                swarm_id: "swarm_123".to_string(),
                version: 2,
                items: vec![PlanItem {
                    content: "Run parser tests".to_string(),
                    status: "running".to_string(),
                    priority: "high".to_string(),
                    id: "task-1".to_string(),
                    subsystem: None,
                    file_scope: Vec::new(),
                    blocked_by: vec![],
                    assigned_to: Some("session_fox".to_string()),
                }],
                participants: vec!["session_fox".to_string()],
                reason: Some("proposal approved".to_string()),
            },
        },
    ];

    let timeline = export_timeline(&session);
    assert!(timeline.iter().any(|event| matches!(
        &event.kind,
        TimelineEventKind::DisplayMessage { role, title, content }
            if role == "swarm"
                && title.as_deref() == Some("DM from fox")
                && content == "Take parser tests"
    )));
    assert!(timeline.iter().any(|event| matches!(
        &event.kind,
        TimelineEventKind::SwarmStatus { members }
            if members.len() == 1 && members[0].status == "running"
    )));
    assert!(timeline.iter().any(|event| matches!(
        &event.kind,
        TimelineEventKind::SwarmPlan { swarm_id, version, items, .. }
            if swarm_id == "swarm_123" && *version == 2 && items.len() == 1
    )));
}

#[test]
fn test_timeline_to_replay_events_converts_swarm_replay_events() {
    let timeline = vec![
        TimelineEvent {
            t: 100,
            kind: TimelineEventKind::DisplayMessage {
                role: "swarm".to_string(),
                title: Some("Broadcast · oak".to_string()),
                content: "Plan updated".to_string(),
            },
        },
        TimelineEvent {
            t: 200,
            kind: TimelineEventKind::SwarmStatus {
                members: vec![SwarmMemberStatus {
                    session_id: "session_oak".to_string(),
                    friendly_name: Some("oak".to_string()),
                    status: "completed".to_string(),
                    detail: None,
                    role: Some("agent".to_string()),
                    is_headless: None,
                    live_attachments: None,
                    status_age_secs: None,
                }],
            },
        },
        TimelineEvent {
            t: 300,
            kind: TimelineEventKind::SwarmPlan {
                swarm_id: "swarm_abc".to_string(),
                version: 7,
                items: vec![PlanItem {
                    content: "Integrate results".to_string(),
                    status: "pending".to_string(),
                    priority: "high".to_string(),
                    id: "task-7".to_string(),
                    subsystem: None,
                    file_scope: Vec::new(),
                    blocked_by: vec![],
                    assigned_to: None,
                }],
                participants: vec![],
                reason: None,
            },
        },
    ];

    let replay_events = timeline_to_replay_events(&timeline);
    assert!(replay_events.iter().any(|(_, event)| matches!(
        event,
        ReplayEvent::DisplayMessage { role, title, content }
            if role == "swarm"
                && title.as_deref() == Some("Broadcast · oak")
                && content == "Plan updated"
    )));
    assert!(replay_events.iter().any(|(_, event)| matches!(
        event,
        ReplayEvent::SwarmStatus { members }
            if members.len() == 1 && members[0].status == "completed"
    )));
    assert!(replay_events.iter().any(|(_, event)| matches!(
        event,
        ReplayEvent::SwarmPlan { swarm_id, version, items }
            if swarm_id == "swarm_abc" && *version == 7 && items.len() == 1
    )));
}

#[test]
fn test_load_swarm_sessions_discovers_related_sessions() {
    let _env_lock = lock_env();
    let temp_home = tempfile::Builder::new()
        .prefix("jcode-replay-swarm-test-")
        .tempdir()
        .expect("create temp JCODE_HOME");
    let _home = EnvVarGuard::set("JCODE_HOME", temp_home.path().as_os_str());

    let mut seed = Session::create_with_id("session_seed".to_string(), None, None);
    seed.working_dir = Some("/tmp/repo".to_string());
    seed.record_swarm_status_event(vec![SwarmMemberStatus {
        session_id: "session_seed".to_string(),
        friendly_name: Some("seed".to_string()),
        status: "running".to_string(),
        detail: None,
        role: Some("coordinator".to_string()),
        is_headless: None,
        live_attachments: None,
        status_age_secs: None,
    }]);
    seed.save().unwrap();

    let mut child = Session::create_with_id(
        "session_child".to_string(),
        Some(seed.id.clone()),
        Some("child".to_string()),
    );
    child.working_dir = Some("/tmp/repo".to_string());
    child.record_swarm_plan_event(
        "swarm_x".to_string(),
        1,
        vec![PlanItem {
            content: "Task".to_string(),
            status: "running".to_string(),
            priority: "high".to_string(),
            id: "task-1".to_string(),
            subsystem: None,
            file_scope: Vec::new(),
            blocked_by: vec![],
            assigned_to: Some(seed.id.clone()),
        }],
        vec![seed.id.clone(), child.id.clone()],
        None,
    );
    child.save().unwrap();

    let mut unrelated = Session::create_with_id("session_other".to_string(), None, None);
    unrelated.working_dir = Some("/tmp/other".to_string());
    unrelated.save().unwrap();

    let loaded = load_swarm_sessions("session_seed", false).unwrap();
    let ids: Vec<_> = loaded.iter().map(|s| s.session.id.as_str()).collect();
    assert!(ids.contains(&"session_seed"));
    assert!(ids.contains(&"session_child"));
    assert!(!ids.contains(&"session_other"));
}

#[test]
fn test_compose_swarm_buffers_combines_panes() {
    use ratatui::{buffer::Buffer, layout::Rect, style::Style};

    let mut left = Buffer::empty(Rect::new(0, 0, 4, 2));
    left[(0, 0)].set_symbol("L").set_style(Style::default());
    let mut right = Buffer::empty(Rect::new(0, 0, 4, 2));
    right[(0, 0)].set_symbol("R").set_style(Style::default());

    let panes = vec![
        SwarmPaneFrames {
            session_id: "left".to_string(),
            title: "left".to_string(),
            frames: vec![(0.0, left)],
        },
        SwarmPaneFrames {
            session_id: "right".to_string(),
            title: "right".to_string(),
            frames: vec![(0.0, right)],
        },
    ];

    let frames = compose_swarm_buffers(&panes, 8, 2, 1, 2);
    assert!(!frames.is_empty());
    let buf = &frames[0].1;
    assert_eq!(buf[(0, 0)].symbol(), "L");
    assert_eq!(buf[(4, 0)].symbol(), "R");
}

#[test]
fn test_tool_ids_match_between_start_and_done() {
    let events = vec![
        TimelineEvent {
            t: 0,
            kind: TimelineEventKind::ToolStart {
                name: "file_read".to_string(),
                input: serde_json::json!({"file_path": "/tmp/test.rs"}),
            },
        },
        TimelineEvent {
            t: 500,
            kind: TimelineEventKind::ToolDone {
                name: "file_read".to_string(),
                output: "fn main() {}".to_string(),
                is_error: false,
            },
        },
    ];

    let replay_events = timeline_to_replay_events(&events);

    let start_id = replay_events.iter().find_map(|(_, e)| match e {
        ReplayEvent::Server(ServerEvent::ToolStart { id, .. }) => Some(id.clone()),
        _ => None,
    });
    let exec_id = replay_events.iter().find_map(|(_, e)| match e {
        ReplayEvent::Server(ServerEvent::ToolExec { id, .. }) => Some(id.clone()),
        _ => None,
    });
    let done_id = replay_events.iter().find_map(|(_, e)| match e {
        ReplayEvent::Server(ServerEvent::ToolDone { id, .. }) => Some(id.clone()),
        _ => None,
    });

    assert!(start_id.is_some(), "Should have ToolStart");
    assert_eq!(start_id, exec_id, "ToolStart and ToolExec IDs must match");
    assert_eq!(start_id, done_id, "ToolStart and ToolDone IDs must match");
}

#[test]
fn test_batch_tool_input_preserved() {
    let batch_input = serde_json::json!({
        "tool_calls": [
            {"tool": "file_read", "parameters": {"file_path": "/tmp/a.rs"}},
            {"tool": "file_read", "parameters": {"file_path": "/tmp/b.rs"}},
            {"tool": "file_grep", "parameters": {"pattern": "foo"}},
        ]
    });

    let events = vec![
            TimelineEvent {
                t: 0,
                kind: TimelineEventKind::ToolStart {
                    name: "batch".to_string(),
                    input: batch_input.clone(),
                },
            },
            TimelineEvent {
                t: 1000,
                kind: TimelineEventKind::ToolDone {
                    name: "batch".to_string(),
                    output: "--- [1] file_read ---\nok\n--- [2] file_read ---\nok\n--- [3] file_grep ---\nok".to_string(),
                    is_error: false,
                },
            },
        ];

    let replay_events = timeline_to_replay_events(&events);

    // Verify the ToolInput delta contains the batch input
    let input_delta = replay_events.iter().find_map(|(_, e)| match e {
        ReplayEvent::Server(ServerEvent::ToolInput { delta }) => Some(delta.clone()),
        _ => None,
    });
    assert!(
        input_delta.is_some(),
        "Should have ToolInput with batch params"
    );
    let parsed: serde_json::Value = serde_json::from_str(&input_delta.unwrap()).unwrap();
    let tool_calls = parsed.get("tool_calls").and_then(|v| v.as_array());
    assert_eq!(
        tool_calls.map(|a| a.len()),
        Some(3),
        "Batch should have 3 tool calls"
    );

    // Verify IDs match
    let start_id = replay_events.iter().find_map(|(_, e)| match e {
        ReplayEvent::Server(ServerEvent::ToolStart { id, .. }) => Some(id.clone()),
        _ => None,
    });
    let done_id = replay_events.iter().find_map(|(_, e)| match e {
        ReplayEvent::Server(ServerEvent::ToolDone { id, .. }) => Some(id.clone()),
        _ => None,
    });
    assert_eq!(
        start_id, done_id,
        "Batch ToolStart and ToolDone IDs must match"
    );
}

#[test]
fn test_auto_edit_compresses_tool_spans() {
    let events = vec![
        TimelineEvent {
            t: 0,
            kind: TimelineEventKind::UserMessage { text: "hi".into() },
        },
        TimelineEvent {
            t: 500,
            kind: TimelineEventKind::Thinking { duration: 800 },
        },
        TimelineEvent {
            t: 1300,
            kind: TimelineEventKind::StreamText {
                text: "Let me check.".into(),
                speed: 80,
            },
        },
        TimelineEvent {
            t: 2000,
            kind: TimelineEventKind::ToolStart {
                name: "file_read".into(),
                input: serde_json::json!({}),
            },
        },
        TimelineEvent {
            t: 12000,
            kind: TimelineEventKind::ToolDone {
                name: "file_read".into(),
                output: "ok".into(),
                is_error: false,
            },
        },
        TimelineEvent {
            t: 13000,
            kind: TimelineEventKind::StreamText {
                text: "Done!".into(),
                speed: 80,
            },
        },
        TimelineEvent {
            t: 14000,
            kind: TimelineEventKind::Done,
        },
    ];

    let opts = AutoEditOpts {
        tool_max_ms: 800,
        gap_max_ms: 2000,
        think_max_ms: 1200,
        response_delay_max_ms: 1000,
    };
    let edited = auto_edit_timeline(&events, &opts);

    assert_eq!(edited.len(), events.len());

    let tool_start_t = edited[3].t;
    let tool_done_t = edited[4].t;
    let tool_span = tool_done_t - tool_start_t;
    assert!(
        tool_span <= 800,
        "Tool span should be compressed to ≤800ms, got {tool_span}ms"
    );

    assert!(
        edited[5].t > tool_done_t,
        "Events after tool_done should still be ordered"
    );
}

#[test]
fn test_auto_edit_compresses_post_tool_idle_gap() {
    let events = vec![
        TimelineEvent {
            t: 0,
            kind: TimelineEventKind::UserMessage { text: "hi".into() },
        },
        TimelineEvent {
            t: 500,
            kind: TimelineEventKind::Thinking { duration: 800 },
        },
        TimelineEvent {
            t: 1500,
            kind: TimelineEventKind::ToolStart {
                name: "selfdev".into(),
                input: serde_json::json!({"action": "reload"}),
            },
        },
        TimelineEvent {
            t: 2500,
            kind: TimelineEventKind::ToolDone {
                name: "selfdev".into(),
                output: "Reload initiated. Process restarting...".into(),
                is_error: false,
            },
        },
        TimelineEvent {
            t: 48000,
            kind: TimelineEventKind::Thinking { duration: 800 },
        },
        TimelineEvent {
            t: 49000,
            kind: TimelineEventKind::StreamText {
                text: "Reloaded.".into(),
                speed: 80,
            },
        },
    ];

    let opts = AutoEditOpts::default();
    let edited = auto_edit_timeline(&events, &opts);

    let tool_done_t = edited[3].t;
    let resumed_t = edited[4].t;
    let gap = resumed_t - tool_done_t;
    assert!(
        gap <= opts.response_delay_max_ms,
        "Gap after tool completion should be compressed to ≤{}ms, got {gap}ms",
        opts.response_delay_max_ms
    );
}

#[test]
fn test_auto_edit_compresses_inter_prompt_gaps() {
    let events = vec![
        TimelineEvent {
            t: 0,
            kind: TimelineEventKind::UserMessage {
                text: "first".into(),
            },
        },
        TimelineEvent {
            t: 500,
            kind: TimelineEventKind::Thinking { duration: 800 },
        },
        TimelineEvent {
            t: 1500,
            kind: TimelineEventKind::StreamText {
                text: "response".into(),
                speed: 80,
            },
        },
        TimelineEvent {
            t: 2000,
            kind: TimelineEventKind::Done,
        },
        TimelineEvent {
            t: 30000,
            kind: TimelineEventKind::UserMessage {
                text: "second".into(),
            },
        },
        TimelineEvent {
            t: 30500,
            kind: TimelineEventKind::Thinking { duration: 800 },
        },
        TimelineEvent {
            t: 31500,
            kind: TimelineEventKind::StreamText {
                text: "response2".into(),
                speed: 80,
            },
        },
        TimelineEvent {
            t: 32000,
            kind: TimelineEventKind::Done,
        },
    ];

    let opts = AutoEditOpts::default();
    let edited = auto_edit_timeline(&events, &opts);

    let done_t = edited[3].t;
    let next_user_t = edited[4].t;
    let gap = next_user_t - done_t;
    assert!(
        gap <= 2000,
        "Gap between turns should be compressed to ≤2000ms, got {gap}ms"
    );

    let total_original = events.last().unwrap().t;
    let total_edited = edited.last().unwrap().t;
    assert!(
        total_edited < total_original,
        "Total time should be shorter: {total_edited} < {total_original}"
    );
}

#[test]
fn test_auto_edit_clamps_thinking() {
    let events = vec![
        TimelineEvent {
            t: 0,
            kind: TimelineEventKind::UserMessage { text: "hi".into() },
        },
        TimelineEvent {
            t: 500,
            kind: TimelineEventKind::Thinking { duration: 5000 },
        },
        TimelineEvent {
            t: 5500,
            kind: TimelineEventKind::StreamText {
                text: "ok".into(),
                speed: 80,
            },
        },
    ];

    let opts = AutoEditOpts {
        think_max_ms: 1200,
        ..Default::default()
    };
    let edited = auto_edit_timeline(&events, &opts);

    match &edited[1].kind {
        TimelineEventKind::Thinking { duration } => {
            assert_eq!(*duration, 1200, "Thinking should be clamped to 1200ms");
        }
        _ => panic!("Expected Thinking event"),
    }
}

#[test]
fn test_auto_edit_preserves_already_fast_timeline() {
    let events = vec![
        TimelineEvent {
            t: 0,
            kind: TimelineEventKind::UserMessage { text: "hi".into() },
        },
        TimelineEvent {
            t: 200,
            kind: TimelineEventKind::Thinking { duration: 500 },
        },
        TimelineEvent {
            t: 700,
            kind: TimelineEventKind::StreamText {
                text: "hello!".into(),
                speed: 80,
            },
        },
        TimelineEvent {
            t: 900,
            kind: TimelineEventKind::Done,
        },
        TimelineEvent {
            t: 1500,
            kind: TimelineEventKind::UserMessage { text: "bye".into() },
        },
    ];

    let opts = AutoEditOpts::default();
    let edited = auto_edit_timeline(&events, &opts);

    for (orig, ed) in events.iter().zip(edited.iter()) {
        assert_eq!(orig.t, ed.t, "Fast timeline should not be modified");
    }
}

#[test]
fn test_auto_edit_empty_timeline() {
    let edited = auto_edit_timeline(&[], &AutoEditOpts::default());
    assert!(edited.is_empty());
}
