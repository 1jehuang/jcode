use super::*;

#[test]
fn expanded_diff_rows_preserve_source_and_align_continuations() {
    let source = "    let result = compute(\"界界 e\u{301}\", argument);";
    let parsed = ParsedDiffLine {
        kind: DiffLineKind::Add,
        prefix: "9+ ".into(),
        content: source.into(),
        file_path: None,
    };
    let rows = expanded_diff_row(&parsed, Some("rs"), 3, 24);
    assert!(rows.len() > 1);
    assert!(extract_line_text(&rows[0]).starts_with("│   9 +     let"));
    let mut reconstructed = String::new();
    for (index, row) in rows.iter().enumerate() {
        assert!(row.width() <= 24, "{row:?}");
        assert_eq!(row.alignment, Some(Alignment::Left));
        if index > 0 {
            assert!(extract_line_text(row).starts_with("│     ↪ "));
        }
        for span in row.spans.iter().skip(if index == 0 { 2 } else { 1 }) {
            reconstructed.push_str(&span.content);
        }
    }
    assert_eq!(reconstructed, source);
    assert_eq!(rows[0].spans[1].style.fg, Some(diff_add_color()));
}

#[test]
fn expanded_diff_rows_fit_narrow_widths_and_expand_tabs() {
    let parsed = ParsedDiffLine {
        kind: DiffLineKind::Del,
        prefix: "123- ".into(),
        content: "\t界\tvalue".into(),
        file_path: None,
    };
    for width in 0..60 {
        let rows = expanded_diff_row(&parsed, None, 3, width);
        assert!(
            rows.iter().all(|row| row.width() <= width),
            "{width}: {rows:?}"
        );
    }
    let rows = expanded_diff_row(&parsed, None, 3, 60);
    assert_eq!(extract_line_text(&rows[0]), "│ 123 -     界  value");
    assert_eq!(rows[0].spans[1].style.fg, Some(diff_del_color()));
    let blank = ParsedDiffLine {
        content: String::new(),
        ..parsed
    };
    assert_eq!(expanded_diff_row(&blank, None, 3, 60).len(), 1);
}

#[test]
fn inline_diff_header_compacts_directories_before_filename() {
    let path = "/home/user/project/crates/very_long_directory/src/important_file.rs";
    let line = inline_diff_header("╭─", Some((23, 4)), Some(path), 48);
    let text = extract_line_text(&line);
    assert!(text.starts_with("╭─ diff · +23 -4 · "), "{text}");
    assert!(text.ends_with("important_file.rs"), "{text}");
    assert!(text.contains('…'), "{text}");
    assert!(line.width() <= 48);
    assert!(
        line.spans
            .iter()
            .any(|span| span.content == "+23" && span.style.fg == Some(diff_add_color()))
    );
}

#[test]
fn inline_diff_headers_fit_all_cell_budgets() {
    for path in [
        "/home/user/a/very/long/path/to/source.rs",
        "C:\\Users\\user\\project\\very_long_directory\\source.rs",
        "/目录/很长的目录/另一个目录/主文件.rs",
        "/very/long/path/a_filename_too_long_for_the_remaining_space.rs",
    ] {
        for width in 0..100 {
            for (prefix, counts) in [("╭─", Some((123, 45))), ("├─", None)] {
                let line = inline_diff_header(prefix, counts, Some(path), width);
                assert!(line.width() <= width, "width={width}, line={line:?}");
                if width >= 64 {
                    assert!(extract_line_text(&line).ends_with(".rs"), "{line:?}");
                }
            }
        }
    }
}

#[test]
fn render_tool_message_compacts_every_multi_file_diff_header() {
    let first = "/home/user/project/very_long_directory/nested/first.rs";
    let second = "/home/user/project/another_very_long_directory/nested/second.rs";
    let msg = DisplayMessage {
        role: "tool".into(),
        content: "Updated 2 files".into(),
        tool_calls: Vec::new(),
        duration_secs: None,
        title: None,
        tool_data: Some(crate::message::ToolCall {
            id: "compact_multi_file_headers".into(),
            name: "apply_patch".into(),
            input: serde_json::json!({"patch_text": format!(
                "*** Begin Patch\n*** Update File: {first}\n@@\n-old\n+new\n*** Update File: {second}\n@@\n-old\n+new\n*** End Patch\n"
            )}),
            intent: None,
            thought_signature: None,
        }),
    };
    for width in [40, 72, 120] {
        for mode in [
            crate::config::DiffDisplayMode::Inline,
            crate::config::DiffDisplayMode::FullInline,
        ] {
            let lines = render_tool_message(&msg, width, mode);
            let headers: Vec<_> = lines
                .iter()
                .filter(|line| {
                    let text = extract_line_text(line);
                    text.trim_start().starts_with("╭─ diff")
                        || text.trim_start().starts_with("├─ diff")
                })
                .collect();
            assert_eq!(headers.len(), 2, "{lines:?}");
            for (header, filename) in headers.iter().zip(["first.rs", "second.rs"]) {
                assert!(header.width() <= width as usize, "{header:?}");
                assert!(extract_line_text(header).ends_with(filename), "{header:?}");
            }
        }
    }
}

#[test]
fn oauth_login_progress_renderer_keeps_short_link_and_hides_full_details() {
    let url =
        "https://example.invalid/authorize?state=private-state&code_challenge=private-challenge";
    let summary = format!(
        "✓ Sign-in link ready\n◌ Waiting for browser approval\n[Open OpenAI sign-in]({url})\nPaste callback URL here · /cancel"
    );
    let mut msg = DisplayMessage::system(jcode_tui_messages::encode_collapsible_swarm_content(
        "Sign-in details",
        &format!(
            "{summary}\n\n<!-- oauth-details -->\n\nFull sign-in URL:\n{url}\n\nManual fallback details"
        ),
    ));
    msg.title = Some("OpenAI login".to_string());
    for width in [40, 80, 120] {
        let lines = render_system_message(&msg, width, crate::config::DiffDisplayMode::Off);
        let text = lines
            .iter()
            .map(extract_line_text)
            .collect::<Vec<_>>()
            .join("\n");
        assert!(text.contains("Sign-in link ready"));
        assert!(text.contains("Waiting for browser approval"));
        assert!(text.contains("Open OpenAI sign-in"));
        assert!(text.contains(SWARM_EXPAND_BADGE));
        assert!(!text.contains("private-state"));
        assert!(!text.contains("Manual fallback details"));
        assert!(lines.iter().all(|line| line.width() <= width as usize));
    }
    msg.content = jcode_tui_messages::toggle_collapsible_swarm_content(&msg.content).unwrap();
    let lines = render_system_message(&msg, 120, crate::config::DiffDisplayMode::Off);
    let text = lines
        .iter()
        .map(extract_line_text)
        .collect::<Vec<_>>()
        .join("\n");
    assert!(text.contains("private-state"));
    assert!(text.contains("Manual fallback details"));
    assert!(text.contains(SWARM_COLLAPSE_BADGE));
}

fn extract_line_text(line: &Line<'_>) -> String {
    line.spans
        .iter()
        .map(|span| span.content.as_ref())
        .collect::<String>()
}

#[test]
fn usage_card_fits_content_on_wide_terminals() {
    let msg =
        DisplayMessage::system("# Updated\n+ OpenAI - available\n  Banked resets: 12 available");
    let render = |width| render_usage_message(&msg, width, crate::config::DiffDisplayMode::Off);
    let lines = render(240);
    assert_eq!(lines, render(120));
    assert_eq!(lines, render(160));
    assert_eq!(
        lines[0].width(),
        "  Banked resets: 12 available".width() + 4
    );
    assert!(lines.iter().all(|line| line.width() == lines[0].width()));
    assert!(extract_line_text(&lines[0]).contains("Usage"));
    assert!(extract_line_text(lines.last().unwrap()).starts_with('╰'));
}

#[test]
fn usage_card_compacts_padded_labels_in_narrow_terminals() {
    let msg = DisplayMessage::system(
        "  7-day window              : ████████░░░░░░  59% · resets in 5d 4h",
    );
    let lines = render_usage_message(&msg, 36, crate::config::DiffDisplayMode::Off);
    let rows: Vec<_> = lines.iter().map(extract_line_text).collect();
    assert!(
        rows.iter()
            .any(|row| row.contains("7-day window:") && row.contains("59%")),
        "{rows:#?}"
    );
    assert!(rows.iter().any(|row| row.contains("resets in 5d 4h")));
    assert!(lines.iter().all(|line| line.width() <= 36));
}

#[test]
fn usage_card_never_overflows_even_tiny_terminals() {
    let mut msg = DisplayMessage::system(
        "# Usage updated\n+ 工作账户 e\u{301} - available\n  Weekly: █████░░░░░ 50% · resets in 2h 5m\n  Session resets: 1 available\n\n! Claude - error\n  error: unavailable",
    );
    msg.title = Some("使用情况 with a very long title".into());
    for width in 0..=120 {
        let lines = render_usage_message(&msg, width, crate::config::DiffDisplayMode::Off);
        assert_eq!(lines.is_empty(), width == 0, "width {width}");
        for line in &lines {
            assert!(
                line.width() <= usize::from(width),
                "width {width}: {line:?}"
            );
        }
        if width >= 10 {
            assert!(lines.iter().all(|line| line.width() == lines[0].width()));
        }
    }
}

#[test]
fn usage_card_preserves_unicode_and_wrapped_metadata() {
    let msg = DisplayMessage::system(
        "+ 工作账户 e\u{301} - available\n  Weekly: █████░░░░░ 50% · resets in 2h 5m\n  Session resets: 1 available",
    );
    for width in [10, 18, 20, 32, 40, 60, 120] {
        let lines = render_usage_message(&msg, width, crate::config::DiffDisplayMode::Off);
        let body: String = lines[1..lines.len() - 1]
            .iter()
            .map(|line| {
                let text = extract_line_text(line);
                text.strip_prefix("│ ")
                    .unwrap()
                    .strip_suffix(" │")
                    .unwrap()
                    .to_owned()
            })
            .collect();
        let body = without_whitespace(&body);
        for expected in [
            "工作账户e\u{301}",
            "available",
            "50%",
            "resetsin2h5m",
            "Sessionresets:1available",
        ] {
            assert!(body.contains(expected), "width {width}: {body}");
        }
    }
    assert_eq!(
        msg.content.matches(" - available").count(),
        1,
        "report protocol is unchanged"
    );
}

#[test]
fn usage_card_styles_heading_and_account_statuses() {
    use ratatui::style::Modifier;
    let msg = DisplayMessage::system(
        "# Updated\n+ OpenAI - available\n~ Work - 70% used\n! Claude - hard limit\n! Other - error\nProvider - no data",
    );
    let lines = render_usage_message(&msg, 120, crate::config::DiffDisplayMode::Off);
    let spans: Vec<_> = lines.iter().flat_map(|line| &line.spans).collect();
    for name in ["Updated", "OpenAI", "Work", "Claude", "Other"] {
        let span = spans.iter().find(|span| span.content == name).unwrap();
        assert_eq!(span.style.fg, Some(Color::White));
        assert!(span.style.add_modifier.contains(Modifier::BOLD));
    }
    for (status, color) in [
        (" · available", rgb(100, 220, 170)),
        (" · 70% used", rgb(255, 200, 100)),
        (" · hard limit", Color::Red),
        (" · error", Color::Red),
    ] {
        assert!(
            spans
                .iter()
                .any(|span| span.content == status && span.style.fg == Some(color))
        );
    }
    assert!(
        spans
            .iter()
            .any(|span| span.content == "Provider - no data" && span.style.fg == Some(dim_color()))
    );
}

#[test]
fn usage_card_keeps_narrow_gauges_and_words_together() {
    let msg = DisplayMessage::system(
        "+ OpenAI - available\n  5-hour window: ███████░░░░░░░ 50% · resets in 2h 5m\n\n~ Claude - 70% used",
    );
    for width in [20, 40, 60] {
        let lines = render_usage_message(&msg, width, crate::config::DiffDisplayMode::Off);
        let texts: Vec<_> = lines.iter().map(extract_line_text).collect();
        let gauges: Vec<_> = texts.iter().filter(|text| text.contains('█')).collect();
        assert_eq!(gauges.len(), 1, "width {width}: {texts:?}");
        assert!(gauges[0].contains("50%"), "width {width}: {texts:?}");
        assert!(texts.iter().any(|text| text.contains("available")));
        assert!(texts.iter().any(|text| text.contains("resets")));
        assert!(texts.iter().any(|text| text.contains("──────")));
        if width == 40 {
            assert!(!gauges[0].contains("resets"));
        }
    }
}

#[test]
fn usage_card_compacts_long_summary_without_mutating_report() {
    let report =
        "# Showing cached usage while refreshing all connected accounts\n+ OpenAI - available";
    let msg = DisplayMessage::system(report);
    let lines = render_usage_message(&msg, 20, crate::config::DiffDisplayMode::Off);
    assert!(extract_line_text(&lines[1]).contains('…'));
    assert!(extract_line_text(&lines[2]).contains("OpenAI"));
    assert_eq!(msg.content, report);
}

#[test]
fn usage_card_colors_utilization_and_dims_empty_bars_and_countdowns() {
    use ratatui::style::Modifier;
    for (percent, color) in [
        (0, rgb(100, 220, 170)),
        (69, rgb(100, 220, 170)),
        (70, rgb(255, 200, 100)),
        (89, rgb(255, 200, 100)),
        (90, Color::Red),
        (100, Color::Red),
    ] {
        let bar = if percent == 0 {
            "░░░░"
        } else {
            "██░░"
        };
        let msg = DisplayMessage::system(format!("  Weekly: {bar} {percent}% · resets in 2h"));
        let lines = render_usage_message(&msg, 120, crate::config::DiffDisplayMode::Off);
        let spans: Vec<_> = lines.iter().flat_map(|line| &line.spans).collect();
        let value = spans
            .iter()
            .find(|span| span.content == format!(" {percent}%"))
            .unwrap();
        assert_eq!(value.style.fg, Some(color));
        assert!(value.style.add_modifier.contains(Modifier::BOLD));
        for span in spans
            .iter()
            .filter(|span| span.content.contains('░') || span.content.contains("resets in"))
        {
            assert_eq!(span.style.fg, Some(dim_color()));
        }
        if percent > 0 {
            assert!(
                spans
                    .iter()
                    .any(|span| span.content == "██" && span.style.fg == Some(color))
            );
        }
    }
}

#[test]
fn usage_card_highlights_reset_counts_and_handles_empty_reports() {
    use ratatui::style::Modifier;
    for label in ["Banked", "Session"] {
        for count in [0_u64, 1, 12, u64::MAX] {
            let msg =
                DisplayMessage::system(format!("  {label} resets: {count} available · next in 2d"));
            let lines = render_usage_message(&msg, 120, crate::config::DiffDisplayMode::Off);
            let spans: Vec<_> = lines.iter().flat_map(|line| &line.spans).collect();
            let number = spans
                .iter()
                .find(|span| span.content == count.to_string())
                .unwrap();
            assert!(number.style.add_modifier.contains(Modifier::BOLD));
            assert_eq!(
                number.style.fg,
                Some(if count == 0 {
                    dim_color()
                } else {
                    rgb(100, 220, 170)
                })
            );
            assert!(
                spans.iter().any(|span| span.content.contains("next in 2d")
                    && span.style.fg == Some(dim_color()))
            );
        }
    }
    for content in ["", "\n", "x"] {
        let mut msg = DisplayMessage::system(content);
        msg.title = Some("U".into());
        let lines = render_usage_message(&msg, 120, crate::config::DiffDisplayMode::Off);
        assert!(!lines.is_empty());
        assert!(lines.iter().all(|line| line.width() == lines[0].width()));
    }
}

fn without_whitespace(text: &str) -> String {
    text.chars().filter(|ch| !ch.is_whitespace()).collect()
}

fn leading_spaces(text: &str) -> usize {
    text.chars().take_while(|c| *c == ' ').count()
}

fn system_glyph_env_lock() -> std::sync::MutexGuard<'static, ()> {
    use std::sync::{Mutex, OnceLock};

    static LOCK: OnceLock<Mutex<()>> = OnceLock::new();
    LOCK.get_or_init(|| Mutex::new(()))
        .lock()
        .unwrap_or_else(|poisoned| poisoned.into_inner())
}

#[test]
fn successful_reload_notice_hides_internal_directive() {
    for content in [
        "[auto] Reload succeeded (old → new-dirty). Continue immediately from where you left off.",
        "[auto] Reload complete. Resuming your task.\n\nContinue the user's latest request.\nBuild: old → new.",
        "Reload complete. Resuming your task.\nBuild: old → new.",
    ] {
        let msg = DisplayMessage::system(content);
        for width in [1, 10, 30, 80, 120] {
            let lines = render_system_message(&msg, width, crate::config::DiffDisplayMode::Off);
            assert_eq!(lines.len(), 1);
            assert!(lines[0].width() <= width as usize);
            if width >= 80 {
                assert_eq!(
                    extract_line_text(&lines[0]).trim(),
                    "Reload complete. Resuming your task."
                );
            }
        }
        assert_eq!(msg.content, content);
    }
}

#[test]
fn transient_system_notices_are_single_line_without_changing_content() {
    let notices = [
        "[auto] Your session was interrupted by a server reload while a tool was running. The\ntool was aborted and results may be incomplete. Continue exactly where you left off.",
        "Reload complete - continuing because a recovery directive was pending.",
        "Stream read timeout: no data received for 180 seconds",
        "📡 Network appears offline - waiting to retry automatically. listening for Linux\nnetlink changes via `ip monitor`; also verifying with reconnect probes",
        "✓ Retrying continuation... (system message)",
        "An ordinary status notice that should never wrap even in a narrow terminal.",
        "[auto] Continue the task.\nKeep the todo up to date.",
    ];
    for content in notices {
        let msg = DisplayMessage::system(content);
        for width in [1, 10, 30, 80, 120] {
            let lines = render_system_message(&msg, width, crate::config::DiffDisplayMode::Off);
            assert_eq!(lines.len(), 1, "{content:?} at width {width}");
            assert!(lines[0].width() <= width as usize);
            assert!(!extract_line_text(&lines[0]).contains('\n'));
        }
        assert_eq!(
            msg.content, content,
            "display must not rewrite agent instructions"
        );
    }
    let msg = DisplayMessage::system(notices[0]);
    let lines = render_system_message(&msg, 80, crate::config::DiffDisplayMode::Off);
    assert_eq!(
        extract_line_text(&lines[0]).trim(),
        "Server reloaded; resuming interrupted work."
    );
}

#[test]
fn ordinary_multiline_and_markdown_notices_are_one_terminal_row() {
    let saved = markdown::center_code_blocks();
    for centered in [false, true] {
        markdown::set_center_code_blocks(centered);
        for content in [
            "Header line\n  indented detail\n\nNext block",
            "**Status updated**\n- 工作账户 e\u{301} reconnected\n- More detail that must not wrap",
            "Using `some/very/long/path.rs` with **formatting** and a long status message",
            "\u{1b}[35mUpdated\u{1b}[0m\n\tMore details",
        ] {
            let msg = DisplayMessage::system(content);
            for width in 0..=120 {
                let lines = render_system_message(&msg, width, crate::config::DiffDisplayMode::Off);
                assert_eq!(
                    lines.len(),
                    usize::from(width != 0),
                    "{content:?} at {width}"
                );
                for line in &lines {
                    assert!(line.width() <= width as usize, "{line:?} at {width}");
                    assert!(!extract_line_text(line).contains(['\n', '\t', '\u{1b}']));
                }
                assert_eq!(
                    msg.content, content,
                    "display must not rewrite stored content"
                );
            }
        }
    }
    markdown::set_center_code_blocks(saved);
}

#[test]
fn file_activity_is_neutral_and_one_row_including_legacy_titles() {
    let saved = markdown::center_code_blocks();
    let content = jcode_tui_messages::encode_collapsible_swarm_content(
        "…/src/工作账户/e\u{301}.rs · Edited lines 323-348 (1 occurrence)",
        "Intent: fix the parser\n\n```text\n323- old line\n323+ new line\n```",
    );
    for centered in [false, true] {
        markdown::set_center_code_blocks(centered);
        for title in ["File activity · moss", "File activity"] {
            let msg = DisplayMessage::swarm(title, &content);
            for width in 0..=120 {
                let lines = render_swarm_message(&msg, width, crate::config::DiffDisplayMode::Off);
                assert_eq!(lines.len(), usize::from(width != 0), "{title} at {width}");
                for line in &lines {
                    assert!(line.width() <= width as usize, "{line:?} at {width}");
                    let text = extract_line_text(line);
                    assert!(!text.contains("Intent:"));
                    assert!(!text.contains("old line"));
                    for span in &line.spans {
                        if !span.content.trim().is_empty() {
                            assert_eq!(span.style.fg, Some(dim_color()), "{span:?}");
                        }
                    }
                    if width >= 80 {
                        assert!(text.contains(SWARM_DIFF_EXPAND_BADGE));
                        assert!(text.trim_end().ends_with(SWARM_DIFF_EXPAND_BADGE));
                    }
                }
            }
        }
    }
    markdown::set_center_code_blocks(saved);
}

