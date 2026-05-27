use super::*;
use crate::desktop_rich_text::{
    AnsiColor, AnsiStyle, RichLine, RichLineStyle, RichSpanStyle, SyntaxTokenKind,
};
use crate::single_session::{
    InlineWidgetKind, SingleSessionInlineSpan, SingleSessionInlineSpanKind,
    SingleSessionToolLineKind, SingleSessionToolLineMetadata, SingleSessionToolVisualState,
    SingleSessionTypography, single_session_assistant_font_family,
    single_session_trimmed_line_end_preserving_inline_code_whitespace,
    single_session_user_font_family,
};

mod handwriting;

use handwriting::handwritten_welcome_paths_for_phrase;
use std::collections::{HashMap, hash_map::DefaultHasher};
use std::hash::{Hash, Hasher};

pub(crate) const INLINE_MATH_BACKGROUND_COLOR: [f32; 4] = [0.035, 0.220, 0.155, 0.115];
pub(crate) const MARKDOWN_HEADING_BACKGROUND_COLOR: [f32; 4] = [0.060, 0.180, 0.520, 0.055];
pub(crate) const MARKDOWN_MEDIA_BACKGROUND_COLOR: [f32; 4] = [0.030, 0.255, 0.185, 0.070];
pub(crate) const MARKDOWN_RULE_COLOR: [f32; 4] = [0.060, 0.130, 0.260, 0.220];
pub(crate) const MARKDOWN_LIST_MARKER_COLOR: [f32; 4] = [0.060, 0.110, 0.240, 0.960];
pub(crate) const MARKDOWN_TASK_DONE_COLOR: [f32; 4] = [0.025, 0.350, 0.190, 1.000];
pub(crate) const MARKDOWN_TASK_OPEN_COLOR: [f32; 4] = [0.420, 0.320, 0.075, 0.980];
pub(crate) const MARKDOWN_STRIKE_TEXT_COLOR: [f32; 4] = [0.310, 0.330, 0.380, 0.880];
pub(crate) const STREAMING_ACTIVITY_PILL_COLOR: [f32; 4] = [0.965, 0.985, 1.000, 0.58];
pub(crate) const STREAMING_ACTIVITY_PILL_BORDER_COLOR: [f32; 4] = [0.000, 0.260, 0.720, 0.18];
const STREAMING_ACTIVITY_CUE_ENTRY_DURATION: Duration = Duration::from_millis(145);
const STREAMING_ACTIVITY_CUE_EXIT_DURATION: Duration = Duration::from_millis(130);
const STREAMING_ACTIVITY_CUE_ENTRY_OFFSET_PIXELS: f32 = 7.0;
const STREAMING_ACTIVITY_CUE_ENTRY_SCALE: f32 = 0.94;
const INLINE_WIDGET_CARD_SHADOW_COLOR: [f32; 4] = [0.020, 0.035, 0.070, 0.080];
pub(crate) const INLINE_WIDGET_CARD_BACKGROUND_COLOR: [f32; 4] = [0.992, 0.996, 1.000, 0.72];
const INLINE_WIDGET_CARD_BORDER_COLOR: [f32; 4] = [0.105, 0.185, 0.360, 0.20];
const INLINE_WIDGET_CARD_HIGHLIGHT_COLOR: [f32; 4] = [1.000, 1.000, 1.000, 0.52];
const INLINE_WIDGET_CARD_ACCENT_COLOR: [f32; 4] = [0.125, 0.420, 0.920, 0.34];
pub(crate) const SLASH_SUGGESTIONS_INLINE_CARD_BACKGROUND_COLOR: [f32; 4] =
    [0.948, 0.966, 1.000, 0.90];
const SLASH_SUGGESTIONS_INLINE_CARD_BORDER_COLOR: [f32; 4] = [0.090, 0.230, 0.620, 0.32];
const SLASH_SUGGESTIONS_INLINE_CARD_HIGHLIGHT_COLOR: [f32; 4] = [1.000, 1.000, 1.000, 0.62];
const SLASH_SUGGESTIONS_INLINE_CARD_ACCENT_COLOR: [f32; 4] = [0.105, 0.355, 0.950, 0.48];
pub(crate) const SLASH_SUGGESTIONS_INLINE_SELECTION_BACKGROUND_COLOR: [f32; 4] =
    [0.215, 0.420, 0.900, 0.155];
const SINGLE_SESSION_SCROLLBAR_TRACK_WIDTH: f32 = 3.0;
const SINGLE_SESSION_SCROLLBAR_GAP: f32 = 8.0;
const SINGLE_SESSION_SCROLLBAR_THUMB_TRANSITION_DURATION: Duration = Duration::from_millis(140);
const SINGLE_SESSION_SCROLLBAR_FADE_IDLE_DURATION: Duration = Duration::from_millis(620);
const SINGLE_SESSION_SCROLLBAR_FADE_DURATION: Duration = Duration::from_millis(260);
const SINGLE_SESSION_SCROLLBAR_TRACK_COLOR: [f32; 4] = [0.040, 0.055, 0.090, 0.075];
const SINGLE_SESSION_SCROLLBAR_THUMB_COLOR: [f32; 4] = [0.035, 0.065, 0.145, 0.34];
const TRANSCRIPT_CARD_ENTRY_DURATION: Duration = Duration::from_millis(170);
const TRANSCRIPT_CARD_SHIFT_DURATION: Duration = Duration::from_millis(150);
const TRANSCRIPT_CARD_EXIT_DURATION: Duration = Duration::from_millis(145);
const TRANSCRIPT_CARD_ENTRY_OFFSET_PIXELS: f32 = 10.0;
const TRANSCRIPT_CARD_ENTRY_SCALE: f32 = 0.988;
const TRANSCRIPT_MESSAGE_ENTRY_DURATION: Duration = Duration::from_millis(150);
const TRANSCRIPT_MESSAGE_SHIFT_DURATION: Duration = Duration::from_millis(135);
const TRANSCRIPT_MESSAGE_ENTRY_OFFSET_PIXELS: f32 = 7.0;
const TRANSCRIPT_MESSAGE_ENTRY_SCALE: f32 = 0.992;
const TRANSCRIPT_MESSAGE_ASSISTANT_HIGHLIGHT_COLOR: [f32; 4] = [0.070, 0.125, 0.260, 0.038];
const TRANSCRIPT_MESSAGE_USER_HIGHLIGHT_COLOR: [f32; 4] = [0.060, 0.210, 0.650, 0.058];
const TRANSCRIPT_MESSAGE_META_HIGHLIGHT_COLOR: [f32; 4] = [0.075, 0.160, 0.260, 0.046];
const TRANSCRIPT_MESSAGE_ERROR_HIGHLIGHT_COLOR: [f32; 4] = [0.700, 0.080, 0.100, 0.060];
const TRANSCRIPT_MESSAGE_ACCENT_ALPHA_MULTIPLIER: f32 = 2.8;
const INLINE_MARKDOWN_PILL_ENTRY_DURATION: Duration = Duration::from_millis(145);
const INLINE_MARKDOWN_PILL_SHIFT_DURATION: Duration = Duration::from_millis(130);
const INLINE_MARKDOWN_PILL_EXIT_DURATION: Duration = Duration::from_millis(125);
const INLINE_MARKDOWN_PILL_ENTRY_OFFSET_PIXELS: f32 = 4.0;
const INLINE_MARKDOWN_PILL_ENTRY_SCALE: f32 = 0.94;
const INLINE_WIDGET_SELECTION_TRANSITION_DURATION: Duration = Duration::from_millis(135);
const INLINE_WIDGET_PREVIEW_PANE_FOCUS_DURATION: Duration = Duration::from_millis(150);
const INLINE_WIDGET_PREVIEW_PANE_CONTENT_DURATION: Duration = Duration::from_millis(145);
pub(crate) const INLINE_WIDGET_PREVIEW_PANE_BACKGROUND_COLOR: [f32; 4] =
    [0.968, 0.984, 1.000, 0.430];
const INLINE_WIDGET_PREVIEW_PANE_BORDER_COLOR: [f32; 4] = [0.090, 0.205, 0.480, 0.180];
pub(crate) const INLINE_WIDGET_PREVIEW_PANE_FOCUS_COLOR: [f32; 4] = [0.100, 0.340, 0.920, 0.180];
const INLINE_WIDGET_PREVIEW_PANE_CONTENT_COLOR: [f32; 4] = [0.125, 0.420, 0.920, 0.105];
const INLINE_WIDGET_LIST_REFLOW_ENTRY_DURATION: Duration = Duration::from_millis(145);
const INLINE_WIDGET_LIST_REFLOW_SHIFT_DURATION: Duration = Duration::from_millis(145);
const INLINE_WIDGET_LIST_REFLOW_EXIT_DURATION: Duration = Duration::from_millis(120);
const INLINE_WIDGET_LIST_REFLOW_COLOR: [f32; 4] = [0.105, 0.355, 0.950, 0.110];
const COMPOSER_MOTION_DURATION: Duration = Duration::from_millis(165);
pub(crate) const COMPOSER_CARD_BACKGROUND_COLOR: [f32; 4] = [0.990, 0.994, 1.000, 0.420];
pub(crate) const COMPOSER_FOCUS_RING_COLOR: [f32; 4] = [0.090, 0.250, 0.680, 0.185];
pub(crate) const COMPOSER_PLACEHOLDER_RAIL_COLOR: [f32; 4] = [0.105, 0.185, 0.360, 0.185];
pub(crate) const COMPOSER_SUBMIT_READY_COLOR: [f32; 4] = [0.105, 0.355, 0.950, 0.700];
pub(crate) const COMPOSER_SUBMIT_BUSY_COLOR: [f32; 4] = [0.055, 0.540, 0.360, 0.700];
const ATTACHMENT_CHIP_ENTRY_DURATION: Duration = Duration::from_millis(150);
const ATTACHMENT_CHIP_SHIFT_DURATION: Duration = Duration::from_millis(140);
const ATTACHMENT_CHIP_EXIT_DURATION: Duration = Duration::from_millis(130);
const ATTACHMENT_CHIP_WIDTH: f32 = 42.0;
const ATTACHMENT_CHIP_HEIGHT: f32 = 20.0;
const ATTACHMENT_CHIP_GAP: f32 = 6.0;
const ATTACHMENT_CHIP_VISIBLE_LIMIT: usize = 4;
pub(crate) const ATTACHMENT_CHIP_BACKGROUND_COLOR: [f32; 4] = [0.940, 0.972, 1.000, 0.720];
pub(crate) const ATTACHMENT_CHIP_ACCENT_COLOR: [f32; 4] = [0.090, 0.355, 0.900, 0.620];
pub(crate) const ATTACHMENT_CHIP_EXIT_COLOR: [f32; 4] = [0.530, 0.590, 0.690, 0.430];
const STDIN_OVERLAY_ENTRY_DURATION: Duration = Duration::from_millis(165);
const STDIN_OVERLAY_RESIZE_DURATION: Duration = Duration::from_millis(155);
const STDIN_OVERLAY_EXIT_DURATION: Duration = Duration::from_millis(145);
const STDIN_OVERLAY_ENTRY_OFFSET_PIXELS: f32 = 9.0;
const STDIN_OVERLAY_ENTRY_SCALE: f32 = 0.985;
pub(crate) const STDIN_OVERLAY_BACKGROUND_COLOR: [f32; 4] = [0.966, 0.982, 1.000, 0.640];
pub(crate) const STDIN_OVERLAY_BORDER_COLOR: [f32; 4] = [0.085, 0.270, 0.760, 0.250];
pub(crate) const STDIN_OVERLAY_INPUT_RAIL_COLOR: [f32; 4] = [0.115, 0.410, 0.940, 0.300];
pub(crate) const STDIN_OVERLAY_SUBMIT_COLOR: [f32; 4] = [0.060, 0.500, 0.340, 0.660];
pub(crate) const STDIN_OVERLAY_EXIT_COLOR: [f32; 4] = [0.500, 0.570, 0.680, 0.420];
const TOOL_CARD_ENTRY_DURATION: Duration = Duration::from_millis(180);
const TOOL_CARD_EXIT_DURATION: Duration = Duration::from_millis(160);
const TOOL_CARD_STATE_TRANSITION_DURATION: Duration = Duration::from_millis(160);
const TOOL_CARD_OUTPUT_REVEAL_DURATION: Duration = Duration::from_millis(180);
const TOOL_CARD_RESOLUTION_FLASH_DURATION: Duration = Duration::from_millis(320);
const TOOL_CARD_ENTRY_OFFSET_PIXELS: f32 = 12.0;
const TOOL_CARD_ENTRY_SCALE: f32 = 0.985;

#[derive(Clone, Debug, Eq, PartialEq)]
pub(crate) struct SingleSessionTextKey {
    pub(crate) size: (u32, u32),
    pub(crate) fresh_welcome_visible: bool,
    pub(crate) title: String,
    pub(crate) version: String,
    pub(crate) welcome_hero: String,
    pub(crate) welcome_hint: Vec<SingleSessionStyledLine>,
    pub(crate) activity_active: bool,
    pub(crate) welcome_handoff_visible: bool,
    pub(crate) text_scale_bits: u32,
    pub(crate) body_top_offset_pixels_bits: u32,
    pub(crate) user_font_family: &'static str,
    pub(crate) assistant_font_family: &'static str,
    pub(crate) body: Vec<SingleSessionStyledLine>,
    pub(crate) inline_widget_kind: Option<InlineWidgetKind>,
    pub(crate) inline_widget: Vec<SingleSessionStyledLine>,
    pub(crate) inline_widget_preview: Vec<SingleSessionStyledLine>,
    pub(crate) draft: String,
}

#[derive(Clone, Copy, Debug)]
pub(crate) struct WelcomeHeroStrokeSegment {
    pub(crate) start: [f32; 2],
    pub(crate) end: [f32; 2],
    pub(crate) start_progress: f32,
    pub(crate) end_progress: f32,
}

#[derive(Clone, Debug)]
pub(crate) struct WelcomeHeroRuntimeMaskSpec {
    pub(crate) phrase: String,
    pub(crate) rect: Rect,
    pub(crate) font_size: f32,
}

#[cfg(test)]
pub(crate) fn build_single_session_vertices(
    app: &SingleSessionApp,
    size: PhysicalSize<u32>,
    focus_pulse: f32,
    spinner_tick: u64,
) -> Vec<Vertex> {
    build_single_session_vertices_with_scroll(app, size, focus_pulse, spinner_tick, 0.0)
}

#[cfg(test)]
pub(crate) fn build_single_session_vertices_with_scroll(
    app: &SingleSessionApp,
    size: PhysicalSize<u32>,
    focus_pulse: f32,
    spinner_tick: u64,
    smooth_scroll_lines: f32,
) -> Vec<Vertex> {
    let welcome_hero_reveal_progress = welcome_hero_reveal_progress_for_tick(spinner_tick);
    build_single_session_vertices_with_scroll_and_reveal(
        app,
        size,
        focus_pulse,
        spinner_tick,
        smooth_scroll_lines,
        welcome_hero_reveal_progress,
    )
}

pub(crate) fn build_single_session_vertices_with_scroll_and_reveal(
    app: &SingleSessionApp,
    size: PhysicalSize<u32>,
    focus_pulse: f32,
    spinner_tick: u64,
    smooth_scroll_lines: f32,
    welcome_hero_reveal_progress: f32,
) -> Vec<Vertex> {
    let width = size.width as f32;
    let height = size.height as f32;
    let mut vertices = Vec::new();
    let rendered_body_lines = single_session_rendered_body_lines_for_tick(app, size, spinner_tick);

    push_gradient_rect(
        &mut vertices,
        Rect {
            x: 0.0,
            y: 0.0,
            width,
            height,
        },
        BACKGROUND_TOP_LEFT,
        BACKGROUND_BOTTOM_LEFT,
        BACKGROUND_BOTTOM_RIGHT,
        BACKGROUND_TOP_RIGHT,
        size,
    );

    let rect = Rect {
        x: 0.0,
        y: 0.0,
        width: width.max(1.0),
        height: height.max(1.0),
    };
    let surface = single_session_surface(app.session.as_ref());
    push_single_session_surface_without_bottom_rule(
        &mut vertices,
        rect,
        surface.color_index,
        focus_pulse,
        size,
    );

    let layout = single_session_layout_for_total_lines(app, size, rendered_body_lines.len());
    push_single_session_composer_chrome(&mut vertices, app, size, None, None, Some(layout));

    let welcome_chrome_offset = if app.is_welcome_timeline_visible() {
        welcome_timeline_visual_offset_pixels_for_total_lines(
            app,
            size,
            smooth_scroll_lines,
            rendered_body_lines.len(),
        )
    } else {
        0.0
    };
    if welcome_timeline_chrome_visible(app, size, welcome_chrome_offset) {
        push_fresh_welcome_ambient(&mut vertices, size, spinner_tick, welcome_chrome_offset);
        push_handwritten_welcome_hero_with_offset(
            &mut vertices,
            &app.welcome_hero_text(),
            size,
            app.text_scale(),
            welcome_hero_reveal_progress,
            welcome_chrome_offset,
        );
    }

    push_single_session_inline_widget_card(
        &mut vertices,
        app,
        size,
        welcome_chrome_offset,
        rendered_body_lines.len(),
        None,
        None,
        None,
    );
    push_single_session_stdin_overlay(&mut vertices, app, size, &rendered_body_lines, None);
    let viewport = single_session_body_viewport_from_lines(
        app,
        size,
        smooth_scroll_lines,
        &rendered_body_lines,
    );
    push_single_session_transcript_message_highlights_from_viewport(
        &mut vertices,
        app,
        size,
        &viewport,
        rendered_body_lines.len(),
        None,
    );
    push_single_session_transcript_cards(
        &mut vertices,
        app,
        size,
        spinner_tick,
        smooth_scroll_lines,
    );
    push_single_session_tool_cards(
        &mut vertices,
        app,
        size,
        spinner_tick,
        smooth_scroll_lines,
        None,
    );
    push_single_session_inline_code_cards(
        &mut vertices,
        app,
        size,
        spinner_tick,
        smooth_scroll_lines,
    );
    push_single_session_markdown_rule_lines(
        &mut vertices,
        app,
        size,
        spinner_tick,
        smooth_scroll_lines,
    );
    if app.has_activity_indicator() {
        push_streaming_activity_cue(&mut vertices, app, size, spinner_tick, None, None);
    }
    push_single_session_selection(&mut vertices, app, size);
    push_single_session_scrollbar(
        &mut vertices,
        app,
        size,
        spinner_tick,
        smooth_scroll_lines,
        None,
    );

    vertices
}

pub(crate) fn build_single_session_vertices_with_cached_body(
    app: &SingleSessionApp,
    size: PhysicalSize<u32>,
    focus_pulse: f32,
    spinner_tick: u64,
    smooth_scroll_lines: f32,
    welcome_hero_reveal_progress: f32,
    rendered_body_lines: &[SingleSessionStyledLine],
) -> Vec<Vertex> {
    build_single_session_vertices_with_cached_body_internal(
        app,
        size,
        focus_pulse,
        spinner_tick,
        smooth_scroll_lines,
        welcome_hero_reveal_progress,
        rendered_body_lines,
        None,
        None,
        None,
        None,
        None,
        None,
        None,
        None,
        None,
        None,
        None,
        None,
    )
}

#[allow(clippy::too_many_arguments)]
pub(crate) fn build_single_session_vertices_with_cached_body_and_tool_motion(
    app: &SingleSessionApp,
    size: PhysicalSize<u32>,
    focus_pulse: f32,
    spinner_tick: u64,
    smooth_scroll_lines: f32,
    welcome_hero_reveal_progress: f32,
    rendered_body_lines: &[SingleSessionStyledLine],
    inline_selection_motion: Option<&InlineWidgetSelectionMotionFrame>,
    inline_list_reflow_motion: Option<&InlineWidgetListReflowMotionFrame>,
    inline_preview_pane_motion: Option<&InlineWidgetPreviewPaneMotionFrame>,
    composer_motion: Option<&ComposerMotionFrame>,
    attachment_chip_motion: Option<&AttachmentChipMotionFrame>,
    stdin_overlay_motion: Option<&StdinOverlayMotionFrame>,
    transcript_message_motion: Option<&TranscriptMessageMotionFrame>,
    transcript_motion: Option<&TranscriptCardMotionFrame>,
    inline_markdown_motion: Option<&InlineMarkdownPillMotionFrame>,
    activity_cue_motion: Option<&StreamingActivityCueMotionFrame>,
    tool_motion: &ToolCardMotionFrame,
    scrollbar_motion: Option<&SingleSessionScrollbarMotionFrame>,
) -> Vec<Vertex> {
    build_single_session_vertices_with_cached_body_internal(
        app,
        size,
        focus_pulse,
        spinner_tick,
        smooth_scroll_lines,
        welcome_hero_reveal_progress,
        rendered_body_lines,
        inline_selection_motion,
        inline_list_reflow_motion,
        inline_preview_pane_motion,
        composer_motion,
        attachment_chip_motion,
        stdin_overlay_motion,
        transcript_message_motion,
        transcript_motion,
        inline_markdown_motion,
        activity_cue_motion,
        Some(tool_motion),
        scrollbar_motion,
    )
}

#[allow(clippy::too_many_arguments)]
fn build_single_session_vertices_with_cached_body_internal(
    app: &SingleSessionApp,
    size: PhysicalSize<u32>,
    focus_pulse: f32,
    spinner_tick: u64,
    smooth_scroll_lines: f32,
    welcome_hero_reveal_progress: f32,
    rendered_body_lines: &[SingleSessionStyledLine],
    inline_selection_motion: Option<&InlineWidgetSelectionMotionFrame>,
    inline_list_reflow_motion: Option<&InlineWidgetListReflowMotionFrame>,
    inline_preview_pane_motion: Option<&InlineWidgetPreviewPaneMotionFrame>,
    composer_motion: Option<&ComposerMotionFrame>,
    attachment_chip_motion: Option<&AttachmentChipMotionFrame>,
    stdin_overlay_motion: Option<&StdinOverlayMotionFrame>,
    transcript_message_motion: Option<&TranscriptMessageMotionFrame>,
    transcript_motion: Option<&TranscriptCardMotionFrame>,
    inline_markdown_motion: Option<&InlineMarkdownPillMotionFrame>,
    activity_cue_motion: Option<&StreamingActivityCueMotionFrame>,
    tool_motion: Option<&ToolCardMotionFrame>,
    scrollbar_motion: Option<&SingleSessionScrollbarMotionFrame>,
) -> Vec<Vertex> {
    let width = size.width as f32;
    let height = size.height as f32;
    let mut vertices = Vec::with_capacity(2048);

    push_gradient_rect(
        &mut vertices,
        Rect {
            x: 0.0,
            y: 0.0,
            width,
            height,
        },
        BACKGROUND_TOP_LEFT,
        BACKGROUND_BOTTOM_LEFT,
        BACKGROUND_BOTTOM_RIGHT,
        BACKGROUND_TOP_RIGHT,
        size,
    );

    let rect = Rect {
        x: 0.0,
        y: 0.0,
        width: width.max(1.0),
        height: height.max(1.0),
    };
    let surface = single_session_surface(app.session.as_ref());
    push_single_session_surface_without_bottom_rule(
        &mut vertices,
        rect,
        surface.color_index,
        focus_pulse,
        size,
    );

    let layout = single_session_layout_for_total_lines(app, size, rendered_body_lines.len());
    push_single_session_composer_chrome(
        &mut vertices,
        app,
        size,
        composer_motion,
        attachment_chip_motion,
        Some(layout),
    );

    let welcome_chrome_offset = if app.is_welcome_timeline_visible() {
        welcome_timeline_visual_offset_pixels_for_total_lines(
            app,
            size,
            smooth_scroll_lines,
            rendered_body_lines.len(),
        )
    } else {
        0.0
    };
    if welcome_timeline_chrome_visible(app, size, welcome_chrome_offset) {
        push_fresh_welcome_ambient(&mut vertices, size, spinner_tick, welcome_chrome_offset);
        push_handwritten_welcome_hero_with_offset(
            &mut vertices,
            &app.welcome_hero_text(),
            size,
            app.text_scale(),
            welcome_hero_reveal_progress,
            welcome_chrome_offset,
        );
    }

    push_single_session_inline_widget_card(
        &mut vertices,
        app,
        size,
        welcome_chrome_offset,
        rendered_body_lines.len(),
        inline_selection_motion,
        inline_list_reflow_motion,
        inline_preview_pane_motion,
    );

    push_single_session_stdin_overlay(
        &mut vertices,
        app,
        size,
        rendered_body_lines,
        stdin_overlay_motion,
    );

    let viewport = single_session_body_viewport_from_lines(
        app,
        size,
        smooth_scroll_lines,
        rendered_body_lines,
    );
    push_single_session_transcript_message_highlights_from_viewport(
        &mut vertices,
        app,
        size,
        &viewport,
        rendered_body_lines.len(),
        transcript_message_motion,
    );
    push_single_session_transcript_cards_from_viewport(
        &mut vertices,
        app,
        size,
        &viewport,
        rendered_body_lines.len(),
        transcript_motion,
    );
    push_single_session_tool_cards_from_viewport(
        &mut vertices,
        app,
        size,
        &viewport,
        rendered_body_lines.len(),
        spinner_tick,
        tool_motion,
    );
    push_single_session_inline_code_cards_from_viewport(
        &mut vertices,
        app,
        size,
        &viewport,
        rendered_body_lines.len(),
        inline_markdown_motion,
    );
    push_single_session_markdown_rule_lines_from_viewport(
        &mut vertices,
        app,
        size,
        &viewport,
        rendered_body_lines.len(),
    );
    if app.has_activity_indicator()
        || activity_cue_motion.is_some_and(|motion| motion.exiting().is_some())
    {
        push_streaming_activity_cue(
            &mut vertices,
            app,
            size,
            spinner_tick,
            Some(&viewport),
            activity_cue_motion,
        );
    }
    push_single_session_selection(&mut vertices, app, size);
    push_single_session_scrollbar_for_total_lines(
        &mut vertices,
        app,
        size,
        smooth_scroll_lines,
        rendered_body_lines.len(),
        scrollbar_motion,
    );

    vertices
}

fn single_session_scrollbar_track_x(size: PhysicalSize<u32>) -> f32 {
    size.width as f32 - PANEL_TITLE_LEFT_PADDING - 4.0
}

fn single_session_content_right(size: PhysicalSize<u32>) -> f32 {
    (single_session_scrollbar_track_x(size) - SINGLE_SESSION_SCROLLBAR_GAP)
        .max(PANEL_TITLE_LEFT_PADDING + 1.0)
}

fn single_session_content_width(size: PhysicalSize<u32>) -> f32 {
    (single_session_content_right(size) - PANEL_TITLE_LEFT_PADDING).max(1.0)
}

#[derive(Clone, Copy, Debug)]
struct SingleSessionLayoutMetrics {
    body_line_height: f32,
    composer_line_height: f32,
}

#[derive(Clone, Copy, Debug)]
struct SingleSessionLayout {
    body: Rect,
    draft_top: f32,
    composer: Rect,
    activity_lane: Option<Rect>,
    metrics: SingleSessionLayoutMetrics,
}

impl SingleSessionLayout {
    #[inline]
    fn body_bottom(self) -> f32 {
        rect_bottom(self.body)
    }

    #[inline]
    fn body_text_bounds_bottom(self) -> i32 {
        text_bounds_bottom(self.body_bottom())
    }
}

fn single_session_layout_metrics(app: &SingleSessionApp) -> SingleSessionLayoutMetrics {
    let typography = single_session_typography_for_scale(app.text_scale());
    SingleSessionLayoutMetrics {
        body_line_height: typography.body_size * typography.body_line_height,
        composer_line_height: typography.code_size * typography.code_line_height,
    }
}

fn single_session_layout_for_app(
    app: &SingleSessionApp,
    size: PhysicalSize<u32>,
) -> SingleSessionLayout {
    single_session_layout_from_bounds(
        app,
        size,
        single_session_draft_top_for_app(app, size),
        single_session_body_bottom_base_for_app(app, size),
    )
}

fn single_session_layout_for_total_lines(
    app: &SingleSessionApp,
    size: PhysicalSize<u32>,
    total_lines: usize,
) -> SingleSessionLayout {
    single_session_layout_from_bounds(
        app,
        size,
        single_session_draft_top_for_total_lines(app, size, total_lines),
        single_session_body_bottom_base_for_total_lines(app, size, total_lines),
    )
}

fn single_session_layout_from_bounds(
    app: &SingleSessionApp,
    size: PhysicalSize<u32>,
    draft_top: f32,
    body_base_bottom: f32,
) -> SingleSessionLayout {
    let metrics = single_session_layout_metrics(app);
    let body_top = PANEL_BODY_TOP_PADDING;
    let body_base_bottom = body_base_bottom.max(body_top);
    let inline_widget_reserved_height = inline_widget_reserved_height(app);
    let activity_reserved_height = streaming_activity_reserved_height(app);
    let body_bottom =
        (body_base_bottom - inline_widget_reserved_height - activity_reserved_height).max(body_top);
    let activity_lane = (activity_reserved_height > 0.0).then(|| {
        let activity_top = (body_base_bottom - activity_reserved_height).max(body_top);
        Rect {
            x: PANEL_TITLE_LEFT_PADDING,
            y: activity_top,
            width: single_session_content_width(size),
            height: (body_base_bottom - activity_top).max(0.0),
        }
    });
    let composer_target = composer_motion_target(app);
    let composer_visual = ComposerMotionVisual::settled(composer_target);
    let composer_height = single_session_composer_height(size, metrics, composer_visual);

    SingleSessionLayout {
        body: Rect {
            x: PANEL_TITLE_LEFT_PADDING,
            y: body_top,
            width: single_session_content_width(size),
            height: (body_bottom - body_top).max(0.0),
        },
        draft_top,
        composer: Rect {
            x: PANEL_TITLE_LEFT_PADDING - 10.0,
            y: draft_top - 9.0,
            width: single_session_content_width(size) + 20.0,
            height: composer_height,
        },
        activity_lane,
        metrics,
    }
}

fn inline_widget_bottom_limit_for_layout(
    app: &SingleSessionApp,
    layout: SingleSessionLayout,
    welcome_chrome_visible: bool,
) -> f32 {
    if welcome_chrome_visible
        && app.render_inline_widget_line_count() > 0
        && !app.has_welcome_timeline_transcript()
    {
        return layout.draft_top;
    }

    layout
        .activity_lane
        .map(|activity| activity.y)
        .unwrap_or(layout.draft_top)
}

fn single_session_composer_height(
    size: PhysicalSize<u32>,
    metrics: SingleSessionLayoutMetrics,
    visual: ComposerMotionVisual,
) -> f32 {
    (visual.height_lines.max(1.0) * metrics.composer_line_height + 18.0)
        .min((size.height as f32 * 0.34).max(metrics.composer_line_height + 18.0))
}

#[inline]
fn rect_bottom(rect: Rect) -> f32 {
    rect.y + rect.height
}

#[cfg(test)]
pub(crate) fn welcome_hero_reveal_progress_for_tick(spinner_tick: u64) -> f32 {
    let elapsed =
        Duration::from_millis(spinner_tick.saturating_mul(DESKTOP_SPINNER_FRAME_MS as u64));
    welcome_hero_reveal_progress_for_elapsed(elapsed)
}

pub(crate) fn welcome_hero_reveal_progress_for_elapsed(elapsed: Duration) -> f32 {
    const REVEAL_DURATION: Duration = Duration::from_millis(1350);
    const FIRST_INK_PROGRESS: f32 = 0.018;

    if crate::animation::desktop_reduced_motion_enabled() {
        return 1.0;
    }

    let raw = (elapsed.as_secs_f32() / REVEAL_DURATION.as_secs_f32()).clamp(0.0, 1.0);
    if raw >= 1.0 {
        return 1.0;
    }

    let eased = ease_in_out_cubic(raw);
    FIRST_INK_PROGRESS + (1.0 - FIRST_INK_PROGRESS) * eased
}

pub(crate) fn welcome_hero_runtime_mask_supported(phrase: &str) -> bool {
    let enabled = std::env::var_os("JCODE_DESKTOP_RUNTIME_HERO_MASK").is_none_or(|value| {
        !matches!(
            value.to_string_lossy().trim().to_ascii_lowercase().as_str(),
            "" | "0" | "false" | "off" | "no"
        )
    });
    enabled && phrase.trim().eq_ignore_ascii_case("Hello there")
}

pub(crate) fn welcome_hero_runtime_mask_rect(
    size: PhysicalSize<u32>,
    ui_scale: f32,
    y_offset: f32,
) -> Rect {
    let (hero_min, hero_max) = glyph_welcome_hero_bounds(size, ui_scale);
    Rect {
        x: hero_min[0],
        y: hero_min[1] + y_offset,
        width: (hero_max[0] - hero_min[0]).max(1.0),
        height: (hero_max[1] - hero_min[1]).max(1.0),
    }
}

pub(crate) fn welcome_hero_runtime_font_size(size: PhysicalSize<u32>, ui_scale: f32) -> f32 {
    glyph_welcome_hero_font_size(size, ui_scale)
}

pub(crate) fn welcome_hero_runtime_mask_spec_for_total_lines(
    app: &SingleSessionApp,
    size: PhysicalSize<u32>,
    smooth_scroll_lines: f32,
    total_lines: usize,
) -> Option<WelcomeHeroRuntimeMaskSpec> {
    let y_offset = welcome_timeline_visual_offset_pixels_for_total_lines(
        app,
        size,
        smooth_scroll_lines,
        total_lines,
    );
    if !welcome_timeline_chrome_visible(app, size, y_offset) {
        return None;
    }
    welcome_hero_runtime_mask_spec_for_phrase(
        &app.welcome_hero_text(),
        size,
        app.text_scale(),
        y_offset,
    )
}

pub(crate) fn welcome_hero_runtime_mask_spec_for_phrase(
    phrase: &str,
    size: PhysicalSize<u32>,
    ui_scale: f32,
    y_offset: f32,
) -> Option<WelcomeHeroRuntimeMaskSpec> {
    if !welcome_hero_runtime_mask_supported(phrase) {
        return None;
    }
    Some(WelcomeHeroRuntimeMaskSpec {
        phrase: phrase.to_string(),
        rect: welcome_hero_runtime_mask_rect(size, ui_scale, y_offset),
        font_size: welcome_hero_runtime_font_size(size, ui_scale),
    })
}

pub(crate) fn welcome_hero_normalized_stroke_segments(
    phrase: &str,
) -> Vec<WelcomeHeroStrokeSegment> {
    let paths = handwritten_welcome_paths_for_phrase(phrase);
    let total_length = stroke_paths_length(&paths);
    if total_length <= 0.001 {
        return Vec::new();
    }

    let (source_min, source_max) = stroke_paths_bounds(&paths);
    let source_width = (source_max[0] - source_min[0]).max(0.001);
    let source_height = (source_max[1] - source_min[1]).max(0.001);
    let normalize = |point: [f32; 2]| -> [f32; 2] {
        [
            ((point[0] - source_min[0]) / source_width).clamp(0.0, 1.0),
            ((point[1] - source_min[1]) / source_height).clamp(0.0, 1.0),
        ]
    };

    let mut cursor = 0.0;
    let mut segments = Vec::new();
    for path in &paths {
        for pair in path.windows(2) {
            let start = pair[0];
            let end = pair[1];
            let segment_length = distance(start, end);
            if segment_length <= 0.001 {
                continue;
            }
            let start_progress = cursor / total_length;
            cursor += segment_length;
            let end_progress = (cursor / total_length).clamp(start_progress, 1.0);
            segments.push(WelcomeHeroStrokeSegment {
                start: normalize(start),
                end: normalize(end),
                start_progress,
                end_progress,
            });
        }
    }
    segments
}

pub(crate) fn welcome_hero_reveal_is_active(progress: f32) -> bool {
    progress < 0.999
}

fn ease_in_out_cubic(t: f32) -> f32 {
    if t < 0.5 {
        4.0 * t * t * t
    } else {
        1.0 - (-2.0 * t + 2.0).powi(3) / 2.0
    }
}

fn push_single_session_surface_without_bottom_rule(
    vertices: &mut Vec<Vertex>,
    rect: Rect,
    color_index: usize,
    focus_pulse: f32,
    size: PhysicalSize<u32>,
) {
    let accent = panel_accent_color(color_index, true);
    push_rounded_rect(
        vertices,
        rect,
        PANEL_RADIUS,
        with_alpha(accent, 0.105),
        size,
    );
    push_rounded_rect(
        vertices,
        Rect {
            x: rect.x,
            y: rect.y,
            width: 5.0_f32.min(rect.width),
            height: rect.height,
        },
        PANEL_RADIUS,
        with_alpha(accent, 0.78),
        size,
    );

    let stroke_width = FOCUSED_BORDER_WIDTH + focus_pulse * 2.5;
    push_top_and_side_surface_outline(vertices, rect, stroke_width, accent, size);

    if focus_pulse > 0.0 {
        let pulse_rect = inset_rect(rect, -3.0 * focus_pulse);
        push_top_and_side_surface_outline(
            vertices,
            pulse_rect,
            1.0,
            with_alpha(FOCUS_RING_COLOR, 0.32 * focus_pulse),
            size,
        );
    }
}

fn push_top_and_side_surface_outline(
    vertices: &mut Vec<Vertex>,
    rect: Rect,
    stroke_width: f32,
    color: [f32; 4],
    size: PhysicalSize<u32>,
) {
    let stroke_width = stroke_width.max(1.0).min(rect.width).min(rect.height);
    push_rect(
        vertices,
        Rect {
            x: rect.x,
            y: rect.y,
            width: rect.width,
            height: stroke_width,
        },
        color,
        size,
    );
    push_rect(
        vertices,
        Rect {
            x: rect.x,
            y: rect.y,
            width: stroke_width,
            height: rect.height,
        },
        color,
        size,
    );
    push_rect(
        vertices,
        Rect {
            x: rect.x + rect.width - stroke_width,
            y: rect.y,
            width: stroke_width,
            height: rect.height,
        },
        color,
        size,
    );
}

fn push_single_session_composer_chrome(
    vertices: &mut Vec<Vertex>,
    app: &SingleSessionApp,
    size: PhysicalSize<u32>,
    composer_motion: Option<&ComposerMotionFrame>,
    attachment_chip_motion: Option<&AttachmentChipMotionFrame>,
    layout: Option<SingleSessionLayout>,
) {
    if welcome_status_lane_visible(app) {
        return;
    }

    let typography = single_session_typography();
    let layout = layout.unwrap_or_else(|| single_session_layout_for_app(app, size));
    let target = composer_motion_target(app);
    let visual = composer_motion
        .map(|frame| frame.visual())
        .unwrap_or_else(|| ComposerMotionVisual::settled(target));
    let line_height = layout.metrics.composer_line_height;
    let draft_top = layout.draft_top;
    let content_width = layout.body.width;
    let rect = Rect {
        height: single_session_composer_height(size, layout.metrics, visual),
        ..layout.composer
    };
    if rect.width <= 12.0 || rect.height <= 10.0 {
        return;
    }

    let radius = 13.0;
    let focus_alpha = COMPOSER_FOCUS_RING_COLOR[3]
        * (0.38 + 0.62 * visual.focus_opacity)
        * (1.0 - visual.blocked_progress * 0.42);
    let halo_inset = -2.0 - 2.0 * visual.focus_opacity;
    push_rounded_rect(
        vertices,
        inset_rect(rect, halo_inset),
        radius + 3.0,
        with_alpha(COMPOSER_FOCUS_RING_COLOR, focus_alpha),
        size,
    );

    let card_color = mix_color(
        COMPOSER_CARD_BACKGROUND_COLOR,
        [0.970, 0.984, 1.000, COMPOSER_CARD_BACKGROUND_COLOR[3]],
        visual.blocked_progress * 0.35,
    );
    push_rounded_rect(vertices, rect, radius, card_color, size);

    push_single_session_attachment_chips(vertices, app, size, rect, attachment_chip_motion);

    let accent_alpha =
        (0.18 + 0.22 * visual.focus_opacity) * (1.0 - visual.blocked_progress * 0.55);
    push_rounded_rect(
        vertices,
        Rect {
            x: rect.x + 7.0,
            y: rect.y + 7.0,
            width: 3.0,
            height: (rect.height - 14.0).max(1.0),
        },
        2.0,
        with_alpha(COMPOSER_SUBMIT_READY_COLOR, accent_alpha),
        size,
    );

    if visual.placeholder_opacity > 0.001 {
        let prompt_width =
            app.composer_prompt().chars().count() as f32 * typography.code_size * 0.58;
        let rail_width = (content_width * 0.32).clamp(96.0, 260.0);
        push_rounded_rect(
            vertices,
            Rect {
                x: PANEL_TITLE_LEFT_PADDING + prompt_width + 8.0,
                y: draft_top + line_height * 0.50,
                width: rail_width,
                height: 4.0,
            },
            2.0,
            with_alpha(
                COMPOSER_PLACEHOLDER_RAIL_COLOR,
                COMPOSER_PLACEHOLDER_RAIL_COLOR[3] * visual.placeholder_opacity,
            ),
            size,
        );
    }

    if visual.submit_opacity > 0.001 {
        let pill_height = 22.0 * visual.submit_scale.max(0.72);
        let pill_width = 36.0 * visual.submit_scale.max(0.72);
        let pill_x = single_session_content_right(size) - pill_width;
        let pill_y = draft_top + (line_height - pill_height) * 0.5;
        let submit_color = mix_color(
            COMPOSER_SUBMIT_READY_COLOR,
            COMPOSER_SUBMIT_BUSY_COLOR,
            visual.processing_progress,
        );
        push_rounded_rect(
            vertices,
            Rect {
                x: pill_x,
                y: pill_y,
                width: pill_width,
                height: pill_height,
            },
            pill_height * 0.5,
            with_alpha(submit_color, submit_color[3] * visual.submit_opacity),
            size,
        );
        let arrow_alpha = (0.54 + 0.26 * visual.focus_opacity) * visual.submit_opacity;
        let arrow_y = pill_y + pill_height * 0.5 - 1.0;
        push_rect(
            vertices,
            Rect {
                x: pill_x + pill_width * 0.30,
                y: arrow_y,
                width: pill_width * 0.36,
                height: 2.0,
            },
            [1.0, 1.0, 1.0, arrow_alpha],
            size,
        );
        push_rect(
            vertices,
            Rect {
                x: pill_x + pill_width * 0.55,
                y: arrow_y - 4.0,
                width: 2.0,
                height: 10.0,
            },
            [1.0, 1.0, 1.0, arrow_alpha],
            size,
        );
    }
}

fn push_single_session_attachment_chips(
    vertices: &mut Vec<Vertex>,
    app: &SingleSessionApp,
    size: PhysicalSize<u32>,
    composer_rect: Rect,
    attachment_chip_motion: Option<&AttachmentChipMotionFrame>,
) {
    let runs = attachment_chip_runs(&app.pending_images);
    if runs.is_empty() && attachment_chip_motion.is_none_or(|motion| motion.exiting().is_empty()) {
        return;
    }

    for run in runs {
        let visual = attachment_chip_motion
            .and_then(|motion| motion.visual_for_key(run.key))
            .unwrap_or_else(AttachmentChipVisual::settled);
        push_single_session_attachment_chip(vertices, composer_rect, run, visual, false, size);
    }

    if let Some(motion) = attachment_chip_motion {
        for (run, visual) in motion.exiting() {
            push_single_session_attachment_chip(vertices, composer_rect, *run, *visual, true, size);
        }
    }
}

fn push_single_session_attachment_chip(
    vertices: &mut Vec<Vertex>,
    composer_rect: Rect,
    run: AttachmentChipRun,
    visual: AttachmentChipVisual,
    exiting: bool,
    size: PhysicalSize<u32>,
) {
    if visual.opacity <= 0.001 || visual.scale <= 0.05 {
        return;
    }
    let scaled_width = ATTACHMENT_CHIP_WIDTH * visual.scale;
    let scaled_height = ATTACHMENT_CHIP_HEIGHT * visual.scale;
    let step = ATTACHMENT_CHIP_WIDTH + ATTACHMENT_CHIP_GAP;
    let x = composer_rect.x
        + 18.0
        + run.index as f32 * step
        + visual.x_offset_pixels
        + (ATTACHMENT_CHIP_WIDTH - scaled_width) * 0.5;
    let y = (composer_rect.y - ATTACHMENT_CHIP_HEIGHT - 8.0).max(PANEL_BODY_TOP_PADDING + 8.0)
        + visual.y_offset_pixels
        + (ATTACHMENT_CHIP_HEIGHT - scaled_height) * 0.5;
    let max_right = composer_rect.x + composer_rect.width - 16.0;
    if x >= max_right || y >= composer_rect.y + composer_rect.height {
        return;
    }
    let chip_rect = Rect {
        x,
        y,
        width: scaled_width.min((max_right - x).max(0.0)),
        height: scaled_height,
    };
    if chip_rect.width <= 5.0 || chip_rect.height <= 5.0 {
        return;
    }
    let fill = if exiting {
        ATTACHMENT_CHIP_EXIT_COLOR
    } else {
        ATTACHMENT_CHIP_BACKGROUND_COLOR
    };
    push_rounded_rect(
        vertices,
        chip_rect,
        chip_rect.height * 0.5,
        with_alpha(fill, fill[3] * visual.opacity),
        size,
    );
    let accent_width = (chip_rect.height * 0.34).clamp(5.0, 8.0);
    push_rounded_rect(
        vertices,
        Rect {
            x: chip_rect.x + 5.0 * visual.scale,
            y: chip_rect.y + (chip_rect.height - accent_width) * 0.5,
            width: accent_width,
            height: accent_width,
        },
        2.5 * visual.scale,
        with_alpha(
            ATTACHMENT_CHIP_ACCENT_COLOR,
            ATTACHMENT_CHIP_ACCENT_COLOR[3] * visual.opacity,
        ),
        size,
    );
    push_rect(
        vertices,
        Rect {
            x: chip_rect.x + chip_rect.width * 0.45,
            y: chip_rect.y + chip_rect.height * 0.43,
            width: chip_rect.width * 0.32,
            height: 2.0 * visual.scale,
        },
        with_alpha(COMPOSER_FOCUS_RING_COLOR, 0.42 * visual.opacity),
        size,
    );
}

fn push_single_session_stdin_overlay(
    vertices: &mut Vec<Vertex>,
    app: &SingleSessionApp,
    size: PhysicalSize<u32>,
    rendered_body_lines: &[SingleSessionStyledLine],
    stdin_overlay_motion: Option<&StdinOverlayMotionFrame>,
) {
    let settled_current = stdin_overlay_target(app, rendered_body_lines)
        .map(|target| (target, StdinOverlayVisual::settled(target)));
    let current = stdin_overlay_motion
        .and_then(|motion| motion.current)
        .or(settled_current);
    if let Some((target, visual)) = current {
        push_single_session_stdin_overlay_visual(vertices, app, size, target, visual, false);
    }
    if let Some((target, visual)) = stdin_overlay_motion.and_then(|motion| motion.exiting) {
        push_single_session_stdin_overlay_visual(vertices, app, size, target, visual, true);
    }
}

fn push_single_session_stdin_overlay_visual(
    vertices: &mut Vec<Vertex>,
    app: &SingleSessionApp,
    size: PhysicalSize<u32>,
    target: StdinOverlayTarget,
    visual: StdinOverlayVisual,
    exiting: bool,
) {
    if visual.opacity <= 0.001 || visual.scale <= 0.05 {
        return;
    }
    let typography = single_session_typography_for_scale(app.text_scale());
    let line_height = typography.body_size * typography.body_line_height;
    let left = PANEL_TITLE_LEFT_PADDING - 10.0;
    let width = single_session_content_width(size) + 20.0;
    let body_top = single_session_body_top_for_app(app, size);
    let body_bottom = single_session_body_bottom_for_total_lines(app, size, target.line_count);
    let height = (visual.height_lines.max(1.0) * line_height + 18.0)
        .min((body_bottom - body_top + 20.0).max(line_height + 18.0));
    let rect = scaled_rect(
        Rect {
            x: left,
            y: body_top - 8.0 + visual.y_offset_pixels,
            width,
            height,
        },
        visual.scale,
    );
    if rect.width <= 12.0 || rect.height <= 10.0 {
        return;
    }

    let background = if exiting {
        mix_color(
            STDIN_OVERLAY_BACKGROUND_COLOR,
            STDIN_OVERLAY_EXIT_COLOR,
            0.42,
        )
    } else if target.password {
        mix_color(
            STDIN_OVERLAY_BACKGROUND_COLOR,
            [0.990, 0.968, 1.000, 0.660],
            0.36,
        )
    } else {
        STDIN_OVERLAY_BACKGROUND_COLOR
    };
    push_rounded_rect(
        vertices,
        inset_rect(rect, 2.0),
        15.0,
        stdin_overlay_alpha([0.020, 0.035, 0.080, 0.070], visual.opacity),
        size,
    );
    push_rounded_rect(
        vertices,
        rect,
        14.0,
        stdin_overlay_alpha(background, visual.opacity),
        size,
    );
    push_rounded_rect(
        vertices,
        Rect {
            x: rect.x + 7.0,
            y: rect.y + 7.0,
            width: 4.0,
            height: (rect.height - 14.0).max(1.0),
        },
        2.0,
        stdin_overlay_alpha(STDIN_OVERLAY_BORDER_COLOR, visual.opacity * 1.35),
        size,
    );
    push_top_and_side_surface_outline(
        vertices,
        rect,
        1.25,
        stdin_overlay_alpha(STDIN_OVERLAY_BORDER_COLOR, visual.opacity),
        size,
    );

    let input_top = body_top
        + target.input_line_start as f32 * line_height
        + visual.y_offset_pixels
        + line_height * 0.12;
    let input_height = (target.input_line_count as f32 * line_height - line_height * 0.24).max(8.0);
    let input_rect = Rect {
        x: rect.x + 16.0,
        y: input_top.max(rect.y + 8.0).min(rect.y + rect.height - 10.0),
        width: (rect.width - 32.0).max(1.0),
        height: input_height.min((rect.y + rect.height - input_top - 8.0).max(8.0)),
    };
    push_rounded_rect(
        vertices,
        input_rect,
        8.0,
        stdin_overlay_alpha(
            STDIN_OVERLAY_INPUT_RAIL_COLOR,
            visual.opacity * (0.55 + 0.45 * visual.input_glow),
        ),
        size,
    );

    if visual.submit_opacity > 0.001 {
        let pill_width = 44.0;
        let pill_height = 20.0;
        let pill = Rect {
            x: rect.x + rect.width - pill_width - 13.0,
            y: rect.y + rect.height - pill_height - 10.0,
            width: pill_width,
            height: pill_height,
        };
        push_rounded_rect(
            vertices,
            pill,
            pill_height * 0.5,
            stdin_overlay_alpha(
                STDIN_OVERLAY_SUBMIT_COLOR,
                visual.opacity * visual.submit_opacity,
            ),
            size,
        );
        let mark_alpha = visual.opacity * visual.submit_opacity * 0.74;
        push_rect(
            vertices,
            Rect {
                x: pill.x + pill.width * 0.30,
                y: pill.y + pill.height * 0.50,
                width: pill.width * 0.36,
                height: 2.0,
            },
            [1.0, 1.0, 1.0, mark_alpha],
            size,
        );
        push_rect(
            vertices,
            Rect {
                x: pill.x + pill.width * 0.55,
                y: pill.y + pill.height * 0.30,
                width: 2.0,
                height: pill.height * 0.42,
            },
            [1.0, 1.0, 1.0, mark_alpha],
            size,
        );
    }
}

fn stdin_overlay_alpha(mut color: [f32; 4], opacity: f32) -> [f32; 4] {
    color[3] = (color[3] * opacity.clamp(0.0, 1.0)).clamp(0.0, 1.0);
    color
}

fn scaled_rect(rect: Rect, scale: f32) -> Rect {
    let scale = scale.clamp(0.01, 1.5);
    let width = rect.width * scale;
    let height = rect.height * scale;
    Rect {
        x: rect.x + (rect.width - width) * 0.5,
        y: rect.y + (rect.height - height) * 0.5,
        width,
        height,
    }
}

fn push_fresh_welcome_ambient(
    vertices: &mut Vec<Vertex>,
    size: PhysicalSize<u32>,
    tick: u64,
    y_offset: f32,
) {
    let draft_top = single_session_draft_top(size);
    let usable_height = (draft_top - PANEL_BODY_TOP_PADDING).max(180.0);
    let t = tick as f32 * 0.055;

    push_aurora_ribbon(
        vertices,
        size,
        PANEL_BODY_TOP_PADDING + usable_height * 0.18 + (t * 0.60).sin() * 18.0 + y_offset,
        usable_height * 0.30,
        t * 0.85,
        WELCOME_AURORA_BLUE,
        WELCOME_AURORA_VIOLET,
    );
    push_aurora_ribbon(
        vertices,
        size,
        PANEL_BODY_TOP_PADDING + usable_height * 0.39 + (t * 0.47).cos() * 24.0 + y_offset,
        usable_height * 0.34,
        t * -0.72 + 1.8,
        WELCOME_AURORA_MINT,
        WELCOME_AURORA_BLUE,
    );
    push_aurora_ribbon(
        vertices,
        size,
        PANEL_BODY_TOP_PADDING + usable_height * 0.58 + (t * 0.52).sin() * 16.0 + y_offset,
        usable_height * 0.24,
        t * 0.64 + 3.2,
        WELCOME_AURORA_WARM,
        WELCOME_AURORA_MINT,
    );
}

fn push_handwritten_welcome_hero_with_offset(
    vertices: &mut Vec<Vertex>,
    phrase: &str,
    size: PhysicalSize<u32>,
    ui_scale: f32,
    reveal_progress: f32,
    y_offset: f32,
) {
    if !welcome_hero_approx_bounds_visible(size, ui_scale, y_offset) {
        return;
    }

    let progress = reveal_progress.clamp(0.0, 1.0);
    if !welcome_hero_reveal_is_active(progress) {
        return;
    }

    if welcome_hero_runtime_mask_supported(phrase) {
        return;
    }

    let paths = handwritten_welcome_paths_for_phrase(phrase);
    let total_length = stroke_paths_length(&paths);
    if total_length <= 0.0 {
        return;
    }

    let (bounds_min, bounds_max) = glyph_welcome_hero_bounds(size, ui_scale);
    let hero_height = (bounds_max[1] - bounds_min[1]).max(1.0);
    let baseline_lift = hero_height * 0.11;
    let bounds_min = [bounds_min[0], bounds_min[1] + y_offset - baseline_lift];
    let bounds_max = [bounds_max[0], bounds_max[1] + y_offset - baseline_lift];
    let (source_min, source_max) = stroke_paths_bounds(&paths);
    let source_width = (source_max[0] - source_min[0]).max(1.0);
    let scale = (bounds_max[0] - bounds_min[0]) / source_width;
    let origin = [
        bounds_min[0] - source_min[0] * scale,
        bounds_min[1] - source_min[1] * scale,
    ];
    let thickness = (scale * 0.036).clamp(1.8, 4.6);
    let mut remaining = total_length * progress;
    let mut lead = None;

    for path in &paths {
        for pair in path.windows(2) {
            let a = pair[0];
            let b = pair[1];
            let segment_length = distance(a, b);
            if segment_length <= 0.001 || remaining <= 0.0 {
                continue;
            }
            let draw_fraction = (remaining / segment_length).clamp(0.0, 1.0);
            let end = lerp_point(a, b, draw_fraction);
            let pa = transform_handwriting_point(a, origin, scale);
            let pb = transform_handwriting_point(end, origin, scale);
            push_stroke_segment(vertices, pa, pb, thickness, WELCOME_HANDWRITING_COLOR, size);
            lead = Some(pb);
            remaining -= segment_length;
            if draw_fraction < 1.0 {
                break;
            }
        }
    }

    if let Some(point) = lead
        && (0.01..0.995).contains(&progress)
    {
        push_stroke_dot(
            vertices,
            point,
            thickness * 1.65,
            WELCOME_HANDWRITING_COLOR,
            size,
        );
    }
}

fn welcome_timeline_chrome_visible(
    app: &SingleSessionApp,
    size: PhysicalSize<u32>,
    y_offset: f32,
) -> bool {
    app.is_welcome_timeline_visible()
        && (!app.has_welcome_timeline_transcript()
            || welcome_hero_approx_bounds_visible(size, app.text_scale(), y_offset))
}

fn welcome_hero_approx_bounds_visible(
    size: PhysicalSize<u32>,
    ui_scale: f32,
    y_offset: f32,
) -> bool {
    let body_top = PANEL_BODY_TOP_PADDING;
    let draft_top = single_session_draft_top(size);
    let top = body_top + (draft_top - body_top) * 0.18 + y_offset;
    let bottom = body_top + (draft_top - body_top) * 0.74 * ui_scale + y_offset;
    bottom >= -64.0 && top <= size.height as f32 + 64.0
}

fn welcome_timeline_visual_offset_pixels(
    app: &SingleSessionApp,
    size: PhysicalSize<u32>,
    smooth_scroll_lines: f32,
) -> f32 {
    welcome_timeline_visual_offset_pixels_for_total_lines(
        app,
        size,
        smooth_scroll_lines,
        welcome_timeline_total_body_lines(app, size),
    )
}

fn welcome_timeline_visual_offset_pixels_for_total_lines(
    app: &SingleSessionApp,
    size: PhysicalSize<u32>,
    smooth_scroll_lines: f32,
    total_lines: usize,
) -> f32 {
    if !app.is_welcome_timeline_visible() {
        return 0.0;
    }

    if !app.has_welcome_timeline_transcript() {
        return fresh_welcome_inline_widget_visual_offset(app, size);
    }

    let typography = single_session_typography_for_scale(app.text_scale());
    let line_height = typography.body_size * typography.body_line_height;
    let body_top = single_session_body_top_for_app(app, size);
    let body_bottom = single_session_body_bottom_for_total_lines(app, size, total_lines);
    let visible_lines = (((body_bottom - body_top).max(line_height)) / line_height)
        .floor()
        .max(1.0);
    let total_lines = total_lines as f32;
    if total_lines <= visible_lines {
        return 0.0;
    }

    let max_scroll = (total_lines - visible_lines).max(0.0);
    let scroll = (app.body_scroll_lines + smooth_scroll_lines).clamp(0.0, max_scroll);
    let top_line = (total_lines - scroll - visible_lines).max(0.0);
    -top_line * line_height
}

fn fresh_welcome_inline_widget_visual_offset(
    app: &SingleSessionApp,
    size: PhysicalSize<u32>,
) -> f32 {
    if app.render_inline_widget_line_count() == 0 {
        return 0.0;
    }

    let typography = single_session_typography_for_scale(app.text_scale());
    let line_height = typography.body_size * typography.body_line_height;
    let visual_bottom = fresh_welcome_visual_bottom_for_scale(size, app.text_scale());
    let gap = fresh_welcome_inline_widget_gap_for_scale(app.text_scale());
    let draft_top = single_session_draft_top_for_app(app, size);
    let inline_height = inline_widget_visible_text_height(app).max(line_height);
    let available = (draft_top - visual_bottom - gap).max(0.0);

    if inline_height <= available {
        0.0
    } else {
        -(inline_height - available)
    }
}

fn push_single_session_inline_widget_card(
    vertices: &mut Vec<Vertex>,
    app: &SingleSessionApp,
    size: PhysicalSize<u32>,
    welcome_chrome_offset_pixels: f32,
    total_lines: usize,
    inline_selection_motion: Option<&InlineWidgetSelectionMotionFrame>,
    inline_list_reflow_motion: Option<&InlineWidgetListReflowMotionFrame>,
    inline_preview_pane_motion: Option<&InlineWidgetPreviewPaneMotionFrame>,
) {
    let line_count = app.render_inline_widget_visible_line_count();
    if line_count == 0 {
        return;
    }

    let progress = app.render_inline_widget_reveal_progress().clamp(0.0, 1.0);
    if progress <= 0.001 {
        return;
    }

    let typography = single_session_typography_for_scale(app.text_scale());
    let session_layout = single_session_layout_for_total_lines(app, size, total_lines);
    let body_bottom = session_layout.body_bottom();
    let welcome_chrome_visible =
        welcome_timeline_chrome_visible(app, size, welcome_chrome_offset_pixels);
    let inline_bottom_limit =
        inline_widget_bottom_limit_for_layout(app, session_layout, welcome_chrome_visible);
    let target_top = inline_widget_target_top(
        size,
        app.text_scale(),
        body_bottom,
        welcome_chrome_visible,
        welcome_chrome_offset_pixels,
    );
    let inline_lines = app.render_inline_widget_styled_lines();
    let Some(layout) = inline_widget_card_layout_with_bottom_limit(
        size,
        app.render_inline_widget_kind(),
        &typography,
        line_count,
        inline_widget_text_width_for_lines(
            app.render_inline_widget_kind(),
            &inline_lines,
            size,
            app.text_scale(),
        ),
        target_top,
        progress,
        inline_bottom_limit,
    ) else {
        return;
    };

    if app.render_inline_widget_kind().is_some() {
        let card_style = inline_widget_card_style(app.render_inline_widget_kind());
        push_rounded_rect(
            vertices,
            Rect {
                x: layout.card.x + 0.0,
                y: layout.card.y + 5.0,
                width: layout.card.width,
                height: layout.card.height,
            },
            layout.radius + 2.0,
            with_alpha(
                INLINE_WIDGET_CARD_SHADOW_COLOR,
                INLINE_WIDGET_CARD_SHADOW_COLOR[3] * progress,
            ),
            size,
        );
        push_rounded_rect(
            vertices,
            layout.card,
            layout.radius,
            with_alpha(card_style.border, card_style.border[3] * progress),
            size,
        );
        push_rounded_rect(
            vertices,
            inset_rect(layout.card, 1.0),
            (layout.radius - 1.0).max(1.0),
            with_alpha(card_style.background, card_style.background[3] * progress),
            size,
        );
        push_rounded_rect(
            vertices,
            Rect {
                x: layout.card.x + 1.5,
                y: layout.card.y + 1.5,
                width: 3.0,
                height: (layout.card.height - 3.0).max(0.0),
            },
            2.0,
            with_alpha(card_style.accent, card_style.accent[3] * progress),
            size,
        );
        push_rounded_rect(
            vertices,
            Rect {
                x: layout.card.x + 8.0,
                y: layout.card.y + 1.5,
                width: (layout.card.width - 16.0).max(0.0),
                height: 1.0,
            },
            0.5,
            with_alpha(card_style.highlight, card_style.highlight[3] * progress),
            size,
        );
    }

    if app.render_inline_widget_kind() == Some(InlineWidgetKind::ModelPicker) {
        push_single_session_inline_widget_structured_chrome(
            vertices,
            app.render_inline_widget_kind(),
            &inline_lines,
            line_count,
            &typography,
            &layout,
            progress,
            size,
        );
    } else {
        push_single_session_inline_widget_preview_panes(
            vertices,
            app.render_inline_widget_kind(),
            &inline_lines,
            line_count,
            &typography,
            &layout,
            progress,
            inline_preview_pane_motion,
            size,
        );
    }

    push_single_session_inline_widget_list_reflow(
        vertices,
        app.render_inline_widget_kind(),
        &inline_lines,
        line_count,
        &typography,
        &layout,
        progress,
        inline_list_reflow_motion,
        size,
    );

    push_single_session_inline_widget_selection(
        vertices,
        app.render_inline_widget_kind(),
        &inline_lines,
        line_count,
        &typography,
        &layout,
        progress,
        inline_selection_motion,
        size,
    );
}

#[derive(Clone, Copy, Debug)]
struct InlineWidgetPreviewPaneGeometry {
    sessions: Rect,
    preview: Rect,
    radius: f32,
}

#[allow(clippy::too_many_arguments)]
fn push_single_session_inline_widget_preview_panes(
    vertices: &mut Vec<Vertex>,
    kind: Option<InlineWidgetKind>,
    inline_lines: &[SingleSessionStyledLine],
    line_count: usize,
    typography: &SingleSessionTypography,
    layout: &InlineWidgetCardLayout,
    reveal_progress: f32,
    inline_preview_pane_motion: Option<&InlineWidgetPreviewPaneMotionFrame>,
    size: PhysicalSize<u32>,
) {
    let Some(geometry) =
        inline_widget_preview_pane_geometry(kind, inline_lines, line_count, typography, layout)
    else {
        return;
    };
    let visual = inline_preview_pane_motion
        .and_then(InlineWidgetPreviewPaneMotionFrame::visual)
        .unwrap_or(InlineWidgetPreviewPaneVisual {
            focus_pane_position: inline_widget_preview_pane_target(kind, inline_lines, line_count)
                .map(|target| target.focus_pane as f32)
                .unwrap_or_default(),
            preview_opacity: 1.0,
            preview_y_offset_pixels: 0.0,
        });
    let alpha = reveal_progress.clamp(0.0, 1.0);
    if alpha <= 0.001 {
        return;
    }

    for pane in [geometry.sessions, geometry.preview] {
        push_rounded_rect(
            vertices,
            pane,
            geometry.radius,
            with_alpha(
                INLINE_WIDGET_PREVIEW_PANE_BACKGROUND_COLOR,
                INLINE_WIDGET_PREVIEW_PANE_BACKGROUND_COLOR[3] * alpha,
            ),
            size,
        );
        push_rounded_rect(
            vertices,
            inset_rect(pane, 0.8),
            (geometry.radius - 1.0).max(1.0),
            with_alpha(
                INLINE_WIDGET_PREVIEW_PANE_BORDER_COLOR,
                INLINE_WIDGET_PREVIEW_PANE_BORDER_COLOR[3] * alpha,
            ),
            size,
        );
    }

    let content_rect = Rect {
        x: geometry.preview.x + 5.0,
        y: geometry.preview.y + 4.0 + visual.preview_y_offset_pixels,
        width: (geometry.preview.width - 10.0).max(0.0),
        height: (geometry.preview.height - 8.0).max(0.0),
    };
    push_rounded_rect(
        vertices,
        content_rect,
        (geometry.radius - 2.0).max(1.0),
        with_alpha(
            INLINE_WIDGET_PREVIEW_PANE_CONTENT_COLOR,
            INLINE_WIDGET_PREVIEW_PANE_CONTENT_COLOR[3] * alpha * visual.preview_opacity,
        ),
        size,
    );

    let focus_rect = interpolate_inline_widget_preview_pane_rect(
        geometry.sessions,
        geometry.preview,
        visual.focus_pane_position,
    );
    push_rounded_rect(
        vertices,
        inset_rect(focus_rect, -1.4),
        geometry.radius + 1.4,
        with_alpha(
            INLINE_WIDGET_PREVIEW_PANE_FOCUS_COLOR,
            INLINE_WIDGET_PREVIEW_PANE_FOCUS_COLOR[3] * alpha,
        ),
        size,
    );
}

fn inline_widget_preview_pane_geometry(
    kind: Option<InlineWidgetKind>,
    inline_lines: &[SingleSessionStyledLine],
    line_count: usize,
    typography: &SingleSessionTypography,
    layout: &InlineWidgetCardLayout,
) -> Option<InlineWidgetPreviewPaneGeometry> {
    if kind != Some(InlineWidgetKind::SessionSwitcher) {
        return None;
    }
    if line_count > 0
        && let Some(columns) = session_switcher_split_columns(layout)
    {
        return Some(InlineWidgetPreviewPaneGeometry {
            sessions: columns.rail,
            preview: columns.preview,
            radius: 13.0,
        });
    }
    let visible_len = line_count.min(inline_lines.len());
    let visible_lines = &inline_lines[..visible_len];
    let header_line = visible_lines
        .iter()
        .position(|line| line.text.contains("sessions") && line.text.contains("preview"))?;
    let end_line = visible_lines
        .iter()
        .enumerate()
        .skip(header_line + 1)
        .find_map(|(index, line)| {
            (line.text.contains('╰') || line.text.contains("preview lines ")).then_some(index)
        })
        .unwrap_or(visible_len);

    let line_height = inline_widget_line_height(kind, typography);
    let top = layout.text_top + header_line as f32 * line_height - 2.0;
    let bottom = (layout.text_top + end_line as f32 * line_height + 4.0)
        .min(layout.visible_text_bottom)
        .max(top + line_height);
    let inner_left = layout.card.x + layout.padding_x * 0.72;
    let inner_right = layout.card.x + layout.card.width - layout.padding_x * 0.72;
    let inner_width = (inner_right - inner_left).max(1.0);
    let gap = 10.0_f32.min(inner_width * 0.08);
    let sessions_width = ((inner_width - gap) * 0.42).max(1.0);
    let preview_width = (inner_width - gap - sessions_width).max(1.0);
    let height = bottom - top;

    Some(InlineWidgetPreviewPaneGeometry {
        sessions: Rect {
            x: inner_left,
            y: top,
            width: sessions_width,
            height,
        },
        preview: Rect {
            x: inner_left + sessions_width + gap,
            y: top,
            width: preview_width,
            height,
        },
        radius: 13.0,
    })
}

fn interpolate_inline_widget_preview_pane_rect(
    sessions: Rect,
    preview: Rect,
    position: f32,
) -> Rect {
    let position = position.clamp(0.0, 1.0);
    Rect {
        x: lerp_f32(sessions.x, preview.x, position),
        y: lerp_f32(sessions.y, preview.y, position),
        width: lerp_f32(sessions.width, preview.width, position),
        height: lerp_f32(sessions.height, preview.height, position),
    }
}

#[allow(clippy::too_many_arguments)]
fn push_single_session_inline_widget_structured_chrome(
    vertices: &mut Vec<Vertex>,
    kind: Option<InlineWidgetKind>,
    inline_lines: &[SingleSessionStyledLine],
    line_count: usize,
    typography: &SingleSessionTypography,
    layout: &InlineWidgetCardLayout,
    reveal_progress: f32,
    size: PhysicalSize<u32>,
) {
    match kind {
        Some(InlineWidgetKind::ModelPicker) => push_inline_command_row_cards(
            vertices,
            kind,
            inline_lines,
            line_count,
            typography,
            layout,
            reveal_progress,
            size,
        ),
        Some(InlineWidgetKind::SessionSwitcher) => {
            push_session_switcher_section_panels(
                vertices,
                inline_lines,
                line_count,
                typography,
                layout,
                reveal_progress,
                size,
            );
            push_session_switcher_preview_bubbles(
                vertices,
                inline_lines,
                line_count,
                typography,
                layout,
                reveal_progress,
                size,
            );
            push_inline_command_row_cards(
                vertices,
                kind,
                inline_lines,
                line_count,
                typography,
                layout,
                reveal_progress,
                size,
            );
        }
        _ => {}
    }
}

#[allow(clippy::too_many_arguments)]
fn push_inline_command_row_cards(
    vertices: &mut Vec<Vertex>,
    kind: Option<InlineWidgetKind>,
    inline_lines: &[SingleSessionStyledLine],
    line_count: usize,
    typography: &SingleSessionTypography,
    layout: &InlineWidgetCardLayout,
    reveal_progress: f32,
    size: PhysicalSize<u32>,
) {
    let line_height = inline_widget_line_height(kind, typography);
    for run in inline_widget_list_row_runs(kind, inline_lines, line_count) {
        let primary_text = inline_lines
            .get(run.line)
            .map(|line| line.text.as_str())
            .unwrap_or_default();
        let selected = inline_lines
            .get(run.line)
            .is_some_and(|line| line.style == SingleSessionLineStyle::OverlaySelection);
        let palette = inline_command_row_palette(kind, primary_text, selected);
        push_inline_command_row_card(
            vertices,
            kind,
            run.line,
            run.line_span,
            palette,
            line_height,
            layout,
            reveal_progress,
            size,
        );
        push_inline_command_row_icon(
            vertices,
            kind,
            run.line,
            palette,
            line_height,
            layout,
            reveal_progress,
            size,
        );

        if selected {
            push_inline_command_current_chip(
                vertices,
                kind,
                primary_text,
                run.line,
                line_height,
                layout,
                reveal_progress,
                size,
            );
        }
    }
}

#[allow(clippy::too_many_arguments)]
fn push_inline_command_row_card(
    vertices: &mut Vec<Vertex>,
    kind: Option<InlineWidgetKind>,
    line: usize,
    line_span: usize,
    palette: InlineCommandRowPalette,
    line_height: f32,
    layout: &InlineWidgetCardLayout,
    reveal_progress: f32,
    size: PhysicalSize<u32>,
) {
    let is_session = matches!(kind, Some(InlineWidgetKind::SessionSwitcher));
    let row_top = layout.text_top
        + line as f32 * line_height
        + if is_session {
            INLINE_COMMAND_SESSION_ROW_TOP_INSET
        } else {
            -INLINE_COMMAND_ROW_GAP_Y
        };
    let row_height = (line_span as f32 * line_height
        + if is_session {
            -INLINE_COMMAND_SESSION_ROW_BOTTOM_INSET
        } else {
            INLINE_COMMAND_ROW_GAP_Y * 1.4
        })
    .max(line_height * 0.9);
    let visible_height = (layout.visible_text_bottom - row_top).min(row_height);
    let row_width = (layout.card.width - INLINE_COMMAND_ROW_INSET_X * 2.0).max(0.0);
    if visible_height <= 4.0 || row_width <= 12.0 {
        return;
    }

    let rect = if is_session {
        session_switcher_split_columns(layout)
            .map(|columns| Rect {
                x: columns.rail.x + INLINE_COMMAND_ROW_INSET_X,
                y: row_top,
                width: (columns.rail.width - INLINE_COMMAND_ROW_INSET_X * 2.0).max(0.0),
                height: visible_height,
            })
            .unwrap_or(Rect {
                x: layout.card.x + INLINE_COMMAND_ROW_INSET_X,
                y: row_top,
                width: row_width,
                height: visible_height,
            })
    } else {
        Rect {
            x: layout.card.x + INLINE_COMMAND_ROW_INSET_X,
            y: row_top,
            width: row_width,
            height: visible_height,
        }
    };
    if rect.width <= 12.0 {
        return;
    }
    push_rounded_rect(
        vertices,
        rect,
        INLINE_COMMAND_ROW_RADIUS,
        with_alpha(palette.fill, palette.fill[3] * reveal_progress),
        size,
    );
    push_rounded_rect_border(
        vertices,
        rect,
        INLINE_COMMAND_ROW_RADIUS,
        1.0,
        with_alpha(palette.border, palette.border[3] * reveal_progress),
        size,
    );
    if palette.selected {
        push_rounded_rect(
            vertices,
            Rect {
                x: rect.x + 6.0,
                y: rect.y + 7.0,
                width: 3.0,
                height: (rect.height - 14.0).max(1.0),
            },
            2.0,
            with_alpha(palette.accent, palette.accent[3] * reveal_progress),
            size,
        );
    }
}

#[allow(clippy::too_many_arguments)]
fn push_inline_command_row_icon(
    vertices: &mut Vec<Vertex>,
    kind: Option<InlineWidgetKind>,
    line: usize,
    palette: InlineCommandRowPalette,
    line_height: f32,
    layout: &InlineWidgetCardLayout,
    reveal_progress: f32,
    size: PhysicalSize<u32>,
) {
    let Some(icon) = palette.icon else {
        return;
    };
    let is_session = matches!(kind, Some(InlineWidgetKind::SessionSwitcher));
    let icon_size = if is_session { 19.0 } else { 17.0 };
    let top = layout.text_top + line as f32 * line_height + if is_session { 10.0 } else { 4.0 };
    let left = if is_session {
        session_switcher_split_columns(layout)
            .map(|columns| columns.rail.x + INLINE_COMMAND_ROW_INSET_X + 10.0)
            .unwrap_or(layout.card.x + INLINE_COMMAND_ROW_INSET_X + 10.0)
    } else {
        layout.card.x + layout.card.width - INLINE_COMMAND_ROW_INSET_X - icon_size - 10.0
    };
    if top + icon_size > layout.visible_text_bottom || left + icon_size > layout.visible_text_right
    {
        return;
    }
    if is_session {
        let halo = Rect {
            x: left - 5.0,
            y: top - 5.0,
            width: icon_size + 10.0,
            height: icon_size + 10.0,
        };
        push_rounded_rect(
            vertices,
            halo,
            halo.height * 0.5,
            with_alpha(
                palette.icon_background,
                palette.icon_background[3] * reveal_progress,
            ),
            size,
        );
    }
    push_lucide_icon(
        vertices,
        icon,
        Rect {
            x: left,
            y: top,
            width: icon_size,
            height: icon_size,
        },
        with_alpha(palette.icon_color, palette.icon_color[3] * reveal_progress),
        if is_session { 1.75 } else { 1.55 },
        size,
    );
}

fn push_inline_command_current_chip(
    vertices: &mut Vec<Vertex>,
    kind: Option<InlineWidgetKind>,
    primary_text: &str,
    line: usize,
    line_height: f32,
    layout: &InlineWidgetCardLayout,
    reveal_progress: f32,
    size: PhysicalSize<u32>,
) {
    let chip_width = (layout.card.width * 0.16).clamp(54.0, 98.0);
    let chip_height = (line_height * 0.74).clamp(14.0, 22.0);
    let x = if matches!(kind, Some(InlineWidgetKind::SessionSwitcher)) {
        session_switcher_split_columns(layout)
            .map(|columns| {
                columns.rail.x + columns.rail.width - chip_width - INLINE_COMMAND_ROW_INSET_X - 10.0
            })
            .unwrap_or(
                layout.card.x + layout.card.width - chip_width - INLINE_COMMAND_ROW_INSET_X - 10.0,
            )
    } else {
        layout.card.x + layout.card.width - chip_width - INLINE_COMMAND_ROW_INSET_X - 10.0
    };
    let y = layout.text_top + line as f32 * line_height + (line_height - chip_height) * 0.5;
    if x <= layout.text_left || y + chip_height > layout.visible_text_bottom {
        return;
    }
    push_rounded_rect(
        vertices,
        Rect {
            x,
            y,
            width: chip_width,
            height: chip_height,
        },
        chip_height * 0.5,
        with_alpha(
            INLINE_COMMAND_CHIP_COLOR,
            INLINE_COMMAND_CHIP_COLOR[3] * reveal_progress,
        ),
        size,
    );
    let chip_icon = if matches!(kind, Some(InlineWidgetKind::SessionSwitcher))
        && resume_session_row_is_current(primary_text)
    {
        LucideIcon::BookmarkCheck
    } else {
        LucideIcon::CircleCheck
    };
    let icon_size = chip_height * 0.62;
    push_lucide_icon(
        vertices,
        chip_icon,
        Rect {
            x: x + (chip_width - icon_size) * 0.5,
            y: y + (chip_height - icon_size) * 0.5,
            width: icon_size,
            height: icon_size,
        },
        with_alpha(
            INLINE_COMMAND_CHIP_ICON_COLOR,
            INLINE_COMMAND_CHIP_ICON_COLOR[3] * reveal_progress,
        ),
        1.35,
        size,
    );
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
enum LucideIcon {
    Bot,
    BookmarkCheck,
    CircleCheck,
    CirclePlay,
    CircleX,
    MessageSquare,
    Package,
    RefreshCw,
}

#[derive(Clone, Copy, Debug)]
struct InlineCommandRowPalette {
    fill: [f32; 4],
    border: [f32; 4],
    accent: [f32; 4],
    icon_background: [f32; 4],
    icon_color: [f32; 4],
    icon: Option<LucideIcon>,
    selected: bool,
}

fn inline_command_row_palette(
    kind: Option<InlineWidgetKind>,
    primary_text: &str,
    selected: bool,
) -> InlineCommandRowPalette {
    if matches!(kind, Some(InlineWidgetKind::SessionSwitcher)) {
        return resume_session_row_palette(primary_text, selected);
    }

    InlineCommandRowPalette {
        fill: if selected {
            INLINE_COMMAND_ROW_SELECTED_COLOR
        } else {
            INLINE_COMMAND_ROW_BACKGROUND_COLOR
        },
        border: if selected {
            INLINE_COMMAND_ROW_SELECTED_BORDER_COLOR
        } else {
            INLINE_COMMAND_ROW_BORDER_COLOR
        },
        accent: INLINE_COMMAND_ROW_ACCENT_COLOR,
        icon_background: INLINE_COMMAND_MODEL_ICON_BACKGROUND_COLOR,
        icon_color: INLINE_COMMAND_MODEL_ICON_COLOR,
        icon: matches!(kind, Some(InlineWidgetKind::ModelPicker)).then_some(LucideIcon::Bot),
        selected,
    }
}

fn resume_session_row_palette(primary_text: &str, selected: bool) -> InlineCommandRowPalette {
    let status = resume_session_status_from_row(primary_text);
    let (fill, border, accent, icon_background, icon_color, icon) = match status {
        "active" => (
            RESUME_SESSION_ACTIVE_FILL,
            RESUME_SESSION_ACTIVE_BORDER,
            RESUME_SESSION_ACTIVE_ACCENT,
            RESUME_SESSION_ACTIVE_ICON_BACKGROUND,
            RESUME_SESSION_ACTIVE_ICON_COLOR,
            LucideIcon::CirclePlay,
        ),
        "closed" | "done" | "finished" => (
            RESUME_SESSION_CLOSED_FILL,
            RESUME_SESSION_CLOSED_BORDER,
            RESUME_SESSION_CLOSED_ACCENT,
            RESUME_SESSION_CLOSED_ICON_BACKGROUND,
            RESUME_SESSION_CLOSED_ICON_COLOR,
            LucideIcon::CircleCheck,
        ),
        "crashed" | "failed" | "error" => (
            RESUME_SESSION_ERROR_FILL,
            RESUME_SESSION_ERROR_BORDER,
            RESUME_SESSION_ERROR_ACCENT,
            RESUME_SESSION_ERROR_ICON_BACKGROUND,
            RESUME_SESSION_ERROR_ICON_COLOR,
            LucideIcon::CircleX,
        ),
        "compacted" => (
            RESUME_SESSION_SPECIAL_FILL,
            RESUME_SESSION_SPECIAL_BORDER,
            RESUME_SESSION_SPECIAL_ACCENT,
            RESUME_SESSION_SPECIAL_ICON_BACKGROUND,
            RESUME_SESSION_SPECIAL_ICON_COLOR,
            LucideIcon::Package,
        ),
        "reloaded" => (
            RESUME_SESSION_RELOADED_FILL,
            RESUME_SESSION_RELOADED_BORDER,
            RESUME_SESSION_RELOADED_ACCENT,
            RESUME_SESSION_RELOADED_ICON_BACKGROUND,
            RESUME_SESSION_RELOADED_ICON_COLOR,
            LucideIcon::RefreshCw,
        ),
        _ => (
            RESUME_SESSION_NEUTRAL_FILL,
            RESUME_SESSION_NEUTRAL_BORDER,
            RESUME_SESSION_NEUTRAL_ACCENT,
            RESUME_SESSION_NEUTRAL_ICON_BACKGROUND,
            RESUME_SESSION_NEUTRAL_ICON_COLOR,
            LucideIcon::MessageSquare,
        ),
    };

    InlineCommandRowPalette {
        fill: if selected {
            mix_rgba(fill, RESUME_SESSION_SELECTED_TINT, 0.58)
        } else {
            fill
        },
        border: if selected {
            mix_rgba(border, RESUME_SESSION_SELECTED_BORDER_TINT, 0.46)
        } else {
            border
        },
        accent,
        icon_background: if selected {
            mix_rgba(icon_background, RESUME_SESSION_SELECTED_TINT, 0.28)
        } else {
            icon_background
        },
        icon_color,
        icon: Some(icon),
        selected,
    }
}

fn resume_session_status_from_row(primary_text: &str) -> &str {
    primary_text
        .trim_start()
        .split_once(" session ·")
        .map(|(status, _)| status.trim())
        .unwrap_or("unknown")
}

fn resume_session_row_is_current(primary_text: &str) -> bool {
    primary_text.contains(" current ·")
}

fn mix_rgba(left: [f32; 4], right: [f32; 4], amount: f32) -> [f32; 4] {
    let amount = amount.clamp(0.0, 1.0);
    [
        left[0] + (right[0] - left[0]) * amount,
        left[1] + (right[1] - left[1]) * amount,
        left[2] + (right[2] - left[2]) * amount,
        left[3] + (right[3] - left[3]) * amount,
    ]
}

fn push_lucide_icon(
    vertices: &mut Vec<Vertex>,
    icon: LucideIcon,
    rect: Rect,
    color: [f32; 4],
    stroke_width: f32,
    size: PhysicalSize<u32>,
) {
    if rect.width <= 1.0 || rect.height <= 1.0 || color[3] <= 0.0 {
        return;
    }

    match icon {
        LucideIcon::Bot => {
            push_lucide_rect(
                vertices,
                rect,
                [5.0, 7.0],
                [19.0, 19.0],
                color,
                stroke_width,
                size,
            );
            push_lucide_line(
                vertices,
                rect,
                [12.0, 3.5],
                [12.0, 7.0],
                color,
                stroke_width,
                size,
            );
            push_lucide_line(
                vertices,
                rect,
                [9.0, 3.5],
                [15.0, 3.5],
                color,
                stroke_width,
                size,
            );
            push_lucide_line(
                vertices,
                rect,
                [8.5, 12.0],
                [8.6, 12.0],
                color,
                stroke_width * 2.0,
                size,
            );
            push_lucide_line(
                vertices,
                rect,
                [15.4, 12.0],
                [15.5, 12.0],
                color,
                stroke_width * 2.0,
                size,
            );
            push_lucide_line(
                vertices,
                rect,
                [9.0, 16.0],
                [15.0, 16.0],
                color,
                stroke_width,
                size,
            );
        }
        LucideIcon::BookmarkCheck => {
            push_lucide_polyline(
                vertices,
                rect,
                &[
                    [7.0, 4.0],
                    [17.0, 4.0],
                    [17.0, 20.0],
                    [12.0, 17.0],
                    [7.0, 20.0],
                    [7.0, 4.0],
                ],
                color,
                stroke_width,
                size,
            );
            push_lucide_polyline(
                vertices,
                rect,
                &[[9.3, 11.6], [11.2, 13.5], [15.0, 9.7]],
                color,
                stroke_width,
                size,
            );
        }
        LucideIcon::CircleCheck => {
            push_lucide_circle(vertices, rect, [12.0, 12.0], 8.2, color, stroke_width, size);
            push_lucide_polyline(
                vertices,
                rect,
                &[[8.3, 12.2], [10.8, 14.7], [15.9, 9.4]],
                color,
                stroke_width,
                size,
            );
        }
        LucideIcon::CirclePlay => {
            push_lucide_circle(vertices, rect, [12.0, 12.0], 8.2, color, stroke_width, size);
            push_lucide_polyline(
                vertices,
                rect,
                &[[10.2, 8.7], [15.9, 12.0], [10.2, 15.3], [10.2, 8.7]],
                color,
                stroke_width,
                size,
            );
        }
        LucideIcon::CircleX => {
            push_lucide_circle(vertices, rect, [12.0, 12.0], 8.2, color, stroke_width, size);
            push_lucide_line(
                vertices,
                rect,
                [9.2, 9.2],
                [14.8, 14.8],
                color,
                stroke_width,
                size,
            );
            push_lucide_line(
                vertices,
                rect,
                [14.8, 9.2],
                [9.2, 14.8],
                color,
                stroke_width,
                size,
            );
        }
        LucideIcon::MessageSquare => {
            push_lucide_polyline(
                vertices,
                rect,
                &[
                    [5.0, 6.0],
                    [19.0, 6.0],
                    [19.0, 16.0],
                    [13.0, 16.0],
                    [8.0, 20.0],
                    [8.0, 16.0],
                    [5.0, 16.0],
                    [5.0, 6.0],
                ],
                color,
                stroke_width,
                size,
            );
            push_lucide_line(
                vertices,
                rect,
                [8.5, 10.0],
                [15.5, 10.0],
                color,
                stroke_width,
                size,
            );
            push_lucide_line(
                vertices,
                rect,
                [8.5, 13.0],
                [13.0, 13.0],
                color,
                stroke_width,
                size,
            );
        }
        LucideIcon::Package => {
            push_lucide_polyline(
                vertices,
                rect,
                &[
                    [12.0, 3.8],
                    [19.0, 7.8],
                    [19.0, 16.2],
                    [12.0, 20.2],
                    [5.0, 16.2],
                    [5.0, 7.8],
                    [12.0, 3.8],
                ],
                color,
                stroke_width,
                size,
            );
            push_lucide_polyline(
                vertices,
                rect,
                &[[5.3, 8.0], [12.0, 12.0], [18.7, 8.0]],
                color,
                stroke_width,
                size,
            );
            push_lucide_line(
                vertices,
                rect,
                [12.0, 12.0],
                [12.0, 20.0],
                color,
                stroke_width,
                size,
            );
        }
        LucideIcon::RefreshCw => {
            push_lucide_arc(
                vertices,
                rect,
                [12.0, 12.0],
                7.4,
                -0.10,
                3.55,
                color,
                stroke_width,
                size,
            );
            push_lucide_arc(
                vertices,
                rect,
                [12.0, 12.0],
                7.4,
                3.05,
                6.70,
                color,
                stroke_width,
                size,
            );
            push_lucide_polyline(
                vertices,
                rect,
                &[[17.0, 4.2], [19.4, 7.0], [15.6, 7.2]],
                color,
                stroke_width,
                size,
            );
            push_lucide_polyline(
                vertices,
                rect,
                &[[7.0, 19.8], [4.6, 17.0], [8.4, 16.8]],
                color,
                stroke_width,
                size,
            );
        }
    }
}

fn lucide_point(rect: Rect, point: [f32; 2]) -> [f32; 2] {
    [
        rect.x + rect.width * point[0] / 24.0,
        rect.y + rect.height * point[1] / 24.0,
    ]
}

fn push_lucide_line(
    vertices: &mut Vec<Vertex>,
    rect: Rect,
    a: [f32; 2],
    b: [f32; 2],
    color: [f32; 4],
    stroke_width: f32,
    size: PhysicalSize<u32>,
) {
    push_stroke_segment(
        vertices,
        lucide_point(rect, a),
        lucide_point(rect, b),
        stroke_width,
        color,
        size,
    );
}

fn push_lucide_polyline(
    vertices: &mut Vec<Vertex>,
    rect: Rect,
    points: &[[f32; 2]],
    color: [f32; 4],
    stroke_width: f32,
    size: PhysicalSize<u32>,
) {
    for pair in points.windows(2) {
        push_lucide_line(vertices, rect, pair[0], pair[1], color, stroke_width, size);
    }
}

fn push_lucide_rect(
    vertices: &mut Vec<Vertex>,
    rect: Rect,
    min: [f32; 2],
    max: [f32; 2],
    color: [f32; 4],
    stroke_width: f32,
    size: PhysicalSize<u32>,
) {
    push_lucide_polyline(
        vertices,
        rect,
        &[
            [min[0], min[1]],
            [max[0], min[1]],
            [max[0], max[1]],
            [min[0], max[1]],
            [min[0], min[1]],
        ],
        color,
        stroke_width,
        size,
    );
}

fn push_lucide_circle(
    vertices: &mut Vec<Vertex>,
    rect: Rect,
    center: [f32; 2],
    radius: f32,
    color: [f32; 4],
    stroke_width: f32,
    size: PhysicalSize<u32>,
) {
    push_lucide_arc(
        vertices,
        rect,
        center,
        radius,
        0.0,
        std::f32::consts::TAU,
        color,
        stroke_width,
        size,
    );
}

#[allow(clippy::too_many_arguments)]
fn push_lucide_arc(
    vertices: &mut Vec<Vertex>,
    rect: Rect,
    center: [f32; 2],
    radius: f32,
    start_angle: f32,
    end_angle: f32,
    color: [f32; 4],
    stroke_width: f32,
    size: PhysicalSize<u32>,
) {
    const ICON_ARC_SEGMENTS: usize = 18;
    let mut previous = None;
    for step in 0..=ICON_ARC_SEGMENTS {
        let t = step as f32 / ICON_ARC_SEGMENTS as f32;
        let angle = start_angle + (end_angle - start_angle) * t;
        let point = [
            center[0] + radius * angle.cos(),
            center[1] + radius * angle.sin(),
        ];
        if let Some(previous) = previous {
            push_lucide_line(vertices, rect, previous, point, color, stroke_width, size);
        }
        previous = Some(point);
    }
}

#[derive(Clone, Copy, Debug)]
struct SessionSwitcherSplitColumns {
    rail: Rect,
    preview: Rect,
    gap: Rect,
}

fn session_switcher_split_columns(
    layout: &InlineWidgetCardLayout,
) -> Option<SessionSwitcherSplitColumns> {
    let content_x = layout.card.x + layout.padding_x * 0.72;
    let content_width = (layout.card.width - layout.padding_x * 1.44).max(0.0);
    if content_width <= 260.0 {
        return None;
    }

    let gap_width = (content_width * 0.018).clamp(9.0, 15.0);
    let preferred_rail_width = (content_width * 0.38).clamp(250.0, 365.0);
    let max_rail_width = (content_width - gap_width - 210.0)
        .max(content_width * 0.42)
        .min(content_width - gap_width - 96.0);
    let rail_width = preferred_rail_width
        .min(max_rail_width)
        .max((content_width * 0.32).min(content_width - gap_width - 96.0));
    let preview_width = content_width - rail_width - gap_width;
    if rail_width <= 96.0 || preview_width <= 96.0 {
        return None;
    }

    let y = layout.card.y + layout.padding_x * 0.18;
    let height = (layout.card.height - layout.padding_x * 0.36).max(1.0);
    let rail = Rect {
        x: content_x,
        y,
        width: rail_width,
        height,
    };
    let gap = Rect {
        x: rail.x + rail.width,
        y,
        width: gap_width,
        height,
    };
    let preview = Rect {
        x: gap.x + gap.width,
        y,
        width: preview_width,
        height,
    };
    Some(SessionSwitcherSplitColumns { rail, preview, gap })
}

fn session_switcher_split_panel_rects(
    layout: &InlineWidgetCardLayout,
    top: f32,
    height: f32,
) -> Option<(Rect, Rect, Rect)> {
    let columns = session_switcher_split_columns(layout)?;
    let bottom = (top + height).min(layout.visible_text_bottom);
    if bottom <= top + 8.0 {
        return None;
    }
    let height = bottom - top;
    Some((
        Rect {
            y: top,
            height,
            ..columns.rail
        },
        Rect {
            y: top,
            height,
            ..columns.preview
        },
        Rect {
            y: top,
            height,
            ..columns.gap
        },
    ))
}

#[allow(clippy::too_many_arguments)]
fn push_session_switcher_section_panels(
    vertices: &mut Vec<Vertex>,
    inline_lines: &[SingleSessionStyledLine],
    line_count: usize,
    typography: &SingleSessionTypography,
    layout: &InlineWidgetCardLayout,
    reveal_progress: f32,
    size: PhysicalSize<u32>,
) {
    let visible_len = line_count.min(inline_lines.len());
    let Some(sessions_header) = inline_lines[..visible_len]
        .iter()
        .position(|line| line.text.starts_with("Recent sessions"))
    else {
        return;
    };
    let preview_header = inline_lines[..visible_len]
        .iter()
        .position(|line| line.text.starts_with("Preview"));
    let sessions_end = preview_header
        .unwrap_or(visible_len)
        .max(sessions_header + 1);
    let line_height =
        inline_widget_line_height(Some(InlineWidgetKind::SessionSwitcher), typography);

    let top = layout.text_top + sessions_header as f32 * line_height - 7.0;
    let height = (visible_len - sessions_header) as f32 * line_height + 12.0;
    if let Some((rail, preview, gap)) = session_switcher_split_panel_rects(layout, top, height) {
        push_rounded_rect(
            vertices,
            rail,
            INLINE_COMMAND_ROW_RADIUS + 4.0,
            with_alpha(
                INLINE_COMMAND_SECTION_BACKGROUND_COLOR,
                INLINE_COMMAND_SECTION_BACKGROUND_COLOR[3] * reveal_progress,
            ),
            size,
        );
        push_rounded_rect(
            vertices,
            preview,
            INLINE_COMMAND_ROW_RADIUS + 4.0,
            with_alpha(
                INLINE_COMMAND_PREVIEW_BACKGROUND_COLOR,
                INLINE_COMMAND_PREVIEW_BACKGROUND_COLOR[3] * reveal_progress,
            ),
            size,
        );
        push_rounded_rect(
            vertices,
            Rect {
                x: gap.x + gap.width * 0.5 - 0.5,
                y: gap.y + 9.0,
                width: 1.0,
                height: (gap.height - 18.0).max(1.0),
            },
            0.5,
            with_alpha(
                INLINE_COMMAND_SPLIT_DIVIDER_COLOR,
                INLINE_COMMAND_SPLIT_DIVIDER_COLOR[3] * reveal_progress,
            ),
            size,
        );
    } else {
        push_inline_command_section_panel(
            vertices,
            sessions_header,
            sessions_end,
            line_height,
            layout,
            INLINE_COMMAND_SECTION_BACKGROUND_COLOR,
            reveal_progress,
            size,
        );
        if let Some(preview_header) = preview_header {
            push_inline_command_section_panel(
                vertices,
                preview_header,
                visible_len,
                line_height,
                layout,
                INLINE_COMMAND_PREVIEW_BACKGROUND_COLOR,
                reveal_progress,
                size,
            );
        }
    }
}

#[allow(clippy::too_many_arguments)]
fn push_session_switcher_preview_bubbles(
    vertices: &mut Vec<Vertex>,
    inline_lines: &[SingleSessionStyledLine],
    line_count: usize,
    typography: &SingleSessionTypography,
    layout: &InlineWidgetCardLayout,
    reveal_progress: f32,
    size: PhysicalSize<u32>,
) {
    let visible_len = line_count.min(inline_lines.len());
    let Some(preview_header) = inline_lines[..visible_len]
        .iter()
        .position(|line| line.text.starts_with("Preview"))
    else {
        return;
    };
    let line_height =
        inline_widget_line_height(Some(InlineWidgetKind::SessionSwitcher), typography);
    let radius = (line_height * 0.12).clamp(2.5, 4.5);
    let y = layout.text_top + preview_header as f32 * line_height + line_height * 0.44;
    let right = layout.card.x + layout.card.width - layout.padding_x * 0.72;
    if y + radius > layout.visible_text_bottom {
        return;
    }
    for index in 0..3 {
        let alpha_scale = 1.0 - index as f32 * 0.18;
        push_rounded_rect(
            vertices,
            Rect {
                x: right - (index as f32 + 1.0) * (radius * 2.7),
                y: y - radius,
                width: radius * 2.0,
                height: radius * 2.0,
            },
            radius,
            with_alpha(
                INLINE_COMMAND_ROW_ACCENT_COLOR,
                INLINE_COMMAND_ROW_ACCENT_COLOR[3] * reveal_progress * alpha_scale,
            ),
            size,
        );
    }
}

#[allow(clippy::too_many_arguments)]
fn push_inline_command_section_panel(
    vertices: &mut Vec<Vertex>,
    start_line: usize,
    end_line: usize,
    line_height: f32,
    layout: &InlineWidgetCardLayout,
    color: [f32; 4],
    reveal_progress: f32,
    size: PhysicalSize<u32>,
) {
    if end_line <= start_line {
        return;
    }
    let top = layout.text_top + start_line as f32 * line_height - 7.0;
    let height = (end_line - start_line) as f32 * line_height + 12.0;
    let visible_height = (layout.visible_text_bottom - top).min(height);
    if visible_height <= 8.0 {
        return;
    }
    let rect = Rect {
        x: layout.card.x + layout.padding_x * 0.42,
        y: top,
        width: (layout.card.width - layout.padding_x * 0.84).max(1.0),
        height: visible_height,
    };
    push_rounded_rect(
        vertices,
        rect,
        INLINE_COMMAND_ROW_RADIUS + 4.0,
        with_alpha(color, color[3] * reveal_progress),
        size,
    );
}

#[allow(clippy::too_many_arguments)]
fn push_single_session_inline_widget_list_reflow(
    vertices: &mut Vec<Vertex>,
    kind: Option<InlineWidgetKind>,
    inline_lines: &[SingleSessionStyledLine],
    line_count: usize,
    typography: &SingleSessionTypography,
    layout: &InlineWidgetCardLayout,
    reveal_progress: f32,
    inline_list_reflow_motion: Option<&InlineWidgetListReflowMotionFrame>,
    size: PhysicalSize<u32>,
) {
    let Some(motion) = inline_list_reflow_motion else {
        return;
    };
    let line_height = inline_widget_line_height(kind, typography);
    for run in inline_widget_list_row_runs(kind, inline_lines, line_count) {
        if let Some(visual) = motion.visual_for_key(run.key) {
            push_single_session_inline_widget_reflow_row(
                vertices,
                run,
                visual,
                line_height,
                layout,
                reveal_progress,
                size,
            );
        }
    }
    for (run, visual) in motion.exiting() {
        push_single_session_inline_widget_reflow_row(
            vertices,
            *run,
            *visual,
            line_height,
            layout,
            reveal_progress,
            size,
        );
    }
}

fn push_single_session_inline_widget_reflow_row(
    vertices: &mut Vec<Vertex>,
    run: InlineWidgetListRowRun,
    visual: InlineWidgetListReflowVisual,
    line_height: f32,
    layout: &InlineWidgetCardLayout,
    reveal_progress: f32,
    size: PhysicalSize<u32>,
) {
    if visual.opacity <= 0.001 || visual.line_span <= 0.05 {
        return;
    }
    let row_top = layout.text_top
        + (run.line as f32 + visual.y_offset_lines) * line_height
        + inline_widget_selection_top_offset(Some(run.kind));
    let row_height =
        visual.line_span * line_height + inline_widget_selection_extra_height(Some(run.kind));
    let row_visible_height = (layout.visible_text_bottom - row_top).min(row_height);
    let row_width = (layout.card.width - layout.padding_x).max(0.0);
    if row_visible_height <= 3.0 || row_width <= 6.0 {
        return;
    }
    push_rounded_rect(
        vertices,
        Rect {
            x: layout.card.x + layout.padding_x * 0.5,
            y: row_top,
            width: row_width,
            height: row_visible_height.max(1.0),
        },
        layout.selection_radius,
        with_alpha(
            INLINE_WIDGET_LIST_REFLOW_COLOR,
            INLINE_WIDGET_LIST_REFLOW_COLOR[3] * reveal_progress * visual.opacity,
        ),
        size,
    );
}

#[allow(clippy::too_many_arguments)]
fn push_single_session_inline_widget_selection(
    vertices: &mut Vec<Vertex>,
    kind: Option<InlineWidgetKind>,
    inline_lines: &[SingleSessionStyledLine],
    line_count: usize,
    typography: &SingleSessionTypography,
    layout: &InlineWidgetCardLayout,
    reveal_progress: f32,
    inline_selection_motion: Option<&InlineWidgetSelectionMotionFrame>,
    size: PhysicalSize<u32>,
) {
    let Some(target) = inline_widget_selection_target(kind, inline_lines, line_count) else {
        return;
    };
    let visual = inline_selection_motion
        .and_then(|motion| motion.visual_for_target(target))
        .unwrap_or_else(|| InlineWidgetSelectionVisual::settled(target));
    if visual.opacity <= 0.001 || visual.line_span <= 0.05 {
        return;
    }

    let line_height = inline_widget_line_height(kind, typography);
    let row_top = layout.text_top
        + (target.line as f32 + visual.y_offset_lines) * line_height
        + inline_widget_selection_top_offset(kind);
    let row_height = visual.line_span * line_height + inline_widget_selection_extra_height(kind);
    let row_visible_height = (layout.visible_text_bottom - row_top).min(row_height);
    let row_width = (layout.card.width - layout.padding_x).max(0.0);
    if row_visible_height <= 3.0 || row_width <= 6.0 {
        return;
    }

    let color = inline_widget_selection_background_color(kind);
    push_rounded_rect(
        vertices,
        Rect {
            x: layout.card.x + layout.padding_x * 0.5,
            y: row_top,
            width: row_width,
            height: row_visible_height.max(1.0),
        },
        layout.selection_radius,
        with_alpha(color, color[3] * reveal_progress * visual.opacity),
        size,
    );
}

const INLINE_WIDGET_SIDE_GUTTER_EXTRA: f32 = 24.0;
const INLINE_WIDGET_CARD_PADDING_X: f32 = 14.0;
const INLINE_WIDGET_CARD_PADDING_Y: f32 = 8.0;
const INLINE_WIDGET_BODY_GAP: f32 = 8.0;
const INLINE_WIDGET_CARD_RADIUS: f32 = 18.0;
const INLINE_WIDGET_SELECTION_RADIUS: f32 = 10.0;
const SLASH_SUGGESTIONS_INLINE_CARD_PADDING_X: f32 = 8.0;
const SLASH_SUGGESTIONS_INLINE_CARD_PADDING_Y: f32 = 5.0;
const SLASH_SUGGESTIONS_INLINE_CARD_RADIUS: f32 = 13.0;
const SLASH_SUGGESTIONS_INLINE_SELECTION_RADIUS: f32 = 7.0;
const SLASH_SUGGESTIONS_INLINE_FONT_SCALE: f32 = 0.88;
const INLINE_COMMAND_ROW_RADIUS: f32 = 12.0;
const INLINE_COMMAND_ROW_INSET_X: f32 = 9.0;
const INLINE_COMMAND_ROW_GAP_Y: f32 = 4.0;
const INLINE_COMMAND_ROW_BACKGROUND_COLOR: [f32; 4] = [0.972, 0.982, 1.000, 0.42];
const INLINE_COMMAND_ROW_BORDER_COLOR: [f32; 4] = [0.080, 0.170, 0.420, 0.115];
const INLINE_COMMAND_ROW_SELECTED_COLOR: [f32; 4] = [0.830, 0.900, 1.000, 0.58];
const INLINE_COMMAND_ROW_SELECTED_BORDER_COLOR: [f32; 4] = [0.085, 0.300, 0.850, 0.30];
const INLINE_COMMAND_ROW_ACCENT_COLOR: [f32; 4] = [0.100, 0.360, 0.940, 0.50];
const INLINE_COMMAND_SECTION_BACKGROUND_COLOR: [f32; 4] = [0.955, 0.972, 1.000, 0.30];
const INLINE_COMMAND_PREVIEW_BACKGROUND_COLOR: [f32; 4] = [0.985, 0.990, 1.000, 0.34];
const INLINE_COMMAND_SPLIT_DIVIDER_COLOR: [f32; 4] = [0.120, 0.220, 0.440, 0.16];
const INLINE_COMMAND_CHIP_COLOR: [f32; 4] = [0.900, 0.940, 1.000, 0.64];
const INLINE_COMMAND_CHIP_ICON_COLOR: [f32; 4] = [0.085, 0.270, 0.760, 0.92];
const INLINE_COMMAND_MODEL_ICON_BACKGROUND_COLOR: [f32; 4] = [0.890, 0.930, 1.000, 0.54];
const INLINE_COMMAND_MODEL_ICON_COLOR: [f32; 4] = [0.080, 0.260, 0.720, 0.88];
const INLINE_COMMAND_SESSION_ROW_TOP_INSET: f32 = 3.0;
const INLINE_COMMAND_SESSION_ROW_BOTTOM_INSET: f32 = 10.0;
const RESUME_SESSION_SELECTED_TINT: [f32; 4] = [0.835, 0.905, 1.000, 0.66];
const RESUME_SESSION_SELECTED_BORDER_TINT: [f32; 4] = [0.075, 0.290, 0.900, 0.34];
const RESUME_SESSION_ACTIVE_FILL: [f32; 4] = [0.925, 0.992, 0.955, 0.50];
const RESUME_SESSION_ACTIVE_BORDER: [f32; 4] = [0.050, 0.530, 0.300, 0.22];
const RESUME_SESSION_ACTIVE_ACCENT: [f32; 4] = [0.045, 0.650, 0.355, 0.62];
const RESUME_SESSION_ACTIVE_ICON_BACKGROUND: [f32; 4] = [0.790, 0.970, 0.865, 0.54];
const RESUME_SESSION_ACTIVE_ICON_COLOR: [f32; 4] = [0.025, 0.455, 0.250, 0.92];
const RESUME_SESSION_CLOSED_FILL: [f32; 4] = [0.965, 0.978, 0.994, 0.46];
const RESUME_SESSION_CLOSED_BORDER: [f32; 4] = [0.160, 0.235, 0.360, 0.16];
const RESUME_SESSION_CLOSED_ACCENT: [f32; 4] = [0.290, 0.400, 0.560, 0.44];
const RESUME_SESSION_CLOSED_ICON_BACKGROUND: [f32; 4] = [0.905, 0.935, 0.975, 0.50];
const RESUME_SESSION_CLOSED_ICON_COLOR: [f32; 4] = [0.170, 0.260, 0.420, 0.82];
const RESUME_SESSION_ERROR_FILL: [f32; 4] = [1.000, 0.930, 0.930, 0.50];
const RESUME_SESSION_ERROR_BORDER: [f32; 4] = [0.760, 0.120, 0.160, 0.25];
const RESUME_SESSION_ERROR_ACCENT: [f32; 4] = [0.850, 0.120, 0.180, 0.64];
const RESUME_SESSION_ERROR_ICON_BACKGROUND: [f32; 4] = [1.000, 0.820, 0.835, 0.56];
const RESUME_SESSION_ERROR_ICON_COLOR: [f32; 4] = [0.670, 0.060, 0.110, 0.92];
const RESUME_SESSION_SPECIAL_FILL: [f32; 4] = [0.964, 0.940, 1.000, 0.50];
const RESUME_SESSION_SPECIAL_BORDER: [f32; 4] = [0.405, 0.190, 0.780, 0.23];
const RESUME_SESSION_SPECIAL_ACCENT: [f32; 4] = [0.500, 0.245, 0.900, 0.58];
const RESUME_SESSION_SPECIAL_ICON_BACKGROUND: [f32; 4] = [0.900, 0.830, 1.000, 0.54];
const RESUME_SESSION_SPECIAL_ICON_COLOR: [f32; 4] = [0.360, 0.150, 0.720, 0.90];
const RESUME_SESSION_RELOADED_FILL: [f32; 4] = [0.930, 0.982, 1.000, 0.50];
const RESUME_SESSION_RELOADED_BORDER: [f32; 4] = [0.050, 0.470, 0.680, 0.22];
const RESUME_SESSION_RELOADED_ACCENT: [f32; 4] = [0.050, 0.520, 0.760, 0.56];
const RESUME_SESSION_RELOADED_ICON_BACKGROUND: [f32; 4] = [0.800, 0.940, 1.000, 0.52];
const RESUME_SESSION_RELOADED_ICON_COLOR: [f32; 4] = [0.035, 0.370, 0.620, 0.90];
const RESUME_SESSION_NEUTRAL_FILL: [f32; 4] = [0.972, 0.982, 1.000, 0.44];
const RESUME_SESSION_NEUTRAL_BORDER: [f32; 4] = [0.100, 0.170, 0.320, 0.14];
const RESUME_SESSION_NEUTRAL_ACCENT: [f32; 4] = [0.135, 0.280, 0.620, 0.42];
const RESUME_SESSION_NEUTRAL_ICON_BACKGROUND: [f32; 4] = [0.900, 0.930, 1.000, 0.46];
const RESUME_SESSION_NEUTRAL_ICON_COLOR: [f32; 4] = [0.120, 0.220, 0.460, 0.82];

#[derive(Clone, Copy, Debug)]
struct InlineWidgetCardStyle {
    background: [f32; 4],
    border: [f32; 4],
    highlight: [f32; 4],
    accent: [f32; 4],
}

#[derive(Clone, Copy, Debug)]
struct InlineWidgetCardLayout {
    card: Rect,
    radius: f32,
    padding_x: f32,
    selection_radius: f32,
    text_left: f32,
    text_top: f32,
    visible_text_right: f32,
    visible_text_bottom: f32,
}

fn inline_widget_card_layout(
    size: PhysicalSize<u32>,
    kind: Option<InlineWidgetKind>,
    typography: &SingleSessionTypography,
    line_count: usize,
    text_width: f32,
    text_top: f32,
    progress: f32,
) -> Option<InlineWidgetCardLayout> {
    inline_widget_card_layout_with_bottom_limit(
        size,
        kind,
        typography,
        line_count,
        text_width,
        text_top,
        progress,
        single_session_draft_top(size),
    )
}

fn inline_widget_card_layout_with_bottom_limit(
    size: PhysicalSize<u32>,
    kind: Option<InlineWidgetKind>,
    typography: &SingleSessionTypography,
    line_count: usize,
    text_width: f32,
    text_top: f32,
    progress: f32,
    bottom_limit: f32,
) -> Option<InlineWidgetCardLayout> {
    if line_count == 0 {
        return None;
    }

    let progress = progress.clamp(0.0, 1.0);
    if progress <= 0.001 {
        return None;
    }

    let line_height = inline_widget_line_height(kind, typography);
    let padding_x = inline_widget_card_padding_x(kind);
    let padding_y = inline_widget_card_padding_y(kind);
    let text_left = inline_widget_text_left_for_kind(kind, size);
    let text_width = text_width
        .max(line_height * 8.0)
        .min(inline_widget_max_text_width_for_kind(kind, size))
        .max(1.0);
    let text_height = line_count as f32 * line_height;
    let requested_card_height = text_height + padding_y * 2.0;
    let card_y = (text_top - padding_y).max(PANEL_TITLE_TOP_PADDING);
    let draft_top = single_session_draft_top(size);
    let bottom_limit = bottom_limit.min(draft_top);
    let constrained_by_bottom = bottom_limit < draft_top - 0.001;
    let minimum_card_height = if constrained_by_bottom {
        (line_height * 0.72).min(requested_card_height)
    } else {
        (line_height + padding_y * 2.0).min(requested_card_height)
    };
    let available_card_height = if constrained_by_bottom {
        (bottom_limit - card_y).max(1.0)
    } else {
        (bottom_limit - card_y - 8.0).max(minimum_card_height)
    };
    let max_card_height = available_card_height
        .min((size.height as f32 * 0.56).max(line_height * 3.0 + padding_y * 2.0));
    let final_card_height = requested_card_height
        .min(max_card_height)
        .max(minimum_card_height.min(max_card_height));
    let final_card = Rect {
        x: (text_left - padding_x).max(0.0),
        y: card_y,
        width: text_width + padding_x * 2.0,
        height: final_card_height,
    };
    let start_width = (line_height * 2.0).min(final_card.width);
    let start_height = (line_height * 0.72).min(final_card.height);
    let card = Rect {
        x: final_card.x,
        y: final_card.y,
        width: start_width + (final_card.width - start_width) * progress,
        height: start_height + (final_card.height - start_height) * progress,
    };
    let visible_text_right = (card.x + card.width - padding_x)
        .max(text_left)
        .min(text_left + text_width);
    let visible_text_bottom = (card.y + card.height - padding_y)
        .max(text_top)
        .min(text_top + text_height);

    Some(InlineWidgetCardLayout {
        card,
        radius: inline_widget_card_radius(kind),
        padding_x,
        selection_radius: inline_widget_selection_radius(kind),
        text_left,
        text_top,
        visible_text_right,
        visible_text_bottom,
    })
}

fn inline_widget_line_height(
    kind: Option<InlineWidgetKind>,
    typography: &SingleSessionTypography,
) -> f32 {
    match kind {
        Some(InlineWidgetKind::SlashSuggestions) => {
            inline_widget_font_size(kind, typography) * typography.meta_line_height
        }
        _ => typography.body_size * typography.body_line_height,
    }
}

fn inline_widget_text_width_for_lines(
    kind: Option<InlineWidgetKind>,
    lines: &[SingleSessionStyledLine],
    size: PhysicalSize<u32>,
    ui_scale: f32,
) -> f32 {
    let typography = single_session_typography_for_scale(ui_scale);
    let average_char_width = inline_widget_font_size(kind, &typography) * 0.57;
    let max_columns = lines
        .iter()
        .map(|line| inline_widget_visual_columns(&line.text))
        .max()
        .unwrap_or_default() as f32;
    (max_columns * average_char_width)
        .ceil()
        .min(inline_widget_max_text_width_for_kind(kind, size))
}

fn inline_widget_font_size(
    kind: Option<InlineWidgetKind>,
    typography: &SingleSessionTypography,
) -> f32 {
    match kind {
        Some(InlineWidgetKind::SlashSuggestions) => {
            (typography.meta_size * SLASH_SUGGESTIONS_INLINE_FONT_SCALE).max(12.0)
        }
        _ => typography.body_size,
    }
}

fn inline_widget_card_padding_x(kind: Option<InlineWidgetKind>) -> f32 {
    match kind {
        Some(InlineWidgetKind::SlashSuggestions) => SLASH_SUGGESTIONS_INLINE_CARD_PADDING_X,
        _ => INLINE_WIDGET_CARD_PADDING_X,
    }
}

fn inline_widget_card_padding_y(kind: Option<InlineWidgetKind>) -> f32 {
    match kind {
        Some(InlineWidgetKind::SlashSuggestions) => SLASH_SUGGESTIONS_INLINE_CARD_PADDING_Y,
        _ => INLINE_WIDGET_CARD_PADDING_Y,
    }
}

fn inline_widget_card_radius(kind: Option<InlineWidgetKind>) -> f32 {
    match kind {
        Some(InlineWidgetKind::SlashSuggestions) => SLASH_SUGGESTIONS_INLINE_CARD_RADIUS,
        _ => INLINE_WIDGET_CARD_RADIUS,
    }
}

fn inline_widget_selection_radius(kind: Option<InlineWidgetKind>) -> f32 {
    match kind {
        Some(InlineWidgetKind::SlashSuggestions) => SLASH_SUGGESTIONS_INLINE_SELECTION_RADIUS,
        _ => INLINE_WIDGET_SELECTION_RADIUS,
    }
}

fn inline_widget_selection_top_offset(kind: Option<InlineWidgetKind>) -> f32 {
    match kind {
        Some(InlineWidgetKind::SlashSuggestions) => -1.0,
        _ => -2.0,
    }
}

fn inline_widget_selection_extra_height(kind: Option<InlineWidgetKind>) -> f32 {
    match kind {
        Some(InlineWidgetKind::SlashSuggestions) => 2.0,
        _ => 2.0,
    }
}

fn inline_widget_selection_background_color(kind: Option<InlineWidgetKind>) -> [f32; 4] {
    match kind {
        Some(InlineWidgetKind::SlashSuggestions) => {
            SLASH_SUGGESTIONS_INLINE_SELECTION_BACKGROUND_COLOR
        }
        _ => OVERLAY_SELECTION_BACKGROUND_COLOR,
    }
}

fn inline_widget_card_style(kind: Option<InlineWidgetKind>) -> InlineWidgetCardStyle {
    match kind {
        Some(InlineWidgetKind::SlashSuggestions) => InlineWidgetCardStyle {
            background: SLASH_SUGGESTIONS_INLINE_CARD_BACKGROUND_COLOR,
            border: SLASH_SUGGESTIONS_INLINE_CARD_BORDER_COLOR,
            highlight: SLASH_SUGGESTIONS_INLINE_CARD_HIGHLIGHT_COLOR,
            accent: SLASH_SUGGESTIONS_INLINE_CARD_ACCENT_COLOR,
        },
        _ => InlineWidgetCardStyle {
            background: INLINE_WIDGET_CARD_BACKGROUND_COLOR,
            border: INLINE_WIDGET_CARD_BORDER_COLOR,
            highlight: INLINE_WIDGET_CARD_HIGHLIGHT_COLOR,
            accent: INLINE_WIDGET_CARD_ACCENT_COLOR,
        },
    }
}

fn inline_widget_visual_columns(text: &str) -> usize {
    text.chars()
        .map(|ch| match ch {
            '\t' => 4,
            '\u{200d}' | '\u{fe0e}' | '\u{fe0f}' => 0,
            ch if ch.is_control() => 0,
            ch if is_wide_inline_widget_char(ch) => 2,
            _ => 1,
        })
        .sum()
}

fn is_wide_inline_widget_char(ch: char) -> bool {
    matches!(
        ch as u32,
        0x1100..=0x115F
            | 0x2329..=0x232A
            | 0x2E80..=0xA4CF
            | 0xAC00..=0xD7A3
            | 0xF900..=0xFAFF
            | 0xFE10..=0xFE19
            | 0xFE30..=0xFE6F
            | 0xFF00..=0xFF60
            | 0xFFE0..=0xFFE6
            | 0x1F300..=0x1FAFF
    )
}

fn inline_widget_text_left(size: PhysicalSize<u32>) -> f32 {
    let preferred = PANEL_TITLE_LEFT_PADDING + INLINE_WIDGET_SIDE_GUTTER_EXTRA;
    let responsive_max = (size.width as f32 * 0.18).max(PANEL_TITLE_LEFT_PADDING);
    preferred.min(responsive_max).max(PANEL_TITLE_LEFT_PADDING)
}

fn inline_widget_text_left_for_kind(
    kind: Option<InlineWidgetKind>,
    size: PhysicalSize<u32>,
) -> f32 {
    match kind {
        Some(InlineWidgetKind::SlashSuggestions) => PANEL_TITLE_LEFT_PADDING + 4.0,
        _ => inline_widget_text_left(size),
    }
}

fn inline_widget_max_text_width(size: PhysicalSize<u32>) -> f32 {
    let gutter = inline_widget_text_left(size);
    let available_card_width = (size.width as f32 - gutter * 2.0).max(1.0);
    (available_card_width - INLINE_WIDGET_CARD_PADDING_X * 2.0).max(1.0)
}

fn inline_widget_max_text_width_for_kind(
    kind: Option<InlineWidgetKind>,
    size: PhysicalSize<u32>,
) -> f32 {
    match kind {
        Some(InlineWidgetKind::SlashSuggestions) => {
            let left = inline_widget_text_left_for_kind(kind, size);
            let padding_x = inline_widget_card_padding_x(kind);
            (single_session_content_right(size) - left - padding_x).max(1.0)
        }
        _ => inline_widget_max_text_width(size),
    }
}

#[cfg(test)]
pub(crate) fn handwritten_welcome_bounds(size: PhysicalSize<u32>) -> ([f32; 2], [f32; 2]) {
    handwritten_welcome_bounds_for_phrase(size, handwritten_welcome_phrase(0))
}

#[cfg(test)]
fn handwritten_welcome_bounds_for_phrase(
    size: PhysicalSize<u32>,
    phrase: &str,
) -> ([f32; 2], [f32; 2]) {
    handwritten_welcome_bounds_for_phrase_with_scale(size, phrase, 1.0)
}

fn handwritten_welcome_bounds_for_phrase_with_scale(
    size: PhysicalSize<u32>,
    phrase: &str,
    ui_scale: f32,
) -> ([f32; 2], [f32; 2]) {
    let paths = handwritten_welcome_paths_for_phrase(phrase);
    let (source_min, source_max) = stroke_paths_bounds(&paths);
    let source_width = (source_max[0] - source_min[0]).max(1.0);
    let source_height = (source_max[1] - source_min[1]).max(1.0);
    let normal_draft_top = single_session_draft_top(size);
    let target_width = size.width as f32 * 0.68 * ui_scale;
    let scale = target_width / source_width;
    let left = (size.width as f32 - target_width) * 0.5;
    let top = PANEL_BODY_TOP_PADDING + (normal_draft_top - PANEL_BODY_TOP_PADDING) * 0.31;
    (
        [left, top],
        [left + target_width, top + source_height * scale],
    )
}

fn glyph_welcome_hero_bounds(size: PhysicalSize<u32>, ui_scale: f32) -> ([f32; 2], [f32; 2]) {
    let normal_draft_top = single_session_draft_top(size);
    let target_width = size.width as f32 * 0.68 * ui_scale;
    let font_size = glyph_welcome_hero_font_size(size, ui_scale);
    let left = (size.width as f32 - target_width) * 0.5;
    let top = PANEL_BODY_TOP_PADDING + (normal_draft_top - PANEL_BODY_TOP_PADDING) * 0.31;
    ([left, top], [left + target_width, top + font_size * 1.35])
}

fn glyph_welcome_hero_font_size(size: PhysicalSize<u32>, ui_scale: f32) -> f32 {
    let normal_draft_top = single_session_draft_top(size);
    let available_height = (normal_draft_top - PANEL_BODY_TOP_PADDING).max(1.0);
    (available_height * 0.24 * ui_scale).clamp(82.0 * ui_scale, 170.0 * ui_scale)
}

fn stroke_paths_bounds(paths: &[Vec<[f32; 2]>]) -> ([f32; 2], [f32; 2]) {
    let mut min = [f32::INFINITY, f32::INFINITY];
    let mut max = [f32::NEG_INFINITY, f32::NEG_INFINITY];
    for point in paths.iter().flatten() {
        min[0] = min[0].min(point[0]);
        min[1] = min[1].min(point[1]);
        max[0] = max[0].max(point[0]);
        max[1] = max[1].max(point[1]);
    }
    if !min[0].is_finite() || !max[0].is_finite() {
        ([0.0, 0.0], [1.0, 1.0])
    } else {
        (min, max)
    }
}

fn stroke_paths_length(paths: &[Vec<[f32; 2]>]) -> f32 {
    paths
        .iter()
        .map(|path| {
            path.windows(2)
                .map(|pair| distance(pair[0], pair[1]))
                .sum::<f32>()
        })
        .sum()
}

fn distance(a: [f32; 2], b: [f32; 2]) -> f32 {
    ((b[0] - a[0]).powi(2) + (b[1] - a[1]).powi(2)).sqrt()
}

fn lerp_point(a: [f32; 2], b: [f32; 2], t: f32) -> [f32; 2] {
    [a[0] + (b[0] - a[0]) * t, a[1] + (b[1] - a[1]) * t]
}

fn transform_handwriting_point(point: [f32; 2], origin: [f32; 2], scale: f32) -> [f32; 2] {
    [origin[0] + point[0] * scale, origin[1] + point[1] * scale]
}

fn push_stroke_segment(
    vertices: &mut Vec<Vertex>,
    a: [f32; 2],
    b: [f32; 2],
    thickness: f32,
    color: [f32; 4],
    size: PhysicalSize<u32>,
) {
    let dx = b[0] - a[0];
    let dy = b[1] - a[1];
    let length = (dx * dx + dy * dy).sqrt();
    if length <= 0.001 {
        return;
    }
    let nx = -dy / length * thickness * 0.5;
    let ny = dx / length * thickness * 0.5;
    let p0 = [a[0] + nx, a[1] + ny];
    let p1 = [b[0] + nx, b[1] + ny];
    let p2 = [b[0] - nx, b[1] - ny];
    let p3 = [a[0] - nx, a[1] - ny];
    push_pixel_triangle(vertices, p0, p1, p2, color, size);
    push_pixel_triangle(vertices, p0, p2, p3, color, size);
    push_stroke_dot(vertices, a, thickness * 0.52, color, size);
    push_stroke_dot(vertices, b, thickness * 0.52, color, size);
}

fn push_stroke_dot(
    vertices: &mut Vec<Vertex>,
    center: [f32; 2],
    radius: f32,
    color: [f32; 4],
    size: PhysicalSize<u32>,
) {
    let segments = 12;
    for index in 0..segments {
        let a = index as f32 / segments as f32 * std::f32::consts::TAU;
        let b = (index + 1) as f32 / segments as f32 * std::f32::consts::TAU;
        push_pixel_triangle(
            vertices,
            center,
            [center[0] + a.cos() * radius, center[1] + a.sin() * radius],
            [center[0] + b.cos() * radius, center[1] + b.sin() * radius],
            color,
            size,
        );
    }
}

fn push_aurora_ribbon(
    vertices: &mut Vec<Vertex>,
    size: PhysicalSize<u32>,
    center_y: f32,
    height: f32,
    phase: f32,
    left_color: [f32; 4],
    right_color: [f32; 4],
) {
    let width = size.width as f32;
    let segments = 18;
    for segment in 0..segments {
        let a = segment as f32 / segments as f32;
        let b = (segment + 1) as f32 / segments as f32;
        let x0 = -width * 0.08 + a * width * 1.16;
        let x1 = -width * 0.08 + b * width * 1.16;
        let wave0 = (a * std::f32::consts::TAU * 1.35 + phase).sin() * height * 0.23
            + (a * std::f32::consts::TAU * 2.10 + phase * 0.7).cos() * height * 0.10;
        let wave1 = (b * std::f32::consts::TAU * 1.35 + phase).sin() * height * 0.23
            + (b * std::f32::consts::TAU * 2.10 + phase * 0.7).cos() * height * 0.10;
        let color0 = mix_color(left_color, right_color, a);
        let color1 = mix_color(left_color, right_color, b);
        let edge0 = transparent(color0);
        let edge1 = transparent(color1);
        let top0 = [x0, center_y + wave0 - height * 0.55];
        let mid0 = [x0, center_y + wave0];
        let bot0 = [x0, center_y + wave0 + height * 0.55];
        let top1 = [x1, center_y + wave1 - height * 0.55];
        let mid1 = [x1, center_y + wave1];
        let bot1 = [x1, center_y + wave1 + height * 0.55];
        push_gradient_quad(
            vertices, top0, mid0, mid1, top1, edge0, color0, color1, edge1, size,
        );
        push_gradient_quad(
            vertices, mid0, bot0, bot1, mid1, color0, edge0, edge1, color1, size,
        );
    }
}

#[allow(clippy::too_many_arguments)]
fn push_gradient_quad(
    vertices: &mut Vec<Vertex>,
    a: [f32; 2],
    b: [f32; 2],
    c: [f32; 2],
    d: [f32; 2],
    a_color: [f32; 4],
    b_color: [f32; 4],
    c_color: [f32; 4],
    d_color: [f32; 4],
    size: PhysicalSize<u32>,
) {
    push_gradient_triangle(vertices, a, b, c, a_color, b_color, c_color, size);
    push_gradient_triangle(vertices, a, c, d, a_color, c_color, d_color, size);
}

fn mix_color(a: [f32; 4], b: [f32; 4], t: f32) -> [f32; 4] {
    [
        a[0] + (b[0] - a[0]) * t,
        a[1] + (b[1] - a[1]) * t,
        a[2] + (b[2] - a[2]) * t,
        a[3] + (b[3] - a[3]) * t,
    ]
}

#[allow(clippy::too_many_arguments)]
fn push_gradient_triangle(
    vertices: &mut Vec<Vertex>,
    a: [f32; 2],
    b: [f32; 2],
    c: [f32; 2],
    a_color: [f32; 4],
    b_color: [f32; 4],
    c_color: [f32; 4],
    size: PhysicalSize<u32>,
) {
    vertices.extend_from_slice(&[
        Vertex {
            position: pixel_to_ndc(a, size),
            color: a_color,
        },
        Vertex {
            position: pixel_to_ndc(b, size),
            color: b_color,
        },
        Vertex {
            position: pixel_to_ndc(c, size),
            color: c_color,
        },
    ]);
}

fn transparent(mut color: [f32; 4]) -> [f32; 4] {
    color[3] = 0.0;
    color
}

#[derive(Clone, Copy, Debug, PartialEq)]
pub(crate) struct StreamingActivityCueVisual {
    opacity: f32,
    y_offset_pixels: f32,
    scale: f32,
}

impl StreamingActivityCueVisual {
    fn settled() -> Self {
        Self {
            opacity: 1.0,
            y_offset_pixels: 0.0,
            scale: 1.0,
        }
    }

    fn entry(progress: f32) -> Self {
        let eased = ease_out_cubic_local(progress);
        Self {
            opacity: eased,
            y_offset_pixels: STREAMING_ACTIVITY_CUE_ENTRY_OFFSET_PIXELS * (1.0 - eased),
            scale: lerp_f32(STREAMING_ACTIVITY_CUE_ENTRY_SCALE, 1.0, eased),
        }
    }

    fn exit(progress: f32) -> Self {
        let eased = ease_out_cubic_local(progress);
        Self {
            opacity: 1.0 - eased,
            y_offset_pixels: -STREAMING_ACTIVITY_CUE_ENTRY_OFFSET_PIXELS * 0.55 * eased,
            scale: lerp_f32(1.0, 0.975, eased),
        }
    }
}

#[derive(Clone, Debug, Default)]
pub(crate) struct StreamingActivityCueMotionFrame {
    current: Option<StreamingActivityCueVisual>,
    exiting: Option<StreamingActivityCueVisual>,
    active: bool,
    cache_key: u64,
}

impl StreamingActivityCueMotionFrame {
    pub(crate) fn current(&self) -> Option<StreamingActivityCueVisual> {
        self.current
    }

    pub(crate) fn exiting(&self) -> Option<StreamingActivityCueVisual> {
        self.exiting
    }

    pub(crate) fn is_active(&self) -> bool {
        self.active
    }

    pub(crate) fn cache_key(&self) -> u64 {
        self.cache_key
    }
}

#[derive(Default)]
pub(crate) struct StreamingActivityCueMotionRegistry {
    initialized: bool,
    visible: bool,
    entered_at: Option<Instant>,
    exiting_at: Option<Instant>,
}

impl StreamingActivityCueMotionRegistry {
    pub(crate) fn frame(
        &mut self,
        app: &SingleSessionApp,
        now: Instant,
    ) -> StreamingActivityCueMotionFrame {
        self.frame_for_visible(app.has_activity_indicator(), now)
    }

    fn frame_for_visible(
        &mut self,
        visible: bool,
        now: Instant,
    ) -> StreamingActivityCueMotionFrame {
        let reduced_motion = crate::animation::desktop_reduced_motion_enabled();
        if !self.initialized {
            self.initialized = true;
            self.visible = visible;
            self.entered_at = None;
            self.exiting_at = None;
        } else if self.visible != visible {
            if reduced_motion {
                self.entered_at = None;
                self.exiting_at = None;
            } else if visible {
                self.entered_at = Some(now);
                self.exiting_at = None;
            } else {
                self.exiting_at = Some(now);
                self.entered_at = None;
            }
            self.visible = visible;
        }

        if reduced_motion {
            self.entered_at = None;
            self.exiting_at = None;
        }

        let mut active = false;
        let current = if visible {
            let visual = if let Some(started_at) = self.entered_at {
                let (progress, running) = timed_animation_progress(
                    started_at,
                    now,
                    STREAMING_ACTIVITY_CUE_ENTRY_DURATION,
                );
                active |= running;
                if running {
                    StreamingActivityCueVisual::entry(progress)
                } else {
                    self.entered_at = None;
                    StreamingActivityCueVisual::settled()
                }
            } else {
                StreamingActivityCueVisual::settled()
            };
            Some(visual)
        } else {
            None
        };

        let exiting = if !visible {
            self.exiting_at.and_then(|started_at| {
                let (progress, running) =
                    timed_animation_progress(started_at, now, STREAMING_ACTIVITY_CUE_EXIT_DURATION);
                if running {
                    active = true;
                    Some(StreamingActivityCueVisual::exit(progress))
                } else {
                    self.exiting_at = None;
                    None
                }
            })
        } else {
            None
        };

        StreamingActivityCueMotionFrame {
            current,
            exiting,
            active,
            cache_key: streaming_activity_cue_motion_cache_key(current, exiting, active),
        }
    }

    pub(crate) fn clear(&mut self) {
        self.initialized = false;
        self.visible = false;
        self.entered_at = None;
        self.exiting_at = None;
    }
}

fn streaming_activity_cue_motion_cache_key(
    current: Option<StreamingActivityCueVisual>,
    exiting: Option<StreamingActivityCueVisual>,
    active: bool,
) -> u64 {
    let mut hasher = DefaultHasher::new();
    active.hash(&mut hasher);
    current.is_some().hash(&mut hasher);
    if let Some(visual) = current {
        streaming_activity_cue_visual_hash(visual, &mut hasher);
    }
    exiting.is_some().hash(&mut hasher);
    if let Some(visual) = exiting {
        streaming_activity_cue_visual_hash(visual, &mut hasher);
    }
    hasher.finish()
}

fn streaming_activity_cue_visual_hash(
    visual: StreamingActivityCueVisual,
    hasher: &mut impl Hasher,
) {
    hash_f32(visual.opacity, hasher);
    hash_f32(visual.y_offset_pixels, hasher);
    hash_f32(visual.scale, hasher);
}

pub(crate) fn push_streaming_activity_cue(
    vertices: &mut Vec<Vertex>,
    app: &SingleSessionApp,
    size: PhysicalSize<u32>,
    tick: u64,
    viewport: Option<&SingleSessionBodyViewport>,
    motion: Option<&StreamingActivityCueMotionFrame>,
) {
    let current = if app.has_activity_indicator() {
        Some(
            motion
                .and_then(StreamingActivityCueMotionFrame::current)
                .unwrap_or_else(StreamingActivityCueVisual::settled),
        )
    } else {
        None
    };
    let exiting = motion.and_then(StreamingActivityCueMotionFrame::exiting);
    if current.is_none() && exiting.is_none() {
        return;
    }

    if let Some(visual) = exiting {
        push_streaming_activity_cue_visual(vertices, app, size, tick, viewport, visual);
    }
    if let Some(visual) = current {
        push_streaming_activity_cue_visual(vertices, app, size, tick, viewport, visual);
    }
}

fn push_streaming_activity_cue_visual(
    vertices: &mut Vec<Vertex>,
    app: &SingleSessionApp,
    size: PhysicalSize<u32>,
    tick: u64,
    viewport: Option<&SingleSessionBodyViewport>,
    visual: StreamingActivityCueVisual,
) {
    if visual.opacity <= 0.001 || visual.scale <= 0.05 {
        return;
    }
    let typography = single_session_typography_for_scale(app.text_scale());
    let viewport = viewport
        .cloned()
        .unwrap_or_else(|| single_session_body_viewport_for_tick(app, size, tick, 0.0));
    let pill_width = (typography.body_size * 2.05).clamp(26.0, 34.0);
    let pill_height = (typography.body_size * 0.82).clamp(11.0, 15.0);
    let layout = single_session_layout_for_total_lines(app, size, viewport.total_lines);
    let activity_lane = layout.activity_lane.unwrap_or(Rect {
        x: PANEL_TITLE_LEFT_PADDING,
        y: layout.body_bottom(),
        width: layout.body.width,
        height: (layout.draft_top - layout.body_bottom()).max(pill_height),
    });
    let cue_y = activity_lane.y + (activity_lane.height - pill_height).max(0.0) * 0.5;
    let cue_x = activity_lane.x;
    let cue_rect = Rect {
        x: cue_x,
        y: cue_y + visual.y_offset_pixels,
        width: pill_width,
        height: pill_height,
    };
    let cue_rect = scaled_rect(cue_rect, visual.scale);
    push_rounded_rect(
        vertices,
        cue_rect,
        pill_height * 0.5,
        with_alpha(
            STREAMING_ACTIVITY_PILL_COLOR,
            STREAMING_ACTIVITY_PILL_COLOR[3] * visual.opacity,
        ),
        size,
    );
    push_rounded_rect_border(
        vertices,
        cue_rect,
        pill_height * 0.5,
        1.0,
        with_alpha(
            STREAMING_ACTIVITY_PILL_BORDER_COLOR,
            STREAMING_ACTIVITY_PILL_BORDER_COLOR[3] * visual.opacity,
        ),
        size,
    );

    let dot_radius = (typography.body_size * 0.105).clamp(1.8, 2.8);
    let dot_y = cue_rect.y + cue_rect.height * 0.50 - dot_radius;
    let dot_gap = dot_radius * 2.35;
    let dot_total_width = dot_radius * 2.0 * 3.0 + dot_gap * 2.0;
    let dot_start_x = cue_rect.x + (cue_rect.width - dot_total_width) * 0.5;
    for dot in 0..3 {
        let dot_phase = ((tick + dot as u64 * 4) % 18) as f32 / 18.0;
        let dot_pulse = 0.5 + 0.5 * (dot_phase * std::f32::consts::TAU).sin();
        let mut dot_color = NATIVE_SPINNER_HEAD_COLOR;
        let base_alpha = if app.streaming_response.is_empty() {
            0.34
        } else {
            0.46
        };
        dot_color[3] = (base_alpha + 0.38 * dot_pulse).clamp(0.30, 0.86) * visual.opacity;
        push_rounded_rect(
            vertices,
            Rect {
                x: dot_start_x + dot as f32 * (dot_radius * 2.0 + dot_gap),
                y: dot_y,
                width: dot_radius * 2.0,
                height: dot_radius * 2.0,
            },
            dot_radius,
            dot_color,
            size,
        );
    }
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub(crate) struct SingleSessionTranscriptCardRun {
    pub(crate) line: usize,
    pub(crate) line_count: usize,
    pub(crate) style: SingleSessionLineStyle,
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
struct InlineWidgetSelectionTarget {
    kind: InlineWidgetKind,
    line: usize,
    line_span: usize,
}

#[derive(Clone, Copy, Debug, PartialEq)]
pub(crate) struct InlineWidgetSelectionVisual {
    pub(crate) opacity: f32,
    pub(crate) y_offset_lines: f32,
    pub(crate) line_span: f32,
}

impl InlineWidgetSelectionVisual {
    fn settled(target: InlineWidgetSelectionTarget) -> Self {
        Self {
            opacity: 1.0,
            y_offset_lines: 0.0,
            line_span: target.line_span as f32,
        }
    }
}

#[derive(Clone, Copy, Debug)]
struct InlineWidgetSelectionTransition {
    from_line: usize,
    from_line_span: usize,
    started_at: Instant,
}

#[derive(Clone, Debug, Default)]
pub(crate) struct InlineWidgetSelectionMotionFrame {
    target: Option<InlineWidgetSelectionTarget>,
    visual: Option<InlineWidgetSelectionVisual>,
    active: bool,
}

impl InlineWidgetSelectionMotionFrame {
    fn visual_for_target(
        &self,
        target: InlineWidgetSelectionTarget,
    ) -> Option<InlineWidgetSelectionVisual> {
        (self.target == Some(target)).then_some(self.visual?)
    }

    pub(crate) fn is_active(&self) -> bool {
        self.active
    }
}

#[derive(Default)]
pub(crate) struct InlineWidgetSelectionMotionRegistry {
    initialized: bool,
    current: Option<InlineWidgetSelectionTarget>,
    transition: Option<InlineWidgetSelectionTransition>,
}

impl InlineWidgetSelectionMotionRegistry {
    pub(crate) fn frame(
        &mut self,
        app: &SingleSessionApp,
        now: Instant,
    ) -> InlineWidgetSelectionMotionFrame {
        let kind = app.render_inline_widget_kind();
        let lines = app.render_inline_widget_styled_lines();
        let visible_line_count = kind
            .map(|kind| lines.len().min(kind.visible_line_limit()))
            .unwrap_or(0);
        let target = inline_widget_selection_target(kind, &lines, visible_line_count);
        self.frame_for_target(target, now)
    }

    fn frame_for_target(
        &mut self,
        target: Option<InlineWidgetSelectionTarget>,
        now: Instant,
    ) -> InlineWidgetSelectionMotionFrame {
        let Some(target) = target else {
            self.clear();
            return InlineWidgetSelectionMotionFrame::default();
        };

        if !self.initialized {
            self.initialized = true;
            self.current = Some(target);
            self.transition = None;
        } else if self.current != Some(target) {
            self.transition = self.current.and_then(|current| {
                (current.kind == target.kind && !crate::animation::desktop_reduced_motion_enabled())
                    .then_some(InlineWidgetSelectionTransition {
                        from_line: current.line,
                        from_line_span: current.line_span,
                        started_at: now,
                    })
            });
            self.current = Some(target);
        }

        let (visual, active) =
            inline_widget_selection_visual_from_transition(&mut self.transition, target, now);
        InlineWidgetSelectionMotionFrame {
            target: Some(target),
            visual: Some(visual),
            active,
        }
    }

    pub(crate) fn clear(&mut self) {
        self.initialized = false;
        self.current = None;
        self.transition = None;
    }
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
struct InlineWidgetPreviewPaneTarget {
    kind: InlineWidgetKind,
    focus_pane: usize,
    preview_key: u64,
}

#[derive(Clone, Copy, Debug, PartialEq)]
pub(crate) struct InlineWidgetPreviewPaneVisual {
    focus_pane_position: f32,
    preview_opacity: f32,
    preview_y_offset_pixels: f32,
}

impl InlineWidgetPreviewPaneVisual {
    fn settled(target: InlineWidgetPreviewPaneTarget) -> Self {
        Self {
            focus_pane_position: target.focus_pane as f32,
            preview_opacity: 1.0,
            preview_y_offset_pixels: 0.0,
        }
    }
}

#[derive(Clone, Copy, Debug)]
struct InlineWidgetPreviewPaneFocusTransition {
    from_pane: usize,
    started_at: Instant,
}

#[derive(Clone, Debug, Default)]
pub(crate) struct InlineWidgetPreviewPaneMotionFrame {
    visual: Option<InlineWidgetPreviewPaneVisual>,
    active: bool,
    cache_key: u64,
}

impl InlineWidgetPreviewPaneMotionFrame {
    pub(crate) fn visual(&self) -> Option<InlineWidgetPreviewPaneVisual> {
        self.visual
    }

    pub(crate) fn is_active(&self) -> bool {
        self.active
    }

    pub(crate) fn cache_key(&self) -> u64 {
        self.cache_key
    }
}

#[derive(Default)]
pub(crate) struct InlineWidgetPreviewPaneMotionRegistry {
    initialized: bool,
    current: Option<InlineWidgetPreviewPaneTarget>,
    focus_transition: Option<InlineWidgetPreviewPaneFocusTransition>,
    content_started_at: Option<Instant>,
}

impl InlineWidgetPreviewPaneMotionRegistry {
    pub(crate) fn frame(
        &mut self,
        app: &SingleSessionApp,
        now: Instant,
    ) -> InlineWidgetPreviewPaneMotionFrame {
        let kind = app.render_inline_widget_kind();
        let lines = app.render_inline_widget_styled_lines();
        let visible_line_count = app.render_inline_widget_visible_line_count();
        let target = inline_widget_preview_pane_target(kind, &lines, visible_line_count);
        self.frame_for_target(target, now)
    }

    fn frame_for_target(
        &mut self,
        target: Option<InlineWidgetPreviewPaneTarget>,
        now: Instant,
    ) -> InlineWidgetPreviewPaneMotionFrame {
        let Some(target) = target else {
            self.clear();
            return InlineWidgetPreviewPaneMotionFrame::default();
        };

        let reduced_motion = crate::animation::desktop_reduced_motion_enabled();
        if !self.initialized {
            self.initialized = true;
            self.current = Some(target);
            self.focus_transition = None;
            self.content_started_at = None;
        } else if self.current != Some(target) {
            if reduced_motion {
                self.focus_transition = None;
                self.content_started_at = None;
            } else if let Some(current) = self.current {
                if current.focus_pane != target.focus_pane {
                    self.focus_transition = Some(InlineWidgetPreviewPaneFocusTransition {
                        from_pane: current.focus_pane,
                        started_at: now,
                    });
                }
                if current.preview_key != target.preview_key {
                    self.content_started_at = Some(now);
                }
            }
            self.current = Some(target);
        }

        if reduced_motion {
            self.focus_transition = None;
            self.content_started_at = None;
        }

        let (visual, active) = inline_widget_preview_pane_visual_from_state(
            target,
            &mut self.focus_transition,
            &mut self.content_started_at,
            now,
        );
        InlineWidgetPreviewPaneMotionFrame {
            visual: Some(visual),
            active,
            cache_key: inline_widget_preview_pane_cache_key(Some(visual), active),
        }
    }

    pub(crate) fn clear(&mut self) {
        self.initialized = false;
        self.current = None;
        self.focus_transition = None;
        self.content_started_at = None;
    }
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
struct InlineWidgetListRowRun {
    kind: InlineWidgetKind,
    key: u64,
    line: usize,
    line_span: usize,
}

#[derive(Clone, Copy, Debug, PartialEq)]
pub(crate) struct InlineWidgetListReflowVisual {
    opacity: f32,
    y_offset_lines: f32,
    line_span: f32,
}

#[derive(Clone, Copy, Debug)]
struct InlineWidgetListReflowShift {
    from_line: usize,
    from_line_span: usize,
    started_at: Instant,
}

#[derive(Clone, Copy, Debug)]
struct InlineWidgetListReflowState {
    run: InlineWidgetListRowRun,
    entered_at: Option<Instant>,
    exiting_at: Option<Instant>,
    shift: Option<InlineWidgetListReflowShift>,
    last_seen_generation: u64,
}

#[derive(Clone, Debug, Default)]
pub(crate) struct InlineWidgetListReflowMotionFrame {
    visuals: HashMap<u64, InlineWidgetListReflowVisual>,
    exiting: Vec<(InlineWidgetListRowRun, InlineWidgetListReflowVisual)>,
    active: bool,
    cache_key: u64,
}

impl InlineWidgetListReflowMotionFrame {
    fn visual_for_key(&self, key: u64) -> Option<InlineWidgetListReflowVisual> {
        self.visuals.get(&key).copied()
    }

    fn exiting(&self) -> &[(InlineWidgetListRowRun, InlineWidgetListReflowVisual)] {
        &self.exiting
    }

    pub(crate) fn is_active(&self) -> bool {
        self.active
    }

    pub(crate) fn cache_key(&self) -> u64 {
        self.cache_key
    }
}

#[derive(Default)]
pub(crate) struct InlineWidgetListReflowMotionRegistry {
    initialized: bool,
    kind: Option<InlineWidgetKind>,
    generation: u64,
    states: HashMap<u64, InlineWidgetListReflowState>,
}

impl InlineWidgetListReflowMotionRegistry {
    pub(crate) fn frame(
        &mut self,
        app: &SingleSessionApp,
        now: Instant,
    ) -> InlineWidgetListReflowMotionFrame {
        let kind = app.render_inline_widget_kind();
        let lines = app.render_inline_widget_styled_lines();
        let visible_line_count = app.render_inline_widget_visible_line_count();
        self.frame_for_rows(kind, &lines, visible_line_count, now)
    }

    fn frame_for_rows(
        &mut self,
        kind: Option<InlineWidgetKind>,
        lines: &[SingleSessionStyledLine],
        visible_line_count: usize,
        now: Instant,
    ) -> InlineWidgetListReflowMotionFrame {
        let Some(kind) = kind else {
            self.clear();
            return InlineWidgetListReflowMotionFrame::default();
        };

        if self.kind != Some(kind) {
            self.clear();
            self.kind = Some(kind);
        }

        self.generation = self.generation.wrapping_add(1).max(1);
        let generation = self.generation;
        let reduced_motion = crate::animation::desktop_reduced_motion_enabled();
        let animate_new_rows = self.initialized && !reduced_motion;
        self.initialized = true;

        let runs = inline_widget_list_row_runs(Some(kind), lines, visible_line_count);
        let mut visuals = HashMap::new();
        let mut active = false;
        for run in runs {
            let state = self
                .states
                .entry(run.key)
                .or_insert_with(|| InlineWidgetListReflowState {
                    run,
                    entered_at: animate_new_rows.then_some(now),
                    exiting_at: None,
                    shift: None,
                    last_seen_generation: generation,
                });
            state.last_seen_generation = generation;
            state.exiting_at = None;

            if reduced_motion {
                state.entered_at = None;
                state.shift = None;
            }

            if state.run.line != run.line || state.run.line_span != run.line_span {
                if reduced_motion {
                    state.shift = None;
                } else {
                    state.shift = Some(InlineWidgetListReflowShift {
                        from_line: state.run.line,
                        from_line_span: state.run.line_span,
                        started_at: now,
                    });
                }
            }
            state.run = run;

            let (visual, visual_active) = inline_widget_list_reflow_visual_from_state(state, now);
            active |= visual_active;
            if visual.opacity > 0.001 {
                visuals.insert(run.key, visual);
            }
        }

        let mut exiting = Vec::new();
        if !reduced_motion {
            for state in self.states.values_mut() {
                if state.last_seen_generation == generation {
                    continue;
                }
                let exiting_at = *state.exiting_at.get_or_insert(now);
                let (progress, running) = timed_animation_progress(
                    exiting_at,
                    now,
                    INLINE_WIDGET_LIST_REFLOW_EXIT_DURATION,
                );
                if !running {
                    continue;
                }
                state.last_seen_generation = generation;
                active = true;
                exiting.push((
                    state.run,
                    exiting_inline_widget_list_reflow_visual(progress),
                ));
            }
        }

        self.states
            .retain(|_, state| state.last_seen_generation == generation);

        InlineWidgetListReflowMotionFrame {
            cache_key: inline_widget_list_reflow_cache_key(&visuals, &exiting, active),
            visuals,
            exiting,
            active,
        }
    }

    pub(crate) fn clear(&mut self) {
        self.initialized = false;
        self.kind = None;
        self.generation = 0;
        self.states.clear();
    }
}

#[derive(Clone, Copy, Debug, Eq, Hash, PartialEq)]
pub(crate) struct ComposerMotionTarget {
    line_count: usize,
    empty: bool,
    blocked: bool,
    processing: bool,
    ready_to_submit: bool,
}

#[derive(Clone, Copy, Debug, PartialEq)]
pub(crate) struct ComposerMotionVisual {
    height_lines: f32,
    placeholder_opacity: f32,
    focus_opacity: f32,
    blocked_progress: f32,
    submit_opacity: f32,
    submit_scale: f32,
    processing_progress: f32,
}

impl ComposerMotionVisual {
    fn settled(target: ComposerMotionTarget) -> Self {
        Self {
            height_lines: target.line_count.max(1) as f32,
            placeholder_opacity: if target.empty && !target.processing {
                1.0
            } else {
                0.0
            },
            focus_opacity: if target.blocked { 0.28 } else { 1.0 },
            blocked_progress: if target.blocked { 1.0 } else { 0.0 },
            submit_opacity: if target.ready_to_submit || target.processing {
                1.0
            } else {
                0.0
            },
            submit_scale: if target.ready_to_submit || target.processing {
                1.0
            } else {
                0.82
            },
            processing_progress: if target.processing { 1.0 } else { 0.0 },
        }
    }
}

#[derive(Clone, Copy, Debug)]
struct ComposerMotionTransition {
    from: ComposerMotionVisual,
    to: ComposerMotionVisual,
    started_at: Instant,
}

#[derive(Clone, Copy, Debug)]
pub(crate) struct ComposerMotionFrame {
    visual: ComposerMotionVisual,
    active: bool,
    cache_key: u64,
}

impl ComposerMotionFrame {
    pub(crate) fn visual(&self) -> ComposerMotionVisual {
        self.visual
    }

    pub(crate) fn is_active(&self) -> bool {
        self.active
    }

    pub(crate) fn cache_key(&self) -> u64 {
        self.cache_key
    }
}

#[derive(Default)]
pub(crate) struct ComposerMotionRegistry {
    initialized: bool,
    target: Option<ComposerMotionTarget>,
    visual: Option<ComposerMotionVisual>,
    transition: Option<ComposerMotionTransition>,
}

impl ComposerMotionRegistry {
    pub(crate) fn frame(&mut self, app: &SingleSessionApp, now: Instant) -> ComposerMotionFrame {
        self.frame_for_target(composer_motion_target(app), now)
    }

    fn frame_for_target(
        &mut self,
        target: ComposerMotionTarget,
        now: Instant,
    ) -> ComposerMotionFrame {
        let target_visual = ComposerMotionVisual::settled(target);
        let reduced_motion = crate::animation::desktop_reduced_motion_enabled();
        if reduced_motion || !self.initialized {
            self.initialized = true;
            self.target = Some(target);
            self.visual = Some(target_visual);
            self.transition = None;
            return ComposerMotionFrame {
                visual: target_visual,
                active: false,
                cache_key: composer_motion_cache_key(target, target_visual, false),
            };
        }

        if self.target != Some(target) {
            let from = self.current_visual(now);
            self.transition = Some(ComposerMotionTransition {
                from,
                to: target_visual,
                started_at: now,
            });
            self.target = Some(target);
        }

        let mut active = false;
        let visual = if let Some(transition) = self.transition {
            let (progress, running) =
                timed_animation_progress(transition.started_at, now, COMPOSER_MOTION_DURATION);
            let eased = ease_out_cubic_local(progress);
            let visual = composer_motion_visual_lerp(transition.from, transition.to, eased);
            active = running;
            if !running {
                self.transition = None;
            }
            visual
        } else {
            target_visual
        };
        self.visual = Some(visual);

        ComposerMotionFrame {
            visual,
            active,
            cache_key: composer_motion_cache_key(target, visual, active),
        }
    }

    fn current_visual(&mut self, now: Instant) -> ComposerMotionVisual {
        if let Some(transition) = self.transition {
            let (progress, running) =
                timed_animation_progress(transition.started_at, now, COMPOSER_MOTION_DURATION);
            if !running {
                self.transition = None;
                transition.to
            } else {
                composer_motion_visual_lerp(
                    transition.from,
                    transition.to,
                    ease_out_cubic_local(progress),
                )
            }
        } else {
            self.visual
                .or_else(|| self.target.map(ComposerMotionVisual::settled))
                .unwrap_or_else(|| ComposerMotionVisual::settled(ComposerMotionTarget::default()))
        }
    }

    pub(crate) fn clear(&mut self) {
        self.initialized = false;
        self.target = None;
        self.visual = None;
        self.transition = None;
    }
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
struct AttachmentChipRun {
    key: u64,
    index: usize,
}

#[derive(Clone, Copy, Debug, PartialEq)]
pub(crate) struct AttachmentChipVisual {
    opacity: f32,
    x_offset_pixels: f32,
    y_offset_pixels: f32,
    scale: f32,
}

impl AttachmentChipVisual {
    fn settled() -> Self {
        Self {
            opacity: 1.0,
            x_offset_pixels: 0.0,
            y_offset_pixels: 0.0,
            scale: 1.0,
        }
    }
}

#[derive(Clone, Copy, Debug)]
struct AttachmentChipShift {
    from_index: usize,
    started_at: Instant,
}

#[derive(Clone, Copy, Debug)]
struct AttachmentChipState {
    run: AttachmentChipRun,
    entered_at: Option<Instant>,
    exiting_at: Option<Instant>,
    shift: Option<AttachmentChipShift>,
    last_seen_generation: u64,
}

#[derive(Clone, Debug, Default)]
pub(crate) struct AttachmentChipMotionFrame {
    visuals: HashMap<u64, AttachmentChipVisual>,
    exiting: Vec<(AttachmentChipRun, AttachmentChipVisual)>,
    active: bool,
    cache_key: u64,
}

impl AttachmentChipMotionFrame {
    fn visual_for_key(&self, key: u64) -> Option<AttachmentChipVisual> {
        self.visuals.get(&key).copied()
    }

    fn exiting(&self) -> &[(AttachmentChipRun, AttachmentChipVisual)] {
        &self.exiting
    }

    pub(crate) fn is_active(&self) -> bool {
        self.active
    }

    pub(crate) fn cache_key(&self) -> u64 {
        self.cache_key
    }
}

#[derive(Default)]
pub(crate) struct AttachmentChipMotionRegistry {
    initialized: bool,
    generation: u64,
    states: HashMap<u64, AttachmentChipState>,
}

impl AttachmentChipMotionRegistry {
    pub(crate) fn frame(
        &mut self,
        app: &SingleSessionApp,
        now: Instant,
    ) -> AttachmentChipMotionFrame {
        self.frame_for_images(&app.pending_images, now)
    }

    fn frame_for_images(
        &mut self,
        images: &[(String, String)],
        now: Instant,
    ) -> AttachmentChipMotionFrame {
        self.generation = self.generation.wrapping_add(1).max(1);
        let generation = self.generation;
        let reduced_motion = crate::animation::desktop_reduced_motion_enabled();
        let animate_new_chips = self.initialized && !reduced_motion;
        self.initialized = true;

        let runs = attachment_chip_runs(images);
        let mut visuals = HashMap::new();
        let mut active = false;
        for run in runs {
            let state = self
                .states
                .entry(run.key)
                .or_insert_with(|| AttachmentChipState {
                    run,
                    entered_at: animate_new_chips.then_some(now),
                    exiting_at: None,
                    shift: None,
                    last_seen_generation: generation,
                });
            state.last_seen_generation = generation;
            state.exiting_at = None;

            if reduced_motion {
                state.entered_at = None;
                state.shift = None;
            } else if state.run.index != run.index {
                state.shift = Some(AttachmentChipShift {
                    from_index: state.run.index,
                    started_at: now,
                });
            }
            state.run = run;

            let (visual, visual_active) = attachment_chip_visual_from_state(state, now);
            active |= visual_active;
            if visual.opacity > 0.001 {
                visuals.insert(run.key, visual);
            }
        }

        let mut exiting = Vec::new();
        if !reduced_motion {
            for state in self.states.values_mut() {
                if state.last_seen_generation == generation {
                    continue;
                }
                let exiting_at = *state.exiting_at.get_or_insert(now);
                let (progress, running) =
                    timed_animation_progress(exiting_at, now, ATTACHMENT_CHIP_EXIT_DURATION);
                if !running {
                    continue;
                }
                state.last_seen_generation = generation;
                active = true;
                exiting.push((state.run, exiting_attachment_chip_visual(progress)));
            }
        }

        self.states
            .retain(|_, state| state.last_seen_generation == generation);

        AttachmentChipMotionFrame {
            cache_key: attachment_chip_motion_cache_key(&visuals, &exiting, active),
            visuals,
            exiting,
            active,
        }
    }

    pub(crate) fn clear(&mut self) {
        self.initialized = false;
        self.generation = 0;
        self.states.clear();
    }
}

#[derive(Clone, Copy, Debug, Eq, Hash, PartialEq)]
struct StdinOverlayTarget {
    key: u64,
    line_count: usize,
    input_line_start: usize,
    input_line_count: usize,
    password: bool,
    has_input: bool,
}

#[derive(Clone, Copy, Debug, PartialEq)]
pub(crate) struct StdinOverlayVisual {
    opacity: f32,
    y_offset_pixels: f32,
    scale: f32,
    height_lines: f32,
    input_glow: f32,
    submit_opacity: f32,
}

impl StdinOverlayVisual {
    fn settled(target: StdinOverlayTarget) -> Self {
        Self {
            opacity: 1.0,
            y_offset_pixels: 0.0,
            scale: 1.0,
            height_lines: target.line_count.max(1) as f32,
            input_glow: if target.has_input { 1.0 } else { 0.22 },
            submit_opacity: if target.has_input { 1.0 } else { 0.0 },
        }
    }

    fn entry(target: StdinOverlayTarget) -> Self {
        let mut visual = Self::settled(target);
        visual.opacity = 0.0;
        visual.y_offset_pixels = STDIN_OVERLAY_ENTRY_OFFSET_PIXELS;
        visual.scale = STDIN_OVERLAY_ENTRY_SCALE;
        visual.input_glow = 0.0;
        visual.submit_opacity = 0.0;
        visual
    }
}

#[derive(Clone, Copy, Debug)]
struct StdinOverlayTransition {
    from: StdinOverlayVisual,
    to: StdinOverlayVisual,
    started_at: Instant,
    duration: Duration,
}

#[derive(Clone, Copy, Debug)]
struct StdinOverlayExit {
    target: StdinOverlayTarget,
    from: StdinOverlayVisual,
    started_at: Instant,
}

#[derive(Clone, Copy, Debug, Default)]
pub(crate) struct StdinOverlayMotionFrame {
    current: Option<(StdinOverlayTarget, StdinOverlayVisual)>,
    exiting: Option<(StdinOverlayTarget, StdinOverlayVisual)>,
    active: bool,
    cache_key: u64,
}

impl StdinOverlayMotionFrame {
    pub(crate) fn is_active(&self) -> bool {
        self.active
    }

    pub(crate) fn cache_key(&self) -> u64 {
        self.cache_key
    }
}

#[derive(Default)]
pub(crate) struct StdinOverlayMotionRegistry {
    initialized: bool,
    target: Option<StdinOverlayTarget>,
    visual: Option<StdinOverlayVisual>,
    transition: Option<StdinOverlayTransition>,
    exit: Option<StdinOverlayExit>,
}

impl StdinOverlayMotionRegistry {
    pub(crate) fn frame(
        &mut self,
        app: &SingleSessionApp,
        rendered_body_lines: &[SingleSessionStyledLine],
        now: Instant,
    ) -> StdinOverlayMotionFrame {
        self.frame_for_target(stdin_overlay_target(app, rendered_body_lines), now)
    }

    fn frame_for_target(
        &mut self,
        target: Option<StdinOverlayTarget>,
        now: Instant,
    ) -> StdinOverlayMotionFrame {
        let reduced_motion = crate::animation::desktop_reduced_motion_enabled();
        if reduced_motion || !self.initialized {
            self.initialized = true;
            self.target = target;
            self.visual = target.map(StdinOverlayVisual::settled);
            self.transition = None;
            self.exit = None;
            return self.frame_from_state(false, now);
        }

        if self.target != target {
            let from = self
                .current_visual(now)
                .or_else(|| {
                    self.exit
                        .map(|exit| stdin_overlay_exit_visual(exit.from, 0.0))
                })
                .unwrap_or_else(|| {
                    target.map_or_else(
                        || StdinOverlayVisual::entry(StdinOverlayTarget::empty()),
                        StdinOverlayVisual::entry,
                    )
                });
            match (self.target, target) {
                (Some(previous), None) => {
                    self.exit = Some(StdinOverlayExit {
                        target: previous,
                        from,
                        started_at: now,
                    });
                    self.transition = None;
                    self.visual = None;
                    self.target = None;
                }
                (_, Some(next)) => {
                    let entering = self.target.is_none() && self.exit.is_none();
                    let entry_from = if entering {
                        StdinOverlayVisual::entry(next)
                    } else {
                        from
                    };
                    self.exit = None;
                    self.transition = Some(StdinOverlayTransition {
                        from: entry_from,
                        to: StdinOverlayVisual::settled(next),
                        started_at: now,
                        duration: if entering {
                            STDIN_OVERLAY_ENTRY_DURATION
                        } else {
                            STDIN_OVERLAY_RESIZE_DURATION
                        },
                    });
                    self.target = Some(next);
                }
                (None, None) => {}
            }
        }

        self.frame_from_state(false, now)
    }

    fn frame_from_state(&mut self, mut active: bool, now: Instant) -> StdinOverlayMotionFrame {
        let current = if let Some(target) = self.target {
            let visual = if let Some(transition) = self.transition {
                let (progress, running) =
                    timed_animation_progress(transition.started_at, now, transition.duration);
                active |= running;
                if !running {
                    self.transition = None;
                    transition.to
                } else {
                    stdin_overlay_visual_lerp(
                        transition.from,
                        transition.to,
                        ease_out_cubic_local(progress),
                    )
                }
            } else {
                self.visual
                    .unwrap_or_else(|| StdinOverlayVisual::settled(target))
            };
            self.visual = Some(visual);
            Some((target, visual))
        } else {
            None
        };

        let exiting = if let Some(exit) = self.exit {
            let (progress, running) =
                timed_animation_progress(exit.started_at, now, STDIN_OVERLAY_EXIT_DURATION);
            if running {
                active = true;
                Some((exit.target, stdin_overlay_exit_visual(exit.from, progress)))
            } else {
                self.exit = None;
                None
            }
        } else {
            None
        };

        StdinOverlayMotionFrame {
            current,
            exiting,
            active,
            cache_key: stdin_overlay_motion_cache_key(current, exiting, active),
        }
    }

    fn current_visual(&mut self, now: Instant) -> Option<StdinOverlayVisual> {
        if let Some(transition) = self.transition {
            let (progress, running) =
                timed_animation_progress(transition.started_at, now, transition.duration);
            if !running {
                self.transition = None;
                Some(transition.to)
            } else {
                Some(stdin_overlay_visual_lerp(
                    transition.from,
                    transition.to,
                    ease_out_cubic_local(progress),
                ))
            }
        } else {
            self.visual
        }
    }

    pub(crate) fn clear(&mut self) {
        self.initialized = false;
        self.target = None;
        self.visual = None;
        self.transition = None;
        self.exit = None;
    }
}

impl StdinOverlayTarget {
    fn empty() -> Self {
        Self {
            key: 0,
            line_count: 1,
            input_line_start: 0,
            input_line_count: 1,
            password: false,
            has_input: false,
        }
    }
}

impl Default for ComposerMotionTarget {
    fn default() -> Self {
        Self {
            line_count: 1,
            empty: true,
            blocked: false,
            processing: false,
            ready_to_submit: false,
        }
    }
}

#[derive(Clone, Copy, Debug, PartialEq)]
pub(crate) struct SurfaceMotionVisual {
    pub(crate) opacity: f32,
    pub(crate) y_offset_pixels: f32,
    pub(crate) scale: f32,
}

impl Default for SurfaceMotionVisual {
    fn default() -> Self {
        Self {
            opacity: 1.0,
            y_offset_pixels: 0.0,
            scale: 1.0,
        }
    }
}

impl SurfaceMotionVisual {
    fn entry(entry_offset_pixels: f32, entry_scale: f32, progress: f32) -> Self {
        let eased = ease_out_cubic_local(progress);
        Self {
            opacity: eased,
            y_offset_pixels: (1.0 - eased) * entry_offset_pixels,
            scale: lerp_f32(entry_scale, 1.0, eased),
        }
    }

    fn exit(
        entry_offset_pixels: f32,
        entry_scale: f32,
        exit_offset_multiplier: f32,
        exit_scale_multiplier: f32,
        progress: f32,
    ) -> Self {
        let eased = ease_out_cubic_local(progress);
        Self {
            opacity: 1.0 - eased,
            y_offset_pixels: -entry_offset_pixels * exit_offset_multiplier * eased,
            scale: 1.0 - (1.0 - entry_scale) * exit_scale_multiplier * eased,
        }
    }

    fn apply_line_shift(
        &mut self,
        from_line: usize,
        to_line: usize,
        line_height: f32,
        progress: f32,
    ) {
        let eased = ease_out_cubic_local(progress);
        let line_delta = from_line as f32 - to_line as f32;
        self.y_offset_pixels += line_delta * line_height * (1.0 - eased);
    }
}

pub(crate) type TranscriptCardVisual = SurfaceMotionVisual;

#[derive(Clone, Copy, Debug)]
struct TranscriptCardLineShift {
    from_line: usize,
    started_at: Instant,
}

#[derive(Clone, Copy, Debug)]
struct TranscriptCardMotionState {
    line: usize,
    last_run: SingleSessionTranscriptCardRun,
    entered_at: Option<Instant>,
    exiting_at: Option<Instant>,
    line_shift: Option<TranscriptCardLineShift>,
    last_seen_generation: u64,
}

#[derive(Clone, Debug, Default)]
pub(crate) struct TranscriptCardMotionFrame {
    visuals: HashMap<u64, TranscriptCardVisual>,
    exiting: Vec<(SingleSessionTranscriptCardRun, TranscriptCardVisual)>,
    active: bool,
    cache_key: u64,
}

impl TranscriptCardMotionFrame {
    pub(crate) fn visual_for_key(&self, key: u64) -> Option<TranscriptCardVisual> {
        self.visuals.get(&key).copied()
    }

    fn exiting(&self) -> &[(SingleSessionTranscriptCardRun, TranscriptCardVisual)] {
        &self.exiting
    }

    pub(crate) fn is_active(&self) -> bool {
        self.active
    }

    pub(crate) fn cache_key(&self) -> u64 {
        self.cache_key
    }
}

#[derive(Clone, Copy, Debug, Eq, Hash, PartialEq)]
enum TranscriptMessageRole {
    User,
    Assistant,
    Meta,
    Error,
}

#[derive(Clone, Copy, Debug, Eq, Hash, PartialEq)]
pub(crate) struct TranscriptMessageRun {
    line: usize,
    line_count: usize,
    role: TranscriptMessageRole,
}

pub(crate) type TranscriptMessageVisual = SurfaceMotionVisual;

#[derive(Clone, Copy, Debug)]
struct TranscriptMessageLineShift {
    from_line: usize,
    started_at: Instant,
}

#[derive(Clone, Copy, Debug)]
struct TranscriptMessageMotionState {
    run: TranscriptMessageRun,
    entered_at: Option<Instant>,
    line_shift: Option<TranscriptMessageLineShift>,
    last_seen_generation: u64,
}

#[derive(Clone, Debug, Default)]
pub(crate) struct TranscriptMessageMotionFrame {
    visuals: HashMap<u64, TranscriptMessageVisual>,
    active: bool,
    cache_key: u64,
}

impl TranscriptMessageMotionFrame {
    pub(crate) fn visual_for_key(&self, key: u64) -> Option<TranscriptMessageVisual> {
        self.visuals.get(&key).copied()
    }

    pub(crate) fn is_active(&self) -> bool {
        self.active
    }

    pub(crate) fn cache_key(&self) -> u64 {
        self.cache_key
    }
}

#[derive(Default)]
pub(crate) struct TranscriptMessageMotionRegistry {
    initialized: bool,
    generation: u64,
    states: HashMap<u64, TranscriptMessageMotionState>,
}

#[derive(Default)]
pub(crate) struct TranscriptCardMotionRegistry {
    initialized: bool,
    generation: u64,
    states: HashMap<u64, TranscriptCardMotionState>,
}

#[derive(Clone, Copy, Debug, Eq, Hash, PartialEq)]
enum InlineMarkdownPillKind {
    Code,
    Math,
}

#[derive(Clone, Copy, Debug, Eq, Hash, PartialEq)]
struct InlineMarkdownPillRun {
    line: usize,
    start_column: usize,
    column_count: usize,
    kind: InlineMarkdownPillKind,
}

pub(crate) type InlineMarkdownPillVisual = SurfaceMotionVisual;

#[derive(Clone, Copy, Debug)]
struct InlineMarkdownPillLineShift {
    from_line: usize,
    started_at: Instant,
}

#[derive(Clone, Copy, Debug)]
struct InlineMarkdownPillMotionState {
    run: InlineMarkdownPillRun,
    entered_at: Option<Instant>,
    exiting_at: Option<Instant>,
    line_shift: Option<InlineMarkdownPillLineShift>,
    last_seen_generation: u64,
}

#[derive(Clone, Debug, Default)]
pub(crate) struct InlineMarkdownPillMotionFrame {
    visuals: HashMap<u64, InlineMarkdownPillVisual>,
    exiting: Vec<(InlineMarkdownPillRun, InlineMarkdownPillVisual)>,
    active: bool,
    cache_key: u64,
}

impl InlineMarkdownPillMotionFrame {
    fn visual_for_key(&self, key: u64) -> Option<InlineMarkdownPillVisual> {
        self.visuals.get(&key).copied()
    }

    fn exiting(&self) -> &[(InlineMarkdownPillRun, InlineMarkdownPillVisual)] {
        &self.exiting
    }

    pub(crate) fn is_active(&self) -> bool {
        self.active
    }

    pub(crate) fn cache_key(&self) -> u64 {
        self.cache_key
    }
}

#[derive(Default)]
pub(crate) struct InlineMarkdownPillMotionRegistry {
    initialized: bool,
    generation: u64,
    states: HashMap<u64, InlineMarkdownPillMotionState>,
}

impl TranscriptMessageMotionRegistry {
    pub(crate) fn frame(
        &mut self,
        lines: &[SingleSessionStyledLine],
        line_height: f32,
        now: Instant,
    ) -> TranscriptMessageMotionFrame {
        self.generation = self.generation.wrapping_add(1).max(1);
        let generation = self.generation;
        let reduced_motion = crate::animation::desktop_reduced_motion_enabled();
        let animate_new_messages = self.initialized && !reduced_motion;
        self.initialized = true;

        let mut visuals = HashMap::new();
        let mut active = false;
        let mut occurrences = HashMap::new();
        for run in single_session_transcript_message_runs(lines) {
            let key = transcript_message_motion_key(lines, &run, &mut occurrences);
            let state = self
                .states
                .entry(key)
                .or_insert_with(|| TranscriptMessageMotionState {
                    run,
                    entered_at: animate_new_messages.then_some(now),
                    line_shift: None,
                    last_seen_generation: generation,
                });
            state.last_seen_generation = generation;

            if reduced_motion {
                state.entered_at = None;
                state.line_shift = None;
            }

            if state.run.line != run.line {
                if reduced_motion {
                    state.line_shift = None;
                } else {
                    state.line_shift = Some(TranscriptMessageLineShift {
                        from_line: state.run.line,
                        started_at: now,
                    });
                }
            }
            state.run = run;

            let (visual, visual_active) =
                transcript_message_visual_from_state(state, line_height, now);
            active |= visual_active;
            visuals.insert(key, visual);
        }

        self.states
            .retain(|_, state| state.last_seen_generation == generation);

        TranscriptMessageMotionFrame {
            cache_key: transcript_message_motion_cache_key(&visuals, active),
            visuals,
            active,
        }
    }

    pub(crate) fn clear(&mut self) {
        self.initialized = false;
        self.generation = 0;
        self.states.clear();
    }
}

impl TranscriptCardMotionRegistry {
    pub(crate) fn frame(
        &mut self,
        lines: &[SingleSessionStyledLine],
        line_height: f32,
        now: Instant,
    ) -> TranscriptCardMotionFrame {
        self.generation = self.generation.wrapping_add(1).max(1);
        let generation = self.generation;
        let reduced_motion = crate::animation::desktop_reduced_motion_enabled();
        let animate_new_cards = self.initialized && !reduced_motion;
        self.initialized = true;

        let mut visuals = HashMap::new();
        let mut active = false;
        let mut occurrences = HashMap::new();
        for run in single_session_transcript_card_runs(lines) {
            let key = transcript_card_motion_key(lines, &run, &mut occurrences);
            let state = self
                .states
                .entry(key)
                .or_insert_with(|| TranscriptCardMotionState {
                    line: run.line,
                    last_run: run,
                    entered_at: animate_new_cards.then_some(now),
                    exiting_at: None,
                    line_shift: None,
                    last_seen_generation: generation,
                });
            state.last_seen_generation = generation;
            state.last_run = run;
            state.exiting_at = None;

            if reduced_motion {
                state.entered_at = None;
                state.line_shift = None;
            }

            if state.line != run.line {
                if reduced_motion {
                    state.line_shift = None;
                } else {
                    state.line_shift = Some(TranscriptCardLineShift {
                        from_line: state.line,
                        started_at: now,
                    });
                }
                state.line = run.line;
            }

            let (visual, visual_active) =
                transcript_card_visual_from_state(state, line_height, now);
            active |= visual_active;
            visuals.insert(key, visual);
        }

        let mut exiting = Vec::new();
        if !reduced_motion {
            for state in self.states.values_mut() {
                if state.last_seen_generation == generation {
                    continue;
                }
                let exiting_at = *state.exiting_at.get_or_insert(now);
                let (progress, running) =
                    timed_animation_progress(exiting_at, now, TRANSCRIPT_CARD_EXIT_DURATION);
                if !running {
                    continue;
                }
                active = true;
                state.last_seen_generation = generation;
                exiting.push((state.last_run, exiting_transcript_card_visual(progress)));
            }
        }

        self.states
            .retain(|_, state| state.last_seen_generation == generation);

        TranscriptCardMotionFrame {
            cache_key: transcript_card_motion_cache_key(&visuals, &exiting, active),
            visuals,
            exiting,
            active,
        }
    }

    pub(crate) fn clear(&mut self) {
        self.initialized = false;
        self.generation = 0;
        self.states.clear();
    }
}

impl InlineMarkdownPillMotionRegistry {
    pub(crate) fn frame(
        &mut self,
        lines: &[SingleSessionStyledLine],
        line_height: f32,
        now: Instant,
    ) -> InlineMarkdownPillMotionFrame {
        self.generation = self.generation.wrapping_add(1).max(1);
        let generation = self.generation;
        let reduced_motion = crate::animation::desktop_reduced_motion_enabled();
        let animate_new_pills = self.initialized && !reduced_motion;
        self.initialized = true;

        let mut visuals = HashMap::new();
        let mut active = false;
        let mut occurrences = HashMap::new();
        for run in single_session_inline_markdown_pill_runs(lines) {
            let key = inline_markdown_pill_motion_key(lines, &run, &mut occurrences);
            let state = self
                .states
                .entry(key)
                .or_insert_with(|| InlineMarkdownPillMotionState {
                    run,
                    entered_at: animate_new_pills.then_some(now),
                    exiting_at: None,
                    line_shift: None,
                    last_seen_generation: generation,
                });
            state.last_seen_generation = generation;
            state.exiting_at = None;

            if reduced_motion {
                state.entered_at = None;
                state.line_shift = None;
            }

            if state.run.line != run.line {
                if reduced_motion {
                    state.line_shift = None;
                } else {
                    state.line_shift = Some(InlineMarkdownPillLineShift {
                        from_line: state.run.line,
                        started_at: now,
                    });
                }
            }
            state.run = run;

            let (visual, visual_active) =
                inline_markdown_pill_visual_from_state(state, line_height, now);
            active |= visual_active;
            visuals.insert(key, visual);
        }

        let mut exiting = Vec::new();
        if !reduced_motion {
            for state in self.states.values_mut() {
                if state.last_seen_generation == generation {
                    continue;
                }
                let exiting_at = *state.exiting_at.get_or_insert(now);
                let (progress, running) =
                    timed_animation_progress(exiting_at, now, INLINE_MARKDOWN_PILL_EXIT_DURATION);
                if !running {
                    continue;
                }
                active = true;
                state.last_seen_generation = generation;
                exiting.push((state.run, exiting_inline_markdown_pill_visual(progress)));
            }
        }

        self.states
            .retain(|_, state| state.last_seen_generation == generation);

        InlineMarkdownPillMotionFrame {
            cache_key: inline_markdown_pill_motion_cache_key(&visuals, &exiting, active),
            visuals,
            exiting,
            active,
        }
    }

    pub(crate) fn clear(&mut self) {
        self.initialized = false;
        self.generation = 0;
        self.states.clear();
    }
}

#[cfg(test)]
#[derive(Clone, Copy, Debug)]
pub(crate) struct SingleSessionTranscriptCardGeometry {
    pub(crate) run: SingleSessionTranscriptCardRun,
    pub(crate) card_rect: Rect,
    pub(crate) text_left: f32,
    pub(crate) line_height: f32,
}

#[derive(Clone, Debug, Eq, PartialEq)]
pub(crate) struct SingleSessionToolCardRun {
    pub(crate) line: usize,
    pub(crate) line_count: usize,
    pub(crate) call_id: String,
    pub(crate) name: String,
    pub(crate) state: SingleSessionToolVisualState,
    pub(crate) active: bool,
    pub(crate) expanded: bool,
    pub(crate) detail_line_count: usize,
    pub(crate) kind: SingleSessionToolLineKind,
}

#[cfg(test)]
#[derive(Clone, Debug)]
pub(crate) struct SingleSessionToolCardGeometry {
    pub(crate) run: SingleSessionToolCardRun,
    pub(crate) card_rect: Rect,
    pub(crate) rail_rect: Rect,
    pub(crate) line_height: f32,
}

#[derive(Clone, Copy, Debug)]
struct ToolCardPalette {
    background: [f32; 4],
    border: [f32; 4],
    rail: [f32; 4],
    chip: [f32; 4],
}

#[derive(Clone, Copy, Debug)]
struct ToolCardStateTransition {
    from_state: SingleSessionToolVisualState,
    from_active: bool,
    started_at: Instant,
}

#[derive(Clone, Copy, Debug)]
struct ToolCardOutputTransition {
    from_detail_line_count: usize,
    started_at: Instant,
}

#[derive(Clone, Copy, Debug)]
struct ToolCardResolutionFlash {
    state: SingleSessionToolVisualState,
    started_at: Instant,
}

#[derive(Clone, Debug)]
struct ToolCardMotionState {
    target_state: SingleSessionToolVisualState,
    target_active: bool,
    detail_line_count: usize,
    last_run: SingleSessionToolCardRun,
    entered_at: Option<Instant>,
    exiting_at: Option<Instant>,
    state_transition: Option<ToolCardStateTransition>,
    output_transition: Option<ToolCardOutputTransition>,
    resolution_flash: Option<ToolCardResolutionFlash>,
    last_seen_generation: u64,
}

#[derive(Clone, Copy, Debug)]
pub(crate) struct ToolCardVisual {
    pub(crate) opacity: f32,
    pub(crate) y_offset_pixels: f32,
    pub(crate) scale: f32,
    pub(crate) background: [f32; 4],
    pub(crate) border: [f32; 4],
    pub(crate) rail: [f32; 4],
    pub(crate) chip: [f32; 4],
    pub(crate) output_reveal: f32,
    pub(crate) flash_color: [f32; 4],
    pub(crate) flash_alpha: f32,
    pub(crate) active_phase: f32,
}

impl Default for ToolCardVisual {
    fn default() -> Self {
        Self {
            opacity: 1.0,
            y_offset_pixels: 0.0,
            scale: 1.0,
            background: TOOL_CARD_BACKGROUND_COLOR,
            border: TOOL_CARD_BORDER_COLOR,
            rail: TOOL_TIMELINE_RAIL_COLOR,
            chip: TOOL_STATUS_CHIP_COLOR,
            output_reveal: 1.0,
            flash_color: TOOL_TIMELINE_RAIL_COLOR,
            flash_alpha: 0.0,
            active_phase: 0.0,
        }
    }
}

#[derive(Clone, Debug, Default)]
pub(crate) struct ToolCardMotionFrame {
    visuals: HashMap<String, ToolCardVisual>,
    exiting: Vec<(SingleSessionToolCardRun, ToolCardVisual)>,
    active: bool,
    cache_key: u64,
}

impl ToolCardMotionFrame {
    pub(crate) fn visual_for(&self, call_id: &str) -> Option<ToolCardVisual> {
        self.visuals.get(call_id).copied()
    }

    pub(crate) fn is_active(&self) -> bool {
        self.active
    }

    pub(crate) fn cache_key(&self) -> u64 {
        self.cache_key
    }

    pub(crate) fn exiting(&self) -> &[(SingleSessionToolCardRun, ToolCardVisual)] {
        &self.exiting
    }
}

#[derive(Default)]
pub(crate) struct ToolCardMotionRegistry {
    initialized: bool,
    generation: u64,
    states: HashMap<String, ToolCardMotionState>,
}

impl ToolCardMotionRegistry {
    pub(crate) fn frame(
        &mut self,
        lines: &[SingleSessionStyledLine],
        now: Instant,
        tick: u64,
    ) -> ToolCardMotionFrame {
        self.generation = self.generation.wrapping_add(1).max(1);
        let generation = self.generation;
        let reduced_motion = crate::animation::desktop_reduced_motion_enabled();
        let animate_new_cards = self.initialized && !reduced_motion;
        self.initialized = true;

        let mut visuals = HashMap::new();
        let mut active = false;
        for run in single_session_tool_card_runs(lines) {
            let state =
                self.states
                    .entry(run.call_id.clone())
                    .or_insert_with(|| ToolCardMotionState {
                        target_state: run.state,
                        target_active: run.active,
                        detail_line_count: run.detail_line_count,
                        last_run: run.clone(),
                        entered_at: animate_new_cards.then_some(now),
                        exiting_at: None,
                        state_transition: None,
                        output_transition: None,
                        resolution_flash: None,
                        last_seen_generation: generation,
                    });
            state.last_seen_generation = generation;
            state.exiting_at = None;

            if state.target_state != run.state || state.target_active != run.active {
                let previous_state = state.target_state;
                let previous_active = state.target_active;
                state.state_transition = Some(ToolCardStateTransition {
                    from_state: previous_state,
                    from_active: previous_active,
                    started_at: now,
                });
                if (previous_state.is_active() || previous_active)
                    && !(run.state.is_active() || run.active)
                    && matches!(
                        run.state,
                        SingleSessionToolVisualState::Succeeded
                            | SingleSessionToolVisualState::Failed
                    )
                {
                    state.resolution_flash = Some(ToolCardResolutionFlash {
                        state: run.state,
                        started_at: now,
                    });
                }
                state.target_state = run.state;
                state.target_active = run.active;
            }

            if state.detail_line_count != run.detail_line_count {
                state.output_transition = Some(ToolCardOutputTransition {
                    from_detail_line_count: state.detail_line_count,
                    started_at: now,
                });
                state.detail_line_count = run.detail_line_count;
            }

            state.last_run = run.clone();

            let (visual, visual_active) =
                tool_card_visual_from_state(state, &run, now, tick, reduced_motion);
            active |= visual_active || (!reduced_motion && (run.active || run.state.is_active()));
            visuals.insert(run.call_id, visual);
        }

        let mut exiting = Vec::new();
        for state in self.states.values_mut() {
            if state.last_seen_generation == generation {
                continue;
            }
            let exiting_at = *state.exiting_at.get_or_insert(now);
            let (progress, running) =
                timed_animation_progress(exiting_at, now, TOOL_CARD_EXIT_DURATION);
            if !running {
                continue;
            }
            let visual = exiting_tool_card_visual(&state.last_run, progress, tick);
            active = true;
            state.last_seen_generation = generation;
            exiting.push((state.last_run.clone(), visual));
        }

        self.states
            .retain(|_, state| state.last_seen_generation == generation);

        ToolCardMotionFrame {
            cache_key: tool_card_motion_cache_key(&visuals, &exiting, active),
            visuals,
            exiting,
            active,
        }
    }

    pub(crate) fn clear(&mut self) {
        self.initialized = false;
        self.generation = 0;
        self.states.clear();
    }
}

#[derive(Clone, Copy, Debug, PartialEq)]
struct SingleSessionScrollbarGeometry {
    thumb_y: f32,
    thumb_height: f32,
}

#[derive(Clone, Copy, Debug, PartialEq)]
pub(crate) struct SingleSessionScrollbarVisual {
    pub(crate) thumb_y: f32,
    pub(crate) thumb_height: f32,
    pub(crate) opacity: f32,
}

#[derive(Clone, Copy, Debug, Default, PartialEq)]
pub(crate) struct SingleSessionScrollbarMotionFrame {
    visual: Option<SingleSessionScrollbarVisual>,
    active: bool,
    cache_key: u64,
}

impl SingleSessionScrollbarMotionFrame {
    pub(crate) fn visual(&self) -> Option<SingleSessionScrollbarVisual> {
        self.visual
    }

    pub(crate) fn is_active(&self) -> bool {
        self.active
    }

    pub(crate) fn cache_key(&self) -> u64 {
        self.cache_key
    }
}

#[derive(Default)]
pub(crate) struct SingleSessionScrollbarMotionRegistry {
    initialized: bool,
    start_geometry: Option<SingleSessionScrollbarGeometry>,
    current_geometry: Option<SingleSessionScrollbarGeometry>,
    target_geometry: Option<SingleSessionScrollbarGeometry>,
    transition_started_at: Option<Instant>,
    last_activity_at: Option<Instant>,
}

impl SingleSessionScrollbarMotionRegistry {
    pub(crate) fn frame(
        &mut self,
        app: &SingleSessionApp,
        size: PhysicalSize<u32>,
        total_lines: usize,
        smooth_scroll_lines: f32,
        now: Instant,
    ) -> SingleSessionScrollbarMotionFrame {
        let metrics = single_session_body_scroll_metrics_for_total_lines(app, size, total_lines);
        self.frame_for_metrics(size, smooth_scroll_lines, metrics, now)
    }

    fn frame_for_metrics(
        &mut self,
        size: PhysicalSize<u32>,
        smooth_scroll_lines: f32,
        metrics: Option<SingleSessionBodyScrollMetrics>,
        now: Instant,
    ) -> SingleSessionScrollbarMotionFrame {
        let Some(metrics) = metrics else {
            self.clear();
            return SingleSessionScrollbarMotionFrame::default();
        };
        let target_geometry = single_session_scrollbar_geometry(size, smooth_scroll_lines, metrics);

        if !self.initialized {
            self.initialized = true;
            self.start_geometry = Some(target_geometry);
            self.current_geometry = Some(target_geometry);
            self.target_geometry = Some(target_geometry);
            self.transition_started_at = None;
            self.last_activity_at = Some(now);
        } else if self
            .target_geometry
            .is_none_or(|previous| scrollbar_geometry_changed(previous, target_geometry))
        {
            let start_geometry = self.current_geometry.unwrap_or(target_geometry);
            self.start_geometry = Some(start_geometry);
            self.current_geometry = Some(start_geometry);
            self.target_geometry = Some(target_geometry);
            self.transition_started_at = Some(now);
            self.last_activity_at = Some(now);
        }

        let transition_active = self.update_transition(now);
        let (opacity, fade_active) = self.opacity_for_frame(now);
        let active = transition_active || fade_active;
        let visual = (opacity > 0.001 || transition_active).then(|| {
            let geometry = self.current_geometry.unwrap_or(target_geometry);
            SingleSessionScrollbarVisual {
                thumb_y: geometry.thumb_y,
                thumb_height: geometry.thumb_height,
                opacity,
            }
        });
        SingleSessionScrollbarMotionFrame {
            visual,
            active,
            cache_key: scrollbar_motion_cache_key(visual, active),
        }
    }

    pub(crate) fn clear(&mut self) {
        self.initialized = false;
        self.start_geometry = None;
        self.current_geometry = None;
        self.target_geometry = None;
        self.transition_started_at = None;
        self.last_activity_at = None;
    }

    fn update_transition(&mut self, now: Instant) -> bool {
        let Some(started_at) = self.transition_started_at else {
            return false;
        };
        let Some(start) = self.start_geometry else {
            self.transition_started_at = None;
            return false;
        };
        let Some(target) = self.target_geometry else {
            self.transition_started_at = None;
            return false;
        };
        let (progress, running) = timed_animation_progress(
            started_at,
            now,
            SINGLE_SESSION_SCROLLBAR_THUMB_TRANSITION_DURATION,
        );
        let eased = ease_out_cubic_local(progress);
        self.current_geometry = Some(SingleSessionScrollbarGeometry {
            thumb_y: lerp_f32(start.thumb_y, target.thumb_y, eased),
            thumb_height: lerp_f32(start.thumb_height, target.thumb_height, eased),
        });
        if !running {
            self.current_geometry = Some(target);
            self.transition_started_at = None;
        }
        running
    }

    fn opacity_for_frame(&self, now: Instant) -> (f32, bool) {
        let Some(last_activity_at) = self.last_activity_at else {
            return (0.0, false);
        };
        let elapsed = now.saturating_duration_since(last_activity_at);
        if crate::animation::desktop_reduced_motion_enabled() {
            let opacity = if elapsed <= SINGLE_SESSION_SCROLLBAR_FADE_IDLE_DURATION {
                1.0
            } else {
                0.0
            };
            return (opacity, false);
        }
        if elapsed <= SINGLE_SESSION_SCROLLBAR_FADE_IDLE_DURATION {
            return (1.0, true);
        }
        let fade_elapsed = elapsed - SINGLE_SESSION_SCROLLBAR_FADE_IDLE_DURATION;
        let (progress, running) = timed_animation_progress(
            last_activity_at + SINGLE_SESSION_SCROLLBAR_FADE_IDLE_DURATION,
            last_activity_at + SINGLE_SESSION_SCROLLBAR_FADE_IDLE_DURATION + fade_elapsed,
            SINGLE_SESSION_SCROLLBAR_FADE_DURATION,
        );
        let opacity = 1.0 - ease_out_cubic_local(progress);
        (opacity, running)
    }
}

fn scrollbar_geometry_changed(
    previous: SingleSessionScrollbarGeometry,
    next: SingleSessionScrollbarGeometry,
) -> bool {
    (previous.thumb_y - next.thumb_y).abs() > 0.25
        || (previous.thumb_height - next.thumb_height).abs() > 0.25
}

fn single_session_scrollbar_geometry(
    size: PhysicalSize<u32>,
    smooth_scroll_lines: f32,
    metrics: SingleSessionBodyScrollMetrics,
) -> SingleSessionScrollbarGeometry {
    let track_top = single_session_scrollbar_track_top();
    let track_bottom = single_session_scrollbar_track_bottom(size);
    let track_height = (track_bottom - track_top).max(1.0);
    let thumb_height = (metrics.visible_lines as f32 / metrics.total_lines as f32 * track_height)
        .clamp(28.0, track_height);
    let travel = (track_height - thumb_height).max(0.0);
    let smooth_scroll_lines =
        (metrics.scroll_lines + smooth_scroll_lines).clamp(0.0, metrics.max_scroll_lines as f32);
    let scroll_fraction = smooth_scroll_lines / metrics.max_scroll_lines.max(1) as f32;
    let thumb_y = track_top + (1.0 - scroll_fraction.clamp(0.0, 1.0)) * travel;
    SingleSessionScrollbarGeometry {
        thumb_y,
        thumb_height,
    }
}

fn scrollbar_motion_cache_key(visual: Option<SingleSessionScrollbarVisual>, active: bool) -> u64 {
    let mut hasher = DefaultHasher::new();
    active.hash(&mut hasher);
    visual.is_some().hash(&mut hasher);
    if let Some(visual) = visual {
        hash_f32(visual.thumb_y, &mut hasher);
        hash_f32(visual.thumb_height, &mut hasher);
        hash_f32(visual.opacity, &mut hasher);
    }
    hasher.finish()
}

fn lerp_f32(start: f32, end: f32, progress: f32) -> f32 {
    start + (end - start) * progress
}

fn tool_card_visual_from_state(
    state: &mut ToolCardMotionState,
    run: &SingleSessionToolCardRun,
    now: Instant,
    tick: u64,
    reduced_motion: bool,
) -> (ToolCardVisual, bool) {
    let target_palette = tool_card_palette(run.state, run.active);
    let mut palette = target_palette;
    let mut active = false;

    if let Some(transition) = state.state_transition {
        let (progress, running) = timed_animation_progress(
            transition.started_at,
            now,
            TOOL_CARD_STATE_TRANSITION_DURATION,
        );
        let eased = ease_out_cubic_local(progress);
        let from = tool_card_palette(transition.from_state, transition.from_active);
        palette = mix_tool_card_palette(from, target_palette, eased);
        active |= running;
        if !running {
            state.state_transition = None;
        }
    }

    let mut opacity = 1.0;
    let mut y_offset_pixels = 0.0;
    let mut scale = 1.0;
    if let Some(entered_at) = state.entered_at {
        let (progress, running) =
            timed_animation_progress(entered_at, now, TOOL_CARD_ENTRY_DURATION);
        let eased = ease_out_cubic_local(progress);
        opacity = eased;
        y_offset_pixels = (1.0 - eased) * TOOL_CARD_ENTRY_OFFSET_PIXELS;
        scale = TOOL_CARD_ENTRY_SCALE + (1.0 - TOOL_CARD_ENTRY_SCALE) * eased;
        active |= running;
        if !running {
            state.entered_at = None;
        }
    }

    let mut output_reveal = 1.0;
    if let Some(transition) = state.output_transition {
        let (progress, running) =
            timed_animation_progress(transition.started_at, now, TOOL_CARD_OUTPUT_REVEAL_DURATION);
        let eased = ease_out_cubic_local(progress);
        if state.detail_line_count > transition.from_detail_line_count {
            output_reveal = eased;
        } else {
            output_reveal = 1.0 - eased;
        }
        active |= running;
        if !running {
            state.output_transition = None;
            output_reveal = 1.0;
        }
    }

    let mut flash_color = TOOL_TIMELINE_RAIL_COLOR;
    let mut flash_alpha = 0.0;
    if let Some(flash) = state.resolution_flash {
        let (progress, running) =
            timed_animation_progress(flash.started_at, now, TOOL_CARD_RESOLUTION_FLASH_DURATION);
        let fade = 1.0 - ease_out_cubic_local(progress);
        flash_color = single_session_tool_state_accent(flash.state);
        flash_alpha = (0.34 * fade).clamp(0.0, 0.34);
        active |= running;
        if !running {
            state.resolution_flash = None;
        }
    }

    let pulse = if reduced_motion {
        0.0
    } else {
        active_tool_card_pulse(tick)
    };
    let active_phase = if reduced_motion {
        0.0
    } else {
        (tick % 18) as f32 / 18.0
    };
    if run.active || run.state.is_active() {
        palette.background[3] = (palette.background[3] + 0.08 * pulse).clamp(0.0, 0.82);
        palette.border[3] = (palette.border[3] + 0.16 * pulse).clamp(0.0, 0.62);
        palette.rail[3] = (palette.rail[3] + 0.24 * pulse).clamp(0.0, 0.78);
    }

    (
        ToolCardVisual {
            opacity,
            y_offset_pixels,
            scale,
            background: palette.background,
            border: palette.border,
            rail: palette.rail,
            chip: palette.chip,
            output_reveal,
            flash_color,
            flash_alpha,
            active_phase,
        },
        active,
    )
}

fn exiting_tool_card_visual(
    run: &SingleSessionToolCardRun,
    progress: f32,
    tick: u64,
) -> ToolCardVisual {
    let eased = ease_out_cubic_local(progress);
    let mut visual = default_tool_card_visual(run, active_tool_card_pulse(tick));
    visual.opacity = 1.0 - eased;
    visual.y_offset_pixels = -TOOL_CARD_ENTRY_OFFSET_PIXELS * 0.55 * eased;
    visual.scale = 1.0 - (1.0 - TOOL_CARD_ENTRY_SCALE) * eased;
    visual.output_reveal = 1.0 - eased * 0.65;
    visual
}

fn timed_animation_progress(started_at: Instant, now: Instant, duration: Duration) -> (f32, bool) {
    if duration.is_zero() || crate::animation::desktop_reduced_motion_enabled() {
        return (1.0, false);
    }
    let progress = (now.saturating_duration_since(started_at).as_secs_f32()
        / duration.as_secs_f32())
    .clamp(0.0, 1.0);
    (progress, progress < 1.0)
}

fn ease_out_cubic_local(progress: f32) -> f32 {
    1.0 - (1.0 - progress.clamp(0.0, 1.0)).powi(3)
}

fn inline_widget_selection_target(
    kind: Option<InlineWidgetKind>,
    lines: &[SingleSessionStyledLine],
    visible_line_count: usize,
) -> Option<InlineWidgetSelectionTarget> {
    let kind = kind?;
    let visible_len = visible_line_count.min(lines.len());
    let visible_lines = &lines[..visible_len];
    let selected_line = visible_lines
        .iter()
        .position(|line| line.style == SingleSessionLineStyle::OverlaySelection)?;
    let line_span = match kind {
        InlineWidgetKind::ModelPicker => {
            // Model rows use a selected primary line followed by a metadata
            // detail line. Keep the highlight as one two-line target so the
            // keyboard selection feels like a card moving through the list.
            if selected_line + 1 < visible_len {
                2
            } else {
                1
            }
        }
        InlineWidgetKind::SessionSwitcher => visible_lines[selected_line..]
            .iter()
            .take_while(|line| line.style == SingleSessionLineStyle::OverlaySelection)
            .count()
            .max(1),
        InlineWidgetKind::SlashSuggestions => 1,
        InlineWidgetKind::HotkeyHelp | InlineWidgetKind::SessionInfo => return None,
    };

    Some(InlineWidgetSelectionTarget {
        kind,
        line: selected_line,
        line_span: line_span
            .min(visible_len.saturating_sub(selected_line))
            .max(1),
    })
}

fn inline_widget_preview_pane_target(
    kind: Option<InlineWidgetKind>,
    lines: &[SingleSessionStyledLine],
    visible_line_count: usize,
) -> Option<InlineWidgetPreviewPaneTarget> {
    let kind = kind?;
    if kind != InlineWidgetKind::SessionSwitcher {
        return None;
    }
    let visible_len = visible_line_count.min(lines.len());
    let visible_lines = &lines[..visible_len];
    let header_line = visible_lines
        .iter()
        .position(|line| line.text.contains("sessions") && line.text.contains("preview"))?;
    let focus_pane = usize::from(visible_lines[header_line].text.contains("preview ›"));
    let mut hasher = DefaultHasher::new();
    kind.hash(&mut hasher);
    for line in visible_lines.iter().skip(header_line + 1) {
        if line.text.contains("preview lines ") {
            break;
        }
        line.text.hash(&mut hasher);
        line.style.hash(&mut hasher);
    }
    Some(InlineWidgetPreviewPaneTarget {
        kind,
        focus_pane,
        preview_key: hasher.finish(),
    })
}

fn inline_widget_preview_pane_visual_from_state(
    target: InlineWidgetPreviewPaneTarget,
    focus_transition: &mut Option<InlineWidgetPreviewPaneFocusTransition>,
    content_started_at: &mut Option<Instant>,
    now: Instant,
) -> (InlineWidgetPreviewPaneVisual, bool) {
    let settled = InlineWidgetPreviewPaneVisual::settled(target);
    let mut active = false;
    let mut focus_pane_position = settled.focus_pane_position;
    if let Some(transition) = *focus_transition {
        let (progress, running) = timed_animation_progress(
            transition.started_at,
            now,
            INLINE_WIDGET_PREVIEW_PANE_FOCUS_DURATION,
        );
        let eased = ease_out_cubic_local(progress);
        focus_pane_position =
            lerp_f32(transition.from_pane as f32, target.focus_pane as f32, eased);
        active |= running;
        if !running {
            *focus_transition = None;
            focus_pane_position = target.focus_pane as f32;
        }
    }

    let mut preview_opacity = settled.preview_opacity;
    let mut preview_y_offset_pixels = settled.preview_y_offset_pixels;
    if let Some(started_at) = *content_started_at {
        let (progress, running) =
            timed_animation_progress(started_at, now, INLINE_WIDGET_PREVIEW_PANE_CONTENT_DURATION);
        let eased = ease_out_cubic_local(progress);
        preview_opacity = 0.35 + 0.65 * eased;
        preview_y_offset_pixels = 5.0 * (1.0 - eased);
        active |= running;
        if !running {
            *content_started_at = None;
            preview_opacity = 1.0;
            preview_y_offset_pixels = 0.0;
        }
    }

    (
        InlineWidgetPreviewPaneVisual {
            focus_pane_position,
            preview_opacity,
            preview_y_offset_pixels,
        },
        active,
    )
}

fn inline_widget_preview_pane_cache_key(
    visual: Option<InlineWidgetPreviewPaneVisual>,
    active: bool,
) -> u64 {
    let mut hasher = DefaultHasher::new();
    active.hash(&mut hasher);
    visual.is_some().hash(&mut hasher);
    if let Some(visual) = visual {
        hash_f32(visual.focus_pane_position, &mut hasher);
        hash_f32(visual.preview_opacity, &mut hasher);
        hash_f32(visual.preview_y_offset_pixels, &mut hasher);
    }
    hasher.finish()
}

fn inline_widget_list_row_runs(
    kind: Option<InlineWidgetKind>,
    lines: &[SingleSessionStyledLine],
    visible_line_count: usize,
) -> Vec<InlineWidgetListRowRun> {
    let Some(kind) = kind else {
        return Vec::new();
    };
    let visible_len = visible_line_count.min(lines.len());
    let mut runs = Vec::new();
    let mut occurrences = HashMap::new();

    match kind {
        InlineWidgetKind::SlashSuggestions => {
            for line in 1..visible_len {
                if matches!(
                    lines[line].style,
                    SingleSessionLineStyle::OverlaySelection | SingleSessionLineStyle::Overlay
                ) {
                    push_inline_widget_list_row_run(
                        &mut runs,
                        &mut occurrences,
                        kind,
                        lines,
                        line,
                        1,
                    );
                }
            }
        }
        InlineWidgetKind::ModelPicker => {
            let mut line = 2;
            while line < visible_len {
                let primary_style = lines[line].style;
                let looks_like_primary = matches!(
                    primary_style,
                    SingleSessionLineStyle::OverlaySelection | SingleSessionLineStyle::Overlay
                ) && line + 1 < visible_len
                    && lines[line + 1].style == SingleSessionLineStyle::Meta
                    && lines[line + 1].text.trim_start().contains('·');
                if looks_like_primary {
                    push_inline_widget_list_row_run(
                        &mut runs,
                        &mut occurrences,
                        kind,
                        lines,
                        line,
                        2,
                    );
                    line += 2;
                } else {
                    line += 1;
                }
            }
        }
        InlineWidgetKind::SessionSwitcher => {
            let mut line = 0;
            while line < visible_len {
                if lines[line].text.starts_with("Preview") {
                    break;
                }
                let looks_like_session_card = matches!(
                    lines[line].style,
                    SingleSessionLineStyle::OverlaySelection | SingleSessionLineStyle::Overlay
                ) && lines[line].text.contains(" session ·")
                    && line + 1 < visible_len
                    && lines[line + 1].text.trim_start().starts_with("Status ");
                if looks_like_session_card {
                    let mut span = 1;
                    while line + span < visible_len
                        && span < 4
                        && !lines[line + span].text.starts_with("Preview")
                        && lines[line + span].style != SingleSessionLineStyle::Blank
                        && lines[line + span].style != SingleSessionLineStyle::OverlayTitle
                    {
                        span += 1;
                    }
                    push_inline_widget_list_row_run(
                        &mut runs,
                        &mut occurrences,
                        kind,
                        lines,
                        line,
                        span,
                    );
                    line += span;
                } else {
                    line += 1;
                }
            }
        }
        InlineWidgetKind::HotkeyHelp | InlineWidgetKind::SessionInfo => {}
    }

    runs
}

fn push_inline_widget_list_row_run(
    runs: &mut Vec<InlineWidgetListRowRun>,
    occurrences: &mut HashMap<u64, usize>,
    kind: InlineWidgetKind,
    lines: &[SingleSessionStyledLine],
    line: usize,
    line_span: usize,
) {
    let base_key = inline_widget_list_row_base_key(kind, lines, line, line_span);
    let key = motion_occurrence_key(base_key, occurrences);
    runs.push(InlineWidgetListRowRun {
        kind,
        key,
        line,
        line_span,
    });
}

fn inline_widget_list_row_base_key(
    kind: InlineWidgetKind,
    lines: &[SingleSessionStyledLine],
    line: usize,
    line_span: usize,
) -> u64 {
    let mut hasher = DefaultHasher::new();
    kind.hash(&mut hasher);
    line_span.hash(&mut hasher);
    let end = line.saturating_add(line_span).min(lines.len());
    for styled_line in &lines[line.min(lines.len())..end] {
        styled_line.style.hash(&mut hasher);
        normalized_inline_widget_list_row_text(&styled_line.text).hash(&mut hasher);
    }
    hasher.finish()
}

fn normalized_inline_widget_list_row_text(text: &str) -> String {
    text.chars()
        .map(|ch| match ch {
            '›' | '▶' => ' ',
            _ => ch,
        })
        .collect()
}

fn inline_widget_selection_visual_from_transition(
    transition: &mut Option<InlineWidgetSelectionTransition>,
    target: InlineWidgetSelectionTarget,
    now: Instant,
) -> (InlineWidgetSelectionVisual, bool) {
    let Some(active_transition) = *transition else {
        return (InlineWidgetSelectionVisual::settled(target), false);
    };

    let (progress, running) = timed_animation_progress(
        active_transition.started_at,
        now,
        INLINE_WIDGET_SELECTION_TRANSITION_DURATION,
    );
    let eased = ease_out_cubic_local(progress);
    let from_line = active_transition.from_line as f32;
    let to_line = target.line as f32;
    let from_span = active_transition.from_line_span as f32;
    let to_span = target.line_span as f32;
    let visual = InlineWidgetSelectionVisual {
        opacity: 1.0,
        y_offset_lines: (from_line - to_line) * (1.0 - eased),
        line_span: from_span + (to_span - from_span) * eased,
    };
    if !running {
        *transition = None;
    }
    (visual, running)
}

fn inline_widget_list_reflow_visual_from_state(
    state: &mut InlineWidgetListReflowState,
    now: Instant,
) -> (InlineWidgetListReflowVisual, bool) {
    let mut visual = InlineWidgetListReflowVisual {
        opacity: 0.0,
        y_offset_lines: 0.0,
        line_span: state.run.line_span as f32,
    };
    let mut active = false;

    if let Some(entered_at) = state.entered_at {
        let (progress, running) =
            timed_animation_progress(entered_at, now, INLINE_WIDGET_LIST_REFLOW_ENTRY_DURATION);
        let eased = ease_out_cubic_local(progress);
        visual.opacity = visual.opacity.max(1.0 - eased);
        visual.y_offset_lines += 0.45 * (1.0 - eased);
        active |= running;
        if !running {
            state.entered_at = None;
        }
    }

    if let Some(shift) = state.shift {
        let (progress, running) = timed_animation_progress(
            shift.started_at,
            now,
            INLINE_WIDGET_LIST_REFLOW_SHIFT_DURATION,
        );
        let eased = ease_out_cubic_local(progress);
        let line_delta = shift.from_line as f32 - state.run.line as f32;
        let span_delta = shift.from_line_span as f32 - state.run.line_span as f32;
        visual.opacity = visual.opacity.max(1.0 - eased * 0.15);
        visual.y_offset_lines += line_delta * (1.0 - eased);
        visual.line_span = state.run.line_span as f32 + span_delta * (1.0 - eased);
        active |= running;
        if !running {
            state.shift = None;
        }
    }

    (visual, active)
}

fn exiting_inline_widget_list_reflow_visual(progress: f32) -> InlineWidgetListReflowVisual {
    let eased = ease_out_cubic_local(progress);
    InlineWidgetListReflowVisual {
        opacity: 1.0 - eased,
        y_offset_lines: -0.35 * eased,
        line_span: 1.0,
    }
}

fn inline_widget_list_reflow_cache_key(
    visuals: &HashMap<u64, InlineWidgetListReflowVisual>,
    exiting: &[(InlineWidgetListRowRun, InlineWidgetListReflowVisual)],
    active: bool,
) -> u64 {
    let mut hasher = DefaultHasher::new();
    active.hash(&mut hasher);
    for (key, visual) in sorted_u64_visual_entries(visuals) {
        key.hash(&mut hasher);
        hash_f32(visual.opacity, &mut hasher);
        hash_f32(visual.y_offset_lines, &mut hasher);
        hash_f32(visual.line_span, &mut hasher);
    }
    for (run, visual) in exiting {
        run.kind.hash(&mut hasher);
        run.key.hash(&mut hasher);
        run.line.hash(&mut hasher);
        run.line_span.hash(&mut hasher);
        hash_f32(visual.opacity, &mut hasher);
        hash_f32(visual.y_offset_lines, &mut hasher);
        hash_f32(visual.line_span, &mut hasher);
    }
    hasher.finish()
}

fn composer_motion_target(app: &SingleSessionApp) -> ComposerMotionTarget {
    let line_count = app.composer_text().split('\n').count().max(1);
    let ready_to_submit = !app.draft.trim().is_empty();
    ComposerMotionTarget {
        line_count,
        empty: app.draft.is_empty(),
        blocked: !app.should_draw_composer_caret(),
        processing: app.is_processing,
        ready_to_submit,
    }
}

fn composer_motion_visual_lerp(
    from: ComposerMotionVisual,
    to: ComposerMotionVisual,
    progress: f32,
) -> ComposerMotionVisual {
    ComposerMotionVisual {
        height_lines: lerp_f32(from.height_lines, to.height_lines, progress),
        placeholder_opacity: lerp_f32(from.placeholder_opacity, to.placeholder_opacity, progress),
        focus_opacity: lerp_f32(from.focus_opacity, to.focus_opacity, progress),
        blocked_progress: lerp_f32(from.blocked_progress, to.blocked_progress, progress),
        submit_opacity: lerp_f32(from.submit_opacity, to.submit_opacity, progress),
        submit_scale: lerp_f32(from.submit_scale, to.submit_scale, progress),
        processing_progress: lerp_f32(from.processing_progress, to.processing_progress, progress),
    }
}

fn composer_motion_cache_key(
    target: ComposerMotionTarget,
    visual: ComposerMotionVisual,
    active: bool,
) -> u64 {
    let mut hasher = DefaultHasher::new();
    target.hash(&mut hasher);
    active.hash(&mut hasher);
    hash_f32(visual.height_lines, &mut hasher);
    hash_f32(visual.placeholder_opacity, &mut hasher);
    hash_f32(visual.focus_opacity, &mut hasher);
    hash_f32(visual.blocked_progress, &mut hasher);
    hash_f32(visual.submit_opacity, &mut hasher);
    hash_f32(visual.submit_scale, &mut hasher);
    hash_f32(visual.processing_progress, &mut hasher);
    hasher.finish()
}

fn attachment_chip_runs(images: &[(String, String)]) -> Vec<AttachmentChipRun> {
    let mut runs = Vec::new();
    let mut occurrences = HashMap::new();
    for (index, (media_type, base64_data)) in images
        .iter()
        .take(ATTACHMENT_CHIP_VISIBLE_LIMIT)
        .enumerate()
    {
        let base_key = attachment_chip_base_key(media_type, base64_data);
        let key = motion_occurrence_key(base_key, &mut occurrences);
        runs.push(AttachmentChipRun { key, index });
    }
    runs
}

fn attachment_chip_base_key(media_type: &str, base64_data: &str) -> u64 {
    let mut hasher = DefaultHasher::new();
    media_type.hash(&mut hasher);
    base64_data.len().hash(&mut hasher);
    let bytes = base64_data.as_bytes();
    let sample = bytes.len().min(48);
    bytes[..sample].hash(&mut hasher);
    if bytes.len() > sample {
        bytes[bytes.len() - sample..].hash(&mut hasher);
    }
    hasher.finish()
}

fn attachment_chip_visual_from_state(
    state: &mut AttachmentChipState,
    now: Instant,
) -> (AttachmentChipVisual, bool) {
    let mut visual = AttachmentChipVisual::settled();
    let mut active = false;

    if let Some(entered_at) = state.entered_at {
        let (progress, running) =
            timed_animation_progress(entered_at, now, ATTACHMENT_CHIP_ENTRY_DURATION);
        let eased = ease_out_cubic_local(progress);
        visual.opacity = eased;
        visual.y_offset_pixels += 5.0 * (1.0 - eased);
        visual.scale *= 0.90 + 0.10 * eased;
        active |= running;
        if !running {
            state.entered_at = None;
        }
    }

    if let Some(shift) = state.shift {
        let (progress, running) =
            timed_animation_progress(shift.started_at, now, ATTACHMENT_CHIP_SHIFT_DURATION);
        let eased = ease_out_cubic_local(progress);
        let index_delta = shift.from_index as f32 - state.run.index as f32;
        visual.x_offset_pixels +=
            index_delta * (ATTACHMENT_CHIP_WIDTH + ATTACHMENT_CHIP_GAP) * (1.0 - eased);
        active |= running;
        if !running {
            state.shift = None;
        }
    }

    (visual, active)
}

fn exiting_attachment_chip_visual(progress: f32) -> AttachmentChipVisual {
    let eased = ease_out_cubic_local(progress);
    AttachmentChipVisual {
        opacity: 1.0 - eased,
        x_offset_pixels: 0.0,
        y_offset_pixels: -5.0 * eased,
        scale: 1.0 - 0.08 * eased,
    }
}

fn attachment_chip_motion_cache_key(
    visuals: &HashMap<u64, AttachmentChipVisual>,
    exiting: &[(AttachmentChipRun, AttachmentChipVisual)],
    active: bool,
) -> u64 {
    let mut hasher = DefaultHasher::new();
    active.hash(&mut hasher);
    for (key, visual) in sorted_u64_visual_entries(visuals) {
        key.hash(&mut hasher);
        hash_f32(visual.opacity, &mut hasher);
        hash_f32(visual.x_offset_pixels, &mut hasher);
        hash_f32(visual.y_offset_pixels, &mut hasher);
        hash_f32(visual.scale, &mut hasher);
    }
    for (run, visual) in exiting {
        run.key.hash(&mut hasher);
        run.index.hash(&mut hasher);
        hash_f32(visual.opacity, &mut hasher);
        hash_f32(visual.x_offset_pixels, &mut hasher);
        hash_f32(visual.y_offset_pixels, &mut hasher);
        hash_f32(visual.scale, &mut hasher);
    }
    hasher.finish()
}

fn stdin_overlay_target(
    app: &SingleSessionApp,
    rendered_body_lines: &[SingleSessionStyledLine],
) -> Option<StdinOverlayTarget> {
    let state = app.stdin_response.as_ref()?;
    let mut hasher = DefaultHasher::new();
    state.request_id.hash(&mut hasher);
    state.prompt.hash(&mut hasher);
    state.tool_call_id.hash(&mut hasher);
    state.is_password.hash(&mut hasher);
    let key = hasher.finish();
    let input_line_start = rendered_body_lines
        .iter()
        .position(|line| line.style == SingleSessionLineStyle::OverlaySelection)
        .unwrap_or_else(|| rendered_body_lines.len().saturating_sub(1));
    let input_line_count = rendered_body_lines
        .get(input_line_start..)
        .unwrap_or_default()
        .iter()
        .take_while(|line| line.style == SingleSessionLineStyle::OverlaySelection)
        .count()
        .max(1);
    Some(StdinOverlayTarget {
        key,
        line_count: rendered_body_lines.len().max(1),
        input_line_start,
        input_line_count,
        password: state.is_password,
        has_input: !state.input.is_empty(),
    })
}

fn stdin_overlay_visual_lerp(
    from: StdinOverlayVisual,
    to: StdinOverlayVisual,
    progress: f32,
) -> StdinOverlayVisual {
    StdinOverlayVisual {
        opacity: lerp_f32(from.opacity, to.opacity, progress),
        y_offset_pixels: lerp_f32(from.y_offset_pixels, to.y_offset_pixels, progress),
        scale: lerp_f32(from.scale, to.scale, progress),
        height_lines: lerp_f32(from.height_lines, to.height_lines, progress),
        input_glow: lerp_f32(from.input_glow, to.input_glow, progress),
        submit_opacity: lerp_f32(from.submit_opacity, to.submit_opacity, progress),
    }
}

fn stdin_overlay_exit_visual(from: StdinOverlayVisual, progress: f32) -> StdinOverlayVisual {
    let eased = ease_out_cubic_local(progress);
    StdinOverlayVisual {
        opacity: from.opacity * (1.0 - eased),
        y_offset_pixels: from.y_offset_pixels - STDIN_OVERLAY_ENTRY_OFFSET_PIXELS * 0.55 * eased,
        scale: from.scale * (1.0 - (1.0 - STDIN_OVERLAY_ENTRY_SCALE) * eased),
        height_lines: from.height_lines,
        input_glow: from.input_glow * (1.0 - eased * 0.45),
        submit_opacity: (from.submit_opacity + 0.35 * (1.0 - eased)).clamp(0.0, 1.0),
    }
}

fn stdin_overlay_motion_cache_key(
    current: Option<(StdinOverlayTarget, StdinOverlayVisual)>,
    exiting: Option<(StdinOverlayTarget, StdinOverlayVisual)>,
    active: bool,
) -> u64 {
    let mut hasher = DefaultHasher::new();
    active.hash(&mut hasher);
    current.is_some().hash(&mut hasher);
    if let Some((target, visual)) = current {
        stdin_overlay_target_hash(target, &mut hasher);
        stdin_overlay_visual_hash(visual, &mut hasher);
    }
    exiting.is_some().hash(&mut hasher);
    if let Some((target, visual)) = exiting {
        stdin_overlay_target_hash(target, &mut hasher);
        stdin_overlay_visual_hash(visual, &mut hasher);
    }
    hasher.finish()
}

fn stdin_overlay_target_hash(target: StdinOverlayTarget, hasher: &mut impl Hasher) {
    target.hash(hasher);
}

fn stdin_overlay_visual_hash(visual: StdinOverlayVisual, hasher: &mut impl Hasher) {
    hash_f32(visual.opacity, hasher);
    hash_f32(visual.y_offset_pixels, hasher);
    hash_f32(visual.scale, hasher);
    hash_f32(visual.height_lines, hasher);
    hash_f32(visual.input_glow, hasher);
    hash_f32(visual.submit_opacity, hasher);
}

fn tool_card_palette(state: SingleSessionToolVisualState, active: bool) -> ToolCardPalette {
    let accent = single_session_tool_state_accent(state);
    let background = single_session_tool_card_background(state, active);
    let border = if active || state.is_active() {
        TOOL_CARD_ACTIVE_BORDER_COLOR
    } else if matches!(
        state,
        SingleSessionToolVisualState::Succeeded | SingleSessionToolVisualState::Failed
    ) {
        with_alpha(accent, 0.44)
    } else {
        TOOL_CARD_BORDER_COLOR
    };
    let rail = if active || state.is_active() {
        TOOL_TIMELINE_ACTIVE_RAIL_COLOR
    } else {
        accent
    };
    let chip = mix_color(
        TOOL_STATUS_CHIP_COLOR,
        with_alpha(accent, TOOL_STATUS_CHIP_COLOR[3]),
        0.22,
    );
    ToolCardPalette {
        background,
        border,
        rail,
        chip,
    }
}

fn mix_tool_card_palette(
    from: ToolCardPalette,
    to: ToolCardPalette,
    progress: f32,
) -> ToolCardPalette {
    ToolCardPalette {
        background: mix_color(from.background, to.background, progress),
        border: mix_color(from.border, to.border, progress),
        rail: mix_color(from.rail, to.rail, progress),
        chip: mix_color(from.chip, to.chip, progress),
    }
}

fn tool_card_motion_cache_key(
    visuals: &HashMap<String, ToolCardVisual>,
    exiting: &[(SingleSessionToolCardRun, ToolCardVisual)],
    active: bool,
) -> u64 {
    let mut hasher = DefaultHasher::new();
    active.hash(&mut hasher);
    let mut entries = visuals.iter().collect::<Vec<_>>();
    entries.sort_by(|(left, _), (right, _)| left.cmp(right));
    for (call_id, visual) in entries {
        call_id.hash(&mut hasher);
        hash_f32(visual.opacity, &mut hasher);
        hash_f32(visual.y_offset_pixels, &mut hasher);
        hash_f32(visual.scale, &mut hasher);
        hash_color(visual.background, &mut hasher);
        hash_color(visual.border, &mut hasher);
        hash_color(visual.rail, &mut hasher);
        hash_color(visual.chip, &mut hasher);
        hash_f32(visual.output_reveal, &mut hasher);
        hash_color(visual.flash_color, &mut hasher);
        hash_f32(visual.flash_alpha, &mut hasher);
        hash_f32(visual.active_phase, &mut hasher);
    }
    for (run, visual) in exiting {
        run.call_id.hash(&mut hasher);
        run.line.hash(&mut hasher);
        run.line_count.hash(&mut hasher);
        hash_f32(visual.opacity, &mut hasher);
        hash_f32(visual.y_offset_pixels, &mut hasher);
        hash_f32(visual.scale, &mut hasher);
        hash_color(visual.background, &mut hasher);
        hash_color(visual.border, &mut hasher);
        hash_color(visual.rail, &mut hasher);
        hash_color(visual.chip, &mut hasher);
        hash_f32(visual.output_reveal, &mut hasher);
        hash_f32(visual.active_phase, &mut hasher);
    }
    hasher.finish()
}

fn hash_color(color: [f32; 4], hasher: &mut impl Hasher) {
    for component in color {
        hash_f32(component, hasher);
    }
}

fn hash_f32(value: f32, hasher: &mut impl Hasher) {
    value.to_bits().hash(hasher);
}

fn motion_occurrence_key(base_key: u64, occurrences: &mut HashMap<u64, usize>) -> u64 {
    let occurrence = occurrences.entry(base_key).or_insert(0);
    let occurrence_index = *occurrence;
    *occurrence += 1;

    let mut hasher = DefaultHasher::new();
    base_key.hash(&mut hasher);
    occurrence_index.hash(&mut hasher);
    hasher.finish()
}

fn sorted_u64_visual_entries<V>(visuals: &HashMap<u64, V>) -> Vec<(&u64, &V)> {
    let mut entries = visuals.iter().collect::<Vec<_>>();
    entries.sort_by_key(|(key, _)| **key);
    entries
}

fn hash_surface_motion_visual(visual: SurfaceMotionVisual, hasher: &mut impl Hasher) {
    hash_f32(visual.opacity, hasher);
    hash_f32(visual.y_offset_pixels, hasher);
    hash_f32(visual.scale, hasher);
}

fn surface_motion_visual_rect(rect: Rect, visual: SurfaceMotionVisual) -> Rect {
    let scale = visual.scale.clamp(0.01, 1.5);
    let width = rect.width * scale;
    let height = rect.height * scale;
    Rect {
        x: rect.x + (rect.width - width) * 0.5,
        y: rect.y + (rect.height - height) * 0.5 + visual.y_offset_pixels,
        width,
        height,
    }
}

fn surface_motion_alpha(mut color: [f32; 4], opacity: f32) -> [f32; 4] {
    color[3] *= opacity.clamp(0.0, 1.0);
    color
}

fn transcript_card_visual_from_state(
    state: &mut TranscriptCardMotionState,
    line_height: f32,
    now: Instant,
) -> (TranscriptCardVisual, bool) {
    let mut visual = TranscriptCardVisual::default();
    let mut active = false;

    if let Some(entered_at) = state.entered_at {
        let (progress, running) =
            timed_animation_progress(entered_at, now, TRANSCRIPT_CARD_ENTRY_DURATION);
        visual = SurfaceMotionVisual::entry(
            TRANSCRIPT_CARD_ENTRY_OFFSET_PIXELS,
            TRANSCRIPT_CARD_ENTRY_SCALE,
            progress,
        );
        active |= running;
        if !running {
            state.entered_at = None;
        }
    }

    if let Some(shift) = state.line_shift {
        let (progress, running) =
            timed_animation_progress(shift.started_at, now, TRANSCRIPT_CARD_SHIFT_DURATION);
        visual.apply_line_shift(shift.from_line, state.line, line_height, progress);
        active |= running;
        if !running {
            state.line_shift = None;
        }
    }

    (visual, active)
}

fn transcript_message_visual_from_state(
    state: &mut TranscriptMessageMotionState,
    line_height: f32,
    now: Instant,
) -> (TranscriptMessageVisual, bool) {
    let mut visual = TranscriptMessageVisual::default();
    let mut active = false;

    if let Some(entered_at) = state.entered_at {
        let (progress, running) =
            timed_animation_progress(entered_at, now, TRANSCRIPT_MESSAGE_ENTRY_DURATION);
        visual = SurfaceMotionVisual::entry(
            TRANSCRIPT_MESSAGE_ENTRY_OFFSET_PIXELS,
            TRANSCRIPT_MESSAGE_ENTRY_SCALE,
            progress,
        );
        active |= running;
        if !running {
            state.entered_at = None;
        }
    }

    if let Some(shift) = state.line_shift {
        let (progress, running) =
            timed_animation_progress(shift.started_at, now, TRANSCRIPT_MESSAGE_SHIFT_DURATION);
        visual.apply_line_shift(shift.from_line, state.run.line, line_height, progress);
        active |= running;
        if !running {
            state.line_shift = None;
        }
    }

    (visual, active)
}

fn exiting_transcript_card_visual(progress: f32) -> TranscriptCardVisual {
    SurfaceMotionVisual::exit(
        TRANSCRIPT_CARD_ENTRY_OFFSET_PIXELS,
        TRANSCRIPT_CARD_ENTRY_SCALE,
        0.42,
        1.35,
        progress,
    )
}

fn inline_markdown_pill_visual_from_state(
    state: &mut InlineMarkdownPillMotionState,
    line_height: f32,
    now: Instant,
) -> (InlineMarkdownPillVisual, bool) {
    let mut visual = InlineMarkdownPillVisual::default();
    let mut active = false;

    if let Some(entered_at) = state.entered_at {
        let (progress, running) =
            timed_animation_progress(entered_at, now, INLINE_MARKDOWN_PILL_ENTRY_DURATION);
        visual = SurfaceMotionVisual::entry(
            INLINE_MARKDOWN_PILL_ENTRY_OFFSET_PIXELS,
            INLINE_MARKDOWN_PILL_ENTRY_SCALE,
            progress,
        );
        active |= running;
        if !running {
            state.entered_at = None;
        }
    }

    if let Some(shift) = state.line_shift {
        let (progress, running) =
            timed_animation_progress(shift.started_at, now, INLINE_MARKDOWN_PILL_SHIFT_DURATION);
        visual.apply_line_shift(shift.from_line, state.run.line, line_height, progress);
        active |= running;
        if !running {
            state.line_shift = None;
        }
    }

    (visual, active)
}

fn exiting_inline_markdown_pill_visual(progress: f32) -> InlineMarkdownPillVisual {
    SurfaceMotionVisual::exit(
        INLINE_MARKDOWN_PILL_ENTRY_OFFSET_PIXELS,
        INLINE_MARKDOWN_PILL_ENTRY_SCALE,
        0.55,
        1.0,
        progress,
    )
}

fn transcript_card_visual_rect(rect: Rect, visual: TranscriptCardVisual) -> Rect {
    surface_motion_visual_rect(rect, visual)
}

fn transcript_card_alpha(color: [f32; 4], opacity: f32) -> [f32; 4] {
    surface_motion_alpha(color, opacity)
}

fn inline_markdown_pill_visual_rect(rect: Rect, visual: InlineMarkdownPillVisual) -> Rect {
    surface_motion_visual_rect(rect, visual)
}

fn inline_markdown_pill_alpha(color: [f32; 4], opacity: f32) -> [f32; 4] {
    surface_motion_alpha(color, opacity)
}

fn transcript_message_motion_key(
    lines: &[SingleSessionStyledLine],
    run: &TranscriptMessageRun,
    occurrences: &mut HashMap<u64, usize>,
) -> u64 {
    let base_key = transcript_message_motion_base_key(lines, run);
    motion_occurrence_key(base_key, occurrences)
}

fn transcript_message_motion_base_key(
    lines: &[SingleSessionStyledLine],
    run: &TranscriptMessageRun,
) -> u64 {
    let mut hasher = DefaultHasher::new();
    run.role.hash(&mut hasher);
    run.line_count.hash(&mut hasher);
    let end = run.line.saturating_add(run.line_count).min(lines.len());
    for line in &lines[run.line.min(lines.len())..end] {
        line.style.hash(&mut hasher);
        line.text.hash(&mut hasher);
        line.inline_spans.hash(&mut hasher);
        line.tool.hash(&mut hasher);
    }
    hasher.finish()
}

fn transcript_message_motion_cache_key(
    visuals: &HashMap<u64, TranscriptMessageVisual>,
    active: bool,
) -> u64 {
    let mut hasher = DefaultHasher::new();
    active.hash(&mut hasher);
    for (key, visual) in sorted_u64_visual_entries(visuals) {
        key.hash(&mut hasher);
        hash_surface_motion_visual(*visual, &mut hasher);
    }
    hasher.finish()
}

fn transcript_card_motion_key(
    lines: &[SingleSessionStyledLine],
    run: &SingleSessionTranscriptCardRun,
    occurrences: &mut HashMap<u64, usize>,
) -> u64 {
    let base_key = transcript_card_motion_base_key(lines, run);
    motion_occurrence_key(base_key, occurrences)
}

fn transcript_card_motion_base_key(
    lines: &[SingleSessionStyledLine],
    run: &SingleSessionTranscriptCardRun,
) -> u64 {
    let mut hasher = DefaultHasher::new();
    run.style.hash(&mut hasher);
    run.line_count.hash(&mut hasher);
    let end = run.line.saturating_add(run.line_count).min(lines.len());
    for line in &lines[run.line.min(lines.len())..end] {
        line.style.hash(&mut hasher);
        line.text.hash(&mut hasher);
        line.inline_spans.len().hash(&mut hasher);
    }
    hasher.finish()
}

fn transcript_card_motion_cache_key(
    visuals: &HashMap<u64, TranscriptCardVisual>,
    exiting: &[(SingleSessionTranscriptCardRun, TranscriptCardVisual)],
    active: bool,
) -> u64 {
    let mut hasher = DefaultHasher::new();
    active.hash(&mut hasher);
    for (key, visual) in sorted_u64_visual_entries(visuals) {
        key.hash(&mut hasher);
        hash_surface_motion_visual(*visual, &mut hasher);
    }
    for (run, visual) in exiting {
        run.line.hash(&mut hasher);
        run.line_count.hash(&mut hasher);
        run.style.hash(&mut hasher);
        hash_surface_motion_visual(*visual, &mut hasher);
    }
    hasher.finish()
}

fn inline_markdown_pill_motion_key(
    lines: &[SingleSessionStyledLine],
    run: &InlineMarkdownPillRun,
    occurrences: &mut HashMap<u64, usize>,
) -> u64 {
    let base_key = inline_markdown_pill_motion_base_key(lines, run);
    motion_occurrence_key(base_key, occurrences)
}

fn inline_markdown_pill_motion_base_key(
    lines: &[SingleSessionStyledLine],
    run: &InlineMarkdownPillRun,
) -> u64 {
    let mut hasher = DefaultHasher::new();
    run.kind.hash(&mut hasher);
    run.start_column.hash(&mut hasher);
    run.column_count.hash(&mut hasher);
    if let Some(line) = lines.get(run.line) {
        line.style.hash(&mut hasher);
        line.text.hash(&mut hasher);
        line.inline_spans.hash(&mut hasher);
    }
    hasher.finish()
}

fn inline_markdown_pill_motion_cache_key(
    visuals: &HashMap<u64, InlineMarkdownPillVisual>,
    exiting: &[(InlineMarkdownPillRun, InlineMarkdownPillVisual)],
    active: bool,
) -> u64 {
    let mut hasher = DefaultHasher::new();
    active.hash(&mut hasher);
    for (key, visual) in sorted_u64_visual_entries(visuals) {
        key.hash(&mut hasher);
        hash_surface_motion_visual(*visual, &mut hasher);
    }
    for (run, visual) in exiting {
        run.hash(&mut hasher);
        hash_surface_motion_visual(*visual, &mut hasher);
    }
    hasher.finish()
}

fn push_single_session_transcript_cards(
    vertices: &mut Vec<Vertex>,
    app: &SingleSessionApp,
    size: PhysicalSize<u32>,
    tick: u64,
    smooth_scroll_lines: f32,
) {
    let viewport = single_session_body_viewport_for_tick(app, size, tick, smooth_scroll_lines);
    push_single_session_transcript_cards_from_viewport(
        vertices,
        app,
        size,
        &viewport,
        viewport.total_lines,
        None,
    );
}

fn push_single_session_transcript_cards_from_viewport(
    vertices: &mut Vec<Vertex>,
    app: &SingleSessionApp,
    size: PhysicalSize<u32>,
    viewport: &SingleSessionBodyViewport,
    total_lines: usize,
    transcript_motion: Option<&TranscriptCardMotionFrame>,
) {
    let typography = single_session_typography_for_scale(app.text_scale());
    let line_height = typography.body_size * typography.body_line_height;
    let width = (single_session_content_right(size) - (PANEL_TITLE_LEFT_PADDING - 6.0)).max(1.0);
    let body_top = single_session_body_top_for_app(app, size);
    let body_bottom = single_session_body_bottom_for_total_lines(app, size, total_lines);

    let mut occurrences = HashMap::new();
    for run in single_session_transcript_card_runs(&viewport.lines) {
        let motion_key = transcript_card_motion_key(&viewport.lines, &run, &mut occurrences);
        let visual = transcript_motion
            .and_then(|motion| motion.visual_for_key(motion_key))
            .unwrap_or_default();
        push_single_session_transcript_card(
            vertices,
            run,
            visual,
            TranscriptCardGeometryContext {
                size,
                line_height,
                width,
                body_top,
                body_bottom,
                top_offset_pixels: viewport.top_offset_pixels,
            },
        );
    }

    if let Some(transcript_motion) = transcript_motion {
        for (run, visual) in transcript_motion.exiting() {
            push_single_session_transcript_card(
                vertices,
                *run,
                *visual,
                TranscriptCardGeometryContext {
                    size,
                    line_height,
                    width,
                    body_top,
                    body_bottom,
                    top_offset_pixels: viewport.top_offset_pixels,
                },
            );
        }
    }
}

fn push_single_session_transcript_message_highlights_from_viewport(
    vertices: &mut Vec<Vertex>,
    app: &SingleSessionApp,
    size: PhysicalSize<u32>,
    viewport: &SingleSessionBodyViewport,
    total_lines: usize,
    message_motion: Option<&TranscriptMessageMotionFrame>,
) {
    if app.messages.is_empty() && app.streaming_response.is_empty() && app.error.is_none() {
        return;
    }

    let typography = single_session_typography_for_scale(app.text_scale());
    let line_height = typography.body_size * typography.body_line_height;
    let width = (single_session_content_right(size) - (PANEL_TITLE_LEFT_PADDING - 7.0)).max(1.0);
    let body_top = single_session_body_top_for_app(app, size);
    let body_bottom = single_session_body_bottom_for_total_lines(app, size, total_lines);

    let mut occurrences = HashMap::new();
    for run in single_session_transcript_message_runs(&viewport.lines) {
        let motion_key = transcript_message_motion_key(&viewport.lines, &run, &mut occurrences);
        let visual = message_motion
            .and_then(|motion| motion.visual_for_key(motion_key))
            .unwrap_or_default();
        push_single_session_transcript_message_highlight(
            vertices,
            run,
            visual,
            TranscriptCardGeometryContext {
                size,
                line_height,
                width,
                body_top,
                body_bottom,
                top_offset_pixels: viewport.top_offset_pixels,
            },
        );
    }
}

fn push_single_session_transcript_message_highlight(
    vertices: &mut Vec<Vertex>,
    run: TranscriptMessageRun,
    visual: TranscriptMessageVisual,
    context: TranscriptCardGeometryContext,
) {
    if visual.opacity <= 0.001 {
        return;
    }
    let Some(color) = transcript_message_highlight_color(run.role) else {
        return;
    };
    let rect = Rect {
        x: PANEL_TITLE_LEFT_PADDING - 7.0,
        y: context.body_top
            + context.top_offset_pixels
            + run.line as f32 * context.line_height
            + 2.0,
        width: context.width,
        height: (run.line_count as f32 * context.line_height - 4.0).max(1.0),
    };
    let rect = transcript_message_visual_rect(rect, visual);
    let Some(rect) = clip_rect_to_vertical_bounds(rect, context.body_top, context.body_bottom)
    else {
        return;
    };
    let opacity = visual.opacity.clamp(0.0, 1.0);
    push_rounded_rect(
        vertices,
        rect,
        8.0,
        transcript_message_alpha(color, opacity),
        context.size,
    );
    push_rounded_rect(
        vertices,
        Rect {
            x: rect.x,
            y: rect.y + 2.0,
            width: 2.2,
            height: (rect.height - 4.0).max(1.0),
        },
        1.1,
        transcript_message_alpha(color, opacity * TRANSCRIPT_MESSAGE_ACCENT_ALPHA_MULTIPLIER),
        context.size,
    );
}

fn transcript_message_highlight_color(role: TranscriptMessageRole) -> Option<[f32; 4]> {
    Some(match role {
        TranscriptMessageRole::User => TRANSCRIPT_MESSAGE_USER_HIGHLIGHT_COLOR,
        TranscriptMessageRole::Assistant => TRANSCRIPT_MESSAGE_ASSISTANT_HIGHLIGHT_COLOR,
        TranscriptMessageRole::Meta => TRANSCRIPT_MESSAGE_META_HIGHLIGHT_COLOR,
        TranscriptMessageRole::Error => TRANSCRIPT_MESSAGE_ERROR_HIGHLIGHT_COLOR,
    })
}

fn transcript_message_visual_rect(rect: Rect, visual: TranscriptMessageVisual) -> Rect {
    surface_motion_visual_rect(rect, visual)
}

fn transcript_message_alpha(color: [f32; 4], opacity: f32) -> [f32; 4] {
    surface_motion_alpha(color, opacity)
}

#[derive(Clone, Copy)]
struct TranscriptCardGeometryContext {
    size: PhysicalSize<u32>,
    line_height: f32,
    width: f32,
    body_top: f32,
    body_bottom: f32,
    top_offset_pixels: f32,
}

fn push_single_session_transcript_card(
    vertices: &mut Vec<Vertex>,
    run: SingleSessionTranscriptCardRun,
    visual: TranscriptCardVisual,
    context: TranscriptCardGeometryContext,
) {
    let Some(color) = single_session_line_card_color(run.style) else {
        return;
    };
    if visual.opacity <= 0.001 {
        return;
    }
    let rect = Rect {
        x: PANEL_TITLE_LEFT_PADDING - 6.0,
        y: context.body_top
            + context.top_offset_pixels
            + run.line as f32 * context.line_height
            + 3.0,
        width: context.width,
        height: (run.line_count as f32 * context.line_height - 6.0).max(1.0),
    };
    let rect = transcript_card_visual_rect(rect, visual);
    let Some(rect) = clip_rect_to_vertical_bounds(rect, context.body_top, context.body_bottom)
    else {
        return;
    };
    push_rounded_rect(
        vertices,
        rect,
        7.0,
        transcript_card_alpha(color, visual.opacity),
        context.size,
    );
}

fn push_single_session_tool_cards(
    vertices: &mut Vec<Vertex>,
    app: &SingleSessionApp,
    size: PhysicalSize<u32>,
    tick: u64,
    smooth_scroll_lines: f32,
    tool_motion: Option<&ToolCardMotionFrame>,
) {
    let viewport = single_session_body_viewport_for_tick(app, size, tick, smooth_scroll_lines);
    push_single_session_tool_cards_from_viewport(
        vertices,
        app,
        size,
        &viewport,
        viewport.total_lines,
        tick,
        tool_motion,
    );
}

fn push_single_session_tool_cards_from_viewport(
    vertices: &mut Vec<Vertex>,
    app: &SingleSessionApp,
    size: PhysicalSize<u32>,
    viewport: &SingleSessionBodyViewport,
    total_lines: usize,
    tick: u64,
    tool_motion: Option<&ToolCardMotionFrame>,
) {
    let typography = single_session_typography_for_scale(app.text_scale());
    let line_height = typography.body_size * typography.body_line_height;
    let width = (single_session_content_right(size) - (PANEL_TITLE_LEFT_PADDING - 10.0)).max(1.0);
    let body_top = single_session_body_top_for_app(app, size);
    let body_bottom = single_session_body_bottom_for_total_lines(app, size, total_lines);
    let pulse = active_tool_card_pulse(tick);

    for run in single_session_tool_card_runs(&viewport.lines) {
        let rect = Rect {
            x: PANEL_TITLE_LEFT_PADDING - 10.0,
            y: body_top + viewport.top_offset_pixels + run.line as f32 * line_height + 2.0,
            width,
            height: (run.line_count as f32 * line_height - 4.0).max(1.0),
        };
        let Some(rect) = clip_rect_to_vertical_bounds(rect, body_top, body_bottom) else {
            continue;
        };
        let visual = tool_motion
            .and_then(|motion| motion.visual_for(&run.call_id))
            .unwrap_or_else(|| default_tool_card_visual(&run, pulse));
        push_single_session_tool_card(vertices, &run, rect, line_height, pulse, visual, size);
    }

    if let Some(tool_motion) = tool_motion {
        for (run, visual) in tool_motion.exiting() {
            let rect = Rect {
                x: PANEL_TITLE_LEFT_PADDING - 10.0,
                y: body_top + viewport.top_offset_pixels + run.line as f32 * line_height + 2.0,
                width,
                height: (run.line_count as f32 * line_height - 4.0).max(1.0),
            };
            let Some(rect) = clip_rect_to_vertical_bounds(rect, body_top, body_bottom) else {
                continue;
            };
            push_single_session_tool_card(vertices, run, rect, line_height, pulse, *visual, size);
        }
    }
}

fn push_single_session_tool_card(
    vertices: &mut Vec<Vertex>,
    run: &SingleSessionToolCardRun,
    rect: Rect,
    line_height: f32,
    _pulse: f32,
    visual: ToolCardVisual,
    size: PhysicalSize<u32>,
) {
    let radius = 9.0;
    let opacity = visual.opacity.clamp(0.0, 1.0);
    if opacity <= 0.001 {
        return;
    }
    let rect = tool_card_visual_rect(rect, visual);

    let shadow = Rect {
        x: rect.x + 1.5,
        y: rect.y + 2.0,
        width: rect.width,
        height: rect.height,
    };
    push_rounded_rect(
        vertices,
        shadow,
        radius,
        tool_card_alpha([0.030, 0.050, 0.090, 0.035], opacity),
        size,
    );
    push_rounded_rect(
        vertices,
        rect,
        radius,
        tool_card_alpha(visual.border, opacity),
        size,
    );
    let inner = Rect {
        x: rect.x + 1.0,
        y: rect.y + 1.0,
        width: (rect.width - 2.0).max(1.0),
        height: (rect.height - 2.0).max(1.0),
    };
    push_rounded_rect(
        vertices,
        inner,
        radius - 1.0,
        tool_card_alpha(visual.background, opacity),
        size,
    );

    if visual.flash_alpha > 0.001 {
        push_rounded_rect(
            vertices,
            inner,
            radius - 1.0,
            tool_card_alpha(with_alpha(visual.flash_color, visual.flash_alpha), opacity),
            size,
        );
        push_rounded_rect_border(
            vertices,
            rect,
            radius,
            1.5,
            tool_card_alpha(
                with_alpha(visual.flash_color, visual.flash_alpha * 1.35),
                opacity,
            ),
            size,
        );
    }

    let rail_rect = tool_card_rail_rect(rect);
    push_rounded_rect(
        vertices,
        rail_rect,
        rail_rect.width / 2.0,
        tool_card_alpha(visual.rail, opacity),
        size,
    );
    if run.active || run.state.is_active() {
        push_active_tool_card_motion(vertices, rect, rail_rect, visual, opacity, size);
    }

    let dot_size = 9.0;
    push_rounded_rect(
        vertices,
        Rect {
            x: rail_rect.x + (rail_rect.width - dot_size) * 0.5,
            y: rect.y + line_height * 0.44 - dot_size * 0.5,
            width: dot_size,
            height: dot_size,
        },
        dot_size / 2.0,
        tool_card_alpha(visual.rail, opacity),
        size,
    );

    let chip_width = (run.state.label().chars().count() as f32 * 8.0 + 24.0).clamp(52.0, 96.0);
    let chip_rect = Rect {
        x: rect.x + rect.width - chip_width - 10.0,
        y: rect.y + 7.0,
        width: chip_width,
        height: (line_height * 0.52).clamp(17.0, 25.0),
    };
    push_rounded_rect(
        vertices,
        chip_rect,
        chip_rect.height / 2.0,
        tool_card_alpha(visual.chip, opacity),
        size,
    );

    if run.detail_line_count > 0 {
        let drawer_target_height = (rect.height - line_height - 7.0).max(1.0);
        let drawer_height = (drawer_target_height * visual.output_reveal.clamp(0.0, 1.0)).max(1.0);
        let drawer = Rect {
            x: rect.x + 26.0,
            y: rect.y + line_height + 1.0,
            width: (rect.width - 38.0).max(1.0),
            height: drawer_height,
        };
        push_rounded_rect(
            vertices,
            drawer,
            7.0,
            tool_card_alpha(
                TOOL_OUTPUT_DRAWER_COLOR,
                opacity * visual.output_reveal.clamp(0.0, 1.0),
            ),
            size,
        );
    }
}

fn default_tool_card_visual(run: &SingleSessionToolCardRun, pulse: f32) -> ToolCardVisual {
    let mut palette = tool_card_palette(run.state, run.active);
    if run.active || run.state.is_active() {
        palette.background[3] = (palette.background[3] + 0.08 * pulse).clamp(0.0, 0.82);
        palette.border[3] = (palette.border[3] + 0.16 * pulse).clamp(0.0, 0.62);
        palette.rail[3] = (palette.rail[3] + 0.24 * pulse).clamp(0.0, 0.78);
    }
    ToolCardVisual {
        background: palette.background,
        border: palette.border,
        rail: palette.rail,
        chip: palette.chip,
        ..ToolCardVisual::default()
    }
}

fn tool_card_visual_rect(rect: Rect, visual: ToolCardVisual) -> Rect {
    let scale = visual.scale.clamp(0.01, 1.5);
    let width = rect.width * scale;
    let height = rect.height * scale;
    Rect {
        x: rect.x + (rect.width - width) * 0.5,
        y: rect.y + (rect.height - height) * 0.5 + visual.y_offset_pixels,
        width,
        height,
    }
}

fn tool_card_alpha(mut color: [f32; 4], opacity: f32) -> [f32; 4] {
    color[3] = (color[3] * opacity.clamp(0.0, 1.0)).clamp(0.0, 1.0);
    color
}

fn push_active_tool_card_motion(
    vertices: &mut Vec<Vertex>,
    rect: Rect,
    rail_rect: Rect,
    visual: ToolCardVisual,
    opacity: f32,
    size: PhysicalSize<u32>,
) {
    let phase = visual.active_phase.fract();
    let mut head_color = visual.rail;
    head_color[3] = (head_color[3] + 0.20).clamp(0.0, 0.92);
    let head_color = tool_card_alpha(head_color, opacity);

    let head_height = (rail_rect.height * 0.34)
        .clamp(10.0, 34.0)
        .min(rail_rect.height);
    let head_top = rail_rect.y - head_height + (rail_rect.height + head_height) * phase;
    let visible_top = head_top.max(rail_rect.y);
    let visible_bottom = (head_top + head_height).min(rail_rect.y + rail_rect.height);
    if visible_bottom > visible_top {
        push_rounded_rect(
            vertices,
            Rect {
                x: rail_rect.x - 0.5,
                y: visible_top,
                width: rail_rect.width + 1.0,
                height: (visible_bottom - visible_top).max(1.0),
            },
            (rail_rect.width + 1.0) * 0.5,
            head_color,
            size,
        );
    }

    let sweep_width = (rect.width * 0.16)
        .clamp(26.0, 92.0)
        .min(rect.width.max(1.0));
    let travel = rect.width + sweep_width;
    let sweep_x = rect.x - sweep_width + travel * phase;
    let top_rect = clipped_horizontal_sweep(sweep_x, sweep_width, rect.x, rect.x + rect.width).map(
        |(x, width)| Rect {
            x,
            y: rect.y + 1.0,
            width,
            height: 1.5,
        },
    );
    if let Some(top_rect) = top_rect {
        push_rounded_rect(vertices, top_rect, 1.0, head_color, size);
    }

    let reverse_x = rect.x - sweep_width + travel * (1.0 - phase);
    let bottom_rect = clipped_horizontal_sweep(reverse_x, sweep_width, rect.x, rect.x + rect.width)
        .map(|(x, width)| Rect {
            x,
            y: rect.y + rect.height - 2.5,
            width,
            height: 1.5,
        });
    if let Some(bottom_rect) = bottom_rect {
        push_rounded_rect(vertices, bottom_rect, 1.0, head_color, size);
    }
}

fn clipped_horizontal_sweep(x: f32, width: f32, min_x: f32, max_x: f32) -> Option<(f32, f32)> {
    let left = x.max(min_x);
    let right = (x + width).min(max_x);
    (right > left).then_some((left, right - left))
}

#[cfg(test)]
pub(crate) fn single_session_tool_card_geometries(
    app: &SingleSessionApp,
    size: PhysicalSize<u32>,
    rendered_body_lines: &[SingleSessionStyledLine],
) -> Vec<SingleSessionToolCardGeometry> {
    let typography = single_session_typography_for_scale(app.text_scale());
    let line_height = typography.body_size * typography.body_line_height;
    let width = (single_session_content_right(size) - (PANEL_TITLE_LEFT_PADDING - 10.0)).max(1.0);
    let body_top = single_session_body_top_for_app(app, size);

    single_session_tool_card_runs(rendered_body_lines)
        .into_iter()
        .map(|run| {
            let card_rect = Rect {
                x: PANEL_TITLE_LEFT_PADDING - 10.0,
                y: body_top + run.line as f32 * line_height + 2.0,
                width,
                height: (run.line_count as f32 * line_height - 4.0).max(1.0),
            };
            SingleSessionToolCardGeometry {
                run,
                rail_rect: tool_card_rail_rect(card_rect),
                card_rect,
                line_height,
            }
        })
        .collect()
}

pub(crate) fn single_session_tool_card_runs(
    lines: &[SingleSessionStyledLine],
) -> Vec<SingleSessionToolCardRun> {
    let mut runs = Vec::new();
    let mut current: Option<SingleSessionToolCardRun> = None;

    for (line, styled_line) in lines.iter().enumerate() {
        let Some(metadata) = styled_line.tool.as_ref() else {
            if let Some(run) = current.take() {
                runs.push(run);
            }
            continue;
        };

        match &mut current {
            Some(run) if run.call_id == metadata.call_id && run.line + run.line_count == line => {
                run.line_count += 1;
                run.active |= metadata.active;
                run.expanded |= metadata.expanded;
                if metadata.kind == SingleSessionToolLineKind::Detail {
                    run.detail_line_count += 1;
                }
                if metadata.state.is_active() || !run.state.is_active() {
                    run.state = metadata.state;
                }
            }
            Some(run) => {
                runs.push(run.clone());
                current = Some(tool_card_run_from_metadata(line, metadata));
            }
            None => current = Some(tool_card_run_from_metadata(line, metadata)),
        }
    }

    if let Some(run) = current {
        runs.push(run);
    }

    runs
}

fn tool_card_run_from_metadata(
    line: usize,
    metadata: &SingleSessionToolLineMetadata,
) -> SingleSessionToolCardRun {
    SingleSessionToolCardRun {
        line,
        line_count: 1,
        call_id: metadata.call_id.clone(),
        name: metadata.name.clone(),
        state: metadata.state,
        active: metadata.active,
        expanded: metadata.expanded,
        detail_line_count: usize::from(metadata.kind == SingleSessionToolLineKind::Detail),
        kind: metadata.kind,
    }
}

fn tool_card_rail_rect(card_rect: Rect) -> Rect {
    Rect {
        x: card_rect.x + 9.0,
        y: card_rect.y + 7.0,
        width: 3.0,
        height: (card_rect.height - 14.0).max(6.0),
    }
}

fn active_tool_card_pulse(tick: u64) -> f32 {
    let phase = (tick % 36) as f32 / 36.0;
    0.5 + 0.5 * (phase * std::f32::consts::TAU).sin()
}

fn single_session_tool_card_background(
    state: SingleSessionToolVisualState,
    active: bool,
) -> [f32; 4] {
    if active || state.is_active() {
        return TOOL_CARD_ACTIVE_BACKGROUND_COLOR;
    }
    match state {
        SingleSessionToolVisualState::Succeeded => TOOL_CARD_SUCCESS_BACKGROUND_COLOR,
        SingleSessionToolVisualState::Failed => TOOL_CARD_FAILED_BACKGROUND_COLOR,
        SingleSessionToolVisualState::Group => TOOL_CARD_GROUP_BACKGROUND_COLOR,
        _ => TOOL_CARD_BACKGROUND_COLOR,
    }
}

fn single_session_tool_state_accent(state: SingleSessionToolVisualState) -> [f32; 4] {
    match state {
        SingleSessionToolVisualState::Succeeded => TOOL_SUCCESS_TEXT_COLOR,
        SingleSessionToolVisualState::Failed => TOOL_FAILED_TEXT_COLOR,
        SingleSessionToolVisualState::Running => TOOL_RUNNING_TEXT_COLOR,
        SingleSessionToolVisualState::Preparing => TOOL_PENDING_TEXT_COLOR,
        SingleSessionToolVisualState::Group => TOOL_TEXT_COLOR,
        SingleSessionToolVisualState::Unknown => TOOL_TIMELINE_RAIL_COLOR,
    }
}

#[cfg(test)]
pub(crate) fn single_session_transcript_card_geometries(
    app: &SingleSessionApp,
    size: PhysicalSize<u32>,
    rendered_body_lines: &[SingleSessionStyledLine],
) -> Vec<SingleSessionTranscriptCardGeometry> {
    let typography = single_session_typography_for_scale(app.text_scale());
    let line_height = typography.body_size * typography.body_line_height;
    let width = (single_session_content_right(size) - (PANEL_TITLE_LEFT_PADDING - 6.0)).max(1.0);
    let body_top = single_session_body_top_for_app(app, size);

    single_session_transcript_card_runs(rendered_body_lines)
        .into_iter()
        .filter_map(|run| {
            single_session_line_card_color(run.style)?;
            let card_rect = Rect {
                x: PANEL_TITLE_LEFT_PADDING - 6.0,
                y: body_top + run.line as f32 * line_height + 3.0,
                width,
                height: (run.line_count as f32 * line_height - 6.0).max(1.0),
            };
            Some(SingleSessionTranscriptCardGeometry {
                run,
                card_rect,
                text_left: PANEL_TITLE_LEFT_PADDING,
                line_height,
            })
        })
        .collect()
}

fn push_single_session_inline_code_cards(
    vertices: &mut Vec<Vertex>,
    app: &SingleSessionApp,
    size: PhysicalSize<u32>,
    tick: u64,
    smooth_scroll_lines: f32,
) {
    let viewport = single_session_body_viewport_for_tick(app, size, tick, smooth_scroll_lines);
    push_single_session_inline_code_cards_from_viewport(
        vertices,
        app,
        size,
        &viewport,
        viewport.total_lines,
        None,
    );
}

fn push_single_session_inline_code_cards_from_viewport(
    vertices: &mut Vec<Vertex>,
    app: &SingleSessionApp,
    size: PhysicalSize<u32>,
    viewport: &SingleSessionBodyViewport,
    total_lines: usize,
    inline_markdown_motion: Option<&InlineMarkdownPillMotionFrame>,
) {
    if !viewport
        .lines
        .iter()
        .any(single_session_line_has_inline_code_or_math)
    {
        return;
    }

    let text_scale = app.text_scale();
    let typography = single_session_typography_for_scale(text_scale);
    let line_height = typography.body_size * typography.body_line_height;
    let char_width = single_session_body_char_width_for_scale(text_scale);
    let body_top = single_session_body_top_for_app(app, size);
    let body_bottom = single_session_body_bottom_for_total_lines(app, size, total_lines);
    let card_height = inline_code_card_height(&typography);
    let radius = (5.0 * text_scale).clamp(4.0, 8.0);
    let horizontal_pad = (3.5 * text_scale).clamp(3.0, 6.0);
    let pill_context = InlineMarkdownPillGeometryContext {
        size,
        line_height,
        char_width,
        body_top,
        body_bottom,
        card_height,
        radius,
        horizontal_pad,
        top_offset_pixels: viewport.top_offset_pixels,
    };
    let mut font_system = FontSystem::new();
    let body_buffer = single_session_body_text_buffer_from_lines(
        &mut font_system,
        &viewport.lines,
        size,
        text_scale,
    );
    let layout_runs = body_buffer.layout_runs().collect::<Vec<_>>();

    let mut occurrences = HashMap::new();
    for (line_index, line) in viewport.lines.iter().enumerate() {
        if !single_session_line_style_supports_inline_code_cards(line.style) {
            continue;
        }
        let line_y = layout_runs
            .get(line_index)
            .map(|run| body_top + viewport.top_offset_pixels + run.line_top)
            .unwrap_or(body_top + viewport.top_offset_pixels + line_index as f32 * line_height);
        let code_runs = single_session_inline_code_runs_for_line(line);
        for (run_index, run) in code_runs.iter().enumerate() {
            let glyph_bounds = layout_runs.get(line_index).and_then(|layout_run| {
                line.inline_spans
                    .iter()
                    .filter(|span| span.kind == SingleSessionInlineSpanKind::Code)
                    .nth(run_index)
                    .and_then(|span| {
                        layout_run
                            .highlight(
                                glyphon::Cursor::new(layout_run.line_i, span.start),
                                glyphon::Cursor::new(layout_run.line_i, span.end),
                            )
                            .and_then(|(left, width)| (width > 0.0).then_some((left, left + width)))
                    })
            });
            let (x, width) = if let Some((glyph_left, glyph_right)) = glyph_bounds {
                let x = PANEL_TITLE_LEFT_PADDING + glyph_left - horizontal_pad;
                (x, glyph_right - glyph_left + horizontal_pad * 2.0)
            } else {
                (
                    PANEL_TITLE_LEFT_PADDING + run.start_column as f32 * char_width
                        - horizontal_pad,
                    run.column_count as f32 * char_width + horizontal_pad * 2.0,
                )
            };
            let clipped_right = (x + width).min(size.width as f32);
            if clipped_right <= x {
                continue;
            }
            let rect = Rect {
                x,
                y: line_y + (line_height - card_height) * 0.5,
                width: clipped_right - x,
                height: card_height,
            };
            let pill_run = InlineMarkdownPillRun {
                line: line_index,
                start_column: run.start_column,
                column_count: run.column_count,
                kind: InlineMarkdownPillKind::Code,
            };
            let motion_key =
                inline_markdown_pill_motion_key(&viewport.lines, &pill_run, &mut occurrences);
            let visual = inline_markdown_motion
                .and_then(|motion| motion.visual_for_key(motion_key))
                .unwrap_or_default();
            push_single_session_inline_markdown_pill_rect(
                vertices,
                rect,
                InlineMarkdownPillKind::Code,
                visual,
                pill_context,
            );
        }
        for run in single_session_inline_math_runs_for_line(line) {
            if code_runs.iter().any(|code_run| {
                inline_markdown_runs_overlap(
                    run.start_column,
                    run.column_count,
                    code_run.start_column,
                    code_run.column_count,
                )
            }) {
                continue;
            }
            let x =
                PANEL_TITLE_LEFT_PADDING + run.start_column as f32 * char_width - horizontal_pad;
            let width = run.column_count as f32 * char_width + horizontal_pad * 2.0;
            let clipped_right = (x + width).min(size.width as f32);
            if clipped_right <= x {
                continue;
            }
            let rect = Rect {
                x,
                y: line_y + (line_height - card_height) * 0.5,
                width: clipped_right - x,
                height: card_height,
            };
            let pill_run = InlineMarkdownPillRun {
                line: line_index,
                start_column: run.start_column,
                column_count: run.column_count,
                kind: InlineMarkdownPillKind::Math,
            };
            let motion_key =
                inline_markdown_pill_motion_key(&viewport.lines, &pill_run, &mut occurrences);
            let visual = inline_markdown_motion
                .and_then(|motion| motion.visual_for_key(motion_key))
                .unwrap_or_default();
            push_single_session_inline_markdown_pill_rect(
                vertices,
                rect,
                InlineMarkdownPillKind::Math,
                visual,
                pill_context,
            );
        }
    }

    if let Some(inline_markdown_motion) = inline_markdown_motion {
        for (run, visual) in inline_markdown_motion.exiting() {
            push_single_session_inline_markdown_pill_run(vertices, *run, *visual, pill_context);
        }
    }
}

#[derive(Clone, Copy)]
struct InlineMarkdownPillGeometryContext {
    size: PhysicalSize<u32>,
    line_height: f32,
    char_width: f32,
    body_top: f32,
    body_bottom: f32,
    card_height: f32,
    radius: f32,
    horizontal_pad: f32,
    top_offset_pixels: f32,
}

fn push_single_session_inline_markdown_pill_run(
    vertices: &mut Vec<Vertex>,
    run: InlineMarkdownPillRun,
    visual: InlineMarkdownPillVisual,
    context: InlineMarkdownPillGeometryContext,
) {
    let x = PANEL_TITLE_LEFT_PADDING + run.start_column as f32 * context.char_width
        - context.horizontal_pad;
    let width = run.column_count as f32 * context.char_width + context.horizontal_pad * 2.0;
    let clipped_right = (x + width).min(context.size.width as f32);
    if clipped_right <= x {
        return;
    }
    let line_y =
        context.body_top + context.top_offset_pixels + run.line as f32 * context.line_height;
    let rect = Rect {
        x,
        y: line_y + (context.line_height - context.card_height) * 0.5,
        width: clipped_right - x,
        height: context.card_height,
    };
    push_single_session_inline_markdown_pill_rect(vertices, rect, run.kind, visual, context);
}

fn push_single_session_inline_markdown_pill_rect(
    vertices: &mut Vec<Vertex>,
    rect: Rect,
    kind: InlineMarkdownPillKind,
    visual: InlineMarkdownPillVisual,
    context: InlineMarkdownPillGeometryContext,
) {
    if visual.opacity <= 0.001 {
        return;
    }
    let rect = inline_markdown_pill_visual_rect(rect, visual);
    let Some(rect) = clip_rect_to_vertical_bounds(rect, context.body_top, context.body_bottom)
    else {
        return;
    };
    push_rounded_rect(
        vertices,
        rect,
        context.radius,
        inline_markdown_pill_alpha(inline_markdown_pill_color(kind), visual.opacity),
        context.size,
    );
}

fn inline_markdown_pill_color(kind: InlineMarkdownPillKind) -> [f32; 4] {
    match kind {
        InlineMarkdownPillKind::Code => INLINE_CODE_BACKGROUND_COLOR,
        InlineMarkdownPillKind::Math => INLINE_MATH_BACKGROUND_COLOR,
    }
}

fn single_session_line_has_inline_code_or_math(line: &SingleSessionStyledLine) -> bool {
    line.inline_spans.iter().any(|span| {
        matches!(
            span.kind,
            SingleSessionInlineSpanKind::Code | SingleSessionInlineSpanKind::Math
        )
    }) || line.text.contains('$')
}

fn inline_code_card_height(typography: &SingleSessionTypography) -> f32 {
    let line_height = typography.body_size * typography.body_line_height;
    line_height + 2.0
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub(crate) struct SingleSessionInlineCodeRun {
    pub(crate) start_column: usize,
    pub(crate) column_count: usize,
}

pub(crate) fn single_session_inline_code_runs(text: &str) -> Vec<SingleSessionInlineCodeRun> {
    let mut runs = Vec::new();
    let mut search_start = 0;

    while let Some(open_rel) = text[search_start..].find('`') {
        let open = search_start + open_rel;
        let code_start = open + '`'.len_utf8();
        let Some(close_rel) = text[code_start..].find('`') else {
            break;
        };
        let close = code_start + close_rel;
        let after_close = close + '`'.len_utf8();
        let start_column = text[..open].chars().count();
        let column_count = text[open..after_close].chars().count();
        if column_count > 1 {
            runs.push(SingleSessionInlineCodeRun {
                start_column,
                column_count,
            });
        }
        search_start = after_close;
    }

    runs
}

pub(crate) fn single_session_inline_code_runs_for_line(
    line: &SingleSessionStyledLine,
) -> Vec<SingleSessionInlineCodeRun> {
    if line.inline_spans.is_empty() {
        return single_session_inline_code_runs(&line.text);
    }
    line.inline_spans
        .iter()
        .filter(|span| span.kind == SingleSessionInlineSpanKind::Code)
        .filter_map(|span| inline_code_run_from_span(&line.text, span))
        .collect()
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub(crate) struct SingleSessionInlineMathRun {
    pub(crate) start_column: usize,
    pub(crate) column_count: usize,
}

pub(crate) fn single_session_inline_math_runs(text: &str) -> Vec<SingleSessionInlineMathRun> {
    let mut runs = Vec::new();
    let mut search_start = 0;
    let code_ranges = single_session_inline_code_byte_ranges(text);

    while let Some(open_rel) = text[search_start..].find('$') {
        let open = search_start + open_rel;
        if byte_index_inside_any_range(open, &code_ranges) {
            search_start = open + '$'.len_utf8();
            continue;
        }
        if text[open..].starts_with("$$") {
            search_start = open + '$'.len_utf8();
            continue;
        }
        let math_start = open + '$'.len_utf8();
        let Some(close_rel) = text[math_start..].find('$') else {
            break;
        };
        let close = math_start + close_rel;
        if text[close..].starts_with("$$") || close == math_start {
            search_start = close + '$'.len_utf8();
            continue;
        }
        let after_close = close + '$'.len_utf8();
        if byte_range_overlaps_any_range(open, after_close, &code_ranges) {
            search_start = after_close;
            continue;
        }
        let start_column = text[..open].chars().count();
        let column_count = text[open..after_close].chars().count();
        runs.push(SingleSessionInlineMathRun {
            start_column,
            column_count,
        });
        search_start = after_close;
    }

    runs
}

pub(crate) fn single_session_inline_math_runs_for_line(
    line: &SingleSessionStyledLine,
) -> Vec<SingleSessionInlineMathRun> {
    if line.inline_spans.is_empty() {
        return single_session_inline_math_runs(&line.text);
    }
    line.inline_spans
        .iter()
        .filter(|span| span.kind == SingleSessionInlineSpanKind::Math)
        .filter_map(|span| inline_math_run_from_span(&line.text, span))
        .collect()
}

fn single_session_inline_markdown_pill_runs(
    lines: &[SingleSessionStyledLine],
) -> Vec<InlineMarkdownPillRun> {
    let mut runs = Vec::new();
    for (line_index, line) in lines.iter().enumerate() {
        if !single_session_line_style_supports_inline_code_cards(line.style) {
            continue;
        }
        let code_runs = single_session_inline_code_runs_for_line(line);
        runs.extend(code_runs.iter().map(|run| InlineMarkdownPillRun {
            line: line_index,
            start_column: run.start_column,
            column_count: run.column_count,
            kind: InlineMarkdownPillKind::Code,
        }));
        runs.extend(
            single_session_inline_math_runs_for_line(line)
                .into_iter()
                .filter(|math_run| {
                    !code_runs.iter().any(|code_run| {
                        inline_markdown_runs_overlap(
                            math_run.start_column,
                            math_run.column_count,
                            code_run.start_column,
                            code_run.column_count,
                        )
                    })
                })
                .map(|run| InlineMarkdownPillRun {
                    line: line_index,
                    start_column: run.start_column,
                    column_count: run.column_count,
                    kind: InlineMarkdownPillKind::Math,
                }),
        );
    }
    runs
}

fn inline_code_run_from_span(
    text: &str,
    span: &SingleSessionInlineSpan,
) -> Option<SingleSessionInlineCodeRun> {
    let (start_column, column_count) = inline_run_columns_from_span(text, span)?;
    (column_count > 0).then_some(SingleSessionInlineCodeRun {
        start_column,
        column_count,
    })
}

fn inline_math_run_from_span(
    text: &str,
    span: &SingleSessionInlineSpan,
) -> Option<SingleSessionInlineMathRun> {
    let (start_column, column_count) = inline_run_columns_from_span(text, span)?;
    (column_count > 0).then_some(SingleSessionInlineMathRun {
        start_column,
        column_count,
    })
}

fn inline_run_columns_from_span(
    text: &str,
    span: &SingleSessionInlineSpan,
) -> Option<(usize, usize)> {
    if span.start >= span.end || span.end > text.len() {
        return None;
    }
    let content = text.get(span.start..span.end)?;
    let start_column = text.get(..span.start)?.chars().count();
    let column_count = content.chars().count();
    Some((start_column, column_count))
}

fn single_session_inline_code_byte_ranges(text: &str) -> Vec<(usize, usize)> {
    let mut ranges = Vec::new();
    let mut search_start = 0;

    while let Some(open_rel) = text[search_start..].find('`') {
        let open = search_start + open_rel;
        let code_start = open + '`'.len_utf8();
        let Some(close_rel) = text[code_start..].find('`') else {
            break;
        };
        let close = code_start + close_rel;
        let after_close = close + '`'.len_utf8();
        ranges.push((open, after_close));
        search_start = after_close;
    }

    ranges
}

fn byte_index_inside_any_range(index: usize, ranges: &[(usize, usize)]) -> bool {
    ranges
        .iter()
        .any(|(start, end)| *start <= index && index < *end)
}

fn byte_range_overlaps_any_range(start: usize, end: usize, ranges: &[(usize, usize)]) -> bool {
    ranges
        .iter()
        .any(|(range_start, range_end)| start < *range_end && *range_start < end)
}

fn inline_markdown_runs_overlap(
    start_a: usize,
    count_a: usize,
    start_b: usize,
    count_b: usize,
) -> bool {
    let end_a = start_a.saturating_add(count_a);
    let end_b = start_b.saturating_add(count_b);
    start_a < end_b && start_b < end_a
}

fn single_session_line_style_supports_inline_code_cards(style: SingleSessionLineStyle) -> bool {
    matches!(
        style,
        SingleSessionLineStyle::Assistant
            | SingleSessionLineStyle::AssistantHeading
            | SingleSessionLineStyle::AssistantQuote
            | SingleSessionLineStyle::AssistantLink
            | SingleSessionLineStyle::AssistantMedia
    )
}

fn push_single_session_markdown_rule_lines(
    vertices: &mut Vec<Vertex>,
    app: &SingleSessionApp,
    size: PhysicalSize<u32>,
    tick: u64,
    smooth_scroll_lines: f32,
) {
    let viewport = single_session_body_viewport_for_tick(app, size, tick, smooth_scroll_lines);
    push_single_session_markdown_rule_lines_from_viewport(
        vertices,
        app,
        size,
        &viewport,
        viewport.total_lines,
    );
}

fn push_single_session_markdown_rule_lines_from_viewport(
    vertices: &mut Vec<Vertex>,
    app: &SingleSessionApp,
    size: PhysicalSize<u32>,
    viewport: &SingleSessionBodyViewport,
    total_lines: usize,
) {
    let typography = single_session_typography_for_scale(app.text_scale());
    let line_height = typography.body_size * typography.body_line_height;
    let body_top = single_session_body_top_for_app(app, size);
    let body_bottom = single_session_body_bottom_for_total_lines(app, size, total_lines);
    let left = PANEL_TITLE_LEFT_PADDING - 2.0;
    let right = single_session_content_right(size).max(left + 1.0);
    let thickness = (1.7 * app.text_scale()).clamp(1.0, 3.0);

    for (line_index, line) in viewport.lines.iter().enumerate() {
        if !is_single_session_markdown_rule_line(line) {
            continue;
        }
        let center_y = body_top
            + viewport.top_offset_pixels
            + line_index as f32 * line_height
            + line_height * 0.5;
        let rect = Rect {
            x: left,
            y: center_y - thickness * 0.5,
            width: right - left,
            height: thickness,
        };
        let Some(rect) = clip_rect_to_vertical_bounds(rect, body_top, body_bottom) else {
            continue;
        };
        push_rounded_rect(vertices, rect, thickness, MARKDOWN_RULE_COLOR, size);
    }
}

fn is_single_session_markdown_rule_line(line: &SingleSessionStyledLine) -> bool {
    if line.style != SingleSessionLineStyle::Meta {
        return false;
    }
    let trimmed = line.text.trim();
    trimmed.chars().count() >= 3 && trimmed.chars().all(|ch| ch == '─')
}

fn push_single_session_scrollbar(
    vertices: &mut Vec<Vertex>,
    app: &SingleSessionApp,
    size: PhysicalSize<u32>,
    tick: u64,
    smooth_scroll_lines: f32,
    motion: Option<&SingleSessionScrollbarMotionFrame>,
) {
    let Some(metrics) = single_session_body_scroll_metrics(app, size, tick) else {
        return;
    };
    push_single_session_scrollbar_for_metrics(vertices, size, smooth_scroll_lines, metrics, motion);
}

fn push_single_session_scrollbar_for_total_lines(
    vertices: &mut Vec<Vertex>,
    app: &SingleSessionApp,
    size: PhysicalSize<u32>,
    smooth_scroll_lines: f32,
    total_lines: usize,
    motion: Option<&SingleSessionScrollbarMotionFrame>,
) {
    let Some(metrics) = single_session_body_scroll_metrics_for_total_lines(app, size, total_lines)
    else {
        return;
    };
    push_single_session_scrollbar_for_metrics(vertices, size, smooth_scroll_lines, metrics, motion);
}

fn push_single_session_scrollbar_for_metrics(
    vertices: &mut Vec<Vertex>,
    size: PhysicalSize<u32>,
    smooth_scroll_lines: f32,
    metrics: SingleSessionBodyScrollMetrics,
    motion: Option<&SingleSessionScrollbarMotionFrame>,
) {
    let track_top = single_session_scrollbar_track_top();
    let track_bottom = single_session_scrollbar_track_bottom(size);
    let track_height = (track_bottom - track_top).max(1.0);
    let x = single_session_scrollbar_track_x(size);
    let fallback_geometry = single_session_scrollbar_geometry(size, smooth_scroll_lines, metrics);
    let visual = match motion {
        Some(motion) => match motion.visual() {
            Some(visual) => visual,
            None => return,
        },
        None => SingleSessionScrollbarVisual {
            thumb_y: fallback_geometry.thumb_y,
            thumb_height: fallback_geometry.thumb_height,
            opacity: 1.0,
        },
    };
    if visual.opacity <= 0.001 {
        return;
    }

    push_rounded_rect(
        vertices,
        Rect {
            x,
            y: track_top,
            width: SINGLE_SESSION_SCROLLBAR_TRACK_WIDTH,
            height: track_height,
        },
        2.0,
        with_alpha(
            SINGLE_SESSION_SCROLLBAR_TRACK_COLOR,
            SINGLE_SESSION_SCROLLBAR_TRACK_COLOR[3] * visual.opacity,
        ),
        size,
    );
    push_rounded_rect(
        vertices,
        Rect {
            x: x - 0.5,
            y: visual.thumb_y,
            width: 4.0,
            height: visual.thumb_height,
        },
        2.0,
        with_alpha(
            SINGLE_SESSION_SCROLLBAR_THUMB_COLOR,
            SINGLE_SESSION_SCROLLBAR_THUMB_COLOR[3] * visual.opacity,
        ),
        size,
    );
}

fn single_session_scrollbar_track_top() -> f32 {
    PANEL_BODY_TOP_PADDING + 4.0
}

fn single_session_scrollbar_track_bottom(size: PhysicalSize<u32>) -> f32 {
    single_session_body_bottom(size) - 4.0
}

#[derive(Clone, Copy, Debug, PartialEq)]
pub(crate) struct SingleSessionBodyScrollMetrics {
    pub(crate) total_lines: usize,
    pub(crate) visible_lines: usize,
    pub(crate) scroll_lines: f32,
    pub(crate) max_scroll_lines: usize,
}

pub(crate) fn single_session_body_scroll_metrics(
    app: &SingleSessionApp,
    size: PhysicalSize<u32>,
    tick: u64,
) -> Option<SingleSessionBodyScrollMetrics> {
    let _ = tick;
    let total_lines = welcome_timeline_total_body_lines(app, size);
    single_session_body_scroll_metrics_for_total_lines(app, size, total_lines)
}

pub(crate) fn single_session_body_scroll_metrics_for_total_lines(
    app: &SingleSessionApp,
    size: PhysicalSize<u32>,
    total_lines: usize,
) -> Option<SingleSessionBodyScrollMetrics> {
    let typography = single_session_typography_for_scale(app.text_scale());
    let line_height = typography.body_size * typography.body_line_height;
    let body_top = single_session_body_top_for_app(app, size);
    let body_bottom = single_session_body_bottom_for_total_lines(app, size, total_lines);
    let available_height = (body_bottom - body_top).max(line_height);
    let visible_lines = ((available_height / line_height).floor() as usize).max(1);
    let max_scroll_lines = total_lines.saturating_sub(visible_lines);
    (max_scroll_lines > 0).then_some(SingleSessionBodyScrollMetrics {
        total_lines,
        visible_lines,
        scroll_lines: app.body_scroll_lines.min(max_scroll_lines as f32),
        max_scroll_lines,
    })
}

pub(crate) fn single_session_transcript_card_runs(
    lines: &[SingleSessionStyledLine],
) -> Vec<SingleSessionTranscriptCardRun> {
    let mut runs = Vec::new();
    let mut current: Option<SingleSessionTranscriptCardRun> = None;

    for (line, styled_line) in lines.iter().enumerate() {
        if single_session_line_card_color(styled_line.style).is_none() {
            if let Some(run) = current.take() {
                runs.push(run);
            }
            continue;
        }

        match &mut current {
            Some(run)
                if single_session_line_card_color(run.style)
                    == single_session_line_card_color(styled_line.style)
                    && run.line + run.line_count == line =>
            {
                run.line_count += 1;
            }
            Some(run) => {
                runs.push(*run);
                current = Some(SingleSessionTranscriptCardRun {
                    line,
                    line_count: 1,
                    style: styled_line.style,
                });
            }
            None => {
                current = Some(SingleSessionTranscriptCardRun {
                    line,
                    line_count: 1,
                    style: styled_line.style,
                });
            }
        }
    }

    if let Some(run) = current {
        runs.push(run);
    }
    runs
}

pub(crate) fn single_session_transcript_message_runs(
    lines: &[SingleSessionStyledLine],
) -> Vec<TranscriptMessageRun> {
    let mut runs = Vec::new();
    let mut current: Option<TranscriptMessageRun> = None;

    for (line, styled_line) in lines.iter().enumerate() {
        let Some(role) = transcript_message_role_for_style(styled_line.style) else {
            if let Some(run) = current.take() {
                runs.push(run);
            }
            continue;
        };

        match &mut current {
            Some(run) if run.role == role && run.line + run.line_count == line => {
                run.line_count += 1;
            }
            Some(run) => {
                runs.push(*run);
                current = Some(TranscriptMessageRun {
                    line,
                    line_count: 1,
                    role,
                });
            }
            None => {
                current = Some(TranscriptMessageRun {
                    line,
                    line_count: 1,
                    role,
                });
            }
        }
    }

    if let Some(run) = current {
        runs.push(run);
    }
    runs
}

fn transcript_message_role_for_style(
    style: SingleSessionLineStyle,
) -> Option<TranscriptMessageRole> {
    match style {
        SingleSessionLineStyle::User | SingleSessionLineStyle::UserContinuation => {
            Some(TranscriptMessageRole::User)
        }
        SingleSessionLineStyle::Assistant
        | SingleSessionLineStyle::AssistantHeading
        | SingleSessionLineStyle::AssistantQuote
        | SingleSessionLineStyle::AssistantTable
        | SingleSessionLineStyle::AssistantLink
        | SingleSessionLineStyle::AssistantMedia
        | SingleSessionLineStyle::CodeHeader
        | SingleSessionLineStyle::Code => Some(TranscriptMessageRole::Assistant),
        SingleSessionLineStyle::Meta | SingleSessionLineStyle::Status => {
            Some(TranscriptMessageRole::Meta)
        }
        SingleSessionLineStyle::Error => Some(TranscriptMessageRole::Error),
        SingleSessionLineStyle::Tool
        | SingleSessionLineStyle::OverlayTitle
        | SingleSessionLineStyle::Overlay
        | SingleSessionLineStyle::OverlaySelection
        | SingleSessionLineStyle::Blank => None,
    }
}

fn single_session_line_card_color(style: SingleSessionLineStyle) -> Option<[f32; 4]> {
    match style {
        SingleSessionLineStyle::AssistantHeading => Some(MARKDOWN_HEADING_BACKGROUND_COLOR),
        SingleSessionLineStyle::CodeHeader | SingleSessionLineStyle::Code => {
            Some(CODE_BLOCK_BACKGROUND_COLOR)
        }
        SingleSessionLineStyle::AssistantQuote => Some(QUOTE_CARD_BACKGROUND_COLOR),
        SingleSessionLineStyle::AssistantTable => Some(TABLE_CARD_BACKGROUND_COLOR),
        SingleSessionLineStyle::AssistantMedia => Some(MARKDOWN_MEDIA_BACKGROUND_COLOR),
        SingleSessionLineStyle::Error => Some(ERROR_CARD_BACKGROUND_COLOR),
        SingleSessionLineStyle::OverlaySelection => Some(OVERLAY_SELECTION_BACKGROUND_COLOR),
        _ => None,
    }
}

fn push_single_session_selection(
    vertices: &mut Vec<Vertex>,
    app: &SingleSessionApp,
    size: PhysicalSize<u32>,
) {
    if !app.has_body_selection() && !app.has_draft_selection() {
        return;
    }

    let typography = single_session_typography_for_scale(app.text_scale());
    let line_height = typography.body_size * typography.body_line_height;
    let char_width = single_session_body_char_width();
    let visible_lines = single_session_visible_body(app, size);
    let body_top = single_session_body_top_for_app(app, size);
    for segment in app.selection_segments(&visible_lines) {
        let selected_columns = segment
            .end_column
            .saturating_sub(segment.start_column)
            .max(1);
        push_rect(
            vertices,
            Rect {
                x: PANEL_TITLE_LEFT_PADDING - 2.0 + segment.start_column as f32 * char_width,
                y: body_top + segment.line as f32 * line_height,
                width: selected_columns as f32 * char_width + 4.0,
                height: line_height,
            },
            SELECTION_HIGHLIGHT_COLOR,
            size,
        );
    }

    if welcome_status_lane_visible(app) {
        return;
    }
    let typography = single_session_typography_for_scale(app.text_scale());
    let line_height = typography.code_size * typography.code_line_height;
    let char_width = typography.code_size * 0.58;
    let draft_top = single_session_draft_top_for_app(app, size);
    for segment in app.draft_selection_segments() {
        let selected_columns = segment
            .end_column
            .saturating_sub(segment.start_column)
            .max(1);
        push_rect(
            vertices,
            Rect {
                x: PANEL_TITLE_LEFT_PADDING - 2.0 + segment.start_column as f32 * char_width,
                y: draft_top + segment.line as f32 * line_height,
                width: selected_columns as f32 * char_width + 4.0,
                height: line_height,
            },
            SELECTION_HIGHLIGHT_COLOR,
            size,
        );
    }
}

pub(crate) fn push_single_session_caret(
    vertices: &mut Vec<Vertex>,
    app: &SingleSessionApp,
    size: PhysicalSize<u32>,
    draft_buffer: Option<&Buffer>,
) {
    if welcome_status_lane_visible(app) {
        return;
    }

    let caret = draft_buffer
        .and_then(|buffer| glyphon_draft_caret_position(app, buffer, size))
        .unwrap_or_else(|| approximate_draft_caret_position(app, size));

    push_rect(
        vertices,
        Rect {
            x: caret.x,
            y: caret.y,
            width: SINGLE_SESSION_CARET_WIDTH,
            height: caret.height,
        },
        SINGLE_SESSION_CARET_COLOR,
        size,
    );
}

#[derive(Clone, Copy, Debug, PartialEq)]
pub(crate) struct CaretPosition {
    pub(crate) x: f32,
    pub(crate) y: f32,
    pub(crate) height: f32,
}

pub(crate) fn glyphon_draft_caret_position(
    app: &SingleSessionApp,
    draft_buffer: &Buffer,
    size: PhysicalSize<u32>,
) -> Option<CaretPosition> {
    let typography = single_session_typography_for_scale(app.text_scale());
    let target = app.composer_cursor_line_byte_index();
    let target_line = target.0;
    let target_index = target.1;
    let mut fallback = None;

    for run in draft_buffer.layout_runs() {
        if run.line_i != target_line {
            continue;
        }
        let y = single_session_draft_top_for_app(app, size) + run.line_top;
        let height = typography.code_size * 1.12;
        if run.glyphs.is_empty() {
            return Some(CaretPosition {
                x: PANEL_TITLE_LEFT_PADDING,
                y,
                height,
            });
        }

        let first = run.glyphs.first()?;
        let last = run.glyphs.last()?;
        let mut run_position = CaretPosition {
            x: PANEL_TITLE_LEFT_PADDING + last.x + last.w,
            y,
            height,
        };
        if target_index <= first.start {
            run_position.x = PANEL_TITLE_LEFT_PADDING + first.x;
            return Some(run_position);
        }
        for glyph in run.glyphs {
            if target_index <= glyph.start {
                run_position.x = PANEL_TITLE_LEFT_PADDING + glyph.x;
                return Some(run_position);
            }
            if target_index <= glyph.end {
                run_position.x = PANEL_TITLE_LEFT_PADDING + glyph.x + glyph.w;
                return Some(run_position);
            }
        }
        if target_index >= first.start && target_index >= last.end {
            fallback = Some(run_position);
        }
    }

    fallback
}

pub(crate) fn approximate_draft_caret_position(
    app: &SingleSessionApp,
    size: PhysicalSize<u32>,
) -> CaretPosition {
    let typography = single_session_typography_for_scale(app.text_scale());
    let line_height = typography.code_size * typography.code_line_height;
    let draft_top = single_session_draft_top_for_app(app, size);
    let (cursor_line, cursor_column) = app.draft_cursor_line_col();
    let char_width = typography.code_size * 0.58;
    let prompt_column = if cursor_line == 0 {
        app.composer_prompt().chars().count()
    } else {
        0
    };
    let x = PANEL_TITLE_LEFT_PADDING
        + ((prompt_column + cursor_column) as f32 * char_width)
            .min((single_session_content_width(size)).max(0.0));
    let y = draft_top + cursor_line as f32 * line_height;
    CaretPosition {
        x,
        y,
        height: typography.code_size * 1.12,
    }
}

pub(crate) fn single_session_draft_line_col_at_position(
    app: &SingleSessionApp,
    size: PhysicalSize<u32>,
    x: f32,
    y: f32,
) -> Option<(usize, usize)> {
    let typography = single_session_typography_for_scale(app.text_scale());
    let line_height = typography.code_size * typography.code_line_height;
    let draft_top = single_session_draft_top_for_app(app, size);
    let draft_bottom = size.height as f32 - PANEL_TITLE_TOP_PADDING;
    if y < draft_top || y > draft_bottom || x < PANEL_TITLE_LEFT_PADDING {
        return None;
    }

    let line = ((y - draft_top) / line_height).floor().max(0.0) as usize;
    let draft_lines: Vec<&str> = app.draft.split('\n').collect();
    let line = line.min(draft_lines.len().saturating_sub(1));
    let char_width = typography.code_size * 0.58;
    let raw_column = ((x - PANEL_TITLE_LEFT_PADDING) / char_width)
        .round()
        .max(0.0) as usize;
    let prompt_columns = if line == 0 {
        app.composer_prompt().chars().count()
    } else {
        0
    };
    let draft_column = raw_column.saturating_sub(prompt_columns);
    let max_column = draft_lines
        .get(line)
        .map(|text| text.chars().count())
        .unwrap_or_default();
    Some((line, draft_column.min(max_column)))
}

pub(crate) fn single_session_draft_top(size: PhysicalSize<u32>) -> f32 {
    (size.height as f32 - SINGLE_SESSION_DRAFT_TOP_OFFSET).max(112.0)
}

pub(crate) fn single_session_draft_top_for_app(
    app: &SingleSessionApp,
    size: PhysicalSize<u32>,
) -> f32 {
    if app.is_welcome_timeline_visible() {
        if app.render_inline_widget_line_count() > 0 {
            return single_session_draft_top(size);
        }
        if app.has_welcome_timeline_transcript() {
            return welcome_timeline_draft_top(app, size);
        }
        return fresh_welcome_draft_top_for_scale(size, app.text_scale());
    }

    single_session_draft_top(size)
}

fn welcome_timeline_draft_top(app: &SingleSessionApp, size: PhysicalSize<u32>) -> f32 {
    welcome_timeline_draft_top_for_total_lines(
        app,
        size,
        welcome_timeline_total_body_lines(app, size),
    )
}

fn welcome_timeline_draft_top_for_total_lines(
    app: &SingleSessionApp,
    size: PhysicalSize<u32>,
    total_lines: usize,
) -> f32 {
    let typography = single_session_typography_for_scale(app.text_scale());
    let line_height = typography.body_size * typography.body_line_height;
    let body_top = PANEL_BODY_TOP_PADDING;
    let timeline_lines = total_lines.max(1) as f32;
    let desired = body_top + timeline_lines * line_height + welcome_timeline_body_draft_gap();
    let clamped = desired.min(single_session_draft_top(size));
    if clamped > body_top {
        clamped
    } else {
        clamped.max(fresh_welcome_draft_top(size))
    }
}

fn single_session_draft_top_for_total_lines(
    app: &SingleSessionApp,
    size: PhysicalSize<u32>,
    total_lines: usize,
) -> f32 {
    if app.is_welcome_timeline_visible() {
        if app.render_inline_widget_line_count() > 0 {
            return single_session_draft_top(size);
        }
        if app.has_welcome_timeline_transcript() {
            return welcome_timeline_draft_top_for_total_lines(app, size, total_lines);
        }
        return fresh_welcome_draft_top_for_scale(size, app.text_scale());
    }

    single_session_draft_top(size)
}

fn welcome_timeline_body_draft_gap() -> f32 {
    let typography = single_session_typography();
    let body_line_height = typography.body_size * typography.body_line_height;
    let composer_line_height = typography.code_size * typography.code_line_height;
    body_line_height.max(composer_line_height * 0.86)
}

fn welcome_timeline_total_body_lines(app: &SingleSessionApp, size: PhysicalSize<u32>) -> usize {
    let transcript_lines =
        single_session_wrapped_body_lines(app.body_styled_lines(), size, app.text_scale()).len();
    if app.is_welcome_timeline_visible() && app.has_welcome_timeline_transcript() {
        welcome_timeline_virtual_body_lines(app, size) + transcript_lines
    } else {
        transcript_lines
    }
}

fn welcome_timeline_virtual_body_lines(app: &SingleSessionApp, size: PhysicalSize<u32>) -> usize {
    // Reserve scrollable visual space for the handwritten hero without adding
    // the hero phrase to transcript text or model-derived body lines.
    let typography = single_session_typography_for_scale(app.text_scale());
    let line_height = typography.body_size * typography.body_line_height;
    ((fresh_welcome_visual_bottom(size) - PANEL_BODY_TOP_PADDING).max(0.0) / line_height)
        .ceil()
        .max(0.0) as usize
}

pub(crate) fn single_session_draft_top_for_fresh_state(
    size: PhysicalSize<u32>,
    fresh_welcome_visible: bool,
) -> f32 {
    if fresh_welcome_visible {
        fresh_welcome_draft_top(size)
    } else {
        single_session_draft_top(size)
    }
}

pub(crate) fn fresh_welcome_draft_top(size: PhysicalSize<u32>) -> f32 {
    fresh_welcome_draft_top_for_scale(size, 1.0)
}

fn fresh_welcome_draft_top_for_scale(size: PhysicalSize<u32>, ui_scale: f32) -> f32 {
    let hero_bottom = handwritten_welcome_bounds_for_phrase_with_scale(
        size,
        handwritten_welcome_phrase(0),
        ui_scale,
    )
    .1[1];
    let typography = single_session_typography_for_scale(ui_scale);
    let version_clearance = fresh_welcome_version_gap_for_scale(ui_scale)
        + fresh_welcome_version_font_size() * ui_scale * 1.4
        + (typography.body_size * 0.38).max(8.0);
    let clearance = (typography.code_size * 1.85)
        .max(version_clearance)
        .max(54.0);
    hero_bottom + clearance
}

fn fresh_welcome_visual_bottom(size: PhysicalSize<u32>) -> f32 {
    fresh_welcome_visual_bottom_for_scale(size, 1.0)
}

fn fresh_welcome_visual_bottom_for_scale(size: PhysicalSize<u32>, ui_scale: f32) -> f32 {
    fresh_welcome_version_top_for_scale(size, ui_scale)
        + fresh_welcome_version_font_size() * ui_scale * 1.4
}

fn fresh_welcome_inline_widget_gap_for_scale(ui_scale: f32) -> f32 {
    let typography = single_session_typography_for_scale(ui_scale);
    (typography.body_size * 0.58).max(10.0 * ui_scale)
}

#[cfg(test)]
pub(crate) fn single_session_text_buffers(
    app: &SingleSessionApp,
    size: PhysicalSize<u32>,
    font_system: &mut FontSystem,
) -> Vec<Buffer> {
    let key = single_session_text_key(app, size);
    single_session_text_buffers_from_key(&key, size, font_system)
}

#[cfg(test)]
pub(crate) fn single_session_text_key(
    app: &SingleSessionApp,
    size: PhysicalSize<u32>,
) -> SingleSessionTextKey {
    single_session_text_key_for_tick(app, size, 0)
}

#[cfg(test)]
pub(crate) fn single_session_text_key_for_tick(
    app: &SingleSessionApp,
    size: PhysicalSize<u32>,
    tick: u64,
) -> SingleSessionTextKey {
    single_session_text_key_for_tick_with_scroll(app, size, tick, 0.0)
}

pub(crate) fn single_session_text_key_for_tick_with_scroll(
    app: &SingleSessionApp,
    size: PhysicalSize<u32>,
    tick: u64,
    smooth_scroll_lines: f32,
) -> SingleSessionTextKey {
    let rendered_body_lines = single_session_rendered_body_lines_for_tick(app, size, tick);
    single_session_text_key_for_tick_with_rendered_body(
        app,
        size,
        tick,
        smooth_scroll_lines,
        &rendered_body_lines,
    )
}

pub(crate) fn single_session_text_key_for_tick_with_rendered_body(
    app: &SingleSessionApp,
    size: PhysicalSize<u32>,
    tick: u64,
    smooth_scroll_lines: f32,
    rendered_body_lines: &[SingleSessionStyledLine],
) -> SingleSessionTextKey {
    let viewport = single_session_body_viewport_from_lines(
        app,
        size,
        smooth_scroll_lines,
        rendered_body_lines,
    );
    let welcome_chrome_offset_pixels = welcome_timeline_visual_offset_pixels_for_total_lines(
        app,
        size,
        smooth_scroll_lines,
        viewport.total_lines,
    );
    let welcome_chrome_visible =
        welcome_timeline_chrome_visible(app, size, welcome_chrome_offset_pixels);
    single_session_text_key_for_body_lines(
        app,
        size,
        tick,
        viewport.top_offset_pixels,
        viewport.lines,
        welcome_chrome_visible,
    )
}

fn inline_widget_split_preview_start(
    kind: Option<InlineWidgetKind>,
    lines: &[SingleSessionStyledLine],
) -> Option<usize> {
    if kind != Some(InlineWidgetKind::SessionSwitcher) {
        return None;
    }
    lines
        .iter()
        .position(|line| line.text.starts_with("Preview"))
}

fn inline_widget_split_primary_lines(
    kind: Option<InlineWidgetKind>,
    lines: Vec<SingleSessionStyledLine>,
) -> Vec<SingleSessionStyledLine> {
    let Some(preview_start) = inline_widget_split_preview_start(kind, &lines) else {
        return lines;
    };
    lines
        .into_iter()
        .enumerate()
        .map(|(index, line)| {
            if index < preview_start {
                line
            } else {
                blank_render_line()
            }
        })
        .collect()
}

fn inline_widget_split_preview_lines(
    kind: Option<InlineWidgetKind>,
    lines: &[SingleSessionStyledLine],
) -> Vec<SingleSessionStyledLine> {
    let Some(preview_start) = inline_widget_split_preview_start(kind, lines) else {
        return Vec::new();
    };
    lines[preview_start..].to_vec()
}

fn single_session_text_key_for_body_lines(
    app: &SingleSessionApp,
    size: PhysicalSize<u32>,
    _tick: u64,
    body_top_offset_pixels: f32,
    body: Vec<SingleSessionStyledLine>,
    welcome_chrome_visible: bool,
) -> SingleSessionTextKey {
    let welcome_handoff_visible = false;
    let welcome_input_visible = true;
    let (welcome_hero, welcome_hint) = if welcome_chrome_visible {
        let welcome_hint = if app.draft.is_empty() {
            let mut lines = vec![SingleSessionStyledLine::new(
                "Type a message to start. Ask me to build, debug, explain, or automate something.",
                SingleSessionLineStyle::Meta,
            )];
            if let Some(suggestion) = app.welcome_continuation_suggestion() {
                lines.push(SingleSessionStyledLine::new(
                    format!("Suggestion: {suggestion}"),
                    SingleSessionLineStyle::Status,
                ));
            }
            lines
        } else {
            Vec::new()
        };
        (app.welcome_hero_text(), welcome_hint)
    } else if app.is_fresh_welcome_visible() && app.draft.is_empty() {
        let mut lines = vec![SingleSessionStyledLine::new(
            "Type a message to start. Ask me to build, debug, explain, or automate something.",
            SingleSessionLineStyle::Meta,
        )];
        if let Some(suggestion) = app.welcome_continuation_suggestion() {
            lines.push(SingleSessionStyledLine::new(
                format!("Suggestion: {suggestion}"),
                SingleSessionLineStyle::Status,
            ));
        }
        (String::new(), lines)
    } else {
        (String::new(), Vec::new())
    };
    let inline_widget_kind = app.render_inline_widget_kind();
    let inline_widget = app.render_inline_widget_styled_lines();
    let inline_widget_preview =
        inline_widget_split_preview_lines(inline_widget_kind, &inline_widget);
    let inline_widget = inline_widget_split_primary_lines(inline_widget_kind, inline_widget);
    SingleSessionTextKey {
        size: (size.width, size.height),
        fresh_welcome_visible: welcome_chrome_visible,
        title: if welcome_chrome_visible {
            String::new()
        } else {
            app.header_title()
        },
        version: if welcome_chrome_visible {
            if welcome_input_visible {
                fresh_welcome_version_label()
            } else {
                String::new()
            }
        } else {
            desktop_header_version_label()
        },
        welcome_hero,
        welcome_hint,
        activity_active: app.has_activity_indicator(),
        welcome_handoff_visible,
        text_scale_bits: app.text_scale().to_bits(),
        body_top_offset_pixels_bits: body_top_offset_pixels.to_bits(),
        user_font_family: single_session_user_font_family(),
        assistant_font_family: single_session_assistant_font_family(),
        body,
        inline_widget_kind,
        inline_widget,
        inline_widget_preview,
        draft: if welcome_input_visible {
            visualize_composer_whitespace(&app.composer_text())
        } else {
            String::new()
        },
    }
}

pub(crate) fn single_session_text_buffers_from_key(
    key: &SingleSessionTextKey,
    size: PhysicalSize<u32>,
    font_system: &mut FontSystem,
) -> Vec<Buffer> {
    single_session_text_buffers_from_key_reusing_unchanged(
        key,
        None,
        Vec::new(),
        false,
        size,
        font_system,
    )
}

pub(crate) fn single_session_text_buffers_from_key_reusing_unchanged(
    key: &SingleSessionTextKey,
    previous_key: Option<&SingleSessionTextKey>,
    old_buffers: Vec<Buffer>,
    reuse_body_buffer: bool,
    size: PhysicalSize<u32>,
    font_system: &mut FontSystem,
) -> Vec<Buffer> {
    single_session_text_buffers_from_key_reusing_unchanged_from_options(
        key,
        previous_key,
        old_buffers.into_iter().map(Some).collect(),
        reuse_body_buffer,
        size,
        font_system,
    )
}

fn single_session_text_buffers_from_key_reusing_unchanged_from_options(
    key: &SingleSessionTextKey,
    previous_key: Option<&SingleSessionTextKey>,
    mut old_buffers: Vec<Option<Buffer>>,
    reuse_body_buffer: bool,
    size: PhysicalSize<u32>,
    font_system: &mut FontSystem,
) -> Vec<Buffer> {
    let text_scale = f32::from_bits(key.text_scale_bits);
    let typography = single_session_typography_for_scale(text_scale);
    let content_width = single_session_content_width(size);

    let draft_top = if key.fresh_welcome_visible {
        fresh_welcome_draft_top_for_scale(size, text_scale)
    } else {
        single_session_draft_top_for_fresh_state(size, false)
    };
    let prompt_height = (size.height as f32 - draft_top - 18.0)
        .max(typography.code_size * typography.code_line_height * 2.0);
    let version_font_size = if key.fresh_welcome_visible {
        fresh_welcome_version_font_size()
    } else {
        typography.meta_size
    };

    let user_font_compatible = previous_key.is_some_and(|previous| {
        previous.user_font_family == key.user_font_family
            && previous.assistant_font_family == key.assistant_font_family
    });
    let exact_layout_compatible = previous_key.is_some_and(|previous| {
        previous.size == key.size
            && previous.text_scale_bits == key.text_scale_bits
            && user_font_compatible
    });
    let body_layout_compatible = previous_key.is_some_and(|previous| {
        previous.text_scale_bits == key.text_scale_bits
            && single_session_body_text_buffer_layout_bucket(previous.size, text_scale)
                == single_session_body_text_buffer_layout_bucket(key.size, text_scale)
            && user_font_compatible
    });
    let take_reusable =
        |old_buffers: &mut Vec<Option<Buffer>>, index: usize, reusable: bool| -> Option<Buffer> {
            if !reusable {
                return None;
            }
            old_buffers.get_mut(index).and_then(Option::take)
        };
    let exact_previous = previous_key.filter(|_| exact_layout_compatible);
    let body_previous = previous_key.filter(|_| body_layout_compatible);

    let title_buffer = take_reusable(
        &mut old_buffers,
        0,
        exact_previous.is_some_and(|previous| previous.title == key.title),
    )
    .unwrap_or_else(|| {
        single_session_text_buffer(
            font_system,
            &key.title,
            typography.title_size,
            typography.title_size * typography.meta_line_height,
            content_width,
            48.0,
        )
    });

    let body_buffer = take_reusable(
        &mut old_buffers,
        1,
        (reuse_body_buffer && user_font_compatible)
            || body_previous.is_some_and(|previous| previous.body == key.body),
    )
    .unwrap_or_else(|| {
        single_session_body_text_buffer_from_lines(font_system, &key.body, size, text_scale)
    });

    let inline_widget_line_count = inline_widget_visual_line_count(
        key.inline_widget_kind,
        &key.inline_widget,
        &key.inline_widget_preview,
    );
    let inline_widget_width = if inline_widget_line_count == 0 {
        content_width
    } else {
        inline_widget_text_width_for_split_buffers(
            key.inline_widget_kind,
            &key.inline_widget,
            &key.inline_widget_preview,
            size,
            text_scale,
        )
        .max(1.0)
        .min(content_width)
    };
    let inline_widget_height = if key.inline_widget.is_empty() {
        prompt_height
    } else {
        let inline_widget_line_height =
            inline_widget_line_height(key.inline_widget_kind, &typography);
        prompt_height
            .max(size.height as f32)
            .max(inline_widget_line_count as f32 * inline_widget_line_height)
    };
    let (inline_widget_primary_width, inline_widget_preview_width) =
        inline_widget_split_text_widths(
            key.inline_widget_kind,
            &typography,
            size,
            inline_widget_line_count,
            inline_widget_width,
        );
    let inline_widget_buffer = take_reusable(
        &mut old_buffers,
        4,
        exact_previous.is_some_and(|previous| {
            previous.inline_widget == key.inline_widget
                && previous.inline_widget_kind == key.inline_widget_kind
        }),
    )
    .unwrap_or_else(|| {
        let inline_widget_font_size = inline_widget_font_size(key.inline_widget_kind, &typography);
        let inline_widget_line_height =
            inline_widget_line_height(key.inline_widget_kind, &typography);
        let inline_widget_wrap =
            if key.inline_widget_kind == Some(InlineWidgetKind::SlashSuggestions) {
                Wrap::None
            } else {
                Wrap::Word
            };
        single_session_styled_text_buffer(
            font_system,
            &key.inline_widget,
            inline_widget_font_size,
            inline_widget_line_height,
            inline_widget_primary_width,
            inline_widget_height,
            inline_widget_wrap,
        )
    });

    let inline_widget_preview_buffer = take_reusable(
        &mut old_buffers,
        7,
        exact_previous.is_some_and(|previous| {
            previous.inline_widget_preview == key.inline_widget_preview
                && previous.inline_widget_kind == key.inline_widget_kind
        }),
    )
    .unwrap_or_else(|| {
        let inline_widget_font_size = inline_widget_font_size(key.inline_widget_kind, &typography);
        let inline_widget_line_height =
            inline_widget_line_height(key.inline_widget_kind, &typography);
        let inline_widget_preview_height = inline_widget_estimated_wrapped_text_height(
            key.inline_widget_kind,
            &key.inline_widget_preview,
            inline_widget_preview_width,
            &typography,
        )
        .min(inline_widget_height)
        .max(inline_widget_line_height);
        single_session_styled_text_buffer(
            font_system,
            &key.inline_widget_preview,
            inline_widget_font_size,
            inline_widget_line_height,
            inline_widget_preview_width,
            inline_widget_preview_height,
            Wrap::Word,
        )
    });

    let draft_buffer = take_reusable(
        &mut old_buffers,
        2,
        exact_previous.is_some_and(|previous| previous.draft == key.draft),
    )
    .unwrap_or_else(|| {
        single_session_text_buffer_with_family(
            font_system,
            &key.draft,
            key.user_font_family,
            typography.code_size,
            typography.code_size * typography.code_line_height,
            content_width,
            prompt_height,
        )
    });

    let version_buffer = take_reusable(
        &mut old_buffers,
        3,
        exact_previous.is_some_and(|previous| previous.version == key.version),
    )
    .unwrap_or_else(|| {
        single_session_text_buffer(
            font_system,
            &key.version,
            version_font_size,
            version_font_size * typography.meta_line_height,
            content_width,
            24.0,
        )
    });

    let (hero_min, hero_max) = glyph_welcome_hero_bounds(size, text_scale);
    let hero_width = (hero_max[0] - hero_min[0]).max(1.0);
    let hero_height = (hero_max[1] - hero_min[1]).max(1.0);
    let hero_font_size = glyph_welcome_hero_font_size(size, text_scale);
    let hero_buffer = take_reusable(
        &mut old_buffers,
        5,
        exact_previous.is_some_and(|previous| previous.welcome_hero == key.welcome_hero),
    )
    .unwrap_or_else(|| {
        single_session_text_buffer_with_family(
            font_system,
            &key.welcome_hero,
            SINGLE_SESSION_WELCOME_FONT_FAMILY,
            hero_font_size,
            hero_font_size * 1.18,
            hero_width,
            hero_height,
        )
    });

    let welcome_hint_buffer = take_reusable(
        &mut old_buffers,
        6,
        exact_previous.is_some_and(|previous| previous.welcome_hint == key.welcome_hint),
    )
    .unwrap_or_else(|| {
        single_session_styled_text_buffer(
            font_system,
            &key.welcome_hint,
            typography.meta_size,
            typography.meta_size * typography.meta_line_height,
            content_width,
            48.0,
            Wrap::Word,
        )
    });

    vec![
        title_buffer,
        body_buffer,
        draft_buffer,
        version_buffer,
        inline_widget_buffer,
        hero_buffer,
        welcome_hint_buffer,
        inline_widget_preview_buffer,
    ]
}

fn inline_widget_visual_line_count(
    kind: Option<InlineWidgetKind>,
    primary: &[SingleSessionStyledLine],
    preview: &[SingleSessionStyledLine],
) -> usize {
    if kind != Some(InlineWidgetKind::SessionSwitcher) || preview.is_empty() {
        return primary.len();
    }
    primary.len().max(preview.len())
}

fn inline_widget_text_width_for_split_buffers(
    kind: Option<InlineWidgetKind>,
    primary: &[SingleSessionStyledLine],
    preview: &[SingleSessionStyledLine],
    size: PhysicalSize<u32>,
    ui_scale: f32,
) -> f32 {
    if kind != Some(InlineWidgetKind::SessionSwitcher) || preview.is_empty() {
        return inline_widget_text_width_for_lines(kind, primary, size, ui_scale);
    }

    let typography = single_session_typography_for_scale(ui_scale);
    let average_char_width = inline_widget_font_size(kind, &typography) * 0.57;
    let max_columns = primary
        .iter()
        .chain(preview.iter())
        .map(|line| inline_widget_visual_columns(&line.text))
        .max()
        .unwrap_or_default() as f32;
    (max_columns * average_char_width)
        .ceil()
        .min(inline_widget_max_text_width_for_kind(kind, size))
}

fn inline_widget_estimated_wrapped_text_height(
    kind: Option<InlineWidgetKind>,
    lines: &[SingleSessionStyledLine],
    width: f32,
    typography: &SingleSessionTypography,
) -> f32 {
    let line_height = inline_widget_line_height(kind, typography);
    if lines.is_empty() {
        return line_height;
    }

    let average_char_width = inline_widget_font_size(kind, typography) * 0.57;
    let columns_per_line = (width / average_char_width).floor().max(1.0) as usize;
    let visual_lines = lines
        .iter()
        .map(|line| {
            inline_widget_visual_columns(&line.text)
                .max(1)
                .div_ceil(columns_per_line)
        })
        .sum::<usize>();

    // glyphon::Buffer::shape_until_scroll is intentionally viewport-limited;
    // leave a small amount of slack so the last row is shaped even when glyph
    // metrics or word wrapping round up slightly differently than this cheap
    // column estimate. This keeps split previews compact without restoring the
    // old full-window buffer height.
    visual_lines.saturating_add(2) as f32 * line_height
}

fn inline_widget_split_text_widths(
    kind: Option<InlineWidgetKind>,
    typography: &SingleSessionTypography,
    size: PhysicalSize<u32>,
    line_count: usize,
    full_text_width: f32,
) -> (f32, f32) {
    if kind != Some(InlineWidgetKind::SessionSwitcher) || line_count == 0 {
        return (full_text_width, 1.0);
    }
    let Some(layout) = inline_widget_card_layout(
        size,
        kind,
        typography,
        line_count,
        full_text_width,
        PANEL_TITLE_TOP_PADDING,
        1.0,
    ) else {
        return (full_text_width, full_text_width);
    };
    let Some(columns) = session_switcher_split_columns(&layout) else {
        return (full_text_width, full_text_width);
    };
    (
        (columns.rail.width - INLINE_COMMAND_ROW_INSET_X * 2.0).max(1.0),
        (columns.preview.width - layout.padding_x * 1.8).max(1.0),
    )
}

pub(crate) fn single_session_body_text_buffer_from_lines(
    font_system: &mut FontSystem,
    lines: &[SingleSessionStyledLine],
    size: PhysicalSize<u32>,
    text_scale: f32,
) -> Buffer {
    single_session_body_text_buffer_from_lines_with_opacity(
        font_system,
        lines,
        size,
        text_scale,
        1.0,
    )
}

pub(crate) fn single_session_body_text_buffer_from_lines_with_opacity(
    font_system: &mut FontSystem,
    lines: &[SingleSessionStyledLine],
    size: PhysicalSize<u32>,
    text_scale: f32,
    opacity: f32,
) -> Buffer {
    let typography = single_session_typography_for_scale(text_scale);
    let content_width = single_session_content_width(size);
    let mut buffer = single_session_styled_text_buffer_with_opacity(
        font_system,
        lines,
        typography.body_size,
        typography.body_size * typography.body_line_height,
        content_width,
        single_session_body_text_buffer_layout_height(size, text_scale),
        Wrap::None,
        opacity,
    );
    buffer.shape_until(font_system, i32::MAX);
    buffer
}

pub(crate) fn single_session_body_layout_cache_size(
    app: &SingleSessionApp,
    size: PhysicalSize<u32>,
) -> (u32, u32) {
    let max_columns =
        single_session_body_max_columns(size, app.text_scale()).min(u32::MAX as usize) as u32;
    let welcome_virtual_lines =
        if app.is_welcome_timeline_visible() && app.has_welcome_timeline_transcript() {
            welcome_timeline_virtual_body_lines(app, size).min(u32::MAX as usize) as u32
        } else {
            0
        };
    (max_columns, welcome_virtual_lines)
}

pub(crate) fn single_session_body_text_buffer_layout_compatible(
    previous_size: (u32, u32),
    size: PhysicalSize<u32>,
    text_scale: f32,
) -> bool {
    single_session_body_text_buffer_layout_bucket(previous_size, text_scale)
        == single_session_body_text_buffer_layout_bucket((size.width, size.height), text_scale)
}

fn single_session_body_text_buffer_layout_bucket(size: (u32, u32), text_scale: f32) -> (u32, u32) {
    let physical_size = PhysicalSize::new(size.0, size.1);
    let width_columns =
        single_session_body_max_columns(physical_size, text_scale).min(u32::MAX as usize) as u32;
    let height_lines = single_session_body_text_buffer_layout_lines(physical_size, text_scale)
        .min(u32::MAX as usize) as u32;
    (width_columns, height_lines)
}

fn single_session_body_text_buffer_layout_height(size: PhysicalSize<u32>, text_scale: f32) -> f32 {
    let typography = single_session_typography_for_scale(text_scale);
    let line_height = typography.body_size * typography.body_line_height;
    single_session_body_text_buffer_layout_lines(size, text_scale) as f32 * line_height
}

fn single_session_body_text_buffer_layout_lines(size: PhysicalSize<u32>, text_scale: f32) -> usize {
    let typography = single_session_typography_for_scale(text_scale);
    let line_height = typography.body_size * typography.body_line_height;
    let available_height = (size.height as f32 - 150.0).max(line_height);
    ((available_height / line_height).floor() as usize).max(1)
}

pub(crate) fn single_session_visible_body(
    app: &SingleSessionApp,
    size: PhysicalSize<u32>,
) -> Vec<String> {
    single_session_visible_styled_body(app, size)
        .into_iter()
        .map(|line| line.text)
        .collect()
}

pub(crate) fn single_session_visible_styled_body(
    app: &SingleSessionApp,
    size: PhysicalSize<u32>,
) -> Vec<SingleSessionStyledLine> {
    single_session_visible_styled_body_for_tick(app, size, 0)
}

pub(crate) fn single_session_visible_styled_body_for_tick(
    app: &SingleSessionApp,
    size: PhysicalSize<u32>,
    tick: u64,
) -> Vec<SingleSessionStyledLine> {
    single_session_body_viewport_for_tick(app, size, tick, 0.0).lines
}

#[derive(Clone, Debug)]
pub(crate) struct SingleSessionBodyViewport {
    pub(crate) lines: Vec<SingleSessionStyledLine>,
    pub(crate) top_offset_pixels: f32,
    pub(crate) start_line: usize,
    pub(crate) total_lines: usize,
}

pub(crate) fn single_session_body_viewport_for_tick(
    app: &SingleSessionApp,
    size: PhysicalSize<u32>,
    tick: u64,
    smooth_scroll_lines: f32,
) -> SingleSessionBodyViewport {
    let lines = single_session_rendered_body_lines_for_tick(app, size, tick);
    single_session_body_viewport_from_lines(app, size, smooth_scroll_lines, &lines)
}

pub(crate) fn single_session_body_viewport_from_lines(
    app: &SingleSessionApp,
    size: PhysicalSize<u32>,
    smooth_scroll_lines: f32,
    lines: &[SingleSessionStyledLine],
) -> SingleSessionBodyViewport {
    let total_lines = lines.len();
    let layout = single_session_layout_for_total_lines(app, size, total_lines);
    let line_height = layout.metrics.body_line_height;
    let available_height = layout.body.height.max(line_height);
    let visible_lines = ((available_height / line_height).floor() as usize).max(1);
    if lines.len() <= visible_lines {
        return SingleSessionBodyViewport {
            lines: lines.to_vec(),
            top_offset_pixels: 0.0,
            start_line: 0,
            total_lines,
        };
    }

    let max_scroll = lines.len().saturating_sub(visible_lines);
    let scroll = (app.body_scroll_lines + smooth_scroll_lines).clamp(0.0, max_scroll as f32);
    let bottom_line = lines.len() as f32 - scroll;
    let top_line = bottom_line - visible_lines as f32;
    let start = top_line.floor().max(0.0) as usize;
    let end = bottom_line.ceil().min(lines.len() as f32) as usize;
    let top_offset_pixels = (start as f32 - top_line) * line_height;
    SingleSessionBodyViewport {
        lines: lines[start..end.max(start)].to_vec(),
        top_offset_pixels,
        start_line: start,
        total_lines,
    }
}

pub(crate) fn single_session_rendered_body_lines_for_tick(
    app: &SingleSessionApp,
    size: PhysicalSize<u32>,
    tick: u64,
) -> Vec<SingleSessionStyledLine> {
    single_session_rendered_body_lines_from_raw(app, size, app.body_styled_lines_for_tick(tick))
}

pub(crate) fn single_session_rendered_body_lines_from_raw(
    app: &SingleSessionApp,
    size: PhysicalSize<u32>,
    raw_lines: Vec<SingleSessionStyledLine>,
) -> Vec<SingleSessionStyledLine> {
    let lines = single_session_wrapped_body_lines(raw_lines, size, app.text_scale());
    single_session_rendered_body_lines_from_wrapped(app, size, lines)
}

pub(crate) fn single_session_rendered_body_lines_from_raw_ref(
    app: &SingleSessionApp,
    size: PhysicalSize<u32>,
    raw_lines: &[SingleSessionStyledLine],
) -> Vec<SingleSessionStyledLine> {
    let lines = single_session_wrapped_body_lines_ref(raw_lines, size, app.text_scale());
    single_session_rendered_body_lines_from_wrapped(app, size, lines)
}

fn single_session_rendered_body_lines_from_wrapped(
    app: &SingleSessionApp,
    size: PhysicalSize<u32>,
    lines: Vec<SingleSessionStyledLine>,
) -> Vec<SingleSessionStyledLine> {
    if !(app.is_welcome_timeline_visible() && app.has_welcome_timeline_transcript()) {
        return lines;
    }

    // The welcome hero is visual chrome. These blank prelude rows make it
    // scroll like the first timeline block while keeping transcript text pure.
    let virtual_lines = welcome_timeline_virtual_body_lines(app, size);
    let mut rendered = Vec::with_capacity(virtual_lines + lines.len());
    rendered.extend((0..virtual_lines).map(|_| blank_render_line()));
    rendered.extend(lines);
    rendered
}

pub(crate) fn single_session_rendered_static_body_lines_for_streaming(
    app: &SingleSessionApp,
    size: PhysicalSize<u32>,
    _tick: u64,
) -> Option<Vec<SingleSessionStyledLine>> {
    let lines = single_session_wrapped_body_lines(
        app.body_styled_lines_without_streaming_response()?,
        size,
        app.text_scale(),
    );
    if !(app.is_welcome_timeline_visible() && app.has_welcome_timeline_transcript()) {
        return Some(lines);
    }

    let virtual_lines = welcome_timeline_virtual_body_lines(app, size);
    let mut rendered = Vec::with_capacity(virtual_lines + lines.len());
    rendered.extend((0..virtual_lines).map(|_| blank_render_line()));
    rendered.extend(lines);
    Some(rendered)
}

pub(crate) fn append_single_session_streaming_response_rendered_body_lines(
    app: &SingleSessionApp,
    size: PhysicalSize<u32>,
    rendered_lines: &mut Vec<SingleSessionStyledLine>,
) {
    if app.streaming_response.is_empty() {
        return;
    }
    if !app.messages.is_empty() {
        rendered_lines.push(blank_render_line());
    }
    rendered_lines.extend(single_session_wrapped_body_lines(
        app.streaming_response_styled_lines(),
        size,
        app.text_scale(),
    ));
}

pub(crate) fn single_session_streaming_response_rendered_body_line_count(
    app: &SingleSessionApp,
    size: PhysicalSize<u32>,
) -> usize {
    if app.streaming_response.is_empty() {
        return 0;
    }
    let separator = usize::from(!app.messages.is_empty());
    separator
        + single_session_wrapped_body_line_count(
            app.streaming_response_styled_lines(),
            size,
            app.text_scale(),
        )
}

fn blank_render_line() -> SingleSessionStyledLine {
    SingleSessionStyledLine::new(String::new(), SingleSessionLineStyle::Blank)
}

fn single_session_wrapped_body_lines(
    lines: Vec<SingleSessionStyledLine>,
    size: PhysicalSize<u32>,
    text_scale: f32,
) -> Vec<SingleSessionStyledLine> {
    // Glyphon also wraps, but explicit visual rows keep scroll metrics,
    // selection hit-testing, and the rendered text viewport in agreement.
    let max_columns = single_session_body_max_columns(size, text_scale);
    if should_parallel_wrap_body_lines(lines.len()) {
        return parallel_wrap_body_lines(&lines, max_columns);
    }

    let mut wrapped = Vec::with_capacity(lines.len());

    for line in lines {
        push_wrapped_body_line_owned(&mut wrapped, line, max_columns);
    }

    wrapped
}

fn single_session_wrapped_body_line_count(
    lines: Vec<SingleSessionStyledLine>,
    size: PhysicalSize<u32>,
    text_scale: f32,
) -> usize {
    let max_columns = single_session_body_max_columns(size, text_scale);
    lines
        .iter()
        .map(|line| wrapped_body_line_count(line, max_columns))
        .sum()
}

fn single_session_wrapped_body_lines_ref(
    lines: &[SingleSessionStyledLine],
    size: PhysicalSize<u32>,
    text_scale: f32,
) -> Vec<SingleSessionStyledLine> {
    // Glyphon also wraps, but explicit visual rows keep scroll metrics,
    // selection hit-testing, and the rendered text viewport in agreement.
    let max_columns = single_session_body_max_columns(size, text_scale);
    if should_parallel_wrap_body_lines(lines.len()) {
        return parallel_wrap_body_lines(lines, max_columns);
    }

    wrap_body_lines_slice(lines, max_columns)
}

fn should_parallel_wrap_body_lines(line_count: usize) -> bool {
    line_count >= 512
        && std::thread::available_parallelism()
            .map(|parallelism| parallelism.get() > 1)
            .unwrap_or(false)
}

fn parallel_wrap_body_lines(
    lines: &[SingleSessionStyledLine],
    max_columns: usize,
) -> Vec<SingleSessionStyledLine> {
    let available_parallelism = std::thread::available_parallelism()
        .map(|parallelism| parallelism.get())
        .unwrap_or(1);
    let worker_count = available_parallelism
        .min(lines.len().div_ceil(256).max(1))
        .max(1);
    if worker_count <= 1 {
        return wrap_body_lines_slice(lines, max_columns);
    }

    let chunk_size = lines.len().div_ceil(worker_count).max(1);
    std::thread::scope(|scope| {
        let mut handles = Vec::with_capacity(worker_count);
        for chunk in lines.chunks(chunk_size) {
            handles.push(scope.spawn(move || wrap_body_lines_slice(chunk, max_columns)));
        }
        let mut wrapped = Vec::with_capacity(lines.len());
        for handle in handles {
            wrapped.extend(
                handle
                    .join()
                    .expect("desktop body wrap worker panicked unexpectedly"),
            );
        }
        wrapped
    })
}

fn wrap_body_lines_slice(
    lines: &[SingleSessionStyledLine],
    max_columns: usize,
) -> Vec<SingleSessionStyledLine> {
    let mut wrapped = Vec::with_capacity(lines.len());
    for line in lines {
        push_wrapped_body_line_ref(&mut wrapped, line, max_columns);
    }
    wrapped
}

fn push_wrapped_body_line_owned(
    wrapped: &mut Vec<SingleSessionStyledLine>,
    line: SingleSessionStyledLine,
    max_columns: usize,
) {
    if line.text.is_empty() {
        wrapped.push(line);
        return;
    }
    if line.inline_spans.is_empty() && line.text.is_ascii() {
        if line.text.len() <= max_columns.max(1) {
            wrapped.push(line);
        } else {
            push_wrapped_ascii_body_line_parts(
                wrapped,
                &line.text,
                line.style,
                line.tool.as_ref(),
                max_columns,
            );
        }
        return;
    }
    if !text_exceeds_columns(&line.text, max_columns) {
        wrapped.push(line);
        return;
    }
    push_wrapped_body_line_parts(
        wrapped,
        &line.text,
        &line.inline_spans,
        line.style,
        line.tool.as_ref(),
        max_columns,
    );
}

fn push_wrapped_body_line_ref(
    wrapped: &mut Vec<SingleSessionStyledLine>,
    line: &SingleSessionStyledLine,
    max_columns: usize,
) {
    if line.text.is_empty() {
        wrapped.push(line.clone());
        return;
    }
    if line.inline_spans.is_empty() && line.text.is_ascii() {
        if line.text.len() <= max_columns.max(1) {
            wrapped.push(line.clone());
        } else {
            push_wrapped_ascii_body_line_parts(
                wrapped,
                &line.text,
                line.style,
                line.tool.as_ref(),
                max_columns,
            );
        }
        return;
    }
    if !text_exceeds_columns(&line.text, max_columns) {
        wrapped.push(line.clone());
        return;
    }
    push_wrapped_body_line_parts(
        wrapped,
        &line.text,
        &line.inline_spans,
        line.style,
        line.tool.as_ref(),
        max_columns,
    );
}

fn wrapped_body_line_count(line: &SingleSessionStyledLine, max_columns: usize) -> usize {
    if line.text.is_empty() {
        return 1;
    }
    if line.inline_spans.is_empty() && line.text.is_ascii() {
        return wrapped_ascii_body_line_count(&line.text, max_columns);
    }
    if !text_exceeds_columns(&line.text, max_columns) {
        return 1;
    }
    wrapped_body_line_text_count(&line.text, &line.inline_spans, max_columns)
}

fn wrapped_ascii_body_line_count(text: &str, max_columns: usize) -> usize {
    let max_columns = max_columns.max(1);
    let trimmed_end = text.trim_end().len();
    let mut remaining = &text[..trimmed_end];
    let mut count = 1usize;

    while remaining.len() > max_columns {
        let split = ascii_word_wrap_split_index(remaining, max_columns);
        remaining = remaining[split..].trim_start();
        count += 1;
    }

    count
}

fn push_wrapped_body_line_parts(
    wrapped: &mut Vec<SingleSessionStyledLine>,
    text: &str,
    inline_spans: &[SingleSessionInlineSpan],
    style: SingleSessionLineStyle,
    tool: Option<&SingleSessionToolLineMetadata>,
    max_columns: usize,
) {
    for (text, inline_spans) in wrap_body_line_text_with_spans(text, inline_spans, max_columns) {
        let mut line = SingleSessionStyledLine::with_inline_spans(text, style, inline_spans);
        line.tool = tool.cloned();
        wrapped.push(line);
    }
}

fn push_wrapped_ascii_body_line_parts(
    wrapped: &mut Vec<SingleSessionStyledLine>,
    text: &str,
    style: SingleSessionLineStyle,
    tool: Option<&SingleSessionToolLineMetadata>,
    max_columns: usize,
) {
    let max_columns = max_columns.max(1);
    let trimmed_end = text.trim_end().len();
    let mut remaining = &text[..trimmed_end];

    while remaining.len() > max_columns {
        let split = ascii_word_wrap_split_index(remaining, max_columns);
        let line = remaining[..split].trim_end();
        let mut wrapped_line = SingleSessionStyledLine::new(line.to_string(), style);
        wrapped_line.tool = tool.cloned();
        wrapped.push(wrapped_line);

        remaining = remaining[split..].trim_start();
    }

    let mut wrapped_line = SingleSessionStyledLine::new(remaining.to_string(), style);
    wrapped_line.tool = tool.cloned();
    wrapped.push(wrapped_line);
}

fn single_session_body_max_columns(size: PhysicalSize<u32>, text_scale: f32) -> usize {
    let content_width = single_session_content_width(size);
    (content_width / single_session_body_char_width_for_scale(text_scale))
        .floor()
        .max(20.0) as usize
}

fn wrap_body_line_text_with_spans(
    text: &str,
    inline_spans: &[SingleSessionInlineSpan],
    max_columns: usize,
) -> Vec<(String, Vec<SingleSessionInlineSpan>)> {
    let max_columns = max_columns.max(1);
    let trimmed_end =
        single_session_trimmed_line_end_preserving_inline_code_whitespace(text, inline_spans);
    let mut remaining = &text[..trimmed_end];
    let mut lines = Vec::new();
    let mut base_byte = 0usize;

    while text_exceeds_columns(remaining, max_columns) {
        let split = word_wrap_split_index(remaining, max_columns);
        let (line, rest) = remaining.split_at(split);
        let line = line.trim_end();
        let start = base_byte;
        let end = start + line.len();
        lines.push((
            line.to_string(),
            inline_spans_for_wrapped_range(inline_spans, start, end),
        ));

        let trimmed_rest = rest.trim_start();
        base_byte += split + rest.len().saturating_sub(trimmed_rest.len());
        remaining = trimmed_rest;
    }

    let start = base_byte;
    let end = start + remaining.len();
    lines.push((
        remaining.to_string(),
        inline_spans_for_wrapped_range(inline_spans, start, end),
    ));
    lines
}

fn wrapped_body_line_text_count(
    text: &str,
    inline_spans: &[SingleSessionInlineSpan],
    max_columns: usize,
) -> usize {
    let max_columns = max_columns.max(1);
    let trimmed_end =
        single_session_trimmed_line_end_preserving_inline_code_whitespace(text, inline_spans);
    let mut remaining = &text[..trimmed_end];
    let mut count = 1usize;

    while text_exceeds_columns(remaining, max_columns) {
        let split = word_wrap_split_index(remaining, max_columns);
        let (_, rest) = remaining.split_at(split);
        remaining = rest.trim_start();
        count += 1;
    }

    count
}

fn inline_spans_for_wrapped_range(
    inline_spans: &[SingleSessionInlineSpan],
    start: usize,
    end: usize,
) -> Vec<SingleSessionInlineSpan> {
    if inline_spans.is_empty() {
        return Vec::new();
    }

    inline_spans
        .iter()
        .filter_map(|span| {
            let span_start = span.start.max(start);
            let span_end = span.end.min(end);
            (span_start < span_end).then(|| SingleSessionInlineSpan {
                start: span_start - start,
                end: span_end - start,
                kind: span.kind,
            })
        })
        .collect()
}

fn text_exceeds_columns(text: &str, max_columns: usize) -> bool {
    if text.is_ascii() {
        return text.len() > max_columns.max(1);
    }

    text.chars().nth(max_columns.max(1)).is_some()
}

fn word_wrap_split_index(text: &str, max_columns: usize) -> usize {
    let max_columns = max_columns.max(1);
    if text.is_ascii() {
        return ascii_word_wrap_split_index(text, max_columns);
    }

    let hard_split = byte_index_at_char_limit(text, max_columns);
    text[..hard_split]
        .char_indices()
        .rev()
        .find_map(|(index, ch)| ch.is_whitespace().then_some(index))
        .filter(|index| *index > 0)
        .unwrap_or(hard_split)
}

fn ascii_word_wrap_split_index(text: &str, max_columns: usize) -> usize {
    let hard_split = text.len().min(max_columns.max(1));
    text.as_bytes()[..hard_split]
        .iter()
        .rposition(u8::is_ascii_whitespace)
        .filter(|index| *index > 0)
        .unwrap_or(hard_split)
}

fn byte_index_at_char_limit(text: &str, max_columns: usize) -> usize {
    text.char_indices()
        .map(|(index, _)| index)
        .chain(std::iter::once(text.len()))
        .nth(max_columns)
        .unwrap_or(text.len())
}

pub(crate) fn single_session_body_line_at_y(size: PhysicalSize<u32>, y: f32) -> Option<usize> {
    let typography = single_session_typography();
    let line_height = typography.body_size * typography.body_line_height;
    if y < PANEL_BODY_TOP_PADDING || y >= single_session_body_bottom(size) {
        return None;
    }
    Some(((y - PANEL_BODY_TOP_PADDING) / line_height).floor() as usize)
}

pub(crate) fn single_session_body_point_at_position(
    size: PhysicalSize<u32>,
    x: f32,
    y: f32,
    lines: &[String],
) -> Option<SelectionPoint> {
    let line = single_session_body_line_at_y(size, y)?;
    let text = lines.get(line)?;
    Some(SelectionPoint {
        line,
        column: single_session_body_column_at_x(x, text),
    })
}

pub(crate) fn single_session_body_column_at_x(x: f32, line: &str) -> usize {
    let char_count = line.chars().count();
    if x <= PANEL_TITLE_LEFT_PADDING {
        return 0;
    }
    let raw = ((x - PANEL_TITLE_LEFT_PADDING) / single_session_body_char_width()).round();
    raw.max(0.0).min(char_count as f32) as usize
}

pub(crate) fn single_session_body_char_width() -> f32 {
    single_session_body_char_width_for_scale(1.0)
}

pub(crate) fn single_session_body_char_width_for_scale(text_scale: f32) -> f32 {
    let typography = single_session_typography_for_scale(text_scale);
    typography.body_size * 0.58
}

fn single_session_body_top_for_app(_app: &SingleSessionApp, _size: PhysicalSize<u32>) -> f32 {
    PANEL_BODY_TOP_PADDING
}

fn single_session_body_bottom_base_for_app(app: &SingleSessionApp, size: PhysicalSize<u32>) -> f32 {
    if app.is_welcome_timeline_visible() {
        // Treat the welcome hero as the first visual item in the chat timeline.
        // Anything inline, such as the /model picker, must reserve space between
        // that timeline and the composer instead of floating over the hero.
        return (single_session_draft_top_for_app(app, size) - welcome_timeline_body_draft_gap())
            .max(single_session_body_top_for_app(app, size));
    }

    single_session_body_bottom(size)
}

fn single_session_body_bottom_base_for_total_lines(
    app: &SingleSessionApp,
    size: PhysicalSize<u32>,
    total_lines: usize,
) -> f32 {
    if app.is_welcome_timeline_visible() {
        return (single_session_draft_top_for_total_lines(app, size, total_lines)
            - welcome_timeline_body_draft_gap())
        .max(single_session_body_top_for_app(app, size));
    }

    single_session_body_bottom(size)
}

pub(crate) fn single_session_body_bottom_for_total_lines(
    app: &SingleSessionApp,
    size: PhysicalSize<u32>,
    total_lines: usize,
) -> f32 {
    single_session_layout_for_total_lines(app, size, total_lines).body_bottom()
}

fn streaming_activity_reserved_height(app: &SingleSessionApp) -> f32 {
    if !app.has_activity_indicator() {
        return 0.0;
    }

    let typography = single_session_typography_for_scale(app.text_scale());
    typography.body_size * typography.body_line_height
}

fn inline_widget_visible_text_height(app: &SingleSessionApp) -> f32 {
    let lines = app.render_inline_widget_visible_line_count();
    if lines == 0 {
        return 0.0;
    }
    let typography = single_session_typography_for_scale(app.text_scale());
    lines as f32 * inline_widget_line_height(app.render_inline_widget_kind(), &typography)
}

fn inline_widget_reserved_height(app: &SingleSessionApp) -> f32 {
    if app.render_inline_widget_line_count() == 0 {
        0.0
    } else {
        let padding_y = inline_widget_card_padding_y(app.render_inline_widget_kind());
        (inline_widget_visible_text_height(app) + padding_y * 2.0 + INLINE_WIDGET_BODY_GAP)
            * app.render_inline_widget_reveal_progress().clamp(0.0, 1.0)
    }
}

fn inline_widget_target_top(
    size: PhysicalSize<u32>,
    ui_scale: f32,
    body_bottom: f32,
    welcome_chrome_visible: bool,
    welcome_chrome_offset_pixels: f32,
) -> f32 {
    if welcome_chrome_visible {
        fresh_welcome_visual_bottom_for_scale(size, ui_scale)
            + welcome_chrome_offset_pixels
            + fresh_welcome_inline_widget_gap_for_scale(ui_scale)
    } else {
        body_bottom + INLINE_WIDGET_BODY_GAP
    }
}

pub(crate) fn single_session_body_bottom(size: PhysicalSize<u32>) -> f32 {
    single_session_draft_top(size) - 12.0
}

fn clip_rect_to_vertical_bounds(rect: Rect, top: f32, bottom: f32) -> Option<Rect> {
    let clipped_y = rect.y.max(top);
    let clipped_bottom = (rect.y + rect.height).min(bottom);
    (clipped_bottom > clipped_y).then_some(Rect {
        y: clipped_y,
        height: clipped_bottom - clipped_y,
        ..rect
    })
}

fn text_bounds_bottom(value: f32) -> i32 {
    value.ceil().clamp(0.0, i32::MAX as f32) as i32
}

fn single_session_text_buffer(
    font_system: &mut FontSystem,
    text: &str,
    font_size: f32,
    line_height: f32,
    width: f32,
    height: f32,
) -> Buffer {
    single_session_text_buffer_with_family(
        font_system,
        text,
        SINGLE_SESSION_FONT_FAMILY,
        font_size,
        line_height,
        width,
        height,
    )
}

fn single_session_text_buffer_with_family(
    font_system: &mut FontSystem,
    text: &str,
    family: &'static str,
    font_size: f32,
    line_height: f32,
    width: f32,
    height: f32,
) -> Buffer {
    let mut buffer = Buffer::new(font_system, Metrics::new(font_size, line_height));
    buffer.set_size(font_system, width, height);
    buffer.set_wrap(font_system, Wrap::Word);
    buffer.set_text(
        font_system,
        text,
        Attrs::new().family(Family::Name(family)),
        desktop_text_shaping(text),
    );
    buffer.shape_until_scroll(font_system);
    buffer
}

fn single_session_styled_text_buffer(
    font_system: &mut FontSystem,
    lines: &[SingleSessionStyledLine],
    font_size: f32,
    line_height: f32,
    width: f32,
    height: f32,
    wrap: Wrap,
) -> Buffer {
    single_session_styled_text_buffer_with_opacity(
        font_system,
        lines,
        font_size,
        line_height,
        width,
        height,
        wrap,
        1.0,
    )
}

#[allow(clippy::too_many_arguments)]
fn single_session_styled_text_buffer_with_opacity(
    font_system: &mut FontSystem,
    lines: &[SingleSessionStyledLine],
    font_size: f32,
    line_height: f32,
    width: f32,
    height: f32,
    wrap: Wrap,
    opacity: f32,
) -> Buffer {
    let mut buffer = Buffer::new(font_system, Metrics::new(font_size, line_height));
    buffer.set_size(font_system, width, height);
    buffer.set_wrap(font_system, wrap);
    let segments = single_session_styled_text_segments_with_opacity(lines, opacity);
    // Inline span geometry uses glyphon cursors with byte offsets. Basic shaping
    // reports glyph clusters relative to each styled run, so spans after a
    // multi-byte marker or a style boundary can shift their pills into prose.
    let shaping = if lines.iter().any(|line| !line.inline_spans.is_empty())
        || segments
            .iter()
            .any(|(text, _)| text_needs_advanced_shaping(text))
    {
        Shaping::Advanced
    } else {
        Shaping::Basic
    };
    buffer.set_rich_text(font_system, segments.iter().copied(), shaping);
    buffer.shape_until_scroll(font_system);
    buffer
}

fn desktop_text_shaping(text: &str) -> Shaping {
    if text_needs_advanced_shaping(text) {
        Shaping::Advanced
    } else {
        Shaping::Basic
    }
}

fn text_needs_advanced_shaping(text: &str) -> bool {
    text.chars().any(char_needs_advanced_shaping)
}

fn char_needs_advanced_shaping(ch: char) -> bool {
    let code = ch as u32;
    matches!(
        code,
        // Combining marks and joiners.
        0x0300..=0x036F
            | 0x1AB0..=0x1AFF
            | 0x1DC0..=0x1DFF
            | 0x20D0..=0x20FF
            | 0xFE00..=0xFE0F
            | 0xFE20..=0xFE2F
            | 0x200C..=0x200D
            // Scripts where shaping, bidi, or syllable reordering matter.
            | 0x0590..=0x08FF
            | 0x0900..=0x0DFF
            | 0x1780..=0x18AF
            // Emoji and symbol sequences often depend on variation selectors / ZWJ.
            | 0x1F000..=0x1FAFF
    )
}

#[cfg_attr(not(test), allow(dead_code))]
pub(crate) fn single_session_styled_text_segments(
    lines: &[SingleSessionStyledLine],
) -> Vec<(&str, Attrs<'static>)> {
    single_session_styled_text_segments_with_opacity(lines, 1.0)
}

#[cfg_attr(not(test), allow(dead_code))]
pub(crate) fn single_session_styled_text_segments_with_opacity(
    lines: &[SingleSessionStyledLine],
    opacity: f32,
) -> Vec<(&str, Attrs<'static>)> {
    let mut segments = Vec::new();
    let total_user_turns = lines
        .iter()
        .filter(|line| line.style == SingleSessionLineStyle::User)
        .count();
    for (index, line) in lines.iter().enumerate() {
        if !line.text.is_empty() {
            if line.style == SingleSessionLineStyle::User {
                push_user_prompt_segments(&mut segments, &line.text, total_user_turns);
            } else if line.style == SingleSessionLineStyle::Tool {
                push_tool_line_segments(&mut segments, &line.text);
            } else if push_assistant_markdown_inline_segments(&mut segments, line) {
                // Markdown prose can mix display fonts with inline code/math, emphasis,
                // strong text, strike-through spans, and task/list markers. Segmenting
                // here keeps rendered text clean while giving each semantic run a
                // distinct font, weight, style, or color.
            } else {
                segments.push((
                    line.text.as_str(),
                    single_session_style_attrs_for_text(line.style, &line.text),
                ));
            }
        }
        if index + 1 < lines.len() {
            segments.push((
                "\n",
                single_session_style_attrs(SingleSessionLineStyle::Blank),
            ));
        }
    }
    if segments.is_empty() {
        segments.push((
            "",
            single_session_style_attrs(SingleSessionLineStyle::Blank),
        ));
    }
    let opacity = opacity.clamp(0.0, 1.0);
    if opacity < 0.999 {
        for (_, attrs) in &mut segments {
            *attrs = text_attrs_with_opacity(*attrs, opacity);
        }
    }
    segments
}

fn text_attrs_with_opacity(mut attrs: Attrs<'static>, opacity: f32) -> Attrs<'static> {
    let Some(color) = attrs.color_opt else {
        return attrs;
    };
    let (r, g, b, a) = color.as_rgba_tuple();
    attrs.color_opt = Some(TextColor::rgba(
        r,
        g,
        b,
        (a as f32 * opacity).round().clamp(0.0, 255.0) as u8,
    ));
    attrs
}

fn push_assistant_markdown_inline_segments<'a>(
    segments: &mut Vec<(&'a str, Attrs<'static>)>,
    line: &'a SingleSessionStyledLine,
) -> bool {
    if !single_session_line_style_supports_markdown_inline_segments(line.style) {
        return false;
    }

    if let Some(marker) = assistant_markdown_list_marker_span(&line.text) {
        if marker.prefix_start > 0 {
            push_assistant_markdown_inline_range(segments, line, 0, marker.prefix_start, false);
        }
        if marker.marker_start > marker.prefix_start {
            push_assistant_markdown_inline_range(
                segments,
                line,
                marker.prefix_start,
                marker.marker_start,
                false,
            );
        }
        segments.push((
            &line.text[marker.marker_start..marker.marker_end],
            single_session_inline_color_attrs_for_text(
                line.style,
                &line.text[marker.marker_start..marker.marker_end],
                marker.color,
            ),
        ));
        push_assistant_markdown_inline_range(
            segments,
            line,
            marker.marker_end,
            line.text.len(),
            false,
        );
        return true;
    }

    push_assistant_markdown_inline_range(segments, line, 0, line.text.len(), true)
}

fn single_session_line_style_supports_markdown_inline_segments(
    style: SingleSessionLineStyle,
) -> bool {
    matches!(
        style,
        SingleSessionLineStyle::Assistant
            | SingleSessionLineStyle::AssistantHeading
            | SingleSessionLineStyle::AssistantQuote
            | SingleSessionLineStyle::AssistantLink
            | SingleSessionLineStyle::AssistantMedia
    )
}

fn push_assistant_markdown_inline_range<'a>(
    segments: &mut Vec<(&'a str, Attrs<'static>)>,
    line: &'a SingleSessionStyledLine,
    start: usize,
    end: usize,
    require_semantic_span: bool,
) -> bool {
    if start >= end {
        return false;
    }

    let inline_spans = clipped_inline_spans_for_range(&line.inline_spans, start, end);
    if inline_spans.is_empty() && require_semantic_span {
        return false;
    }

    if inline_spans.is_empty() {
        let text = &line.text[start..end];
        segments.push((text, single_session_style_attrs_for_text(line.style, text)));
        return true;
    }

    let force_main_font = inline_spans.iter().any(|span| {
        matches!(
            span.kind,
            SingleSessionInlineSpanKind::Code | SingleSessionInlineSpanKind::Math
        )
    });

    let mut boundaries = Vec::with_capacity(inline_spans.len().saturating_mul(2) + 2);
    boundaries.push(start);
    boundaries.push(end);
    for span in &inline_spans {
        boundaries.push(span.start);
        boundaries.push(span.end);
    }
    boundaries.sort_unstable();
    boundaries.dedup();

    for window in boundaries.windows(2) {
        let segment_start = window[0];
        let segment_end = window[1];
        if segment_start >= segment_end {
            continue;
        }
        let text = &line.text[segment_start..segment_end];
        let active_kinds =
            active_inline_span_kinds_for_range(&inline_spans, segment_start, segment_end);
        segments.push((
            text,
            assistant_inline_markdown_run_attrs(line.style, text, &active_kinds, force_main_font),
        ));
    }
    true
}

fn clipped_inline_spans_for_range(
    inline_spans: &[SingleSessionInlineSpan],
    start: usize,
    end: usize,
) -> Vec<SingleSessionInlineSpan> {
    inline_spans
        .iter()
        .filter_map(|span| {
            let span_start = span.start.max(start);
            let span_end = span.end.min(end);
            (span_start < span_end).then_some(SingleSessionInlineSpan {
                start: span_start,
                end: span_end,
                kind: span.kind,
            })
        })
        .collect()
}

fn active_inline_span_kinds_for_range(
    inline_spans: &[SingleSessionInlineSpan],
    start: usize,
    end: usize,
) -> Vec<SingleSessionInlineSpanKind> {
    inline_spans
        .iter()
        .filter_map(|span| (span.start <= start && end <= span.end).then_some(span.kind))
        .collect()
}

fn assistant_inline_markdown_run_attrs(
    style: SingleSessionLineStyle,
    text: &str,
    kinds: &[SingleSessionInlineSpanKind],
    force_main_font: bool,
) -> Attrs<'static> {
    if kinds.iter().any(|kind| {
        matches!(
            kind,
            SingleSessionInlineSpanKind::Code | SingleSessionInlineSpanKind::Math
        )
    }) {
        return single_session_style_attrs(SingleSessionLineStyle::Code);
    }

    let mut attrs = if force_main_font {
        single_session_style_attrs_for_family(style, SINGLE_SESSION_FONT_FAMILY)
    } else {
        single_session_style_attrs_for_text(style, text)
    };
    if kinds.contains(&SingleSessionInlineSpanKind::Strike) {
        attrs = attrs.color(text_color(MARKDOWN_STRIKE_TEXT_COLOR));
    }
    if kinds.contains(&SingleSessionInlineSpanKind::Strong) {
        attrs = attrs.weight(glyphon::Weight::BOLD);
    }
    if kinds.contains(&SingleSessionInlineSpanKind::Emphasis) {
        attrs = attrs.style(glyphon::Style::Italic);
    }
    attrs
}

#[cfg_attr(not(test), allow(dead_code))]
pub(crate) fn rich_line_text_segments(line: &RichLine) -> Vec<(&str, Attrs<'static>)> {
    let base_style = rich_line_style_to_single_session_style(line.style);
    let valid_spans = line
        .spans
        .iter()
        .filter(|span| {
            span.start < span.end
                && span.end <= line.text.len()
                && line.text.is_char_boundary(span.start)
                && line.text.is_char_boundary(span.end)
        })
        .collect::<Vec<_>>();
    if valid_spans.is_empty() {
        return vec![(
            &line.text,
            single_session_style_attrs_for_text(base_style, &line.text),
        )];
    }

    let mut boundaries = Vec::with_capacity(valid_spans.len().saturating_mul(2) + 2);
    boundaries.push(0);
    boundaries.push(line.text.len());
    for span in &valid_spans {
        boundaries.push(span.start);
        boundaries.push(span.end);
    }
    boundaries.sort_unstable();
    boundaries.dedup();

    let mut segments = Vec::new();
    for window in boundaries.windows(2) {
        let start = window[0];
        let end = window[1];
        if start >= end {
            continue;
        }
        let text = &line.text[start..end];
        let active = valid_spans
            .iter()
            .filter_map(|span| (span.start <= start && end <= span.end).then_some(&span.style))
            .collect::<Vec<_>>();
        segments.push((text, rich_span_attrs(base_style, text, &active)));
    }
    segments
}

#[cfg_attr(not(test), allow(dead_code))]
pub(crate) fn rich_line_style_to_single_session_style(
    style: RichLineStyle,
) -> SingleSessionLineStyle {
    match style {
        RichLineStyle::User => SingleSessionLineStyle::User,
        RichLineStyle::Assistant => SingleSessionLineStyle::Assistant,
        RichLineStyle::AssistantHeading => SingleSessionLineStyle::AssistantHeading,
        RichLineStyle::AssistantQuote => SingleSessionLineStyle::AssistantQuote,
        RichLineStyle::AssistantTable => SingleSessionLineStyle::AssistantTable,
        RichLineStyle::CodeHeader => SingleSessionLineStyle::CodeHeader,
        RichLineStyle::Code => SingleSessionLineStyle::Code,
        RichLineStyle::ToolHeader | RichLineStyle::ToolOutput | RichLineStyle::ToolMetadata => {
            SingleSessionLineStyle::Tool
        }
        RichLineStyle::System => SingleSessionLineStyle::Status,
        RichLineStyle::Meta => SingleSessionLineStyle::Meta,
        RichLineStyle::MediaPlaceholder => SingleSessionLineStyle::AssistantMedia,
    }
}

fn rich_span_attrs(
    base_style: SingleSessionLineStyle,
    text: &str,
    styles: &[&RichSpanStyle],
) -> Attrs<'static> {
    let mut attrs = single_session_style_attrs_for_text(base_style, text);
    for style in styles {
        match style {
            RichSpanStyle::InlineCode => {
                attrs = single_session_style_attrs(SingleSessionLineStyle::Code);
            }
            RichSpanStyle::Link { .. } => {
                attrs = attrs.color(single_session_line_color(
                    SingleSessionLineStyle::AssistantLink,
                ));
            }
            RichSpanStyle::Emphasis => {
                attrs = attrs.style(glyphon::Style::Italic);
            }
            RichSpanStyle::Strong => {
                attrs = attrs.weight(glyphon::Weight::BOLD);
            }
            RichSpanStyle::Strike => {
                attrs = attrs.color(text_color(MARKDOWN_STRIKE_TEXT_COLOR));
            }
            RichSpanStyle::Syntax(kind) => {
                attrs = attrs.color(text_color(rich_syntax_token_color(*kind)));
            }
            RichSpanStyle::Ansi(style) => {
                if let Some(color) = rich_ansi_foreground(*style) {
                    attrs = attrs.color(text_color(color));
                }
                if style.bold {
                    attrs = attrs.weight(glyphon::Weight::BOLD);
                }
                if style.italic {
                    attrs = attrs.style(glyphon::Style::Italic);
                }
            }
            RichSpanStyle::SearchMatch => {
                attrs = attrs
                    .color(text_color(STATUS_TEXT_ACCENT_COLOR))
                    .weight(glyphon::Weight::BOLD);
            }
        }
    }
    attrs
}

fn rich_syntax_token_color(kind: SyntaxTokenKind) -> [f32; 4] {
    match kind {
        SyntaxTokenKind::Keyword => [0.350, 0.145, 0.640, 1.0],
        SyntaxTokenKind::String => [0.020, 0.360, 0.190, 1.0],
        SyntaxTokenKind::Number => [0.490, 0.250, 0.035, 1.0],
        SyntaxTokenKind::Comment => [0.320, 0.350, 0.420, 0.95],
        SyntaxTokenKind::Function => [0.000, 0.255, 0.430, 1.0],
        SyntaxTokenKind::Type => [0.225, 0.215, 0.620, 1.0],
        SyntaxTokenKind::Punctuation => [0.270, 0.290, 0.340, 0.98],
        SyntaxTokenKind::Plain => CODE_TEXT_COLOR,
    }
}

fn rich_ansi_foreground(style: AnsiStyle) -> Option<[f32; 4]> {
    let color = if style.inverse {
        style.background.or(style.foreground)
    } else {
        style.foreground
    }?;
    Some(match color {
        AnsiColor::Black => [0.040, 0.045, 0.055, 1.0],
        AnsiColor::Red => [0.560, 0.070, 0.095, 1.0],
        AnsiColor::Green => [0.035, 0.360, 0.220, 1.0],
        AnsiColor::Yellow => [0.520, 0.360, 0.055, 1.0],
        AnsiColor::Blue => [0.045, 0.265, 0.640, 1.0],
        AnsiColor::Magenta => [0.410, 0.145, 0.580, 1.0],
        AnsiColor::Cyan => [0.000, 0.330, 0.430, 1.0],
        AnsiColor::White => [0.700, 0.720, 0.770, 1.0],
        AnsiColor::BrightBlack => [0.320, 0.345, 0.405, 1.0],
        AnsiColor::BrightRed => [0.780, 0.110, 0.145, 1.0],
        AnsiColor::BrightGreen => [0.025, 0.500, 0.275, 1.0],
        AnsiColor::BrightYellow => [0.700, 0.500, 0.080, 1.0],
        AnsiColor::BrightBlue => [0.090, 0.360, 0.850, 1.0],
        AnsiColor::BrightMagenta => [0.560, 0.190, 0.760, 1.0],
        AnsiColor::BrightCyan => [0.000, 0.460, 0.580, 1.0],
        AnsiColor::BrightWhite => [0.900, 0.915, 0.945, 1.0],
    })
}

#[derive(Clone, Copy, Debug, PartialEq)]
struct AssistantMarkdownListMarkerSpan {
    prefix_start: usize,
    marker_start: usize,
    marker_end: usize,
    color: [f32; 4],
}

fn assistant_markdown_list_marker_span(text: &str) -> Option<AssistantMarkdownListMarkerSpan> {
    let mut index = 0;
    while index < text.len() {
        let rest = &text[index..];
        if rest.starts_with("│ ") {
            index += "│ ".len();
        } else if rest.starts_with("  ") {
            index += "  ".len();
        } else {
            break;
        }
    }

    let rest = &text[index..];
    let (marker_len, color) = if rest.starts_with("✓ ") {
        ("✓ ".len(), MARKDOWN_TASK_DONE_COLOR)
    } else if rest.starts_with("☐ ") {
        ("☐ ".len(), MARKDOWN_TASK_OPEN_COLOR)
    } else if rest.starts_with("• ") || rest.starts_with("◦ ") || rest.starts_with("▪ ") {
        (
            rest.chars().take(2).map(char::len_utf8).sum(),
            MARKDOWN_LIST_MARKER_COLOR,
        )
    } else if let Some(marker_len) = ordered_list_marker_len(rest) {
        (marker_len, MARKDOWN_LIST_MARKER_COLOR)
    } else {
        return None;
    };

    Some(AssistantMarkdownListMarkerSpan {
        prefix_start: 0,
        marker_start: index,
        marker_end: index + marker_len,
        color,
    })
}

fn ordered_list_marker_len(text: &str) -> Option<usize> {
    let mut digit_bytes = 0;
    for ch in text.chars() {
        if ch.is_ascii_digit() {
            digit_bytes += ch.len_utf8();
        } else {
            break;
        }
    }
    if digit_bytes == 0 || !text[digit_bytes..].starts_with(". ") {
        return None;
    }
    Some(digit_bytes + ". ".len())
}

fn single_session_inline_color_attrs_for_text(
    style: SingleSessionLineStyle,
    text: &str,
    color: [f32; 4],
) -> Attrs<'static> {
    let family = single_session_font_family_for_text(style, text);
    Attrs::new()
        .family(Family::Name(family))
        .color(text_color(color))
}

fn push_user_prompt_segments<'a>(
    segments: &mut Vec<(&'a str, Attrs<'static>)>,
    line: &'a str,
    total_user_turns: usize,
) {
    let Some((number, text)) = line.split_once("  ") else {
        segments.push((
            line,
            single_session_style_attrs(SingleSessionLineStyle::User),
        ));
        return;
    };
    let Ok(turn) = number.parse::<usize>() else {
        segments.push((
            line,
            single_session_style_attrs(SingleSessionLineStyle::User),
        ));
        return;
    };

    segments.push((
        number,
        single_session_color_attrs(user_prompt_number_color_for_distance(
            total_user_turns.saturating_add(1).saturating_sub(turn),
        )),
    ));
    segments.push((
        "› ",
        single_session_color_attrs(text_color(USER_PROMPT_ACCENT_COLOR)),
    ));
    segments.push((
        text,
        single_session_style_attrs(SingleSessionLineStyle::User),
    ));
}

fn push_tool_line_segments<'a>(segments: &mut Vec<(&'a str, Attrs<'static>)>, line: &'a str) {
    let trimmed = line.trim_start_matches(' ');
    let indent_len = line.len().saturating_sub(trimmed.len());
    if indent_len > 0 {
        segments.push((
            &line[..indent_len],
            single_session_color_attrs(text_color(TOOL_MUTED_TEXT_COLOR)),
        ));
    }

    if trimmed.is_empty() {
        return;
    }

    if push_tool_widget_segments(segments, trimmed) {
        return;
    }

    let Some((icon, icon_text, mut rest)) = split_tool_line_icon(trimmed) else {
        segments.push((
            trimmed,
            single_session_color_attrs(text_color(TOOL_DETAIL_TEXT_COLOR)),
        ));
        return;
    };

    segments.push((
        icon_text,
        single_session_color_attrs(text_color(tool_icon_text_color(icon))),
    ));

    let rest_indent_len = rest
        .char_indices()
        .find(|(_, ch)| *ch != ' ')
        .map(|(index, _)| index)
        .unwrap_or(rest.len());
    if rest_indent_len > 0 {
        segments.push((
            &rest[..rest_indent_len],
            single_session_color_attrs(text_color(TOOL_MUTED_TEXT_COLOR)),
        ));
        rest = &rest[rest_indent_len..];
    }

    push_tool_header_segments(segments, rest);
}

fn push_tool_widget_segments<'a>(
    segments: &mut Vec<(&'a str, Attrs<'static>)>,
    text: &'a str,
) -> bool {
    if text.starts_with('╭') || text.starts_with('╰') {
        segments.push((
            text,
            single_session_color_attrs(text_color(TOOL_MUTED_TEXT_COLOR)),
        ));
        return true;
    }

    if text.starts_with('│') && text.ends_with('│') && text.len() >= '│'.len_utf8() * 2 {
        let border_len = '│'.len_utf8();
        let content_start = border_len;
        let content_end = text.len().saturating_sub(border_len);
        let content = &text[content_start..content_end];
        let visible_content_end = content.trim_end_matches(' ').len();

        segments.push((
            &text[..content_start],
            single_session_color_attrs(text_color(TOOL_MUTED_TEXT_COLOR)),
        ));
        if visible_content_end > 0 {
            segments.push((
                &content[..visible_content_end],
                single_session_color_attrs(text_color(TOOL_DETAIL_TEXT_COLOR)),
            ));
        }
        if visible_content_end < content.len() {
            segments.push((
                &content[visible_content_end..],
                single_session_color_attrs(text_color(TOOL_MUTED_TEXT_COLOR)),
            ));
        }
        segments.push((
            &text[content_end..],
            single_session_color_attrs(text_color(TOOL_MUTED_TEXT_COLOR)),
        ));
        return true;
    }

    false
}

fn split_tool_line_icon(text: &str) -> Option<(char, &str, &str)> {
    let mut chars = text.char_indices();
    let (_, icon) = chars.next()?;
    if !matches!(icon, '✓' | '✕' | '●' | '○' | '▸' | '•') {
        return None;
    }
    let icon_end = chars.next().map(|(index, _)| index).unwrap_or(text.len());
    Some((icon, &text[..icon_end], &text[icon_end..]))
}

fn push_tool_header_segments<'a>(segments: &mut Vec<(&'a str, Attrs<'static>)>, text: &'a str) {
    const TOOL_SEPARATOR: &str = " · ";

    if text.is_empty() {
        return;
    }

    let mut remaining = text;
    let mut part_index = 0usize;
    while let Some(separator_index) = remaining.find(TOOL_SEPARATOR) {
        let part = &remaining[..separator_index];
        push_tool_header_part_segment(segments, part, part_index);
        let separator_end = separator_index + TOOL_SEPARATOR.len();
        segments.push((
            &remaining[separator_index..separator_end],
            single_session_color_attrs(text_color(TOOL_MUTED_TEXT_COLOR)),
        ));
        remaining = &remaining[separator_end..];
        part_index += 1;
    }

    push_tool_header_part_segment(segments, remaining, part_index);
}

fn push_tool_header_part_segment<'a>(
    segments: &mut Vec<(&'a str, Attrs<'static>)>,
    part: &'a str,
    part_index: usize,
) {
    if part.is_empty() {
        return;
    }
    let color = match part_index {
        0 => TOOL_TEXT_COLOR,
        1 => tool_state_text_color(part).unwrap_or(TOOL_MUTED_TEXT_COLOR),
        _ => TOOL_DETAIL_TEXT_COLOR,
    };
    segments.push((part, single_session_color_attrs(text_color(color))));
}

fn tool_icon_text_color(icon: char) -> [f32; 4] {
    match icon {
        '✓' => TOOL_SUCCESS_TEXT_COLOR,
        '✕' => TOOL_FAILED_TEXT_COLOR,
        '●' => TOOL_RUNNING_TEXT_COLOR,
        '○' => TOOL_PENDING_TEXT_COLOR,
        '▸' | '•' => TOOL_TEXT_COLOR,
        _ => TOOL_DETAIL_TEXT_COLOR,
    }
}

fn tool_state_text_color(state: &str) -> Option<[f32; 4]> {
    match state.trim().to_ascii_lowercase().as_str() {
        "done" | "success" | "succeeded" | "passed" => Some(TOOL_SUCCESS_TEXT_COLOR),
        "failed" | "failure" | "error" | "errored" => Some(TOOL_FAILED_TEXT_COLOR),
        "running" | "executing" | "active" => Some(TOOL_RUNNING_TEXT_COLOR),
        "preparing" | "pending" | "queued" | "waiting" => Some(TOOL_PENDING_TEXT_COLOR),
        _ => None,
    }
}

fn single_session_style_attrs(style: SingleSessionLineStyle) -> Attrs<'static> {
    single_session_style_attrs_for_family(style, single_session_font_family_for_style(style))
}

fn single_session_style_attrs_for_text(
    style: SingleSessionLineStyle,
    text: &str,
) -> Attrs<'static> {
    let family = single_session_font_family_for_text(style, text);
    single_session_style_attrs_for_family(style, family)
}

fn single_session_font_family_for_text(style: SingleSessionLineStyle, text: &str) -> &'static str {
    if matches!(
        style,
        SingleSessionLineStyle::User | SingleSessionLineStyle::UserContinuation
    ) {
        return single_session_user_font_family();
    }

    if assistant_text_should_use_handwriting_font(style, text) {
        return single_session_assistant_font_family();
    }

    SINGLE_SESSION_FONT_FAMILY
}

fn single_session_font_family_for_style(style: SingleSessionLineStyle) -> &'static str {
    if matches!(
        style,
        SingleSessionLineStyle::User | SingleSessionLineStyle::UserContinuation
    ) {
        single_session_user_font_family()
    } else if assistant_style_can_use_handwriting_font(style) {
        single_session_assistant_font_family()
    } else {
        SINGLE_SESSION_FONT_FAMILY
    }
}

fn single_session_style_attrs_for_family(
    style: SingleSessionLineStyle,
    family: &'static str,
) -> Attrs<'static> {
    Attrs::new()
        .family(Family::Name(family))
        .color(single_session_line_color(style))
}

fn text_contains_symbol_glyphs(text: &str) -> bool {
    !text.is_ascii()
}

fn assistant_style_can_use_handwriting_font(style: SingleSessionLineStyle) -> bool {
    matches!(
        style,
        SingleSessionLineStyle::Assistant
            | SingleSessionLineStyle::AssistantHeading
            | SingleSessionLineStyle::AssistantQuote
    )
}

fn assistant_text_should_use_handwriting_font(style: SingleSessionLineStyle, text: &str) -> bool {
    assistant_style_can_use_handwriting_font(style)
        && !text.trim().is_empty()
        && !text_contains_symbol_glyphs(text)
        && !text_contains_urlish_token(text)
        && !text_contains_codeish_token(text)
        && !text_has_dense_punctuation(text)
}

fn text_contains_urlish_token(text: &str) -> bool {
    text.split_whitespace().any(|token| {
        let token = token.trim_matches(|ch: char| matches!(ch, ',' | '.' | ')' | ']' | '}'));
        token.starts_with("http://")
            || token.starts_with("https://")
            || token.starts_with("www.")
            || token.contains("://")
            || token.contains('@')
            || (token.contains('.')
                && token.rsplit_once('.').is_some_and(|(_, suffix)| {
                    suffix.len() >= 2 && suffix.chars().all(|ch| ch.is_ascii_alphabetic())
                }))
    })
}

fn text_contains_codeish_token(text: &str) -> bool {
    const CODE_MARKERS: &[&str] = &[
        "`", "```", "::", "->", "=>", "==", "!=", "<=", ">=", "&&", "||", "</", "/>",
    ];
    if CODE_MARKERS.iter().any(|marker| text.contains(marker)) {
        return true;
    }
    text.split_whitespace().any(|token| {
        token
            .chars()
            .any(|ch| matches!(ch, '{' | '}' | '[' | ']' | ';' | '$' | '\\'))
            || (token.contains('/') && token.chars().any(|ch| ch.is_ascii_alphabetic()))
            || token
                .split('_')
                .nth(1)
                .is_some_and(|_| token.chars().any(|ch| ch.is_ascii_alphabetic()))
    })
}

fn text_has_dense_punctuation(text: &str) -> bool {
    let mut punctuation = 0_usize;
    let mut non_space = 0_usize;
    for ch in text.chars() {
        if ch.is_whitespace() {
            continue;
        }
        non_space += 1;
        if ch.is_ascii_punctuation() && !matches!(ch, '.' | ',' | '!' | '?' | ':' | '-') {
            punctuation += 1;
        }
    }
    non_space > 0 && punctuation * 4 > non_space
}

fn single_session_color_attrs(color: TextColor) -> Attrs<'static> {
    Attrs::new()
        .family(Family::Name(SINGLE_SESSION_FONT_FAMILY))
        .color(color)
}

#[cfg_attr(not(test), allow(dead_code))]
pub(crate) fn user_prompt_number_color(turn: usize) -> TextColor {
    user_prompt_number_color_for_distance(turn.saturating_sub(1))
}

fn user_prompt_number_color_for_distance(distance: usize) -> TextColor {
    // Match the TUI prompt-number effect: recent prompts start in a softened
    // rainbow and older prompts exponentially decay toward gray.
    const RAINBOW: [[f32; 3]; 7] = [
        [1.000, 0.314, 0.314],
        [1.000, 0.627, 0.314],
        [1.000, 0.902, 0.314],
        [0.314, 0.863, 0.392],
        [0.314, 0.784, 0.863],
        [0.392, 0.549, 1.000],
        [0.706, 0.392, 1.000],
    ];
    const GRAY: [f32; 3] = [0.314, 0.314, 0.314];

    let decay = (-0.4 * distance as f32).exp();
    let rainbow = RAINBOW[distance.min(RAINBOW.len() - 1)];
    text_color([
        rainbow[0] * decay + GRAY[0] * (1.0 - decay),
        rainbow[1] * decay + GRAY[1] * (1.0 - decay),
        rainbow[2] * decay + GRAY[2] * (1.0 - decay),
        1.0,
    ])
}

pub(crate) fn single_session_line_color(style: SingleSessionLineStyle) -> TextColor {
    text_color(single_session_line_rgba(style))
}

fn single_session_line_rgba(style: SingleSessionLineStyle) -> [f32; 4] {
    match style {
        SingleSessionLineStyle::Assistant => ASSISTANT_TEXT_COLOR,
        SingleSessionLineStyle::AssistantHeading => ASSISTANT_HEADING_TEXT_COLOR,
        SingleSessionLineStyle::AssistantQuote => ASSISTANT_QUOTE_TEXT_COLOR,
        SingleSessionLineStyle::AssistantTable => ASSISTANT_TABLE_TEXT_COLOR,
        SingleSessionLineStyle::AssistantLink | SingleSessionLineStyle::AssistantMedia => {
            ASSISTANT_LINK_TEXT_COLOR
        }
        SingleSessionLineStyle::CodeHeader => META_TEXT_COLOR,
        SingleSessionLineStyle::Code => CODE_TEXT_COLOR,
        SingleSessionLineStyle::User => USER_TEXT_COLOR,
        SingleSessionLineStyle::UserContinuation => USER_CONTINUATION_TEXT_COLOR,
        SingleSessionLineStyle::Tool => TOOL_TEXT_COLOR,
        SingleSessionLineStyle::Meta | SingleSessionLineStyle::Blank => META_TEXT_COLOR,
        SingleSessionLineStyle::Status => STATUS_TEXT_ACCENT_COLOR,
        SingleSessionLineStyle::Error => ERROR_TEXT_COLOR,
        SingleSessionLineStyle::OverlayTitle => PANEL_TITLE_COLOR,
        SingleSessionLineStyle::Overlay => OVERLAY_TEXT_COLOR,
        SingleSessionLineStyle::OverlaySelection => OVERLAY_SELECTION_TEXT_COLOR,
    }
}

pub(crate) fn single_session_text_areas(
    buffers: &[Buffer],
    size: PhysicalSize<u32>,
) -> Vec<TextArea<'_>> {
    single_session_text_areas_for_fresh_state(buffers, size, false)
}

#[cfg(test)]
pub(crate) fn single_session_text_areas_for_app<'a>(
    app: &SingleSessionApp,
    buffers: &'a [Buffer],
    size: PhysicalSize<u32>,
) -> Vec<TextArea<'a>> {
    single_session_text_areas_for_app_with_scroll(app, buffers, size, 0, 0.0)
}

pub(crate) fn single_session_text_areas_for_app_with_scroll<'a>(
    app: &SingleSessionApp,
    buffers: &'a [Buffer],
    size: PhysicalSize<u32>,
    tick: u64,
    smooth_scroll_lines: f32,
) -> Vec<TextArea<'a>> {
    let inline_widget_kind = app.render_inline_widget_kind();
    let inline_widget_lines = app.render_inline_widget_styled_lines();
    let inline_widget_preview_start_line =
        inline_widget_split_preview_start(inline_widget_kind, &inline_widget_lines);
    let inline_widget_text_width = inline_widget_text_width_for_lines(
        inline_widget_kind,
        &inline_widget_lines,
        size,
        app.text_scale(),
    );
    let viewport = single_session_body_viewport_for_tick(app, size, tick, smooth_scroll_lines);
    let layout = single_session_layout_for_total_lines(app, size, viewport.total_lines);
    let welcome_chrome_offset_pixels =
        welcome_timeline_visual_offset_pixels(app, size, smooth_scroll_lines);
    let welcome_chrome_visible =
        welcome_timeline_chrome_visible(app, size, welcome_chrome_offset_pixels);
    single_session_text_areas_for_state(
        buffers,
        size,
        welcome_chrome_visible,
        false,
        viewport.top_offset_pixels,
        layout.body.y,
        layout.body_text_bounds_bottom(),
        app.render_inline_widget_visible_line_count(),
        inline_widget_kind,
        inline_widget_preview_start_line,
        inline_widget_text_width,
        inline_widget_bottom_limit_for_layout(app, layout, welcome_chrome_visible),
        layout.draft_top,
        welcome_chrome_offset_pixels,
        welcome_status_lane_visible(app),
        app.is_fresh_welcome_visible() && app.draft.is_empty(),
        app.text_scale(),
        welcome_hero_runtime_mask_supported(&app.welcome_hero_text()),
        1.0,
        app.render_inline_widget_reveal_progress(),
    )
}

pub(crate) fn single_session_text_areas_for_app_with_cached_body<'a>(
    app: &SingleSessionApp,
    buffers: &'a [Buffer],
    size: PhysicalSize<u32>,
    smooth_scroll_lines: f32,
    rendered_body_lines: &[SingleSessionStyledLine],
) -> Vec<TextArea<'a>> {
    let viewport = single_session_body_viewport_from_lines(
        app,
        size,
        smooth_scroll_lines,
        rendered_body_lines,
    );
    single_session_text_areas_for_app_with_cached_body_viewport(
        app,
        buffers,
        size,
        smooth_scroll_lines,
        viewport,
    )
}

pub(crate) fn single_session_text_areas_for_app_with_cached_body_viewport<'a>(
    app: &SingleSessionApp,
    buffers: &'a [Buffer],
    size: PhysicalSize<u32>,
    smooth_scroll_lines: f32,
    viewport: SingleSessionBodyViewport,
) -> Vec<TextArea<'a>> {
    single_session_text_areas_for_app_with_cached_body_viewport_and_reveal(
        app,
        buffers,
        size,
        smooth_scroll_lines,
        viewport,
        1.0,
    )
}

pub(crate) fn single_session_text_areas_for_app_with_cached_body_viewport_and_reveal<'a>(
    app: &SingleSessionApp,
    buffers: &'a [Buffer],
    size: PhysicalSize<u32>,
    smooth_scroll_lines: f32,
    viewport: SingleSessionBodyViewport,
    welcome_hero_reveal_progress: f32,
) -> Vec<TextArea<'a>> {
    let inline_widget_kind = app.render_inline_widget_kind();
    let inline_widget_lines = app.render_inline_widget_styled_lines();
    let inline_widget_preview_start_line =
        inline_widget_split_preview_start(inline_widget_kind, &inline_widget_lines);
    let inline_widget_text_width = inline_widget_text_width_for_lines(
        inline_widget_kind,
        &inline_widget_lines,
        size,
        app.text_scale(),
    );
    let welcome_chrome_offset_pixels = welcome_timeline_visual_offset_pixels_for_total_lines(
        app,
        size,
        smooth_scroll_lines,
        viewport.total_lines,
    );
    let layout = single_session_layout_for_total_lines(app, size, viewport.total_lines);
    let welcome_chrome_visible =
        welcome_timeline_chrome_visible(app, size, welcome_chrome_offset_pixels);
    single_session_text_areas_for_state(
        buffers,
        size,
        welcome_chrome_visible,
        false,
        viewport.top_offset_pixels,
        layout.body.y,
        layout.body_text_bounds_bottom(),
        app.render_inline_widget_visible_line_count(),
        inline_widget_kind,
        inline_widget_preview_start_line,
        inline_widget_text_width,
        inline_widget_bottom_limit_for_layout(app, layout, welcome_chrome_visible),
        layout.draft_top,
        welcome_chrome_offset_pixels,
        welcome_status_lane_visible(app),
        app.is_fresh_welcome_visible() && app.draft.is_empty(),
        app.text_scale(),
        welcome_hero_runtime_mask_supported(&app.welcome_hero_text()),
        welcome_hero_reveal_progress,
        app.render_inline_widget_reveal_progress(),
    )
}

pub(crate) fn single_session_streaming_text_area_for_cached_body_viewport<'a>(
    app: &SingleSessionApp,
    buffer: &'a Buffer,
    size: PhysicalSize<u32>,
    viewport: SingleSessionBodyViewport,
    streaming_start_line: usize,
    opacity: f32,
    y_offset_pixels: f32,
) -> TextArea<'a> {
    let layout = single_session_layout_for_total_lines(app, size, viewport.total_lines);
    let line_height = layout.metrics.body_line_height;
    let left = PANEL_TITLE_LEFT_PADDING;
    let right = single_session_content_right(size) as i32;
    let body_top = layout.body.y;
    let top = body_top
        + viewport.top_offset_pixels
        + streaming_start_line.saturating_sub(viewport.start_line) as f32 * line_height
        + y_offset_pixels.max(0.0);
    TextArea {
        buffer,
        left,
        top,
        scale: 1.0,
        bounds: TextBounds {
            left: 0,
            top: body_top as i32,
            right,
            bottom: layout.body_text_bounds_bottom(),
        },
        default_color: text_color([
            ASSISTANT_TEXT_COLOR[0],
            ASSISTANT_TEXT_COLOR[1],
            ASSISTANT_TEXT_COLOR[2],
            ASSISTANT_TEXT_COLOR[3] * opacity.clamp(0.0, 1.0),
        ]),
    }
}

pub(crate) fn single_session_text_areas_for_fresh_state(
    buffers: &[Buffer],
    size: PhysicalSize<u32>,
    fresh_welcome_visible: bool,
) -> Vec<TextArea<'_>> {
    single_session_text_areas_for_state(
        buffers,
        size,
        fresh_welcome_visible,
        false,
        0.0,
        PANEL_BODY_TOP_PADDING,
        text_bounds_bottom(single_session_body_bottom(size)),
        0,
        None,
        None,
        0.0,
        single_session_draft_top_for_fresh_state(size, fresh_welcome_visible),
        single_session_draft_top_for_fresh_state(size, fresh_welcome_visible),
        0.0,
        false,
        false,
        1.0,
        false,
        1.0,
        1.0,
    )
}

fn welcome_status_lane_visible(app: &SingleSessionApp) -> bool {
    let _ = app;
    false
}

#[allow(clippy::too_many_arguments)]
pub(crate) fn single_session_text_areas_for_state(
    buffers: &[Buffer],
    size: PhysicalSize<u32>,
    welcome_chrome_visible: bool,
    welcome_handoff_visible: bool,
    body_top_offset_pixels: f32,
    body_top: f32,
    body_bottom: i32,
    inline_widget_line_count: usize,
    inline_widget_kind: Option<InlineWidgetKind>,
    inline_widget_preview_start_line: Option<usize>,
    inline_widget_text_width: f32,
    inline_widget_bottom_limit: f32,
    draft_top: f32,
    welcome_chrome_offset_pixels: f32,
    status_lane_visible: bool,
    startup_hint_visible: bool,
    ui_scale: f32,
    welcome_hero_runtime_mask_available: bool,
    welcome_hero_reveal_progress: f32,
    inline_widget_reveal_progress: f32,
) -> Vec<TextArea<'_>> {
    if buffers.len() < 4 {
        return Vec::new();
    }

    let left = PANEL_TITLE_LEFT_PADDING;
    let right = single_session_content_right(size) as i32;
    let bottom = size.height.saturating_sub(PANEL_TITLE_TOP_PADDING as u32) as i32;
    let body_top = if welcome_handoff_visible {
        draft_top
    } else {
        body_top
    };
    let body_bottom = if welcome_handoff_visible {
        bottom
    } else {
        body_bottom
    };
    let version_label = fresh_welcome_version_label();
    let version_font_size = fresh_welcome_version_font_size() * ui_scale;
    let version_left = if welcome_chrome_visible {
        fresh_welcome_version_left(&version_label, size, version_font_size)
    } else {
        (size.width as f32 * 0.42).max(left + 220.0)
    };
    let version_top = if welcome_chrome_visible {
        fresh_welcome_version_top_for_scale(size, ui_scale) + welcome_chrome_offset_pixels
    } else {
        PANEL_TITLE_TOP_PADDING + 3.0
    };
    let version_bounds_top = if welcome_chrome_visible {
        version_top as i32
    } else {
        0
    };
    let version_bounds_bottom = if welcome_chrome_visible {
        (version_top + version_font_size * 1.4) as i32
    } else {
        64
    };

    let typography = single_session_typography_for_scale(ui_scale);
    let inline_widget_layout = if inline_widget_line_count > 0 {
        let target_top = inline_widget_target_top(
            size,
            ui_scale,
            body_bottom as f32,
            welcome_chrome_visible,
            welcome_chrome_offset_pixels,
        );
        inline_widget_card_layout_with_bottom_limit(
            size,
            inline_widget_kind,
            &typography,
            inline_widget_line_count,
            inline_widget_text_width,
            target_top,
            inline_widget_reveal_progress,
            inline_widget_bottom_limit,
        )
    } else {
        None
    };

    let mut areas = Vec::new();

    // Keep the composer lane first in glyphon preparation order. The visual
    // positions are unchanged, but fresh keystrokes get shaped before the
    // heavier transcript/chrome text on frames where both changed.
    if !status_lane_visible && !welcome_handoff_visible {
        areas.push(TextArea {
            buffer: &buffers[2],
            left,
            top: draft_top,
            scale: 1.0,
            bounds: TextBounds {
                left: 0,
                top: draft_top as i32,
                right,
                bottom,
            },
            default_color: text_color(PANEL_SECTION_COLOR),
        });
    }

    if startup_hint_visible
        && !welcome_handoff_visible
        && !status_lane_visible
        && let Some(hint_buffer) = buffers.get(6)
    {
        let hint_top = draft_top + typography.code_size * typography.code_line_height * 1.35;
        areas.push(TextArea {
            buffer: hint_buffer,
            left,
            top: hint_top,
            scale: 1.0,
            bounds: TextBounds {
                left: 0,
                top: hint_top as i32,
                right,
                bottom,
            },
            default_color: text_color(META_TEXT_COLOR),
        });
    }

    areas.push(TextArea {
        buffer: &buffers[0],
        left,
        top: PANEL_TITLE_TOP_PADDING,
        scale: 1.0,
        bounds: TextBounds {
            left: 0,
            top: 0,
            right,
            bottom: 64,
        },
        default_color: text_color(PANEL_TITLE_COLOR),
    });
    areas.push(TextArea {
        buffer: &buffers[3],
        left: version_left,
        top: version_top,
        scale: 1.0,
        bounds: TextBounds {
            left: 0,
            top: version_bounds_top,
            right,
            bottom: version_bounds_bottom,
        },
        default_color: text_color(META_TEXT_COLOR),
    });
    areas.push(TextArea {
        buffer: &buffers[1],
        left,
        top: body_top + body_top_offset_pixels,
        scale: 1.0,
        bounds: TextBounds {
            left: 0,
            top: body_top as i32,
            right,
            bottom: body_bottom,
        },
        default_color: text_color(ASSISTANT_TEXT_COLOR),
    });

    if welcome_chrome_visible
        && !welcome_hero_runtime_mask_available
        && !welcome_hero_reveal_is_active(welcome_hero_reveal_progress)
        && let Some(hero_buffer) = buffers.get(5)
    {
        let (hero_min, hero_max) = glyph_welcome_hero_bounds(size, ui_scale);
        areas.push(TextArea {
            buffer: hero_buffer,
            left: hero_min[0],
            top: hero_min[1] + welcome_chrome_offset_pixels,
            scale: 1.0,
            bounds: TextBounds {
                left: hero_min[0] as i32,
                top: (hero_min[1] + welcome_chrome_offset_pixels) as i32,
                right: hero_max[0].ceil() as i32,
                bottom: (hero_max[1] + welcome_chrome_offset_pixels).ceil() as i32,
            },
            default_color: text_color(WELCOME_HANDWRITING_COLOR),
        });
    }

    if inline_widget_line_count > 0
        && let Some(buffer) = buffers.get(4)
        && let Some(layout) = inline_widget_layout
    {
        let split_columns = (inline_widget_kind == Some(InlineWidgetKind::SessionSwitcher))
            .then(|| session_switcher_split_columns(&layout))
            .flatten();
        let rail_bounds_right = split_columns
            .map(|columns| columns.rail.x + columns.rail.width - layout.padding_x * 0.75);
        let inline_bounds_right = rail_bounds_right
            .unwrap_or(layout.visible_text_right)
            .min(right as f32)
            .max(layout.text_left);
        let inline_bounds_bottom = layout
            .visible_text_bottom
            .min(draft_top)
            .max(layout.text_top);
        if inline_bounds_right > layout.text_left && inline_bounds_bottom > layout.text_top {
            areas.push(TextArea {
                buffer,
                left: layout.text_left,
                top: layout.text_top,
                scale: 1.0,
                bounds: TextBounds {
                    left: 0,
                    top: layout.text_top as i32,
                    right: inline_bounds_right as i32,
                    bottom: inline_bounds_bottom as i32,
                },
                default_color: text_color(ASSISTANT_TEXT_COLOR),
            });
        }
        if inline_widget_kind == Some(InlineWidgetKind::SessionSwitcher)
            && let Some(preview_buffer) = buffers.get(7)
        {
            let columns = split_columns.unwrap_or_else(|| {
                let fallback_gap = (layout.card.width * 0.018).clamp(9.0, 15.0);
                let rail_width = (layout.card.width * 0.38).clamp(220.0, layout.card.width * 0.55);
                let rail = Rect {
                    x: layout.card.x + layout.padding_x * 0.72,
                    y: layout.card.y + layout.padding_x * 0.18,
                    width: rail_width,
                    height: (layout.card.height - layout.padding_x * 0.36).max(1.0),
                };
                let gap = Rect {
                    x: rail.x + rail.width,
                    y: rail.y,
                    width: fallback_gap,
                    height: rail.height,
                };
                let preview = Rect {
                    x: gap.x + gap.width,
                    y: rail.y,
                    width: (layout.card.x + layout.card.width
                        - gap.x
                        - gap.width
                        - layout.padding_x * 0.72)
                        .max(96.0),
                    height: rail.height,
                };
                SessionSwitcherSplitColumns { rail, preview, gap }
            });
            let preview_left = columns.preview.x + layout.padding_x * 0.95;
            let preview_right = (columns.preview.x + columns.preview.width
                - layout.padding_x * 0.85)
                .min(right as f32)
                .max(preview_left);
            let preview_top = (layout.text_top
                + inline_widget_preview_start_line.unwrap_or(0) as f32
                    * inline_widget_line_height(inline_widget_kind, &typography))
            .max(columns.preview.y + 8.0);
            let preview_bottom = layout
                .visible_text_bottom
                .min(columns.preview.y + columns.preview.height - 8.0)
                .min(draft_top)
                .max(preview_top + 1.0);
            if preview_right > preview_left {
                areas.push(TextArea {
                    buffer: preview_buffer,
                    left: preview_left,
                    top: preview_top,
                    scale: 1.0,
                    bounds: TextBounds {
                        left: preview_left as i32,
                        top: preview_top as i32,
                        right: preview_right as i32,
                        bottom: preview_bottom as i32,
                    },
                    default_color: text_color(ASSISTANT_TEXT_COLOR),
                });
            }
        }
    }

    areas
}

fn visualize_composer_whitespace(text: &str) -> String {
    text.to_string()
}

pub(crate) fn desktop_header_version_label() -> String {
    desktop_app_directory_label()
}

fn desktop_app_directory_label() -> String {
    std::env::current_exe()
        .ok()
        .and_then(|path| {
            path.parent()
                .map(|directory| directory.display().to_string())
        })
        .unwrap_or_else(|| "unknown app directory".to_string())
}

pub(crate) fn fresh_welcome_version_label() -> String {
    desktop_app_directory_label()
}

fn fresh_welcome_version_font_size() -> f32 {
    (single_session_typography().meta_size * 0.58).clamp(11.0, 14.0)
}

fn fresh_welcome_version_top_for_scale(size: PhysicalSize<u32>, ui_scale: f32) -> f32 {
    handwritten_welcome_bounds_for_phrase_with_scale(size, handwritten_welcome_phrase(0), ui_scale)
        .1[1]
        + fresh_welcome_version_gap_for_scale(ui_scale)
}

fn fresh_welcome_version_gap_for_scale(ui_scale: f32) -> f32 {
    (fresh_welcome_version_font_size() * ui_scale * 2.25).max(30.0 * ui_scale)
}

fn fresh_welcome_version_left(label: &str, size: PhysicalSize<u32>, font_size: f32) -> f32 {
    let estimated_width = label.chars().count() as f32 * font_size * 0.58;
    ((size.width as f32 - estimated_width) * 0.5).max(PANEL_TITLE_LEFT_PADDING)
}

pub(crate) fn text_color(color: [f32; 4]) -> TextColor {
    TextColor::rgba(
        (color[0].clamp(0.0, 1.0) * 255.0).round() as u8,
        (color[1].clamp(0.0, 1.0) * 255.0).round() as u8,
        (color[2].clamp(0.0, 1.0) * 255.0).round() as u8,
        (color[3].clamp(0.0, 1.0) * 255.0).round() as u8,
    )
}

#[cfg(test)]
mod tests {
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
            inline_widget_target_top(size, app.text_scale(), layout.body_bottom(), false, 0.0),
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
            SingleSessionStyledLine::new(
                "Anthropic · chat · available",
                SingleSessionLineStyle::Meta,
            ),
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
        let mut model_vertices = Vec::new();
        push_single_session_inline_widget_structured_chrome(
            &mut model_vertices,
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
            vertex_count_for_color(&model_vertices, INLINE_COMMAND_ROW_ACCENT_COLOR) > 0,
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
            SingleSessionStyledLine::new(
                " /copy       copy latest",
                SingleSessionLineStyle::Overlay,
            ),
            SingleSessionStyledLine::new(
                " /model      switch model",
                SingleSessionLineStyle::Overlay,
            ),
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
            SingleSessionStyledLine::new(
                " /copy       copy latest",
                SingleSessionLineStyle::Overlay,
            ),
            SingleSessionStyledLine::new(
                " /model      switch model",
                SingleSessionLineStyle::Overlay,
            ),
        ];
        let reflow = registry.frame_for_rows(
            Some(kind),
            &filtered,
            filtered.len(),
            now + Duration::from_millis(4),
        );
        assert!(reflow.is_active());
        let inserted =
            test_inline_widget_reflow_visual_for_text(&reflow, kind, &filtered, "/commands");
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
            SingleSessionStyledLine::new(
                " /copy       copy latest",
                SingleSessionLineStyle::Overlay,
            ),
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

        let remove_time =
            settled_time + ATTACHMENT_CHIP_ENTRY_DURATION * 2 + Duration::from_millis(5);
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
        let shift_start =
            registry.frame(&shifted_lines, line_height, now + Duration::from_millis(4));
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
        let entry_visual = test_inline_markdown_pill_visual_for_line(
            &entry,
            &lines,
            2,
            InlineMarkdownPillKind::Code,
        );
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

        let start =
            registry.frame_for_metrics(size, 0.0, Some(bottom), now + Duration::from_millis(5));
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
}
