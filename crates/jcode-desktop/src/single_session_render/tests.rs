//! Tests for single_session_render, split out of the parent module.

use super::*;
use crate::single_session::{
    InlineWidgetKind, SingleSessionApp, SingleSessionLineStyle, SingleSessionStyledLine,
    SingleSessionToolLineKind, SingleSessionToolLineMetadata, SingleSessionToolVisualState,
};
use crate::workspace::{KeyInput, KeyOutcome, SessionCard};

fn test_tool_line(
    call_id: &str,
    state: SingleSessionToolVisualState,
    active: bool,
    kind: SingleSessionToolLineKind,
) -> SingleSessionStyledLine {
    SingleSessionStyledLine::new(format!("  ▾ {call_id}"), SingleSessionLineStyle::Tool)
        .with_tool_metadata(SingleSessionToolLineMetadata {
            call_id: call_id.to_string(),
            name: call_id.to_string(),
            state,
            kind,
            active,
            expanded: matches!(kind, SingleSessionToolLineKind::Detail),
            stdin_prompt: None,
        })
}

fn test_transcript_card_visual_for_line(
    frame: &TranscriptCardMotionFrame,
    lines: &[SingleSessionStyledLine],
    target_line: usize,
) -> TranscriptCardVisual {
    let mut occurrences = HashMap::new();
    for run in single_session_transcript_card_runs(lines) {
        let key = transcript_card_motion_key(lines, &run, &mut occurrences);
        if run.line == target_line {
            return frame.visual_for_key(key).expect("transcript card visual");
        }
    }
    panic!("missing transcript card run at line {target_line}");
}

fn test_transcript_message_visual_for_line(
    frame: &TranscriptMessageMotionFrame,
    lines: &[SingleSessionStyledLine],
    target_line: usize,
) -> TranscriptMessageVisual {
    let mut occurrences = HashMap::new();
    for run in single_session_transcript_message_runs(lines) {
        let key = transcript_message_motion_key(lines, &run, &mut occurrences);
        if run.line == target_line {
            return frame
                .visual_for_key(key)
                .expect("transcript message visual");
        }
    }
    panic!("missing transcript message run at line {target_line}");
}

fn test_inline_markdown_pill_visual_for_line(
    frame: &InlineMarkdownPillMotionFrame,
    lines: &[SingleSessionStyledLine],
    target_line: usize,
    target_kind: InlineMarkdownPillKind,
) -> InlineMarkdownPillVisual {
    let mut occurrences = HashMap::new();
    for run in single_session_inline_markdown_pill_runs(lines) {
        let key = inline_markdown_pill_motion_key(lines, &run, &mut occurrences);
        if run.line == target_line && run.kind == target_kind {
            return frame
                .visual_for_key(key)
                .expect("inline markdown pill visual");
        }
    }
    panic!("missing inline markdown pill run at line {target_line}");
}

fn test_inline_widget_reflow_visual_for_text(
    frame: &InlineWidgetListReflowMotionFrame,
    kind: InlineWidgetKind,
    lines: &[SingleSessionStyledLine],
    needle: &str,
) -> InlineWidgetListReflowVisual {
    for run in inline_widget_list_row_runs(Some(kind), lines, lines.len()) {
        let end = run.line.saturating_add(run.line_span).min(lines.len());
        if lines[run.line..end]
            .iter()
            .any(|line| line.text.contains(needle))
        {
            return frame
                .visual_for_key(run.key)
                .expect("inline widget reflow visual");
        }
    }
    panic!("missing inline widget reflow row containing {needle}");
}

fn test_attachment_chip_visual_for_index(
    frame: &AttachmentChipMotionFrame,
    images: &[(String, String)],
    index: usize,
) -> AttachmentChipVisual {
    let run = attachment_chip_runs(images)
        .into_iter()
        .find(|run| run.index == index)
        .expect("attachment chip run");
    frame
        .visual_for_key(run.key)
        .expect("attachment chip visual")
}

fn test_session_card(session_id: &str, title: &str) -> SessionCard {
    SessionCard {
        session_id: session_id.to_string(),
        title: title.to_string(),
        subtitle: "active · test-model".to_string(),
        detail: "3 msgs · just now · jcode".to_string(),
        preview_lines: vec![
            "Prompt 1  inspect compact desktop geometry".to_string(),
            "Assistant  layout lanes should stay separated".to_string(),
        ],
        detail_lines: vec![
            "Prompt 1  inspect compact desktop geometry".to_string(),
            "Assistant  layout lanes should stay separated".to_string(),
        ],
        transcript_messages: Vec::new(),
    }
}

fn assert_single_session_layout_invariants(app: &SingleSessionApp, size: PhysicalSize<u32>) {
    let total_lines = welcome_timeline_total_body_lines(app, size).max(1);
    let layout = single_session_layout_for_total_lines(app, size, total_lines);
    let base_bottom = single_session_body_bottom_base_for_total_lines(app, size, total_lines);

    assert!(
        layout.body.width >= 1.0,
        "body width should be renderable: {layout:?}"
    );
    assert!(
        layout.body.y >= PANEL_BODY_TOP_PADDING - 0.001,
        "body starts above panel lane: {layout:?}"
    );
    assert!(
        layout.body.height >= 0.0,
        "body height should never be negative: {layout:?}"
    );
    assert!(
        layout.body_bottom() <= base_bottom + 0.001,
        "body exceeds reserved bottom: {layout:?}, base_bottom={base_bottom}"
    );
    assert!(
        layout.composer.y >= layout.draft_top - 9.001,
        "composer y should derive from draft lane: {layout:?}"
    );
    assert!(
        layout.composer.width >= layout.body.width,
        "composer should cover body width: {layout:?}"
    );
    if let Some(activity) = layout.activity_lane {
        assert!(
            layout.body_bottom() <= activity.y + 0.001,
            "activity overlaps body: {layout:?}"
        );
        assert!(
            rect_bottom(activity) <= base_bottom + 0.001,
            "activity exceeds base bottom: {layout:?}, base_bottom={base_bottom}"
        );
        assert!(
            activity.height >= 0.0,
            "activity height should not be negative: {layout:?}"
        );
    }
}

#[test]
fn single_session_layout_lanes_do_not_overlap_across_common_states() {
    let sizes = [
        PhysicalSize::new(360, 260),
        PhysicalSize::new(900, 700),
        PhysicalSize::new(1440, 1000),
    ];

    for size in sizes {
        let idle = SingleSessionApp::new(None);
        assert_single_session_layout_invariants(&idle, size);

        let mut streaming = SingleSessionApp::new(None);
        streaming.apply_session_event(session_launch::DesktopSessionEvent::TextDelta(
            "streaming response".to_string(),
        ));
        assert_single_session_layout_invariants(&streaming, size);

        let mut with_images = SingleSessionApp::new(None);
        with_images
            .pending_images
            .push(("/tmp/a.png".to_string(), "a".to_string()));
        with_images
            .pending_images
            .push(("/tmp/b.png".to_string(), "b".to_string()));
        assert_single_session_layout_invariants(&with_images, size);

        let mut multiline = SingleSessionApp::new(None);
        multiline.draft = "first\nsecond\nthird".to_string();
        multiline.draft_cursor = multiline.draft.len();
        assert_single_session_layout_invariants(&multiline, size);
    }
}

#[test]
fn small_window_inline_activity_and_composer_lanes_do_not_overlap() {
    let size = PhysicalSize::new(520, 320);
    let mut app = SingleSessionApp::new(Some(test_session_card(
        "current-session",
        "current compact session",
    )));
    assert_eq!(
        app.handle_key(KeyInput::OpenSessionSwitcher),
        KeyOutcome::LoadSessionSwitcher
    );
    app.apply_session_switcher_cards(
        (0..6)
            .map(|index| {
                test_session_card(
                    &format!("resume-session-{index}"),
                    &format!("resume compact session {index}"),
                )
            })
            .collect(),
    );
    app.draft = "first line\nsecond line\nthird line".to_string();
    app.draft_cursor = app.draft.len();
    app.apply_session_event(session_launch::DesktopSessionEvent::TextDelta(
        "streaming response while the resume picker is open".to_string(),
    ));

    assert!(app.has_activity_indicator());
    assert_eq!(
        app.render_inline_widget_kind(),
        Some(InlineWidgetKind::SessionSwitcher)
    );
    assert_single_session_layout_invariants(&app, size);

    let total_lines = welcome_timeline_total_body_lines(&app, size).max(1);
    let layout = single_session_layout_for_total_lines(&app, size, total_lines);
    let activity = layout.activity_lane.expect("streaming activity lane");
    let inline_lines = app.render_inline_widget_styled_lines();
    let inline_kind = app.render_inline_widget_kind();
    let typography = single_session_typography_for_scale(app.text_scale());
    let inline_width =
        inline_widget_text_width_for_lines(inline_kind, &inline_lines, size, app.text_scale());
    let inline_layout = inline_widget_card_layout_with_bottom_limit(
        size,
        inline_kind,
        &typography,
        app.render_inline_widget_visible_line_count(),
        inline_width,
        inline_widget_target_top(
            size,
            inline_kind,
            app.text_scale(),
            layout.body_bottom(),
            false,
            0.0,
        ),
        app.render_inline_widget_reveal_progress(),
        activity.y,
    )
    .expect("inline widget card layout");

    assert!(
        inline_layout.text_top >= layout.body_bottom() + INLINE_WIDGET_BODY_GAP - 0.001,
        "inline text should start below the body: layout={layout:?}, inline={inline_layout:?}"
    );
    assert!(
        rect_bottom(inline_layout.card) <= activity.y + 0.001,
        "inline card should stay above the activity lane: activity={activity:?}, inline={inline_layout:?}"
    );
    assert!(
        rect_bottom(activity) <= layout.composer.y + 0.001,
        "activity lane should stay above composer chrome: layout={layout:?}, activity={activity:?}"
    );
    assert!(
        rect_bottom(inline_layout.card) <= layout.draft_top - 7.5,
        "inline card should leave the composer lane clear: layout={layout:?}, inline={inline_layout:?}"
    );

    if let Some(columns) = session_switcher_split_columns(&inline_layout) {
        assert!(
            columns.rail.x + columns.rail.width <= columns.gap.x + 0.001,
            "session rail should not overlap split gap: {columns:?}"
        );
        assert!(
            columns.gap.x + columns.gap.width <= columns.preview.x + 0.001,
            "split gap should not overlap preview pane: {columns:?}"
        );
        assert!(
            columns.preview.x + columns.preview.width
                <= inline_layout.card.x + inline_layout.card.width + 0.001,
            "preview pane should stay inside inline card: columns={columns:?}, inline={inline_layout:?}"
        );
    }

    assert!(
        !build_single_session_vertices(&app, size, 0.0, 0).is_empty(),
        "compact combined state should render primitives"
    );
}