#[test]
fn file_activity_explicit_expansion_preserves_diff_details() {
    let content = jcode_tui_messages::encode_collapsible_swarm_content(
        "src/parser.rs · Edited lines 323-348",
        "Intent: fix the parser\n\n```text\n323- old line\n323+ new line\n```",
    );
    let expanded = jcode_tui_messages::toggle_collapsible_swarm_content(&content).unwrap();
    let msg = DisplayMessage::swarm("File activity · moss", expanded);
    let lines = render_swarm_message(&msg, 120, crate::config::DiffDisplayMode::Off);
    let text = lines
        .iter()
        .map(extract_line_text)
        .collect::<Vec<_>>()
        .join("\n");
    assert!(lines.len() > 1);
    assert!(text.contains(SWARM_DIFF_COLLAPSE_BADGE));
    assert!(text.contains("Intent: fix the parser"));
    assert!(text.contains("323- old line"));
    assert!(text.contains("323+ new line"));
}

#[test]
fn raw_file_activity_system_notice_is_neutral_without_rewriting_content() {
    let content = "⚠️ File activity: /home/jeremy/jcode/src/parser.rs - moss previously read this file: read lines 1-1000\nMore details";
    let msg = DisplayMessage::system(content);
    let lines = render_system_message(&msg, 60, crate::config::DiffDisplayMode::Off);
    assert_eq!(lines.len(), 1);
    assert!(lines[0].width() <= 60);
    assert!(
        lines[0]
            .spans
            .iter()
            .all(|span| span.style.fg == Some(dim_color()))
    );
    assert_eq!(msg.content, content);
}

#[test]
fn interactive_login_prompt_keeps_instructions_visible() {
    let msg = DisplayMessage::system(
        "Azure OpenAI Login\n\nEnter your endpoint below.\nType /cancel to abort.",
    );
    let lines = render_system_message(&msg, 80, crate::config::DiffDisplayMode::Off);
    let text = lines
        .iter()
        .map(extract_line_text)
        .collect::<Vec<_>>()
        .join("\n");
    assert!(lines.len() > 1);
    assert!(text.contains("Enter your endpoint below."));
    assert!(text.contains("/cancel"));
}

#[test]
fn render_system_message_forces_system_color_on_all_spans() {
    let msg = DisplayMessage::system("**Reload complete** - continuing.");

    let lines = render_system_message(&msg, 80, crate::config::DiffDisplayMode::Off);

    assert!(!lines.is_empty(), "expected rendered system message lines");
    for line in lines {
        for span in line.spans {
            assert_eq!(span.style.fg, Some(system_message_color()));
        }
    }
}

#[test]
fn render_system_message_drops_emoji_variation_selectors() {
    let msg = DisplayMessage::system(
        "⚠️ KV cache miss [harness: prefix changed] turn 13: ~336K tokens resent (an earlier message was modified). See KV_CACHE_USAGE in logs.",
    );

    let lines = render_system_message(&msg, 80, crate::config::DiffDisplayMode::Off);
    let text: String = lines.iter().map(extract_line_text).collect();

    assert!(text.contains("⚠ KV cache miss"), "got: {text}");
    assert!(!text.contains('\u{FE0F}'), "VS16 must not reach the buffer");
}

#[test]
fn render_cold_cache_warning_is_always_one_width_bounded_line() {
    let saved = crate::tui::markdown::center_code_blocks();
    let msg = DisplayMessage::system(
        "🧊 Prompt cache went cold · next turn may resend ~96K tok · /cache extends",
    );

    for centered in [false, true] {
        crate::tui::markdown::set_center_code_blocks(centered);
        for width in [80_u16, 50, 30] {
            let lines = render_system_message(&msg, width, crate::config::DiffDisplayMode::Off);
            assert_eq!(
                lines.len(),
                1,
                "cold-cache notice wrapped at width {width} (centered={centered}): {lines:?}"
            );
            let text = extract_line_text(&lines[0]);
            assert!(
                !text.contains('\n'),
                "cold-cache notice contains a newline: {text:?}"
            );
            assert!(
                lines[0].width() <= width as usize,
                "cold-cache notice width {} exceeds {width}: {text:?}",
                lines[0].width()
            );
            assert!(
                text.contains("Prompt cache went cold"),
                "cold-cache identity was truncated away: {text:?}"
            );
            if width < 80 {
                assert!(
                    text.ends_with('…'),
                    "narrow cold-cache notice should end in an ellipsis: {text:?}"
                );
            }
            for span in &lines[0].spans {
                assert_eq!(span.style.fg, Some(system_message_color()));
            }
        }
    }

    crate::tui::markdown::set_center_code_blocks(saved);
}

#[test]
fn render_launch_hotkeys_keeps_both_shortcuts_visible() {
    let saved = crate::tui::markdown::center_code_blocks();
    let content = "Hotkeys: Super+; → jcode · Super+' → home";
    let msg = DisplayMessage::system(content).with_title("Launch hotkeys");

    for centered in [false, true] {
        crate::tui::markdown::set_center_code_blocks(centered);
        for width in [80_u16, 50] {
            let lines = render_system_message(&msg, width, crate::config::DiffDisplayMode::Off);
            assert_eq!(lines.len(), 1);
            assert_eq!(extract_line_text(&lines[0]).trim(), content);
            assert!(lines[0].width() <= width as usize);
        }
    }

    crate::tui::markdown::set_center_code_blocks(saved);
}

#[test]
fn render_compact_launch_and_divergence_notices_as_one_line() {
    let saved = crate::tui::markdown::center_code_blocks();
    let notices = [
        DisplayMessage::system(
            "Configured Jcode launch hotkeys (niri):\nSuper+; → jcode (/home/user/project)\n\nBound system-wide.",
        )
        .with_title("Launch hotkeys"),
        DisplayMessage::system(
            "Update diverged. Press Ctrl+Y to let a jcode agent merge local and upstream (or run `git pull` / `git rebase` yourself).",
        )
        .with_title("Update"),
    ];

    for centered in [false, true] {
        crate::tui::markdown::set_center_code_blocks(centered);
        for msg in &notices {
            for width in [80_u16, 50, 30] {
                let lines = render_system_message(msg, width, crate::config::DiffDisplayMode::Off);
                assert_eq!(
                    lines.len(),
                    1,
                    "compact notice wrapped at width {width} (centered={centered}): {lines:?}"
                );
                let text = extract_line_text(&lines[0]);
                assert!(!text.contains('\n'), "notice contains a newline: {text:?}");
                assert!(
                    lines[0].width() <= width as usize,
                    "notice width {} exceeds {width}: {text:?}",
                    lines[0].width()
                );
                if width < 80 {
                    assert!(
                        text.ends_with('…'),
                        "narrow compact notice should end in an ellipsis: {text:?}"
                    );
                }
            }
        }
    }

    crate::tui::markdown::set_center_code_blocks(saved);
}

#[test]
fn render_system_message_renders_markdown_formatting() {
    let msg = DisplayMessage::system(
        "**bold** and `code` and # heading\n- bullet item\n[link](http://example.com)",
    );

    let lines = render_system_message(&msg, 200, crate::config::DiffDisplayMode::Off);
    let plain = lines
        .iter()
        .map(extract_line_text)
        .collect::<Vec<_>>()
        .join("\n");

    assert_eq!(
        lines.len(),
        1,
        "ordinary markdown notices must stay compact"
    );
    // System messages render markdown: the inline markers are consumed and
    // the underlying text survives. Bold/code markers should no longer appear
    // literally, while the text content and a bullet glyph remain.
    assert!(plain.contains("bold"), "keeps bold text: {plain:?}");
    assert!(
        !plain.contains("**bold**"),
        "strips bold markers: {plain:?}"
    );
    assert!(plain.contains("code"), "keeps code text: {plain:?}");
    assert!(plain.contains("heading"), "keeps heading text: {plain:?}");
    assert!(
        plain.contains("bullet item"),
        "keeps bullet text: {plain:?}"
    );
    // The link text renders without the raw markdown link syntax.
    assert!(plain.contains("link"), "keeps link text: {plain:?}");
    assert!(
        !plain.contains("[link](http://example.com)"),
        "strips raw link syntax: {plain:?}"
    );

    // Color is still forced to the system color over every span.
    for line in &lines {
        for span in &line.spans {
            assert_eq!(span.style.fg, Some(system_message_color()));
        }
    }
}

#[test]
fn explicit_system_report_preserves_indentation_and_newlines() {
    let msg = DisplayMessage::system("Header line\n  indented detail\n\nNext block")
        .with_title("Context");

    let lines = render_system_message(&msg, 80, crate::config::DiffDisplayMode::Off);
    let rendered = lines.iter().map(extract_line_text).collect::<Vec<_>>();

    // Centered mode may add uniform left padding; compare relative structure.
    assert_eq!(rendered.len(), 4, "got: {rendered:?}");
    assert!(
        rendered[0].trim_end().ends_with("Header line"),
        "got: {rendered:?}"
    );
    assert!(
        rendered[1].trim_end().ends_with("indented detail"),
        "got: {rendered:?}"
    );
    assert!(
        rendered[2].trim().is_empty(),
        "blank line preserved, got: {rendered:?}"
    );
    assert!(
        rendered[3].trim_end().ends_with("Next block"),
        "got: {rendered:?}"
    );

    // The detail line keeps exactly two more leading spaces than the header.
    assert_eq!(
        leading_spaces(&rendered[1]),
        leading_spaces(&rendered[0]) + 2,
        "indentation should be preserved, got: {rendered:?}"
    );
}

#[test]
fn render_plaintext_lines_hang_indents_wrapped_continuations() {
    // An indented line longer than the wrap width keeps its indent on the wrap.
    let lines = render_plaintext_lines("  alpha beta gamma delta", 12);
    let rendered = lines.iter().map(extract_line_text).collect::<Vec<_>>();

    assert!(rendered.len() >= 2, "expected wrapping, got: {rendered:?}");
    for line in &rendered {
        assert!(
            line.is_empty() || line.starts_with("  "),
            "continuation lines should keep indent, got: {rendered:?}"
        );
        assert!(line.width() <= 12, "line too wide: {line:?}");
    }
}

#[test]
fn render_system_message_centered_mode_left_aligns_with_padding() {
    let saved = crate::tui::markdown::center_code_blocks();
    crate::tui::markdown::set_center_code_blocks(true);
    let msg = DisplayMessage::system("Reload complete - continuing.");

    let lines = render_system_message(&msg, 80, crate::config::DiffDisplayMode::Off);

    assert!(!lines.is_empty(), "expected rendered system message lines");
    for line in &lines {
        assert_eq!(
            line.alignment,
            Some(ratatui::layout::Alignment::Left),
            "centered system lines should be left-aligned with padding"
        );
        assert!(
            line.spans
                .first()
                .is_some_and(|span| span.content.starts_with(' ')),
            "centered system lines should start with padding"
        );
    }
    crate::tui::markdown::set_center_code_blocks(saved);
}

#[test]
fn render_system_message_uses_width_stable_titles_on_kitty() {
    let _guard = system_glyph_env_lock();
    let prev_term_program = std::env::var("TERM_PROGRAM").ok();
    let prev_term = std::env::var("TERM").ok();
    crate::env::set_var("TERM_PROGRAM", "kitty");
    crate::env::set_var("TERM", "xterm-kitty");

    let msg = DisplayMessage::system(
        "⚡ Connection lost - retrying (attempt 2, 7s) - connection reset by server",
    )
    .with_title("Connection");

    let lines = render_system_message(&msg, 80, crate::config::DiffDisplayMode::Off);
    let plain = lines
        .iter()
        .map(extract_line_text)
        .collect::<Vec<_>>()
        .join("\n");

    assert!(plain.contains("Connection lost - retrying"));
    assert!(!plain.contains('⚡'));

    match prev_term_program {
        Some(value) => crate::env::set_var("TERM_PROGRAM", value),
        None => crate::env::remove_var("TERM_PROGRAM"),
    }
    match prev_term {
        Some(value) => crate::env::set_var("TERM", value),
        None => crate::env::remove_var("TERM"),
    }
}

#[test]
fn render_background_task_failure_is_compact() {
    let called = (chrono::Utc::now() - chrono::Duration::seconds(125)).to_rfc3339();
    let msg = DisplayMessage::background_task(format!(
        "**Background task** `bg123` · `Comparison trace` (`bash`) · ✗ failed · 11.0s · exit 101\n\n_Called at:_ `{called}`\n\n```text\n[stderr] justrust: remote build\n[stderr] warning: unused variable\n[stderr] error[E0308]: mismatched types\n```\n\n_Full output:_ `bg action=\"output\" task_id=\"bg123\"`"
    ));
    let lines = render_background_task_message(&msg, 100, crate::config::DiffDisplayMode::Off);
    assert_eq!(lines.len(), 2);
    let plain = lines
        .iter()
        .map(extract_line_text)
        .collect::<Vec<_>>()
        .join("\n");
    assert!(
        plain.contains("✗ bash · Comparison trace · 2m ago"),
        "{plain}"
    );
    assert!(
        plain.contains("╰─exit 101 · [E0308]: mismatched types"),
        "{plain}"
    );
    assert!(!plain.contains("remote build"));
    assert_eq!(
        extract_line_text(&lines[1]).find('╰').unwrap(),
        extract_line_text(&lines[0]).find('✗').unwrap()
    );
    assert!(
        lines[0]
            .spans
            .iter()
            .any(|span| span.content == "bash" && span.style.fg == Some(tool_color()))
    );
    assert!(
        lines[0]
            .spans
            .iter()
            .any(|span| span.content.contains('✗') && span.style.fg == Some(rgb(220, 100, 100)))
    );
    assert!(
        lines[1]
            .spans
            .iter()
            .all(|span| span.content.trim().is_empty() || span.style.fg == Some(dim_color()))
    );
    assert!(
        msg.content
            .contains("bg action=\"output\" task_id=\"bg123\"")
    );
    for width in [0, 1, 2, 8, 20, 40] {
        let lines =
            render_background_task_message(&msg, width, crate::config::DiffDisplayMode::Off);
        assert_eq!(lines.len(), 2);
        for line in lines {
            assert!(UnicodeWidthStr::width(extract_line_text(&line).as_str()) <= width as usize);
        }
    }
}

#[test]
fn render_background_task_failure_without_diagnostic_is_honest() {
    let msg = DisplayMessage::background_task(
        "**Background task** `bg123` · `bash` · ✗ failed · 7.1s · exit 1\n\n_No output captured._\n\n_Full output:_ `bg action=\"output\" task_id=\"bg123\"`",
    );
    let plain = render_background_task_message(&msg, 100, crate::config::DiffDisplayMode::Off)
        .iter()
        .map(extract_line_text)
        .collect::<Vec<_>>()
        .join("\n");
    assert!(plain.contains("✗ bash · took 7.1s"), "{plain}");
    assert!(plain.contains("╰─exit 1 · no diagnostic captured"));
}

#[test]
fn render_background_task_success_is_compact_and_preserves_raw_output() {
    let msg = DisplayMessage::background_task(
        "**Background task** `bg123` · `bash` · ✓ completed · 7.1s · exit 0\n\n```text\nline 1\nline 2\nline 3\nline 4\nline 5\n```\n\n_Full output:_ `bg action=\"output\" task_id=\"bg123\"`",
    );

    let lines = render_background_task_message(&msg, 80, crate::config::DiffDisplayMode::Off);
    let plain = lines
        .iter()
        .map(|line| {
            line.spans
                .iter()
                .map(|span| span.content.as_ref())
                .collect::<String>()
        })
        .collect::<Vec<_>>()
        .join("\n");

    assert_eq!(lines.len(), 1);
    assert!(plain.contains("✓ bash · took 7.1s"), "{plain}");
    assert!(!plain.contains("exit 0"));
    assert!(!plain.contains("line 1"));
    assert!(!plain.contains("more line"));
    assert!(!plain.contains('╭') && !plain.contains('│') && !plain.contains('╰'));
    assert!(
        lines[0].spans.iter().any(|span| {
            span.content.contains('✓') && span.style.fg == Some(rgb(120, 210, 140))
        })
    );
    assert!(
        lines[0]
            .spans
            .iter()
            .any(|span| { span.content == "bash" && span.style.fg == Some(tool_color()) })
    );
    assert!(msg.content.contains("line 5"));
    assert!(
        msg.content
            .contains("bg action=\"output\" task_id=\"bg123\"")
    );
    assert!(!plain.contains("task bg123 · bash"));
    assert!(!plain.contains("Preview"));
    assert!(!plain.contains("Full output"));
    assert!(!plain.contains("bg action=\"output\" task_id=\"bg123\""));
}

#[test]
fn render_background_task_success_hides_ansi_preview_without_changing_raw() {
    let msg = DisplayMessage::background_task(
        "**Background task** `bg123` · `bash` · ✓ completed · 0.1s · exit 0\n\n```text\n\u{1b}[32m✓\u{1b}[39m passes \u{1b}[2m12ms\u{1b}[22m\n```\n\n_Full output:_ `bg action=\"output\" task_id=\"bg123\"`",
    );

    let raw = msg.content.clone();
    let plain = render_background_task_message(&msg, 80, crate::config::DiffDisplayMode::Off)
        .iter()
        .map(extract_line_text)
        .collect::<Vec<_>>()
        .join("\n");

    assert!(plain.contains("✓ bash · took 0.1s"), "{plain}");
    assert!(!plain.contains("passes"));
    assert_eq!(msg.content, raw);
    assert!(!plain.contains('\u{1b}'));
    assert!(!plain.contains("[32m"));
    assert!(!plain.contains("[2m"));
}

#[test]
fn render_background_task_success_fits_unicode_and_narrow_widths() {
    let raw = "**Background task** `bg-unicode` · `验证界面 e\u{301} 🦀 long background intent` (`bash`) · ✓ completed · 12.3s · exit 0\n\n```text\nlarge output must stay hidden\n```\n\n_Full output:_ `bg action=\"output\" task_id=\"bg-unicode\"`";
    let msg = DisplayMessage::background_task(raw);
    for width in [0, 1, 2, 3, 8, 16, 20, 40, 100] {
        let lines =
            render_background_task_message(&msg, width, crate::config::DiffDisplayMode::Off);
        assert_eq!(lines.len(), 1);
        let text = extract_line_text(&lines[0]);
        assert!(
            UnicodeWidthStr::width(text.as_str()) <= width as usize,
            "width={width}: {text}"
        );
        assert!(!text.contains("large output"));
        if width >= 40 {
            assert!(text.contains("took 12.3s"), "{text}");
        }
        if width == 100 {
            assert!(
                text.contains("✓ bash · 验证界面 e\u{301} 🦀 long background intent"),
                "{text}"
            );
        }
    }
    assert_eq!(msg.content, raw);
}

#[test]
fn render_system_message_strips_ansi_from_existing_inline_command_preview() {
    let msg = DisplayMessage::system(
        "Shell command · ✓ exit 0 · 12ms\n\n  cargo test\n\n  \u{1b}[32m✓\u{1b}[39m passes \u{1b}[2m12ms\u{1b}[22m",
    );

    let plain = render_system_message(&msg, 80, crate::config::DiffDisplayMode::Off)
        .iter()
        .map(extract_line_text)
        .collect::<Vec<_>>()
        .join("\n");

    assert!(
        plain.contains("✓ passes 12ms"),
        "rendered preview:\n{plain}"
    );
    assert!(!plain.contains('\u{1b}'));
    assert!(!plain.contains("[32m"));
    assert!(!plain.contains("[2m"));
}

#[test]
fn render_background_task_message_uses_swarm_flavor_for_swarm_tool() {
    crate::tui::markdown::set_center_code_blocks(false);
    let msg = DisplayMessage::background_task(
        "**Background task** `bg777` · `run_plan (6 nodes, deep mode)` (`swarm`) · ✓ completed · 92.4s · exit 0\n\n```text\nSwarm plan reached terminal/blocked state after 9 loop(s). completed=6 blocked=0 cycles=0 active=0 assignments=8\n```\n\n_Full output:_ `bg action=\"output\" task_id=\"bg777\"`",
    );

    let lines = render_background_task_message(&msg, 100, crate::config::DiffDisplayMode::Off);
    let plain = lines
        .iter()
        .map(extract_line_text)
        .collect::<Vec<_>>()
        .join("\n");

    assert_eq!(plain, "🐝 ✓ run plan · 92.4s");
    assert!(!plain.contains("bg777"));
    assert!(!plain.contains("Swarm plan reached terminal/blocked state"));
}

#[test]
fn render_background_task_progress_message_uses_swarm_flavor_for_swarm_tool() {
    crate::tui::markdown::set_center_code_blocks(false);
    let msg = DisplayMessage::background_task(
        "**Background task progress** `bg777` · `run_plan (6 nodes, deep mode)` (`swarm`)\n\n[####--------] 33% · 2/6 nodes · completed 2 · blocked 0 · active 3 · assignments 5 (reported)",
    );

    let lines = render_background_task_message(&msg, 100, crate::config::DiffDisplayMode::Off);
    let plain = lines
        .iter()
        .map(extract_line_text)
        .collect::<Vec<_>>()
        .join("\n");

    assert_eq!(plain, "🐝 ● run plan · 2/6");
    assert!(!plain.contains("bg777"));
}

#[test]
fn render_background_task_progress_message_uses_box_with_progress_bar() {
    let msg = DisplayMessage::background_task(
        "**Background task progress** `bg123` · `bash`\n\n[#####-------] 42% · Running tests (reported)",
    );

    let lines = render_background_task_message(&msg, 80, crate::config::DiffDisplayMode::Off);
    let plain = lines
        .iter()
        .map(extract_line_text)
        .collect::<Vec<_>>()
        .join("\n");

    assert!(plain.contains("◌ bg bash · bg123"));
    assert!(plain.contains("█"));
    assert!(plain.contains("░"));
    assert!(plain.contains("42%"));
    assert!(plain.contains("Running tests"));
    assert!(plain.contains("Latest status: bg action=\"status\" task_id=\"bg123\""));
    assert_eq!(
        plain.matches('│').count(),
        4,
        "expected compact progress row plus status hint:\n{plain}"
    );
    assert!(!plain.contains("Latest update"));
    assert!(!plain.contains("Source: reported"));
    assert!(!plain.contains("**Background task progress**"));
}

