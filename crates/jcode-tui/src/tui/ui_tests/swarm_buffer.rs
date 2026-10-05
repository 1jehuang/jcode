//! Buffer-level (terminal cell) verification for the inline swarm strip and
//! the notification line.
//!
//! Prior swarm-strip/notification tests only checked `Line` construction
//! (span widths). These tests close that gap: they render through ratatui's
//! `TestBackend` so actual cell writes are exercised, including the full
//! `ui::draw` layout path (ui.rs strip Paragraph at chunk 2, notification at
//! chunk 4) and direct widget draws into sub-areas, asserting no panics and
//! that nothing is written outside the target area even with wide glyphs.

use super::*;
use crate::protocol::SwarmMemberStatus;
use crate::tui::ui::clear_flicker_frame_history_for_tests;
use ratatui::Terminal;
use ratatui::backend::TestBackend;

fn strip_member(id: &str, name: &str, status: &str) -> SwarmMemberStatus {
    SwarmMemberStatus {
        session_id: id.to_string(),
        friendly_name: Some(name.to_string()),
        status: status.to_string(),
        detail: Some("working on task".to_string()),
        task_label: None,
        role: None,
        is_headless: Some(true),
        live_attachments: None,
        status_age_secs: Some(5),
        output_tail: None,
        report_back_to_session_id: None,
        todo_progress: Some((2, 5)),
        todo_items: Vec::new(),
        runtime: crate::protocol::SwarmMemberRuntime::default(),
    }
}

/// Buffer contents as one string per row (not trimmed, full width).
fn buffer_rows(terminal: &Terminal<TestBackend>) -> Vec<String> {
    let buf = terminal.backend().buffer();
    let width = buf.area.width;
    let height = buf.area.height;
    (0..height)
        .map(|y| {
            (0..width)
                .map(|x| buf[(x, y)].symbol().to_string())
                .collect::<String>()
        })
        .collect()
}

fn fact_test_state(input: String, scheduled: bool) -> TestState {
    let home = std::env::var("HOME").unwrap_or_else(|_| "/home/test".to_string());
    let ambient_info = scheduled.then(|| info_widget::AmbientWidgetData {
        show_widget: false,
        status: crate::ambient::AmbientStatus::Idle,
        queue_count: 1,
        next_queue_preview: Some("check the build".to_string()),
        reminder_count: 1,
        next_reminder_preview: Some("check the build".to_string()),
        last_run_ago: None,
        last_summary: None,
        next_wake: None,
        next_reminder_wake: Some("in 4m".to_string()),
        budget_percent: None,
    });
    let info_widget_data = info_widget::InfoWidgetData {
        model: Some("gpt-5.6-sol".to_string()),
        reasoning_effort: Some("high".to_string()),
        context_limit: Some(256_000),
        provider_name: Some("openai".to_string()),
        auth_method: info_widget::AuthMethod::OpenAIOAuth,
        observed_context_tokens: Some(74_000),
        ambient_info,
        ..Default::default()
    };
    TestState {
        cursor_pos: input.len(),
        input,
        provider_name: Some("openai".to_string()),
        provider_model: Some("gpt-5.6-sol".to_string()),
        working_dir: Some(format!("{home}/jcode")),
        info_widget_data,
        suppress_info_widgets: true,
        display_messages: vec![DisplayMessage::assistant("last transcript line")],
        messages_version: 1,
        ..Default::default()
    }
}

