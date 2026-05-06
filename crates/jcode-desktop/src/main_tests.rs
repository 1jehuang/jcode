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
fn workspace_status_text_includes_build_hash() {
    let mut workspace = Workspace::fake();

    assert_eq!(
        workspace_status_text(&workspace),
        format!("NAV P25 {}", desktop_build_hash_label())
    );

    workspace.mode = InputMode::Insert;
    assert_eq!(
        workspace_status_text(&workspace),
        format!("INS P25 {}", desktop_build_hash_label())
    );
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
    assert_eq!(SINGLE_SESSION_DEFAULT_FONT_SIZE, 22.0);
    assert_eq!(
        SINGLE_SESSION_TITLE_FONT_SIZE,
        SINGLE_SESSION_DEFAULT_FONT_SIZE
    );
    assert_eq!(
        SINGLE_SESSION_BODY_FONT_SIZE,
        SINGLE_SESSION_DEFAULT_FONT_SIZE + 3.0
    );
    assert_eq!(
        SINGLE_SESSION_META_FONT_SIZE,
        SINGLE_SESSION_DEFAULT_FONT_SIZE
    );
    assert_eq!(
        SINGLE_SESSION_CODE_FONT_SIZE,
        SINGLE_SESSION_DEFAULT_FONT_SIZE + 3.0
    );
    assert!(SINGLE_SESSION_BODY_LINE_HEIGHT > SINGLE_SESSION_CODE_LINE_HEIGHT);
    assert!(SINGLE_SESSION_CODE_LINE_HEIGHT > SINGLE_SESSION_META_LINE_HEIGHT);
}

#[test]
fn single_session_vertices_include_a_draft_caret() {
    let mut app = SingleSessionApp::new(None);
    let empty_vertices = build_single_session_vertices(&app, PhysicalSize::new(640, 480), 0.0, 0);
    app.handle_key(KeyInput::Character("abc".to_string()));
    let mut typed_vertices =
        build_single_session_vertices(&app, PhysicalSize::new(640, 480), 0.0, 0);
    push_single_session_caret(&mut typed_vertices, &app, PhysicalSize::new(640, 480), None);

    assert!(vertices_have_color(&empty_vertices, WELCOME_AURORA_BLUE));
    assert!(!vertices_have_color(&typed_vertices, WELCOME_AURORA_BLUE));
    assert!(
        typed_vertices
            .iter()
            .any(|vertex| vertex.color == SINGLE_SESSION_CARET_COLOR)
    );
}

#[test]
fn single_session_vertices_include_composer_line() {
    let app = SingleSessionApp::new(None);
    let vertices = build_single_session_vertices(&app, PhysicalSize::new(900, 700), 0.0, 0);

    assert!(vertices_have_color(&vertices, COMPOSER_LINE_COLOR));
    assert!(!vertices_have_color(
        &vertices,
        COMPOSER_CARD_BACKGROUND_COLOR
    ));
}

#[test]
fn fresh_single_session_uses_animated_welcome_ambient() {
    let mut app = SingleSessionApp::new(None);
    let size = PhysicalSize::new(900, 700);
    let tick_zero = build_single_session_vertices(&app, size, 0.0, 0);
    let tick_later = build_single_session_vertices(&app, size, 0.0, 18);

    assert!(vertices_have_color(&tick_zero, WELCOME_AURORA_BLUE));
    assert_ne!(
        positions_for_color(&tick_zero, WELCOME_AURORA_BLUE),
        positions_for_color(&tick_later, WELCOME_AURORA_BLUE)
    );

    app.handle_key(KeyInput::Character("hello".to_string()));
    let typed = build_single_session_vertices(&app, size, 0.0, 18);
    assert!(!vertices_have_color(&typed, WELCOME_AURORA_BLUE));
}

#[test]
fn single_session_active_work_uses_native_spinner_geometry() {
    let mut app = SingleSessionApp::new(None);
    let idle = build_single_session_vertices(&app, PhysicalSize::new(900, 700), 0.0, 0);
    assert!(!vertices_have_color(&idle, NATIVE_SPINNER_HEAD_COLOR));

    app.apply_session_event(session_launch::DesktopSessionEvent::TextDelta(
        "streaming".to_string(),
    ));
    let tick_zero = build_single_session_vertices(&app, PhysicalSize::new(900, 700), 0.0, 0);
    let tick_one = build_single_session_vertices(&app, PhysicalSize::new(900, 700), 0.0, 1);

    assert!(vertices_have_color(&tick_zero, NATIVE_SPINNER_HEAD_COLOR));
    assert!(vertices_have_color(&tick_one, NATIVE_SPINNER_HEAD_COLOR));
    assert_ne!(
        positions_for_color(&tick_zero, NATIVE_SPINNER_HEAD_COLOR),
        positions_for_color(&tick_one, NATIVE_SPINNER_HEAD_COLOR)
    );
}

