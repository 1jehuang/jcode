use super::*;
use crate::suggested_replies::{
    ScoredSuggestion, SuggestedRepliesContext, distribute_suggestions, score_locally_with_context,
};

fn render_panel_lines(view: &SuggestedRepliesView, width: u16, height: u16) -> Vec<String> {
    let backend = ratatui::backend::TestBackend::new(width, height);
    let mut terminal = ratatui::Terminal::new(backend).expect("test terminal");
    terminal
        .draw(|frame| {
            let area = ratatui::layout::Rect::new(0, 0, width, height);
            crate::tui::ui::inline_ui::draw_suggested_replies_panel(frame, view, area);
        })
        .expect("draw suggested replies panel");
    let buf = terminal.backend().buffer();
    let mut rows = Vec::new();
    for y in 0..height {
        let mut line = String::new();
        for x in 0..width {
            line.push_str(buf[(x, y)].symbol());
        }
        let trimmed = line.trim_end().to_string();
        if !trimmed.is_empty() {
            rows.push(trimmed);
        }
    }
    rows
}

#[test]
fn distribution_and_focus_state_transitions_work_cleanly() {
    let resp = distribute_suggestions(
        "verification",
        vec![
            ScoredSuggestion {
                text: "Run cargo test".to_string(),
                confidence: 0.82,
            },
            ScoredSuggestion {
                text: "Inspect git diff".to_string(),
                confidence: 0.61,
            },
        ],
        0.55,
        3,
    );
    assert_eq!(resp.default_action.as_deref(), Some("Run cargo test"));
    assert_eq!(resp.secondary_suggestions, vec!["Inspect git diff"]);
}

#[test]
fn app_tab_focuses_then_activates_default_action_and_esc_cancels_explorer() {
    let mut app = crate::tui::app::tests::create_test_app();
    app.suggested_replies.force_enable_in_tests = true;
    app.push_display_message(super::super::DisplayMessage::assistant(
        "Implemented the feature.",
    ));

    let resp = distribute_suggestions(
        "implementation",
        vec![
            ScoredSuggestion {
                text: "Run unit tests".to_string(),
                confidence: 0.85,
            },
            ScoredSuggestion {
                text: "Inspect git diff".to_string(),
                confidence: 0.60,
            },
        ],
        0.55,
        3,
    );
    app.apply_suggested_replies_response(resp.clone());
    assert!(app.suggested_replies_visible());
    assert_eq!(app.suggested_replies.focus, None);

    // First Tab focuses DefaultAction and populates app.input for editing.
    let tab1 = app.handle_suggested_replies_tab();
    assert_eq!(tab1, Some(None));
    assert_eq!(
        app.suggested_replies.focus,
        Some(SuggestedReplyFocus::DefaultAction)
    );
    assert_eq!(app.input, "Run unit tests");
    assert_eq!(app.cursor_pos, app.input.len());
    assert!(app.suggested_replies_visible());

    // Second Tab steps focus to Secondary(0) and updates app.input; Up arrow returns to DefaultAction.
    let tab2 = app.handle_suggested_replies_tab();
    assert_eq!(tab2, Some(None));
    assert_eq!(
        app.suggested_replies.focus,
        Some(SuggestedReplyFocus::Secondary(0))
    );
    assert_eq!(app.input, "Inspect git diff");
    assert!(app.suggested_replies_visible());
    assert!(app.handle_suggested_replies_arrow_key(
        crossterm::event::KeyCode::Up,
        crossterm::event::KeyModifiers::NONE
    ));
    assert_eq!(
        app.suggested_replies.focus,
        Some(SuggestedReplyFocus::DefaultAction)
    );
    assert_eq!(app.input, "Run unit tests");

    // Enter (take_focused_or_default_suggested_reply(false)) submits the focused item.
    assert_eq!(
        app.take_focused_or_default_suggested_reply(false),
        Some("Run unit tests".to_string())
    );
    assert!(!app.suggested_replies_visible());

    // Re-apply and test digit selection populating app.input for editing.
    app.input.clear();
    app.cursor_pos = 0;
    app.apply_suggested_replies_response(resp.clone());
    assert!(app.stage_suggested_reply_by_digit('1'));
    assert_eq!(app.input, "Inspect git diff");
    assert_eq!(
        app.suggested_replies.focus,
        Some(SuggestedReplyFocus::Secondary(0))
    );
    assert!(app.suggested_replies_visible());
    assert_eq!(
        app.take_focused_or_default_suggested_reply(false),
        Some("Inspect git diff".to_string())
    );
    assert!(!app.suggested_replies_visible());

    // Re-apply and test Ctrl+Enter (unfocused immediate DefaultAction activation).
    app.input.clear();
    app.cursor_pos = 0;
    app.apply_suggested_replies_response(resp.clone());
    assert_eq!(
        app.take_focused_or_default_suggested_reply(true),
        Some("Run unit tests".to_string())
    );
    assert!(!app.suggested_replies_visible());

    // Re-apply with simulated explorer deadline and test Esc cancellation.
    app.input.clear();
    app.cursor_pos = 0;
    app.apply_suggested_replies_response(resp);
    app.suggested_replies.explorer_deadline = Some(Instant::now() - Duration::from_millis(10));
    let (_, expired_prompt) = app.poll_suggested_replies_tick();
    if crate::config::config()
        .suggested_replies
        .enable_full_auto_explorer
    {
        assert_eq!(expired_prompt, Some("Run unit tests".to_string()));
    } else {
        assert_eq!(expired_prompt, None);
    }

    // Verify cancel_suggested_replies_interaction clears deadline and suggestions.
    app.suggested_replies.explorer_deadline = Some(Instant::now() + Duration::from_millis(500));
    assert!(app.cancel_suggested_replies_interaction(true));
    assert!(app.suggested_replies.explorer_deadline.is_none());
    assert!(app.suggested_replies.current.is_none());
}