#[test]
fn swarm_strip_full_draw_writes_chips_row_above_status_line() {
    let _lock = viewport_snapshot_test_lock();
    clear_flicker_frame_history_for_tests();
    // Placement state is process-global; a dock placed by another test would
    // make the strip stand down. Clear it so this frame is self-contained.
    crate::tui::info_widget::clear_widget_placements_for_tests();
    let state = TestState {
        display_messages: vec![DisplayMessage::assistant("hello from the coordinator")],
        messages_version: 1,
        swarm_members: vec![
            strip_member("s1", "researcher", "running"),
            strip_member("s2", "reviewer", "completed"),
        ],
        ..Default::default()
    };

    let backend = TestBackend::new(80, 24);
    let mut terminal = Terminal::new(backend).expect("test terminal");
    terminal
        .draw(|frame| crate::tui::ui::draw(frame, &state))
        .expect("full draw with inline swarm strip should not panic");

    let status_area = crate::tui::ui::last_status_area().expect("status area recorded");
    assert!(status_area.y > 0, "status line should not be the top row");
    let rows = buffer_rows(&terminal);
    // Vertical strip (default layout): one agent per row directly above the
    // status line, first row carrying the 🐝 marker.
    let strip_rows = rows[..status_area.y as usize]
        .iter()
        .rev()
        .take(2)
        .cloned()
        .collect::<Vec<_>>()
        .join("\n");
    assert!(
        strip_rows.contains("🐝"),
        "expected the swarm marker above the status line, got: {strip_rows:?}"
    );
    assert!(
        strip_rows.contains("researcher"),
        "expected member row in strip cells, got: {strip_rows:?}"
    );
    assert!(
        strip_rows.contains("2/5"),
        "expected todo progress counter in strip cells, got: {strip_rows:?}"
    );
}

#[test]
fn swarm_strip_full_draw_survives_narrow_width_sweep() {
    let _lock = viewport_snapshot_test_lock();
    crate::tui::info_widget::clear_widget_placements_for_tests();
    let state = TestState {
        display_messages: vec![DisplayMessage::assistant("narrow sweep")],
        messages_version: 1,
        swarm_members: vec![
            strip_member("s1", "alpha", "running"),
            strip_member("s2", "beta", "ready"),
            strip_member("s3", "gamma", "failed"),
        ],
        ..Default::default()
    };

    for width in 12_u16..=44 {
        for height in [8_u16, 12, 20] {
            clear_flicker_frame_history_for_tests();
            let backend = TestBackend::new(width, height);
            let mut terminal = Terminal::new(backend).expect("test terminal");
            terminal
                .draw(|frame| crate::tui::ui::draw(frame, &state))
                .unwrap_or_else(|e| {
                    panic!("swarm strip draw failed at {width}x{height}: {e}");
                });
        }
    }
}

#[test]
fn swarm_strip_full_draw_handles_wide_glyph_member_names() {
    let _lock = viewport_snapshot_test_lock();
    crate::tui::info_widget::clear_widget_placements_for_tests();
    let mut coordinator = strip_member("s0", "調整役エージェント", "running");
    coordinator.role = Some("coordinator".to_string());
    let mut streaming = strip_member("s1", "深度搜索智能体", "running");
    streaming.output_tail = Some("正在分析：渲染管線的寬字元邊界 🐝🎨".to_string());
    let members = vec![
        coordinator,
        streaming,
        strip_member("s2", "🦊🦊🦊 fox-agent 🦊🦊🦊", "completed"),
    ];

    // Unfocused (1 line) and focused (chips + preview + hints) variants both
    // must survive cell-level rendering with wide glyphs at every width.
    for focused in [false, true] {
        let state = TestState {
            display_messages: vec![DisplayMessage::assistant("wide glyph check")],
            messages_version: 1,
            swarm_members: members.clone(),
            swarm_panel_focused: focused,
            swarm_panel_selected: 1,
            ..Default::default()
        };
        for width in [24_u16, 25, 30, 31, 44, 80] {
            clear_flicker_frame_history_for_tests();
            let backend = TestBackend::new(width, 16);
            let mut terminal = Terminal::new(backend).expect("test terminal");
            terminal
                .draw(|frame| crate::tui::ui::draw(frame, &state))
                .unwrap_or_else(|e| {
                    panic!("wide-glyph strip draw failed at width {width} focused={focused}: {e}");
                });
        }
    }
}

