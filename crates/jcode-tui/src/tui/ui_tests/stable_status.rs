use super::*;
use ratatui::{Terminal, backend::TestBackend};

fn render_status(state: &TestState, width: u16, queued: usize) -> Vec<String> {
    // A second sentinel row catches accidental wrapping or layout spillover.
    let mut terminal = Terminal::new(TestBackend::new(width.max(1), 2)).unwrap();
    terminal
        .draw(|frame| {
            input_ui::draw_status(frame, state, Rect::new(0, 0, width, 1), queued);
        })
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
        ProcessingStatus::Thinking(std::time::Instant::now()),
        ProcessingStatus::Streaming,
        ProcessingStatus::RunningTool("bash".into()),
        ProcessingStatus::RunningTool("batch".into()),
        ProcessingStatus::WaitingForNetwork {
            listener: "界\nlistener".into(),
        },
    ]
}

#[test]
fn stable_status_phase_transitions_preserve_clock_and_activity_columns() {
    let mut clock_column = None;
    let mut activity_column = None;
    for status in phases() {
        let state = TestState {
            status,
            status_elapsed: Some(Duration::from_secs(12)),
            ..Default::default()
        };
        let rows = render_status(&state, 120, 0);
        let text = &rows[0];
        let clock = text.find("00:12").expect(text);
        let activity = text.find('│').expect(text);
        assert_eq!(*clock_column.get_or_insert(clock), clock, "{text}");
        assert_eq!(*activity_column.get_or_insert(activity), activity, "{text}");
        assert!(rows[1].trim().is_empty(), "{rows:?}");
    }
}

#[test]
fn stable_status_centered_mode_does_not_move_changing_text() {
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
fn stable_status_narrow_widths_do_not_wrap_or_panic() {
    for status in phases() {
        let state = TestState {
            status,
            status_elapsed: Some(Duration::from_secs(360_000)),
            status_background: vec![crate::tui::BackgroundTaskRow {
                task_id: "bg".into(),
                label: "界\nlong\tbackground intent".repeat(20),
                percent: None,
                status: crate::tui::BackgroundTaskRowStatus::Running,
                completed_at: None,
            }],
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
fn stable_status_rate_limit_uses_the_same_clock_position() {
    let mut state = TestState {
        status: ProcessingStatus::Sending,
        status_elapsed: Some(Duration::from_secs(12)),
        ..Default::default()
    };
    let active = render_status(&state, 100, 0);
    state.status_rate_limit = Some(Duration::from_secs(80));
    let limited = render_status(&state, 100, 0);
    assert_eq!(active[0].find("00:12"), limited[0].find("00:12"));
    assert!(limited[0].to_lowercase().contains("retry"), "{limited:?}");
}

#[test]
fn stable_status_idle_does_not_rotate_tips() {
    let mut state = TestState::default();
    let initial = render_status(&state, 100, 0);
    for elapsed in [1.0, 15.0, 60.0, 120.0, 1000.0] {
        state.anim_elapsed = elapsed;
        assert_eq!(initial, render_status(&state, 100, 0));
    }
}

#[test]
fn stable_status_badges_do_not_move_activity_or_telemetry() {
    let mut state = TestState {
        status: ProcessingStatus::RunningTool("bash".into()),
        status_elapsed: Some(Duration::from_secs(12)),
        ..Default::default()
    };
    let before = render_status(&state, 120, 0);
    state.status_background = (0..100)
        .map(|index| crate::tui::BackgroundTaskRow {
            task_id: index.to_string(),
            label: "unbounded background intent".repeat(20),
            percent: None,
            status: crate::tui::BackgroundTaskRowStatus::Running,
            completed_at: None,
        })
        .collect();
    let after = render_status(&state, 120, 100);
    assert_eq!(before[0].find("bash"), after[0].find("bash"));
    assert_eq!(before[0].find('↑'), after[0].find('↑'));
    assert!(
        after[0].contains("[+99+ queued]") && after[0].contains("[bg 99+]"),
        "{after:?}"
    );
    state.status_background[0].status = crate::tui::BackgroundTaskRowStatus::Completed;
    let completed = render_status(&state, 120, 1);
    assert!(completed[0].contains("[bg 99]"), "{completed:?}");
    assert!(!completed[0].contains("unbounded"));
    let without_queue = render_status(&state, 120, 0);
    assert_eq!(completed[0].find("[bg"), without_queue[0].find("[bg"));
    state.status = ProcessingStatus::Idle;
    let idle = render_status(&state, 120, 0);
    assert_eq!(without_queue[0].find("[bg"), idle[0].find("[bg"));
}