#[test]
fn body_wrap_ascii_fast_path_preserves_word_boundaries() {
    assert!(!text_exceeds_columns("0123456789", 10));
    assert!(text_exceeds_columns("0123456789a", 10));
    assert_eq!(word_wrap_split_index("alpha beta gamma", 10), 5);
    assert_eq!(word_wrap_split_index("abcdefghijk", 10), 10);
}

#[test]
fn body_wrap_unicode_path_keeps_character_boundaries() {
    assert!(!text_exceeds_columns("你好世界", 4));
    assert!(text_exceeds_columns("你好世界a", 4));
    assert_eq!(word_wrap_split_index("你好 abc", 3), "你好".len());
    assert_eq!(byte_index_at_char_limit("你好abc", 2), "你好".len());
}

#[test]
fn body_wrap_line_count_matches_wrapped_output_without_allocating_lines() {
    let cases = [
        SingleSessionStyledLine::new("alpha beta gamma", SingleSessionLineStyle::Assistant),
        SingleSessionStyledLine::new("abcdefghijk", SingleSessionLineStyle::Assistant),
        SingleSessionStyledLine::new("你好 abc", SingleSessionLineStyle::Assistant),
        SingleSessionStyledLine::with_inline_spans(
            "code span keeps trailing spaces   ".to_string(),
            SingleSessionLineStyle::Assistant,
            vec![SingleSessionInlineSpan {
                start: 0,
                end: "code span keeps trailing spaces   ".len(),
                kind: SingleSessionInlineSpanKind::Code,
            }],
        ),
    ];

    for line in cases {
        let mut wrapped = Vec::new();
        push_wrapped_body_line_ref(&mut wrapped, &line, 10);
        assert_eq!(wrapped_body_line_count(&line, 10), wrapped.len());
    }
}

#[test]
fn inline_widget_selection_target_detects_widget_row_shapes() {
    let model_lines = vec![
        SingleSessionStyledLine::new("title", SingleSessionLineStyle::OverlayTitle),
        SingleSessionStyledLine::new("filter", SingleSessionLineStyle::Overlay),
        SingleSessionStyledLine::new("gpt", SingleSessionLineStyle::OverlaySelection),
        SingleSessionStyledLine::new("provider · detail", SingleSessionLineStyle::Meta),
        SingleSessionStyledLine::new("footer", SingleSessionLineStyle::Overlay),
    ];
    assert_eq!(
        inline_widget_selection_target(
            Some(InlineWidgetKind::ModelPicker),
            &model_lines,
            model_lines.len()
        ),
        Some(InlineWidgetSelectionTarget {
            kind: InlineWidgetKind::ModelPicker,
            line: 2,
            line_span: 2,
        })
    );

    let session_lines = vec![
        SingleSessionStyledLine::new("header", SingleSessionLineStyle::OverlayTitle),
        SingleSessionStyledLine::new("body", SingleSessionLineStyle::Overlay),
        SingleSessionStyledLine::new(
            "active session · current · alpha",
            SingleSessionLineStyle::OverlaySelection,
        ),
        SingleSessionStyledLine::new(
            "Status active · Model test-model",
            SingleSessionLineStyle::OverlaySelection,
        ),
        SingleSessionStyledLine::new(
            "2 msgs · alpha-workspace",
            SingleSessionLineStyle::OverlaySelection,
        ),
        SingleSessionStyledLine::new(
            "latest prompt: hello",
            SingleSessionLineStyle::OverlaySelection,
        ),
        SingleSessionStyledLine::new("next", SingleSessionLineStyle::Overlay),
    ];
    assert_eq!(
        inline_widget_selection_target(
            Some(InlineWidgetKind::SessionSwitcher),
            &session_lines,
            session_lines.len()
        ),
        Some(InlineWidgetSelectionTarget {
            kind: InlineWidgetKind::SessionSwitcher,
            line: 2,
            line_span: 4,
        })
    );
}

fn vertex_count_for_color(vertices: &[Vertex], color: [f32; 4]) -> usize {
    vertices
        .iter()
        .filter(|vertex| vertex.color == color)
        .count()
}

#[test]
fn inline_widget_command_palettes_draw_structured_cards_not_text_boxes() {
    let size = PhysicalSize::new(1000, 720);
    let typography = single_session_typography_for_scale(1.0);
    let model_lines = vec![
        SingleSessionStyledLine::new("Model picker", SingleSessionLineStyle::OverlayTitle),
        SingleSessionStyledLine::new("type to filter", SingleSessionLineStyle::Overlay),
        SingleSessionStyledLine::new("gpt-5.4", SingleSessionLineStyle::OverlaySelection),
        SingleSessionStyledLine::new("OpenAI · chat · available", SingleSessionLineStyle::Meta),
        SingleSessionStyledLine::new("claude-sonnet", SingleSessionLineStyle::Overlay),
        SingleSessionStyledLine::new("Anthropic · chat · available", SingleSessionLineStyle::Meta),
    ];
    let model_layout = inline_widget_card_layout(
        size,
        Some(InlineWidgetKind::ModelPicker),
        &typography,
        model_lines.len(),
        520.0,
        130.0,
        1.0,
    )
    .expect("model picker layout");
    let mut app = SingleSessionApp::new(None);
    app.handle_key(KeyInput::OpenModelPicker);
    app.apply_session_event(session_launch::DesktopSessionEvent::ModelCatalog {
        current_model: Some("gpt-5.4".to_string()),
        provider_name: Some("OpenAI".to_string()),
        models: vec![
            session_launch::DesktopModelChoice {
                model: "gpt-5.4".to_string(),
                provider: Some("OpenAI".to_string()),
                api_method: Some("chat".to_string()),
                detail: Some("available".to_string()),
                available: true,
            },
            session_launch::DesktopModelChoice {
                model: "claude-sonnet".to_string(),
                provider: Some("Anthropic".to_string()),
                api_method: Some("chat".to_string()),
                detail: Some("available".to_string()),
                available: true,
            },
        ],
        reasoning_effort: None,
        service_tier: None,
        compaction_mode: None,
    });
    let mut model_vertices = Vec::new();
    push_single_session_inline_widget_structured_chrome(
        &mut model_vertices,
        &app,
        Some(InlineWidgetKind::ModelPicker),
        &model_lines,
        model_lines.len(),
        &typography,
        &model_layout,
        1.0,
        size,
    );
    assert!(
        vertex_count_for_color(&model_vertices, INLINE_COMMAND_ROW_SELECTED_COLOR) > 0,
        "selected model row should be a rendered rounded card"
    );
    assert!(
        vertex_count_for_color(&model_vertices, INLINE_COMMAND_ROW_BACKGROUND_COLOR) > 0,
        "unselected model row should be a rendered rounded card"
    );
    assert!(
        vertex_count_for_color(&model_vertices, MODEL_PICKER_ROW_ACCENT_COLOR) > 0,
        "selected model row should use a rendered accent rail instead of selector text"
    );

    let session_lines = vec![
        SingleSessionStyledLine::new("Resume sessions", SingleSessionLineStyle::OverlayTitle),
        SingleSessionStyledLine::new(
            "Recent sessions · focused · newest first",
            SingleSessionLineStyle::OverlayTitle,
        ),
        SingleSessionStyledLine::new(
            "active session · current · alpha",
            SingleSessionLineStyle::OverlaySelection,
        ),
        SingleSessionStyledLine::new(
            "Status active · Model test-model",
            SingleSessionLineStyle::OverlaySelection,
        ),
        SingleSessionStyledLine::new(
            "2 msgs · alpha-workspace",
            SingleSessionLineStyle::OverlaySelection,
        ),
        SingleSessionStyledLine::new(
            "latest prompt: hello",
            SingleSessionLineStyle::OverlaySelection,
        ),
        SingleSessionStyledLine::new("", SingleSessionLineStyle::Blank),
        SingleSessionStyledLine::new(
            "Preview · selected session transcript",
            SingleSessionLineStyle::OverlayTitle,
        ),
        SingleSessionStyledLine::new("Prompt 1  hello", SingleSessionLineStyle::User),
    ];
    let session_layout = inline_widget_card_layout(
        size,
        Some(InlineWidgetKind::SessionSwitcher),
        &typography,
        session_lines.len(),
        760.0,
        80.0,
        1.0,
    )
    .expect("session switcher layout");
    let mut session_vertices = Vec::new();
    push_single_session_inline_widget_structured_chrome(
        &mut session_vertices,
        &app,
        Some(InlineWidgetKind::SessionSwitcher),
        &session_lines,
        session_lines.len(),
        &typography,
        &session_layout,
        1.0,
        size,
    );
    assert!(
        vertex_count_for_color(&session_vertices, INLINE_COMMAND_SECTION_BACKGROUND_COLOR) > 0,
        "resume list section should be a rendered panel"
    );
    assert!(
        vertex_count_for_color(&session_vertices, INLINE_COMMAND_PREVIEW_BACKGROUND_COLOR) > 0,
        "resume preview section should be a rendered panel"
    );
    let selected_resume_fill =
        resume_session_row_palette("active session · current · alpha", true).fill;
    assert!(
        vertex_count_for_color(&session_vertices, selected_resume_fill) > 0,
        "selected resume row should be a rendered status card"
    );
}