#[test]
fn swarm_strip_paragraph_never_writes_outside_target_area() {
    // Mirror the exact ui.rs render path (Paragraph::new(lines) into a chunk),
    // but deliberately render lines built for a wider area into a narrow Rect
    // to prove clipping happens at the cell level, including wide glyphs.
    let members = vec![
        strip_member("s1", "深度搜索エージェント", "running"),
        strip_member("s2", "reviewer-with-a-long-name", "completed"),
    ];
    let gallery_lines = crate::tui::info_widget::swarm_gallery::render_swarm_strip_lines(
        &members, 0, true, "ctrl+t", 3, 80, 16,
    );
    assert!(!gallery_lines.is_empty(), "expected focused strip lines");

    let backend = TestBackend::new(40, 6);
    let mut terminal = Terminal::new(backend).expect("test terminal");
    let area = Rect::new(2, 1, 20, gallery_lines.len() as u16);
    terminal
        .draw(|frame| {
            frame.render_widget(Paragraph::new(gallery_lines.clone()), area);
        })
        .expect("over-wide strip paragraph should clip, not panic");

    let rows = buffer_rows(&terminal);
    for (y, row) in rows.iter().enumerate() {
        let cells: Vec<char> = row.chars().collect();
        let inside_rows = (area.y as usize)..(area.y + area.height) as usize;
        if !inside_rows.contains(&y) {
            assert!(
                row.trim().is_empty(),
                "row {y} outside strip area should be untouched, got: {row:?}"
            );
            continue;
        }
        for (x, ch) in cells.iter().enumerate() {
            if x < area.x as usize || x >= (area.x + area.width) as usize {
                assert_eq!(
                    *ch, ' ',
                    "cell ({x},{y}) outside strip area must stay blank, got {ch:?} in row {row:?}"
                );
            }
        }
    }
    let first_row = &rows[area.y as usize];
    assert!(
        !first_row.trim().is_empty(),
        "strip content should be written inside the area"
    );
}

#[test]
fn notification_full_draw_survives_overwide_swarm_plan_notice() {
    let _lock = viewport_snapshot_test_lock();
    let notice = "Swarm plan v3 · 12/24 tasks · gate 'critique-swarm-ui' blocked · \
                  reassigning 深度搜索エージェント → sheep-1 · awaiting verify-buffer-draw \
                  · retry budget 2/5 · ⚠ worker fox timed out"
        .to_string();

    for (width, height) in [(30_u16, 12_u16), (44, 16), (80, 24)] {
        clear_flicker_frame_history_for_tests();
        let state = TestState {
            display_messages: vec![DisplayMessage::assistant("plan running")],
            messages_version: 1,
            status_notice: Some(notice.clone()),
            ..Default::default()
        };
        let backend = TestBackend::new(width, height);
        let mut terminal = Terminal::new(backend).expect("test terminal");
        terminal
            .draw(|frame| crate::tui::ui::draw(frame, &state))
            .unwrap_or_else(|e| {
                panic!("over-wide notification draw failed at {width}x{height}: {e}");
            });

        let status_area = crate::tui::ui::last_status_area().expect("status area recorded");
        let rows = buffer_rows(&terminal);
        let notification_row = &rows[(status_area.y + 1) as usize];
        assert!(
            notification_row.contains("Swarm plan v3"),
            "expected notification cells below status line at {width}x{height}, got: {notification_row:?}"
        );
    }
}

