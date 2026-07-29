use super::*;

#[test]
fn test_render_rounded_box_sides_aligned() {
    let content = vec![
        Line::from("short"),
        Line::from("a longer line of text here"),
        Line::from("mid"),
    ];
    let style = Style::default();
    let lines = render_rounded_box("title", content, 40, style);
    assert!(lines.len() >= 5);
    let top_width = lines[0].width();
    let bottom_width = lines[lines.len() - 1].width();
    assert_eq!(
        top_width, bottom_width,
        "top and bottom borders must be same width: top={}, bottom={}",
        top_width, bottom_width
    );
    for (i, line) in lines.iter().enumerate() {
        assert_eq!(
            line.width(),
            top_width,
            "line {} has width {} but expected {} (content: {:?})",
            i,
            line.width(),
            top_width,
            line.spans
                .iter()
                .map(|s| s.content.as_ref())
                .collect::<Vec<_>>()
        );
    }
}

#[test]
fn test_render_rounded_box_emoji_title_aligned() {
    let content = vec![
        Line::from("memory content line one"),
        Line::from("memory content line two"),
    ];
    let style = Style::default();
    let lines = render_rounded_box("🧠 recalled 2 memories", content, 50, style);
    assert!(lines.len() >= 4);
    let top_width = lines[0].width();
    let bottom_width = lines[lines.len() - 1].width();
    assert_eq!(
        top_width, bottom_width,
        "emoji title: top={}, bottom={}",
        top_width, bottom_width
    );
    for (i, line) in lines.iter().enumerate() {
        assert_eq!(
            line.width(),
            top_width,
            "emoji title: line {} width {} != expected {}",
            i,
            line.width(),
            top_width
        );
    }
}

#[test]
fn test_render_rounded_box_long_title_keeps_body_width_in_sync() {
    let content = vec![Line::from("tiny")];
    let style = Style::default();
    let lines = render_rounded_box("✓ bg bash completed · 6150794bik", content, 24, style);

    assert!(lines.len() >= 3);
    let top_width = lines[0].width();
    assert_eq!(top_width, 24, "box should respect max width");
    for (i, line) in lines.iter().enumerate() {
        assert_eq!(
            line.width(),
            top_width,
            "long title: line {} width {} != expected {}",
            i,
            line.width(),
            top_width
        );
    }
}

#[test]
fn test_render_direct_message_as_compact_agent_row() {
    crate::tui::markdown::set_center_code_blocks(false);
    let msg = DisplayMessage::swarm("DM from fox", "Can you take parser tests?");

    let lines = render_swarm_message(&msg, 80, crate::config::DiffDisplayMode::Off);
    let rendered: Vec<String> = lines.iter().map(extract_line_text).collect();

    assert_eq!(rendered, vec!["🦊 Can you take parser tests?"]);
    assert!(
        rendered
            .iter()
            .all(|line| !line.contains('│') && !line.contains('✉')),
        "direct messages should render without rails or type icons: {:?}",
        rendered
    );
}

#[test]
fn test_render_swarm_message_matches_exact_compact_snapshot() {
    crate::tui::markdown::set_center_code_blocks(false);
    let msg = DisplayMessage::swarm("Task · sheep", "Implement compaction asymptotic fixes");

    let lines = render_swarm_message(&msg, 80, crate::config::DiffDisplayMode::Off);
    let rendered: Vec<String> = lines.iter().map(extract_line_text).collect();

    assert_eq!(
        rendered,
        vec!["🐑 Implement compaction asymptotic fixes".to_string()]
    );
}

#[test]
fn test_render_swarm_await_as_compact_rail_free_summary() {
    crate::tui::markdown::set_center_code_blocks(false);
    let msg = DisplayMessage::swarm("🐝 Swarm await", "✓ 2/2");

    let lines = render_swarm_message(&msg, 80, crate::config::DiffDisplayMode::Off);
    let rendered: Vec<String> = lines.iter().map(extract_line_text).collect();

    assert_eq!(rendered, vec!["🐝 ✓ 2/2"]);
    assert!(rendered.iter().all(|line| !line.contains('│')));
}