#[test]
fn inline_widget_card_layout_clamps_tall_command_palettes_above_composer() {
    let size = PhysicalSize::new(920, 500);
    let typography = single_session_typography_for_scale(1.0);
    let line_height =
        inline_widget_line_height(Some(InlineWidgetKind::SessionSwitcher), &typography);
    let layout = inline_widget_card_layout(
        size,
        Some(InlineWidgetKind::SessionSwitcher),
        &typography,
        80,
        1400.0,
        92.0,
        1.0,
    )
    .expect("session switcher layout");
    let draft_top = single_session_draft_top(size);
    assert!(layout.card.y >= PANEL_TITLE_TOP_PADDING);
    assert!(
        layout.card.y + layout.card.height <= draft_top - 7.5,
        "inline card should leave the composer lane clear: card_bottom={} draft_top={}",
        layout.card.y + layout.card.height,
        draft_top
    );
    assert!(
        layout.card.height <= size.height as f32 * 0.56 + 0.1,
        "tall resume preview should be capped to a desktop palette height"
    );
    assert!(layout.visible_text_bottom <= draft_top);
    assert!(
        layout.visible_text_bottom < layout.text_top + line_height * 80.0,
        "oversized session lists should clip inside the card instead of growing into the composer"
    );
}

#[test]
fn inline_widget_preview_pane_target_tracks_focus_and_preview_content() {
    let sessions_focused = vec![
        SingleSessionStyledLine::new(
            "desktop session switcher",
            SingleSessionLineStyle::OverlayTitle,
        ),
        SingleSessionStyledLine::new("filter", SingleSessionLineStyle::Overlay),
        SingleSessionStyledLine::new("", SingleSessionLineStyle::Blank),
        SingleSessionStyledLine::new(
            "│ sessions › · recent │ preview · full selected-session preview │",
            SingleSessionLineStyle::OverlayTitle,
        ),
        SingleSessionStyledLine::new(
            "│ alpha │ assistant answer │",
            SingleSessionLineStyle::OverlaySelection,
        ),
        SingleSessionStyledLine::new("╰────╯ ╰────╯", SingleSessionLineStyle::Meta),
    ];
    let preview_focused = vec![
        SingleSessionStyledLine::new(
            "desktop session switcher",
            SingleSessionLineStyle::OverlayTitle,
        ),
        SingleSessionStyledLine::new("filter", SingleSessionLineStyle::Overlay),
        SingleSessionStyledLine::new("", SingleSessionLineStyle::Blank),
        SingleSessionStyledLine::new(
            "│ sessions · recent │ preview › · full selected-session preview │",
            SingleSessionLineStyle::OverlayTitle,
        ),
        SingleSessionStyledLine::new(
            "│ alpha │ assistant answer │",
            SingleSessionLineStyle::OverlaySelection,
        ),
        SingleSessionStyledLine::new("╰────╯ ╰────╯", SingleSessionLineStyle::Meta),
    ];
    let changed_preview = vec![
        SingleSessionStyledLine::new(
            "desktop session switcher",
            SingleSessionLineStyle::OverlayTitle,
        ),
        SingleSessionStyledLine::new("filter", SingleSessionLineStyle::Overlay),
        SingleSessionStyledLine::new("", SingleSessionLineStyle::Blank),
        SingleSessionStyledLine::new(
            "│ sessions · recent │ preview › · full selected-session preview │",
            SingleSessionLineStyle::OverlayTitle,
        ),
        SingleSessionStyledLine::new(
            "│ beta │ user different prompt │",
            SingleSessionLineStyle::OverlaySelection,
        ),
        SingleSessionStyledLine::new("╰────╯ ╰────╯", SingleSessionLineStyle::Meta),
    ];

    let sessions_target = inline_widget_preview_pane_target(
        Some(InlineWidgetKind::SessionSwitcher),
        &sessions_focused,
        sessions_focused.len(),
    )
    .expect("session switcher preview target");
    let preview_target = inline_widget_preview_pane_target(
        Some(InlineWidgetKind::SessionSwitcher),
        &preview_focused,
        preview_focused.len(),
    )
    .expect("preview focused target");
    let changed_target = inline_widget_preview_pane_target(
        Some(InlineWidgetKind::SessionSwitcher),
        &changed_preview,
        changed_preview.len(),
    )
    .expect("changed preview target");

    assert_eq!(sessions_target.focus_pane, 0);
    assert_eq!(preview_target.focus_pane, 1);
    assert_ne!(preview_target.preview_key, changed_target.preview_key);
    assert!(
        inline_widget_preview_pane_target(
            Some(InlineWidgetKind::ModelPicker),
            &preview_focused,
            preview_focused.len(),
        )
        .is_none()
    );
}

#[test]
fn inline_widget_preview_pane_motion_animates_focus_and_content_changes() {
    let mut registry = InlineWidgetPreviewPaneMotionRegistry::default();
    let now = Instant::now();
    let sessions_target = InlineWidgetPreviewPaneTarget {
        kind: InlineWidgetKind::SessionSwitcher,
        focus_pane: 0,
        preview_key: 10,
    };
    let preview_target = InlineWidgetPreviewPaneTarget {
        kind: InlineWidgetKind::SessionSwitcher,
        focus_pane: 1,
        preview_key: 10,
    };
    let changed_preview_target = InlineWidgetPreviewPaneTarget {
        kind: InlineWidgetKind::SessionSwitcher,
        focus_pane: 1,
        preview_key: 42,
    };

    let initial = registry.frame_for_target(Some(sessions_target), now);
    assert!(!initial.is_active());
    assert_eq!(
        initial.visual(),
        Some(InlineWidgetPreviewPaneVisual::settled(sessions_target))
    );

    let focus_start =
        registry.frame_for_target(Some(preview_target), now + Duration::from_millis(6));
    let focus_start_visual = focus_start.visual().expect("preview focus visual");
    assert!(focus_start.is_active());
    assert_eq!(focus_start_visual.focus_pane_position, 0.0);

    let focus = registry.frame_for_target(
        Some(preview_target),
        now + Duration::from_millis(6) + INLINE_WIDGET_PREVIEW_PANE_FOCUS_DURATION / 2,
    );
    let focus_visual = focus.visual().expect("preview focus visual");
    assert!(focus.is_active());
    assert!(focus_visual.focus_pane_position > 0.0);
    assert!(focus_visual.focus_pane_position < 1.0);
    assert_eq!(focus_visual.preview_opacity, 1.0);

    let settled_focus = registry.frame_for_target(
        Some(preview_target),
        now + INLINE_WIDGET_PREVIEW_PANE_FOCUS_DURATION * 2,
    );
    assert!(!settled_focus.is_active());

    let content = registry.frame_for_target(
        Some(changed_preview_target),
        now + INLINE_WIDGET_PREVIEW_PANE_FOCUS_DURATION * 2 + Duration::from_millis(4),
    );
    let content_visual = content.visual().expect("preview content visual");
    assert!(content.is_active());
    assert_eq!(content_visual.focus_pane_position, 1.0);
    assert!(content_visual.preview_opacity < 0.5);
    assert!(content_visual.preview_y_offset_pixels > 3.0);

    let settled_content = registry.frame_for_target(
        Some(changed_preview_target),
        now + INLINE_WIDGET_PREVIEW_PANE_FOCUS_DURATION * 2
            + INLINE_WIDGET_PREVIEW_PANE_CONTENT_DURATION * 2,
    );
    assert!(!settled_content.is_active());
    assert_eq!(
        settled_content.visual(),
        Some(InlineWidgetPreviewPaneVisual::settled(
            changed_preview_target
        ))
    );
}