/// Regression: the inline swarm strip must not oscillate against the dock.
///
/// The strip row grows the bottom chrome, so every appearance shoves the
/// transcript up one row. When the strip keyed off raw last-frame dock
/// visibility, the dock's natural placement churn (hidden-in-place blinks
/// while content scrolls under it) made the strip pop in and out every few
/// frames: visible up/down flicker. Now the stand-down is sticky: an anchored
/// (hidden-in-place) dock still counts as engaged, and disengagement is
/// debounced by a linger.
#[test]
fn swarm_strip_stands_down_through_dock_blinks() {
    let _lock = viewport_snapshot_test_lock();
    use crate::tui::info_widget::{
        WidgetKind, calculate_placements, swarm_strip_stands_down_for_dock,
    };
    crate::tui::info_widget::clear_widget_placements_for_tests();

    let mut coordinator = strip_member("s0", "researcher", "running");
    coordinator.role = Some("coordinator".to_string());
    let data = crate::tui::info_widget::InfoWidgetData {
        swarm_info: Some(crate::tui::info_widget::SwarmInfo {
            managed_members: vec![coordinator, strip_member("s1", "reviewer", "completed")],
            ..Default::default()
        }),
        ..Default::default()
    };
    let messages_area = Rect::new(0, 0, 120, 26);
    let wide_margins = crate::tui::info_widget::Margins {
        right_widths: vec![44; 26],
        ..Default::default()
    };
    // Zero free margin: the dock cannot render this frame (a wide line is
    // covering its slot), so it hides in place behind its anchor.
    let covered_margins = crate::tui::info_widget::Margins {
        right_widths: vec![0; 26],
        ..Default::default()
    };

    assert!(
        !swarm_strip_stands_down_for_dock(),
        "no dock engagement yet: strip should be free to show"
    );

    // Dock places: strip stands down.
    let placed = calculate_placements(messages_area, &wide_margins, &data);
    assert!(
        placed.iter().any(|p| p.kind == WidgetKind::SwarmStatus),
        "dock should place with a wide free margin"
    );
    assert!(
        swarm_strip_stands_down_for_dock(),
        "strip must stand down while the dock shows"
    );

    // Full-draw integration: with the dock engaged, ui::draw omits the strip
    // row above the status line (TestState's empty widget data means this
    // draw's own widget pass will clear the engagement afterwards, so this
    // must be checked before continuing the state-machine sequence).
    {
        let state = TestState {
            display_messages: vec![DisplayMessage::assistant("coordinating agents")],
            messages_version: 1,
            swarm_members: vec![
                strip_member("s0", "researcher", "running"),
                strip_member("s1", "reviewer", "completed"),
            ],
            ..Default::default()
        };
        clear_flicker_frame_history_for_tests();
        let backend = TestBackend::new(120, 30);
        let mut terminal = Terminal::new(backend).expect("test terminal");
        terminal
            .draw(|frame| crate::tui::ui::draw(frame, &state))
            .expect("draw with engaged dock should not panic");
        let status_area = crate::tui::ui::last_status_area().expect("status area recorded");
        let rows = buffer_rows(&terminal);
        let above_status = rows[..status_area.y as usize]
            .iter()
            .rev()
            .take(2)
            .cloned()
            .collect::<Vec<_>>()
            .join("\n");
        assert!(
            !above_status.contains("🐝") && !above_status.contains("researcher"),
            "strip must not render while the dock stands it down, got: {above_status:?}"
        );
    }

    // Re-engage (the integration draw above cleared state via its own empty
    // widget pass), then blink the dock hidden-in-place: anchor retained,
    // nothing placed. The strip must NOT pop back for the blink.
    crate::tui::info_widget::clear_widget_placements_for_tests();
    calculate_placements(messages_area, &wide_margins, &data);
    let blink = calculate_placements(messages_area, &covered_margins, &data);
    assert!(
        blink.iter().all(|p| p.kind != WidgetKind::SwarmStatus),
        "covered margin must hide the dock this frame"
    );
    assert!(
        swarm_strip_stands_down_for_dock(),
        "strip must keep standing down through a hidden-in-place dock blink"
    );

    // Even after the anchor is abandoned (hidden too long), the linger keeps
    // the strip down so a re-homing dock does not race a strip pop-in.
    for _ in 0..32 {
        calculate_placements(messages_area, &covered_margins, &data);
    }
    assert!(
        swarm_strip_stands_down_for_dock(),
        "strip must keep standing down through the post-disengage linger"
    );

    // A real teardown (widget pass skipped entirely) releases the stand-down.
    crate::tui::info_widget::note_widget_pass_skipped();
    assert!(
        !swarm_strip_stands_down_for_dock(),
        "strip should be free to return once the dock is genuinely gone"
    );
}

