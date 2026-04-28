use super::animation::{FOCUS_PULSE_DURATION, VIEWPORT_ANIMATION_DURATION};
use super::single_session::*;
use super::*;

#[test]
fn quarter_size_preset_follows_quarter_screen_width_steps() {
    let monitor_width = Some(2000);

    assert_eq!(inferred_visible_column_count(500, monitor_width, 0.25), 1);
    assert_eq!(inferred_visible_column_count(1000, monitor_width, 0.25), 2);
    assert_eq!(inferred_visible_column_count(1500, monitor_width, 0.25), 3);
    assert_eq!(inferred_visible_column_count(2000, monitor_width, 0.25), 4);
}

#[test]
fn preferred_panel_size_limits_visible_column_count() {
    let monitor_width = Some(2000);

    assert_eq!(inferred_visible_column_count(2000, monitor_width, 0.25), 4);
    assert_eq!(inferred_visible_column_count(2000, monitor_width, 0.50), 2);
    assert_eq!(inferred_visible_column_count(2000, monitor_width, 0.75), 1);
    assert_eq!(inferred_visible_column_count(2000, monitor_width, 1.00), 1);

    assert_eq!(inferred_visible_column_count(500, monitor_width, 0.25), 1);
    assert_eq!(inferred_visible_column_count(500, monitor_width, 1.00), 1);
}

#[test]
fn visible_column_count_tolerates_window_manager_gaps() {
    let monitor_width = Some(2000);

    assert_eq!(inferred_visible_column_count(1940, monitor_width, 0.25), 4);
    assert_eq!(inferred_visible_column_count(970, monitor_width, 0.25), 2);
    assert_eq!(inferred_visible_column_count(1940, monitor_width, 0.50), 2);
}

#[test]
fn visible_column_count_is_clamped_and_safe_without_monitor() {
    assert_eq!(inferred_visible_column_count(1, Some(2000), 0.25), 1);
    assert_eq!(inferred_visible_column_count(3000, Some(2000), 0.25), 4);
    assert_eq!(inferred_visible_column_count(1000, Some(0), 0.25), 1);
    assert_eq!(inferred_visible_column_count(1000, None, 0.25), 1);
}

#[test]
fn viewport_animation_interpolates_to_new_layout_target() {
    let mut animation = AnimatedViewport::default();
    let now = Instant::now();
    let visible = VisibleColumnLayout {
        visible_columns: 2,
        first_visible_column: 0,
    };
    let start = WorkspaceRenderLayout {
        visible,
        column_width: 200.0,
        scroll_offset: 0.0,
        vertical_scroll_offset: 0.0,
    };
    let target = WorkspaceRenderLayout {
        visible: VisibleColumnLayout {
            visible_columns: 2,
            first_visible_column: 2,
        },
        column_width: 300.0,
        scroll_offset: 600.0,
        vertical_scroll_offset: 800.0,
    };

    let first_frame = animation.frame(start, now);
    assert_eq!(first_frame.column_width, 200.0);
    assert_eq!(first_frame.scroll_offset, 0.0);
    assert_eq!(first_frame.vertical_scroll_offset, 0.0);
    assert!(!animation.is_animating());

    let transition_start = animation.frame(target, now);
    assert_eq!(transition_start.column_width, 200.0);
    assert_eq!(transition_start.scroll_offset, 0.0);
    assert_eq!(transition_start.vertical_scroll_offset, 0.0);
    assert!(animation.is_animating());

    let middle = animation.frame(target, now + VIEWPORT_ANIMATION_DURATION / 2);
    assert!(middle.column_width > 200.0);
    assert!(middle.column_width < 300.0);
    assert!(middle.scroll_offset > 0.0);
    assert!(middle.scroll_offset < 600.0);
    assert!(middle.vertical_scroll_offset > 0.0);
    assert!(middle.vertical_scroll_offset < 800.0);

    let final_frame = animation.frame(target, now + VIEWPORT_ANIMATION_DURATION);
    assert_eq!(final_frame.column_width, 300.0);
    assert_eq!(final_frame.scroll_offset, 600.0);
    assert_eq!(final_frame.vertical_scroll_offset, 800.0);
    assert!(!animation.is_animating());
}

#[test]
fn focus_pulse_runs_when_focused_surface_changes() {
    let mut pulse = FocusPulse::default();
    let now = Instant::now();

    assert_eq!(pulse.frame(1, now), 0.0);
    assert!(!pulse.is_animating());

    let start = pulse.frame(2, now);
    assert!(start > 0.0);
    assert!(pulse.is_animating());

    let middle = pulse.frame(2, now + FOCUS_PULSE_DURATION / 2);
    assert!(middle > 0.0);
    assert!(middle < start);

    let end = pulse.frame(2, now + FOCUS_PULSE_DURATION);
    assert_eq!(end, 0.0);
    assert!(!pulse.is_animating());
}