#[test]
fn streaming_activity_cue_motion_animates_entry_and_exit() {
    let mut registry = StreamingActivityCueMotionRegistry::default();
    let now = Instant::now();

    let idle = registry.frame_for_visible(false, now);
    assert!(!idle.is_active());
    assert!(idle.current().is_none());
    assert!(idle.exiting().is_none());

    let entry_start = registry.frame_for_visible(true, now + Duration::from_millis(8));
    let entry_start_visual = entry_start.current().expect("activity entry visual");
    assert!(entry_start.is_active());
    assert!(entry_start_visual.opacity <= 0.001);
    assert!(entry_start_visual.y_offset_pixels > 0.0);
    assert!(entry_start_visual.scale < 1.0);

    let entry_mid = registry.frame_for_visible(
        true,
        now + Duration::from_millis(8) + STREAMING_ACTIVITY_CUE_ENTRY_DURATION / 2,
    );
    let entry_mid_visual = entry_mid.current().expect("activity entry visual");
    assert!(entry_mid.is_active());
    assert!(entry_mid_visual.opacity > 0.0);
    assert!(entry_mid_visual.opacity < 1.0);

    let settled = registry.frame_for_visible(
        true,
        now + Duration::from_millis(8) + STREAMING_ACTIVITY_CUE_ENTRY_DURATION * 2,
    );
    assert!(!settled.is_active());
    assert_eq!(
        settled.current(),
        Some(StreamingActivityCueVisual::settled())
    );

    let exit_start = registry.frame_for_visible(
        false,
        now + Duration::from_millis(8) + STREAMING_ACTIVITY_CUE_ENTRY_DURATION * 3,
    );
    let exit_start_visual = exit_start.exiting().expect("activity exit visual");
    assert!(exit_start.is_active());
    assert_eq!(exit_start.current(), None);
    assert!(exit_start_visual.opacity > 0.99);

    let exit_mid = registry.frame_for_visible(
        false,
        now + Duration::from_millis(8)
            + STREAMING_ACTIVITY_CUE_ENTRY_DURATION * 3
            + STREAMING_ACTIVITY_CUE_EXIT_DURATION / 2,
    );
    let exit_mid_visual = exit_mid.exiting().expect("activity exit visual");
    assert!(exit_mid.is_active());
    assert!(exit_mid_visual.opacity > 0.0);
    assert!(exit_mid_visual.opacity < 1.0);
    assert!(exit_mid_visual.y_offset_pixels < 0.0);

    let exit_done = registry.frame_for_visible(
        false,
        now + Duration::from_millis(8)
            + STREAMING_ACTIVITY_CUE_ENTRY_DURATION * 3
            + STREAMING_ACTIVITY_CUE_EXIT_DURATION * 2,
    );
    assert!(!exit_done.is_active());
    assert!(exit_done.exiting().is_none());
}

#[test]
fn reduced_motion_snaps_streaming_activity_cue_motion() {
    let _guard = crate::animation::DesktopReducedMotionEnvGuard::set(true);
    let mut registry = StreamingActivityCueMotionRegistry::default();
    let now = Instant::now();

    assert!(!registry.frame_for_visible(false, now).is_active());
    let visible = registry.frame_for_visible(true, now + Duration::from_millis(1));
    assert!(!visible.is_active());
    assert_eq!(
        visible.current(),
        Some(StreamingActivityCueVisual::settled())
    );

    let hidden = registry.frame_for_visible(false, now + Duration::from_millis(2));
    assert!(!hidden.is_active());
    assert!(hidden.current().is_none());
    assert!(hidden.exiting().is_none());
}

#[test]
fn inline_widget_selection_motion_animates_row_changes() {
    let mut registry = InlineWidgetSelectionMotionRegistry::default();
    let now = Instant::now();
    let first_target = InlineWidgetSelectionTarget {
        kind: InlineWidgetKind::SlashSuggestions,
        line: 1,
        line_span: 1,
    };
    let next_target = InlineWidgetSelectionTarget {
        kind: InlineWidgetKind::SlashSuggestions,
        line: 3,
        line_span: 1,
    };

    let initial = registry.frame_for_target(Some(first_target), now);
    assert!(!initial.is_active());
    assert_eq!(
        initial.visual_for_target(first_target),
        Some(InlineWidgetSelectionVisual::settled(first_target))
    );

    let start = registry.frame_for_target(Some(next_target), now + Duration::from_millis(5));
    let start_visual = start
        .visual_for_target(next_target)
        .expect("selection visual");
    assert!(start.is_active());
    assert!(start_visual.y_offset_lines < -1.9);
    assert_eq!(start_visual.line_span, 1.0);

    let middle = registry.frame_for_target(
        Some(next_target),
        now + Duration::from_millis(5) + INLINE_WIDGET_SELECTION_TRANSITION_DURATION / 2,
    );
    let middle_visual = middle
        .visual_for_target(next_target)
        .expect("selection visual");
    assert!(middle.is_active());
    assert!(middle_visual.y_offset_lines < 0.0);
    assert!(middle_visual.y_offset_lines > -2.0);

    let settled = registry.frame_for_target(
        Some(next_target),
        now + Duration::from_millis(5) + INLINE_WIDGET_SELECTION_TRANSITION_DURATION * 2,
    );
    assert!(!settled.is_active());
    assert_eq!(
        settled.visual_for_target(next_target),
        Some(InlineWidgetSelectionVisual::settled(next_target))
    );
}

#[test]
fn inline_widget_list_reflow_motion_animates_filter_insert_shift_and_exit() {
    let mut registry = InlineWidgetListReflowMotionRegistry::default();
    let now = Instant::now();
    let kind = InlineWidgetKind::SlashSuggestions;
    let first = vec![
        SingleSessionStyledLine::new(
            "slash command suggestions",
            SingleSessionLineStyle::OverlayTitle,
        ),
        SingleSessionStyledLine::new(" /copy       copy latest", SingleSessionLineStyle::Overlay),
        SingleSessionStyledLine::new(" /model      switch model", SingleSessionLineStyle::Overlay),
    ];

    let initial = registry.frame_for_rows(Some(kind), &first, first.len(), now);
    assert!(!initial.is_active());

    let filtered = vec![
        SingleSessionStyledLine::new(
            "slash command suggestions",
            SingleSessionLineStyle::OverlayTitle,
        ),
        SingleSessionStyledLine::new(
            " /commands   show commands",
            SingleSessionLineStyle::Overlay,
        ),
        SingleSessionStyledLine::new(" /copy       copy latest", SingleSessionLineStyle::Overlay),
        SingleSessionStyledLine::new(" /model      switch model", SingleSessionLineStyle::Overlay),
    ];
    let reflow = registry.frame_for_rows(
        Some(kind),
        &filtered,
        filtered.len(),
        now + Duration::from_millis(4),
    );
    assert!(reflow.is_active());
    let inserted = test_inline_widget_reflow_visual_for_text(&reflow, kind, &filtered, "/commands");
    assert!(inserted.opacity > 0.9);
    assert!(inserted.y_offset_lines > 0.4);
    let shifted = test_inline_widget_reflow_visual_for_text(&reflow, kind, &filtered, "/copy");
    assert!(shifted.opacity > 0.8);
    assert!(shifted.y_offset_lines < -0.9);

    let settled = registry.frame_for_rows(
        Some(kind),
        &filtered,
        filtered.len(),
        now + Duration::from_millis(4) + INLINE_WIDGET_LIST_REFLOW_SHIFT_DURATION * 2,
    );
    assert!(!settled.is_active());

    let removed = vec![
        SingleSessionStyledLine::new(
            "slash command suggestions",
            SingleSessionLineStyle::OverlayTitle,
        ),
        SingleSessionStyledLine::new(" /copy       copy latest", SingleSessionLineStyle::Overlay),
    ];
    let exit = registry.frame_for_rows(
        Some(kind),
        &removed,
        removed.len(),
        now + Duration::from_millis(4)
            + INLINE_WIDGET_LIST_REFLOW_SHIFT_DURATION * 2
            + Duration::from_millis(4),
    );
    assert!(exit.is_active());
    assert_eq!(exit.exiting().len(), 2);
    assert!(
        exit.exiting()
            .iter()
            .all(|(_, visual)| visual.opacity > 0.9)
    );
}

#[test]
fn composer_motion_animates_height_placeholder_focus_and_submit_affordance() {
    let mut registry = ComposerMotionRegistry::default();
    let now = Instant::now();
    let empty = ComposerMotionTarget::default();

    let initial = registry.frame_for_target(empty, now);
    assert!(!initial.is_active());
    assert_eq!(initial.visual().height_lines, 1.0);
    assert_eq!(initial.visual().placeholder_opacity, 1.0);
    assert_eq!(initial.visual().submit_opacity, 0.0);

    let typed = ComposerMotionTarget {
        line_count: 3,
        empty: false,
        blocked: false,
        processing: false,
        ready_to_submit: true,
    };
    let entry_start_time = now + Duration::from_millis(5);
    let entry_start = registry.frame_for_target(typed, entry_start_time);
    assert!(entry_start.is_active());
    assert_eq!(entry_start.visual().height_lines, 1.0);
    let entry_mid =
        registry.frame_for_target(typed, entry_start_time + COMPOSER_MOTION_DURATION / 2);
    assert!(entry_mid.is_active());
    assert!(entry_mid.visual().height_lines > 1.0);
    assert!(entry_mid.visual().height_lines < 3.0);
    assert!(entry_mid.visual().placeholder_opacity < 1.0);
    assert!(entry_mid.visual().submit_opacity > 0.0);
    assert!(entry_mid.visual().submit_opacity < 1.0);
    assert!(entry_mid.visual().submit_scale > 0.82);
    assert!(entry_mid.visual().submit_scale < 1.0);

    let settled = registry.frame_for_target(typed, now + COMPOSER_MOTION_DURATION * 2);
    assert!(!settled.is_active());
    assert_eq!(settled.visual(), ComposerMotionVisual::settled(typed));

    let blocked = ComposerMotionTarget {
        line_count: 3,
        empty: false,
        blocked: true,
        processing: true,
        ready_to_submit: true,
    };
    let blocked_start_time = now + COMPOSER_MOTION_DURATION * 2 + Duration::from_millis(5);
    let blocked_start = registry.frame_for_target(blocked, blocked_start_time);
    assert!(blocked_start.is_active());
    let blocked_mid =
        registry.frame_for_target(blocked, blocked_start_time + COMPOSER_MOTION_DURATION / 2);
    assert!(blocked_mid.is_active());
    assert!(blocked_mid.visual().focus_opacity < 1.0);
    assert!(blocked_mid.visual().blocked_progress > 0.0);
    assert!(blocked_mid.visual().processing_progress > 0.0);
}