#[test]
fn render_overnight_message_uses_rounded_progress_card() {
    let card = crate::overnight::OvernightProgressCard {
        run_id: "overnight_1234567890abcdef".to_string(),
        status: "running".to_string(),
        phase: "running".to_string(),
        coordinator_session_id: "session_coord".to_string(),
        coordinator_session_name: "Overnight coordinator".to_string(),
        elapsed_label: "2h 15m".to_string(),
        target_duration_label: "7h".to_string(),
        progress_percent: 32.0,
        target_wake_at: "2026-05-01T15:00:00Z".to_string(),
        time_relation: "target in 4h 45m".to_string(),
        last_activity_label: "4m ago".to_string(),
        next_prompt_label: "handoff mode in 4h 15m or after current turn".to_string(),
        usage_risk: "medium".to_string(),
        usage_confidence: "low".to_string(),
        usage_projection: "projected 48% to 76%".to_string(),
        resources_summary: "RAM 62%, load 2.4/8, battery 80% discharging, disk 52.0 GB free"
            .to_string(),
        latest_event_kind: Some("coordinator_turn_completed".to_string()),
        latest_event_summary: Some("Coordinator turn completed".to_string()),
        task_summary: crate::overnight::OvernightTaskCardSummary {
            total: 4,
            counts: crate::overnight::OvernightTaskStatusCounts {
                completed: 2,
                active: 1,
                blocked: 0,
                deferred: 1,
                failed: 0,
                skipped: 0,
                unknown: 0,
            },
            validated: 2,
            high_risk: 0,
            latest_title: Some("Verify provider reload".to_string()),
            latest_status: Some("active".to_string()),
        },
        active_task_title: Some("Verify provider reload".to_string()),
        review_path: "/tmp/overnight/review.html".to_string(),
        log_path: "/tmp/overnight/run.log".to_string(),
        run_dir: "/tmp/overnight".to_string(),
        completed_at: None,
    };
    let msg = DisplayMessage::overnight(serde_json::to_string(&card).unwrap());

    let lines = render_overnight_message(&msg, 100, crate::config::DiffDisplayMode::Off);
    let plain = lines
        .iter()
        .map(extract_line_text)
        .collect::<Vec<_>>()
        .join("\n");

    assert!(plain.contains("overnight · running"));
    assert!(plain.contains("█"));
    assert!(plain.contains("░"));
    assert!(plain.contains("32%"));
    assert!(plain.contains("2 complete, 1 active, 0 blocked, 1 deferred"));
    assert!(plain.contains("Verify provider reload"));
    assert!(plain.contains("medium risk"));
    assert!(plain.contains("review.html"));
}

#[test]
fn background_dependency_todo_card_clock_and_progress_are_width_safe() {
    use unicode_width::UnicodeWidthStr;
    let todo = crate::todo::TodoItem {
        id: "wait".into(),
        content: "等待 build 🦀 with a long task title".into(),
        status: "pending".into(),
        waiting_on: vec!["build-task".into(), "test-task".into()],
        ..Default::default()
    };
    let tasks = vec![
        TodoCardBackgroundTask {
            task_id: "build-task".into(),
            label: "构建 🦀".into(),
            percent: Some(40.0),
        },
        TodoCardBackgroundTask {
            task_id: "test-task".into(),
            label: "tests".into(),
            percent: Some(75.0),
        },
    ];
    let wide = render_todo_item_line_with_background(&todo, &[], &tasks, "", 160);
    let text = extract_line_text(&wide);
    assert!(text.contains("🕒"), "{text}");
    assert!(!text.contains('○'), "{text}");
    assert!(text.contains("构建 🦀") && text.contains("tests"), "{text}");
    assert!(text.contains('▰') && text.contains('▱'), "{text}");
    assert!(text.find("等待").unwrap() < text.find("构建").unwrap());
    for width in 0..160 {
        let line = render_todo_item_line_with_background(&todo, &[], &tasks, "", width);
        assert!(extract_line_text(&line).width() <= width, "width={width}");
    }
    // A terminal or unknown reference is absent from the live snapshot.
    let resolved = extract_line_text(&render_todo_item_line_with_background(
        &todo,
        &[],
        &[],
        "",
        80,
    ));
    assert!(!resolved.contains("🕒") && !resolved.contains('%'));
    assert!(resolved.contains('○'));
    for status in ["completed", "cancelled"] {
        let mut done = todo.clone();
        done.status = status.into();
        let text = extract_line_text(&render_todo_item_line_with_background(
            &done,
            &[],
            &tasks,
            "",
            80,
        ));
        assert!(!text.contains("🕒") && !text.contains('%'));
    }
}

#[test]
fn background_dependency_indeterminate_progress_is_not_fabricated() {
    let todo = crate::todo::TodoItem {
        waiting_on: vec!["task".into()],
        status: "in_progress".into(),
        content: "Wait for task".into(),
        ..Default::default()
    };
    let tasks = vec![TodoCardBackgroundTask {
        task_id: "task".into(),
        label: "build".into(),
        percent: None,
    }];
    let text = extract_line_text(&render_todo_item_line_with_background(
        &todo,
        &[],
        &tasks,
        "",
        80,
    ));
    assert!(text.contains("🕒") && text.contains("running"));
    assert!(!text.contains('%'));
}

#[test]
fn background_dependency_clock_and_progress_render_in_flat_and_stage_cards() {
    use unicode_width::UnicodeWidthStr;
    for stage in [None, Some(1)] {
        let todo = crate::todo::TodoItem {
            id: "wait".into(),
            content: "Await build".into(),
            status: "in_progress".into(),
            stage,
            group: stage.map(|_| "compile".into()),
            waiting_on: vec!["task".into()],
            ..Default::default()
        };
        let message = DisplayMessage::todos(serde_json::json!({
            "todos": [todo], "background_tasks": [{"task_id":"task", "label":"build", "percent":40.0}]
        }).to_string());
        let lines = render_todos_message(&message, 100, crate::config::DiffDisplayMode::Off);
        let text = lines
            .iter()
            .map(extract_line_text)
            .collect::<Vec<_>>()
            .join("\n");
        assert!(
            text.contains("🕒") && text.contains("build") && text.contains('%'),
            "{text}"
        );
        for width in 20..100 {
            for line in render_todos_message(&message, width, crate::config::DiffDisplayMode::Off) {
                assert!(
                    extract_line_text(&line).width() <= width as usize,
                    "width={width}: {}",
                    extract_line_text(&line)
                );
            }
        }
    }
}

#[test]
fn render_todos_message_shows_grouped_card_with_status_glyphs() {
    fn todo(id: &str, content: &str, status: &str, group: Option<&str>) -> crate::todo::TodoItem {
        crate::todo::TodoItem {
            id: id.to_string(),
            content: content.to_string(),
            status: status.to_string(),
            priority: "high".to_string(),
            group: group.map(str::to_string),
            confidence: Some(crate::todo::ConfidenceState::from_legacy_score(80)),
            completion_confidence: (status == "completed")
                .then_some(crate::todo::ConfidenceState::from_legacy_score(95)),
            confidence_history: Vec::new(),
            waiting_on: Vec::new(),
            blocked_by: Vec::new(),
            assigned_to: None,
            stage: None,
            details: None,
        }
    }

    let todos = vec![
        todo("1", "Wire the hotkey", "completed", Some("todo card")),
        todo("2", "Render the card", "in_progress", Some("todo card")),
        todo("3", "Unrelated cleanup", "pending", None),
    ];
    let msg = DisplayMessage::todos(
        serde_json::json!({
            "todos": todos,
            "plan": {
                "user_intention": "Keep overall progress visible",
                "understands_user_intent": "clear"
            }
        })
        .to_string(),
    );

    let lines = render_todos_message(&msg, 100, crate::config::DiffDisplayMode::Off);
    let intent_row = lines
        .iter()
        .position(|line| extract_line_text(line).contains("Keep overall progress visible"))
        .unwrap();
    assert_eq!(extract_line_text(&lines[intent_row + 1]).trim(), "●●○");
    let plain = lines
        .iter()
        .map(extract_line_text)
        .collect::<Vec<_>>()
        .join("\n");

    assert!(!plain.contains("Todos"), "{plain}");
    assert!(plain.contains("todo card"), "{plain}");
    assert!(plain.contains("other"), "{plain}");
    let todo_card_header = lines
        .iter()
        .map(extract_line_text)
        .find(|line| line.contains("todo card"))
        .unwrap();
    assert_eq!(todo_card_header.matches('●').count(), 2, "{plain}");
    assert_eq!(todo_card_header.matches('○').count(), 0, "{plain}");
    let other_header = lines
        .iter()
        .map(extract_line_text)
        .find(|line| line.contains("other"))
        .unwrap();
    assert_eq!(other_header.matches('○').count(), 1, "{plain}");
    assert!(plain.contains("✓ Wire the hotkey"), "{plain}");
    assert!(plain.contains("● Render the card"), "{plain}");
    assert!(plain.contains("○ Unrelated cleanup"), "{plain}");
    // Confidence remains metadata rather than a visible item suffix.
    assert!(!plain.contains("plausible"), "{plain}");
    // Priority remains metadata and is not repeated in the visible item label.
    assert!(!plain.contains("(high)"), "{plain}");
    assert!(
        !plain.contains('╭'),
        "todo card should be borderless:\n{plain}"
    );
    assert!(
        !plain.contains('╰'),
        "todo card should be borderless:\n{plain}"
    );
}

#[test]
fn render_todos_message_shows_goal_scores_without_verbose_feedback() {
    let todos = vec![crate::todo::TodoItem {
        stage: None,
        details: None,
        id: "1".to_string(),
        content: "Render the card".to_string(),
        status: "in_progress".to_string(),
        priority: "high".to_string(),
        group: Some("todo rendering".to_string()),
        confidence: Some(crate::todo::ConfidenceState::from_legacy_score(85)),
        completion_confidence: None,
        waiting_on: Vec::new(),
        confidence_history: vec![
            crate::todo::ConfidenceState::from_legacy_score(80),
            crate::todo::ConfidenceState::from_legacy_score(85),
        ],
        blocked_by: Vec::new(),
        assigned_to: None,
    }];
    let goals = vec![crate::todo::TodoGoal {
        group: Some("todo rendering".to_string()),
        closed_feedback_loop: Some(crate::todo::FeedbackLoopState::from_legacy_score(95)),
        feedback_loop: Some("Inspect a debug frame".to_string()),
        feedback_loop_relevance: Some(crate::todo::FeedbackLoopRelevance::Representative),
        feedback_loop_coverage: Some(crate::todo::FeedbackLoopCoverage::MainPaths),
        delivery_state: Some(crate::todo::DeliveryState::from_legacy_score(90)),
        ..Default::default()
    }];
    let plan = crate::todo::TodoPlan {
        user_intention: Some("Keep the agent aligned with the user's request".to_string()),
        understands_user_intent: Some(crate::todo::IntentUnderstanding::from_legacy_score(98)),
        ..Default::default()
    };
    let msg = DisplayMessage::todos(
        serde_json::json!({ "todos": todos, "plan": plan, "goals": goals }).to_string(),
    );

    let plain = render_todos_message(&msg, 100, crate::config::DiffDisplayMode::Off)
        .iter()
        .map(extract_line_text)
        .collect::<Vec<_>>()
        .join("\n");

    assert!(plain.contains("Closed feedback loop strong"), "{plain}");
    assert!(!plain.contains("Delivery "), "{plain}");
    assert!(!plain.contains("Relevance representative"), "{plain}");
    assert!(!plain.contains("Coverage main_paths"), "{plain}");
    // Only the plan-level assessment renders above the groups.
    assert!(
        plain.contains("Intent clear: Keep the agent aligned"),
        "{plain}"
    );
    assert!(!plain.contains("Feedback ·"), "{plain}");
    assert!(!plain.contains("Inspect a debug frame"), "{plain}");
    assert!(plain.contains("● Render the card"), "{plain}");
    assert!(!plain.contains(" · plausible"), "{plain}");
    assert!(!plain.contains("(high)"), "{plain}");
}

#[test]
fn render_todos_message_shows_user_intention_when_understanding_is_unclear() {
    let long_text = "This deliberately long assessment detail should not consume several rows in a narrow terminal window";
    let todos = vec![crate::todo::TodoItem {
        id: "1".to_string(),
        content: "Keep the task visible".to_string(),
        status: "in_progress".to_string(),
        priority: "high".to_string(),
        group: Some("responsive card".to_string()),
        confidence: None,
        completion_confidence: None,
        confidence_history: Vec::new(),
        waiting_on: Vec::new(),
        blocked_by: Vec::new(),
        assigned_to: None,
        stage: None,
        details: None,
    }];
    let plan = crate::todo::TodoPlan {
        user_intention: Some(long_text.to_string()),
        understands_user_intent: Some(crate::todo::IntentUnderstanding::Partial),
        ..Default::default()
    };
    let goals = vec![crate::todo::TodoGoal {
        group: Some("responsive card".to_string()),
        feedback_loop: Some(long_text.to_string()),
        ..Default::default()
    }];
    let msg = DisplayMessage::todos(
        serde_json::json!({ "todos": todos, "plan": plan, "goals": goals }).to_string(),
    );

    let narrow = render_todos_message(&msg, 60, crate::config::DiffDisplayMode::Off)
        .iter()
        .map(extract_line_text)
        .collect::<Vec<_>>();
    assert_eq!(
        narrow
            .iter()
            .filter(|line| line.contains("Intent partial:"))
            .count(),
        1
    );
    assert_eq!(
        narrow
            .iter()
            .filter(|line| line.contains("Feedback"))
            .count(),
        0,
        "verbose goal feedback should stay out of inline cards: {}",
        narrow.join("\n")
    );
    assert!(
        narrow
            .iter()
            .any(|line| line.contains("Keep the task visible")),
        "{}",
        narrow.join("\n")
    );

    let wide = render_todos_message(&msg, 100, crate::config::DiffDisplayMode::Off)
        .iter()
        .map(extract_line_text)
        .collect::<Vec<_>>();
    assert!(
        wide.iter()
            .any(|line| line.contains("Intent partial: This deliberately long assessment detail")),
        "wide={wide:?}"
    );
    assert!(
        wide.iter().any(|line| line.contains('…')),
        "wide intent should remain on one ellipsized line: {wide:?}"
    );
    assert!(
        !narrow
            .iter()
            .any(|line| line.contains("narrow terminal window")),
        "narrow={narrow:?}"
    );
    assert!(
        narrow.iter().any(|line| line.contains('…')),
        "narrow intent should be ellipsized: {narrow:?}"
    );
}

#[test]
fn render_todos_message_uses_readable_semantic_colors() {
    let todos = vec![crate::todo::TodoItem {
        id: "1".to_string(),
        content: "Tune the palette".to_string(),
        status: "in_progress".to_string(),
        priority: "high".to_string(),
        group: Some("todo rendering".to_string()),
        confidence: Some(crate::todo::ConfidenceState::from_legacy_score(85)),
        completion_confidence: None,
        confidence_history: Vec::new(),
        waiting_on: Vec::new(),
        blocked_by: Vec::new(),
        assigned_to: None,
        stage: None,
        details: None,
    }];
    let goals = vec![crate::todo::TodoGoal {
        group: Some("todo rendering".to_string()),
        closed_feedback_loop: Some(crate::todo::FeedbackLoopState::from_legacy_score(95)),
        feedback_loop: None,
        ..Default::default()
    }];
    let plan = crate::todo::TodoPlan {
        user_intention: Some("Readable metadata".to_string()),
        understands_user_intent: Some(crate::todo::IntentUnderstanding::from_legacy_score(98)),
        ..Default::default()
    };
    let msg = DisplayMessage::todos(
        serde_json::json!({ "todos": todos, "plan": plan, "goals": goals }).to_string(),
    );
    let lines = render_todos_message(&msg, 100, crate::config::DiffDisplayMode::Off);
    let color_for = |text: &str| {
        lines
            .iter()
            .flat_map(|line| line.spans.iter())
            .find(|span| span.content.as_ref() == text)
            .and_then(|span| span.style.fg)
    };

    assert_eq!(color_for("todo rendering"), Some(todo_group_color()));
    assert_eq!(color_for("clear"), Some(todo_score_color()));
    assert_eq!(color_for("Readable metadata"), Some(todo_meta_color()));
    assert_eq!(color_for("● "), Some(asap_color()));
    assert_eq!(color_for(" (high)"), None);
    assert_eq!(color_for(" · plausible"), None);
    assert_eq!(color_for("strong"), Some(todo_warning_color()));
    assert_eq!(color_for("missing"), Some(todo_failure_color()));
    assert_ne!(todo_meta_color(), dim_color());
}

#[test]
fn finished_chat_todo_cards_show_lane_names_and_dots_without_changing_saved_data() {
    use unicode_width::UnicodeWidthStr;
    for staged in [false, true] {
        let todos = serde_json::json!([
            {"id":"done", "content":"Finished title 🦀", "details":"Long description", "priority":"high",
             "status":"completed", "group":"Implementation", "stage":if staged { Some(1) } else { None }},
            {"id":"cancel", "content":"Cancelled title", "status":"cancelled", "priority":"medium",
             "group":"Validation", "stage":if staged { Some(2) } else { None }}
        ]);
        for legacy in [false, true] {
            let content = if legacy {
                todos.to_string()
            } else {
                serde_json::json!({"todos":todos,
                    "plan":{"user_intention":"Verbose intention", "understands_user_intent":"complete"},
                    "goals":[{"closed_feedback_loop":"strong", "feedback_loop":"Verbose evidence"}]
                }).to_string()
            };
            let msg = DisplayMessage::todos(content.clone());
            let lines = render_todos_message(&msg, 100, crate::config::DiffDisplayMode::Off);
            assert_eq!(lines.len(), 2);
            assert_eq!(extract_line_text(&lines[0]).trim(), "✓ Implementation  ●");
            assert_eq!(extract_line_text(&lines[1]).trim(), "○ Validation      ○");
            for width in 0..110 {
                let lines = render_todos_message(&msg, width, crate::config::DiffDisplayMode::Off);
                assert_eq!(lines.len(), 2);
                assert!(
                    lines
                        .iter()
                        .all(|line| extract_line_text(line).width() <= width as usize),
                    "width={width}"
                );
            }
            assert_eq!(msg.content, content);
            let pinned =
                render_pinned_todos_message(&msg, 100, crate::config::DiffDisplayMode::Off)
                    .iter()
                    .map(extract_line_text)
                    .collect::<Vec<_>>()
                    .join("\n");
            assert!(pinned.contains("Finished title"), "{pinned}");
        }
    }
}

#[test]
fn unfinished_chat_todo_cards_retain_item_titles() {
    for status in ["pending", "in_progress", "blocked"] {
        let msg = DisplayMessage::todos(
            serde_json::json!({"todos":[
                {"id":"done", "content":"Finished title", "status":"completed", "priority":"high"},
                {"id":"open", "content":"Unfinished title", "status":status, "priority":"medium"}
            ]})
            .to_string(),
        );
        let text = render_todos_message(&msg, 100, crate::config::DiffDisplayMode::Off)
            .iter()
            .map(extract_line_text)
            .collect::<Vec<_>>()
            .join("\n");
        assert!(
            text.contains("Finished title") && text.contains("Unfinished title"),
            "{text}"
        );
    }
}

#[test]
fn render_todos_message_hides_item_confidence_transitions() {
    let todo: crate::todo::TodoItem = serde_json::from_value(serde_json::json!({
        "id": "hidden-confidence", "content": "Completed task", "status": "completed",
        "priority": "high", "confidence": "validated", "completion_confidence": "verified"
    }))
    .unwrap();
    let msg = DisplayMessage::todos(serde_json::json!({"todos": [todo]}).to_string());
    for width in [20, 40, 80] {
        let text = render_todos_message(&msg, width, crate::config::DiffDisplayMode::Off)
            .iter()
            .map(extract_line_text)
            .collect::<Vec<_>>()
            .join("\n");
        assert_eq!(text.trim(), "●", "{text}");
        assert!(
            !text.contains("validated") && !text.contains("verified") && !text.contains('→'),
            "{text}"
        );
    }
}

#[test]
fn render_todos_message_color_codes_every_intent_state() {
    let cases = [
        (
            crate::todo::IntentUnderstanding::Uncertain,
            todo_failure_color(),
        ),
        (
            crate::todo::IntentUnderstanding::Partial,
            todo_warning_color(),
        ),
        (crate::todo::IntentUnderstanding::Clear, todo_score_color()),
        (
            crate::todo::IntentUnderstanding::Complete,
            todo_score_color(),
        ),
    ];

    for (state, expected_color) in cases {
        let state_text = state.as_str().to_string();
        let msg = DisplayMessage::todos(
            serde_json::json!({
                "todos": [],
                "plan": {
                    "user_intention": "Keep intent visible",
                    "understands_user_intent": state,
                },
                "goals": [],
            })
            .to_string(),
        );
        let lines = render_todos_message(&msg, 100, crate::config::DiffDisplayMode::Off);
        let rendered_color = lines
            .iter()
            .flat_map(|line| line.spans.iter())
            .find(|span| span.content.as_ref() == state_text)
            .and_then(|span| span.style.fg);

        assert_eq!(
            rendered_color,
            Some(expected_color),
            "intent state {state_text} should keep its semantic color in the todo renderer"
        );
    }
}