#[test]
fn bitmap_text_normalization_sanitizes_panel_titles() {
    assert_eq!(
        normalize_bitmap_text("fox · coordinator"),
        "FOX COORDINATOR"
    );
    assert_eq!(normalize_bitmap_text("agent-12"), "AGENT-12");
    assert_eq!(bitmap_text_width("NAV", 2.0), 34.0);
}

#[test]
fn bitmap_text_wrapping_breaks_on_words() {
    assert_eq!(
        wrap_bitmap_text("ONE TWO THREE", 1.0, bitmap_char_advance(1.0) * 7.0),
        vec!["ONE TWO", "THREE"]
    );
}

#[test]
fn bitmap_text_wrapping_splits_long_words() {
    assert_eq!(
        wrap_bitmap_text("ABCDEFGHI", 1.0, bitmap_char_advance(1.0) * 4.0),
        vec!["ABCD", "EFGH", "I"]
    );
}

#[test]
fn single_session_typography_targets_jetbrains_mono_light_nerd() {
    assert_eq!(SINGLE_SESSION_FONT_FAMILY, "JetBrainsMono Nerd Font");
    assert_eq!(SINGLE_SESSION_FONT_WEIGHT, "Light");
    assert!(SINGLE_SESSION_FONT_FALLBACKS.contains(&"monospace"));
    assert!(SINGLE_SESSION_TITLE_FONT_SIZE >= 28.0);
    assert!(SINGLE_SESSION_BODY_FONT_SIZE >= 22.0);
    assert!(SINGLE_SESSION_CODE_FONT_SIZE >= 21.0);
    assert!(SINGLE_SESSION_TITLE_FONT_SIZE > SINGLE_SESSION_BODY_FONT_SIZE);
    assert!(SINGLE_SESSION_BODY_FONT_SIZE > SINGLE_SESSION_META_FONT_SIZE);
    assert!(SINGLE_SESSION_CODE_FONT_SIZE <= SINGLE_SESSION_BODY_FONT_SIZE);
    assert!(SINGLE_SESSION_BODY_LINE_HEIGHT > SINGLE_SESSION_CODE_LINE_HEIGHT);
    assert!(SINGLE_SESSION_CODE_LINE_HEIGHT > SINGLE_SESSION_META_LINE_HEIGHT);
}

#[test]
fn single_session_vertices_include_a_draft_caret() {
    let mut app = SingleSessionApp::new(None);
    let empty_vertices = build_single_session_vertices(&app, PhysicalSize::new(640, 480), 0.0);
    app.handle_key(KeyInput::Character("abc".to_string()));
    let mut typed_vertices = build_single_session_vertices(&app, PhysicalSize::new(640, 480), 0.0);
    push_single_session_caret(&mut typed_vertices, &app, PhysicalSize::new(640, 480), None);

    assert!(typed_vertices.len() >= empty_vertices.len());
    assert!(
        typed_vertices
            .iter()
            .any(|vertex| vertex.color == SINGLE_SESSION_CARET_COLOR)
    );
}

#[test]
fn single_session_ctrl_backspace_deletes_previous_word() {
    let mut app = SingleSessionApp::new(None);
    app.handle_key(KeyInput::Character("hello desktop world".to_string()));

    assert_eq!(
        app.handle_key(KeyInput::DeletePreviousWord),
        KeyOutcome::Redraw
    );
    assert_eq!(app.draft, "hello desktop ");
}

#[test]
fn single_session_supports_tui_like_word_movement_delete_and_undo() {
    let mut app = SingleSessionApp::new(None);
    app.handle_key(KeyInput::Character("hello desktop world".to_string()));

    assert_eq!(
        app.handle_key(KeyInput::MoveCursorWordLeft),
        KeyOutcome::Redraw
    );
    assert_eq!(app.draft_cursor, "hello desktop ".len());

    assert_eq!(
        app.handle_key(KeyInput::MoveCursorWordRight),
        KeyOutcome::Redraw
    );
    assert_eq!(app.draft_cursor, app.draft.len());

    app.handle_key(KeyInput::MoveCursorWordLeft);
    assert_eq!(app.handle_key(KeyInput::DeleteNextWord), KeyOutcome::Redraw);
    assert_eq!(app.draft, "hello desktop ");

    assert_eq!(app.handle_key(KeyInput::UndoInput), KeyOutcome::Redraw);
    assert_eq!(app.draft, "hello desktop world");
}