#[test]
fn attachment_chip_motion_animates_entry_shift_and_exit() {
    let mut registry = AttachmentChipMotionRegistry::default();
    let now = Instant::now();
    let empty: Vec<(String, String)> = Vec::new();
    let first = vec![("image/png".to_string(), "aaa111".to_string())];
    let second = ("image/jpeg".to_string(), "bbb222".to_string());
    let two = vec![first[0].clone(), second.clone()];
    let remaining = vec![second];

    let initial = registry.frame_for_images(&empty, now);
    assert!(!initial.is_active());

    let entry_start_time = now + Duration::from_millis(5);
    let entry_start = registry.frame_for_images(&first, entry_start_time);
    assert!(entry_start.is_active());
    let entry_mid = registry.frame_for_images(
        &first,
        entry_start_time + ATTACHMENT_CHIP_ENTRY_DURATION / 2,
    );
    let entry_visual = test_attachment_chip_visual_for_index(&entry_mid, &first, 0);
    assert!(entry_mid.is_active());
    assert!(entry_visual.opacity > 0.0 && entry_visual.opacity < 1.0);
    assert!(entry_visual.y_offset_pixels > 0.0);
    assert!(entry_visual.scale > 0.90 && entry_visual.scale < 1.0);

    let settled_time = entry_start_time + ATTACHMENT_CHIP_ENTRY_DURATION * 2;
    let settled = registry.frame_for_images(&two, settled_time);
    assert!(settled.is_active());
    let settled =
        registry.frame_for_images(&two, settled_time + ATTACHMENT_CHIP_ENTRY_DURATION * 2);
    assert!(!settled.is_active());

    let remove_time = settled_time + ATTACHMENT_CHIP_ENTRY_DURATION * 2 + Duration::from_millis(5);
    let removal = registry.frame_for_images(&remaining, remove_time);
    assert!(removal.is_active());
    assert_eq!(removal.exiting().len(), 1);
    assert!(removal.exiting()[0].1.opacity > 0.9);
    let shifted = test_attachment_chip_visual_for_index(&removal, &remaining, 0);
    assert!(shifted.x_offset_pixels > (ATTACHMENT_CHIP_WIDTH + ATTACHMENT_CHIP_GAP) * 0.9);
}

#[test]
fn stdin_overlay_motion_animates_entry_resize_and_exit() {
    let mut registry = StdinOverlayMotionRegistry::default();
    let now = Instant::now();
    let empty = registry.frame_for_target(None, now);
    assert!(!empty.is_active());
    assert!(empty.current.is_none());
    assert!(empty.exiting.is_none());

    let requested = StdinOverlayTarget {
        key: 42,
        line_count: 8,
        input_line_start: 5,
        input_line_count: 1,
        password: true,
        has_input: false,
    };
    let entry_at = now + Duration::from_millis(5);
    let entry = registry.frame_for_target(Some(requested), entry_at);
    assert!(entry.is_active());
    let (_, entry_visual) = entry.current.expect("entry overlay visual");
    assert_eq!(entry_visual.opacity, 0.0);
    assert!(entry_visual.y_offset_pixels > 0.0);
    assert!(entry_visual.scale < 1.0);

    let entry_mid =
        registry.frame_for_target(Some(requested), entry_at + STDIN_OVERLAY_ENTRY_DURATION / 2);
    let (_, entry_mid_visual) = entry_mid.current.expect("mid entry overlay visual");
    assert!(entry_mid.is_active());
    assert!(entry_mid_visual.opacity > 0.0 && entry_mid_visual.opacity < 1.0);
    assert!(entry_mid_visual.y_offset_pixels > 0.0);

    let settled =
        registry.frame_for_target(Some(requested), entry_at + STDIN_OVERLAY_ENTRY_DURATION * 2);
    assert!(!settled.is_active());
    assert_eq!(
        settled.current.expect("settled overlay visual").1,
        StdinOverlayVisual::settled(requested)
    );

    let resized = StdinOverlayTarget {
        line_count: 11,
        input_line_count: 3,
        has_input: true,
        ..requested
    };
    let resize_at = entry_at + STDIN_OVERLAY_ENTRY_DURATION * 2 + Duration::from_millis(5);
    let resize = registry.frame_for_target(Some(resized), resize_at);
    assert!(resize.is_active());
    let resize_mid =
        registry.frame_for_target(Some(resized), resize_at + STDIN_OVERLAY_RESIZE_DURATION / 2);
    let (_, resize_visual) = resize_mid.current.expect("resize overlay visual");
    assert!(resize_visual.height_lines > requested.line_count as f32);
    assert!(resize_visual.height_lines < resized.line_count as f32);
    assert!(resize_visual.input_glow > 0.22);
    assert!(resize_visual.submit_opacity > 0.0);

    let exit_at = resize_at + STDIN_OVERLAY_RESIZE_DURATION * 2 + Duration::from_millis(5);
    let exit = registry.frame_for_target(None, exit_at);
    assert!(exit.is_active());
    assert!(exit.current.is_none());
    let (_, exit_visual) = exit.exiting.expect("exit overlay visual");
    assert!(exit_visual.opacity > 0.9);
    assert!(exit_visual.submit_opacity > 0.0);

    let exit_mid = registry.frame_for_target(None, exit_at + STDIN_OVERLAY_EXIT_DURATION / 2);
    let (_, exit_mid_visual) = exit_mid.exiting.expect("mid exit overlay visual");
    assert!(exit_mid.is_active());
    assert!(exit_mid_visual.opacity > 0.0 && exit_mid_visual.opacity < 1.0);
    assert!(exit_mid_visual.y_offset_pixels < 0.0);
}

#[test]
fn transcript_message_motion_animates_entry_and_layout_shift() {
    let mut registry = TranscriptMessageMotionRegistry::default();
    let now = Instant::now();
    let line_height = 26.0;
    let user = SingleSessionStyledLine::new("1  hello", SingleSessionLineStyle::User);
    let spacer = SingleSessionStyledLine::new("", SingleSessionLineStyle::Blank);
    let assistant = SingleSessionStyledLine::new("answer", SingleSessionLineStyle::Assistant);
    let intro = SingleSessionStyledLine::new("notice", SingleSessionLineStyle::Meta);

    let initial = registry.frame(std::slice::from_ref(&user), line_height, now);
    let initial_visual =
        test_transcript_message_visual_for_line(&initial, std::slice::from_ref(&user), 0);
    assert_eq!(initial_visual, TranscriptMessageVisual::default());
    assert!(!initial.is_active());

    let lines = vec![user.clone(), spacer.clone(), assistant];
    let entry = registry.frame(&lines, line_height, now + Duration::from_millis(5));
    let entry_visual = test_transcript_message_visual_for_line(&entry, &lines, 2);
    assert!(entry.is_active());
    assert_eq!(entry_visual.opacity, 0.0);
    assert!(entry_visual.y_offset_pixels > 0.0);
    assert!(entry_visual.scale < 1.0);

    let shifted_lines = vec![intro, user.clone(), spacer];
    let shift = registry.frame(&shifted_lines, line_height, now + Duration::from_millis(10));
    let shift_visual = test_transcript_message_visual_for_line(&shift, &shifted_lines, 1);
    assert!(shift.is_active());
    assert!(shift_visual.y_offset_pixels < -line_height * 0.9);

    let settled = registry.frame(
        &shifted_lines,
        line_height,
        now + Duration::from_millis(10) + TRANSCRIPT_MESSAGE_SHIFT_DURATION * 2,
    );
    let settled_visual = test_transcript_message_visual_for_line(&settled, &shifted_lines, 1);
    assert_eq!(settled_visual.y_offset_pixels, 0.0);
    assert_eq!(settled_visual.opacity, 1.0);
    assert!(!settled.is_active());
}

#[test]
fn transcript_message_runs_group_roles_and_skip_tool_chrome() {
    let lines = vec![
        SingleSessionStyledLine::new("1  hello", SingleSessionLineStyle::User),
        SingleSessionStyledLine::new("   again", SingleSessionLineStyle::UserContinuation),
        SingleSessionStyledLine::new("tool", SingleSessionLineStyle::Tool),
        SingleSessionStyledLine::new("answer", SingleSessionLineStyle::Assistant),
        SingleSessionStyledLine::new("meta", SingleSessionLineStyle::Meta),
    ];

    let runs = single_session_transcript_message_runs(&lines);
    assert_eq!(runs.len(), 3);
    assert_eq!(runs[0].line, 0);
    assert_eq!(runs[0].line_count, 2);
    assert_eq!(runs[0].role, TranscriptMessageRole::User);
    assert_eq!(runs[1].line, 3);
    assert_eq!(runs[1].role, TranscriptMessageRole::Assistant);
    assert_eq!(runs[2].line, 4);
    assert_eq!(runs[2].role, TranscriptMessageRole::Meta);
}