#[test]
fn render_todos_message_collapses_passing_quality_gates() {
    let todos = vec![crate::todo::TodoItem {
        id: "1".to_string(),
        content: "Verify the result".to_string(),
        status: "completed".to_string(),
        priority: "high".to_string(),
        group: Some("quality".to_string()),
        confidence: None,
        completion_confidence: Some(crate::todo::ConfidenceState::Validated),
        confidence_history: Vec::new(),
        waiting_on: Vec::new(),
        blocked_by: Vec::new(),
        assigned_to: None,
        stage: None,
        details: None,
    }];
    let goals = vec![crate::todo::TodoGoal {
        group: Some("quality".to_string()),
        closed_feedback_loop: Some(crate::todo::FeedbackLoopState::Closed),
        feedback_loop_relevance: Some(crate::todo::FeedbackLoopRelevance::AcceptanceAligned),
        feedback_loop_coverage: Some(crate::todo::FeedbackLoopCoverage::EdgeAndIntegrationPaths),
        feedback_loop_traceability: Some(crate::todo::FeedbackLoopTraceability::Complete),
        delivery_state: Some(crate::todo::DeliveryState::OutcomeDelivered),
        ..Default::default()
    }];
    let msg =
        DisplayMessage::todos(serde_json::json!({ "todos": todos, "goals": goals }).to_string());
    // The detailed pinned view still exposes assessments. Finished transcript
    // cards deliberately hide all prose, including passing quality summaries.
    let lines = render_pinned_todos_message(&msg, 100, crate::config::DiffDisplayMode::Off);
    let plain = lines
        .iter()
        .map(extract_line_text)
        .collect::<Vec<_>>()
        .join("\n");

    assert!(plain.contains("✓ All quality gates passing"), "{plain}");
    assert!(!plain.contains("Delivery "), "{plain}");
    assert!(!plain.contains("Closed feedback loop closed"), "{plain}");
    assert!(!plain.contains("Relevance acceptance_aligned"), "{plain}");
    assert!(
        !plain.contains("Coverage edge_and_integration_paths"),
        "{plain}"
    );
    let passing = lines
        .iter()
        .flat_map(|line| line.spans.iter())
        .find(|span| span.content.as_ref() == "✓ All quality gates passing")
        .and_then(|span| span.style.fg);
    assert_eq!(passing, Some(todo_score_color()));
}

#[test]
fn render_todos_message_wraps_goal_scores_at_narrow_widths() {
    let todos = vec![crate::todo::TodoItem {
        id: "1".to_string(),
        content: "Render the card".to_string(),
        status: "in_progress".to_string(),
        priority: "high".to_string(),
        group: Some("todo rendering".to_string()),
        confidence: Some(crate::todo::ConfidenceState::from_legacy_score(85)),
        completion_confidence: None,
        confidence_history: Vec::new(),
        waiting_on: Vec::new(),
        blocked_by: Vec::new(),
        assigned_to: None,
        stage: None,
        details: None,
    }];
    let goals = vec![crate::todo::TodoGoal {
        group: Some("todo rendering".to_string()),
        closed_feedback_loop: Some(crate::todo::FeedbackLoopState::from_legacy_score(95)),
        feedback_loop: None,
        delivery_state: Some(crate::todo::DeliveryState::from_legacy_score(90)),
        ..Default::default()
    }];
    let msg =
        DisplayMessage::todos(serde_json::json!({ "todos": todos, "goals": goals }).to_string());

    let lines = render_todos_message(&msg, 40, crate::config::DiffDisplayMode::Off);
    let plain = lines
        .iter()
        .map(extract_line_text)
        .collect::<Vec<_>>()
        .join("\n");

    assert!(plain.contains("Closed feedback loop strong"), "{plain}");
    assert!(!plain.contains("Delivery "), "{plain}");
    assert!(
        lines.iter().all(|line| line.width() <= 38),
        "card exceeded its 38-column content budget: {plain}"
    );
}

#[test]
fn render_todos_message_packs_multiple_goal_gates_per_row() {
    let todos = vec![crate::todo::TodoItem {
        id: "1".to_string(),
        content: "Render the card".to_string(),
        status: "in_progress".to_string(),
        priority: "high".to_string(),
        group: Some("todo rendering".to_string()),
        confidence: Some(crate::todo::ConfidenceState::from_legacy_score(85)),
        completion_confidence: None,
        confidence_history: Vec::new(),
        waiting_on: Vec::new(),
        blocked_by: Vec::new(),
        assigned_to: None,
        stage: None,
        details: None,
    }];
    let goals = vec![crate::todo::TodoGoal {
        group: Some("todo rendering".to_string()),
        closed_feedback_loop: Some(crate::todo::FeedbackLoopState::Usable),
        feedback_loop_relevance: Some(crate::todo::FeedbackLoopRelevance::Representative),
        feedback_loop_coverage: Some(crate::todo::FeedbackLoopCoverage::MainPaths),
        delivery_state: Some(crate::todo::DeliveryState::WorkflowValidated),
        ..Default::default()
    }];
    let msg =
        DisplayMessage::todos(serde_json::json!({ "todos": todos, "goals": goals }).to_string());

    let lines = render_todos_message(&msg, 80, crate::config::DiffDisplayMode::Off);
    let rendered: Vec<String> = lines.iter().map(extract_line_text).collect();
    let gate_rows: Vec<&String> = rendered
        .iter()
        .filter(|line| {
            ["Closed feedback loop", "Relevance", "Coverage", "Delivery"]
                .iter()
                .any(|label| line.contains(label))
        })
        .collect();
    assert!(
        gate_rows.len() < 4,
        "gates should share rows instead of one per line: {rendered:#?}"
    );
    assert!(
        gate_rows.iter().any(|line| line.contains(" · ")),
        "{rendered:#?}"
    );
    assert!(lines.iter().all(|line| line.width() <= 78), "{rendered:#?}");
}

#[test]
fn render_todos_message_empty_list_shows_placeholder() {
    let msg = DisplayMessage::todos("[]");
    let plain = render_todos_message(&msg, 100, crate::config::DiffDisplayMode::Off)
        .iter()
        .map(extract_line_text)
        .collect::<Vec<_>>()
        .join("\n");
    assert!(!plain.contains("Todos"), "{plain}");
    assert!(plain.contains("No tasks yet"), "{plain}");
}

#[test]
fn render_todos_message_bad_payload_falls_back_to_system() {
    let msg = DisplayMessage::todos("not json");
    let lines = render_todos_message(&msg, 100, crate::config::DiffDisplayMode::Off);
    assert!(!lines.is_empty());
}

#[test]
fn render_todo_tool_result_uses_borderless_card_with_goal_scores() {
    let todos = vec![crate::todo::TodoItem {
        stage: None,
        details: None,
        id: "render".to_string(),
        content: "Render the todo result".to_string(),
        status: "in_progress".to_string(),
        priority: "high".to_string(),
        group: Some("todo rendering".to_string()),
        confidence: Some(crate::todo::ConfidenceState::from_legacy_score(92)),
        completion_confidence: None,
        waiting_on: Vec::new(),
        confidence_history: vec![
            crate::todo::ConfidenceState::from_legacy_score(85),
            crate::todo::ConfidenceState::from_legacy_score(92),
        ],
        blocked_by: Vec::new(),
        assigned_to: None,
    }];
    let goals = vec![crate::todo::TodoGoal {
        group: Some("todo rendering".to_string()),
        closed_feedback_loop: Some(crate::todo::FeedbackLoopState::from_legacy_score(95)),
        feedback_loop: Some("Inspect the rendered frame".to_string()),
        delivery_state: Some(crate::todo::DeliveryState::from_legacy_score(92)),
        ..Default::default()
    }];
    let content = format!(
        "[todo] [tool timing: start=2026-07-13T19:51:50.261Z finish=2026-07-13T19:51:50.265Z duration=4ms] {}\n\nGoals:\n{}\n\n{}",
        serde_json::to_string_pretty(&todos).unwrap(),
        serde_json::to_string_pretty(&goals).unwrap(),
        crate::todo::TODO_CLOSED_FEEDBACK_LOOP_CONTINUATION_MESSAGE
    );
    let msg = DisplayMessage {
        role: "tool".to_string(),
        content,
        tool_calls: Vec::new(),
        duration_secs: None,
        title: Some("1 todos".to_string()),
        tool_data: Some(crate::message::ToolCall {
            id: "call_todo".to_string(),
            name: "todo".to_string(),
            input: serde_json::json!({ "todos": todos, "goals": goals }),
            intent: Some("Track todo card work".to_string()),
            thought_signature: None,
        }),
    };

    let plain = render_tool_message(&msg, 100, crate::config::DiffDisplayMode::Off)
        .iter()
        .map(extract_line_text)
        .collect::<Vec<_>>()
        .join("\n");

    assert!(!plain.contains("Todos"), "{plain}");
    assert!(plain.contains("todo rendering  ●"), "{plain}");
    assert!(plain.contains("Closed feedback loop strong"), "{plain}");
    assert!(plain.contains("Relevance missing"), "{plain}");
    assert!(plain.contains("Coverage missing"), "{plain}");
    assert!(!plain.contains("Delivery "), "{plain}");
    assert!(plain.contains("● Render the todo result"), "{plain}");
    assert!(!plain.contains("(high)"), "{plain}");
    assert!(
        !plain.contains('╭'),
        "todo tool result should be borderless:\n{plain}"
    );
    assert!(
        !plain.contains("todo 1 items"),
        "generic tool row leaked:\n{plain}"
    );
}

#[test]
fn render_todo_quality_gate_retry_shows_only_changed_goal_fields() {
    let todos = vec![crate::todo::TodoItem {
        id: "render".to_string(),
        content: "Render the entire unchanged todo plan".to_string(),
        status: "in_progress".to_string(),
        priority: "high".to_string(),
        group: Some("todo rendering".to_string()),
        confidence: Some(crate::todo::ConfidenceState::from_legacy_score(92)),
        ..Default::default()
    }];
    let before = crate::todo::TodoGoal {
        group: Some("todo rendering".to_string()),
        closed_feedback_loop: Some(crate::todo::FeedbackLoopState::from_legacy_score(90)),
        feedback_loop: Some("Inspect one frame".to_string()),
        feedback_loop_relevance: Some(crate::todo::FeedbackLoopRelevance::Indirect),
        feedback_loop_coverage: Some(crate::todo::FeedbackLoopCoverage::Narrow),
        ..Default::default()
    };
    let after = crate::todo::TodoGoal {
        closed_feedback_loop: Some(crate::todo::FeedbackLoopState::from_legacy_score(98)),
        feedback_loop: Some(
            "Render before and after fixtures and assert unchanged fields are absent".to_string(),
        ),
        feedback_loop_relevance: Some(crate::todo::FeedbackLoopRelevance::Representative),
        feedback_loop_coverage: Some(crate::todo::FeedbackLoopCoverage::MainPaths),
        ..before.clone()
    };
    let updates = vec![crate::todo::TodoGoalChange {
        before: Some(before),
        after: Some(after.clone()),
        fields: vec![
            crate::todo::TodoGoalField::ClosedFeedbackLoop,
            crate::todo::TodoGoalField::FeedbackLoop,
            crate::todo::TodoGoalField::FeedbackLoopRelevance,
            crate::todo::TodoGoalField::FeedbackLoopCoverage,
        ],
    }];
    let content = format!(
        "{}\n\nGoals:\n{}\n\nGoal updates:\n{}\n\n{}",
        serde_json::to_string_pretty(&todos).unwrap(),
        serde_json::to_string_pretty(&vec![after]).unwrap(),
        serde_json::to_string_pretty(&updates).unwrap(),
        crate::todo::TODO_CLOSED_FEEDBACK_LOOP_CONTINUATION_MESSAGE,
    );
    let msg = DisplayMessage {
        role: "tool".to_string(),
        content,
        tool_calls: Vec::new(),
        duration_secs: None,
        title: Some("1 todos".to_string()),
        tool_data: Some(crate::message::ToolCall {
            id: "call_todo_update".to_string(),
            name: "todo".to_string(),
            input: serde_json::Value::Null,
            intent: Some("Refine the todo feedback loop".to_string()),
            thought_signature: None,
        }),
    };

    let plain = render_tool_message(&msg, 100, crate::config::DiffDisplayMode::Off)
        .iter()
        .map(extract_line_text)
        .collect::<Vec<_>>()
        .join("\n");

    assert!(plain.contains("todo rendering  updated"), "{plain}");
    assert!(
        plain.contains("Closed feedback loop strong → closed"),
        "{plain}"
    );
    assert!(
        plain.contains("Feedback-loop relevance indirect → representative"),
        "{plain}"
    );
    assert!(
        plain.contains("Feedback-loop coverage narrow → main_paths"),
        "{plain}"
    );
    assert!(!plain.contains("Feedback ·"), "{plain}");
    assert!(
        !plain.contains("assert unchanged fields are absent"),
        "{plain}"
    );
    assert!(
        !plain.contains("Render the entire unchanged todo plan"),
        "{plain}"
    );
    assert!(!plain.contains("Alignment score"), "{plain}");
    assert!(!plain.contains("Keep the todo card concise"), "{plain}");
    assert!(!plain.contains("See current work at a glance"), "{plain}");
}

#[test]
fn render_goal_update_size_is_bounded_when_narrative_evidence_is_long() {
    let long_text = "verbose evidence ".repeat(600);
    let goal = crate::todo::TodoGoal {
        group: Some("compact assessment".to_string()),
        closed_feedback_loop: Some(crate::todo::FeedbackLoopState::Strong),
        delivery_state: Some(crate::todo::DeliveryState::Integrated),
        autonomy: Some(crate::todo::Autonomy::NecessaryFollowthrough),
        iteration_maturity: Some(crate::todo::IterationMaturity::OutcomeReached),
        feedback_loop: Some(long_text.clone()),
        stopping_evidence: Some(long_text),
        ..Default::default()
    };
    let update = crate::todo::TodoGoalChange {
        before: None,
        after: Some(goal),
        fields: vec![
            crate::todo::TodoGoalField::ClosedFeedbackLoop,
            crate::todo::TodoGoalField::DeliveryState,
            crate::todo::TodoGoalField::Autonomy,
            crate::todo::TodoGoalField::IterationMaturity,
            crate::todo::TodoGoalField::FeedbackLoop,
            crate::todo::TodoGoalField::StoppingEvidence,
        ],
    };

    let lines = render_todo_goal_updates(&[update], 95);
    let plain = lines
        .iter()
        .map(extract_line_text)
        .collect::<Vec<_>>()
        .join("\n");

    assert_eq!(lines.len(), 5, "narrative text must not add rows:\n{plain}");
    assert!(!plain.contains("Feedback"), "{plain}");
    assert!(!plain.contains("Stopping evidence"), "{plain}");
    assert!(!plain.contains("verbose evidence"), "{plain}");
}

#[test]
fn render_todo_plan_update_card_shows_only_changed_intent_fields() {
    let todos = vec![crate::todo::TodoItem {
        id: "render".to_string(),
        content: "Render the entire unchanged todo plan".to_string(),
        status: "in_progress".to_string(),
        priority: "high".to_string(),
        confidence: Some(crate::todo::ConfidenceState::from_legacy_score(92)),
        ..Default::default()
    }];
    let before = crate::todo::TodoPlan {
        user_intention: Some("Ship the plan-level intent gate".to_string()),
        understands_user_intent: Some(crate::todo::IntentUnderstanding::from_legacy_score(80)),
        ..Default::default()
    };
    let after = crate::todo::TodoPlan {
        understands_user_intent: Some(crate::todo::IntentUnderstanding::from_legacy_score(97)),
        ..before.clone()
    };
    let update = crate::todo::TodoPlanChange {
        before: Some(before),
        after: Some(after.clone()),
        fields: vec![crate::todo::TodoPlanField::UnderstandsUserIntent],
    };
    let content = format!(
        "{}\n\nPlan:\n{}\n\nPlan updates:\n{}",
        serde_json::to_string_pretty(&todos).unwrap(),
        serde_json::to_string_pretty(&after).unwrap(),
        serde_json::to_string_pretty(&update).unwrap(),
    );
    let msg = DisplayMessage {
        role: "tool".to_string(),
        content,
        tool_calls: Vec::new(),
        duration_secs: None,
        title: Some("1 todos".to_string()),
        tool_data: Some(crate::message::ToolCall {
            id: "call_plan_update".to_string(),
            name: "todo".to_string(),
            input: serde_json::Value::Null,
            intent: Some("Reassess the user's intent".to_string()),
            thought_signature: None,
        }),
    };

    let plain = render_tool_message(&msg, 100, crate::config::DiffDisplayMode::Off)
        .iter()
        .map(extract_line_text)
        .collect::<Vec<_>>()
        .join("\n");

    assert!(plain.contains("Plan  updated"), "{plain}");
    assert!(
        plain.contains("Understands user intent partial → clear"),
        "{plain}"
    );
    // Unchanged fields and the full plan stay out of the refinement card.
    assert!(!plain.contains("User intention"), "{plain}");
    assert!(
        !plain.contains("Render the entire unchanged todo plan"),
        "{plain}"
    );
}

#[test]
fn parse_todo_tool_output_accepts_timestamp_only_header() {
    let todos = vec![crate::todo::TodoItem {
        id: "timed".to_string(),
        content: "Render the restored todo".to_string(),
        status: "in_progress".to_string(),
        priority: "high".to_string(),
        ..Default::default()
    }];
    let content = format!(
        "[2026-07-13T19:51:50.261Z] [todo] {}",
        serde_json::to_string(&todos).unwrap()
    );

    let parsed = parse_todo_tool_output(&content).expect("timestamped todo payload");
    assert_eq!(parsed.todos.len(), 1);
    assert_eq!(parsed.todos[0].id, todos[0].id);
    assert_eq!(parsed.todos[0].content, todos[0].content);
    assert!(parsed.goals.is_empty());
    assert!(parsed.goal_updates.is_empty());
}

#[test]
fn unbiased_visual_prompt_retry_renders_complete_feedback_change() {
    const PROMPT: &str = "can you make a pelican riding a bike animation in html and vanillia js ";
    const INITIAL_FEEDBACK: &str = "Open the page in a browser, inspect runtime errors, and verify animation state changes over time.";
    const REVISED_FEEDBACK: &str = "Serve the files locally, load them in a real browser at desktop and mobile viewport sizes, assert zero console/page errors, sample wheel and scenery transforms at two timestamps to prove motion, and exercise pause plus speed controls to confirm state changes.";
    const REVISED_OBJECTIVE: &str = "Deliver a responsive standalone animation whose pelican visibly pedals a moving bicycle through a layered seaside scene at 60fps where supported, with working pause/resume and three-speed controls, accessible labels, no external runtime dependencies, and zero browser console errors.";

    // Keep the eval input neutral. The visual verification strategy must come
    // from the model's todo refinement, not from criteria planted in the prompt.
    for biased_term in [
        "feedback loop",
        "browser",
        "console",
        "viewport",
        "screenshot",
        "visual quality",
    ] {
        assert!(!PROMPT.to_ascii_lowercase().contains(biased_term));
    }

    let todos = vec![crate::todo::TodoItem {
        id: "implement".to_string(),
        content: "Implement the illustrated pelican bicycle scene and responsive styling"
            .to_string(),
        status: "in_progress".to_string(),
        priority: "high".to_string(),
        group: Some("pelican-bike-animation".to_string()),
        confidence: Some(crate::todo::ConfidenceState::from_legacy_score(90)),
        ..Default::default()
    }];
    let render = |goal: crate::todo::TodoGoal,
                  intention: &str,
                  continuation: Option<&str>,
                  tool_data: Option<crate::message::ToolCall>| {
        let plan = crate::todo::TodoPlan {
            user_intention: Some(intention.to_string()),
            understands_user_intent: Some(crate::todo::IntentUnderstanding::from_legacy_score(96)),
            ..Default::default()
        };
        let mut content = format!(
            "[todo] [tool timing: start=2026-07-13T19:51:50.261Z finish=2026-07-13T19:51:50.265Z duration=4ms] {}\n\nPlan:\n{}\n\nGoals:\n{}",
            serde_json::to_string_pretty(&todos).unwrap(),
            serde_json::to_string_pretty(&plan).unwrap(),
            serde_json::to_string_pretty(&vec![goal]).unwrap()
        );
        if let Some(continuation) = continuation {
            content.push_str("\n\n");
            content.push_str(continuation);
        }
        let msg = DisplayMessage {
            role: "tool".to_string(),
            content,
            tool_calls: Vec::new(),
            duration_secs: None,
            title: Some("1 todos".to_string()),
            tool_data,
        };
        render_tool_message(&msg, 72, crate::config::DiffDisplayMode::Off)
            .iter()
            .map(extract_line_text)
            .collect::<Vec<_>>()
            .join("\n")
    };

    let initial = render(
        crate::todo::TodoGoal {
            group: Some("pelican-bike-animation".to_string()),
            closed_feedback_loop: Some(crate::todo::FeedbackLoopState::from_legacy_score(90)),
            feedback_loop: Some(INITIAL_FEEDBACK.to_string()),
            ..Default::default()
        },
        "Make a pelican riding a bike animation that clearly works in a browser",
        Some(crate::todo::TODO_CLOSED_FEEDBACK_LOOP_CONTINUATION_MESSAGE),
        Some(crate::message::ToolCall {
            id: "call_initial_todo".to_string(),
            name: "todo".to_string(),
            input: serde_json::Value::Null,
            intent: Some("Track implementation and browser verification".to_string()),
            thought_signature: None,
        }),
    );
    assert!(initial.contains("pelican-bike-animation"), "{initial}");
    assert!(initial.contains("Closed feedback loop strong"), "{initial}");
    assert!(!without_whitespace(&initial).contains(&without_whitespace(INITIAL_FEEDBACK)));

    // Simulate a restored/mirrored result whose ToolCall association was lost.
    // The structured result must still render as the same complete todo card.
    let revised = render(
        crate::todo::TodoGoal {
            group: Some("pelican-bike-animation".to_string()),
            closed_feedback_loop: Some(crate::todo::FeedbackLoopState::from_legacy_score(98)),
            feedback_loop: Some(REVISED_FEEDBACK.to_string()),
            ..Default::default()
        },
        REVISED_OBJECTIVE,
        None,
        None,
    );
    let compact_revised = without_whitespace(&revised);
    assert!(revised.contains("pelican-bike-animation"), "{revised}");
    assert!(
        revised.contains("Relevance missing · Coverage missing · Traceability missing"),
        "{revised}"
    );
    assert!(!revised.contains("Closed feedback loop closed"));
    assert!(!compact_revised.contains(&without_whitespace(REVISED_FEEDBACK)));
    assert!(revised.contains("● Implement"), "{revised}");
}