#[test]
fn single_session_cursor_editing_inserts_and_deletes_in_middle() {
    let mut app = SingleSessionApp::new(None);
    app.handle_key(KeyInput::Character("helo".to_string()));
    app.handle_key(KeyInput::MoveCursorLeft);
    app.handle_key(KeyInput::Character("l".to_string()));

    assert_eq!(app.draft, "hello");
    assert_eq!(app.draft_cursor, 4);

    app.handle_key(KeyInput::DeleteNextChar);
    assert_eq!(app.draft, "hell");
}

#[test]
fn single_session_composer_uses_next_prompt_number_and_status_footer() {
    let mut app = SingleSessionApp::new(None);
    assert_eq!(app.next_prompt_number(), 1);
    assert_eq!(app.composer_prompt(), "1› ");
    assert_eq!(app.composer_text(), "1› ");
    assert!(app.composer_status_line().contains("ready"));

    app.handle_key(KeyInput::Character("hello".to_string()));
    assert_eq!(app.composer_text(), "1› hello");
    assert_eq!(app.composer_cursor_line_byte_index(), (0, "1› hello".len()));
    assert_eq!(
        app.handle_key(KeyInput::SubmitDraft),
        KeyOutcome::StartFreshSession {
            message: "hello".to_string(),
            images: Vec::new()
        }
    );

    assert_eq!(app.next_prompt_number(), 2);
    assert_eq!(app.composer_text(), "2› ");
    assert!(app.composer_status_line().contains("Ctrl+C interrupt"));
}

#[test]
fn single_session_transcript_roles_render_without_stringly_labels() {
    let mut app = SingleSessionApp::new(None);
    app.messages.push(SingleSessionMessage::user("question"));
    app.messages.push(SingleSessionMessage::assistant("answer"));
    app.messages
        .push(SingleSessionMessage::tool("using tool bash"));
    app.messages
        .push(SingleSessionMessage::system("system note"));
    app.messages.push(SingleSessionMessage::meta("meta note"));

    let body = app.body_lines().join("\n");
    assert!(body.contains("1  question"));
    assert!(body.contains("answer"));
    assert!(body.contains("• using tool bash"));
    assert!(body.contains("  system note"));
    assert!(body.contains("  meta note"));
    assert!(!body.contains("user:"));
    assert!(!body.contains("assistant:"));
}

#[test]
fn single_session_assistant_markdown_is_prepared_for_desktop_rendering() {
    let mut app = SingleSessionApp::new(None);
    app.messages.push(SingleSessionMessage::assistant(
        "# Plan\n\n- first\n- second\n\nUse `cargo test`.\n\n```rust\nfn main() {}\n```",
    ));

    let body = app.body_lines().join("\n");
    assert!(body.contains("# Plan"));
    assert!(body.contains("• first"));
    assert!(body.contains("• second"));
    assert!(body.contains("Use `cargo test`."));
    assert!(body.contains("``` rust"));
    assert!(body.contains("    fn main() {}"));
    assert!(body.contains("```"));
}

#[test]
fn single_session_tool_events_create_transcript_cards() {
    let mut app = SingleSessionApp::new(None);
    app.apply_session_event(session_launch::DesktopSessionEvent::ToolStarted {
        name: "bash".to_string(),
    });
    app.apply_session_event(session_launch::DesktopSessionEvent::ToolFinished {
        name: "bash".to_string(),
        summary: "tests passed".to_string(),
        is_error: false,
    });

    let body = app.body_lines().join("\n");
    assert!(body.contains("• bash running"));
    assert!(body.contains("• bash done: tests passed"));
    assert_eq!(app.status.as_deref(), Some("tool bash done"));
}

#[test]
fn single_session_hotkey_help_toggles_discoverable_shortcuts() {
    let mut app = SingleSessionApp::new(None);
    app.messages.push(SingleSessionMessage::user("question"));

    assert_eq!(app.handle_key(KeyInput::HotkeyHelp), KeyOutcome::Redraw);
    assert!(app.show_help);
    let help = app.body_lines().join("\n");
    assert!(help.contains("desktop shortcuts"));
    assert!(help.contains("Ctrl+Shift+C copy latest assistant response"));
    assert!(help.contains("Alt+Up/Down jump between user prompts"));
    assert!(!help.contains("1  question"));

    assert_eq!(app.handle_key(KeyInput::Escape), KeyOutcome::Redraw);
    assert!(!app.show_help);
    assert!(app.body_lines().join("\n").contains("1  question"));
}

