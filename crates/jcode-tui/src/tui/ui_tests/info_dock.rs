use super::*;
use ratatui::backend::TestBackend;
use ratatui::{Terminal, layout::Rect};

/// Model, context, usage, KV cache and git: the facts the info box shows.
fn dock_data() -> info_widget::InfoWidgetData {
    info_widget::InfoWidgetData {
        model: Some("claude-opus-5-5".to_string()),
        provider_name: Some("claude".to_string()),
        auth_method: info_widget::AuthMethod::AnthropicOAuth,
        session_count: Some(0),
        session_name: Some("jaguar".to_string()),
        context_info: Some(crate::prompt::ContextInfo {
            system_prompt_chars: 20_000,
            total_chars: 900_000,
            ..Default::default()
        }),
        observed_context_tokens: Some(236_000),
        context_limit: Some(1_000_000),
        usage_info: Some(info_widget::UsageInfo {
            provider: info_widget::UsageProvider::Anthropic,
            primary_limit_label: Some("5-hour".to_string()),
            five_hour: 0.15,
            five_hour_resets_at: Some("2099-01-01T00:00:00Z".to_string()),
            secondary_limit_label: Some("Weekly".to_string()),
            seven_day: 0.73,
            seven_day_resets_at: Some("2099-01-04T00:00:00Z".to_string()),
            available: true,
            ..Default::default()
        }),
        git_info: Some(info_widget::GitInfo {
            branch: "siavz".to_string(),
            modified: 10,
            ahead: 36,
            behind: 162,
            untracked: 1,
            staged: 0,
            dirty_files: vec![info_widget::DirtyFile {
                status: 'M',
                path: "crates/jcode-tui/src/tui/ui.rs".to_string(),
                ..Default::default()
            }],
            dirty_total: 1,
            ..Default::default()
        }),
        ..Default::default()
    }
}

/// A long, ragged transcript: short and full-width lines mixed, like real chat.
fn ragged_chat() -> Vec<DisplayMessage> {
    (0..60)
        .map(|i| DisplayMessage {
            role: if i % 5 == 0 { "user" } else { "assistant" }.to_string(),
            content: if i % 3 == 0 {
                format!("message {i}: {}", "long line of reply text ".repeat(12))
            } else {
                format!("message {i}: short")
            },
            tool_calls: vec![],
            duration_secs: None,
            title: None,
            tool_data: None,
        })
        .collect()
}

fn render(state: &TestState, width: u16, height: u16) -> Vec<String> {
    let mut terminal = Terminal::new(TestBackend::new(width, height)).unwrap();
    terminal
        .draw(|frame| crate::tui::ui::draw(frame, state))
        .unwrap();
    let buf = terminal.backend().buffer();
    (0..height)
        .map(|y| (0..width).map(|x| buf[(x, y)].symbol()).collect::<String>())
        .collect()
}

/// `(row, column, width)` of the first box drawn in the right third.
fn right_box(lines: &[String]) -> Option<(usize, usize, usize)> {
    for (row, line) in lines.iter().enumerate() {
        let cells: Vec<&str> = line_cells(line);
        let Some(left) = cells.iter().position(|c| *c == "╭") else {
            continue;
        };
        if left < cells.len() / 2 {
            continue;
        }
        let right = cells.iter().rposition(|c| *c == "╮")?;
        return Some((row, left, right - left + 1));
    }
    None
}

/// Split a rendered row into cells (one grapheme per terminal column).
fn line_cells(line: &str) -> Vec<&str> {
    let mut cells = Vec::new();
    let mut rest = line;
    while let Some(c) = rest.chars().next() {
        let len = c.len_utf8();
        cells.push(&rest[..len]);
        rest = &rest[len..];
    }
    cells
}

/// The info box must not move with the chat: the same rows and columns at
/// every scroll position (the reported bug: it rode the transcript and jumped
/// between gaps).
#[test]
fn docked_info_box_does_not_move_while_scrolling() {
    let _lock = viewport_snapshot_test_lock();
    let mut state = TestState {
        display_messages: ragged_chat(),
        info_widget_data: dock_data(),
        ..Default::default()
    };
    let anchor = right_box(&render(&state, 160, 40)).expect("info box drawn at the bottom");
    for top in (0..200).step_by(7) {
        state.scroll_top_line = Some(top);
        let lines = render(&state, 160, 40);
        assert_eq!(
            right_box(&lines),
            Some(anchor),
            "box moved at scroll line {top}:\n{}",
            lines.join("\n")
        );
    }
}