#[test]
fn reduced_motion_snaps_transcript_message_motion() {
    let _guard = crate::animation::DesktopReducedMotionEnvGuard::set(true);
    let mut registry = TranscriptMessageMotionRegistry::default();
    let now = Instant::now();
    let user = SingleSessionStyledLine::new("1  hello", SingleSessionLineStyle::User);
    let assistant = SingleSessionStyledLine::new("answer", SingleSessionLineStyle::Assistant);

    registry.frame(std::slice::from_ref(&user), 24.0, now);
    let lines = vec![user, assistant];
    let frame = registry.frame(&lines, 24.0, now + Duration::from_millis(5));
    let visual = test_transcript_message_visual_for_line(&frame, &lines, 1);

    assert_eq!(visual, TranscriptMessageVisual::default());
    assert!(!frame.is_active());
}

#[test]
fn transcript_card_motion_animates_new_card_entry() {
    let mut registry = TranscriptCardMotionRegistry::default();
    let now = Instant::now();
    let line_height = 28.0;
    let first = SingleSessionStyledLine::new("```rust", SingleSessionLineStyle::Code);
    let spacer = SingleSessionStyledLine::new("between", SingleSessionLineStyle::Assistant);
    let second = SingleSessionStyledLine::new("```text", SingleSessionLineStyle::Code);

    let initial = registry.frame(std::slice::from_ref(&first), line_height, now);
    let initial_visual =
        test_transcript_card_visual_for_line(&initial, std::slice::from_ref(&first), 0);
    assert_eq!(initial_visual.opacity, 1.0);
    assert!(!initial.is_active());

    let lines = vec![first.clone(), spacer, second];
    let entry = registry.frame(&lines, line_height, now + Duration::from_millis(5));
    let entry_visual = test_transcript_card_visual_for_line(&entry, &lines, 2);
    assert_eq!(entry_visual.opacity, 0.0);
    assert!(entry_visual.y_offset_pixels > 0.0);
    assert!(entry_visual.scale < 1.0);
    assert!(entry.is_active());

    let settled = registry.frame(
        &lines,
        line_height,
        now + Duration::from_millis(5) + TRANSCRIPT_CARD_ENTRY_DURATION * 2,
    );
    let settled_visual = test_transcript_card_visual_for_line(&settled, &lines, 2);
    assert_eq!(settled_visual.opacity, 1.0);
    assert_eq!(settled_visual.y_offset_pixels, 0.0);
    assert_eq!(settled_visual.scale, 1.0);
}

#[test]
fn transcript_card_motion_animates_layout_shift() {
    let mut registry = TranscriptCardMotionRegistry::default();
    let now = Instant::now();
    let line_height = 30.0;
    let code = SingleSessionStyledLine::new("```rust", SingleSessionLineStyle::Code);
    let intro = SingleSessionStyledLine::new("intro", SingleSessionLineStyle::Assistant);

    registry.frame(std::slice::from_ref(&code), line_height, now);
    let shifted_lines = vec![intro, code];
    let shift_start = registry.frame(&shifted_lines, line_height, now + Duration::from_millis(4));
    let shift_visual = test_transcript_card_visual_for_line(&shift_start, &shifted_lines, 1);
    assert!(shift_start.is_active());
    assert!(shift_visual.y_offset_pixels < -line_height * 0.9);

    let shift_middle = registry.frame(
        &shifted_lines,
        line_height,
        now + Duration::from_millis(4) + TRANSCRIPT_CARD_SHIFT_DURATION / 2,
    );
    let shift_middle_visual =
        test_transcript_card_visual_for_line(&shift_middle, &shifted_lines, 1);
    assert!(shift_middle_visual.y_offset_pixels < 0.0);
    assert!(shift_middle_visual.y_offset_pixels > -line_height);

    let settled = registry.frame(
        &shifted_lines,
        line_height,
        now + Duration::from_millis(4) + TRANSCRIPT_CARD_SHIFT_DURATION * 2,
    );
    let settled_visual = test_transcript_card_visual_for_line(&settled, &shifted_lines, 1);
    assert_eq!(settled_visual.y_offset_pixels, 0.0);
    assert!(!settled.is_active());
}

#[test]
fn transcript_card_motion_animates_card_exit() {
    let mut registry = TranscriptCardMotionRegistry::default();
    let now = Instant::now();
    let line_height = 28.0;
    let code = SingleSessionStyledLine::new("```rust", SingleSessionLineStyle::Code);

    registry.frame(std::slice::from_ref(&code), line_height, now);
    let exit_start = registry.frame(&[], line_height, now + Duration::from_millis(5));
    assert!(exit_start.is_active());
    assert_eq!(exit_start.exiting().len(), 1);
    assert_eq!(
        exit_start.exiting()[0].0.style,
        SingleSessionLineStyle::Code
    );
    assert_eq!(exit_start.exiting()[0].1.opacity, 1.0);

    let exit_middle = registry.frame(
        &[],
        line_height,
        now + Duration::from_millis(5) + TRANSCRIPT_CARD_EXIT_DURATION / 2,
    );
    let middle_visual = exit_middle.exiting()[0].1;
    assert!(exit_middle.is_active());
    assert!(middle_visual.opacity > 0.0 && middle_visual.opacity < 1.0);
    assert!(middle_visual.scale < 1.0);
    assert!(middle_visual.y_offset_pixels < 0.0);

    let settled = registry.frame(
        &[],
        line_height,
        now + Duration::from_millis(5) + TRANSCRIPT_CARD_EXIT_DURATION * 2,
    );
    assert!(!settled.is_active());
    assert!(settled.exiting().is_empty());
}

#[test]
fn inline_markdown_pill_motion_animates_entry_shift_and_exit() {
    let mut registry = InlineMarkdownPillMotionRegistry::default();
    let now = Instant::now();
    let line_height = 24.0;
    let first = SingleSessionStyledLine::with_inline_spans(
        "Use cargo",
        SingleSessionLineStyle::Assistant,
        vec![SingleSessionInlineSpan {
            start: 4,
            end: 9,
            kind: SingleSessionInlineSpanKind::Code,
        }],
    );
    let spacer = SingleSessionStyledLine::new("between", SingleSessionLineStyle::Assistant);
    let second = SingleSessionStyledLine::with_inline_spans(
        "Run test",
        SingleSessionLineStyle::Assistant,
        vec![SingleSessionInlineSpan {
            start: 4,
            end: 8,
            kind: SingleSessionInlineSpanKind::Code,
        }],
    );

    let initial = registry.frame(std::slice::from_ref(&first), line_height, now);
    let initial_visual = test_inline_markdown_pill_visual_for_line(
        &initial,
        std::slice::from_ref(&first),
        0,
        InlineMarkdownPillKind::Code,
    );
    assert_eq!(initial_visual, InlineMarkdownPillVisual::default());
    assert!(!initial.is_active());

    let lines = vec![first.clone(), spacer.clone(), second];
    let entry = registry.frame(&lines, line_height, now + Duration::from_millis(5));
    let entry_visual =
        test_inline_markdown_pill_visual_for_line(&entry, &lines, 2, InlineMarkdownPillKind::Code);
    assert!(entry.is_active());
    assert_eq!(entry_visual.opacity, 0.0);
    assert!(entry_visual.y_offset_pixels > 0.0);
    assert!(entry_visual.scale < 1.0);

    let shifted_lines = vec![spacer, first.clone()];
    let shift = registry.frame(&shifted_lines, line_height, now + Duration::from_millis(10));
    let shift_visual = test_inline_markdown_pill_visual_for_line(
        &shift,
        &shifted_lines,
        1,
        InlineMarkdownPillKind::Code,
    );
    assert!(shift.is_active());
    assert!(shift_visual.y_offset_pixels < -line_height * 0.9);

    let shift_settled = registry.frame(
        &shifted_lines,
        line_height,
        now + Duration::from_millis(10) + INLINE_MARKDOWN_PILL_SHIFT_DURATION * 2,
    );
    assert!(!shift_settled.is_active());

    let exit_at = now
        + Duration::from_millis(10)
        + INLINE_MARKDOWN_PILL_SHIFT_DURATION * 2
        + Duration::from_millis(5);
    let exit_start = registry.frame(&[], line_height, exit_at);
    assert!(exit_start.is_active());
    assert_eq!(exit_start.exiting().len(), 1);
    assert_eq!(exit_start.exiting()[0].0.kind, InlineMarkdownPillKind::Code);
    assert_eq!(exit_start.exiting()[0].1.opacity, 1.0);

    let settled = registry.frame(
        &[],
        line_height,
        exit_at + INLINE_MARKDOWN_PILL_EXIT_DURATION * 2,
    );
    assert!(!settled.is_active());
    assert!(settled.exiting().is_empty());
}

#[test]
fn reduced_motion_snaps_transcript_card_motion() {
    let _guard = crate::animation::DesktopReducedMotionEnvGuard::set(true);
    let mut registry = TranscriptCardMotionRegistry::default();
    let now = Instant::now();
    let line_height = 28.0;
    let first = SingleSessionStyledLine::new("```rust", SingleSessionLineStyle::Code);
    let second = SingleSessionStyledLine::new("```text", SingleSessionLineStyle::Code);
    let spacer = SingleSessionStyledLine::new("between", SingleSessionLineStyle::Assistant);

    registry.frame(std::slice::from_ref(&first), line_height, now);
    let lines = vec![first, spacer, second];
    let frame = registry.frame(&lines, line_height, now + Duration::from_millis(5));
    let visual = test_transcript_card_visual_for_line(&frame, &lines, 2);
    assert_eq!(visual, TranscriptCardVisual::default());
    assert!(!frame.is_active());
}

