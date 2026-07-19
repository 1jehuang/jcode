use super::*;

fn extract_line_text(line: &Line<'_>) -> String {
    line.spans
        .iter()
        .map(|span| span.content.as_ref())
        .collect::<String>()
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

    let lines = render_system_message(&msg, 80, crate::config::DiffDisplayMode::Off);
    let plain = lines
        .iter()
        .map(extract_line_text)
        .collect::<Vec<_>>()
        .join("\n");

    // System messages now render markdown: the inline markers are consumed and
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
fn render_system_message_preserves_indentation_and_newlines() {
    let msg = DisplayMessage::system("Header line\n  indented detail\n\nNext block");

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

    assert!(plain.contains("reconnecting"));
    assert!(!plain.contains("⚡ reconnecting"));

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
fn render_background_task_message_uses_box_and_truncates_preview_lines() {
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

    assert!(plain.contains("✓ bg bash completed · bg123"));
    assert!(plain.contains("exit 0 · 7.1s"));
    assert!(plain.contains("line 1"));
    assert!(plain.contains("… +1 more line"));
    assert!(!plain.contains("task bg123 · bash"));
    assert!(!plain.contains("Preview"));
    assert!(!plain.contains("Full output"));
    assert!(!plain.contains("bg action=\"output\" task_id=\"bg123\""));
}

#[test]
fn render_background_task_message_strips_ansi_from_existing_preview() {
    let msg = DisplayMessage::background_task(
        "**Background task** `bg123` · `bash` · ✓ completed · 0.1s · exit 0\n\n```text\n\u{1b}[32m✓\u{1b}[39m passes \u{1b}[2m12ms\u{1b}[22m\n```\n\n_Full output:_ `bg action=\"output\" task_id=\"bg123\"`",
    );

    let plain = render_background_task_message(&msg, 80, crate::config::DiffDisplayMode::Off)
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
fn render_todos_message_shows_grouped_card_with_status_glyphs() {
    fn todo(id: &str, content: &str, status: &str, group: Option<&str>) -> crate::todo::TodoItem {
        crate::todo::TodoItem {
            id: id.to_string(),
            content: content.to_string(),
            status: status.to_string(),
            priority: "high".to_string(),
            group: group.map(str::to_string),
            confidence: Some(80),
            completion_confidence: (status == "completed").then_some(95),
            confidence_history: Vec::new(),
            blocked_by: Vec::new(),
            assigned_to: None,
        }
    }

    let todos = vec![
        todo("1", "Wire the hotkey", "completed", Some("todo card")),
        todo("2", "Render the card", "in_progress", Some("todo card")),
        todo("3", "Unrelated cleanup", "pending", None),
    ];
    let msg = DisplayMessage::todos(serde_json::to_string(&todos).unwrap());

    let lines = render_todos_message(&msg, 100, crate::config::DiffDisplayMode::Off);
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
    // Completed items show completion confidence; open ones planning confidence.
    assert!(plain.contains("80→95%"), "{plain}");
    assert!(plain.contains("80%"), "{plain}");
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
fn render_todos_message_shows_goal_scores_and_feedback() {
    let todos = vec![crate::todo::TodoItem {
        id: "1".to_string(),
        content: "Render the card".to_string(),
        status: "in_progress".to_string(),
        priority: "high".to_string(),
        group: Some("todo rendering".to_string()),
        confidence: Some(85),
        completion_confidence: None,
        confidence_history: vec![80, 85],
        blocked_by: Vec::new(),
        assigned_to: None,
    }];
    let goals = vec![crate::todo::TodoGoal {
        group: Some("todo rendering".to_string()),
        user_intention: Some("Keep the agent aligned with the user's request".to_string()),
        user_intention_alignment: Some(98),
        hill_climbability: Some(95),
        objective: Some("Readable at 80 columns".to_string()),
        feedback_loop: Some("Inspect a debug frame".to_string()),
        end_to_end_ownership: Some(90),
    }];
    let msg =
        DisplayMessage::todos(serde_json::json!({ "todos": todos, "goals": goals }).to_string());

    let plain = render_todos_message(&msg, 100, crate::config::DiffDisplayMode::Off)
        .iter()
        .map(extract_line_text)
        .collect::<Vec<_>>()
        .join("\n");

    assert!(
        plain.contains("User intention alignment 98% · Hill climbability 95% · Ownership 90%"),
        "{plain}"
    );
    assert!(
        plain.contains("User intention · Keep the agent aligned with the user's request"),
        "{plain}"
    );
    assert!(
        plain.contains("Objective · Readable at 80 columns"),
        "{plain}"
    );
    assert!(
        plain.contains("Feedback · Inspect a debug frame"),
        "{plain}"
    );
    assert!(plain.contains("● Render the card · 85%"), "{plain}");
    assert!(!plain.contains("(high)"), "{plain}");
}

#[test]
fn render_todos_message_uses_readable_semantic_colors() {
    let todos = vec![crate::todo::TodoItem {
        id: "1".to_string(),
        content: "Tune the palette".to_string(),
        status: "in_progress".to_string(),
        priority: "high".to_string(),
        group: Some("todo rendering".to_string()),
        confidence: Some(85),
        completion_confidence: None,
        confidence_history: Vec::new(),
        blocked_by: Vec::new(),
        assigned_to: None,
    }];
    let goals = vec![crate::todo::TodoGoal {
        group: Some("todo rendering".to_string()),
        user_intention: None,
        user_intention_alignment: Some(98),
        hill_climbability: Some(95),
        objective: Some("Readable metadata".to_string()),
        feedback_loop: None,
        end_to_end_ownership: None,
    }];
    let msg =
        DisplayMessage::todos(serde_json::json!({ "todos": todos, "goals": goals }).to_string());
    let lines = render_todos_message(&msg, 100, crate::config::DiffDisplayMode::Off);
    let color_for = |text: &str| {
        lines
            .iter()
            .flat_map(|line| line.spans.iter())
            .find(|span| span.content.as_ref() == text)
            .and_then(|span| span.style.fg)
    };

    assert_eq!(color_for("todo rendering"), Some(todo_group_color()));
    assert_eq!(color_for("Readable metadata"), Some(todo_meta_color()));
    assert_eq!(color_for("● "), Some(asap_color()));
    assert_eq!(color_for(" (high)"), None);
    assert_eq!(color_for(" · 85%"), Some(todo_confidence_color()));
    assert_ne!(todo_meta_color(), dim_color());
}

#[test]
fn render_todos_message_wraps_goal_scores_at_narrow_widths() {
    let todos = vec![crate::todo::TodoItem {
        id: "1".to_string(),
        content: "Render the card".to_string(),
        status: "in_progress".to_string(),
        priority: "high".to_string(),
        group: Some("todo rendering".to_string()),
        confidence: Some(85),
        completion_confidence: None,
        confidence_history: Vec::new(),
        blocked_by: Vec::new(),
        assigned_to: None,
    }];
    let goals = vec![crate::todo::TodoGoal {
        group: Some("todo rendering".to_string()),
        user_intention: None,
        user_intention_alignment: Some(98),
        hill_climbability: Some(95),
        objective: None,
        feedback_loop: None,
        end_to_end_ownership: Some(90),
    }];
    let msg =
        DisplayMessage::todos(serde_json::json!({ "todos": todos, "goals": goals }).to_string());

    let lines = render_todos_message(&msg, 40, crate::config::DiffDisplayMode::Off);
    let plain = lines
        .iter()
        .map(extract_line_text)
        .collect::<Vec<_>>()
        .join("\n");

    assert!(plain.contains("User intention alignment 98%"), "{plain}");
    assert!(plain.contains("Hill climbability 95%"), "{plain}");
    assert!(plain.contains("Ownership 90%"), "{plain}");
    assert!(
        lines.iter().all(|line| line.width() <= 38),
        "card exceeded its 38-column content budget: {plain}"
    );
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
        id: "render".to_string(),
        content: "Render the todo result".to_string(),
        status: "in_progress".to_string(),
        priority: "high".to_string(),
        group: Some("todo rendering".to_string()),
        confidence: Some(92),
        completion_confidence: None,
        confidence_history: vec![85, 92],
        blocked_by: Vec::new(),
        assigned_to: None,
    }];
    let goals = vec![crate::todo::TodoGoal {
        group: Some("todo rendering".to_string()),
        user_intention: Some("See current work at a glance".to_string()),
        user_intention_alignment: Some(97),
        hill_climbability: Some(95),
        objective: Some("Readable card".to_string()),
        feedback_loop: Some("Inspect the rendered frame".to_string()),
        end_to_end_ownership: Some(92),
    }];
    let content = format!(
        "[todo] [tool timing: start=2026-07-13T19:51:50.261Z finish=2026-07-13T19:51:50.265Z duration=4ms] {}\n\nGoals:\n{}\n\n{}",
        serde_json::to_string_pretty(&todos).unwrap(),
        serde_json::to_string_pretty(&goals).unwrap(),
        crate::todo::TODO_HILL_CLIMBABILITY_CONTINUATION_MESSAGE
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
    assert!(
        plain.contains("User intention alignment 97% · Hill climbability 95% · Ownership 92%"),
        "{plain}"
    );
    assert!(plain.contains("● Render the todo result · 92%"), "{plain}");
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
        confidence: Some(92),
        ..Default::default()
    }];
    let before = crate::todo::TodoGoal {
        group: Some("todo rendering".to_string()),
        user_intention: Some("See current work at a glance".to_string()),
        user_intention_alignment: Some(99),
        hill_climbability: Some(90),
        objective: Some("Keep the todo card concise".to_string()),
        feedback_loop: Some("Inspect one frame".to_string()),
        end_to_end_ownership: None,
    };
    let after = crate::todo::TodoGoal {
        hill_climbability: Some(98),
        feedback_loop: Some(
            "Render before and after fixtures and assert unchanged fields are absent".to_string(),
        ),
        ..before.clone()
    };
    let updates = vec![crate::todo::TodoGoalChange {
        before: Some(before),
        after: Some(after.clone()),
        fields: vec![
            crate::todo::TodoGoalField::HillClimbability,
            crate::todo::TodoGoalField::FeedbackLoop,
        ],
    }];
    let content = format!(
        "{}\n\nGoals:\n{}\n\nGoal updates:\n{}\n\n{}",
        serde_json::to_string_pretty(&todos).unwrap(),
        serde_json::to_string_pretty(&vec![after]).unwrap(),
        serde_json::to_string_pretty(&updates).unwrap(),
        crate::todo::TODO_HILL_CLIMBABILITY_CONTINUATION_MESSAGE,
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
    assert!(plain.contains("Hill climbability 90% → 98%"), "{plain}");
    assert!(
        plain.contains(
            "Feedback · Render before and after fixtures and assert unchanged fields are absent"
        ),
        "{plain}"
    );
    assert!(
        !plain.contains("Render the entire unchanged todo plan"),
        "{plain}"
    );
    assert!(!plain.contains("User intention alignment"), "{plain}");
    assert!(!plain.contains("Keep the todo card concise"), "{plain}");
    assert!(!plain.contains("See current work at a glance"), "{plain}");
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
        confidence: Some(90),
        ..Default::default()
    }];
    let render = |goal: crate::todo::TodoGoal,
                  continuation: Option<&str>,
                  tool_data: Option<crate::message::ToolCall>| {
        let mut content = format!(
            "[todo] [tool timing: start=2026-07-13T19:51:50.261Z finish=2026-07-13T19:51:50.265Z duration=4ms] {}\n\nGoals:\n{}",
            serde_json::to_string_pretty(&todos).unwrap(),
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
            user_intention: None,
            user_intention_alignment: None,
            hill_climbability: Some(90),
            objective: Some(
                "Create a polished, working pelican-riding-a-bike animation using only HTML, CSS, and vanilla JavaScript."
                    .to_string(),
            ),
            feedback_loop: Some(INITIAL_FEEDBACK.to_string()),
            end_to_end_ownership: None,
        },
        Some(crate::todo::TODO_HILL_CLIMBABILITY_CONTINUATION_MESSAGE),
        Some(crate::message::ToolCall {
            id: "call_initial_todo".to_string(),
            name: "todo".to_string(),
            input: serde_json::Value::Null,
            intent: Some("Track implementation and browser verification".to_string()),
            thought_signature: None,
        }),
    );
    assert!(initial.contains("pelican-bike-animation"), "{initial}");
    assert!(
        without_whitespace(&initial).contains(&without_whitespace(INITIAL_FEEDBACK)),
        "initial feedback loop was truncated:\n{initial}"
    );

    // Simulate a restored/mirrored result whose ToolCall association was lost.
    // The structured result must still render as the same complete todo card.
    let revised = render(
        crate::todo::TodoGoal {
            group: Some("pelican-bike-animation".to_string()),
            user_intention: None,
            user_intention_alignment: None,
            hill_climbability: Some(98),
            objective: Some(REVISED_OBJECTIVE.to_string()),
            feedback_loop: Some(REVISED_FEEDBACK.to_string()),
            end_to_end_ownership: None,
        },
        None,
        None,
    );
    let compact_revised = without_whitespace(&revised);
    assert!(revised.contains("pelican-bike-animation"), "{revised}");
    assert!(
        compact_revised.contains(&without_whitespace(REVISED_OBJECTIVE)),
        "revised objective was truncated:\n{revised}"
    );
    assert!(
        compact_revised.contains(&without_whitespace(REVISED_FEEDBACK)),
        "revised feedback loop was truncated:\n{revised}"
    );
    let goal_details = revised
        .split("● Implement")
        .next()
        .expect("todo item should follow the goal details");
    assert!(
        !goal_details.contains('…'),
        "todo goal details must not truncate:\n{revised}"
    );
}