/// Streaming text appends lines under the box; it must stay put.
#[test]
fn docked_info_box_does_not_move_while_streaming() {
    let _lock = viewport_snapshot_test_lock();
    let mut state = TestState {
        display_messages: ragged_chat(),
        info_widget_data: dock_data(),
        status: ProcessingStatus::Streaming,
        ..Default::default()
    };
    let mut seen = None;
    for words in 0..30 {
        state.streaming_text = "streamed words that wrap across lines ".repeat(words);
        let lines = render(&state, 160, 40);
        let now = right_box(&lines).expect("info box drawn while streaming");
        assert_eq!(
            *seen.get_or_insert(now),
            now,
            "box moved after {words} chunks"
        );
    }
}

/// Chat text never runs under the docked box: the transcript wraps to the
/// column left of it.
#[test]
fn transcript_never_runs_under_the_docked_box() {
    let _lock = viewport_snapshot_test_lock();
    let mut state = TestState {
        display_messages: ragged_chat(),
        info_widget_data: dock_data(),
        ..Default::default()
    };
    for top in [None, Some(0), Some(40), Some(90)] {
        state.scroll_top_line = top;
        let lines = render(&state, 160, 40);
        let (_, left, _) = right_box(&lines).expect("info box drawn");
        for (row, line) in lines.iter().enumerate().take(30) {
            let tail: String = line_cells(line)[left..].concat();
            assert!(
                !tail.contains("reply text"),
                "row {row} at {top:?}: chat text inside the dock column: {tail:?}"
            );
        }
    }
}

/// Everything the box shows fits: nothing is cut off at the right edge.
#[test]
fn docked_info_box_shows_every_line_in_full() {
    let data = dock_data();
    let width = info_widget::dock_width(&data, 160).expect("dock fits in 160 columns");
    let natural = info_widget::dock_text_lines(&data, 200);
    let docked = info_widget::dock_text_lines(&data, width);
    assert_eq!(natural, docked, "lines changed when fitted into the dock");
    for line in &docked {
        assert!(
            unicode_width::UnicodeWidthStr::width(line.as_str()) <= usize::from(width - 2),
            "line wider than the dock: {line:?}"
        );
    }
    for needle in ["Opus", "OAuth", "Context", "5-hour", "Weekly", "siavz"] {
        assert!(
            docked.iter().any(|line| line.contains(needle)),
            "{needle} missing: {docked:#?}"
        );
    }
}

/// On a terminal too narrow to spare a column, the dock is not used and the
/// chat keeps its full width.
#[test]
fn narrow_terminal_has_no_dock() {
    assert_eq!(info_widget::dock_width(&dock_data(), 80), None);
    assert!(info_widget::dock_width(&dock_data(), 100).is_some());
}

/// The box's width must not follow its content. It used to be measured from
/// the longest line, so the KV cache line appearing after the first reply
/// widened the box by a column and moved its left edge (seen live).
#[test]
fn dock_width_ignores_content() {
    let before_reply = dock_data();
    let after_reply = info_widget::InfoWidgetData {
        cache_hit_info: Some(info_widget::CacheHitInfo {
            reported_input_tokens: 20_000,
            prompt_tokens: Some(38_000),
            last_prompt_tokens: Some(10_000),
            read_tokens: 15_000,
            creation_tokens: 3_000,
            optimal_input_tokens: 16_667,
            optimal_read_tokens: None,
            last_reported_input_tokens: Some(10_000),
            last_read_tokens: Some(9_400),
            last_creation_tokens: Some(0),
            last_optimal_input_tokens: Some(9_895),
            miss_attributions: Vec::new(),
        }),
        ..dock_data()
    };
    let lines = |data: &info_widget::InfoWidgetData| {
        info_widget::dock_text_lines(data, 200)
            .iter()
            .map(|l| unicode_width::UnicodeWidthStr::width(l.as_str()))
            .max()
            .unwrap_or(0)
    };
    assert!(
        lines(&after_reply) > lines(&before_reply),
        "precondition: the KV cache line is longer than the others"
    );
    assert_eq!(
        info_widget::dock_width(&before_reply, 160),
        info_widget::dock_width(&after_reply, 160),
        "box width changed when the KV cache line appeared"
    );
    // Narrow terminals still shrink it to what is free.
    assert!(info_widget::dock_width(&after_reply, 100).unwrap() <= 40);
}

/// No data, no dock: the chat keeps its full width.
#[test]
fn empty_info_has_no_dock() {
    assert_eq!(
        info_widget::dock_width(&info_widget::InfoWidgetData::default(), 160),
        None
    );
}