#[test]
fn test_render_swarm_await_wake_message_as_compact_rail_free_summary() {
    crate::tui::markdown::set_center_code_blocks(false);
    let msg = DisplayMessage::background_task(
        "🐝 **Swarm await finished**\n\nAll members done. All 1 members are done: sabertooth\n\nMember statuses:\n  ✓ sabertooth (completed)\n\nCompletion reports:\n\n--- sabertooth (completed) ---\nAwait UI demo complete."
            .to_string(),
    );

    let lines = render_background_task_message(&msg, 80, crate::config::DiffDisplayMode::Off);
    let rendered: Vec<String> = lines.iter().map(extract_line_text).collect();

    assert_eq!(rendered, vec!["🐝 ✓ 1/1"]);
    assert!(rendered.iter().all(|line| !line.contains('│')));
    assert!(!rendered.join("\n").contains("sabertooth"));
}

#[test]
fn test_render_swarm_message_trims_extra_newlines() {
    crate::tui::markdown::set_center_code_blocks(false);
    let msg = DisplayMessage::swarm("Broadcast · coordinator", "\n\nPlan updated\n\n");

    let lines = render_swarm_message(&msg, 80, crate::config::DiffDisplayMode::Off);
    let rendered: Vec<String> = lines.iter().map(extract_line_text).collect();

    assert_eq!(rendered, vec!["💫 📣 Plan updated"]);
    assert!(rendered.iter().all(|line| !line.contains('│')));
}

#[test]
fn test_render_channel_and_shared_context_as_compact_agent_rows() {
    crate::tui::markdown::set_center_code_blocks(false);

    let channel = DisplayMessage::swarm("#dev · fox", "Can someone review this?");
    let context = DisplayMessage::swarm("Shared context · fox", "branch = feature/auth");

    let channel_lines = render_swarm_message(&channel, 80, crate::config::DiffDisplayMode::Off)
        .iter()
        .map(extract_line_text)
        .collect::<Vec<_>>();
    let context_lines = render_swarm_message(&context, 80, crate::config::DiffDisplayMode::Off)
        .iter()
        .map(extract_line_text)
        .collect::<Vec<_>>();

    assert_eq!(channel_lines, vec!["🦊 #dev · Can someone review this?"]);
    assert_eq!(context_lines, vec!["🦊 🧠 branch · feature/auth"]);
}

#[test]
fn test_render_file_activity_as_collapsible_compact_row() {
    crate::tui::markdown::set_center_code_blocks(false);
    let content = jcode_tui_messages::encode_collapsible_swarm_content(
        "src/auth.rs · modified",
        "```text\n-old\n+new\n```",
    );
    let msg = DisplayMessage::swarm("File activity · fox", content);

    let rendered = render_swarm_message(&msg, 80, crate::config::DiffDisplayMode::Off)
        .iter()
        .map(extract_line_text)
        .collect::<Vec<_>>();

    assert_eq!(rendered, vec!["🦊 ✎ src/auth.rs · modified  ▸ diff"]);
    assert!(rendered.iter().all(|line| !line.contains('│')));
}

#[test]
fn test_render_file_conflict_places_warning_before_agent() {
    crate::tui::markdown::set_center_code_blocks(false);
    let msg = DisplayMessage::swarm("File conflict · fox", "src/auth.rs · concurrent edits");

    let rendered = render_swarm_message(&msg, 80, crate::config::DiffDisplayMode::Off)
        .iter()
        .map(extract_line_text)
        .collect::<Vec<_>>();

    assert_eq!(rendered, vec!["⚠ 🦊 src/auth.rs · concurrent edits"]);
}

#[test]
fn test_render_swarm_message_uses_agent_emoji_for_assignments() {
    crate::tui::markdown::set_center_code_blocks(false);
    let msg = DisplayMessage::swarm("Task · sheep", "Implement compaction asymptotic fixes");

    let lines = render_swarm_message(&msg, 80, crate::config::DiffDisplayMode::Off);
    let rendered: Vec<String> = lines.iter().map(extract_line_text).collect();

    assert_eq!(rendered, vec!["🐑 Implement compaction asymptotic fixes"]);
}

