use super::*;

fn message(role: &str, content: String) -> DisplayMessage {
    DisplayMessage {
        role: role.into(),
        content,
        tool_calls: vec![],
        duration_secs: None,
        title: None,
        tool_data: None,
    }
}

fn transcript(turns: usize) -> Vec<DisplayMessage> {
    (0..turns).flat_map(|n| [
        message("user", format!("Review request {n}: Unicode 界 👩‍💻 e\u{301} and the saved transcript wrapping behavior.\nPreserve all metadata.")),
        message("assistant", format!("## Review {n}\n\nThe **rendering pipeline** preserves `selection` offsets and [links](https://example.com). {}\n\n- Keep prompt numbers\n- Preserve scroll position\n\n> A quoted explanation with Unicode 界.\n\n```rust\nfn example_{n}(value: usize) -> usize {{\n    value.saturating_add(1)\n}}\n```\n\n| Field | Meaning |\n| --- | --- |\n| rows | wrapped output |\n", "A realistic paragraph of markdown with inline code and wrapping. ".repeat(5))),
    ]).collect()
}

fn old(
    messages: &[DisplayMessage],
    wide: u16,
    narrow: u16,
    budget: usize,
    mode: crate::config::DiffDisplayMode,
) -> (PreparedMessages, bool) {
    let prepared = prepare_history_preview(messages, 41, wide, mode);
    let overflow = prepared.wrapped_lines.len() > budget;
    if overflow {
        (prepare_history_preview(messages, 41, narrow, mode), true)
    } else {
        (prepared, false)
    }
}

fn assert_same(a: &PreparedMessages, b: &PreparedMessages) {
    assert_eq!(a.wrapped_lines, b.wrapped_lines);
    assert_eq!(a.wrapped_plain_lines, b.wrapped_plain_lines);
    assert_eq!(a.raw_plain_lines, b.raw_plain_lines);
    assert_eq!(a.wrapped_copy_offsets, b.wrapped_copy_offsets);
    let maps = |p: &PreparedMessages| {
        p.wrapped_line_map
            .iter()
            .map(|m| (m.raw_line, m.start_col, m.end_col))
            .collect::<Vec<_>>()
    };
    assert_eq!(maps(a), maps(b));
    assert_eq!(a.wrapped_user_indices, b.wrapped_user_indices);
    assert_eq!(a.wrapped_user_prompt_starts, b.wrapped_user_prompt_starts);
    assert_eq!(a.wrapped_user_prompt_ends, b.wrapped_user_prompt_ends);
    assert_eq!(a.user_prompt_texts, b.user_prompt_texts);
    assert_eq!(a.message_boundaries, b.message_boundaries);
    assert_eq!(
        format!("{:?}", a.copy_targets),
        format!("{:?}", b.copy_targets)
    );
    assert_eq!(
        format!("{:?}", a.edit_tool_ranges),
        format!("{:?}", b.edit_tool_ranges)
    );
    assert_eq!(a.image_regions.len(), b.image_regions.len());
    assert_eq!(a.mermaid_pending_epoch, b.mermaid_pending_epoch);
}

fn tool_fixture() -> Vec<DisplayMessage> {
    let mut messages = transcript(1);
    let mut edit = message("tool", "Successfully replaced text in src/main.rs".into());
    edit.tool_data = Some(crate::message::ToolCall {
        id: "edit-preview".into(),
        name: "edit".into(),
        input: serde_json::json!({"file_path":"src/main.rs", "old_string":"let old = 1;", "new_string":"let new = 2;"}),
        intent: None,
        thought_signature: None,
    });
    messages.push(edit);
    messages.push(message("system", "Saved context".into()));
    messages.push(message("assistant", "Error: failed command".into()));
    messages
}

#[test]
fn viewport_preview_matches_old_algorithm_at_boundaries() {
    use crate::config::DiffDisplayMode::*;
    let fixtures = [
        vec![],
        vec![message("user", "".into())],
        vec![message("user", "long prompt 界 ".repeat(50))],
        transcript(3),
        tool_fixture(),
    ];
    for messages in fixtures {
        for mode in [Off, Inline, FullInline, File] {
            for (wide, narrow) in [
                (0, 0),
                (1, 0),
                (2, 1),
                (20, 19),
                (80, 79),
                (120, 119),
                (80, 80),
            ] {
                let rows = prepare_history_preview(&messages, 41, wide, mode)
                    .wrapped_lines
                    .len();
                for budget in [
                    0,
                    1,
                    15,
                    rows.saturating_sub(1),
                    rows,
                    rows.saturating_add(1),
                    usize::MAX,
                ] {
                    let (expected, expected_overflow) = old(&messages, wide, narrow, budget, mode);
                    let (actual, overflow) = prepare_history_preview_for_viewport(
                        &messages, 41, wide, narrow, mode, budget,
                    );
                    assert_eq!(overflow, expected_overflow, "wide={wide}, budget={budget}");
                    assert_same(&actual, &expected);
                }
            }
        }
    }
}

#[test]
fn viewport_preview_stops_before_full_wide_render() {
    let messages = transcript(30);
    let mode = crate::config::DiffDisplayMode::Inline;
    assert!(prepare_history_preview_bounded(&messages, 41, 80, mode, Some(25)).is_none());
    assert!(prepare_history_preview_bounded(&messages[..1], 41, 80, mode, Some(100)).is_some());
}

#[test]
fn viewport_preview_detects_overflow_after_wrapping() {
    let messages = vec![message("user", "long words requiring wrapping ".repeat(30))];
    let mode = crate::config::DiffDisplayMode::Inline;
    // One raw prompt line fits the budget, but its wrapped output does not.
    let wide = prepare_history_preview_bounded(&messages, 41, 30, mode, Some(5))
        .expect("raw lower bound must not reject this prompt");
    assert!(wide.wrapped_lines.len() > 5);
    let (actual, overflow) = prepare_history_preview_for_viewport(&messages, 41, 30, 29, mode, 5);
    assert!(overflow);
    assert_same(&actual, &prepare_history_preview(&messages, 41, 29, mode));
}

/// Run alone with --ignored --nocapture --test-threads=1. Clear reconstructible
/// message/highlight and assistant auxiliary caches before every timed sample.
#[test]
#[ignore = "manual cold preview latency measurement"]
fn viewport_preview_cold_benchmark() {
    fn cold() {
        jcode_tui_messages::clear_message_cache();
        jcode_tui_markdown::clear_highlight_cache();
        *assistant_aux_cache().lock().unwrap() = AssistantAuxCacheState::default();
    }
    let messages = transcript(150);
    let mode = crate::config::DiffDisplayMode::Inline;
    let mut old_times = Vec::new();
    let mut new_times = Vec::new();
    for sample in 0..12 {
        for new in if sample % 2 == 0 {
            [false, true]
        } else {
            [true, false]
        } {
            cold();
            let start = Instant::now();
            let result = if new {
                prepare_history_preview_for_viewport(&messages, 41, 100, 99, mode, 35)
            } else {
                old(&messages, 100, 99, 35, mode)
            };
            std::hint::black_box(&result);
            let elapsed = start.elapsed();
            if new {
                new_times.push(elapsed);
            } else {
                old_times.push(elapsed);
            }
        }
    }
    old_times.sort();
    new_times.sort();
    eprintln!(
        "cold markdown preview: {} messages, old median {:?}, viewport median {:?}, ratio {:.3}",
        messages.len(),
        old_times[6],
        new_times[6],
        new_times[6].as_secs_f64() / old_times[6].as_secs_f64()
    );
}