#[test]
fn tool_card_motion_animates_new_card_entry() {
    let mut registry = ToolCardMotionRegistry::default();
    let now = Instant::now();
    let first = test_tool_line(
        "call-a",
        SingleSessionToolVisualState::Succeeded,
        false,
        SingleSessionToolLineKind::Header,
    );
    let second = test_tool_line(
        "call-b",
        SingleSessionToolVisualState::Succeeded,
        false,
        SingleSessionToolLineKind::Header,
    );

    let frame = registry.frame(std::slice::from_ref(&first), now, 0);
    let first_visual = frame.visual_for("call-a").expect("first visual");
    assert_eq!(first_visual.opacity, 1.0);
    assert_eq!(first_visual.y_offset_pixels, 0.0);
    assert_eq!(first_visual.scale, 1.0);

    let lines = vec![first.clone(), second.clone()];
    let entry = registry.frame(&lines, now + Duration::from_millis(10), 0);
    let entry_visual = entry.visual_for("call-b").expect("entry visual");
    assert_eq!(entry_visual.opacity, 0.0);
    assert!(entry_visual.y_offset_pixels > 0.0);
    assert!(entry_visual.scale < 1.0);
    assert!(entry.is_active());

    let middle = registry.frame(
        &lines,
        now + Duration::from_millis(10) + TOOL_CARD_ENTRY_DURATION / 2,
        1,
    );
    let middle_visual = middle.visual_for("call-b").expect("middle visual");
    assert!(middle_visual.opacity > 0.0 && middle_visual.opacity < 1.0);
    assert!(middle_visual.y_offset_pixels > 0.0);

    let final_frame = registry.frame(
        &lines,
        now + Duration::from_millis(10) + TOOL_CARD_ENTRY_DURATION * 2,
        2,
    );
    let final_visual = final_frame.visual_for("call-b").expect("final visual");
    assert_eq!(final_visual.opacity, 1.0);
    assert_eq!(final_visual.y_offset_pixels, 0.0);
    assert_eq!(final_visual.scale, 1.0);
}

#[test]
fn tool_card_motion_animates_state_resolution() {
    let mut registry = ToolCardMotionRegistry::default();
    let now = Instant::now();
    let running = test_tool_line(
        "call-a",
        SingleSessionToolVisualState::Running,
        true,
        SingleSessionToolLineKind::Header,
    );
    let done = test_tool_line(
        "call-a",
        SingleSessionToolVisualState::Succeeded,
        false,
        SingleSessionToolLineKind::Header,
    );

    registry.frame(std::slice::from_ref(&running), now, 0);
    let start = registry.frame(
        std::slice::from_ref(&done),
        now + Duration::from_millis(5),
        0,
    );
    let start_visual = start.visual_for("call-a").expect("start visual");
    assert!(start.is_active());
    assert!(start_visual.flash_alpha > 0.0);
    assert!(colors_close(
        start_visual.rail,
        TOOL_TIMELINE_ACTIVE_RAIL_COLOR,
        0.26
    ));

    let final_frame = registry.frame(
        std::slice::from_ref(&done),
        now + Duration::from_millis(5)
            + TOOL_CARD_STATE_TRANSITION_DURATION
            + TOOL_CARD_RESOLUTION_FLASH_DURATION
            + Duration::from_millis(1),
        2,
    );
    let final_visual = final_frame.visual_for("call-a").expect("final visual");
    assert!(!final_frame.is_active());
    assert_eq!(final_visual.flash_alpha, 0.0);
    assert!(colors_close(
        final_visual.rail,
        single_session_tool_state_accent(SingleSessionToolVisualState::Succeeded),
        0.001,
    ));
}

#[test]
fn tool_card_motion_animates_output_drawer_reveal() {
    let mut registry = ToolCardMotionRegistry::default();
    let now = Instant::now();
    let header = test_tool_line(
        "call-a",
        SingleSessionToolVisualState::Succeeded,
        false,
        SingleSessionToolLineKind::Header,
    );
    let detail = test_tool_line(
        "call-a",
        SingleSessionToolVisualState::Succeeded,
        false,
        SingleSessionToolLineKind::Detail,
    );

    registry.frame(std::slice::from_ref(&header), now, 0);
    let expanded = vec![header.clone(), detail.clone()];
    let start = registry.frame(&expanded, now + Duration::from_millis(7), 0);
    let start_visual = start.visual_for("call-a").expect("start visual");
    assert_eq!(start_visual.output_reveal, 0.0);
    assert!(start.is_active());

    let middle = registry.frame(
        &expanded,
        now + Duration::from_millis(7) + TOOL_CARD_OUTPUT_REVEAL_DURATION / 2,
        1,
    );
    let middle_visual = middle.visual_for("call-a").expect("middle visual");
    assert!(middle_visual.output_reveal > 0.0 && middle_visual.output_reveal < 1.0);

    let final_frame = registry.frame(
        &expanded,
        now + Duration::from_millis(7) + TOOL_CARD_OUTPUT_REVEAL_DURATION * 2,
        2,
    );
    let final_visual = final_frame.visual_for("call-a").expect("final visual");
    assert_eq!(final_visual.output_reveal, 1.0);
    assert!(!final_frame.is_active());
}

#[test]
fn tool_card_motion_animates_group_summary_replacement() {
    let mut registry = ToolCardMotionRegistry::default();
    let now = Instant::now();
    let first = test_tool_line(
        "call-a",
        SingleSessionToolVisualState::Succeeded,
        false,
        SingleSessionToolLineKind::Header,
    );
    let second = test_tool_line(
        "call-b",
        SingleSessionToolVisualState::Succeeded,
        false,
        SingleSessionToolLineKind::Header,
    );
    let group = test_tool_line(
        "tool-group",
        SingleSessionToolVisualState::Group,
        false,
        SingleSessionToolLineKind::GroupSummary,
    );

    registry.frame(&[first, second], now, 0);
    let replaced = registry.frame(
        std::slice::from_ref(&group),
        now + Duration::from_millis(8),
        1,
    );
    assert!(replaced.is_active());
    assert_eq!(replaced.exiting().len(), 2);
    assert_eq!(
        replaced
            .visual_for("tool-group")
            .expect("group visual")
            .opacity,
        0.0
    );
    assert!(
        replaced
            .exiting()
            .iter()
            .all(|(_, visual)| visual.opacity > 0.0 && visual.scale <= 1.0)
    );

    let settled = registry.frame(
        std::slice::from_ref(&group),
        now + Duration::from_millis(8) + TOOL_CARD_ENTRY_DURATION * 2,
        2,
    );
    assert!(settled.exiting().is_empty());
    assert_eq!(
        settled
            .visual_for("tool-group")
            .expect("group visual")
            .opacity,
        1.0
    );
}

#[test]
fn reduced_motion_snaps_tool_card_entry_state_and_grouping() {
    let _guard = crate::animation::DesktopReducedMotionEnvGuard::set(true);
    let mut registry = ToolCardMotionRegistry::default();
    let now = Instant::now();
    let first = test_tool_line(
        "call-a",
        SingleSessionToolVisualState::Running,
        true,
        SingleSessionToolLineKind::Header,
    );
    let second = test_tool_line(
        "call-b",
        SingleSessionToolVisualState::Succeeded,
        false,
        SingleSessionToolLineKind::Header,
    );
    let done = test_tool_line(
        "call-a",
        SingleSessionToolVisualState::Succeeded,
        false,
        SingleSessionToolLineKind::Header,
    );
    let group = test_tool_line(
        "tool-group",
        SingleSessionToolVisualState::Group,
        false,
        SingleSessionToolLineKind::GroupSummary,
    );

    let initial = registry.frame(std::slice::from_ref(&first), now, 9);
    let initial_visual = initial.visual_for("call-a").expect("initial visual");
    assert_eq!(initial_visual.opacity, 1.0);
    assert_eq!(initial_visual.active_phase, 0.0);
    assert!(!initial.is_active());

    let added = registry.frame(&[done.clone(), second], now + Duration::from_millis(5), 10);
    let done_visual = added.visual_for("call-a").expect("done visual");
    let second_visual = added.visual_for("call-b").expect("second visual");
    assert_eq!(done_visual.flash_alpha, 0.0);
    assert_eq!(second_visual.opacity, 1.0);
    assert_eq!(second_visual.y_offset_pixels, 0.0);
    assert_eq!(second_visual.scale, 1.0);
    assert!(!added.is_active());

    let grouped = registry.frame(
        std::slice::from_ref(&group),
        now + Duration::from_millis(10),
        11,
    );
    assert!(grouped.exiting().is_empty());
    assert_eq!(
        grouped
            .visual_for("tool-group")
            .expect("group visual")
            .opacity,
        1.0
    );
    assert!(!grouped.is_active());
}