#[test]
fn test_render_swarm_message_centered_mode_left_aligns_with_shared_padding() {
    let saved = crate::tui::markdown::center_code_blocks();
    crate::tui::markdown::set_center_code_blocks(true);

    let msg = DisplayMessage::swarm("Plan · sheep", "4 items · v1");
    let lines = render_swarm_message(&msg, 80, crate::config::DiffDisplayMode::Off);
    let rendered: Vec<String> = lines.iter().map(extract_line_text).collect();

    assert_eq!(rendered.len(), 1, "expected one compact plan row");

    let header_pad = rendered[0].chars().take_while(|c| *c == ' ').count();
    assert!(
        header_pad > 0,
        "centered swarm header should be padded: {rendered:?}"
    );
    assert_eq!(rendered[0].trim_start(), "🐝 Plan · 4 items · v1");
    assert!(!rendered[0].contains('│'));
    for line in &lines {
        assert_eq!(
            line.alignment,
            Some(ratatui::layout::Alignment::Left),
            "centered swarm lines should be left-aligned after padding"
        );
    }

    crate::tui::markdown::set_center_code_blocks(saved);
}

#[test]
fn test_render_swarm_message_centered_mode_keeps_task_icon_and_padding() {
    let saved = crate::tui::markdown::center_code_blocks();
    crate::tui::markdown::set_center_code_blocks(true);

    let msg = DisplayMessage::swarm("Task · sheep", "Implement compaction asymptotic fixes");
    let lines = render_swarm_message(&msg, 80, crate::config::DiffDisplayMode::Off);
    let rendered: Vec<String> = lines.iter().map(extract_line_text).collect();

    assert!(
        rendered[0].starts_with(' '),
        "centered task header should be padded: {rendered:?}"
    );
    assert_eq!(
        rendered[0].trim_start(),
        "🐑 Implement compaction asymptotic fixes"
    );

    crate::tui::markdown::set_center_code_blocks(saved);
}

#[test]
fn test_render_swarm_message_centered_mode_keeps_file_activity_preview_centered_when_diff_wraps() {
    let saved = crate::tui::markdown::center_code_blocks();
    crate::tui::markdown::set_center_code_blocks(true);

    let msg = DisplayMessage::swarm(
        "File activity · rose",
        "`…/jcode/src/server/comm_sync.rs`

Modified via apply_patch

```text
331-             persist_swarm_state_for(&swarm_id, swarm_state.clone()).await;
331+             persist_swarm_state_for(&swarm_id, swarm_state).await;
```",
    );

    let lines = render_swarm_message(&msg, 120, crate::config::DiffDisplayMode::Off);
    let rendered: Vec<String> = lines.iter().map(extract_line_text).collect();
    let first_pad = rendered[0].chars().take_while(|c| *c == ' ').count();

    assert!(
        first_pad >= 8,
        "centered file activity notification should preserve a visible left gutter: {rendered:?}"
    );
    assert!(
        rendered
            .iter()
            .all(|line| line.is_empty() || line.starts_with(&" ".repeat(first_pad))),
        "wrapped file activity preview should keep one shared left pad: {rendered:?}"
    );
    assert!(
        rendered
            .iter()
            .any(|line| line.contains("persist_swarm_state_for")),
        "expected diff preview to remain visible after wrapping: {rendered:?}"
    );

    crate::tui::markdown::set_center_code_blocks(saved);
}

#[test]
fn test_truncate_line_to_width_uses_display_width() {
    let line = Line::from(Span::raw("🧠 hello world"));
    let truncated = truncate_line_to_width(&line, 8);
    let w = truncated.width();
    assert!(w <= 8, "truncated line display width {} should be <= 8", w);
}