#[test]
fn visually_appealing_prompt_batched_retry_renders_complete_todo_card() {
    // This fixture is only the first todo retry emitted after the
    // closed feedback loop continuation. The eval stops here and deliberately does
    // not depend on the model implementing or completing the visual task.
    const PROMPT: &str =
        "make the most visually appealing pelican on a bike animation with html and vanillia js";
    const FEEDBACK: &str = "At each iteration, render at 1440x900 and 390x844, capture screenshots, and score five checks: scene fills viewport without clipping, focal subject is centered, at least six distinct motion layers run smoothly, controls respond, and no console errors occur. Refine until all checks pass.";
    const OBJECTIVE: &str = "Deliver a single-page vanilla HTML/CSS/JS animation whose pelican cyclist remains legible and visually balanced at desktop and mobile sizes, includes six or more coordinated motion layers, supports interactive speed controls, and runs with zero console errors.";

    assert!(!PROMPT.contains("1440x900"));
    assert!(!PROMPT.contains("screenshot"));
    assert!(!PROMPT.contains("console"));
    assert!(!PROMPT.contains("feedback"));

    let todos = vec![crate::todo::TodoItem {
        id: "inspect".to_string(),
        content: "Inspect the starter project and determine the page structure".to_string(),
        status: "in_progress".to_string(),
        priority: "high".to_string(),
        group: Some("pelican-bike".to_string()),
        confidence: Some(crate::todo::ConfidenceState::from_legacy_score(95)),
        ..Default::default()
    }];
    let goals = vec![crate::todo::TodoGoal {
        group: Some("pelican-bike".to_string()),
        closed_feedback_loop: Some(crate::todo::FeedbackLoopState::from_legacy_score(98)),
        feedback_loop: Some(FEEDBACK.to_string()),
        feedback_loop_traceability: Some(crate::todo::FeedbackLoopTraceability::Complete),
        ..Default::default()
    }];
    let plan = crate::todo::TodoPlan {
        user_intention: Some(OBJECTIVE.to_string()),
        understands_user_intent: Some(crate::todo::IntentUnderstanding::from_legacy_score(97)),
        ..Default::default()
    };
    let todo_output = format!(
        "{}\n\nPlan:\n{}\n\nGoals:\n{}",
        serde_json::to_string_pretty(&todos).unwrap(),
        serde_json::to_string_pretty(&plan).unwrap(),
        serde_json::to_string_pretty(&goals).unwrap()
    );
    let content = format!(
        "--- [1] todo ---\n{todo_output}\n\n--- [2] ls ---\n./\n\n0 files, 0 directories\n\nCompleted: 2 succeeded, 0 failed"
    );
    let msg = DisplayMessage {
        role: "tool".to_string(),
        content,
        tool_calls: Vec::new(),
        duration_secs: None,
        title: None,
        tool_data: Some(crate::message::ToolCall {
            id: "call_batch".to_string(),
            name: "batch".to_string(),
            input: serde_json::json!({
                "intent": "Inspect starter files and strengthen measurable visual goals",
                "tool_calls": [
                    {
                        "tool": "todo",
                        "intent": "Make the visual outcome objectively verifiable",
                        "todos": todos,
                        "goals": goals
                    },
                    { "tool": "ls", "path": "." }
                ]
            }),
            intent: Some(
                "Inspect starter files and strengthen measurable visual goals".to_string(),
            ),
            thought_signature: None,
        }),
    };

    let rendered = render_tool_message(&msg, 84, crate::config::DiffDisplayMode::Off)
        .iter()
        .map(extract_line_text)
        .collect::<Vec<_>>()
        .join("\n");
    let compact = without_whitespace(&rendered);

    assert!(rendered.contains("✓ todo"), "{rendered}");
    assert!(rendered.contains("pelican-bike"), "{rendered}");
    // The plan intent renders inline with the understanding state on a single
    // ellipsized line (2e847827f). The full text lives in the todo payload.
    let intent_line = rendered
        .lines()
        .find(|line| line.contains("Intent clear:"))
        .expect("batched todo card should show the plan intent");
    let shown = intent_line
        .split_once("Intent clear:")
        .map(|(_, rest)| rest.trim().trim_end_matches('…'))
        .unwrap_or_default();
    assert!(
        shown.len() > 20 && OBJECTIVE.starts_with(shown),
        "batched todo plan intent should show the objective prefix:\n{rendered}"
    );
    // Compact transcript cards show the goal's quality assessments rather than
    // repeating its potentially long feedback-loop prose. The full prose remains
    // available in the serialized todo payload and the todos side panel.
    assert!(rendered.contains("Relevance missing · Coverage missing"));
    assert!(!compact.contains(&without_whitespace(FEEDBACK)));
    let goal_details = rendered
        .split_once("pelican-bike")
        .map(|(_, details)| details)
        .and_then(|details| details.split("● Inspect").next())
        .expect("todo item should follow the batched goal details");
    assert!(
        !goal_details.contains('…'),
        "batched todo goal details must not truncate:\n{rendered}"
    );
}

#[test]
fn render_ownership_gated_todo_result_keeps_the_full_card() {
    let todos = vec![crate::todo::TodoItem {
        id: "ship".to_string(),
        content: "Deliver the complete workflow".to_string(),
        status: "in_progress".to_string(),
        priority: "high".to_string(),
        group: Some("ship outcome".to_string()),
        confidence: Some(crate::todo::ConfidenceState::from_legacy_score(95)),
        ..Default::default()
    }];
    let goals = vec![crate::todo::TodoGoal {
        group: Some("ship outcome".to_string()),
        closed_feedback_loop: Some(crate::todo::FeedbackLoopState::from_legacy_score(100)),
        feedback_loop: Some("Run the complete workflow".to_string()),
        feedback_loop_relevance: Some(crate::todo::FeedbackLoopRelevance::Representative),
        feedback_loop_coverage: Some(crate::todo::FeedbackLoopCoverage::MainPaths),
        feedback_loop_traceability: Some(crate::todo::FeedbackLoopTraceability::Complete),
        delivery_state: Some(crate::todo::DeliveryState::from_legacy_score(80)),
        ..Default::default()
    }];
    let content = format!(
        "{}\n\nGoals:\n{}\n\n{}",
        serde_json::to_string_pretty(&todos).unwrap(),
        serde_json::to_string_pretty(&goals).unwrap(),
        crate::todo::TODO_OWNERSHIP_CONTINUATION_MESSAGE
    );
    let msg = DisplayMessage {
        role: "tool".to_string(),
        content,
        tool_calls: Vec::new(),
        duration_secs: None,
        title: Some("1 todos".to_string()),
        tool_data: Some(crate::message::ToolCall {
            id: "call_todo_ownership".to_string(),
            name: "todo".to_string(),
            input: serde_json::json!({ "todos": todos, "goals": goals }),
            intent: Some("Complete the full user outcome".to_string()),
            thought_signature: None,
        }),
    };

    let plain = render_tool_message(&msg, 100, crate::config::DiffDisplayMode::Off)
        .iter()
        .map(extract_line_text)
        .collect::<Vec<_>>()
        .join("\n");

    assert!(plain.contains("ship outcome  ●"), "{plain}");
    assert!(plain.contains("Deliver the complete workflow"), "{plain}");
    assert!(plain.contains("✓ All quality gates passing"), "{plain}");
    assert!(!plain.contains("Delivery "), "{plain}");
    assert!(!plain.contains("todo 1 items"), "{plain}");
}

#[test]
fn render_background_task_messages_prefer_display_name() {
    let completion = DisplayMessage::background_task(
        "**Background task** `bg123` · `Run integration tests` (`bash`) · ✓ completed · 7.1s · exit 0\n\n_No output captured._\n\n_Full output:_ `bg action=\"output\" task_id=\"bg123\"`",
    );
    let completion_plain =
        render_background_task_message(&completion, 100, crate::config::DiffDisplayMode::Off)
            .iter()
            .map(extract_line_text)
            .collect::<Vec<_>>()
            .join("\n");
    assert!(completion_plain.contains("✓ bash · Run integration tests · took 7.1s"));

    let progress = DisplayMessage::background_task(
        "**Background task progress** `bg123` · `Run integration tests` (`bash`)\n\n[#####-------] 42% · Running tests (reported)",
    );
    let progress_plain =
        render_background_task_message(&progress, 100, crate::config::DiffDisplayMode::Off)
            .iter()
            .map(extract_line_text)
            .collect::<Vec<_>>()
            .join("\n");
    assert!(progress_plain.contains("◌ bg Run integration tests · bg123"));
}

#[test]
fn render_system_message_uses_scheduled_task_card() {
    let _guard = system_glyph_env_lock();
    let msg = DisplayMessage::system(
        "[Scheduled task]\nA scheduled task for this session is now due.\n\nTask: Follow up on the scheduler test\nWorking directory: /home/jeremy/jcode\nRelevant files: src/tui/ui_messages.rs\nBranch: master\n\nBackground: Verify the scheduled task card styling\nSuccess criteria: The due task renders clearly\nScheduled by session: session_test",
    );

    let lines = render_system_message(&msg, 100, crate::config::DiffDisplayMode::Off);
    let plain = lines
        .iter()
        .map(extract_line_text)
        .collect::<Vec<_>>()
        .join("\n");

    assert!(plain.contains(width_stable_system_title(
        "⏰ scheduled task due",
        "scheduled task due"
    )));
    assert!(plain.contains("This scheduled task is now active in this session."));
    assert!(plain.contains("Follow up on the scheduler test"));
    assert!(plain.contains("Verify the scheduled task card styling"));
    assert!(!plain.contains("[Scheduled task]"));
    assert!(!plain.contains("A scheduled task for this session is now due."));
}

#[test]
fn render_tool_message_uses_scheduled_card() {
    let _guard = system_glyph_env_lock();
    let msg = DisplayMessage {
        role: "tool".to_string(),
        content: "Scheduled task 'Follow up on the scheduler test' for in 1m (id: sched_abc123)\nWorking directory: /home/jeremy/jcode\nRelevant files: src/tui/ui_messages.rs\nTarget: resume session session_test".to_string(),
        tool_calls: Vec::new(),
        duration_secs: None,
        title: Some("scheduled: Follow up on the scheduler test".to_string()),
        tool_data: Some(crate::message::ToolCall {
            id: "call_schedule_card".to_string(),
            name: "schedule".to_string(),
            input: serde_json::json!({
                "task": "Follow up on the scheduler test",
                "wake_in_minutes": 1,
                "target": "resume"
            }),
            intent: None, thought_signature: None, }),
    };

    let lines = render_tool_message(&msg, 100, crate::config::DiffDisplayMode::Off);
    let plain = lines
        .iter()
        .map(extract_line_text)
        .collect::<Vec<_>>()
        .join("\n");

    assert!(plain.contains(width_stable_system_title("⏰ scheduled", "scheduled")));
    assert!(plain.contains("Will run in 1m."));
    assert!(plain.contains("Follow up on the scheduler test"));
    assert!(plain.contains("session session_test"));
    assert!(plain.contains("sched_abc123"));
    assert!(!plain.contains("✓ schedule"));
}

#[test]
fn render_assistant_message_renders_plan_block_as_card() {
    let saved = crate::tui::markdown::center_code_blocks();
    crate::tui::markdown::set_center_code_blocks(false);
    let msg = DisplayMessage::assistant(
        "Here is the plan:\n\n```plan\n# Ship compact mode\n\n## Goal\nAdd a compact message mode.\n\n## Approach\n1. Add config flag\n2. Wire renderer\n```\n\nLet me know if this works.",
    );

    let lines = render_assistant_message(&msg, 100, crate::config::DiffDisplayMode::Off);
    crate::tui::markdown::set_center_code_blocks(saved);
    let plain = lines
        .iter()
        .map(extract_line_text)
        .collect::<Vec<_>>()
        .join("\n");

    assert!(plain.contains("Here is the plan:"), "plain: {plain}");
    assert!(plain.contains("⛭ Ship compact mode"), "plain: {plain}");
    assert!(plain.contains('╭'), "expected card border: {plain}");
    assert!(plain.contains('╰'), "expected card border: {plain}");
    assert!(plain.contains("Add a compact message mode."));
    assert!(plain.contains("Let me know if this works."));
    assert!(
        !plain.contains("```"),
        "plan fence markers should not render: {plain}"
    );
}

#[test]
fn render_assistant_message_plan_card_survives_unterminated_fence() {
    let saved = crate::tui::markdown::center_code_blocks();
    crate::tui::markdown::set_center_code_blocks(false);
    let msg = DisplayMessage::assistant("```plan\n# Streaming plan\n\n- step one");

    let lines = render_assistant_message(&msg, 80, crate::config::DiffDisplayMode::Off);
    crate::tui::markdown::set_center_code_blocks(saved);
    let plain = lines
        .iter()
        .map(extract_line_text)
        .collect::<Vec<_>>()
        .join("\n");

    assert!(plain.contains("⛭ Streaming plan"), "plain: {plain}");
    assert!(plain.contains("step one"), "plain: {plain}");
}

#[test]
fn render_assistant_message_plan_card_keeps_nested_fences_inside() {
    let saved = crate::tui::markdown::center_code_blocks();
    crate::tui::markdown::set_center_code_blocks(false);
    let msg = DisplayMessage::assistant(
        "```plan\n# Validation plan\n\n```bash\ncargo test -p jcode-tui\n```\n\nAfter the block.\n```\n\nOutside text.",
    );

    let lines = render_assistant_message(&msg, 100, crate::config::DiffDisplayMode::Off);
    crate::tui::markdown::set_center_code_blocks(saved);
    let plain = lines
        .iter()
        .map(extract_line_text)
        .collect::<Vec<_>>()
        .join("\n");

    assert!(plain.contains("⛭ Validation plan"), "plain: {plain}");
    assert!(plain.contains("cargo test -p jcode-tui"), "plain: {plain}");
    assert!(plain.contains("After the block."), "plain: {plain}");
    assert!(plain.contains("Outside text."), "plain: {plain}");
    // The nested bash content stays inside the card borders.
    let bash_line = lines
        .iter()
        .map(extract_line_text)
        .find(|line| line.contains("cargo test -p jcode-tui"))
        .expect("missing bash line");
    assert!(
        bash_line.trim_start().starts_with('│'),
        "nested fence content should be inside the card: {bash_line}"
    );
}

#[test]
fn split_plan_segments_returns_none_without_plan_block() {
    assert!(split_plan_segments("Just some text\n\n```rust\nfn main() {}\n```").is_none());
    assert!(split_plan_segments("mentions plan but no fence").is_none());
}

#[test]
fn transcript_assistant_message_omits_tool_summary_restored_from_history() {
    // History restore fills `tool_calls`, live turns do not. The transcript
    // must look the same either way, so no `tool:` line in the chat view.
    let restored = DisplayMessage {
        role: "assistant".to_string(),
        content: "Formatting touched 2 extra files.".to_string(),
        tool_calls: vec!["bash".to_string(), "todo".to_string()],
        duration_secs: None,
        title: None,
        tool_data: None,
    };
    let live = DisplayMessage {
        tool_calls: Vec::new(),
        ..restored.clone()
    };
    let render = |msg: &DisplayMessage| -> Vec<String> {
        render_assistant_message(msg, 80, crate::config::DiffDisplayMode::Off)
            .iter()
            .map(extract_line_text)
            .collect()
    };
    let restored_lines = render(&restored);
    assert!(
        restored_lines
            .iter()
            .all(|line| !line.contains("tool:") && !line.contains("tools:")),
        "{restored_lines:?}"
    );
    assert_eq!(restored_lines, render(&live));
}

#[test]
fn render_assistant_message_centered_mode_keeps_markdown_unpadded_for_center_alignment() {
    let saved = crate::tui::markdown::center_code_blocks();
    crate::tui::markdown::set_center_code_blocks(true);
    let msg = DisplayMessage::assistant(
        "streaming-block streaming-block streaming-block streaming-block",
    );

    let lines = render_assistant_message(&msg, 120, crate::config::DiffDisplayMode::Off);
    let content_line = lines
        .iter()
        .find(|line| extract_line_text(line).contains("streaming-block"))
        .expect("expected assistant markdown line");

    let first_pad = extract_line_text(content_line)
        .chars()
        .take_while(|c| *c == ' ')
        .count();
    assert_eq!(
        first_pad, 0,
        "centered assistant markdown should not inject left padding: {lines:?}"
    );
    assert_eq!(
        content_line.alignment, None,
        "assistant render should leave centered prose alignment unset for outer centering"
    );

    crate::tui::markdown::set_center_code_blocks(saved);
}

#[test]
fn render_assistant_message_recenters_structured_markdown_to_actual_width() {
    let saved = crate::tui::markdown::center_code_blocks();
    crate::tui::markdown::set_center_code_blocks(true);
    let msg = DisplayMessage::assistant("- one\n- two");

    let lines = render_assistant_message(&msg, 140, crate::config::DiffDisplayMode::Off);
    let rendered: Vec<String> = lines.iter().map(extract_line_text).collect();
    let bullets: Vec<&String> = rendered.iter().filter(|line| line.contains("• ")).collect();

    assert_eq!(
        bullets.len(),
        2,
        "expected two rendered bullet lines: {rendered:?}"
    );
    let first_pad = leading_spaces(bullets[0]);
    let second_pad = leading_spaces(bullets[1]);
    assert_eq!(
        first_pad, second_pad,
        "simple list should share a block pad: {rendered:?}"
    );
    assert!(
        first_pad > 45,
        "list should be re-centered to the full display width: {rendered:?}"
    );
    assert!(
        bullets
            .iter()
            .all(|line| line[leading_spaces(line)..].starts_with("• ")),
        "bullet markers should remain flush-left within the centered block: {rendered:?}"
    );

    crate::tui::markdown::set_center_code_blocks(saved);
}

#[test]
fn render_system_message_centered_mode_caps_wrap_width_for_visible_gutters() {
    let saved = crate::tui::markdown::center_code_blocks();
    crate::tui::markdown::set_center_code_blocks(true);
    let msg = DisplayMessage::system(
        "This is a long centered-mode system notification that should keep visible side gutters instead of stretching nearly edge to edge in a wide terminal.",
    );

    let lines = render_system_message(&msg, 120, crate::config::DiffDisplayMode::Off);
    let rendered: Vec<String> = lines
        .iter()
        .map(|line| {
            line.spans
                .iter()
                .map(|span| span.content.as_ref())
                .collect()
        })
        .collect();

    assert!(
        rendered.iter().all(|line| line.starts_with("          ")),
        "centered system message should retain visible left padding in wide layouts: {rendered:?}"
    );

    crate::tui::markdown::set_center_code_blocks(saved);
}

#[test]
fn render_system_message_uses_minimal_inline_style_for_reload_title() {
    let msg = DisplayMessage::system("Reloading server with newer binary...").with_title("Reload");

    let lines = render_system_message(&msg, 80, crate::config::DiffDisplayMode::Off);
    let plain = lines
        .iter()
        .map(extract_line_text)
        .collect::<Vec<_>>()
        .join("\n");

    assert!(
        !plain.contains('╭'),
        "unexpected reload card border: {plain}"
    );
    assert!(
        !plain.contains('╰'),
        "unexpected reload card border: {plain}"
    );
    assert!(
        !plain.contains("⚡ reload"),
        "unexpected reload card title: {plain}"
    );
    assert!(plain.contains("Reloading server with newer binary"));
}

#[test]
fn render_system_message_uses_one_line_for_reconnect_status() {
    let msg = DisplayMessage::system(
        "⚡ Connection lost - retrying (attempt 2, 7s) - connection reset by server · resume: jcode --resume koala",
    )
    .with_title("Connection");

    let lines = render_system_message(&msg, 80, crate::config::DiffDisplayMode::Off);
    let plain = lines
        .iter()
        .map(extract_line_text)
        .collect::<Vec<_>>()
        .join("\n");

    assert_eq!(lines.len(), 1);
    assert!(lines[0].width() <= 80);
    assert!(plain.contains("Connection lost - retrying (attempt 2, 7s)"));
    assert!(!plain.contains('╭'));
}