/// The swarm dock widget renders the compact summary at the cell level:
/// place it through the real `calculate_placements` + `render_all` path into
/// a TestBackend and assert the summary + progress bar landed inside the
/// placement rect.
#[test]
fn swarm_dock_widget_full_render_writes_agent_rows_in_margin() {
    let _lock = viewport_snapshot_test_lock();
    let mut coordinator = strip_member("s0", "researcher", "running");
    coordinator.role = Some("coordinator".to_string());
    coordinator.output_tail = Some("tracing the refresh path".to_string());
    let data = crate::tui::info_widget::InfoWidgetData {
        swarm_info: Some(crate::tui::info_widget::SwarmInfo {
            managed_members: vec![coordinator, strip_member("s1", "reviewer", "completed")],
            plan_progress: Some((3, 2, 7)),
            ..Default::default()
        }),
        ..Default::default()
    };

    let backend = TestBackend::new(120, 30);
    let mut terminal = Terminal::new(backend).expect("test terminal");
    let messages_area = Rect::new(0, 0, 120, 26);
    let margins = crate::tui::info_widget::Margins {
        right_widths: vec![44; 26],
        left_widths: Vec::new(),
        centered: false,
        ..Default::default()
    };
    let mut dock_rect: Option<Rect> = None;
    terminal
        .draw(|frame| {
            let placements =
                crate::tui::info_widget::calculate_placements(messages_area, &margins, &data);
            dock_rect = placements
                .iter()
                .find(|p| p.kind == crate::tui::info_widget::WidgetKind::SwarmStatus)
                .map(|p| p.rect);
            crate::tui::info_widget::render_all(frame, &placements, &data);
        })
        .expect("dock widget render should not panic");

    let rect = dock_rect.expect("SwarmStatus dock should be placed with a wide free margin");
    let rows = buffer_rows(&terminal);
    let dock_text: String = rows[rect.y as usize..(rect.y + rect.height) as usize]
        .iter()
        .map(|row| {
            row.chars()
                .skip(rect.x as usize)
                .take(rect.width as usize)
                .collect::<String>()
        })
        .collect::<Vec<_>>()
        .join("\n");
    assert!(
        dock_text.contains("1/2 active"),
        "expected agents tally inside dock rect, got:\n{dock_text}"
    );
    assert!(
        dock_text.contains("nodes 3/7"),
        "expected node progress on dock border, got:\n{dock_text}"
    );
    assert!(
        dock_text.contains("researcher") && dock_text.contains("reviewer"),
        "expected one row per agent inside dock rect, got:\n{dock_text}"
    );
    // Nothing from the dock leaked left of its rect.
    for row in &rows[rect.y as usize..(rect.y + rect.height) as usize] {
        let left: String = row.chars().take(rect.x as usize).collect();
        assert!(
            left.trim().is_empty(),
            "dock must not write left of its rect, got: {left:?}"
        );
    }
}

#[test]
fn draw_notification_clips_overwide_notice_at_area_width() {
    let notice: String = "Swarm plan v3 · 12/24 tasks · gate blocked · ".repeat(8);
    let state = TestState {
        status_notice: Some(notice),
        ..Default::default()
    };

    let backend = TestBackend::new(60, 3);
    let mut terminal = Terminal::new(backend).expect("test terminal");
    let area = Rect::new(2, 1, 20, 1);
    terminal
        .draw(|frame| input_ui::draw_notification(frame, &state, area))
        .expect("over-wide notification should clip, not panic");

    let rows = buffer_rows(&terminal);
    assert!(rows[0].trim().is_empty(), "row above area must be blank");
    assert!(rows[2].trim().is_empty(), "row below area must be blank");
    let cells: Vec<char> = rows[1].chars().collect();
    for (x, ch) in cells.iter().enumerate() {
        if x < area.x as usize || x >= (area.x + area.width) as usize {
            assert_eq!(
                *ch, ' ',
                "cell ({x},1) outside notification area must stay blank, got {ch:?}"
            );
        }
    }
    let inside: String = cells[area.x as usize..(area.x + area.width) as usize]
        .iter()
        .collect();
    assert!(
        inside.starts_with("Swarm plan v3"),
        "expected clipped notice text inside area, got: {inside:?}"
    );
}