#[test]
fn single_session_model_cycle_updates_status_and_transcript() {
    let mut app = SingleSessionApp::new(None);

    assert_eq!(
        app.handle_key(KeyInput::CycleModel(1)),
        KeyOutcome::CycleModel(1)
    );
    app.apply_session_event(session_launch::DesktopSessionEvent::ModelChanged {
        model: "claude-opus-4-5".to_string(),
        provider_name: Some("Claude".to_string()),
        error: None,
    });

    assert_eq!(
        app.status.as_deref(),
        Some("model: Claude · claude-opus-4-5")
    );
    assert!(
        app.body_lines()
            .join("\n")
            .contains("model switched to Claude · claude-opus-4-5")
    );
}

#[test]
fn single_session_model_picker_loads_filters_and_selects_model() {
    let mut app = SingleSessionApp::new(None);

    assert_eq!(
        app.handle_key(KeyInput::OpenModelPicker),
        KeyOutcome::LoadModelCatalog
    );
    assert!(app.model_picker.open);
    assert!(app.model_picker.loading);
    assert!(app.body_lines().join("\n").contains("loading models"));

    app.apply_session_event(session_launch::DesktopSessionEvent::ModelCatalog {
        current_model: Some("claude-sonnet-4-5".to_string()),
        provider_name: Some("Claude".to_string()),
        models: vec![
            session_launch::DesktopModelChoice {
                model: "claude-sonnet-4-5".to_string(),
                provider: Some("claude".to_string()),
                detail: Some("active account".to_string()),
                available: true,
            },
            session_launch::DesktopModelChoice {
                model: "claude-opus-4-5".to_string(),
                provider: Some("claude".to_string()),
                detail: Some("premium".to_string()),
                available: true,
            },
        ],
    });

    let picker = app.body_lines().join("\n");
    assert!(picker.contains("desktop model/account picker"));
    assert!(picker.contains("current: Claude · claude-sonnet-4-5"));
    assert!(picker.contains("✓ claude-sonnet-4-5"));
    assert!(picker.contains("provider claude"));

    assert_eq!(
        app.handle_key(KeyInput::Character("opus".to_string())),
        KeyOutcome::Redraw
    );
    let filtered = app.body_lines().join("\n");
    assert!(filtered.contains("filter: opus"));
    assert!(filtered.contains("claude-opus-4-5"));

    assert_eq!(
        app.handle_key(KeyInput::SubmitDraft),
        KeyOutcome::SetModel("claude-opus-4-5".to_string())
    );
}

#[test]
fn single_session_model_picker_updates_current_model_after_switch() {
    let mut app = SingleSessionApp::new(None);
    app.handle_key(KeyInput::OpenModelPicker);

    app.apply_session_event(session_launch::DesktopSessionEvent::ModelChanged {
        model: "gpt-5.4".to_string(),
        provider_name: Some("OpenAI".to_string()),
        error: None,
    });

    assert_eq!(app.model_picker.current_model.as_deref(), Some("gpt-5.4"));
    assert_eq!(app.model_picker.provider_name.as_deref(), Some("OpenAI"));
    assert!(
        app.body_lines()
            .join("\n")
            .contains("current: OpenAI · gpt-5.4")
    );
    assert!(app.composer_status_line().contains("model OpenAI/gpt-5.4"));
}

#[test]
fn single_session_stdin_request_is_visible_in_transcript() {
    let mut app = SingleSessionApp::new(None);

    app.apply_session_event(session_launch::DesktopSessionEvent::StdinRequest {
        request_id: "stdin-1".to_string(),
        prompt: "Password:".to_string(),
        is_password: true,
        tool_call_id: "tool-1".to_string(),
    });

    assert_eq!(app.status.as_deref(), Some("interactive input requested"));
    let body = app.body_lines().join("\n");
    assert!(body.contains("interactive password input requested"));
    assert!(body.contains("prompt: Password:"));
    assert!(body.contains("request: stdin-1"));
    assert!(body.contains("tool: tool-1"));
}

#[test]
fn single_session_stdin_response_masks_password_and_sends_input() {
    let mut app = SingleSessionApp::new(None);
    app.apply_session_event(session_launch::DesktopSessionEvent::StdinRequest {
        request_id: "stdin-1".to_string(),
        prompt: "Password:".to_string(),
        is_password: true,
        tool_call_id: "tool-1".to_string(),
    });

    assert_eq!(
        app.handle_key(KeyInput::Character("s3 cr".to_string())),
        KeyOutcome::Redraw
    );
    app.paste_text("et");
    let body = app.body_lines().join("\n");
    assert!(body.contains("input: •••••••"));
    assert!(!body.contains("s3 cr"));

    assert_eq!(
        app.handle_key(KeyInput::SubmitDraft),
        KeyOutcome::SendStdinResponse {
            request_id: "stdin-1".to_string(),
            input: "s3 cret".to_string()
        }
    );
    assert!(app.stdin_response.is_none());
    assert_eq!(app.status.as_deref(), Some("sending interactive input"));
}