#[test]
fn test_render_memory_tiles_uses_variable_box_widths() {
    let mut tiles = group_into_tiles(vec![
        (
            "preference".to_string(),
            "The user wants the mobile experience to be beautiful, animated, and performant."
                .to_string(),
        ),
        (
            "preference".to_string(),
            "User wants a release cut after testing is complete.".to_string(),
        ),
        ("fact".to_string(), "Jeremy".to_string()),
    ]);
    let border_style = Style::default();
    let text_style = Style::default();

    let preference = tiles.remove(0);
    let fact = tiles.remove(0);

    let preference_plan = choose_memory_tile_span(&preference, 20, 2, 2, border_style, text_style)
        .expect("preference span plan");
    let fact_plan =
        choose_memory_tile_span(&fact, 20, 2, 2, border_style, text_style).expect("fact span plan");
    let preference_width = preference_plan.0.width;
    let fact_width = fact_plan.0.width;
    let narrow_preference = plan_memory_tile(&preference, 20, border_style, text_style)
        .expect("narrow preference plan");
    let chosen_preference = preference_plan.0;

    assert!(
        chosen_preference.height <= narrow_preference.height,
        "expected chosen preference width to be at least as space-efficient as the minimum width: chosen_width={}, chosen_height={}, narrow_height={}",
        preference_width,
        chosen_preference.height,
        narrow_preference.height
    );
    assert!(
        preference_width >= fact_width,
        "expected long preference content to not choose a narrower box than fact: pref={}, fact={}",
        preference_width,
        fact_width
    );
}

#[test]
fn test_render_memory_tiles_allows_boxes_below_other_boxes() {
    let tiles = group_into_tiles(vec![
        (
            "preference".to_string(),
            "The mobile experience should be beautiful, animated, and performant.".to_string(),
        ),
        (
            "preference".to_string(),
            "User prefers quick verification that jcode is up-to-date.".to_string(),
        ),
        ("fact".to_string(), "Jeremy".to_string()),
        (
            "entity".to_string(),
            "Star is a named source providing product strategy input.".to_string(),
        ),
        (
            "correction".to_string(),
            "Assistant incorrectly said it had no memory hits despite existing memories."
                .to_string(),
        ),
    ]);

    let lines = render_memory_tiles(
        &tiles,
        120,
        Style::default(),
        Style::default(),
        Some(Line::from("🧠 recalled 5 memories")),
    );
    let rendered: Vec<String> = lines.iter().map(extract_line_text).collect();

    let correction_idx = rendered
        .iter()
        .position(|line| line.contains(" correction "))
        .expect("correction box present");

    assert!(
        correction_idx > 0,
        "expected correction box to render below first row: {:?}",
        rendered
    );
    assert!(
        rendered
            .iter()
            .skip(1)
            .any(|line| line.contains(" correction ")),
        "expected at least one box to appear on a later visual row: {:?}",
        rendered
    );
}

#[test]
fn test_render_memory_tiles_uses_full_row_width_for_stable_alignment() {
    let tiles = group_into_tiles(vec![
            (
                "fact".to_string(),
                "home.html has a new \"Final Oral Test\" link under Scripts · Memorization"
                    .to_string(),
            ),
            (
                "preference".to_string(),
                "User wants unprofessional demo/chat messages removed or replaced with professional wording for demos."
                    .to_string(),
            ),
            ("entity".to_string(), "User account name is `jeremy`.".to_string()),
            ("note".to_string(), "The number 42".to_string()),
        ]);

    let lines = render_memory_tiles(
        &tiles,
        96,
        Style::default(),
        Style::default(),
        Some(Line::from("🧠 recalled 4 memories")),
    );
    let rendered: Vec<String> = lines.iter().skip(1).map(extract_line_text).collect();

    assert!(
        rendered
            .iter()
            .all(|line| unicode_width::UnicodeWidthStr::width(line.as_str()) == 96),
        "expected each rendered memory row to fill full layout width for stable centering: {:?}",
        rendered
    );
}

#[test]
fn test_parse_memory_display_entries_extracts_updated_at_metadata() {
    let ts = (chrono::Utc::now() - chrono::Duration::hours(2)).to_rfc3339();
    let content = format!(
        "# Memory\n\n## Facts\n1. The build is green\n<!-- updated_at: {} -->\n",
        ts
    );

    let entries = parse_memory_display_entries(&content);
    assert_eq!(entries.len(), 1);
    assert_eq!(entries[0].0, "Facts");
    assert_eq!(entries[0].1.content, "The build is green");
    assert!(entries[0].1.updated_at.is_some());
}