#[test]
fn tui_renders_and_navigates_exact_suggestion_counts_for_1_3_and_20() {
    let mut app = crate::tui::app::tests::create_test_app();
    app.suggested_replies.force_enable_in_tests = true;
    app.push_display_message(super::super::DisplayMessage::user(
        "Проверь работу подсказок при 1, 3 и 20 элементах",
    ));
    app.push_display_message(super::super::DisplayMessage::assistant(
        "Готово! Выберите действие:\n1. Запустить тесты\n2. Проверить рендеринг",
    ));

    let ctx = SuggestedRepliesContext::from_transcript(
        "[user]: Проверь работу подсказок при 1, 3 и 20 элементах\n\n[assistant]: Готово! Выберите действие:\n1. Запустить тесты\n2. Проверить рендеринг",
    );

    for count in [1usize, 3usize, 20usize] {
        app.input.clear();
        app.cursor_pos = 0;
        app.cancel_suggested_replies_interaction(true);

        let resp = score_locally_with_context(
            &ctx,
            None,
            &["Постоянный вопрос пользователя".to_string()],
            None,
            0.55,
            count,
        );
        assert!(resp.default_action.is_some());
        assert_eq!(
            resp.secondary_suggestions.len(),
            count,
            "cascade must return {count} secondary suggestions"
        );

        app.apply_suggested_replies_response(resp);
        let mut view = crate::tui::TuiState::suggested_replies_view(&app).expect("view present");
        view.show_confidence = true;

        let expected_total_rows = 1 + count;
        let height = crate::tui::ui::inline_ui::inline_ui_height(&app) as usize;
        assert_eq!(
            height, expected_total_rows,
            "inline_ui_height must equal 1 (Default Action) + {count} secondary rows"
        );

        let rendered_rows = render_panel_lines(&view, 120, height as u16 + 2);
        assert_eq!(
            rendered_rows.len(),
            expected_total_rows,
            "TUI panel must render {expected_total_rows} rows for count={count}"
        );
        assert!(
            rendered_rows[0].contains('★') && rendered_rows[0].contains("%)"),
            "row 0 must be Default Action with confidence badge: {}",
            rendered_rows[0]
        );
        for i in 1..=count {
            let row = &rendered_rows[i];
            let expected_prefix = format!("{i}. ");
            assert!(
                row.contains(&expected_prefix),
                "row {i} must contain prefix '{expected_prefix}', got: {row}"
            );
            assert!(
                row.contains("%)"),
                "row {i} must display model confidence percentage when show_confidence=true, got: {row}"
            );
        }

        // When show_confidence = false, confidence percentages are omitted from all rows.
        view.show_confidence = false;
        let rows_no_conf = render_panel_lines(&view, 120, height as u16 + 2);
        assert_eq!(rows_no_conf.len(), expected_total_rows);
        for row in &rows_no_conf {
            assert!(
                !row.contains("%)"),
                "confidence badge must be hidden when show_confidence=false: {row}"
            );
        }

        // Verify Tab cycling visits all 1 + count items (DefaultAction + Secondary(0..count-1)).
        let targets = app.ordered_suggested_reply_targets();
        assert_eq!(targets.len(), expected_total_rows);
        for expected_target in targets {
            assert!(app.step_suggested_replies_focus(1));
            assert_eq!(app.suggested_replies.focus, Some(expected_target));
            assert!(!app.input.is_empty());
        }
    }
}