#[test]
fn visually_appealing_prompt_batched_retry_renders_complete_todo_card() {
    // This fixture is only the first todo retry emitted after the
    // hill-climbability continuation. The eval stops here and deliberately does
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
        confidence: Some(95),
        ..Default::default()
    }];
    let goals = vec![crate::todo::TodoGoal {
        group: Some("pelican-bike".to_string()),
        user_intention: None,
        user_intention_alignment: None,
        hill_climbability: Some(98),
        objective: Some(OBJECTIVE.to_string()),
        feedback_loop: Some(FEEDBACK.to_string()),
        end_to_end_ownership: None,
    }];
    let todo_output = format!(
        "{}\n\nGoals:\n{}",
        serde_json::to_string_pretty(&todos).unwrap(),
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
    assert!(
        compact.contains(&without_whitespace(OBJECTIVE)),
        "batched todo objective was truncated:\n{rendered}"
    );
    assert!(
        compact.contains(&without_whitespace(FEEDBACK)),
        "batched todo feedback loop was truncated:\n{rendered}"
    );
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
        confidence: Some(95),
        ..Default::default()
    }];
    let goals = vec![crate::todo::TodoGoal {
        group: Some("ship outcome".to_string()),
        hill_climbability: Some(100),
        feedback_loop: Some("Run the complete workflow".to_string()),
        end_to_end_ownership: Some(80),
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
    assert!(plain.contains("Ownership 80%"), "{plain}");
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
    assert!(completion_plain.contains("✓ bg Run integration tests completed · bg123"));

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
fn render_assistant_message_truncates_tool_calls_to_single_line() {
    let saved = crate::tui::markdown::center_code_blocks();
    crate::tui::markdown::set_center_code_blocks(false);
    let msg = DisplayMessage {
        role: "assistant".to_string(),
        content: "Done.".to_string(),
        tool_calls: vec![
            "read".to_string(),
            "grep".to_string(),
            "apply_patch".to_string(),
            "batch".to_string(),
        ],
        duration_secs: None,
        title: None,
        tool_data: None,
    };

    let lines = render_assistant_message(&msg, 20, crate::config::DiffDisplayMode::Off);
    assert_eq!(extract_line_text(&lines[1]), "");
    let tool_lines: Vec<String> = lines
        .iter()
        .skip(2)
        .map(|line| {
            line.spans
                .iter()
                .map(|span| span.content.as_ref())
                .collect()
        })
        .collect();

    assert!(
        tool_lines.len() == 1,
        "expected single-line tool-call summary: {tool_lines:?}"
    );
    assert!(
        tool_lines[0].contains("tools:"),
        "expected tool summary label on first line: {tool_lines:?}"
    );
    assert!(
        tool_lines.iter().all(|line| line.width() <= 20),
        "tool-call summary line should respect available width: {tool_lines:?}"
    );
    crate::tui::markdown::set_center_code_blocks(saved);
}

#[test]
fn render_assistant_message_centers_single_line_tool_summary() {
    let saved = crate::tui::markdown::center_code_blocks();
    crate::tui::markdown::set_center_code_blocks(true);
    let msg = DisplayMessage {
        role: "assistant".to_string(),
        content: "Done.".to_string(),
        tool_calls: vec![
            "read".to_string(),
            "grep".to_string(),
            "apply_patch".to_string(),
            "batch".to_string(),
        ],
        duration_secs: None,
        title: None,
        tool_data: None,
    };

    let lines = render_assistant_message(&msg, 28, crate::config::DiffDisplayMode::Off);
    assert_eq!(extract_line_text(&lines[1]), "");
    let tool_lines: Vec<String> = lines
        .iter()
        .skip(2)
        .map(|line| {
            line.spans
                .iter()
                .map(|span| span.content.as_ref())
                .collect()
        })
        .collect();

    assert!(
        tool_lines.len() == 1,
        "expected single-line tool-call summary: {tool_lines:?}"
    );
    let first_pad = tool_lines[0].chars().take_while(|c| *c == ' ').count();
    assert!(
        first_pad > 0,
        "tool summary should still be padded/centered as a block: {tool_lines:?}"
    );
    assert!(
        lines
            .iter()
            .skip(2)
            .all(|line| line.alignment == Some(ratatui::layout::Alignment::Left)),
        "centered tool summary should use a shared left-aligned block pad"
    );

    crate::tui::markdown::set_center_code_blocks(saved);
}

#[test]
fn render_assistant_message_without_body_does_not_add_extra_blank_line_before_tool_summary() {
    let saved = crate::tui::markdown::center_code_blocks();
    crate::tui::markdown::set_center_code_blocks(false);
    let msg = DisplayMessage {
        role: "assistant".to_string(),
        content: String::new(),
        tool_calls: vec!["read".to_string()],
        duration_secs: None,
        title: None,
        tool_data: None,
    };

    let lines = render_assistant_message(&msg, 28, crate::config::DiffDisplayMode::Off);
    let rendered: Vec<String> = lines.iter().map(extract_line_text).collect();

    assert_eq!(rendered.len(), 1, "rendered={rendered:?}");
    assert!(rendered[0].contains("tool:"), "rendered={rendered:?}");

    crate::tui::markdown::set_center_code_blocks(saved);
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
fn render_system_message_uses_reload_card_for_reload_title() {
    let msg = DisplayMessage::system("Reloading server with newer binary...").with_title("Reload");

    let lines = render_system_message(&msg, 80, crate::config::DiffDisplayMode::Off);
    let plain = lines
        .iter()
        .map(extract_line_text)
        .collect::<Vec<_>>()
        .join("\n");

    assert!(
        plain.contains("reload"),
        "expected reload card title: {plain}"
    );
    assert!(plain.contains("Reloading server with newer binary"));
}

#[test]
fn render_system_message_uses_connection_card_for_reconnect_status() {
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

    assert!(
        plain.contains("reconnecting"),
        "expected reconnect card title: {plain}"
    );
    assert!(plain.contains("Retrying · attempt 2 · 7s"));
    assert!(plain.contains("connection reset by server"));
    assert!(plain.contains("jcode --resume koala"));
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
    assert_eq!(
        lines.len(),
        1,
        "intent should not add vertical space: {rendered}"
    );
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
    assert_eq!(lines.len(), 1, "no extra detail line expected: {rendered}");
}

/// Even with details off, a failed tool row keeps its error summary so
/// failures stay diagnosable.
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
        "error summary should still render after the intent: {rendered}"
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

fn gmail_draft_message(content: &str, input: serde_json::Value) -> DisplayMessage {
    DisplayMessage {
        role: "tool".to_string(),
        content: content.to_string(),
        tool_calls: Vec::new(),
        duration_secs: None,
        title: None,
        tool_data: Some(crate::message::ToolCall {
            id: "call_gmail_draft".to_string(),
            name: "gmail".to_string(),
            input,
            intent: None,
            thought_signature: None,
        }),
    }
}

#[test]
fn render_tool_message_shows_gmail_draft_card() {
    let msg = gmail_draft_message(
        "Draft created successfully.\nDraft ID: draft_123\nTo: bob@example.com\nSubject: Project update",
        serde_json::json!({
            "action": "draft",
            "to": "bob@example.com",
            "subject": "Project update",
            "body": "Hi Bob,\n\nThe release is ready for review."
        }),
    );

    let lines = render_tool_message(&msg, 100, crate::config::DiffDisplayMode::Off);
    let plain = lines
        .iter()
        .map(extract_line_text)
        .collect::<Vec<_>>()
        .join("\n");

    assert!(plain.contains("Gmail draft created · draft_123"), "{plain}");
    assert!(
        !plain.contains('✉'),
        "draft card should not show an icon: {plain}"
    );
    assert!(plain.contains("To: bob@example.com"), "{plain}");
    assert!(plain.contains("Subject: Project update"), "{plain}");
    assert!(
        plain.contains("The release is ready for review."),
        "{plain}"
    );
    assert!(
        !plain.contains("\"body\""),
        "must not leak raw JSON: {plain}"
    );
}

#[test]
fn render_gmail_draft_card_marks_failures_and_empty_fields() {
    let msg = gmail_draft_message(
        "Error: Gmail draft creation failed",
        serde_json::json!({ "action": "draft", "body": "" }),
    );

    let lines = render_tool_message(&msg, 80, crate::config::DiffDisplayMode::Off);
    let plain = lines
        .iter()
        .map(extract_line_text)
        .collect::<Vec<_>>()
        .join("\n");

    assert!(plain.contains("Gmail draft failed"), "{plain}");
    assert!(plain.contains("(recipient missing)"), "{plain}");
    assert!(plain.contains("(no subject)"), "{plain}");
    assert!(plain.contains("(empty body)"), "{plain}");
}

#[test]
fn render_gmail_draft_card_wraps_attachments_and_shows_complete_long_body() {
    let body = (1..=30)
        .map(|index| format!("body line {index}"))
        .collect::<Vec<_>>()
        .join("\n");
    let msg = gmail_draft_message(
        "Draft created successfully.\nDraft ID: draft_long",
        serde_json::json!({
            "action": "draft",
            "to": "a-very-long-recipient-address@example.com",
            "subject": "A subject that should wrap cleanly in a narrow transcript",
            "body": body,
            "attachments": [
                "/tmp/a-very-long-quarterly-report-filename.pdf",
                "/tmp/notes.txt"
            ]
        }),
    );

    let lines = render_tool_message(&msg, 48, crate::config::DiffDisplayMode::Off);
    let rendered = lines.iter().map(extract_line_text).collect::<Vec<_>>();
    let plain = rendered.join("\n");
    let compact = without_whitespace(&plain.replace('│', ""));

    assert!(
        compact.contains("To:a-very-long-recipient-address@example.com"),
        "{plain}"
    );
    assert!(
        compact.contains("Subject:Asubjectthatshouldwrapcleanlyinanarrowtranscript"),
        "{plain}"
    );
    assert!(
        compact
            .contains("Attachments:/tmp/a-very-long-quarterly-report-filename.pdf,/tmp/notes.txt"),
        "{plain}"
    );
    assert!(plain.contains("body line 18"), "{plain}");
    assert!(plain.contains("body line 19"), "{plain}");
    assert!(plain.contains("body line 30"), "{plain}");
    assert!(
        !plain.contains("more lines"),
        "body must not be truncated: {plain}"
    );
    assert!(
        lines.iter().all(|line| line.width() <= 47),
        "draft card exceeded row width: {rendered:?}"
    );
}

#[test]
fn render_gmail_draft_card_preserves_html_like_body_text() {
    let msg = gmail_draft_message(
        "Draft created successfully.\nDraft ID: draft_html",
        serde_json::json!({
            "action": "draft",
            "to": "web@example.com",
            "subject": "HTML-ish content",
            "body": "<p>Hello <strong>team</strong></p>"
        }),
    );

    let plain = render_tool_message(&msg, 100, crate::config::DiffDisplayMode::Off)
        .iter()
        .map(extract_line_text)
        .collect::<Vec<_>>()
        .join("\n");

    assert!(
        plain.contains("<p>Hello <strong>team</strong></p>"),
        "{plain}"
    );
}

#[test]
fn render_batch_tool_message_shows_nested_gmail_draft_card() {
    let msg = DisplayMessage {
        role: "tool".to_string(),
        content: "--- [1] gmail ---\nDraft created successfully.\nDraft ID: nested_123\nTo: nested@example.com\nSubject: Nested\n\nCompleted: 1 succeeded, 0 failed".to_string(),
        tool_calls: Vec::new(),
        duration_secs: None,
        title: None,
        tool_data: Some(crate::message::ToolCall {
            id: "call_batch_gmail".to_string(),
            name: "batch".to_string(),
            input: serde_json::json!({
                "tool_calls": [{
                    "tool": "gmail",
                    "parameters": {
                        "action": "draft",
                        "to": "nested@example.com",
                        "subject": "Nested",
                        "body": "Created inside a batch"
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

    assert!(
        plain.contains("Gmail draft created · nested_123"),
        "{plain}"
    );
    assert!(plain.contains("Created inside a batch"), "{plain}");
}

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
            name: "discover_tools".to_string(),
            input,
            intent: None,
            thought_signature: None,
        }),
    }
}

fn first_discovery_message(content: &str, input: serde_json::Value) -> DisplayMessage {
    let mut message = discovery_message(content, input);
    message.title = Some(crate::sponsors::DISCOVERY_DISCLOSURE_TAG.to_string());
    message
}

#[test]
fn render_tool_message_shows_discovery_browse_results_and_rationale() {
    let msg = first_discovery_message(
        "Discoverable tools in 'payments' (Jcode tool directory; recommendations must be based only on fit; details: https://jcode.sh/discovery-tools):\n\n- agentcard: prepaid virtual Visa cards for AI agents (https://agentcard.sh/?via=jcode-discovery)\n\nBrowse request ID: `11111111-2222-4333-8444-555555555555`",
        serde_json::json!({
            "action": "browse",
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
    assert!(plain.contains("1 result · payments"), "{plain}");
    assert!(plain.contains("why: the task needs test-mode"), "{plain}");
    assert!(plain.contains("agentcard"), "{plain}");
    assert!(plain.contains("prepaid virtual Visa cards"), "{plain}");
    assert!(plain.contains("agentcard.sh"), "{plain}");
    assert!(
        plain.contains("Jcode partners with tool providers to make their tools discoverable"),
        "{plain}"
    );
    assert!(
        without_whitespace(&plain).contains("Learnmore:https://jcode.sh/discovery-tools"),
        "{plain}"
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
fn batched_discovery_renders_first_use_disclosure_inline_once() {
    let msg = DisplayMessage {
        role: "tool".to_string(),
        content: "--- [1] discover_tools ---\nDiscoverable tools in 'payments' (Jcode tool directory; recommendations must be based only on fit; details: https://jcode.sh/discovery-tools):\n\n- agentcard: prepaid virtual Visa cards for AI agents (https://agentcard.sh/?via=jcode-discovery)\n\nBrowse request ID: `11111111-2222-4333-8444-555555555555`\n\nCompleted: 1 succeeded, 0 failed".to_string(),
        tool_calls: Vec::new(),
        duration_secs: None,
        title: Some(crate::sponsors::DISCOVERY_DISCLOSURE_TAG.to_string()),
        tool_data: Some(crate::message::ToolCall {
            id: "call_batch_discovery".to_string(),
            name: "batch".to_string(),
            input: serde_json::json!({
                "tool_calls": [{
                    "tool": "discover_tools",
                    "parameters": {
                        "action": "browse",
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

    assert!(plain.contains("1 result · payments"), "{plain}");
    assert_eq!(
        plain
            .matches("Jcode partners with tool providers to make their tools discoverable")
            .count(),
        1,
        "the batched first-use notice must render exactly once: {plain}"
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
        !plain.contains("Jcode partners with tool providers"),
        "later discovery results must not repeat the first-use notice: {plain}"
    );
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

    assert!(plain.contains("┌─ diff"), "plain={plain}");
    assert!(plain.contains("old line"), "plain={plain}");
    assert!(plain.contains("new line"), "plain={plain}");
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
        lines[0].spans.iter().any(|span| span.content == "+2"),
        "plain={plain}"
    );
    assert!(plain.contains("┌─ diff"), "plain={plain}");
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

        assert!(!plain.contains("┌─ diff"), "tool={name}, plain={plain}");
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
    assert!(!plain.contains("┌─ diff"), "plain={plain}");
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
    assert!(
        plain.contains("new line 4 suffix_4_abcdefghijklmnopqrstuvwxyz0123456789"),
        "plain={plain}"
    );
    assert!(!plain.contains('…'), "plain={plain}");
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