#[test]
fn single_session_attached_image_is_sent_with_next_prompt() {
    let mut app = SingleSessionApp::new(None);
    app.attach_image("image/png".to_string(), "abc123".to_string());

    assert!(app.composer_status_line().contains("1 image"));
    app.handle_key(KeyInput::Character("describe this".to_string()));

    assert_eq!(
        app.handle_key(KeyInput::SubmitDraft),
        KeyOutcome::StartFreshSession {
            message: "describe this".to_string(),
            images: vec![("image/png".to_string(), "abc123".to_string())]
        }
    );
    assert!(app.pending_images.is_empty());
}

#[test]
fn single_session_clear_attached_images_shortcut_clears_pending_images() {
    let mut app = SingleSessionApp::new(None);
    app.attach_image("image/png".to_string(), "abc123".to_string());

    assert_eq!(
        app.handle_key(KeyInput::ClearAttachedImages),
        KeyOutcome::Redraw
    );
    assert!(app.pending_images.is_empty());
    assert_eq!(app.status.as_deref(), Some("cleared image attachments"));
    assert_eq!(
        app.handle_key(KeyInput::ClearAttachedImages),
        KeyOutcome::None
    );
}

#[test]
fn clipboard_image_paste_is_disabled_while_answering_stdin() {
    let mut app = SingleSessionApp::new(None);
    assert!(app.accepts_clipboard_image_paste());

    app.apply_session_event(session_launch::DesktopSessionEvent::StdinRequest {
        request_id: "stdin-1".to_string(),
        prompt: "Password:".to_string(),
        is_password: true,
        tool_call_id: "tool-1".to_string(),
    });

    assert!(!app.accepts_clipboard_image_paste());
}

#[test]
fn single_session_ctrl_enter_queues_while_processing_then_dequeues() {
    let mut app = SingleSessionApp::new(None);
    app.is_processing = true;
    app.apply_session_event(session_launch::DesktopSessionEvent::TextDelta(
        "working".to_string(),
    ));
    app.handle_key(KeyInput::Character("next prompt".to_string()));

    assert_eq!(app.handle_key(KeyInput::QueueDraft), KeyOutcome::Redraw);
    assert!(app.composer_status_line().contains("1 queued"));
    assert!(app.draft.is_empty());

    app.apply_session_event(session_launch::DesktopSessionEvent::Done);
    assert_eq!(
        app.take_next_queued_draft(),
        Some(("next prompt".to_string(), Vec::new()))
    );
    assert!(app.is_processing);
}

#[test]
fn single_session_paste_text_preserves_spaces() {
    let mut app = SingleSessionApp::new(None);
    app.paste_text("hello  pasted");
    assert_eq!(app.draft, "hello  pasted");
}

#[test]
fn single_session_character_selection_extracts_visible_text() {
    let mut app = SingleSessionApp::new(None);
    app.messages.push(SingleSessionMessage::user("first"));
    app.messages
        .push(SingleSessionMessage::assistant("second\nthird"));
    let lines = app.body_lines();

    app.begin_selection(SelectionPoint { line: 2, column: 1 });
    app.update_selection(SelectionPoint { line: 3, column: 2 });

    assert_eq!(
        app.selected_text_from_lines(&lines),
        Some(format!("{}\n{}", &lines[2][1..], &lines[3][..2]))
    );
    assert_eq!(
        app.selection_segments(&lines),
        vec![
            SelectionLineSegment {
                line: 2,
                start_column: 1,
                end_column: lines[2].chars().count()
            },
            SelectionLineSegment {
                line: 3,
                start_column: 0,
                end_column: 2
            }
        ]
    );
}

#[test]
fn single_session_character_selection_handles_reverse_unicode_selection() {
    let mut app = SingleSessionApp::new(None);
    let lines = vec!["hello 🦀 world".to_string()];

    app.begin_selection(SelectionPoint { line: 0, column: 9 });
    app.update_selection(SelectionPoint { line: 0, column: 6 });

    assert_eq!(
        app.selected_text_from_lines(&lines),
        Some("🦀 w".to_string())
    );
}

#[test]
fn single_session_body_line_at_y_maps_transcript_region() {
    let size = PhysicalSize::new(800, 600);
    assert_eq!(
        single_session_body_line_at_y(size, PANEL_BODY_TOP_PADDING + 1.0),
        Some(0)
    );
    assert_eq!(single_session_body_line_at_y(size, 1.0), None);
}