#[test]
fn dock_box_is_right_aligned_and_sits_mid_screen() {
    let _lock = viewport_snapshot_test_lock();
    let state = TestState {
        display_messages: ragged_chat(),
        info_widget_data: dock_data(),
        ..Default::default()
    };
    let lines = render(&state, 160, 40);
    let (row, left, width) = right_box(&lines).expect("info box drawn");
    assert_eq!(left + width, 160, "box is flush with the right edge");
    let height = usize::from(info_widget::dock_height(&dock_data(), width as u16));
    assert!(
        (10..=16).contains(&row) && row + height < 36,
        "box should start a third of the way down a 40-row screen and end above \
         the status line, top row {row}, height {height}"
    );
}

/// Typing a long prompt grows the input box. The info box must not move
/// unless the input would reach it.
#[test]
fn docked_info_box_does_not_move_when_input_grows() {
    let _lock = viewport_snapshot_test_lock();
    let mut state = TestState {
        display_messages: ragged_chat(),
        info_widget_data: dock_data(),
        ..Default::default()
    };
    let before = right_box(&render(&state, 160, 40)).expect("info box drawn");
    state.input = "a longer prompt that wraps onto a few lines ".repeat(8);
    state.cursor_pos = state.input.len();
    let after = right_box(&render(&state, 160, 40)).expect("info box drawn");
    assert_eq!(before, after, "box moved when the input grew");
}

#[test]
fn dock_rect_top_is_fixed_and_stays_above_the_status_line() {
    let column = Rect::new(100, 0, 40, 40);
    // Top edge a third of the way down the column, whatever the box height.
    assert_eq!(
        info_widget::dock_rect(column, 36, 10),
        Some(Rect::new(100, 13, 40, 10))
    );
    assert_eq!(
        info_widget::dock_rect(column, 36, 14),
        Some(Rect::new(100, 13, 40, 14))
    );
    // Pulled up when the status line would cut it off.
    assert_eq!(
        info_widget::dock_rect(column, 22, 10),
        Some(Rect::new(100, 12, 40, 10))
    );
    // Clipped to the space above the status line when that is all there is.
    assert_eq!(
        info_widget::dock_rect(column, 6, 10),
        Some(Rect::new(100, 0, 40, 6))
    );
    assert_eq!(info_widget::dock_rect(column, 36, 0), None);
}

/// The box's top edge must not move when its own content changes height, for
/// example when the usage limits come back after the usage endpoint was
/// throttled. It grows downward instead.
#[test]
fn docked_info_box_top_stays_put_when_its_content_grows() {
    let _lock = viewport_snapshot_test_lock();
    let mut without_usage = dock_data();
    without_usage.usage_info = None;
    let short = TestState {
        display_messages: ragged_chat(),
        info_widget_data: without_usage,
        ..Default::default()
    };
    let full = TestState {
        info_widget_data: dock_data(),
        ..short.clone()
    };
    let (short_row, _, _) = right_box(&render(&short, 160, 40)).expect("box without usage");
    let full_lines = render(&full, 160, 40);
    let (full_row, _, _) = right_box(&full_lines).expect("box with usage");
    assert_eq!(
        short_row, full_row,
        "top edge moved when usage lines appeared"
    );
    assert!(
        full_lines.iter().any(|l| l.contains("5-hour")),
        "usage lines are shown:\n{}",
        full_lines.join("\n")
    );
}

/// The fixed dock width fits every section at full length, including the
/// KV cache line (the longest) and a long session name, so nothing is cut.
#[test]
fn fixed_dock_width_fits_the_longest_real_lines() {
    let data = info_widget::InfoWidgetData {
        session_name: Some("hatchling-worktree".to_string()),
        cache_hit_info: Some(info_widget::CacheHitInfo {
            reported_input_tokens: 200_000,
            prompt_tokens: Some(380_000),
            last_prompt_tokens: Some(100_000),
            read_tokens: 150_000,
            creation_tokens: 30_000,
            optimal_input_tokens: 166_667,
            optimal_read_tokens: None,
            last_reported_input_tokens: Some(100_000),
            last_read_tokens: Some(94_000),
            last_creation_tokens: Some(0),
            last_optimal_input_tokens: Some(98_950),
            miss_attributions: Vec::new(),
        }),
        ..dock_data()
    };
    let width = info_widget::dock_width(&data, 160).expect("dock");
    let natural = info_widget::dock_text_lines(&data, 200);
    let docked = info_widget::dock_text_lines(&data, width);
    assert_eq!(natural, docked, "a line was shortened to fit the dock");
}