#[test]
fn test_render_memory_tiles_shows_updated_age_line() {
    let tiles = group_into_tiles(vec![(
        "fact".to_string(),
        MemoryTileItem {
            content: "The build is green".to_string(),
            updated_at: Some(chrono::Utc::now() - chrono::Duration::hours(2)),
        },
    )]);

    let lines = render_memory_tiles(
        &tiles,
        60,
        Style::default(),
        Style::default(),
        Some(Line::from("🧠 recalled 1 memory")),
    );
    let rendered: Vec<String> = lines.iter().map(extract_line_text).collect();

    assert!(rendered.iter().any(|line| line.contains("updated 2h ago")));
}

#[test]
fn test_render_memory_tiles_do_not_use_background_tint() {
    let tiles = group_into_tiles(vec![(
        "fact".to_string(),
        MemoryTileItem {
            content: "The build is green".to_string(),
            updated_at: Some(chrono::Utc::now() - chrono::Duration::hours(2)),
        },
    )]);

    let lines = render_memory_tiles(
        &tiles,
        60,
        Style::default(),
        Style::default(),
        Some(Line::from("🧠 recalled 1 memory")),
    );

    assert!(
        lines
            .iter()
            .skip(1)
            .flat_map(|line| line.spans.iter())
            .all(|span| span.style.bg.is_none())
    );
}

#[test]
fn test_plan_memory_tile_wraps_long_updated_age_line() {
    let tiles = group_into_tiles(vec![(
        "fact".to_string(),
        MemoryTileItem {
            content: "The build is green".to_string(),
            updated_at: Some(chrono::Utc::now() - chrono::Duration::days(400)),
        },
    )]);

    let plan = plan_memory_tile(&tiles[0], 20, Style::default(), Style::default())
        .expect("memory tile plan");

    assert!(
        plan.lines.iter().all(|line| line.width() == 20),
        "expected wrapped updated-at lines to preserve tile width: {:?}",
        plan.lines.iter().map(extract_line_text).collect::<Vec<_>>()
    );
}

#[test]
fn test_plan_memory_tile_truncates_long_category_title() {
    let tiles = group_into_tiles(vec![(
        "this category title is unexpectedly very long".to_string(),
        "The build is green".to_string(),
    )]);

    let plan = plan_memory_tile(&tiles[0], 20, Style::default(), Style::default())
        .expect("memory tile plan");

    assert!(
        plan.lines.iter().all(|line| line.width() == 20),
        "expected long category titles to be truncated to tile width: {:?}",
        plan.lines.iter().map(extract_line_text).collect::<Vec<_>>()
    );
}

/// Light-theme adaptation: rendering a full frame and adapting it for a light
/// background must leave every non-Reset foreground readable against its
/// (possibly Reset = white) background. This exercises the same buffer-level
/// hook `ui::draw` applies, but with an explicit mode so it cannot race other
/// tests through the process-global theme mode.
#[test]
fn test_light_theme_adapted_frame_has_readable_contrast() {
    fn channel_lum(c: ratatui::style::Color) -> Option<f32> {
        let (r, g, b) = match c {
            ratatui::style::Color::Rgb(r, g, b) => (r, g, b),
            ratatui::style::Color::Indexed(n) => crate::tui::color_support::indexed_to_rgb(n),
            _ => return None,
        };
        // Perceived luminance approximation.
        Some((0.299 * r as f32 + 0.587 * g as f32 + 0.114 * b as f32) / 255.0)
    }

    let messages = vec![
        DisplayMessage {
            role: "user".into(),
            content: "hello there".into(),
            tool_calls: vec![],
            duration_secs: None,
            title: None,
            tool_data: None,
        },
        DisplayMessage {
            role: "assistant".into(),
            content: "hi! *bold* and `code`".into(),
            tool_calls: vec![],
            duration_secs: None,
            title: None,
            tool_data: None,
        },
    ];
    let state = TestState {
        display_messages: messages,
        input: "next question".into(),
        ..Default::default()
    };

    let backend = ratatui::backend::TestBackend::new(80, 24);
    let mut terminal = ratatui::Terminal::new(backend).expect("test terminal");
    terminal
        .draw(|frame| {
            crate::tui::ui::draw(frame, &state);
            jcode_tui_style::theme_mode::adapt_buffer(
                frame.buffer_mut(),
                jcode_tui_style::ThemeMode::Light,
            );
        })
        .expect("draw");

    let buffer = terminal.backend().buffer();
    let mut checked = 0usize;
    for cell in buffer.content.iter() {
        if cell.symbol().trim().is_empty() {
            continue;
        }
        let Some(fg_lum) = channel_lum(cell.fg) else {
            continue;
        };
        // Reset bg on a light terminal is near-white (lum ~1.0).
        let bg_lum = channel_lum(cell.bg).unwrap_or(1.0);
        assert!(
            (fg_lum - bg_lum).abs() > 0.2,
            "unreadable cell {:?} fg={:?} bg={:?} (fg_lum={fg_lum:.2} bg_lum={bg_lum:.2})",
            cell.symbol(),
            cell.fg,
            cell.bg,
        );
        checked += 1;
    }
    assert!(
        checked > 20,
        "expected to verify a meaningful number of glyph cells, got {checked}"
    );
}