#[test]
fn single_session_body_point_at_position_maps_x_to_character_column() {
    let size = PhysicalSize::new(800, 600);
    let lines = vec!["abcdef".to_string()];
    let y = PANEL_BODY_TOP_PADDING + 1.0;
    let char_width = single_session_body_char_width();

    assert_eq!(
        single_session_body_point_at_position(size, PANEL_TITLE_LEFT_PADDING - 4.0, y, &lines),
        Some(SelectionPoint { line: 0, column: 0 })
    );
    assert_eq!(
        single_session_body_point_at_position(
            size,
            PANEL_TITLE_LEFT_PADDING + char_width * 2.4,
            y,
            &lines
        ),
        Some(SelectionPoint { line: 0, column: 2 })
    );
    assert_eq!(
        single_session_body_point_at_position(
            size,
            PANEL_TITLE_LEFT_PADDING + char_width * 99.0,
            y,
            &lines
        ),
        Some(SelectionPoint { line: 0, column: 6 })
    );
}

#[test]
fn single_session_prompt_jump_moves_between_user_turns() {
    let mut app = SingleSessionApp::new(None);
    for index in 0..4 {
        app.messages
            .push(SingleSessionMessage::user(format!("question {index}")));
        app.messages
            .push(SingleSessionMessage::assistant(format!("answer {index}")));
    }

    assert_eq!(app.body_scroll_lines, 0);
    assert_eq!(app.handle_key(KeyInput::JumpPrompt(-1)), KeyOutcome::Redraw);
    assert!(app.body_scroll_lines > 0);
    let older_scroll = app.body_scroll_lines;

    assert_eq!(app.handle_key(KeyInput::JumpPrompt(1)), KeyOutcome::Redraw);
    assert!(app.body_scroll_lines < older_scroll || app.body_scroll_lines == 0);
}

#[test]
fn single_session_copy_latest_response_prefers_streaming_text() {
    let mut app = SingleSessionApp::new(None);
    app.messages
        .push(SingleSessionMessage::assistant("completed answer"));
    assert_eq!(
        app.handle_key(KeyInput::CopyLatestResponse),
        KeyOutcome::CopyLatestResponse("completed answer".to_string())
    );

    app.apply_session_event(session_launch::DesktopSessionEvent::TextDelta(
        "streaming answer".to_string(),
    ));
    assert_eq!(
        app.handle_key(KeyInput::CopyLatestResponse),
        KeyOutcome::CopyLatestResponse("streaming answer".to_string())
    );
}

#[test]
fn single_session_streaming_preserves_manual_scroll_but_submit_follows_bottom() {
    let mut app = SingleSessionApp::new(None);
    app.messages.push(SingleSessionMessage::user("older"));
    app.messages
        .push(SingleSessionMessage::assistant("older answer"));
    app.scroll_body_lines(12);

    app.apply_session_event(session_launch::DesktopSessionEvent::TextDelta(
        "new token".to_string(),
    ));
    assert_eq!(app.body_scroll_lines, 12);

    app.handle_key(KeyInput::Character("new prompt".to_string()));
    assert_eq!(
        app.handle_key(KeyInput::SubmitDraft),
        KeyOutcome::StartFreshSession {
            message: "new prompt".to_string(),
            images: Vec::new()
        }
    );
    assert_eq!(app.body_scroll_lines, 0);
}

#[test]
fn single_session_applies_live_server_events_to_visible_body() {
    let mut app = SingleSessionApp::new(None);
    app.handle_key(KeyInput::Character("hello".to_string()));
    assert_eq!(
        app.handle_key(KeyInput::SubmitDraft),
        KeyOutcome::StartFreshSession {
            message: "hello".to_string(),
            images: Vec::new()
        }
    );
    app.apply_session_event(session_launch::DesktopSessionEvent::SessionStarted {
        session_id: "session_desktop_live_123".to_string(),
    });
    app.apply_session_event(session_launch::DesktopSessionEvent::TextDelta(
        "hi".to_string(),
    ));

    let live_lines = app.body_lines().join("\n");
    assert!(live_lines.contains("1  hello"));
    assert!(live_lines.contains("hi"));
    assert!(!live_lines.contains("user:"));
    assert!(!live_lines.contains("assistant:"));
    assert!(!live_lines.contains("status:"));
    assert!(app.has_background_work());

    app.apply_session_event(session_launch::DesktopSessionEvent::Done);
    assert!(!app.has_background_work());
    let completed_lines = app.body_lines().join("\n");
    assert!(completed_lines.contains("1  hello"));
    assert!(completed_lines.contains("hi"));
    assert!(!completed_lines.contains("assistant:"));
}