fn overscroll_line_state() -> TestState {
    let mut state = fact_test_state(String::new(), false);
    state.info_widget_data.git_info = Some(info_widget::GitInfo {
        branch: "main".to_string(),
        modified: 3,
        untracked: 2,
        staged: 0,
        ahead: 1,
        behind: 0,
        dirty_files: Vec::new(),
        dirty_total: 0,
        ..Default::default()
    });
    state
}

fn overscroll_line_row(state: &TestState, width: u16) -> String {
    let _lock = viewport_snapshot_test_lock();
    clear_flicker_frame_history_for_tests();
    let backend = TestBackend::new(width, 18);
    let mut terminal = Terminal::new(backend).expect("test terminal");
    terminal
        .draw(|frame| crate::tui::ui::draw(frame, state))
        .expect("overscroll frame");
    let rows = buffer_rows(&terminal);
    // The overscroll line is the last row of the frame.
    rows.last().cloned().unwrap_or_default()
}

#[test]
fn overscroll_line_orders_dir_git_context_then_model_on_the_right() {
    let row = overscroll_line_row(&overscroll_line_state(), 200);
    let dir = row.find("~/jcode").expect("dir shown");
    let branch = row.find(" main").expect("branch shown");
    let git = row.find("~3 ?2 ↑1").expect("git status shown");
    let context = row.find("74k/256k").expect("token counts shown when roomy");
    let model = row.find("GPT-5.6 Sol high").expect("model shown");
    assert!(
        dir < branch && branch < git && git < context && context < model,
        "{row}"
    );
    assert!(
        row.contains("▰▰▰▱▱▱▱▱▱▱ 29%"),
        "full 10-cell bar when roomy: {row}"
    );
    assert!(row.contains("OAuth") && row.contains("OpenAI"), "{row}");
    assert!(
        !row.contains("(overscroll"),
        "no countdown on the pinned line: {row}"
    );
}

#[test]
fn overscroll_line_compacts_before_dropping_and_always_keeps_dir_model_context() {
    let state = overscroll_line_state();
    let mut prev_len = usize::MAX;
    for width in (20..=200).rev() {
        let row = overscroll_line_row(&state, width);
        let trimmed = row.trim().to_string();
        if width >= 40 {
            assert!(row.contains("jcode"), "dir kept at width {width}: {row}");
            assert!(
                row.contains("GPT-5.6 Sol"),
                "model kept at width {width}: {row}"
            );
            assert!(row.contains("29%"), "context kept at width {width}: {row}");
            assert!(
                !row.contains('…'),
                "no mid-word truncation at width {width}: {row}"
            );
        }
        let len = unicode_width::UnicodeWidthStr::width(trimmed.as_str());
        assert!(len <= width as usize, "fits at width {width}: {row}");
        prev_len = prev_len.min(len.max(1));
    }
}

#[test]
fn overscroll_line_compact_steps_at_medium_width() {
    let row = overscroll_line_row(&overscroll_line_state(), 63);
    assert!(row.contains("~/jcode"), "{row}");
    assert!(row.contains("GPT-5.6 Sol"), "{row}");
    assert!(
        !row.contains("74k/256k"),
        "token counts dropped first: {row}"
    );
    assert!(!row.contains("OAuth"), "auth dropped early: {row}");
}