#[test]
fn apply_response_preserves_existing_focus_and_version_cache_restores_or_refreshes() {
    let mut app = crate::tui::app::tests::create_test_app();
    app.suggested_replies.force_enable_in_tests = true;
    app.push_display_message(super::super::DisplayMessage::assistant("Done."));

    let initial = distribute_suggestions(
        "implementation",
        vec![
            ScoredSuggestion {
                text: "Initial default".to_string(),
                confidence: 0.85,
            },
            ScoredSuggestion {
                text: "Initial secondary 1".to_string(),
                confidence: 0.65,
            },
        ],
        0.55,
        3,
    );
    app.apply_suggested_replies_response(initial.clone());
    app.step_suggested_replies_focus(1);
    assert_eq!(
        app.suggested_replies.focus,
        Some(SuggestedReplyFocus::DefaultAction)
    );

    // An updated response while the panel is already visible preserves focus instead of resetting to None.
    let updated = distribute_suggestions(
        "verification",
        vec![
            ScoredSuggestion {
                text: "Updated default".to_string(),
                confidence: 0.88,
            },
            ScoredSuggestion {
                text: "Updated secondary 1".to_string(),
                confidence: 0.62,
            },
        ],
        0.55,
        3,
    );
    app.apply_suggested_replies_response(updated.clone());
    assert_eq!(
        app.suggested_replies.focus,
        Some(SuggestedReplyFocus::DefaultAction)
    );

    // Re-triggering on the same display_messages_version restores the cached response.
    app.clear_suggested_replies_for_new_turn();
    app.input.clear();
    app.cursor_pos = 0;
    app.maybe_trigger_suggested_replies_at_turn_end();
    assert_eq!(
        app.suggested_replies
            .current
            .as_ref()
            .and_then(|r| r.default_action.as_deref()),
        Some("Updated default")
    );

    // /suggestions debug outputs diagnostic summary.
    assert!(app.handle_suggestions_command("/suggestions debug"));
    assert!(
        app.display_messages
            .last()
            .unwrap()
            .content
            .contains("Suggested Replies Diagnostics")
    );

    // Starting a new turn clears both `current` and `pending_rx`.
    let (_tx, rx) = mpsc::unbounded_channel();
    app.suggested_replies.pending_rx = Some(rx);
    app.clear_suggested_replies_for_new_turn();
    assert!(app.suggested_replies.current.is_none());
    assert!(app.suggested_replies.focus.is_none());
    assert!(app.suggested_replies.pending_rx.is_none());
}

#[test]
fn cyrillic_suggestion_uses_display_columns_instead_of_byte_truncation() {
    let ru = "Проверить отладку подсказок через команду /suggestions debug и запустить тесты";
    let display_cols = unicode_width::UnicodeWidthStr::width(ru);
    assert!(
        ru.len() > display_cols,
        "Cyrillic UTF-8 byte length exceeds column width"
    );
    let rendered = crate::tui::ui::inline_ui::truncate_to_display_width(ru, display_cols + 4);
    assert_eq!(rendered, ru);
    let clipped = crate::tui::ui::inline_ui::truncate_to_display_width(ru, 30);
    assert_eq!(unicode_width::UnicodeWidthStr::width(clipped.as_str()), 30);
    assert!(clipped.ends_with('…'));
}