#[test]
fn desktop_app_drains_session_events_into_visible_debug_snapshot() {
    let mut app = fresh_single_session_app();
    assert_eq!(
        app.handle_key(KeyInput::Character("hello smoke".to_string())),
        KeyOutcome::Redraw
    );
    assert_eq!(
        app.handle_key(KeyInput::SubmitDraft),
        KeyOutcome::StartFreshSession {
            message: "hello smoke".to_string(),
            images: Vec::new()
        }
    );

    let (event_tx, event_rx) = mpsc::channel();
    event_tx
        .send(session_launch::DesktopSessionEvent::SessionStarted {
            session_id: "session_visible_smoke".to_string(),
        })
        .unwrap();
    event_tx
        .send(session_launch::DesktopSessionEvent::TextDelta(
            "visible assistant response".to_string(),
        ))
        .unwrap();
    assert!(apply_pending_session_events(&mut app, &event_rx));

    let streaming = app.debug_snapshot();
    assert_eq!(streaming.mode, "single_session");
    assert_eq!(
        streaming.live_session_id.as_deref(),
        Some("session_visible_smoke")
    );
    assert!(streaming.is_processing);
    assert!(streaming.body_text.contains("1  hello smoke"));
    assert!(streaming.body_text.contains("visible assistant response"));
    assert!(!streaming.body_text.contains("user:"));
    assert!(!streaming.body_text.contains("assistant:"));
    assert!(!streaming.body_text.contains("status:"));

    event_tx
        .send(session_launch::DesktopSessionEvent::Done)
        .unwrap();
    assert!(apply_pending_session_events(&mut app, &event_rx));
    let completed = app.debug_snapshot();
    assert!(!completed.is_processing);
    assert_eq!(completed.status.as_deref(), Some("ready"));
    assert!(completed.body_text.contains("visible assistant response"));
    assert!(!completed.body_text.contains("assistant:"));
}

#[test]
fn headless_chat_smoke_message_parses_hidden_flag() {
    assert_eq!(
        headless_chat_smoke_message(&[
            "jcode-desktop".to_string(),
            "--headless-chat-smoke".to_string(),
            "reply pong".to_string(),
        ]),
        Some("reply pong".to_string())
    );
    assert_eq!(
        headless_chat_smoke_message(&[
            "jcode-desktop".to_string(),
            "--headless-chat-smoke=reply pong".to_string(),
        ]),
        Some("reply pong".to_string())
    );
    assert_eq!(
        headless_chat_smoke_message(&["jcode-desktop".to_string()]),
        None
    );
}

#[test]
fn single_session_reload_event_keeps_worker_state_processing() {
    let mut app = SingleSessionApp::new(None);
    app.apply_session_event(session_launch::DesktopSessionEvent::Reloading {
        new_socket: Some("/tmp/jcode-reload.sock".to_string()),
    });

    assert!(app.has_background_work());
    assert!(app.body_lines().join("\n").contains("server reloading"));
}

#[test]
fn single_session_scrollback_virtualizes_visible_body_lines() {
    let mut app = SingleSessionApp::new(None);
    for index in 0..32 {
        app.apply_session_event(session_launch::DesktopSessionEvent::TextReplace(format!(
            "message {index}"
        )));
        app.apply_session_event(session_launch::DesktopSessionEvent::Done);
    }
    let size = PhysicalSize::new(640, 480);

    let bottom = single_session_visible_body(&app, size).join("\n");
    assert!(bottom.contains("message 31"));
    assert!(!bottom.contains("message 0"));

    app.scroll_body_lines(24);
    let older = single_session_visible_body(&app, size).join("\n");
    assert!(older.contains("message 0") || older.contains("message 1"));
}

#[test]
fn mouse_scroll_delta_maps_to_body_scroll_lines() {
    assert_eq!(
        mouse_scroll_lines(MouseScrollDelta::LineDelta(0.0, 1.0)),
        Some(3)
    );
    assert_eq!(
        mouse_scroll_lines(MouseScrollDelta::LineDelta(0.0, -1.0)),
        Some(-3)
    );
}

#[test]
fn glyphon_caret_position_uses_shaped_draft_buffer() {
    let mut app = SingleSessionApp::new(None);
    app.handle_key(KeyInput::Character("hello".to_string()));
    let mut font_system = FontSystem::new();
    let buffers = single_session_text_buffers(&app, PhysicalSize::new(640, 480), &mut font_system);

    let caret = glyphon_draft_caret_position(&app, &buffers[2], PhysicalSize::new(640, 480))
        .expect("caret position should be available from glyphon layout runs");

    assert!(caret.x > PANEL_TITLE_LEFT_PADDING);
    assert!(caret.y >= single_session_draft_top(PhysicalSize::new(640, 480)));
}

