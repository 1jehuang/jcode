use super::*;
use ratatui::{Terminal, backend::TestBackend};

fn render_status(state: &TestState, width: u16, queued: usize) -> Vec<String> {
    // A second sentinel row catches accidental wrapping or layout spillover.
    let mut terminal = Terminal::new(TestBackend::new(width.max(1), 2)).unwrap();
    terminal
        .draw(|frame| input_ui::draw_status(frame, state, Rect::new(0, 0, width, 1), queued))
        .unwrap();
    let buffer = terminal.backend().buffer();
    (0..2)
        .map(|y| {
            (0..width.max(1))
                .map(|x| buffer[(x, y)].symbol())
                .collect::<String>()
        })
        .collect()
}

fn phases() -> Vec<ProcessingStatus> {
    vec![
        ProcessingStatus::Sending,
        ProcessingStatus::Connecting(crate::message::ConnectionPhase::Authenticating),
        ProcessingStatus::Connecting(crate::message::ConnectionPhase::Connecting),
        ProcessingStatus::Connecting(crate::message::ConnectionPhase::SendingRequest),
        ProcessingStatus::Connecting(crate::message::ConnectionPhase::WaitingForResponse),
        ProcessingStatus::Connecting(crate::message::ConnectionPhase::Retrying {
            attempt: 2,
            max: 3,
        }),
        ProcessingStatus::Thinking(std::time::Instant::now()),
        ProcessingStatus::Streaming,
        ProcessingStatus::RunningTool("bash".into()),
        ProcessingStatus::RunningTool("batch".into()),
        ProcessingStatus::WaitingForNetwork {
            listener: "界\nlistener".into(),
        },
    ]
}

fn tool(name: &str, intent: &str) -> ToolCall {
    serde_json::from_value(serde_json::json!({
        "id": "fixture", "name": name, "intent": intent, "input": { "command": "true" }
    }))
    .unwrap()
}

fn background(
    label: &str,
    status: crate::tui::BackgroundTaskRowStatus,
) -> crate::tui::BackgroundTaskRow {
    crate::tui::BackgroundTaskRow {
        task_id: label.into(),
        label: label.into(),
        percent: None,
        status,
        completed_at: None,
    }
}

#[test]
fn compact_status_phase_transitions_keep_clock_column_without_dashboard() {
    let mut clock_column = None;
    for status in phases() {
        let state = TestState {
            status,
            status_elapsed: Some(Duration::from_secs(12)),
            ..Default::default()
        };
        let rows = render_status(&state, 120, 0);
        let text = rows[0].trim_end();
        let offset = text.find("12s").expect(text);
        let clock = unicode_width::UnicodeWidthStr::width(&text[..offset]);
        assert_eq!(*clock_column.get_or_insert(clock), clock, "{text}");
        assert!(!text.contains('│') && !text.contains("-/s"), "{text}");
        assert!(rows[1].trim().is_empty(), "{rows:?}");
    }
}

#[test]
fn compact_status_centered_mode_does_not_move_changing_text() {
    for status in phases() {
        let mut state = TestState {
            status,
            status_elapsed: Some(Duration::from_secs(12)),
            ..Default::default()
        };
        let left = render_status(&state, 100, 0);
        state.centered_mode = true;
        assert_eq!(left, render_status(&state, 100, 0));
    }
}

#[test]
fn compact_status_narrow_unicode_widths_do_not_wrap_or_panic() {
    for status in phases() {
        let state = TestState {
            status,
            status_elapsed: Some(Duration::from_secs(360_000)),
            status_tool_calls: vec![tool("bash", &"界\nlong\tintent ".repeat(30))],
            status_background: vec![background(
                &"界\nlong\tbackground intent".repeat(20),
                crate::tui::BackgroundTaskRowStatus::Running,
            )],
            ..Default::default()
        };
        for width in 0..=120 {
            let rows = render_status(&state, width, 1234);
            assert!(rows[1].trim().is_empty(), "width={width}: {rows:?}");
            assert!(!rows[0].contains('\n') && !rows[0].contains('\t'));
        }
    }
}

#[test]
fn compact_status_connection_details_and_retry_counts_are_preserved() {
    for (phase, expected) in [
        (
            crate::message::ConnectionPhase::Authenticating,
            "refreshing auth",
        ),
        (
            crate::message::ConnectionPhase::SendingRequest,
            "sending context",
        ),
        (
            crate::message::ConnectionPhase::WaitingForResponse,
            "waiting for response",
        ),
        (
            crate::message::ConnectionPhase::Retrying { attempt: 2, max: 3 },
            "2/3",
        ),
    ] {
        let state = TestState {
            status: ProcessingStatus::Connecting(phase),
            status_elapsed: Some(Duration::from_secs(12)),
            ..Default::default()
        };
        let rows = render_status(&state, 160, 0);
        assert!(rows[0].contains(expected), "{rows:?}");
    }
}