#[test]
fn overscroll_model_is_pink() {
    let _lock = viewport_snapshot_test_lock();
    clear_flicker_frame_history_for_tests();
    let state = overscroll_line_state();
    let mut terminal = Terminal::new(TestBackend::new(200, 18)).expect("test terminal");
    terminal
        .draw(|frame| crate::tui::ui::draw(frame, &state))
        .expect("overscroll frame");
    let buf = terminal.backend().buffer();
    let y = buf.area.height - 1;
    let row: String = (0..buf.area.width)
        .map(|x| buf[(x, y)].symbol().to_string())
        .collect();
    let col = row
        .find("GPT-5.6 Sol")
        .map(|byte| row[..byte].chars().count() as u16)
        .expect("model on overscroll line");
    assert_eq!(
        buf[(col, y)].fg,
        ratatui::style::Color::Rgb(255, 135, 200),
        "{row}"
    );
}

/// End to end: with widgets on and the status line pinned, the frame
/// shows each status-line fact once (on the line) while the widgets carry
/// only the detail behind it.
#[test]
fn widgets_render_detail_layer_without_repeating_status_line_facts() {
    let _lock = viewport_snapshot_test_lock();
    clear_flicker_frame_history_for_tests();
    crate::tui::info_widget::clear_widget_placements_for_tests();
    let mut state = overscroll_line_state();
    state.suppress_info_widgets = false;
    state.info_widget_data.session_name = Some("sauropod".to_string());
    state.info_widget_data.tokens_per_second = Some(62.0);
    state.info_widget_data.context_info = Some(crate::prompt::ContextInfo {
        system_prompt_chars: 16_000,
        tool_defs_chars: 36_000,
        user_messages_chars: 40_000,
        assistant_messages_chars: 48_000,
        tool_results_chars: 164_000,
        total_chars: 304_000,
        ..Default::default()
    });
    if let Some(git) = state.info_widget_data.git_info.as_mut() {
        git.dirty_files = vec![
            info_widget::DirtyFile::new('M', "crates/a/src/agent/turn_execution.rs"),
            info_widget::DirtyFile::new('?', "notes.md"),
        ];
        git.dirty_total = 2;
    }

    let mut terminal = Terminal::new(TestBackend::new(160, 40)).expect("test terminal");
    // The Updates box is a widget box too and lists the build's latest commit
    // subjects; pin it empty so a subject like "… main …" cannot fail the scan.
    crate::tui::ui::header::set_unseen_changelog_entries_override_for_tests(Some(Vec::new()));
    for _ in 0..3 {
        terminal
            .draw(|frame| crate::tui::ui::draw(frame, &state))
            .expect("frame");
    }
    crate::tui::ui::header::set_unseen_changelog_entries_override_for_tests(None);
    let rows = buffer_rows(&terminal);
    let frame = rows.join("\n");
    // Text inside rounded widget boxes only: every column from a box's left
    // edge `╭`/`│`/`╰` to its right edge on the same row.
    let widgets: String = rows
        .iter()
        .filter_map(|row| {
            let start = row.rfind(['╭', '│', '╰'])?;
            let head = &row[..start];
            let left = head.rfind(['╭', '│', '╰'])?;
            Some(row[left..].to_string())
        })
        .collect::<Vec<_>>()
        .join("\n");
    assert!(!widgets.is_empty(), "expected widgets:\n{frame}");

    assert!(
        widgets.contains("turn_execution.rs"),
        "Changes detail:\n{frame}"
    );
    assert!(widgets.contains("62 tok/s"), "Runtime detail:\n{frame}");
    for owned in [
        "GPT-5.6", "74k", "256k", "29%", "OAuth", "OpenAI", "main", "~3", "↑1",
    ] {
        assert!(
            !widgets.contains(owned),
            "{owned:?} is a status-line fact and must not repeat in widgets:\n{frame}"
        );
    }
    let line = rows
        .iter()
        .rev()
        .find(|r| r.contains("~/jcode"))
        .expect("status line");
    for owned in ["GPT-5.6 Sol", "74k/256k", "29%", "OAuth", "main"] {
        assert!(line.contains(owned), "line owns {owned:?}: {line}");
    }
}