#[test]
fn single_session_without_session_is_native_fresh_draft() {
    let mut app = SingleSessionApp::new(None);

    assert!(app.status_title().contains("single session"));
    assert_eq!(
        app.handle_key(KeyInput::SpawnPanel),
        KeyOutcome::SpawnSession
    );
    assert!(
        single_session_lines(None)
            .iter()
            .any(|line| line.contains("desktop-native"))
    );
}

#[test]
fn fresh_single_session_submit_requests_backend_session() {
    let mut app = SingleSessionApp::new(None);
    app.handle_key(KeyInput::Character("hello desktop".to_string()));

    assert_eq!(
        app.handle_key(KeyInput::SubmitDraft),
        KeyOutcome::StartFreshSession {
            message: "hello desktop".to_string(),
            images: Vec::new()
        }
    );
    assert!(app.draft.is_empty());
}

#[test]
fn default_single_session_app_starts_without_attaching_recent_session() {
    let DesktopApp::SingleSession(mut app) = fresh_single_session_app() else {
        panic!("default desktop app should be single-session mode");
    };

    assert!(app.session.is_none());
    assert_eq!(
        app.handle_key(KeyInput::SpawnPanel),
        KeyOutcome::SpawnSession
    );
}

#[test]
fn desktop_mode_defaults_to_single_session_and_gates_workspace_prototype() {
    assert_eq!(
        desktop_mode_from_args(["jcode-desktop"]),
        DesktopMode::SingleSession
    );
    assert_eq!(
        desktop_mode_from_args(["jcode-desktop", "--workspace"]),
        DesktopMode::WorkspacePrototype
    );
}

#[test]
fn single_session_spawn_resets_to_fresh_native_draft() {
    let card = workspace::SessionCard {
        session_id: "session_alpha".to_string(),
        title: "alpha".to_string(),
        subtitle: "active".to_string(),
        detail: "3 msgs".to_string(),
        preview_lines: Vec::new(),
        detail_lines: Vec::new(),
    };
    let mut app = SingleSessionApp::new(Some(card));
    app.handle_key(KeyInput::Character("draft".to_string()));

    app.reset_fresh_session();

    assert!(app.session.is_none());
    assert!(app.draft.is_empty());
    assert_eq!(app.detail_scroll, 0);
    assert!(app.status_title().contains("fresh session"));
}

#[test]
fn single_session_wraps_one_session_card() {
    let card = workspace::SessionCard {
        session_id: "session_alpha".to_string(),
        title: "alpha".to_string(),
        subtitle: "active".to_string(),
        detail: "3 msgs".to_string(),
        preview_lines: vec!["user hello".to_string()],
        detail_lines: vec!["assistant hi".to_string()],
    };
    let mut app = SingleSessionApp::new(Some(card));

    assert_eq!(app.handle_key(KeyInput::Enter), KeyOutcome::Redraw);
    assert_eq!(app.draft, "\n");
    app.handle_key(KeyInput::Character("draft".to_string()));
    assert_eq!(
        app.handle_key(KeyInput::SubmitDraft),
        KeyOutcome::SendDraft {
            session_id: "session_alpha".to_string(),
            title: "alpha".to_string(),
            message: "draft".to_string(),
            images: Vec::new(),
        }
    );
}

#[test]
fn single_session_surface_is_the_panel_primitive() {
    let card = workspace::SessionCard {
        session_id: "session_alpha".to_string(),
        title: "alpha".to_string(),
        subtitle: "active".to_string(),
        detail: "3 msgs".to_string(),
        preview_lines: Vec::new(),
        detail_lines: Vec::new(),
    };

    let surface = single_session_surface(Some(&card));

    assert_eq!(surface.id, 1);
    assert_eq!(surface.title, "alpha");
    assert_eq!(surface.session_id.as_deref(), Some("session_alpha"));
    assert_eq!((surface.lane, surface.column), (0, 0));
    assert!(
        surface
            .body_lines
            .contains(&"single session mode".to_string())
    );
}

#[test]
fn focused_panel_draft_only_shows_for_focused_insert_panel() {
    let mut workspace = Workspace::from_session_cards(vec![workspace::SessionCard {
        session_id: "a".to_string(),
        title: "alpha".to_string(),
        subtitle: "active".to_string(),
        detail: "1 msg".to_string(),
        preview_lines: Vec::new(),
        detail_lines: Vec::new(),
    }]);
    workspace.handle_key(KeyInput::Character("i".to_string()));
    workspace.handle_key(KeyInput::Character("draft text".to_string()));
    workspace.attach_image("image/png".to_string(), "abc123".to_string());

    assert_eq!(
        focused_panel_draft(&workspace, workspace.focused_id),
        Some("draft text · 1 image".to_string())
    );
    assert_eq!(
        focused_panel_draft(&workspace, workspace.focused_id + 1),
        None
    );
}