#[test]
fn compact_status_rate_limit_keeps_clock_and_human_retry_time() {
    let mut state = TestState {
        status: ProcessingStatus::Sending,
        status_elapsed: Some(Duration::from_secs(12)),
        ..Default::default()
    };
    let active = render_status(&state, 100, 0);
    state.status_rate_limit = Some(Duration::from_secs(80));
    let limited = render_status(&state, 100, 2);
    assert_eq!(active[0].find("12s"), limited[0].find("12s"));
    assert!(
        limited[0].contains("1m 20s") && limited[0].contains("+2 queued"),
        "{limited:?}"
    );
}

#[test]
fn compact_status_streaming_keeps_real_tokens_speed_and_transport() {
    let state = TestState {
        status: ProcessingStatus::Streaming,
        status_elapsed: Some(Duration::from_secs(12)),
        status_tokens: (12_345, 456),
        status_tps: Some(35.2),
        status_transport: Some("fresh websocket".into()),
        status_upstream: Some("upstream".into()),
        ..Default::default()
    };
    let rows = render_status(&state, 160, 0);
    for expected in [
        "streaming",
        "12s",
        "↑12k ↓456",
        "35.2 tps",
        "opening websocket",
        "via upstream",
    ] {
        assert!(rows[0].contains(expected), "missing {expected}: {rows:?}");
    }
    assert!(
        rows[0].trim_end().len() < 140,
        "unexpected padding: {rows:?}"
    );
}

#[test]
fn compact_status_tool_keeps_intent_subagent_shortcut_and_queue() {
    let state = TestState {
        status: ProcessingStatus::RunningTool("bash".into()),
        status_elapsed: Some(Duration::from_secs(12)),
        status_tool_calls: vec![tool("bash", "Run focused tests")],
        status_tokens: (12_345, 456),
        status_subagent: Some("worker fox".into()),
        status_upstream: Some("upstream".into()),
        ..Default::default()
    };
    let rows = render_status(&state, 180, 2);
    for expected in [
        "bash",
        "Run focused tests",
        "↑12k ↓456",
        "worker fox",
        "via upstream",
        " bg",
        "+2 queued",
    ] {
        assert!(rows[0].contains(expected), "missing {expected}: {rows:?}");
    }
}

#[test]
fn compact_status_batch_keeps_completed_running_and_last_done() {
    let state = TestState {
        status: ProcessingStatus::RunningTool("batch".into()),
        status_elapsed: Some(Duration::from_secs(12)),
        batch_progress: Some(crate::bus::BatchProgress {
            session_id: "fixture".into(),
            tool_call_id: "batch".into(),
            total: 5,
            completed: 2,
            last_completed: Some("read".into()),
            running: vec![tool("bash", "Run tests")],
            subcalls: Vec::new(),
        }),
        ..Default::default()
    };
    let rows = render_status(&state, 180, 0);
    for expected in ["batch", "2/5 done", "running:", "bash", "last done: read"] {
        assert!(rows[0].contains(expected), "missing {expected}: {rows:?}");
    }
}

#[test]
fn compact_status_background_keeps_actual_intents_and_filters_finished_tasks() {
    let mut state = TestState {
        status: ProcessingStatus::RunningTool("bash".into()),
        status_elapsed: Some(Duration::from_secs(12)),
        status_background: vec![
            background("Run tests", crate::tui::BackgroundTaskRowStatus::Running),
            background("Build app", crate::tui::BackgroundTaskRowStatus::Running),
            background(
                "Already done",
                crate::tui::BackgroundTaskRowStatus::Completed,
            ),
        ],
        ..Default::default()
    };
    let active = render_status(&state, 160, 0);
    for expected in ["bg 2:", "Run tests", "Build app"] {
        assert!(active[0].contains(expected), "{active:?}");
    }
    assert!(!active[0].contains("Already done"));
    state.status = ProcessingStatus::Idle;
    let idle = render_status(&state, 160, 0);
    assert!(
        idle[0].contains("Run tests") && idle[0].contains("Build app"),
        "{idle:?}"
    );
    assert!(
        !idle[0].contains('│') && !idle[0].contains("◼ idle"),
        "{idle:?}"
    );
}

#[test]
fn compact_status_idle_has_no_dashboard_label_or_telemetry() {
    let state = TestState::default();
    let rows = render_status(&state, 120, 0);
    assert!(
        !rows[0].contains('│') && !rows[0].contains("◼ idle") && !rows[0].contains("-/s"),
        "{rows:?}"
    );
}