#[test]
fn render_swarm_message_centered_mode_caps_wrap_width_for_long_notifications() {
    let saved = crate::tui::markdown::center_code_blocks();
    crate::tui::markdown::set_center_code_blocks(true);
    let msg = DisplayMessage::swarm(
        "File activity",
        "/home/jeremy/jcode/src/tui/ui_messages.rs - moss just edited this file while you were working nearby, so the notification should still read as centered in wide layouts.",
    );

    let lines = render_swarm_message(&msg, 120, crate::config::DiffDisplayMode::Off);
    let rendered: Vec<String> = lines.iter().map(extract_line_text).collect();
    let first_pad = rendered[0].chars().take_while(|c| *c == ' ').count();

    assert!(
        first_pad >= 8,
        "centered swarm notification should keep a clearly visible left gutter: {rendered:?}"
    );
    assert!(
        rendered
            .iter()
            .all(|line| line.is_empty() || line.starts_with(&" ".repeat(first_pad))),
        "centered swarm notification should share one left pad across wrapped lines: {rendered:?}"
    );

    crate::tui::markdown::set_center_code_blocks(saved);
}

#[test]
fn render_swarm_message_collapsed_shows_tldr_and_expand_badge_only() {
    let saved = crate::tui::markdown::center_code_blocks();
    crate::tui::markdown::set_center_code_blocks(false);
    let content = jcode_tui_messages::encode_collapsible_swarm_content(
        "fixed the flaky test",
        "The flaky test was caused by a race in the setup helper.\n\nI rewrote it to use a barrier.",
    );
    let msg = DisplayMessage::swarm("DM from sheep", content);

    let lines = render_swarm_message(&msg, 100, crate::config::DiffDisplayMode::Off);
    let plain = lines
        .iter()
        .map(extract_line_text)
        .collect::<Vec<_>>()
        .join("\n");

    assert!(plain.contains("fixed the flaky test"), "{plain}");
    assert!(plain.contains(super::SWARM_EXPAND_BADGE), "{plain}");
    let preview = lines
        .iter()
        .flat_map(|line| &line.spans)
        .find(|span| span.content.contains("fixed the flaky test"))
        .expect("DM preview span");
    assert_eq!(preview.style.fg, Some(rgb(130, 130, 140)));
    assert!(
        !plain.contains("race in the setup helper"),
        "collapsed card must hide the full body: {plain}"
    );
    crate::tui::markdown::set_center_code_blocks(saved);
}

#[test]
fn render_swarm_message_expanded_shows_body_and_collapse_badge() {
    let saved = crate::tui::markdown::center_code_blocks();
    crate::tui::markdown::set_center_code_blocks(false);
    let collapsed = jcode_tui_messages::encode_collapsible_swarm_content(
        "fixed the flaky test",
        "The flaky test was caused by a race in the setup helper.",
    );
    let expanded =
        jcode_tui_messages::toggle_collapsible_swarm_content(&collapsed).expect("toggle");
    let msg = DisplayMessage::swarm("DM from sheep", expanded);

    let lines = render_swarm_message(&msg, 100, crate::config::DiffDisplayMode::Off);
    let plain = lines
        .iter()
        .map(extract_line_text)
        .collect::<Vec<_>>()
        .join("\n");

    assert!(plain.contains("fixed the flaky test"), "{plain}");
    assert!(plain.contains(super::SWARM_COLLAPSE_BADGE), "{plain}");
    assert!(plain.contains("race in the setup helper"), "{plain}");
    crate::tui::markdown::set_center_code_blocks(saved);
}

#[test]
fn render_tool_message_prefers_subagent_title_with_model() {
    let msg = DisplayMessage {
        role: "tool".to_string(),
        content: "done".to_string(),
        tool_calls: Vec::new(),
        duration_secs: None,
        title: Some("Verify subagent model (general · gpt-5.4)".to_string()),
        tool_data: Some(crate::message::ToolCall {
            id: "call_1".to_string(),
            name: "subagent".to_string(),
            input: serde_json::json!({
                "description": "Verify subagent model",
                "subagent_type": "general"
            }),
            intent: None,
            thought_signature: None,
        }),
    };

    let lines = render_tool_message(&msg, 80, crate::config::DiffDisplayMode::Off);
    let rendered: String = lines[0]
        .spans
        .iter()
        .map(|span| span.content.as_ref())
        .collect();

    assert!(rendered.contains("subagent Verify subagent model (general · gpt-5.4)"));
}

#[test]
fn render_tool_message_shows_intent_and_technical_preview_on_one_line() {
    crate::tui::ui::tools_ui::tests_tool_call_details_override::set(true);
    let msg = DisplayMessage {
        role: "tool".to_string(),
        content: "ok".to_string(),
        tool_calls: Vec::new(),
        duration_secs: None,
        title: None,
        tool_data: Some(crate::message::ToolCall {
            id: "call_intent".to_string(),
            name: "bash".to_string(),
            input: serde_json::json!({
                "command": "cargo test -p jcode render_background_task --lib",
                "intent": "Verify compact progress card"
            }),
            intent: Some("Verify compact progress card".to_string()),
            thought_signature: None,
        }),
    };

    let lines = render_tool_message(&msg, 120, crate::config::DiffDisplayMode::Off);
    let rendered = extract_line_text(&lines[0]);

    assert!(rendered.contains("bash · Verify compact progress card · $ cargo test"));
    assert_eq!(lines.len(), 1, "Bash output is hidden by default");
    crate::tui::ui::tools_ui::tests_tool_call_details_override::set(false);
}

/// Default (tool_call_details off): a row with an intent renders only the
/// intent; the dimmed technical preview is dropped and no fallback command
/// line is added.
#[test]
fn render_tool_message_hides_technical_preview_by_default() {
    let msg = DisplayMessage {
        role: "tool".to_string(),
        content: "ok".to_string(),
        tool_calls: Vec::new(),
        duration_secs: None,
        title: None,
        tool_data: Some(crate::message::ToolCall {
            id: "call_intent".to_string(),
            name: "bash".to_string(),
            input: serde_json::json!({
                "command": "cargo test -p jcode render_background_task --lib",
                "intent": "Verify compact progress card"
            }),
            intent: Some("Verify compact progress card".to_string()),
            thought_signature: None,
        }),
    };

    let lines = render_tool_message(&msg, 120, crate::config::DiffDisplayMode::Off);
    let rendered = extract_line_text(&lines[0]);

    assert!(
        rendered.contains("bash · Verify compact progress card"),
        "rendered={rendered}"
    );
    assert!(
        !rendered.contains("cargo test"),
        "technical detail should be hidden by default: {rendered}"
    );
    assert_eq!(lines.len(), 1, "Bash output is hidden by default");
}

/// Failures retain their normal header, with diagnostics on a muted second row.
#[test]
fn render_tool_message_keeps_error_summary_when_details_hidden() {
    let msg = DisplayMessage {
        role: "tool".to_string(),
        content: "Error: command not found: cargoo".to_string(),
        tool_calls: Vec::new(),
        duration_secs: None,
        title: None,
        tool_data: Some(crate::message::ToolCall {
            id: "call_intent_err".to_string(),
            name: "bash".to_string(),
            input: serde_json::json!({
                "command": "cargoo test",
                "intent": "Run the test suite"
            }),
            intent: Some("Run the test suite".to_string()),
            thought_signature: None,
        }),
    };

    let lines = render_tool_message(&msg, 120, crate::config::DiffDisplayMode::Off);
    let rendered = extract_line_text(&lines[0]);

    assert!(
        rendered.contains("Run the test suite ·"),
        "normal intent and token suffix should remain: {rendered}"
    );
    assert!(!rendered.contains("command not found"));
    assert_eq!(lines.len(), 2);
    assert!(
        extract_line_text(&lines[1])
            .trim_start()
            .starts_with("╰─command not found")
    );
    assert!(
        lines[1]
            .spans
            .iter()
            .all(|span| span.content.trim().is_empty() || span.style.fg == Some(dim_color()))
    );
}

#[test]
fn render_tool_message_shows_token_badge() {
    let msg = DisplayMessage {
        role: "tool".to_string(),
        content: "x".repeat(7_600),
        tool_calls: Vec::new(),
        duration_secs: None,
        title: None,
        tool_data: Some(crate::message::ToolCall {
            id: "call_2".to_string(),
            name: "read".to_string(),
            input: serde_json::json!({"file_path": "src/main.rs"}),
            intent: None,
            thought_signature: None,
        }),
    };

    let lines = render_tool_message(&msg, 120, crate::config::DiffDisplayMode::Off);
    let badge_span = lines[0]
        .spans
        .iter()
        .find(|span| span.content.contains("1.9k tok"))
        .expect("missing token badge");

    assert_eq!(badge_span.style.fg, Some(rgb(118, 118, 118)));
}

#[test]
fn render_tool_message_failures_keep_normal_headers_and_muted_reasons() {
    for (name, input, description) in [
        (
            "read",
            serde_json::json!({"file_path": "src/文件.rs"}),
            "src/文件.rs",
        ),
        (
            "edit",
            serde_json::json!({"file_path": "src/文件.rs", "old_string": "old", "new_string": "new"}),
            "src/文件.rs",
        ),
        (
            "bash",
            serde_json::json!({"command": "cat 文件.rs"}),
            "cat 文件.rs",
        ),
    ] {
        for intent in [None, Some("Inspect 文件 e\u{301}")] {
            let msg = DisplayMessage {
                role: "tool".to_string(),
                content: "Error: permission denied 文件\nFull diagnostic retained".to_string(),
                tool_calls: Vec::new(),
                duration_secs: None,
                title: None,
                tool_data: Some(crate::message::ToolCall {
                    id: "failure_style".to_string(),
                    name: name.to_string(),
                    input: input.clone(),
                    intent: intent.map(str::to_string),
                    thought_signature: None,
                }),
            };
            let lines = render_tool_message(&msg, 140, crate::config::DiffDisplayMode::Inline);
            assert_eq!(lines.len(), 2, "{name}");
            let header = extract_line_text(&lines[0]);
            assert!(
                header.trim_start().starts_with(&format!("✗ {name}")),
                "{header}"
            );
            assert!(header.contains(intent.unwrap_or(description)), "{header}");
            assert!(!header.contains("permission denied"));
            assert!(lines[0].spans.iter().any(
                |span| span.content.contains("✗") && span.style.fg == Some(rgb(220, 100, 100))
            ));
            assert!(
                lines[0]
                    .spans
                    .iter()
                    .any(|span| span.content == name && span.style.fg == Some(tool_color()))
            );
            assert!(
                extract_line_text(&lines[1])
                    .trim_start()
                    .starts_with("╰─permission denied")
            );
            assert!(
                lines[1].spans.iter().all(
                    |span| span.content.trim().is_empty() || span.style.fg == Some(dim_color())
                )
            );
            assert!(msg.content.contains("Full diagnostic retained"));
            for width in [0, 1, 2, 5, 8, 16, 32] {
                for line in render_tool_message(&msg, width, crate::config::DiffDisplayMode::Inline)
                {
                    assert!(
                        UnicodeWidthStr::width(extract_line_text(&line).as_str()) <= width as usize
                    );
                }
            }
            let mut successful = msg.clone();
            successful.content = "ok".to_string();
            let success_lines =
                render_tool_message(&successful, 140, crate::config::DiffDisplayMode::Off);
            assert_eq!(
                header.find('✗'),
                extract_line_text(&success_lines[0]).find('✓')
            );
            assert_eq!(
                extract_line_text(&lines[1]).find('╰').unwrap(),
                header.find('✗').unwrap()
            );
            assert!(
                extract_line_text(&success_lines[0])
                    .trim_start()
                    .starts_with(&format!("✓ {name}"))
            );
            assert!(
                !success_lines
                    .iter()
                    .any(|line| extract_line_text(line).contains("╰"))
            );
        }
    }
}

#[test]
fn render_tool_message_mixed_batch_keeps_call_labels_and_nested_failure_reason() {
    let msg = DisplayMessage {
        role: "tool".to_string(),
        content: "--- [1] read ---\nError: permission denied 文件\nFull diagnostic\n\n--- [2] bash ---\nok\n\nCompleted: 1 succeeded, 1 failed".to_string(),
        tool_calls: Vec::new(), duration_secs: None, title: None,
        tool_data: Some(crate::message::ToolCall {
            id: "mixed".to_string(), name: "batch".to_string(),
            input: serde_json::json!({"tool_calls": [
                {"tool": "read", "parameters": {"file_path": "文件.rs"}, "intent": "Read 文件"},
                {"tool": "bash", "parameters": {"command": "true"}, "intent": "Check success"}
            ]}), intent: None, thought_signature: None,
        }),
    };
    let lines = render_tool_message(&msg, 140, crate::config::DiffDisplayMode::Off);
    assert_eq!(lines.len(), 4);
    assert_eq!(
        extract_line_text(&lines[2]).find('╰').unwrap(),
        extract_line_text(&lines[1]).find('✗').unwrap()
    );
    assert!(extract_line_text(&lines[0]).contains("⚠ batch"));
    assert!(
        extract_line_text(&lines[1])
            .trim_start()
            .starts_with("✗ read · Read 文件")
    );
    assert!(!extract_line_text(&lines[1]).contains("permission"));
    assert!(
        extract_line_text(&lines[2])
            .trim_start()
            .starts_with("╰─permission denied")
    );
    assert!(
        lines[2]
            .spans
            .iter()
            .all(|span| span.content.trim().is_empty() || span.style.fg == Some(dim_color()))
    );
    assert!(
        extract_line_text(&lines[3])
            .trim_start()
            .starts_with("✓ bash · Check success")
    );
    for width in [0, 1, 2, 8, 16, 32] {
        for line in render_tool_message(&msg, width, crate::config::DiffDisplayMode::Off) {
            assert!(UnicodeWidthStr::width(extract_line_text(&line).as_str()) <= width as usize);
        }
    }
}

#[test]
fn render_tool_message_batch_level_failure_does_not_invent_successful_children() {
    let msg = DisplayMessage {
        role: "tool".to_string(),
        content: "Error: invalid tool_calls payload".to_string(),
        tool_calls: Vec::new(),
        duration_secs: None,
        title: None,
        tool_data: Some(crate::message::ToolCall {
            id: "invalid_batch".to_string(),
            name: "batch".to_string(),
            input: serde_json::json!({"tool_calls": [{"tool": "read"}]}),
            intent: Some("Inspect sources".to_string()),
            thought_signature: None,
        }),
    };
    let lines = render_tool_message(&msg, 120, crate::config::DiffDisplayMode::Off);
    assert_eq!(lines.len(), 2);
    assert!(extract_line_text(&lines[0]).contains("✗ batch · Inspect sources"));
    assert!(extract_line_text(&lines[1]).contains("╰─invalid tool_calls payload"));
}

#[test]
fn compact_error_rows_normalize_logged_wrappers_and_align_under_status() {
    let raw = "Error: [selfdev] Error: `selfdev build` requires a non-empty `reason` so other queued agents can see why the build is needed.";
    let mut msg = DisplayMessage::system(raw);
    msg.role = "tool".to_string();
    msg.tool_data = Some(crate::message::ToolCall {
        id: "logged-selfdev".to_string(),
        name: "selfdev".to_string(),
        input: serde_json::json!({"action": "build"}),
        intent: Some("Build UI".to_string()),
        thought_signature: None,
    });
    let lines = render_tool_message(&msg, 180, crate::config::DiffDisplayMode::Off);
    assert_eq!(lines.len(), 2);
    let reason = extract_line_text(&lines[1]);
    assert!(
        reason.contains("`selfdev build` requires a non-empty `reason`"),
        "{reason}"
    );
    assert!(
        !reason.contains("Error:") && !reason.contains("error:") && !reason.contains("[selfdev]"),
        "{reason}"
    );
    let wide_reason = extract_line_text(&render_tool_failure_reason(raw, 180, 2));
    assert!(
        !wide_reason.contains('…'),
        "wide reasons must not be pre-truncated: {wide_reason}"
    );
    assert_eq!(extract_line_text(&lines[0]).find('✗'), reason.find('╰'));
    assert_eq!(msg.content, raw, "normalization must not change raw logs");
}

#[test]
fn background_errors_share_cause_selection_and_deduplicate_exit_codes() {
    for (preview, expected) in [
        (
            "Error: [bash] Error: Command exited with code 101\n[stderr] error[E0308]: mismatched types",
            "exit 101 · [E0308]: mismatched types",
        ),
        (
            "Error: [bash] Error: Command exited with code 101",
            "exit 101",
        ),
    ] {
        let msg = DisplayMessage::background_task(format!(
            "**Background task** `logged-bg` · `bash` · ✗ failed · 1.0s · exit 101\n\n```text\n{preview}\n```\n\n_Full output:_ `bg action=\"output\" task_id=\"logged-bg\"`"
        ));
        let lines = render_background_task_message(&msg, 160, crate::config::DiffDisplayMode::Off);
        let reason = extract_line_text(&lines[1]);
        assert!(reason.ends_with(expected), "{reason}");
        assert_eq!(reason.matches("exit 101").count(), 1, "{reason}");
        assert_eq!(extract_line_text(&lines[0]).find('✗'), reason.find('╰'));
    }
}

#[test]
fn background_explicit_failure_reason_survives_without_prefix_or_preview() {
    let msg = DisplayMessage::background_task(
        "**Background task** `reason-only` · `bash` · ✗ failed · 1.0s · exit 1\n\n_Failure:_ permission denied\n\n_No output captured._\n\n_Full output:_ `bg action=\"output\" task_id=\"reason-only\"`",
    );
    let lines = render_background_task_message(&msg, 100, crate::config::DiffDisplayMode::Off);
    assert!(extract_line_text(&lines[1]).ends_with("exit 1 · permission denied"));
}

#[test]
fn tool_failure_reason_handles_exit_codes_fallback_and_unicode_widths() {
    for (content, expected) in [
        (
            "details\n--- Command finished with exit code: 17 ---",
            "exit 17",
        ),
        (
            "[apply_patch] ✗ 文件: Failed to find expected lines\nlarge diff",
            "Failed to find expected lines",
        ),
        (
            "\u{1b}[31mError: 文件 e\u{301} missing\u{1b}[0m\nmore detail",
            "文件 e\u{301} missing",
        ),
        ("", "Tool call failed"),
        (
            "[tool timing: start=2026-10-23T00:00:00Z]\n[stderr] warning: unused variable\n[stderr] error[E0308]: mismatched types\n--- Command finished with exit code: 101 ---",
            "[E0308]: mismatched types",
        ),
        (
            "[tool timing: start=2026-10-23T00:00:00Z]\n[stderr] Error: permission denied",
            "permission denied",
        ),
    ] {
        let reason = render_tool_failure_reason(content, 120, 4);
        let text = extract_line_text(&reason);
        assert!(text.contains(expected), "{text}");
        assert!(!text.contains('\u{1b}'));
        for width in [0, 1, 4, 8, 16, 32] {
            let line = render_tool_failure_reason(content, width, 4);
            assert!(UnicodeWidthStr::width(extract_line_text(&line).as_str()) <= width);
        }
    }
}

#[test]
fn render_tool_message_hides_bash_output() {
    let msg = DisplayMessage {
        role: "tool".to_string(),
        content: "<class 'zip'>\n[('p', 'b'), ('a', 'a'), ('l', 'l'), ('e', 'e')]".to_string(),
        tool_calls: Vec::new(),
        duration_secs: None,
        title: None,
        tool_data: Some(crate::message::ToolCall {
            id: "call_bash_output".to_string(),
            name: "bash".to_string(),
            input: serde_json::json!({
                "command": "python3 -c \"s='pale'; t='bale'; print(type(zip(s,t))); print(list(zip(s,t)))\""
            }),
            intent: None,
            thought_signature: None,
        }),
    };

    let lines = render_tool_message(&msg, 120, crate::config::DiffDisplayMode::Off);
    let rendered = lines.iter().map(extract_line_text).collect::<Vec<_>>();

    assert!(!rendered.iter().any(|line| line.contains("<class 'zip'>")));
    assert!(!rendered.iter().any(|line| line.contains("[('p', 'b')")));
}

#[test]
fn render_tool_message_shows_bash_output_when_enabled() {
    crate::tui::ui::tools_ui::tests_show_bash_output_override::set(true);
    let msg = DisplayMessage {
        role: "tool".to_string(),
        content: "one\ntwo\nthree\nfour".to_string(),
        tool_calls: Vec::new(),
        duration_secs: None,
        title: None,
        tool_data: Some(crate::message::ToolCall {
            id: "call_bash_output_enabled".to_string(),
            name: "bash".to_string(),
            input: serde_json::json!({"command": "printf output"}),
            intent: Some("Print output".to_string()),
            thought_signature: None,
        }),
    };

    let rendered = render_tool_message(&msg, 120, crate::config::DiffDisplayMode::Off)
        .iter()
        .map(extract_line_text)
        .collect::<Vec<_>>();

    assert_eq!(rendered.len(), 4);
    assert!(!rendered.iter().any(|line| line.trim() == "one"));
    assert!(rendered.iter().any(|line| line.trim() == "two"));
    assert!(rendered.iter().any(|line| line.trim() == "four"));
    crate::tui::ui::tools_ui::tests_show_bash_output_override::set(false);
}

include!("gmail_drafts_tests.rs");

#[test]
fn render_batch_tool_message_shows_flat_and_nested_subcall_intents() {
    crate::tui::ui::tools_ui::tests_tool_call_details_override::set(true);
    let msg = DisplayMessage {
        role: "tool".to_string(),
        content: "--- [1] read ---\nflat output\n\n--- [2] read ---\nnested output\n\nCompleted: 2 succeeded, 0 failed".to_string(),
        tool_calls: Vec::new(),
        duration_secs: None,
        title: None,
        tool_data: Some(crate::message::ToolCall {
            id: "call_batch_intents".to_string(),
            name: "batch".to_string(),
            input: serde_json::json!({
                "tool_calls": [
                    {
                        "tool": "read",
                        "intent": "Inspect flat batch input",
                        "file_path": "src/flat.rs"
                    },
                    {
                        "tool": "read",
                        "parameters": {
                            "intent": "Inspect nested batch input",
                            "file_path": "src/nested.rs"
                        }
                    }
                ]
            }),
            intent: None,
            thought_signature: None,
        }),
    };

    let plain = render_tool_message(&msg, 120, crate::config::DiffDisplayMode::Off)
        .iter()
        .map(extract_line_text)
        .collect::<Vec<_>>()
        .join("\n");

    assert!(
        plain.contains("read · Inspect flat batch input ·"),
        "{plain}"
    );
    assert!(
        plain.contains("read · Inspect nested batch input ·"),
        "{plain}"
    );
    assert!(plain.contains("flat.rs"), "{plain}");
    assert!(plain.contains("nested.rs"), "{plain}");
    crate::tui::ui::tools_ui::tests_tool_call_details_override::set(false);
}