/// End-to-end proof that `[display.colors]` reaches a real rendered frame.
///
/// The unit tests in `jcode-tui-style` cover the substitution in isolation, but
/// the thing a user actually cares about is whether configuring a color changes
/// what the TUI paints. This renders a real frame through `ui::draw`, so it also
/// guards the hook's presence and its ordering relative to the light/dark pass.
#[test]
fn test_configured_palette_recolors_a_real_rendered_frame() {
    fn render() -> ratatui::buffer::Buffer {
        let messages = vec![
            DisplayMessage {
                role: "user".into(),
                content: "hello there".into(),
                tool_calls: vec![],
                duration_secs: None,
                title: None,
                tool_data: None,
            },
            DisplayMessage {
                role: "assistant".into(),
                content: "hi! *bold* and `code`".into(),
                tool_calls: vec![],
                duration_secs: None,
                title: None,
                tool_data: None,
            },
        ];
        let state = TestState {
            display_messages: messages,
            input: "next question".into(),
            ..Default::default()
        };
        let backend = ratatui::backend::TestBackend::new(80, 24);
        let mut terminal = ratatui::Terminal::new(backend).expect("test terminal");
        terminal
            .draw(|frame| crate::tui::ui::draw(frame, &state))
            .expect("draw");
        terminal.backend().buffer().clone()
    }

    // The palette is process-global, so always restore it, even on failure.
    struct Restore;
    impl Drop for Restore {
        fn drop(&mut self) {
            jcode_tui_style::set_palette(jcode_tui_style::Palette::default());
        }
    }
    let _restore = Restore;

    jcode_tui_style::set_palette(jcode_tui_style::Palette::default());
    let baseline = render();

    // Recolor the user role to a color nothing in the default palette is near.
    let mut palette = jcode_tui_style::Palette::default();
    palette.set(jcode_tui_style::Role::User, (250, 40, 200));
    jcode_tui_style::set_palette(palette);
    let configured = render();

    assert_ne!(
        baseline
            .content
            .iter()
            .map(|cell| cell.fg)
            .collect::<Vec<_>>(),
        configured
            .content
            .iter()
            .map(|cell| cell.fg)
            .collect::<Vec<_>>(),
        "configuring a color role should change the rendered frame"
    );

    // Text content must be untouched: this is a recolor, not a relayout.
    let text_of = |buffer: &ratatui::buffer::Buffer| {
        buffer
            .content
            .iter()
            .map(|cell| cell.symbol().to_string())
            .collect::<Vec<_>>()
    };
    assert_eq!(
        text_of(&baseline),
        text_of(&configured),
        "recoloring must not change any rendered text"
    );

    // And the default palette must render identically to no palette at all,
    // which is what keeps existing users' terminals looking the same.
    jcode_tui_style::set_palette(jcode_tui_style::Palette::default());
    assert_eq!(
        baseline,
        render(),
        "the default palette must be a no-op on the rendered frame"
    );
}