#[test]
fn mouse_hit_regions_digit_6_to_9_staged_input_editing_and_six_lens_cache_invalidation() {
    let mut app = crate::tui::app::tests::create_test_app();
    app.suggested_replies.force_enable_in_tests = true;
    app.push_display_message(super::super::DisplayMessage::user(
        "Проверим клики мышью, цифры 6..9 и редактирование черновика",
    ));
    app.push_display_message(super::super::DisplayMessage::assistant("Готово."));

    let ctx = SuggestedRepliesContext::from_transcript(
        "[user]: Проверим клики мышью, цифры 6..9 и редактирование черновика\n\n[assistant]: Готово.",
    );
    let resp = score_locally_with_context(&ctx, None, &[], None, 0.55, 8);
    assert_eq!(resp.secondary_suggestions.len(), 8);
    app.apply_suggested_replies_response(resp.clone());

    let view = crate::tui::TuiState::suggested_replies_view(&app).expect("view present");
    let _rows = render_panel_lines(&view, 120, 12);

    // 1. Mouse hit-testing on rendered panel: row 0 = DefaultAction, row 7 = Secondary(6).
    assert_eq!(
        crate::tui::ui::inline_ui::suggested_reply_target_at(4, 0),
        Some(SuggestedReplyFocus::DefaultAction)
    );
    let hit_row_7 =
        crate::tui::ui::inline_ui::suggested_reply_target_at(4, 7).expect("hit region on row 7");
    assert_eq!(hit_row_7, SuggestedReplyFocus::Secondary(6));

    // First click stages Secondary(6) into app.input; second click on focused target consumes it.
    assert!(app.stage_suggested_reply_target(hit_row_7));
    assert_eq!(
        app.suggested_replies.focus,
        Some(SuggestedReplyFocus::Secondary(6))
    );
    assert_eq!(app.input, resp.secondary_suggestions[6]);
    let consumed = app
        .take_suggested_reply_target(hit_row_7)
        .expect("second click consumes focused target");
    assert_eq!(consumed, resp.secondary_suggestions[6]);
    assert!(!app.suggested_replies_visible());

    // 2. Digit '8' ('6'..='9' range) stages Secondary(7) into app.input, and editing detaches focus.
    app.input.clear();
    app.cursor_pos = 0;
    app.apply_suggested_replies_response(resp);
    assert!(app.stage_suggested_reply_by_digit('8'));
    assert_eq!(
        app.suggested_replies.focus,
        Some(SuggestedReplyFocus::Secondary(7))
    );
    let staged_text = app.input.clone();
    assert!(!staged_text.is_empty());
    assert!(app.suggested_replies_has_staged_input());

    // Pressing Backspace edits the draft in app.input and detaches suggestion focus/staged_input.
    assert!(super::super::input::handle_basic_key(
        &mut app,
        crossterm::event::KeyCode::Backspace
    ));
    assert!(app.suggested_replies.focus.is_none());
    assert!(app.suggested_replies.staged_input.is_none());
    assert!(!app.input.is_empty());
    assert!(app.input.len() < staged_text.len());

    // 3. record_six_lens_review_findings invalidates cache and promotes review finding.
    app.input.clear();
    app.cursor_pos = 0;
    app.record_six_lens_review_findings("Исправить проверку границ в парсере".to_string());
    let review_resp = app
        .suggested_replies
        .current
        .as_ref()
        .expect("review suggestions present");
    assert_eq!(review_resp.branch, "review");
    assert!(
        review_resp
            .default_action
            .as_deref()
            .unwrap()
            .contains("Исправить проверку границ в парсере")
    );

    // /suggestions refresh forces a fresh evaluation bypassing cached_response.
    assert!(app.handle_suggestions_command("/suggestions refresh"));
    assert!(app.suggested_replies.current.is_some());
}