fn discovery_message(content: &str, input: serde_json::Value) -> DisplayMessage {
    DisplayMessage {
        role: "tool".to_string(),
        content: content.to_string(),
        tool_calls: Vec::new(),
        duration_secs: None,
        title: None,
        tool_data: Some(crate::message::ToolCall {
            id: "call_discovery".to_string(),
            name: "integration_tools".to_string(),
            input,
            intent: None,
            thought_signature: None,
        }),
    }
}

#[test]
fn render_tool_message_shows_discovery_browse_results_and_rationale() {
    let msg = discovery_message(
        "Discoverable tools in 'payments' (Jcode tool directory; recommendations must be based only on fit; details: https://jcode.sh/discovery-tools):\n\n- agentcard: prepaid virtual Visa cards for AI agents (https://agentcard.sh/?via=jcode-discovery)\n\nSearch request ID: `11111111-2222-4333-8444-555555555555`",
        serde_json::json!({
            "action": "search",
            "category": "payments",
            "query": "manage Stripe sandbox products and recurring prices",
            "reason": "the task needs test-mode catalog administration through scoped agent access"
        }),
    );
    let lines = render_tool_message(&msg, 100, crate::config::DiffDisplayMode::Off);
    let plain = lines
        .iter()
        .map(extract_line_text)
        .collect::<Vec<_>>()
        .join("\n");
    assert!(plain.contains("agentcard"), "{plain}");
    assert!(
        !plain.contains("1 integration"),
        "single-result browse shows only the entry name: {plain}"
    );
    assert!(
        !plain.contains("why:"),
        "browse results stay to a single line without rationale: {plain}"
    );
    assert!(
        !plain.contains("prepaid virtual Visa cards"),
        "browse results must not render descriptions: {plain}"
    );
    assert!(
        !plain.contains("agentcard.sh"),
        "browse results must not render URLs: {plain}"
    );
    assert!(
        !plain.contains("Listings are vetted"),
        "discovery results must not render the disclosure notice: {plain}"
    );
    assert!(!plain.contains("sponsored result"), "{plain}");
    assert!(
        lines.len() <= 8,
        "compact discovery details used {} lines: {plain}",
        lines.len()
    );
    assert!(
        !plain.contains("\n\n"),
        "compact details contain a blank row: {plain}"
    );
    assert!(
        !plain
            .chars()
            .any(|ch| matches!(ch, '╭' | '╮' | '╰' | '╯' | '│')),
        "discovery details must remain borderless: {plain}"
    );
    assert!(
        !plain.contains("11111111-2222"),
        "request IDs stay model-only: {plain}"
    );
}

#[test]
fn batched_discovery_renders_without_disclosure_notice() {
    let msg = DisplayMessage {
        role: "tool".to_string(),
        content: "--- [1] integration_tools ---\nAvailable integrations in 'payments' (Jcode tool directory; recommendations must be based only on fit; details: https://jcode.sh/discovery-tools):\n\n- agentcard: prepaid virtual Visa cards for AI agents (https://agentcard.sh/?via=jcode-discovery)\n\nSearch request ID: `11111111-2222-4333-8444-555555555555`\n\nCompleted: 1 succeeded, 0 failed".to_string(),
        tool_calls: Vec::new(),
        duration_secs: None,
        title: None,
        tool_data: Some(crate::message::ToolCall {
            id: "call_batch_discovery".to_string(),
            name: "batch".to_string(),
            input: serde_json::json!({
                "tool_calls": [{
                    "tool": "integration_tools",
                    "parameters": {
                        "action": "search",
                        "category": "payments",
                        "query": "issue a capped virtual card",
                        "reason": "the task requires a payment instrument with a hard limit"
                    }
                }]
            }),
            intent: None,
            thought_signature: None,
        }),
    };

    let lines = render_tool_message(&msg, 100, crate::config::DiffDisplayMode::Off);
    let plain = lines
        .iter()
        .map(extract_line_text)
        .collect::<Vec<_>>()
        .join("\n");

    assert!(plain.contains("agentcard"), "{plain}");
    assert!(
        !plain.contains("1 integration"),
        "single-result browse shows only the entry name: {plain}"
    );
    assert!(
        !plain.contains("Listings are vetted"),
        "batched discovery must not render the disclosure notice: {plain}"
    );
    assert!(
        !plain
            .chars()
            .any(|ch| matches!(ch, '╭' | '╮' | '╰' | '╯' | '│')),
        "batched discovery details must remain borderless: {plain}"
    );
}

#[test]
fn render_tool_message_shows_selected_discovery_setup() {
    let msg = discovery_message(
        "Selected 'agentcard' from 'payments' (Jcode tool directory; selection must be based only on fit; details: https://jcode.sh/discovery-tools):\n\nagentcard: prepaid virtual Visa cards for AI agents (https://agentcard.sh/?via=jcode-discovery)\n\nSetup: Run `npx -y agentcard-mcp@1.2.3`, then connect the resulting MCP server.\n\nConsequential actions (signups, spending) must note the partnership in the confirmation shown to the user.",
        serde_json::json!({
            "action": "select",
            "category": "payments",
            "tool": "agentcard",
            "query": "create a capped virtual card for an online purchase",
            "reason": "selected because capped cards fit the purchase constraints better than alternatives"
        }),
    );
    let lines = render_tool_message(&msg, 100, crate::config::DiffDisplayMode::Off);
    let plain = lines
        .iter()
        .map(extract_line_text)
        .collect::<Vec<_>>()
        .join("\n");
    assert!(plain.contains("selected agentcard"), "{plain}");
    assert!(!plain.contains("sponsored"), "{plain}");
    assert!(
        plain.contains("details: prepaid virtual Visa cards"),
        "{plain}"
    );
    assert!(plain.contains("https://agentcard.sh"), "{plain}");
    assert!(plain.contains("setup:"), "{plain}");
    assert!(plain.contains("agentcard-mcp@1.2.3"), "{plain}");
    assert!(
        !plain.contains("Listings are vetted"),
        "discovery results must not render the disclosure notice: {plain}"
    );
}

#[test]
fn render_tool_message_does_not_duplicate_selected_when_tool_is_missing() {
    let msg = discovery_message(
        "Selection recorded.",
        serde_json::json!({
            "action": "select",
            "category": "web-search",
            "query": "find current public estimates"
        }),
    );
    let lines = render_tool_message(&msg, 100, crate::config::DiffDisplayMode::Off);
    let plain = lines
        .iter()
        .map(extract_line_text)
        .collect::<Vec<_>>()
        .join("\n");
    assert!(plain.contains("selected tool"), "{plain}");
    assert!(!plain.contains("selected selected tool"), "{plain}");
}

#[test]
fn render_tool_message_marks_off_catalog_selection_without_fake_details() {
    let msg = discovery_message(
        "Selected off-catalog product 'firecrawl' for 'web-data'.\n\nSelection recorded as demand data. Jcode does not list or partner with this product, so no provider information, recommendation, or setup instructions are provided.",
        serde_json::json!({
            "action": "select",
            "category": "web-data",
            "tool": "firecrawl",
            "query": "crawl a documentation site and extract structured markdown",
            "reason": "the user explicitly requested Firecrawl instead of the catalog listing"
        }),
    );
    let lines = render_tool_message(&msg, 100, crate::config::DiffDisplayMode::Off);
    let plain = lines
        .iter()
        .map(extract_line_text)
        .collect::<Vec<_>>()
        .join("\n");
    assert!(plain.contains("selected off-catalog firecrawl"), "{plain}");
    assert!(
        plain.contains("why: the user explicitly requested"),
        "{plain}"
    );
    assert!(!plain.contains("details:"), "{plain}");
    assert!(!plain.contains("setup:"), "{plain}");
}

#[test]
fn render_tool_message_shows_catalog_suggestion_receipt_and_trust_line() {
    let msg = discovery_message(
        "Catalog suggestion submitted.\n\nSuggestion ID: aaaaaaaa-bbbb-4ccc-8ddd-eeeeeeeeeeee\nCategory: payments\nKind: known_product\nCapability: manage Stripe sandbox products\nCatalog gap: no matching catalog entry\nProduct: Stripe sandbox MCP\nPublic URL: https://example.com/stripe-mcp\n\nStatus: received for Jcode maintainer review. Suggestions are not sent to partners. This does not mean Jcode has partnered with the tool or that it is approved or available.",
        serde_json::json!({
            "action": "suggest",
            "category": "payments",
            "query": "manage Stripe sandbox products and recurring prices through scoped agent access",
            "reason": "the listed payment tool only provides cards and cannot administer Stripe test data",
            "suggestion_kind": "known_product",
            "product_name": "Stripe sandbox MCP",
            "product_url": "https://example.com/stripe-mcp",
            "gap_evidence": "Agentcard handles cards rather than Stripe test-mode objects.",
            "requirements": [
                "Scoped authentication without exposing a secret key",
                "Create recurring prices in test mode"
            ],
            "prior_request_id": "11111111-2222-4333-8444-555555555555"
        }),
    );
    let lines = render_tool_message(&msg, 100, crate::config::DiffDisplayMode::Off);
    let plain = lines
        .iter()
        .map(extract_line_text)
        .collect::<Vec<_>>()
        .join("\n");
    assert!(plain.contains("suggestion sent"), "{plain}");
    assert!(
        plain.contains("Known product · Stripe sandbox MCP"),
        "{plain}"
    );
    assert!(plain.contains("gap: the listed payment tool"), "{plain}");
    assert!(plain.contains("needs:"), "{plain}");
    assert!(plain.contains("Jcode maintainers only"), "{plain}");
    assert!(plain.contains("not approval or availability"), "{plain}");
    assert!(
        !plain.contains("11111111-2222"),
        "prior request ID must stay hidden: {plain}"
    );
}

#[test]
fn discovery_cards_wrap_within_narrow_transcript_width() {
    let msg = discovery_message(
        "Catalog suggestion submitted.\n\nStatus: received for Jcode maintainer review.",
        serde_json::json!({
            "action": "suggest",
            "category": "cloud-infrastructure",
            "query": "a deliberately long capability description that must wrap cleanly in a narrow terminal",
            "reason": "the current catalog entries do not satisfy several detailed infrastructure constraints",
            "suggestion_kind": "capability_gap",
            "requirements": ["A long requirement that also needs reliable narrow-width wrapping"]
        }),
    );
    let lines = render_tool_message(&msg, 48, crate::config::DiffDisplayMode::Off);
    assert!(
        lines.iter().all(|line| line.width() <= 47),
        "discovery card exceeded width: {:?}",
        lines.iter().map(extract_line_text).collect::<Vec<_>>()
    );
}

#[test]
fn render_tool_message_groups_duration_and_tokens() {
    for name in [
        "bash",
        "read",
        "edit",
        "webfetch",
        "debug_socket",
        "unknown_tool",
    ] {
        for content in ["ok", "Error: failed"] {
            let mut msg = DisplayMessage::tool(
                content,
                crate::message::ToolCall {
                    id: "timed".into(),
                    name: name.into(),
                    input: serde_json::json!({}),
                    intent: Some("A completed tool call".into()),
                    thought_signature: None,
                },
            );
            msg.duration_secs = Some(2.4);
            let tokens =
                crate::util::format_approx_token_count(crate::util::estimate_tokens(content));
            for width in [32, 60, 120] {
                let lines = render_tool_message(&msg, width, crate::config::DiffDisplayMode::Off);
                let row = extract_line_text(&lines[0]);
                assert!(row.contains(&format!("2.4s, {tokens}")), "{name}: {row}");
                assert!(!row.contains("2.4s ·"), "{row}");
                assert!(lines[0].width() < width as usize, "{row}");
            }
        }
    }
}

#[test]
fn batch_subcall_groups_duration_and_tokens() {
    let tool = crate::message::ToolCall {
        id: "timed-child".into(),
        name: "read".into(),
        input: serde_json::json!({}),
        intent: Some("Read configuration".into()),
        thought_signature: None,
    };
    let line = crate::tui::ui::tools_ui::render_batch_subcall_line(
        &tool,
        "✓",
        rgb(100, 180, 100),
        50,
        Some(80),
        Some("[tool timing: duration=73ms] config content"),
    );
    let text = extract_line_text(&line);
    assert!(text.contains(" · 73ms, "), "{text}");
    assert!(text.ends_with(" tok"), "{text}");
    assert!(line.width() <= 80);
}

#[test]
fn tool_usage_label_handles_missing_and_legacy_duration() {
    use crate::tui::ui::tools_ui::tool_usage_label;
    assert_eq!(tool_usage_label("217 tok", None, "ok"), "217 tok");
    assert_eq!(tool_usage_label("217 tok", Some(0.0), "ok"), "0ms, 217 tok");
    assert_eq!(tool_usage_label("217 tok", Some(f32::NAN), "ok"), "217 tok");
    assert_eq!(
        tool_usage_label(
            "217 tok",
            None,
            "[tool timing: start=x finish=y duration=2.4s] ok"
        ),
        "2.4s, 217 tok"
    );
    assert_eq!(
        tool_usage_label("217 tok", None, "[tool timing: duration=1m 4s] ok"),
        "1m 4s, 217 tok"
    );
    assert_eq!(
        tool_usage_label("217 tok", None, "[tool timing: duration=73ms] ok"),
        "73ms, 217 tok"
    );
    assert_eq!(
        tool_usage_label("217 tok", None, "[tool timing: duration=NaNs] ok"),
        "217 tok"
    );
    assert_eq!(
        tool_usage_label("217 tok", None, "[tool timing: duration=invalid] ok"),
        "217 tok"
    );
}

#[test]
fn render_tool_message_colors_duration_independently_from_tokens() {
    for (seconds, duration_color) in [
        (0.42, rgb(118, 118, 118)),
        (9.99, rgb(118, 118, 118)),
        (10.0, rgb(214, 184, 92)),
        (59.99, rgb(214, 184, 92)),
        (60.0, rgb(224, 118, 118)),
    ] {
        for content in ["ok".to_owned(), "x".repeat(48_000)] {
            let mut msg = DisplayMessage::tool(
                content,
                crate::message::ToolCall {
                    id: "independent-metrics".into(),
                    name: "read".into(),
                    input: serde_json::json!({}),
                    intent: None,
                    thought_signature: None,
                },
            );
            msg.duration_secs = Some(seconds);
            let badge = tool_output_token_badge(&msg.content);
            for width in [32, 120] {
                let lines = render_tool_message(&msg, width, crate::config::DiffDisplayMode::Off);
                let spans = &lines[0].spans;
                let token_index = spans
                    .iter()
                    .position(|span| span.content == badge.label)
                    .expect("missing token span");
                assert_eq!(spans[token_index].style.fg, Some(badge.color));
                assert_eq!(spans[token_index - 1].content, ", ");
                assert_eq!(spans[token_index - 2].style.fg, Some(duration_color));
                assert!(lines[0].width() < width as usize);
            }
        }
    }
}

#[test]
fn batch_subcall_colors_duration_independently_from_tokens() {
    let tool = crate::message::ToolCall {
        id: "timed-child".into(),
        name: "read".into(),
        input: serde_json::json!({}),
        intent: None,
        thought_signature: None,
    };
    for (timing, color) in [
        ("420ms", rgb(118, 118, 118)),
        ("10s", rgb(214, 184, 92)),
        ("1m", rgb(224, 118, 118)),
    ] {
        let content = format!("[tool timing: duration={timing}] {}", "x".repeat(48_000));
        let line = crate::tui::ui::tools_ui::render_batch_subcall_line(
            &tool,
            "✓",
            rgb(100, 180, 100),
            50,
            Some(60),
            Some(&content),
        );
        let duration = line
            .spans
            .iter()
            .find(|span| span.content == timing)
            .expect("missing duration span");
        assert_eq!(duration.style.fg, Some(color));
        let tokens = line
            .spans
            .iter()
            .find(|span| span.content.ends_with(" tok"))
            .expect("missing token span");
        assert_eq!(tokens.style.fg, Some(rgb(224, 118, 118)));
        assert!(line.width() <= 60);
    }
}

#[test]
fn render_tool_message_colors_high_token_badge() {
    let msg = DisplayMessage {
        role: "tool".to_string(),
        content: "x".repeat(48_000),
        tool_calls: Vec::new(),
        duration_secs: None,
        title: None,
        tool_data: Some(crate::message::ToolCall {
            id: "call_3".to_string(),
            name: "read".to_string(),
            input: serde_json::json!({"file_path": "src/main.rs"}),
            intent: None,
            thought_signature: None,
        }),
    };

    let lines = render_tool_message(&msg, 120, crate::config::DiffDisplayMode::Off);
    let badge_span = lines[0]
        .spans
        .iter()
        .find(|span| span.content.contains("12k tok"))
        .expect("missing token badge");

    assert_eq!(badge_span.style.fg, Some(rgb(224, 118, 118)));
}

#[test]
fn render_tool_message_shows_inline_diff_for_pascal_case_multiedit() {
    let msg = DisplayMessage {
        role: "tool".to_string(),
        content: "Edited demo.txt\n\nApplied:\n  ✓ Edit 1: replaced 1 occurrence\n\nTotal: 1 applied, 0 failed\n"
            .to_string(),
        tool_calls: Vec::new(),
        duration_secs: None,
        title: Some("demo.txt".to_string()),
        tool_data: Some(crate::message::ToolCall {
            id: "call_multiedit_pascal".to_string(),
            name: "MultiEdit".to_string(),
            input: serde_json::json!({
                "file_path": "demo.txt",
                "edits": [
                    {"old_string": "old line\n", "new_string": "new line\n"}
                ]
            }),
            intent: None, thought_signature: None, }),
    };

    let lines = render_tool_message(&msg, 100, crate::config::DiffDisplayMode::Inline);
    let plain = lines
        .iter()
        .map(extract_line_text)
        .collect::<Vec<_>>()
        .join("\n");

    assert!(plain.contains("╭─ diff"), "plain={plain}");
    assert!(plain.contains("old line"), "plain={plain}");
    assert!(plain.contains("new line"), "plain={plain}");
}

#[test]
fn render_tool_message_labels_single_file_apply_patch_diff() {
    let msg = DisplayMessage {
        role: "tool".to_string(),
        content: "✓ src/example.rs: modified (1 hunks)".to_string(),
        tool_calls: Vec::new(),
        duration_secs: None,
        title: Some("src/example.rs".to_string()),
        tool_data: Some(crate::message::ToolCall {
            id: "call_apply_patch_single".to_string(),
            name: "apply_patch".to_string(),
            input: serde_json::json!({
                "intent": "Update example behavior",
                "patch_text": "*** Begin Patch\n*** Update File: src/example.rs\n@@\n-old_value\n+new_value\n*** End Patch\n"
            }),
            intent: Some("Update example behavior".to_string()),
            thought_signature: None,
        }),
    };

    let lines = render_tool_message(&msg, 100, crate::config::DiffDisplayMode::Inline);
    let plain = lines
        .iter()
        .map(extract_line_text)
        .collect::<Vec<_>>()
        .join("\n");

    assert!(
        plain.contains("╭─ diff · +1 -1 · src/example.rs"),
        "plain={plain}"
    );
    assert!(plain.contains("old_value"), "plain={plain}");
    assert!(plain.contains("new_value"), "plain={plain}");
}

#[test]
fn render_tool_message_preserves_multi_file_apply_patch_boundaries() {
    let msg = DisplayMessage {
        role: "tool".to_string(),
        content: "✓ a.txt: modified (1 hunks)\n1- old a\n1+ new a\n✓ b.txt: modified (1 hunks)\n1- old b\n1+ new b\n".to_string(),
        tool_calls: Vec::new(),
        duration_secs: None,
        title: Some("2 files".to_string()),
        tool_data: Some(crate::message::ToolCall {
            id: "call_apply_patch_multi".to_string(),
            name: "apply_patch".to_string(),
            input: serde_json::json!({
                "intent": "Update both examples",
                "patch_text": "*** Begin Patch\n*** Update File: a.txt\n@@\n-old a\n+new a\n*** Update File: b.txt\n@@\n-old b\n+new b\n*** End Patch\n"
            }),
            intent: Some("Update both examples".to_string()),
            thought_signature: None,
        }),
    };

    let lines = render_tool_message(&msg, 100, crate::config::DiffDisplayMode::Inline);
    let plain = lines
        .iter()
        .map(extract_line_text)
        .collect::<Vec<_>>()
        .join("\n");

    let a_header = plain
        .find("╭─ diff · +2 -2 · a.txt")
        .expect("missing a.txt header");
    let old_a = plain.find("old a").expect("missing a.txt deletion");
    let new_a = plain.find("new a").expect("missing a.txt addition");
    let b_header = plain
        .find("├─ diff · b.txt")
        .expect("missing b.txt boundary");
    let old_b = plain.find("old b").expect("missing b.txt deletion");
    let new_b = plain.find("new b").expect("missing b.txt addition");
    assert!(
        a_header < old_a && old_a < new_a && new_a < b_header,
        "plain={plain}"
    );
    assert!(b_header < old_b && old_b < new_b, "plain={plain}");
}