#[test]
fn scrollbar_motion_animates_thumb_position() {
    let mut registry = SingleSessionScrollbarMotionRegistry::default();
    let size = PhysicalSize::new(900, 720);
    let now = Instant::now();
    let top = test_scroll_metrics(120, 30, 0.0, 90);
    let bottom = test_scroll_metrics(120, 30, 90.0, 90);

    let first = registry.frame_for_metrics(size, 0.0, Some(top), now);
    let first_visual = first.visual().expect("initial visual");
    assert_eq!(first_visual.opacity, 1.0);
    assert_eq!(
        first_visual.thumb_y,
        single_session_scrollbar_geometry(size, 0.0, top).thumb_y
    );

    let start = registry.frame_for_metrics(size, 0.0, Some(bottom), now + Duration::from_millis(5));
    let start_visual = start.visual().expect("start visual");
    assert!(start.is_active());
    assert_eq!(start_visual.thumb_y, first_visual.thumb_y);

    let middle = registry.frame_for_metrics(
        size,
        0.0,
        Some(bottom),
        now + Duration::from_millis(5) + SINGLE_SESSION_SCROLLBAR_THUMB_TRANSITION_DURATION / 2,
    );
    let middle_visual = middle.visual().expect("middle visual");
    let target_y = single_session_scrollbar_geometry(size, 0.0, bottom).thumb_y;
    assert!(middle_visual.thumb_y < first_visual.thumb_y);
    assert!(middle_visual.thumb_y > target_y);

    let settled = registry.frame_for_metrics(
        size,
        0.0,
        Some(bottom),
        now + Duration::from_millis(5) + SINGLE_SESSION_SCROLLBAR_THUMB_TRANSITION_DURATION * 2,
    );
    let settled_visual = settled.visual().expect("settled visual");
    assert_eq!(settled_visual.thumb_y, target_y);
}

#[test]
fn scrollbar_motion_fades_after_idle() {
    let mut registry = SingleSessionScrollbarMotionRegistry::default();
    let size = PhysicalSize::new(900, 720);
    let now = Instant::now();
    let metrics = test_scroll_metrics(120, 30, 0.0, 90);

    let initial = registry.frame_for_metrics(size, 0.0, Some(metrics), now);
    assert_eq!(initial.visual().expect("initial visual").opacity, 1.0);

    let fading = registry.frame_for_metrics(
        size,
        0.0,
        Some(metrics),
        now + SINGLE_SESSION_SCROLLBAR_FADE_IDLE_DURATION
            + SINGLE_SESSION_SCROLLBAR_FADE_DURATION / 2,
    );
    let fading_visual = fading.visual().expect("fading visual");
    assert!(fading.is_active());
    assert!(fading_visual.opacity > 0.0 && fading_visual.opacity < 1.0);

    let faded = registry.frame_for_metrics(
        size,
        0.0,
        Some(metrics),
        now + SINGLE_SESSION_SCROLLBAR_FADE_IDLE_DURATION
            + SINGLE_SESSION_SCROLLBAR_FADE_DURATION * 2,
    );
    assert!(faded.visual().is_none());
    assert!(!faded.is_active());
}

#[test]
fn scrollbar_motion_clears_when_not_scrollable() {
    let mut registry = SingleSessionScrollbarMotionRegistry::default();
    let size = PhysicalSize::new(900, 720);
    let now = Instant::now();
    let metrics = test_scroll_metrics(120, 30, 0.0, 90);

    assert!(
        registry
            .frame_for_metrics(size, 0.0, Some(metrics), now)
            .visual()
            .is_some()
    );
    let cleared = registry.frame_for_metrics(size, 0.0, None, now + Duration::from_millis(16));
    assert!(cleared.visual().is_none());
    assert!(!cleared.is_active());
}

#[test]
fn reduced_motion_snaps_scrollbar_and_welcome_reveal() {
    let _guard = crate::animation::DesktopReducedMotionEnvGuard::set(true);
    let mut registry = SingleSessionScrollbarMotionRegistry::default();
    let size = PhysicalSize::new(900, 720);
    let now = Instant::now();
    let top = test_scroll_metrics(120, 30, 0.0, 90);
    let bottom = test_scroll_metrics(120, 30, 90.0, 90);

    registry.frame_for_metrics(size, 0.0, Some(top), now);
    let snapped =
        registry.frame_for_metrics(size, 0.0, Some(bottom), now + Duration::from_millis(5));
    let snapped_visual = snapped.visual().expect("snapped visual");
    assert_eq!(
        snapped_visual.thumb_y,
        single_session_scrollbar_geometry(size, 0.0, bottom).thumb_y
    );
    assert_eq!(snapped_visual.opacity, 1.0);
    assert!(!snapped.is_active());

    let hidden = registry.frame_for_metrics(
        size,
        0.0,
        Some(bottom),
        now + Duration::from_millis(5)
            + SINGLE_SESSION_SCROLLBAR_FADE_IDLE_DURATION
            + Duration::from_millis(1),
    );
    assert!(hidden.visual().is_none());
    assert!(!hidden.is_active());

    assert_eq!(
        welcome_hero_reveal_progress_for_elapsed(Duration::ZERO),
        1.0
    );
    assert!(!welcome_hero_reveal_is_active(
        welcome_hero_reveal_progress_for_elapsed(Duration::ZERO)
    ));
}

fn test_scroll_metrics(
    total_lines: usize,
    visible_lines: usize,
    scroll_lines: f32,
    max_scroll_lines: usize,
) -> SingleSessionBodyScrollMetrics {
    SingleSessionBodyScrollMetrics {
        total_lines,
        visible_lines,
        scroll_lines,
        max_scroll_lines,
    }
}

fn colors_close(left: [f32; 4], right: [f32; 4], tolerance: f32) -> bool {
    left.iter()
        .zip(right.iter())
        .all(|(left, right)| (left - right).abs() <= tolerance)
}

#[test]
fn session_switcher_text_buffer_shapes_loaded_session_rows() {
    let size = PhysicalSize::new(1920, 2048);
    let mut app = SingleSessionApp::new(None);

    assert_eq!(
        app.handle_key(KeyInput::OpenSessionSwitcher),
        KeyOutcome::LoadSessionSwitcher
    );
    app.apply_session_switcher_cards(vec![SessionCard {
        session_id: "session_visible".to_string(),
        title: "visible resume row".to_string(),
        subtitle: "active · test-model".to_string(),
        detail: "3 msgs · just now · jcode".to_string(),
        preview_lines: vec!["user hello from resume picker".to_string()],
        detail_lines: vec!["user hello from resume picker".to_string()],
        transcript_messages: Vec::new(),
    }]);
    assert!(
        app.inline_widget_styled_lines()
            .iter()
            .any(|line| line.text.contains("visible resume row")),
        "state-level switcher lines should contain the session row"
    );

    let mut font_system = FontSystem::new();
    let buffers = single_session_text_buffers(&app, size, &mut font_system);
    let rendered_inline_text = buffers
        .get(4)
        .expect("inline widget buffer should be present")
        .layout_runs()
        .map(|run| run.text.to_string())
        .collect::<Vec<_>>()
        .join("\n");

    assert!(
        rendered_inline_text.contains("visible resume row"),
        "desktop text buffer should shape session rows, got:\n{rendered_inline_text}"
    );

    let rendered_preview_text = buffers
        .get(7)
        .expect("split preview buffer should be present")
        .layout_runs()
        .map(|run| run.text.to_string())
        .collect::<Vec<_>>()
        .join("\n");
    assert!(
        rendered_preview_text.contains("Preview"),
        "split preview buffer should shape preview rows, got:\n{rendered_preview_text}"
    );
    assert!(
        rendered_preview_text.contains("hello from resume picker"),
        "split preview buffer should contain preview content, got:\n{rendered_preview_text}"
    );

    let areas = single_session_text_areas_for_app(&app, &buffers, size);
    let inline_area = areas
        .iter()
        .find(|area| std::ptr::eq(area.buffer, &buffers[4]))
        .expect("primary inline widget text area");
    let preview_area = areas
        .iter()
        .find(|area| std::ptr::eq(area.buffer, &buffers[7]))
        .expect("split preview text area");
    let preview_start_line = inline_widget_split_preview_start(
        app.render_inline_widget_kind(),
        &app.render_inline_widget_styled_lines(),
    )
    .expect("session switcher preview start line");
    let typography = single_session_typography_for_scale(app.text_scale());
    let expected_preview_top = inline_area.top
        + preview_start_line as f32
            * inline_widget_line_height(app.render_inline_widget_kind(), &typography);
    assert!(
        (preview_area.top - expected_preview_top).abs() <= 1.0,
        "compact preview buffer should be positioned at its visual row offset: inline_top={}, preview_top={}, expected={}",
        inline_area.top,
        preview_area.top,
        expected_preview_top
    );
    assert!(
        (preview_area.top - preview_area.bounds.top as f32).abs() <= 1.0,
        "compact preview buffer should not rely on clipped leading blank rows: top={}, bounds_top={}",
        preview_area.top,
        preview_area.bounds.top
    );
}

#[test]
fn session_switcher_fallback_rail_width_does_not_panic_on_narrow_cards() {
    // Regression: narrow cards made `card_width * 0.55` drop below the 220px
    // preferred minimum, so the old `clamp(220.0, card_width * 0.55)` panicked
    // with `min > max`, crashing the desktop app on resume into a small window.
    for card_width in [0.0_f32, 1.0, 50.0, 205.0, 220.0, 399.0, 400.0, 1200.0] {
        let rail = session_switcher_fallback_rail_width(card_width);
        assert!(
            rail.is_finite() && rail >= 0.0,
            "rail width must be finite and non-negative for card_width={card_width}, got {rail}"
        );
        // The rail should never exceed the available card width.
        assert!(
            rail <= card_width.max(1.0),
            "rail width {rail} should not exceed card_width {card_width}"
        );
    }
}