#[test]
fn single_session_streaming_response_uses_line_reveal_shimmer() {
    let mut app = SingleSessionApp::new(None);
    let size = PhysicalSize::new(900, 700);
    assert!(single_session_streaming_shimmer(&app, size, 0).is_none());

    app.apply_session_event(session_launch::DesktopSessionEvent::TextDelta(
        "streaming answer".to_string(),
    ));
    let tick_zero = single_session_streaming_shimmer(&app, size, 0).expect("streaming shimmer");
    let tick_one = single_session_streaming_shimmer(&app, size, 8).expect("streaming shimmer");

    assert!(tick_zero.soft_rect.width > tick_zero.core_rect.width);
    assert_eq!(tick_zero.soft_rect.y, tick_zero.core_rect.y);
    assert_eq!(tick_zero.soft_rect.height, tick_zero.core_rect.height);
    assert!(tick_one.core_rect.x > tick_zero.core_rect.x);
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
    assert!(app.composer_status_line().contains("Ctrl+Enter queue/send"));
    assert!(!app.composer_status_line().contains("scrolled up"));

    app.scroll_body_lines(1);
    assert!(app.composer_status_line().contains("scrolled up 1 line"));
    app.scroll_body_lines(2);
    assert!(app.composer_status_line().contains("scrolled up 3 lines"));
    app.scroll_body_to_bottom();
    assert!(!app.composer_status_line().contains("scrolled up"));

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
    assert!(app.composer_status_line().contains("Esc interrupt"));
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
fn single_session_markdown_renderer_handles_rich_commonmark_shapes() {
    let mut app = SingleSessionApp::new(None);
    app.messages.push(SingleSessionMessage::assistant(
        "## Results\n\n> quote line\n> continues\n\n1. first\n2. second\n\n[docs](https://example.com) and **bold** plus _em_.\n\n| name | value |\n| --- | --- |\n| alpha | 42 |\n\n---",
    ));

    let lines = app.body_styled_lines();
    let body = lines
        .iter()
        .map(|line| line.text.as_str())
        .collect::<Vec<_>>()
        .join("\n");

    assert!(body.contains("## Results"));
    assert_eq!(
        style_for_text(&lines, "## Results"),
        Some(SingleSessionLineStyle::AssistantHeading)
    );
    assert!(body.contains("▌ quote line"));
    assert!(body.contains("▌ continues"));
    assert_eq!(
        style_for_text(&lines, "▌ quote line"),
        Some(SingleSessionLineStyle::AssistantQuote)
    );
    assert!(body.contains("1. first"));
    assert!(body.contains("2. second"));
    assert!(body.contains("docs ↗ https://example.com and **bold** plus _em_."));
    assert_eq!(
        style_for_text(&lines, "docs ↗ https://example.com and **bold** plus _em_."),
        Some(SingleSessionLineStyle::AssistantLink)
    );
    assert!(body.contains("┆ name │ value"));
    assert!(body.contains("┆ alpha │ 42"));
    assert_eq!(
        style_for_text(&lines, "┆ alpha │ 42"),
        Some(SingleSessionLineStyle::AssistantTable)
    );
    assert!(body.contains("───"));
}

#[test]
fn single_session_markdown_structure_uses_distinct_colors_and_cards() {
    let mut app = SingleSessionApp::new(None);
    app.messages.push(SingleSessionMessage::assistant(
        "# Heading\n\n> quoted\n\n| a | b |\n| - | - |\n| c | d |",
    ));
    let mut font_system = FontSystem::new();

    let buffers = single_session_text_buffers(&app, PhysicalSize::new(1200, 760), &mut font_system);
    let body = &buffers[1];
    assert_eq!(
        first_glyph_color_for_text(body, "# Heading"),
        Some(single_session_line_color(
            SingleSessionLineStyle::AssistantHeading
        ))
    );
    assert_eq!(
        first_glyph_color_for_text(body, "▌ quoted"),
        Some(single_session_line_color(
            SingleSessionLineStyle::AssistantQuote
        ))
    );
    assert_eq!(
        first_glyph_color_for_text(body, "┆ c │ d"),
        Some(single_session_line_color(
            SingleSessionLineStyle::AssistantTable
        ))
    );

    let vertices = build_single_session_vertices(&app, PhysicalSize::new(1200, 760), 0.0, 0);
    assert!(vertices_have_color(&vertices, QUOTE_CARD_BACKGROUND_COLOR));
    assert!(vertices_have_color(&vertices, TABLE_CARD_BACKGROUND_COLOR));
}

#[test]
fn single_session_header_only_uses_previous_message_title_for_static_preview() {
    let card = test_session_card("session_alpha", "previous user request", "active");
    let mut app = SingleSessionApp::new(Some(card));
    let size = PhysicalSize::new(1000, 720);

    assert!(app.should_show_session_title_header());
    assert_eq!(
        single_session_text_key(&app, size).title,
        "previous user request"
    );

    app.messages.push(SingleSessionMessage::user("live prompt"));
    app.messages
        .push(SingleSessionMessage::assistant("live answer"));

    assert!(!app.should_show_session_title_header());
    assert_eq!(single_session_text_key(&app, size).title, "");
}

#[test]
fn single_session_activity_indicator_appears_only_for_active_work() {
    let mut app = SingleSessionApp::new(None);
    assert!(!app.activity_indicator_active());
    assert!(!app.composer_status_line().starts_with("◴ "));

    app.apply_session_event(session_launch::DesktopSessionEvent::TextDelta(
        "streaming".to_string(),
    ));
    assert!(app.activity_indicator_active());
    assert!(app.composer_status_line().starts_with("receiving"));

    app.apply_session_event(session_launch::DesktopSessionEvent::Done);
    assert!(!app.activity_indicator_active());
    assert!(!app.composer_status_line().starts_with("◴ "));

    assert_eq!(
        app.handle_key(KeyInput::OpenModelPicker),
        KeyOutcome::LoadModelCatalog
    );
    assert!(app.activity_indicator_active());
}

#[test]
fn desktop_space_key_inserts_visible_prompt_space() {
    assert_eq!(
        to_key_input(&Key::Named(NamedKey::Space), ModifiersState::empty()),
        KeyInput::Character(" ".to_string())
    );

    let mut app = SingleSessionApp::new(None);
    assert_eq!(
        app.handle_key(KeyInput::Character("hello".to_string())),
        KeyOutcome::Redraw
    );
    assert_eq!(
        app.handle_key(KeyInput::Character(" ".to_string())),
        KeyOutcome::Redraw
    );
    assert_eq!(
        app.handle_key(KeyInput::Character("world".to_string())),
        KeyOutcome::Redraw
    );
    assert_eq!(app.composer_text(), "1› hello world");
    assert!(
        single_session_text_key(&app, PhysicalSize::new(420, 640))
            .draft
            .contains("hello world")
    );
}

#[test]
fn desktop_arrow_word_navigation_maps_common_modifiers() {
    assert_eq!(
        to_key_input(&Key::Named(NamedKey::ArrowLeft), ModifiersState::CONTROL),
        KeyInput::MoveCursorWordLeft
    );
    assert_eq!(
        to_key_input(&Key::Named(NamedKey::ArrowRight), ModifiersState::CONTROL),
        KeyInput::MoveCursorWordRight
    );
    assert_eq!(
        to_key_input(&Key::Named(NamedKey::ArrowLeft), ModifiersState::ALT),
        KeyInput::MoveCursorWordLeft
    );
    assert_eq!(
        to_key_input(&Key::Named(NamedKey::ArrowRight), ModifiersState::ALT),
        KeyInput::MoveCursorWordRight
    );
}

#[test]
fn desktop_maps_terminal_editing_shortcuts_from_tui() {
    assert_eq!(
        to_key_input(&Key::Character("b".into()), ModifiersState::CONTROL),
        KeyInput::MoveCursorWordLeft
    );
    assert_eq!(
        to_key_input(&Key::Character("f".into()), ModifiersState::CONTROL),
        KeyInput::MoveCursorWordRight
    );
    assert_eq!(
        to_key_input(&Key::Character("w".into()), ModifiersState::CONTROL),
        KeyInput::DeletePreviousWord
    );
    assert_eq!(
        to_key_input(&Key::Character("x".into()), ModifiersState::CONTROL),
        KeyInput::CutInputLine
    );
    assert_eq!(
        to_key_input(&Key::Named(NamedKey::Backspace), ModifiersState::ALT),
        KeyInput::DeletePreviousWord
    );
    assert_eq!(
        to_key_input(&Key::Named(NamedKey::ArrowUp), ModifiersState::CONTROL),
        KeyInput::RetrieveQueuedDraft
    );
    assert_eq!(
        to_key_input(&Key::Character("v".into()), ModifiersState::ALT),
        KeyInput::AttachClipboardImage
    );
    assert_eq!(
        to_key_input(&Key::Character("d".into()), ModifiersState::CONTROL),
        KeyInput::CancelGeneration
    );
}

#[test]
fn single_session_cut_and_retrieve_queued_draft_match_tui_shortcuts() {
    let mut app = SingleSessionApp::new(None);
    app.handle_key(KeyInput::Character("cut me".to_string()));
    assert_eq!(
        app.handle_key(KeyInput::CutInputLine),
        KeyOutcome::CutDraftToClipboard("cut me".to_string())
    );
    assert_eq!(app.composer_text(), "1› ");

    app.is_processing = true;
    app.handle_key(KeyInput::Character("queued".to_string()));
    assert_eq!(app.handle_key(KeyInput::QueueDraft), KeyOutcome::Redraw);
    assert_eq!(app.composer_text(), "1› ");
    assert_eq!(
        app.handle_key(KeyInput::RetrieveQueuedDraft),
        KeyOutcome::Redraw
    );
    assert_eq!(app.composer_text(), "1› queued");
}

#[test]
fn single_session_header_exposes_desktop_binary_and_version() {
    let mut app = SingleSessionApp::new(None);
    app.apply_session_event(session_launch::DesktopSessionEvent::SessionStarted {
        session_id: "session_header".to_string(),
    });
    let key = single_session_text_key(&app, PhysicalSize::new(900, 700));
    let build_version = option_env!("JCODE_DESKTOP_VERSION").unwrap_or(env!("CARGO_PKG_VERSION"));

    assert!(key.version.contains(build_version));
    assert!(
        key.version.contains("jcode-desktop") || key.version.contains("jcode_desktop"),
        "version label should include the running desktop binary path, got {:?}",
        key.version
    );
}

#[test]
fn fresh_single_session_startup_hides_top_left_chrome() {
    let app = SingleSessionApp::new(None);
    let key = single_session_text_key(&app, PhysicalSize::new(900, 700));

    assert_eq!(key.title, "");
    assert_eq!(key.version, "");
    assert_visual_text_contains(&key, "Hello there");
    assert!(key.welcome_hint.is_empty());
}

#[test]
fn single_session_text_buffers_include_header_version_area() {
    let mut app = SingleSessionApp::new(None);
    app.apply_session_event(session_launch::DesktopSessionEvent::SessionStarted {
        session_id: "session_header_buffers".to_string(),
    });
    let size = PhysicalSize::new(900, 700);
    let mut font_system = FontSystem::new();
    let buffers = single_session_text_buffers(&app, size, &mut font_system);

    assert_eq!(buffers.len(), 6);
    assert_eq!(single_session_text_areas(&buffers, size).len(), 5);
}

#[test]
fn fresh_welcome_hero_is_drawn_as_handwritten_strokes() {
    let size = PhysicalSize::new(1000, 720);
    let mut hidden = Vec::new();
    let mut half = Vec::new();
    let mut full = Vec::new();

    push_handwritten_welcome_hero(&mut hidden, size, 0.0);
    push_handwritten_welcome_hero(&mut half, size, 0.5);
    push_handwritten_welcome_hero(&mut full, size, 1.0);

    assert!(hidden.is_empty());
    assert!(half.len() > hidden.len());
    assert!(full.len() > half.len());
    assert!(vertices_have_color(&full, WELCOME_HANDWRITING_COLOR));
}

#[test]
fn fresh_welcome_hero_strokes_are_centered() {
    let size = PhysicalSize::new(1000, 720);
    let mut vertices = Vec::new();
    push_handwritten_welcome_hero(&mut vertices, size, 1.0);
    let points = vertices
        .iter()
        .filter(|vertex| vertex.color == WELCOME_HANDWRITING_COLOR)
        .map(|vertex| vertex.position[0])
        .collect::<Vec<_>>();
    let min_x = points.iter().copied().fold(f32::INFINITY, f32::min);
    let max_x = points.iter().copied().fold(f32::NEG_INFINITY, f32::max);

    assert!(((min_x + max_x) * 0.5).abs() < 0.03);
}

#[test]
fn single_session_status_text_stays_clean_while_native_spinner_animates() {
    let mut app = SingleSessionApp::new(None);
    app.apply_session_event(session_launch::DesktopSessionEvent::TextDelta(
        "streaming".to_string(),
    ));

    let first = single_session_text_key_for_tick(&app, PhysicalSize::new(900, 700), 0).status;
    let second = single_session_text_key_for_tick(&app, PhysicalSize::new(900, 700), 1).status;
    assert!(first.starts_with("receiving"));
    assert_eq!(first, second);
    assert!(!first.contains('◴'));
    assert!(!first.contains('◷'));
}

#[test]
fn single_session_visual_state_smoke_covers_markdown_spinner_and_switcher() {
    let size = PhysicalSize::new(1200, 760);
    let mut markdown_app = SingleSessionApp::new(Some(test_session_card(
        "session_visual",
        "stale title should hide",
        "active",
    )));
    markdown_app
        .messages
        .push(SingleSessionMessage::user("render this"));
    markdown_app.messages.push(SingleSessionMessage::assistant(
        "# Heading\n\n> quoted\n\n[docs](https://example.com)\n\n| k | v |\n| - | - |\n| color | yes |",
    ));
    markdown_app.apply_session_event(session_launch::DesktopSessionEvent::TextDelta(
        "streaming tail".to_string(),
    ));

    let markdown_key = single_session_text_key(&markdown_app, size);
    assert_eq!(markdown_key.title, "");
    assert!(markdown_key.status.starts_with("receiving"));
    assert_visual_text_contains(&markdown_key, "# Heading");
    assert_visual_text_contains(&markdown_key, "▌ quoted");
    assert_visual_text_contains(&markdown_key, "docs ↗ https://example.com");
    assert_visual_text_contains(&markdown_key, "┆ color │ yes");
    assert_visual_text_contains(&markdown_key, "streaming tail");

    let markdown_vertices = build_single_session_vertices(&markdown_app, size, 0.0, 0);
    assert!(vertices_have_color(
        &markdown_vertices,
        QUOTE_CARD_BACKGROUND_COLOR
    ));
    assert!(vertices_have_color(
        &markdown_vertices,
        TABLE_CARD_BACKGROUND_COLOR
    ));

    let mut switcher_app = SingleSessionApp::new(None);
    assert_eq!(
        switcher_app.handle_key(KeyInput::OpenSessionSwitcher),
        KeyOutcome::LoadSessionSwitcher
    );
    let switcher_key = single_session_text_key(&switcher_app, size);
    assert_eq!(switcher_key.title, "");
    assert!(switcher_key.status.starts_with("loading recent sessions"));
    assert_visual_text_contains(&switcher_key, "desktop session switcher");
    assert_visual_text_contains(
        &switcher_key,
        "loading recent sessions from ~/.jcode/sessions...",
    );
}

#[test]
fn single_session_body_styled_lines_follow_roles_and_overlays() {
    let mut app = SingleSessionApp::new(None);
    app.messages
        .push(SingleSessionMessage::user("question\nmore context"));
    app.messages.push(SingleSessionMessage::assistant(
        "answer\n\n```rust\nfn main() {}\n```",
    ));
    app.messages.push(SingleSessionMessage::tool("bash done"));
    app.messages
        .push(SingleSessionMessage::meta("model switched"));

    let lines = app.body_styled_lines();
    let segments = single_session_styled_text_segments(&lines);
    assert!(segments.contains(&("1".to_string(), user_prompt_number_color(1))));
    assert!(segments.contains(&("› ".to_string(), text_color(USER_PROMPT_ACCENT_COLOR))));
    assert!(segments.contains(&(
        "question".to_string(),
        single_session_line_color(SingleSessionLineStyle::User)
    )));

    assert_eq!(
        style_for_text(&lines, "1  question"),
        Some(SingleSessionLineStyle::User)
    );
    assert_eq!(
        style_for_text(&lines, "   more context"),
        Some(SingleSessionLineStyle::UserContinuation)
    );
    assert_eq!(
        style_for_text(&lines, "answer"),
        Some(SingleSessionLineStyle::Assistant)
    );
    assert_eq!(
        style_for_text(&lines, "``` rust"),
        Some(SingleSessionLineStyle::Code)
    );
    assert_eq!(
        style_for_text(&lines, "    fn main() {}"),
        Some(SingleSessionLineStyle::Code)
    );
    assert_eq!(
        style_for_text(&lines, "• bash done"),
        Some(SingleSessionLineStyle::Tool)
    );
    assert_eq!(
        style_for_text(&lines, "  model switched"),
        Some(SingleSessionLineStyle::Meta)
    );

    app.handle_key(KeyInput::HotkeyHelp);
    let help = app.body_styled_lines();
    assert_eq!(
        style_for_text(&help, "desktop shortcuts"),
        Some(SingleSessionLineStyle::OverlayTitle)
    );
    assert_eq!(
        style_for_text(
            &help,
            "  Ctrl+V      paste clipboard image when no text is present"
        ),
        Some(SingleSessionLineStyle::Overlay)
    );
}

#[test]
fn glyphon_body_buffer_uses_line_style_colors() {
    let mut app = SingleSessionApp::new(None);
    app.messages.push(SingleSessionMessage::user("question"));
    app.messages.push(SingleSessionMessage::assistant(
        "answer\n\n```rust\nfn main() {}\n```",
    ));
    app.messages.push(SingleSessionMessage::tool("bash done"));
    app.messages
        .push(SingleSessionMessage::meta("model switched"));
    let mut font_system = FontSystem::new();

    let buffers = single_session_text_buffers(&app, PhysicalSize::new(1200, 760), &mut font_system);
    let body = &buffers[1];

    assert_eq!(
        first_glyph_color_for_text(body, "answer"),
        Some(single_session_line_color(SingleSessionLineStyle::Assistant))
    );
    assert_eq!(
        first_glyph_color_for_text(body, "``` rust"),
        Some(single_session_line_color(SingleSessionLineStyle::Code))
    );
    assert_eq!(
        first_glyph_color_for_text(body, "• bash done"),
        Some(single_session_line_color(SingleSessionLineStyle::Tool))
    );
    assert_eq!(
        first_glyph_color_for_text(body, "  model switched"),
        Some(single_session_line_color(SingleSessionLineStyle::Meta))
    );
}

#[test]
fn single_session_transcript_card_runs_group_card_styles() {
    let mut app = SingleSessionApp::new(None);
    app.messages.push(SingleSessionMessage::assistant(
        "answer\n\n```rust\nfn main() {}\n```",
    ));
    app.messages.push(SingleSessionMessage::tool("bash done"));
    app.error = Some("boom".to_string());

    let lines = app.body_styled_lines();
    let runs = single_session_transcript_card_runs(&lines);

    let code = runs
        .iter()
        .find(|run| run.style == SingleSessionLineStyle::Code)
        .expect("code block should have a card run");
    assert_eq!(code.line_count, 3);
    assert_eq!(lines[code.line].text, "``` rust");

    let tool = runs
        .iter()
        .find(|run| run.style == SingleSessionLineStyle::Tool)
        .expect("tool line should have a card run");
    assert_eq!(tool.line_count, 1);
    assert_eq!(lines[tool.line].text, "• bash done");

    let error = runs
        .iter()
        .find(|run| run.style == SingleSessionLineStyle::Error)
        .expect("error line should have a card run");
    assert_eq!(error.line_count, 1);
    assert_eq!(lines[error.line].text, "error: boom");
}

#[test]
fn single_session_vertices_include_transcript_card_backgrounds() {
    let mut app = SingleSessionApp::new(None);
    app.messages.push(SingleSessionMessage::assistant(
        "answer\n\n```rust\nfn main() {}\n```",
    ));
    app.messages.push(SingleSessionMessage::tool("bash done"));
    app.error = Some("boom".to_string());

    let vertices = build_single_session_vertices(&app, PhysicalSize::new(1000, 720), 0.0, 0);

    assert!(vertices_have_color(&vertices, CODE_BLOCK_BACKGROUND_COLOR));
    assert!(vertices_have_color(&vertices, TOOL_CARD_BACKGROUND_COLOR));
    assert!(vertices_have_color(&vertices, ERROR_CARD_BACKGROUND_COLOR));
}

fn vertices_have_color(vertices: &[Vertex], color: [f32; 4]) -> bool {
    vertices.iter().any(|vertex| vertex.color == color)
}

fn positions_for_color(vertices: &[Vertex], color: [f32; 4]) -> Vec<[u32; 2]> {
    vertices
        .iter()
        .filter(|vertex| vertex.color == color)
        .map(|vertex| vertex.position.map(f32::to_bits))
        .collect()
}

fn assert_visual_text_contains(key: &SingleSessionTextKey, expected: &str) {
    let body_lines = key
        .body
        .iter()
        .map(|line| line.text.as_str())
        .chain(std::iter::once(key.welcome_hero.as_str()))
        .chain(key.welcome_hint.iter().map(|line| line.text.as_str()))
        .collect::<Vec<_>>();
    let body = body_lines.join("\n");
    assert!(
        body.contains(expected),
        "expected visual body to contain {expected:?}, got:\n{body}"
    );
}

fn test_session_card(id: &str, title: &str, status: &str) -> workspace::SessionCard {
    workspace::SessionCard {
        session_id: id.to_string(),
        title: title.to_string(),
        subtitle: format!("{status} · test-model"),
        detail: format!("2 msgs · {title}-workspace"),
        preview_lines: vec![format!("user {title} prompt")],
        detail_lines: vec![format!("assistant {title} response")],
    }
}

fn style_for_text(lines: &[SingleSessionStyledLine], text: &str) -> Option<SingleSessionLineStyle> {
    lines
        .iter()
        .find(|line| line.text == text)
        .map(|line| line.style)
}

fn first_glyph_color_for_text(buffer: &Buffer, text: &str) -> Option<TextColor> {
    buffer
        .layout_runs()
        .find(|run| run.text == text)
        .and_then(|run| run.glyphs.first().and_then(|glyph| glyph.color_opt))
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
    let help = app.body_lines();
    assert!(help.iter().any(|line| line == "desktop shortcuts"));
    assert!(help_has_shortcut(
        &help,
        "Ctrl+Enter",
        "queue while running, send when idle"
    ));
    assert!(help_has_shortcut(
        &help,
        "Ctrl+Shift+C",
        "copy latest assistant response"
    ));
    assert!(help_has_shortcut(
        &help,
        "Ctrl+P/O",
        "open recent session switcher"
    ));
    assert!(help_has_shortcut(
        &help,
        "Alt+Up/Down",
        "jump between user prompts"
    ));
    let help_text = help.join("\n");
    assert!(!help_text.contains("desktop queue follow-up pending"));
    assert!(!help_text.contains("1  question"));

    assert_eq!(app.handle_key(KeyInput::Escape), KeyOutcome::Redraw);
    assert!(!app.show_help);
    assert_eq!(app.handle_key(KeyInput::Escape), KeyOutcome::None);
    assert!(app.body_lines().join("\n").contains("1  question"));
}

#[test]
fn single_session_escape_soft_interrupts_running_generation() {
    let mut app = SingleSessionApp::new(None);
    assert_eq!(app.handle_key(KeyInput::Escape), KeyOutcome::None);

    app.is_processing = true;
    assert_eq!(
        app.handle_key(KeyInput::Escape),
        KeyOutcome::CancelGeneration
    );
}

fn help_has_shortcut(lines: &[String], shortcut: &str, description: &str) -> bool {
    lines
        .iter()
        .any(|line| line.contains(shortcut) && line.contains(description))
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
fn single_session_session_switcher_loads_filters_and_resumes_session() {
    let mut app = SingleSessionApp::new(None);
    app.messages
        .push(SingleSessionMessage::user("stale live transcript"));
    app.handle_key(KeyInput::Character("pending draft".to_string()));

    assert_eq!(
        app.handle_key(KeyInput::OpenSessionSwitcher),
        KeyOutcome::LoadSessionSwitcher
    );
    assert!(app.session_switcher.open);
    assert!(app.session_switcher.loading);
    assert!(
        app.body_lines()
            .join("\n")
            .contains("loading recent sessions")
    );

    app.apply_session_switcher_cards(vec![
        test_session_card("session_alpha", "alpha", "alpha status"),
        test_session_card("session_beta", "beta", "beta status"),
    ]);
    let switcher = app.body_lines().join("\n");
    assert!(switcher.contains("desktop session switcher"));
    assert!(switcher.contains("alpha"));
    assert!(switcher.contains("beta"));

    assert_eq!(
        app.handle_key(KeyInput::Character("beta".to_string())),
        KeyOutcome::Redraw
    );
    assert!(app.body_lines().join("\n").contains("filter: beta"));

    assert_eq!(app.handle_key(KeyInput::SubmitDraft), KeyOutcome::Redraw);
    assert!(!app.session_switcher.open);
    assert_eq!(
        app.session
            .as_ref()
            .map(|session| session.session_id.as_str()),
        Some("session_beta")
    );
    assert_eq!(app.live_session_id.as_deref(), Some("session_beta"));
    assert_eq!(app.draft, "pending draft");
    assert_eq!(app.status.as_deref(), Some("resumed beta"));

    let resumed = app.body_lines().join("\n");
    assert!(resumed.contains("beta status"));
    assert!(!resumed.contains("stale live transcript"));
}

#[test]
fn single_session_session_switcher_marks_current_session_and_reloads() {
    let alpha = test_session_card("session_alpha", "alpha", "active");
    let beta = test_session_card("session_beta", "beta", "idle");
    let mut app = SingleSessionApp::new(Some(alpha.clone()));

    assert_eq!(
        app.handle_key(KeyInput::OpenSessionSwitcher),
        KeyOutcome::LoadSessionSwitcher
    );
    app.apply_session_switcher_cards(vec![beta, alpha]);

    assert_eq!(app.session_switcher.selected, 1);
    assert!(app.body_lines().join("\n").contains("› ✓ alpha"));

    assert_eq!(
        app.handle_key(KeyInput::RefreshSessions),
        KeyOutcome::LoadSessionSwitcher
    );
    assert!(app.session_switcher.loading);
    assert_eq!(app.status.as_deref(), Some("loading recent sessions"));
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
fn desktop_help_text_documents_desktop_options() {
    let help = desktop_help_text();

    assert!(help.contains("Usage:"));
    assert!(help.contains("--fullscreen"));
    assert!(help.contains("--workspace"));
    assert!(help.contains("--headless-chat-smoke <MSG>"));
    assert!(help.contains("--version"));
    assert!(help.contains("--help"));
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
fn long_single_session_transcript_exposes_scrollbar_metrics() {
    let mut app = SingleSessionApp::new(None);
    let size = PhysicalSize::new(900, 360);
    for index in 0..48 {
        app.messages
            .push(SingleSessionMessage::assistant(format!("message {index}")));
    }

    let metrics = single_session_body_scroll_metrics(&app, size, 0).expect("scroll metrics");
    assert!(metrics.total_lines > metrics.visible_lines);
    assert!(metrics.max_scroll_lines > 0);

    let vertices = build_single_session_vertices(&app, size, 0.0, 0);
    assert!(
        vertices
            .iter()
            .any(|vertex| vertex.color == [0.035, 0.065, 0.145, 0.34])
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
            .any(|line| line.contains("shared desktop session runtime"))
    );
    assert!(
        single_session_lines(None)
            .iter()
            .all(|line| !line.contains("execution is connected"))
    );
}

#[test]
fn fresh_single_session_shows_animated_welcome_screen() {
    let app = SingleSessionApp::new(None);
    let first = single_session_text_key_for_tick(&app, PhysicalSize::new(900, 700), 0);
    let later = single_session_text_key_for_tick(&app, PhysicalSize::new(900, 700), 42);

    assert!(
        first.welcome_hero.contains("Hello there") || first.welcome_hero.contains("Welcome, "),
        "expected generic or named welcome, got: {:?}",
        first.welcome_hero
    );
    assert!(first.body.is_empty());
    assert!(first.welcome_hint.is_empty());
    assert_eq!(first.welcome_hero, later.welcome_hero);
}

#[test]
fn welcome_name_is_optional_and_sanitized() {
    assert_eq!(
        sanitize_welcome_name("  Jeremy Huang  "),
        Some("Jeremy".to_string())
    );
    assert_eq!(sanitize_welcome_name("unknown"), None);
    assert_eq!(sanitize_welcome_name("   "), None);

    let named = welcome_styled_lines(&Some("Jeremy".to_string()), 0);
    assert_eq!(named[0].text, "Welcome, Jeremy");
    let generic = welcome_styled_lines(&None, 0);
    assert_eq!(generic[0].text, "Hello there");
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