#[test]
fn render_tool_message_shows_numbered_write_result_diff_after_input_compaction() {
    let msg = DisplayMessage {
        role: "tool".to_string(),
        content: "Created /tmp/head-to-head.html (2 lines):\n1+ <!doctype html>\n2+ <html lang=\"en\">\n..."
            .to_string(),
        tool_calls: Vec::new(),
        duration_secs: None,
        title: Some("/tmp/head-to-head.html".to_string()),
        tool_data: Some(crate::message::ToolCall {
            id: "call_write_compacted".to_string(),
            name: "write".to_string(),
            input: serde_json::json!({"file_path": "/tmp/head-to-head.html"}),
            intent: Some("Create an honest data-driven benchmark comparison page".to_string()),
            thought_signature: None,
        }),
    };

    let lines = render_tool_message(&msg, 100, crate::config::DiffDisplayMode::Inline);
    let plain = lines
        .iter()
        .map(extract_line_text)
        .collect::<Vec<_>>()
        .join("\n");

    assert!(
        lines[1].spans.iter().any(|span| span.content == "+2"),
        "plain={plain}"
    );
    assert!(plain.contains("╭─ diff"), "plain={plain}");
    assert!(plain.contains("<!doctype html>"), "plain={plain}");
    assert!(plain.contains("<html lang=\"en\">"), "plain={plain}");
}

#[test]
fn render_tool_message_never_draws_an_empty_edit_diff_frame() {
    for (name, content) in [
        ("write", "Created empty.txt (0 lines):\n"),
        ("edit", "Edited demo.txt: replaced 1 occurrence(s)"),
        (
            "multiedit",
            "Edited demo.txt\n\nTotal: 1 applied, 0 failed\n",
        ),
        ("patch", "Patch applied successfully"),
        ("apply_patch", "✓ demo.txt: modified (1 hunks)"),
    ] {
        let msg = DisplayMessage {
            role: "tool".to_string(),
            content: content.to_string(),
            tool_calls: Vec::new(),
            duration_secs: None,
            title: Some("demo.txt".to_string()),
            tool_data: Some(crate::message::ToolCall {
                id: format!("call_{name}_compacted"),
                name: name.to_string(),
                input: serde_json::json!({"file_path": "demo.txt"}),
                intent: None,
                thought_signature: None,
            }),
        };

        let lines = render_tool_message(&msg, 100, crate::config::DiffDisplayMode::Inline);
        let plain = lines
            .iter()
            .map(extract_line_text)
            .collect::<Vec<_>>()
            .join("\n");

        assert!(!plain.contains("╭─ diff"), "tool={name}, plain={plain}");
        assert!(!plain.contains("(+0 -0)"), "tool={name}, plain={plain}");
    }
}

#[test]
fn render_tool_message_marks_failed_apply_patch_without_empty_diff() {
    let msg = DisplayMessage {
        role: "tool".to_string(),
        content:
            "[apply_patch] ✗ /tmp/main.rs: Failed to find expected lines in /tmp/main.rs:\nfn missing() {}"
                .to_string(),
        tool_calls: Vec::new(),
        duration_secs: None,
        title: Some("/tmp/main.rs".to_string()),
        tool_data: Some(crate::message::ToolCall {
            id: "call_apply_patch_failed".to_string(),
            name: "apply_patch".to_string(),
            input: serde_json::json!({"file_path": "/tmp/main.rs"}),
            intent: Some("Replace the benchmark placeholder".to_string()),
            thought_signature: None,
        }),
    };

    let lines = render_tool_message(&msg, 100, crate::config::DiffDisplayMode::Inline);
    let plain = lines
        .iter()
        .map(extract_line_text)
        .collect::<Vec<_>>()
        .join("\n");

    assert!(
        plain.trim_start().starts_with("✗ apply_patch"),
        "plain={plain}"
    );
    assert!(!plain.contains("╭─ diff"), "plain={plain}");
    assert!(!plain.contains("(+0 -0)"), "plain={plain}");
}

#[test]
fn render_tool_message_inline_mode_truncates_large_diffs() {
    let old = (1..=7)
        .map(|i| format!("old line {i}\n"))
        .collect::<String>();
    let new = (1..=7)
        .map(|i| format!("new line {i} suffix_{i}_abcdefghijklmnopqrstuvwxyz0123456789\n"))
        .collect::<String>();
    let msg = DisplayMessage {
        role: "tool".to_string(),
        content: "Edited demo.txt".to_string(),
        tool_calls: Vec::new(),
        duration_secs: None,
        title: Some("demo.txt".to_string()),
        tool_data: Some(crate::message::ToolCall {
            id: "call_edit_inline_truncated".to_string(),
            name: "edit".to_string(),
            input: serde_json::json!({
                "file_path": "demo.txt",
                "old_string": old,
                "new_string": new,
            }),
            intent: None,
            thought_signature: None,
        }),
    };

    let lines = render_tool_message(&msg, 40, crate::config::DiffDisplayMode::Inline);
    let plain = lines
        .iter()
        .map(extract_line_text)
        .collect::<Vec<_>>()
        .join("\n");

    assert!(plain.contains("... 2 more changes ..."), "plain={plain}");
    assert!(plain.contains("old line 3"), "plain={plain}");
    assert!(!plain.contains("old line 7"), "plain={plain}");
    assert!(
        !plain.contains("new line 1 suffix_1_abcdefghijklmnopqrstuvwxyz0123456789"),
        "plain={plain}"
    );
    assert!(plain.contains("suffix_2_abcdefghijklm…"), "plain={plain}");
    assert_eq!(plain.matches("+7 -7").count(), 1, "plain={plain}");
    assert!(!extract_line_text(&lines[0]).contains("+7"));
    assert!(!plain.contains("total"));
    let footer = lines.last().unwrap();
    assert_eq!(extract_line_text(footer), "╰─");
    assert!(!plain.contains("hidden"), "plain={plain}");
    let header = &lines[1];
    assert!(extract_line_text(header).starts_with("╭─ diff · +7 -7 · "));
    assert!(
        header
            .spans
            .iter()
            .any(|span| span.content == "+7" && span.style.fg == Some(diff_add_color()))
    );
    assert!(
        header
            .spans
            .iter()
            .any(|span| span.content == "-7" && span.style.fg == Some(diff_del_color()))
    );
}

#[test]
fn render_tool_message_full_inline_mode_shows_full_diff() {
    let old = (1..=7)
        .map(|i| format!("old line {i}\n"))
        .collect::<String>();
    let new = (1..=7)
        .map(|i| format!("new line {i} suffix_{i}_abcdefghijklmnopqrstuvwxyz0123456789\n"))
        .collect::<String>();
    let msg = DisplayMessage {
        role: "tool".to_string(),
        content: "Edited demo.txt".to_string(),
        tool_calls: Vec::new(),
        duration_secs: None,
        title: Some("demo.txt".to_string()),
        tool_data: Some(crate::message::ToolCall {
            id: "call_edit_inline_full".to_string(),
            name: "edit".to_string(),
            input: serde_json::json!({
                "file_path": "demo.txt",
                "old_string": old,
                "new_string": new,
            }),
            intent: None,
            thought_signature: None,
        }),
    };

    let lines = render_tool_message(&msg, 40, crate::config::DiffDisplayMode::FullInline);
    let plain = lines
        .iter()
        .map(extract_line_text)
        .collect::<Vec<_>>()
        .join("\n");

    assert!(!plain.contains("more changes"), "plain={plain}");
    assert!(plain.contains("old line 4"), "plain={plain}");
    let source_text = lines
        .iter()
        .filter(|line| extract_line_text(line).starts_with("│ "))
        .flat_map(|line| {
            line.spans
                .iter()
                .skip(if line.spans[0].content.contains('↪') {
                    1
                } else {
                    2
                })
        })
        .map(|span| span.content.as_ref())
        .collect::<String>();
    assert!(
        source_text.contains("new line 4 suffix_4_abcdefghijklmnopqrstuvwxyz0123456789"),
        "plain={plain}"
    );
    assert!(lines.iter().all(|line| line.width() <= 39), "plain={plain}");
    assert!(!plain.contains('…'), "plain={plain}");
    assert_eq!(plain.matches("+7 -7").count(), 1, "plain={plain}");
    assert_eq!(extract_line_text(lines.last().unwrap()), "╰─");
    assert!(!extract_line_text(&lines[0]).contains("+7"));

    let hidden = render_tool_message(&msg, 100, crate::config::DiffDisplayMode::Off);
    let hidden_plain = hidden
        .iter()
        .map(extract_line_text)
        .collect::<Vec<_>>()
        .join("\n");
    assert_eq!(hidden_plain.matches("(+7 -7)").count(), 1);
    assert!(!hidden_plain.contains("╭─ diff"));
    assert!(
        hidden[0]
            .spans
            .iter()
            .any(|span| span.content == "+7" && span.style.fg == Some(diff_add_color()))
    );
}

#[test]
fn render_tool_message_memory_recall_centered_mode_left_aligns_with_padding() {
    let saved = crate::tui::markdown::center_code_blocks();
    crate::tui::markdown::set_center_code_blocks(true);
    let msg = DisplayMessage {
        role: "tool".to_string(),
        content: concat!(
            "- [fact] Centered mode should keep the recall card centered\n",
            "- [preference] The user likes visible side gutters"
        )
        .to_string(),
        tool_calls: Vec::new(),
        duration_secs: None,
        title: None,
        tool_data: Some(crate::message::ToolCall {
            id: "call_memory_recall_centered".to_string(),
            name: "memory".to_string(),
            input: serde_json::json!({
                "action": "recall",
                "query": "centered mode"
            }),
            intent: None,
            thought_signature: None,
        }),
    };

    let lines = render_tool_message(&msg, 120, crate::config::DiffDisplayMode::Off);
    let rendered: Vec<String> = lines
        .iter()
        .map(|line| {
            line.spans
                .iter()
                .map(|span| span.content.as_ref())
                .collect()
        })
        .collect();

    assert!(!rendered.is_empty(), "expected rendered recall card");
    assert!(
        rendered.iter().all(|line| line.starts_with("  ")),
        "centered recall card should include shared left padding: {rendered:?}"
    );
    assert_eq!(
        lines[0].alignment,
        Some(ratatui::layout::Alignment::Left),
        "centered recall card header should be left-aligned after padding"
    );
    assert!(
        rendered[0]
            .trim_start()
            .starts_with("🧠 recalled 2 memories"),
        "unexpected recall header: {rendered:?}"
    );

    crate::tui::markdown::set_center_code_blocks(saved);
}

#[test]
fn render_tool_message_memory_store_centered_mode_left_aligns_with_padding() {
    let saved = crate::tui::markdown::center_code_blocks();
    crate::tui::markdown::set_center_code_blocks(true);
    let msg = DisplayMessage {
        role: "tool".to_string(),
        content: "Saved memory".to_string(),
        tool_calls: Vec::new(),
        duration_secs: None,
        title: None,
        tool_data: Some(crate::message::ToolCall {
            id: "call_memory_store_centered".to_string(),
            name: "memory".to_string(),
            input: serde_json::json!({
                "action": "remember",
                "category": "fact",
                "content": "Centered mode should pad saved memory cards too"
            }),
            intent: None,
            thought_signature: None,
        }),
    };

    let lines = render_tool_message(&msg, 120, crate::config::DiffDisplayMode::Off);
    let rendered: Vec<String> = lines
        .iter()
        .map(|line| {
            line.spans
                .iter()
                .map(|span| span.content.as_ref())
                .collect()
        })
        .collect();

    assert!(!rendered.is_empty(), "expected rendered saved-memory card");
    assert!(
        rendered.iter().all(|line| line.starts_with("  ")),
        "centered saved-memory card should include shared left padding: {rendered:?}"
    );
    assert_eq!(
        lines[0].alignment,
        Some(ratatui::layout::Alignment::Left),
        "centered saved-memory card should be left-aligned after padding"
    );

    crate::tui::markdown::set_center_code_blocks(saved);
}

#[test]
fn render_tool_message_shows_swarm_spawn_prompt_summary() {
    let msg = DisplayMessage {
        role: "tool".to_string(),
        content: "spawned".to_string(),
        tool_calls: Vec::new(),
        duration_secs: None,
        title: None,
        tool_data: Some(crate::message::ToolCall {
            id: "call_swarm_spawn".to_string(),
            name: "swarm".to_string(),
            input: serde_json::json!({
                "action": "spawn",
                "prompt": "Extract the restart command cluster from cli commands and validate it"
            }),
            intent: None,
            thought_signature: None,
        }),
    };

    let lines = render_tool_message(&msg, 120, crate::config::DiffDisplayMode::Off);
    let rendered: String = lines[0]
        .spans
        .iter()
        .map(|span| span.content.as_ref())
        .collect();

    assert!(rendered.contains("swarm spawn"), "rendered={rendered}");
    assert!(
        rendered.contains("Extract the restart command cluster"),
        "rendered={rendered}"
    );
}

#[test]
fn render_tool_message_batch_subcall_shows_swarm_dm_details() {
    let msg = DisplayMessage {
        role: "tool".to_string(),
        content: "--- [1] swarm ---\nDone\n\nCompleted: 1 succeeded, 0 failed".to_string(),
        tool_calls: Vec::new(),
        duration_secs: None,
        title: None,
        tool_data: Some(crate::message::ToolCall {
            id: "call_batch_swarm".to_string(),
            name: "batch".to_string(),
            input: serde_json::json!({
                "tool_calls": [
                    {
                        "tool": "swarm",
                        "action": "dm",
                        "to_session": "shark",
                        "message": "Please validate the restart extraction and report back"
                    }
                ]
            }),
            intent: None,
            thought_signature: None,
        }),
    };

    let lines = render_tool_message(&msg, 120, crate::config::DiffDisplayMode::Off);
    let rendered = lines
        .iter()
        .map(extract_line_text)
        .collect::<Vec<_>>()
        .join("\n");

    assert!(rendered.contains("swarm dm → shark"), "rendered={rendered}");
    assert!(
        rendered.contains("Please validate the restart"),
        "rendered={rendered}"
    );
}

#[test]
fn render_agentgrep_output_body_borders_each_line() {
    let content = "crates/foo.rs\n  symbols: 1 matched\n    - fn bar @ 1-5";
    let lines = super::render_agentgrep_output_body(content, 120);
    let rendered = lines
        .iter()
        .map(extract_line_text)
        .collect::<Vec<_>>()
        .join("\n");

    assert!(rendered.contains("│ crates/foo.rs"), "rendered={rendered}");
    assert!(
        rendered.contains("│   symbols: 1 matched"),
        "rendered={rendered}"
    );
    assert!(
        rendered.contains("│     - fn bar @ 1-5"),
        "rendered={rendered}"
    );
    assert_eq!(lines.len(), 3, "one bordered line per source line");
}

#[test]
fn render_agentgrep_output_body_caps_huge_output() {
    let content = (0..1000)
        .map(|i| format!("line {i}"))
        .collect::<Vec<_>>()
        .join("\n");
    let lines = super::render_agentgrep_output_body(&content, 120);
    // 400-line cap plus a single truncation summary line.
    assert_eq!(lines.len(), 401, "should cap the body and add a summary");
    let last = extract_line_text(&lines[lines.len() - 1]);
    assert!(last.contains("more lines"), "last={last}");
}

#[test]
fn render_assistant_message_plan_card_wraps_instead_of_truncating() {
    let saved = crate::tui::markdown::center_code_blocks();
    crate::tui::markdown::set_center_code_blocks(false);
    // Long paragraph and long list items must wrap inside the card, not be
    // clipped at the right border by render_rounded_box's truncation.
    let plan_body = "# Long content plan\n\n\
        Goal\n\
        Produce an up-to-date ranked report grounded in current crate paths, then fix the highest-leverage low-risk offenders without destabilizing active work.\n\n\
        Approach\n\
        1. Write an audit document that regenerates metrics with current crate paths, ranks the top issues with evidence, and marks which items from the previous audit are complete versus stale.\n\
        2. Map the provider migration and record whether each module is a thin wrapper, partial duplicate, or full duplicate of the extracted crate.\n";
    let content = format!("Intro text.\n\n```plan\n{plan_body}```\n\nAfter the card.");
    let msg = DisplayMessage::assistant(&content);

    for width in [40u16, 60, 80, 100, 140] {
        let lines = render_assistant_message(&msg, width, crate::config::DiffDisplayMode::Off);
        let squashed = lines
            .iter()
            .map(extract_line_text)
            .collect::<Vec<_>>()
            .join(" ")
            .replace(['│', '╭', '╮', '╰', '╯', '─'], " ")
            .split_whitespace()
            .collect::<Vec<_>>()
            .join(" ");
        for phrase in [
            "without destabilizing active work.",
            "complete versus stale.",
            "or full duplicate of the extracted crate.",
        ] {
            assert!(
                squashed.contains(phrase),
                "width {width}: plan card lost trailing content {phrase:?}\n{squashed}"
            );
        }
        // Card borders stay intact.
        for line in lines
            .iter()
            .map(extract_line_text)
            .filter(|l| l.contains('│'))
        {
            assert!(
                line.trim_end().ends_with('│'),
                "width {width}: card row missing right border: {line:?}"
            );
        }
    }
    crate::tui::markdown::set_center_code_blocks(saved);
}

#[test]
fn render_swarm_message_preserves_inline_image_placeholder_lines() {
    let saved = crate::tui::markdown::center_code_blocks();
    crate::tui::markdown::set_center_code_blocks(false);

    // Simulate a rendered mermaid diagram inside a swarm message body: the
    // marker line plus its blank fill rows must survive rendering without a
    // rail prefix or blank-line cleanup so the image draws at full height.
    let placeholder = crate::tui::mermaid::inline_image_placeholder_lines(0xabcd1234, 4, 20);
    assert_eq!(placeholder.len(), 4);
    let marker_text = placeholder[0]
        .spans
        .iter()
        .map(|s| s.content.as_ref())
        .collect::<String>();

    let msg = DisplayMessage::swarm(
        "Plan graph · v3",
        "```mermaid\nflowchart TD\n    a --> b\n```",
    );
    // Rendering the real message goes through the markdown pipeline; whether a
    // real image materializes depends on protocol availability, so test the
    // line-preservation path directly through render_swarm_message with a body
    // the markdown renderer maps to placeholder lines is not deterministic in
    // tests. Instead assert the parser round-trips the marker we emit.
    let parsed = crate::tui::mermaid::parse_inline_image_placeholder(&placeholder[0]);
    assert_eq!(parsed, Some((0xabcd1234, 4, 20)));
    assert!(
        marker_text.starts_with('\u{0}'),
        "marker must keep its sentinel prefix"
    );

    // And the swarm renderer must not panic or drop content for a mermaid body.
    let lines = render_swarm_message(&msg, 100, crate::config::DiffDisplayMode::Off);
    assert!(!lines.is_empty());

    crate::tui::markdown::set_center_code_blocks(saved);
}

#[test]
fn render_empty_todo_tool_result_collapses_to_compact_line() {
    let msg = DisplayMessage {
        role: "tool".to_string(),
        content: "[todo] []".to_string(),
        tool_calls: Vec::new(),
        duration_secs: None,
        title: Some("0 todos".to_string()),
        tool_data: Some(crate::message::ToolCall {
            id: "call_todo_empty".to_string(),
            name: "todo".to_string(),
            input: serde_json::json!({}),
            intent: Some("Read the todo list".to_string()),
            thought_signature: None,
        }),
    };

    let plain = render_tool_message(&msg, 100, crate::config::DiffDisplayMode::Off)
        .iter()
        .map(extract_line_text)
        .collect::<Vec<_>>()
        .join("\n");

    assert!(!plain.contains("No tasks yet"), "{plain}");
    assert!(plain.contains("no tasks"), "{plain}");
}

/// The edit row must not repeat the file path that the inline diff header
/// directly below already shows. With an intent it reads like any other tool
/// row, and without one it falls back to the bare name plus change counts.
#[test]
fn render_tool_message_edit_row_does_not_duplicate_diff_header_path() {
    for intent in [Some("Fix the network hint"), None] {
        let msg = DisplayMessage {
            role: "tool".to_string(),
            content: "Edited".to_string(),
            tool_calls: Vec::new(),
            duration_secs: None,
            title: None,
            tool_data: Some(crate::message::ToolCall {
                id: "call_edit".to_string(),
                name: "edit".to_string(),
                input: serde_json::json!({
                    "file_path": "/repo/src/very_specific_name.rs",
                    "old_string": "old\n",
                    "new_string": "new\n",
                }),
                intent: intent.map(str::to_string),
                thought_signature: None,
            }),
        };

        let lines = render_tool_message(&msg, 160, crate::config::DiffDisplayMode::Inline);
        let text: Vec<String> = lines.iter().map(extract_line_text).collect();
        let occurrences = text
            .iter()
            .filter(|line| line.contains("very_specific_name.rs"))
            .count();
        assert_eq!(occurrences, 1, "path should appear once: {text:#?}");
        assert!(
            text[1].contains("diff · +1 -1 · /repo/src/very_specific_name.rs"),
            "{text:#?}"
        );
        if let Some(intent) = intent {
            assert!(text[0].contains(&format!("edit · {intent}")), "{text:#?}");
        }

        // With diffs hidden the row is the only place the path can appear.
        let lines = render_tool_message(&msg, 160, crate::config::DiffDisplayMode::Off);
        if intent.is_none() {
            assert!(extract_line_text(&lines[0]).contains("very_specific_name.rs"));
        }
    }
}
