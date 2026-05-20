use super::*;
use crate::desktop_rich_text::{
    AnsiColor, AnsiStyle, RichLine, RichLineStyle, RichSpanStyle, SyntaxTokenKind,
};
use crate::single_session::{
    MODEL_PICKER_INLINE_ROW_LIMIT, SingleSessionInlineSpan, SingleSessionInlineSpanKind,
    SingleSessionTypography, single_session_trimmed_line_end_preserving_inline_code_whitespace,
};

pub(crate) const INLINE_MATH_BACKGROUND_COLOR: [f32; 4] = [0.035, 0.220, 0.155, 0.115];
pub(crate) const MARKDOWN_HEADING_BACKGROUND_COLOR: [f32; 4] = [0.060, 0.180, 0.520, 0.055];
pub(crate) const MARKDOWN_RULE_COLOR: [f32; 4] = [0.060, 0.130, 0.260, 0.220];
pub(crate) const MARKDOWN_LIST_MARKER_COLOR: [f32; 4] = [0.060, 0.110, 0.240, 0.960];
pub(crate) const MARKDOWN_TASK_DONE_COLOR: [f32; 4] = [0.025, 0.350, 0.190, 1.000];
pub(crate) const MARKDOWN_TASK_OPEN_COLOR: [f32; 4] = [0.420, 0.320, 0.075, 0.980];
pub(crate) const MARKDOWN_STRIKE_TEXT_COLOR: [f32; 4] = [0.310, 0.330, 0.380, 0.880];

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
    pub(crate) body: Vec<SingleSessionStyledLine>,
    pub(crate) inline_widget: Vec<SingleSessionStyledLine>,
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

    let welcome_chrome_offset = if app.is_welcome_timeline_visible() {
        welcome_timeline_visual_offset_pixels(app, size, smooth_scroll_lines)
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
        welcome_timeline_total_body_lines(app, size),
    );
    push_single_session_transcript_cards(
        &mut vertices,
        app,
        size,
        spinner_tick,
        smooth_scroll_lines,
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
        push_streaming_activity_cue(&mut vertices, app, size, spinner_tick, None);
    }
    push_single_session_selection(&mut vertices, app, size);
    push_single_session_scrollbar(&mut vertices, app, size, spinner_tick, smooth_scroll_lines);

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
    );

    let viewport = single_session_body_viewport_from_lines(
        app,
        size,
        smooth_scroll_lines,
        rendered_body_lines,
    );
    push_single_session_transcript_cards_from_viewport(
        &mut vertices,
        app,
        size,
        &viewport,
        rendered_body_lines.len(),
    );
    push_single_session_inline_code_cards_from_viewport(
        &mut vertices,
        app,
        size,
        &viewport,
        rendered_body_lines.len(),
    );
    push_single_session_markdown_rule_lines_from_viewport(
        &mut vertices,
        app,
        size,
        &viewport,
        rendered_body_lines.len(),
    );
    if app.has_activity_indicator() {
        push_streaming_activity_cue(&mut vertices, app, size, spinner_tick, Some(&viewport));
    }
    push_single_session_selection(&mut vertices, app, size);
    push_single_session_scrollbar_for_total_lines(
        &mut vertices,
        app,
        size,
        smooth_scroll_lines,
        rendered_body_lines.len(),
    );

    vertices
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
    if app.inline_widget_line_count() == 0 {
        return 0.0;
    }

    let typography = single_session_typography_for_scale(app.text_scale());
    let line_height = typography.body_size * typography.body_line_height;
    let visual_bottom = fresh_welcome_visual_bottom_for_scale(size, app.text_scale());
    let gap = fresh_welcome_inline_widget_gap_for_scale(app.text_scale());
    let draft_top = single_session_draft_top_for_app(app, size);
    let inline_height = inline_widget_text_height(app).max(line_height);
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
) {
    let line_count = app.inline_widget_line_count();
    if line_count == 0 {
        return;
    }

    let progress = app.inline_widget_reveal_progress().clamp(0.0, 1.0);
    if progress <= 0.001 {
        return;
    }

    let typography = single_session_typography_for_scale(app.text_scale());
    let body_bottom = single_session_body_bottom_for_total_lines(app, size, total_lines);
    let welcome_chrome_visible =
        welcome_timeline_chrome_visible(app, size, welcome_chrome_offset_pixels);
    let target_top = inline_widget_target_top(
        size,
        app.text_scale(),
        body_bottom,
        welcome_chrome_visible,
        welcome_chrome_offset_pixels,
    );
    let inline_lines = app.inline_widget_styled_lines();
    let Some(layout) = inline_widget_card_layout(
        size,
        &typography,
        line_count,
        inline_widget_intrinsic_text_width(&inline_lines, size, app.text_scale()),
        target_top,
        progress,
    ) else {
        return;
    };

    if app.active_inline_widget_uses_card_chrome() {
        const INLINE_CARD_BACKGROUND_COLOR: [f32; 4] = [0.972, 0.982, 1.000, 0.54];
        const INLINE_CARD_BORDER_COLOR: [f32; 4] = [0.180, 0.255, 0.430, 0.18];
        push_rounded_rect(
            vertices,
            layout.card,
            INLINE_WIDGET_CARD_RADIUS,
            with_alpha(
                INLINE_CARD_BORDER_COLOR,
                INLINE_CARD_BORDER_COLOR[3] * progress,
            ),
            size,
        );
        push_rounded_rect(
            vertices,
            inset_rect(layout.card, 1.0),
            INLINE_WIDGET_CARD_RADIUS - 1.0,
            with_alpha(
                INLINE_CARD_BACKGROUND_COLOR,
                INLINE_CARD_BACKGROUND_COLOR[3] * progress,
            ),
            size,
        );
    }

    if app.model_picker.open
        && !app.model_picker.loading
        && app.model_picker.error.is_none()
        && let Some(row) = app
            .model_picker
            .selected_row_in_window(MODEL_PICKER_INLINE_ROW_LIMIT)
    {
        let selected_line = 2 + row * 2;
        if selected_line < line_count {
            let line_height = typography.body_size * typography.body_line_height;
            let row_top = layout.text_top + selected_line as f32 * line_height - 2.0;
            let row_visible_height =
                (layout.visible_text_bottom - row_top).min(line_height * 2.0 + 2.0);
            let row_width = (layout.card.width - INLINE_WIDGET_CARD_PADDING_X).max(0.0);
            if row_visible_height <= 3.0 || row_width <= 6.0 {
                return;
            }
            push_rounded_rect(
                vertices,
                Rect {
                    x: layout.card.x + 6.0,
                    y: row_top,
                    width: row_width,
                    height: row_visible_height.max(1.0),
                },
                INLINE_WIDGET_SELECTION_RADIUS,
                with_alpha(
                    OVERLAY_SELECTION_BACKGROUND_COLOR,
                    OVERLAY_SELECTION_BACKGROUND_COLOR[3] * progress,
                ),
                size,
            );
        }
    }
}

const INLINE_WIDGET_SIDE_GUTTER_EXTRA: f32 = 24.0;
const INLINE_WIDGET_CARD_PADDING_X: f32 = 14.0;
const INLINE_WIDGET_CARD_PADDING_Y: f32 = 8.0;
const INLINE_WIDGET_BODY_GAP: f32 = 8.0;
const INLINE_WIDGET_CARD_RADIUS: f32 = 18.0;
const INLINE_WIDGET_SELECTION_RADIUS: f32 = 10.0;

#[derive(Clone, Copy, Debug)]
struct InlineWidgetCardLayout {
    card: Rect,
    text_left: f32,
    text_top: f32,
    visible_text_right: f32,
    visible_text_bottom: f32,
}

fn inline_widget_card_layout(
    size: PhysicalSize<u32>,
    typography: &SingleSessionTypography,
    line_count: usize,
    text_width: f32,
    text_top: f32,
    progress: f32,
) -> Option<InlineWidgetCardLayout> {
    if line_count == 0 {
        return None;
    }

    let progress = progress.clamp(0.0, 1.0);
    if progress <= 0.001 {
        return None;
    }

    let line_height = typography.body_size * typography.body_line_height;
    let text_left = inline_widget_text_left(size);
    let text_width = text_width
        .max(line_height * 8.0)
        .min(inline_widget_max_text_width(size))
        .max(1.0);
    let text_height = line_count as f32 * line_height;
    let final_card = Rect {
        x: (text_left - INLINE_WIDGET_CARD_PADDING_X).max(0.0),
        y: (text_top - INLINE_WIDGET_CARD_PADDING_Y).max(PANEL_TITLE_TOP_PADDING),
        width: text_width + INLINE_WIDGET_CARD_PADDING_X * 2.0,
        height: text_height + INLINE_WIDGET_CARD_PADDING_Y * 2.0,
    };
    let start_width = (line_height * 2.0).min(final_card.width);
    let start_height = (line_height * 0.72).min(final_card.height);
    let card = Rect {
        x: final_card.x,
        y: final_card.y,
        width: start_width + (final_card.width - start_width) * progress,
        height: start_height + (final_card.height - start_height) * progress,
    };
    let visible_text_right = (card.x + card.width - INLINE_WIDGET_CARD_PADDING_X)
        .max(text_left)
        .min(text_left + text_width);
    let visible_text_bottom = (card.y + card.height - INLINE_WIDGET_CARD_PADDING_Y)
        .max(text_top)
        .min(text_top + text_height);

    Some(InlineWidgetCardLayout {
        card,
        text_left,
        text_top,
        visible_text_right,
        visible_text_bottom,
    })
}

fn inline_widget_intrinsic_text_width(
    lines: &[SingleSessionStyledLine],
    size: PhysicalSize<u32>,
    ui_scale: f32,
) -> f32 {
    let typography = single_session_typography_for_scale(ui_scale);
    let average_char_width = typography.body_size * 0.57;
    let max_columns = lines
        .iter()
        .map(|line| inline_widget_visual_columns(&line.text))
        .max()
        .unwrap_or_default() as f32;
    (max_columns * average_char_width)
        .ceil()
        .min(inline_widget_max_text_width(size))
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

fn inline_widget_max_text_width(size: PhysicalSize<u32>) -> f32 {
    let gutter = inline_widget_text_left(size);
    (size.width as f32 - gutter * 2.0).max(1.0)
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

fn handwritten_welcome_paths_for_phrase(phrase: &str) -> Vec<Vec<[f32; 2]>> {
    match phrase.trim().to_ascii_lowercase().as_str() {
        "hi there" => handwritten_hi_there_paths(),
        "hey there" => handwritten_hey_there_paths(),
        _ => handwritten_hello_there_paths(),
    }
}

#[allow(clippy::approx_constant)]
fn handwritten_hello_there_paths() -> Vec<Vec<[f32; 2]>> {
    vec![
        vec![
            [1.36, 0.882],
            [1.34, 0.891],
            [1.32, 0.9],
            [1.29, 0.909],
            [1.27, 0.917],
            [1.25, 0.924],
            [1.23, 0.931],
            [1.2, 0.937],
            [1.18, 0.943],
            [1.15, 0.949],
            [1.13, 0.955],
            [1.11, 0.961],
            [1.08, 0.967],
            [1.06, 0.972],
            [1.04, 0.977],
            [1.03, 0.981],
            [1.0, 0.98],
            [0.984, 0.982],
            [0.964, 0.989],
            [0.944, 0.997],
            [0.922, 1.0],
            [0.904, 1.0],
            [0.884, 0.997],
            [0.866, 1.0],
            [0.848, 1.01],
            [0.832, 1.02],
            [0.82, 1.03],
            [0.826, 1.05],
            [0.817, 1.07],
            [0.808, 1.09],
            [0.796, 1.11],
            [0.787, 1.12],
            [0.781, 1.14],
            [0.774, 1.16],
            [0.766, 1.18],
            [0.757, 1.2],
            [0.75, 1.22],
            [0.744, 1.24],
            [0.743, 1.26],
            [0.744, 1.27],
            [0.731, 1.29],
            [0.722, 1.31],
            [0.716, 1.33],
            [0.712, 1.35],
            [0.706, 1.37],
            [0.698, 1.39],
            [0.684, 1.41],
            [0.666, 1.41],
            [0.648, 1.4],
            [0.636, 1.39],
            [0.626, 1.37],
            [0.616, 1.35],
            [0.607, 1.33],
            [0.606, 1.31],
            [0.614, 1.29],
            [0.615, 1.27],
            [0.621, 1.26],
            [0.629, 1.24],
            [0.625, 1.22],
            [0.634, 1.2],
            [0.642, 1.18],
            [0.643, 1.16],
            [0.642, 1.14],
            [0.639, 1.11],
            [0.637, 1.09],
            [0.638, 1.07],
            [0.645, 1.05],
            [0.646, 1.03],
            [0.633, 1.01],
            [0.612, 1.02],
            [0.592, 1.02],
            [0.573, 1.02],
            [0.554, 1.01],
            [0.534, 1.01],
            [0.517, 1.02],
            [0.5, 1.01],
            [0.479, 1.01],
            [0.46, 1.01],
            [0.441, 1.01],
            [0.422, 1.01],
            [0.403, 1.01],
            [0.383, 1.01],
            [0.363, 1.01],
            [0.344, 1.0],
            [0.324, 1.01],
            [0.312, 1.03],
            [0.299, 1.05],
            [0.285, 1.07],
            [0.271, 1.09],
            [0.256, 1.11],
            [0.243, 1.13],
            [0.23, 1.15],
            [0.218, 1.17],
            [0.207, 1.19],
            [0.196, 1.21],
            [0.187, 1.22],
            [0.176, 1.24],
            [0.166, 1.26],
            [0.156, 1.27],
            [0.152, 1.3],
            [0.144, 1.31],
            [0.13, 1.33],
            [0.112, 1.34],
            [0.0963, 1.35],
            [0.0836, 1.36],
            [0.0645, 1.37],
            [0.0548, 1.39],
            [0.0481, 1.4],
            [0.0356, 1.42],
            [0.0184, 1.41],
            [0.00856, 1.39],
            [0.0104, 1.35],
            [0.02, 1.33],
            [0.0303, 1.3],
            [0.0411, 1.28],
            [0.0525, 1.26],
            [0.0645, 1.23],
            [0.077, 1.21],
            [0.0897, 1.19],
            [0.103, 1.16],
            [0.116, 1.14],
            [0.13, 1.12],
            [0.144, 1.09],
            [0.15, 1.07],
            [0.158, 1.05],
            [0.168, 1.03],
            [0.18, 1.01],
            [0.192, 0.994],
            [0.202, 0.976],
            [0.211, 0.957],
            [0.197, 0.939],
            [0.171, 0.915],
            [0.148, 0.892],
            [0.127, 0.867],
            [0.107, 0.841],
            [0.0883, 0.812],
            [0.0768, 0.793],
            [0.068, 0.777],
            [0.0593, 0.761],
            [0.0515, 0.74],
            [0.0531, 0.713],
            [0.0571, 0.687],
            [0.0676, 0.664],
            [0.0803, 0.648],
            [0.0897, 0.632],
            [0.106, 0.62],
            [0.123, 0.612],
            [0.141, 0.603],
            [0.158, 0.593],
            [0.177, 0.585],
            [0.197, 0.578],
            [0.216, 0.572],
            [0.237, 0.572],
            [0.258, 0.578],
            [0.28, 0.578],
            [0.3, 0.574],
            [0.321, 0.573],
            [0.341, 0.58],
            [0.358, 0.591],
            [0.376, 0.6],
            [0.396, 0.602],
            [0.412, 0.584],
            [0.421, 0.562],
            [0.429, 0.537],
            [0.435, 0.519],
            [0.441, 0.501],
            [0.447, 0.483],
            [0.453, 0.465],
            [0.461, 0.447],
            [0.469, 0.43],
            [0.48, 0.416],
            [0.488, 0.399],
            [0.495, 0.381],
            [0.502, 0.359],
            [0.508, 0.342],
            [0.512, 0.324],
            [0.513, 0.303],
            [0.527, 0.291],
            [0.524, 0.272],
            [0.525, 0.249],
            [0.544, 0.243],
            [0.564, 0.235],
            [0.582, 0.227],
            [0.602, 0.225],
            [0.616, 0.241],
            [0.617, 0.26],
            [0.615, 0.28],
            [0.615, 0.299],
            [0.613, 0.321],
            [0.608, 0.338],
            [0.601, 0.358],
            [0.593, 0.378],
            [0.584, 0.399],
            [0.574, 0.418],
            [0.564, 0.435],
            [0.556, 0.456],
            [0.555, 0.475],
            [0.552, 0.494],
            [0.548, 0.513],
            [0.542, 0.533],
            [0.535, 0.553],
            [0.528, 0.572],
            [0.52, 0.591],
            [0.511, 0.61],
            [0.502, 0.629],
            [0.493, 0.647],
            [0.484, 0.664],
            [0.475, 0.681],
            [0.466, 0.698],
            [0.456, 0.718],
            [0.445, 0.738],
            [0.435, 0.759],
            [0.426, 0.78],
            [0.418, 0.802],
            [0.412, 0.822],
            [0.408, 0.841],
            [0.375, 0.904],
            [0.392, 0.915],
            [0.411, 0.922],
            [0.432, 0.928],
            [0.452, 0.931],
            [0.471, 0.932],
            [0.49, 0.934],
            [0.507, 0.947],
            [0.525, 0.948],
            [0.546, 0.947],
            [0.566, 0.949],
            [0.586, 0.946],
            [0.596, 0.962],
            [0.612, 0.947],
            [0.63, 0.949],
            [0.644, 0.935],
            [0.668, 0.942],
            [0.689, 0.942],
            [0.709, 0.94],
            [0.732, 0.939],
            [0.743, 0.925],
            [0.758, 0.912],
            [0.763, 0.893],
            [0.759, 0.875],
            [0.769, 0.858],
            [0.779, 0.839],
            [0.795, 0.812],
            [0.811, 0.782],
            [0.825, 0.752],
            [0.838, 0.722],
            [0.847, 0.69],
            [0.851, 0.657],
            [0.852, 0.631],
            [0.872, 0.621],
            [0.879, 0.604],
            [0.878, 0.584],
            [0.876, 0.563],
            [0.879, 0.542],
            [0.891, 0.528],
            [0.907, 0.518],
            [0.916, 0.496],
            [0.922, 0.475],
            [0.929, 0.455],
            [0.936, 0.435],
            [0.944, 0.415],
            [0.951, 0.395],
            [0.958, 0.374],
            [0.964, 0.352],
            [0.969, 0.33],
            [0.973, 0.307],
            [0.98, 0.288],
            [0.992, 0.271],
            [1.01, 0.254],
            [1.02, 0.237],
            [1.03, 0.218],
            [1.03, 0.197],
            [1.03, 0.178],
            [1.05, 0.168],
            [1.07, 0.151],
            [1.08, 0.13],
            [1.09, 0.104],
            [1.09, 0.0829],
            [1.09, 0.0636],
            [1.11, 0.0496],
            [1.13, 0.059],
            [1.14, 0.0779],
            [1.15, 0.097],
            [1.15, 0.116],
            [1.14, 0.136],
            [1.14, 0.157],
            [1.13, 0.178],
            [1.13, 0.2],
            [1.12, 0.22],
            [1.11, 0.245],
            [1.1, 0.272],
            [1.09, 0.301],
            [1.09, 0.329],
            [1.09, 0.357],
            [1.07, 0.372],
            [1.06, 0.387],
            [1.06, 0.407],
            [1.05, 0.424],
            [1.04, 0.44],
            [1.04, 0.458],
            [1.03, 0.474],
            [1.03, 0.494],
            [1.02, 0.52],
            [1.01, 0.547],
            [0.993, 0.575],
            [0.982, 0.603],
            [0.971, 0.632],
            [0.961, 0.66],
            [0.952, 0.687],
            [0.944, 0.711],
            [0.935, 0.735],
            [0.926, 0.759],
            [0.916, 0.782],
            [0.905, 0.806],
            [0.903, 0.828],
            [0.905, 0.847],
            [0.895, 0.865],
            [0.88, 0.88],
            [0.867, 0.896],
            [0.872, 0.913],
            [0.889, 0.917],
            [0.91, 0.909],
            [0.931, 0.916],
            [0.951, 0.921],
            [0.967, 0.912],
            [0.984, 0.906],
            [1.0, 0.906],
            [1.02, 0.911],
            [1.03, 0.897],
            [1.06, 0.904],
            [1.08, 0.908],
            [1.09, 0.903],
            [1.11, 0.894],
            [1.13, 0.885],
            [1.15, 0.886],
            [1.17, 0.887],
            [1.19, 0.884],
            [1.21, 0.88],
            [1.24, 0.875],
            [1.26, 0.871],
            [1.29, 0.868],
            [1.31, 0.867],
            [1.34, 0.868],
            [1.36, 0.871],
        ],
        vec![
            [0.35, 0.629],
            [0.332, 0.63],
            [0.313, 0.637],
            [0.291, 0.634],
            [0.258, 0.632],
            [0.225, 0.638],
            [0.194, 0.65],
            [0.165, 0.667],
            [0.14, 0.688],
            [0.125, 0.71],
            [0.124, 0.734],
            [0.129, 0.758],
            [0.138, 0.777],
            [0.151, 0.795],
            [0.167, 0.809],
            [0.184, 0.821],
            [0.203, 0.834],
            [0.221, 0.847],
            [0.241, 0.859],
            [0.261, 0.853],
            [0.275, 0.835],
            [0.282, 0.819],
            [0.287, 0.8],
            [0.292, 0.78],
            [0.298, 0.761],
            [0.311, 0.746],
            [0.316, 0.727],
            [0.321, 0.71],
            [0.331, 0.692],
            [0.342, 0.674],
            [0.35, 0.655],
            [0.351, 0.634],
        ],
        vec![
            [2.12, 0.967],
            [2.11, 0.981],
            [2.1, 0.996],
            [2.08, 1.01],
            [2.07, 1.03],
            [2.06, 1.04],
            [2.04, 1.06],
            [2.03, 1.07],
            [2.01, 1.07],
            [1.99, 1.09],
            [1.98, 1.1],
            [1.97, 1.12],
            [1.95, 1.13],
            [1.93, 1.14],
            [1.9, 1.15],
            [1.88, 1.16],
            [1.86, 1.17],
            [1.84, 1.18],
            [1.82, 1.18],
            [1.69, 1.24],
            [1.67, 1.25],
            [1.65, 1.26],
            [1.63, 1.27],
            [1.61, 1.28],
            [1.59, 1.29],
            [1.57, 1.3],
            [1.55, 1.31],
            [1.52, 1.32],
            [1.53, 1.34],
            [1.52, 1.36],
            [1.5, 1.37],
            [1.48, 1.37],
            [1.45, 1.37],
            [1.43, 1.38],
            [1.41, 1.38],
            [1.39, 1.38],
            [1.37, 1.38],
            [1.35, 1.38],
            [1.33, 1.39],
            [1.3, 1.39],
            [1.28, 1.38],
            [1.26, 1.39],
            [1.24, 1.39],
            [1.22, 1.38],
            [1.2, 1.37],
            [1.19, 1.35],
            [1.18, 1.34],
            [1.17, 1.32],
            [1.16, 1.3],
            [1.15, 1.28],
            [1.13, 1.26],
            [1.12, 1.24],
            [1.11, 1.22],
            [1.11, 1.2],
            [1.11, 1.17],
            [1.11, 1.15],
            [1.12, 1.14],
            [1.13, 1.12],
            [1.14, 1.1],
            [1.14, 1.08],
            [1.14, 1.06],
            [1.14, 1.04],
            [1.16, 1.03],
            [1.18, 1.02],
            [1.19, 1.01],
            [1.18, 0.991],
            [1.19, 0.972],
            [1.2, 0.957],
            [1.21, 0.941],
            [1.21, 0.923],
            [1.22, 0.899],
            [1.23, 0.882],
            [1.24, 0.866],
            [1.26, 0.851],
            [1.27, 0.837],
            [1.29, 0.824],
            [1.3, 0.811],
            [1.32, 0.799],
            [1.33, 0.786],
            [1.35, 0.774],
            [1.37, 0.761],
            [1.38, 0.748],
            [1.41, 0.745],
            [1.43, 0.744],
            [1.44, 0.733],
            [1.46, 0.718],
            [1.47, 0.703],
            [1.49, 0.694],
            [1.51, 0.704],
            [1.54, 0.687],
            [1.56, 0.677],
            [1.59, 0.675],
            [1.61, 0.679],
            [1.63, 0.687],
            [1.65, 0.698],
            [1.66, 0.712],
            [1.68, 0.727],
            [1.68, 0.752],
            [1.68, 0.771],
            [1.68, 0.789],
            [1.68, 0.807],
            [1.68, 0.826],
            [1.67, 0.847],
            [1.66, 0.863],
            [1.65, 0.88],
            [1.64, 0.896],
            [1.63, 0.914],
            [1.63, 0.934],
            [1.62, 0.953],
            [1.61, 0.972],
            [1.59, 0.989],
            [1.58, 1.0],
            [1.56, 1.02],
            [1.54, 1.02],
            [1.53, 1.03],
            [1.52, 1.04],
            [1.51, 1.06],
            [1.49, 1.07],
            [1.47, 1.08],
            [1.46, 1.09],
            [1.45, 1.11],
            [1.43, 1.1],
            [1.4, 1.1],
            [1.39, 1.12],
            [1.39, 1.14],
            [1.37, 1.15],
            [1.36, 1.17],
            [1.34, 1.17],
            [1.32, 1.18],
            [1.3, 1.19],
            [1.28, 1.2],
            [1.26, 1.21],
            [1.24, 1.22],
            [1.23, 1.23],
            [1.21, 1.25],
            [1.23, 1.26],
            [1.25, 1.27],
            [1.27, 1.27],
            [1.29, 1.27],
            [1.31, 1.27],
            [1.33, 1.27],
            [1.35, 1.27],
            [1.36, 1.28],
            [1.39, 1.27],
            [1.41, 1.27],
            [1.43, 1.26],
            [1.45, 1.25],
            [1.47, 1.25],
            [1.49, 1.24],
            [1.5, 1.23],
            [1.52, 1.22],
            [1.54, 1.21],
            [1.56, 1.21],
            [1.58, 1.2],
            [1.61, 1.2],
            [1.62, 1.21],
            [1.64, 1.21],
            [1.65, 1.19],
            [1.66, 1.18],
            [1.67, 1.16],
            [1.69, 1.15],
            [1.7, 1.14],
            [1.73, 1.13],
            [1.75, 1.13],
            [1.77, 1.13],
            [1.78, 1.12],
            [1.8, 1.1],
            [1.82, 1.09],
            [1.83, 1.09],
            [1.85, 1.09],
            [1.87, 1.09],
            [1.89, 1.08],
            [1.91, 1.07],
            [1.92, 1.05],
            [1.94, 1.06],
            [1.96, 1.04],
            [1.97, 1.03],
            [1.99, 1.01],
            [2.01, 1.0],
            [2.03, 0.99],
            [2.04, 0.978],
            [2.06, 0.966],
            [2.08, 0.954],
            [2.1, 0.941],
            [2.12, 0.928],
            [2.12, 0.945],
            [2.12, 0.964],
        ],
        vec![
            [1.58, 0.802],
            [1.56, 0.792],
            [1.53, 0.786],
            [1.52, 0.787],
            [1.49, 0.797],
            [1.48, 0.808],
            [1.47, 0.822],
            [1.45, 0.836],
            [1.44, 0.849],
            [1.42, 0.86],
            [1.41, 0.868],
            [1.39, 0.871],
            [1.36, 0.867],
            [1.36, 0.888],
            [1.36, 0.908],
            [1.35, 0.929],
            [1.35, 0.95],
            [1.33, 0.96],
            [1.32, 0.972],
            [1.3, 0.988],
            [1.3, 1.0],
            [1.29, 1.02],
            [1.28, 1.04],
            [1.27, 1.06],
            [1.26, 1.08],
            [1.28, 1.09],
            [1.3, 1.08],
            [1.32, 1.07],
            [1.33, 1.06],
            [1.35, 1.05],
            [1.37, 1.04],
            [1.38, 1.03],
            [1.4, 1.02],
            [1.42, 1.01],
            [1.43, 1.0],
            [1.45, 0.983],
            [1.47, 0.964],
            [1.5, 0.951],
            [1.52, 0.932],
            [1.53, 0.909],
            [1.54, 0.885],
            [1.55, 0.86],
            [1.56, 0.841],
            [1.57, 0.824],
            [1.58, 0.805],
        ],
        vec![
            [2.81, 0.938],
            [2.81, 0.958],
            [2.81, 0.976],
            [2.8, 0.994],
            [2.75, 1.05],
            [2.72, 1.13],
            [2.71, 1.14],
            [2.69, 1.15],
            [2.68, 1.16],
            [2.66, 1.17],
            [2.65, 1.19],
            [2.63, 1.2],
            [2.62, 1.22],
            [2.6, 1.23],
            [2.58, 1.24],
            [2.57, 1.25],
            [2.54, 1.26],
            [2.53, 1.27],
            [2.51, 1.28],
            [2.49, 1.29],
            [2.48, 1.31],
            [2.46, 1.32],
            [2.45, 1.33],
            [2.43, 1.34],
            [2.41, 1.34],
            [2.39, 1.35],
            [2.37, 1.35],
            [2.34, 1.36],
            [2.32, 1.36],
            [2.3, 1.37],
            [2.28, 1.38],
            [2.26, 1.38],
            [2.23, 1.39],
            [2.21, 1.39],
            [2.19, 1.39],
            [2.16, 1.39],
            [2.14, 1.39],
            [2.11, 1.4],
            [2.1, 1.39],
            [2.08, 1.38],
            [2.06, 1.37],
            [2.05, 1.36],
            [2.03, 1.35],
            [2.02, 1.33],
            [2.01, 1.32],
            [2.0, 1.3],
            [1.99, 1.28],
            [1.98, 1.26],
            [1.98, 1.24],
            [1.97, 1.19],
            [1.97, 1.16],
            [1.97, 1.14],
            [1.96, 1.11],
            [1.96, 1.09],
            [1.97, 1.07],
            [1.98, 1.05],
            [1.98, 1.03],
            [1.99, 1.01],
            [1.99, 0.986],
            [2.0, 0.965],
            [2.01, 0.944],
            [2.01, 0.924],
            [2.02, 0.903],
            [2.03, 0.883],
            [2.04, 0.862],
            [2.04, 0.842],
            [2.05, 0.822],
            [2.06, 0.802],
            [2.07, 0.783],
            [2.08, 0.763],
            [2.09, 0.744],
            [2.1, 0.725],
            [2.11, 0.703],
            [2.12, 0.682],
            [2.12, 0.662],
            [2.13, 0.643],
            [2.15, 0.623],
            [2.16, 0.607],
            [2.17, 0.591],
            [2.18, 0.575],
            [2.19, 0.557],
            [2.2, 0.541],
            [2.21, 0.525],
            [2.23, 0.508],
            [2.24, 0.491],
            [2.24, 0.473],
            [2.25, 0.453],
            [2.26, 0.431],
            [2.27, 0.409],
            [2.28, 0.39],
            [2.29, 0.372],
            [2.3, 0.353],
            [2.31, 0.335],
            [2.33, 0.318],
            [2.34, 0.3],
            [2.35, 0.282],
            [2.37, 0.264],
            [2.38, 0.244],
            [2.39, 0.223],
            [2.46, 0.17],
            [2.48, 0.197],
            [2.55, 0.186],
            [2.59, 0.246],
            [2.58, 0.267],
            [2.58, 0.29],
            [2.58, 0.315],
            [2.57, 0.341],
            [2.57, 0.366],
            [2.57, 0.391],
            [2.57, 0.414],
            [2.57, 0.434],
            [2.57, 0.458],
            [2.56, 0.475],
            [2.55, 0.492],
            [2.54, 0.514],
            [2.52, 0.598],
            [2.47, 0.646],
            [2.46, 0.713],
            [2.42, 0.766],
            [2.41, 0.784],
            [2.41, 0.803],
            [2.41, 0.822],
            [2.4, 0.839],
            [2.39, 0.861],
            [2.38, 0.877],
            [2.37, 0.894],
            [2.36, 0.913],
            [2.34, 0.929],
            [2.32, 0.946],
            [2.3, 0.964],
            [2.28, 0.983],
            [2.27, 1.0],
            [2.26, 1.02],
            [2.25, 1.03],
            [2.24, 1.05],
            [2.23, 1.06],
            [2.21, 1.09],
            [2.2, 1.1],
            [2.18, 1.11],
            [2.17, 1.13],
            [2.16, 1.14],
            [2.15, 1.15],
            [2.13, 1.17],
            [2.12, 1.18],
            [2.11, 1.19],
            [2.09, 1.21],
            [2.08, 1.22],
            [2.07, 1.23],
            [2.1, 1.28],
            [2.11, 1.29],
            [2.13, 1.3],
            [2.15, 1.3],
            [2.17, 1.31],
            [2.19, 1.31],
            [2.21, 1.32],
            [2.23, 1.31],
            [2.25, 1.31],
            [2.27, 1.3],
            [2.29, 1.3],
            [2.31, 1.29],
            [2.33, 1.29],
            [2.35, 1.28],
            [2.37, 1.27],
            [2.39, 1.26],
            [2.41, 1.24],
            [2.43, 1.23],
            [2.45, 1.22],
            [2.48, 1.21],
            [2.5, 1.19],
            [2.53, 1.17],
            [2.56, 1.15],
            [2.59, 1.13],
            [2.62, 1.11],
            [2.64, 1.1],
            [2.66, 1.08],
            [2.68, 1.06],
            [2.69, 1.04],
            [2.71, 1.02],
            [2.73, 1.0],
            [2.75, 0.987],
            [2.76, 0.971],
            [2.78, 0.957],
            [2.8, 0.942],
        ],
        vec![
            [2.46, 0.315],
            [2.44, 0.346],
            [2.42, 0.375],
            [2.4, 0.404],
            [2.38, 0.433],
            [2.36, 0.463],
            [2.34, 0.493],
            [2.33, 0.509],
            [2.32, 0.531],
            [2.31, 0.547],
            [2.3, 0.562],
            [2.29, 0.582],
            [2.28, 0.602],
            [2.27, 0.618],
            [2.26, 0.639],
            [2.25, 0.66],
            [2.24, 0.682],
            [2.23, 0.704],
            [2.22, 0.726],
            [2.21, 0.747],
            [2.2, 0.768],
            [2.19, 0.788],
            [2.18, 0.806],
            [2.16, 0.825],
            [2.15, 0.845],
            [2.14, 0.866],
            [2.12, 0.908],
            [2.11, 0.924],
            [2.1, 0.943],
            [2.1, 0.965],
            [2.1, 0.986],
            [2.09, 1.01],
            [2.08, 1.03],
            [2.07, 1.05],
            [2.07, 1.06],
            [2.06, 1.08],
            [2.05, 1.1],
            [2.07, 1.09],
            [2.09, 1.08],
            [2.1, 1.07],
            [2.12, 1.05],
            [2.13, 1.03],
            [2.15, 1.02],
            [2.17, 1.01],
            [2.18, 0.995],
            [2.2, 0.976],
            [2.22, 0.956],
            [2.23, 0.936],
            [2.24, 0.916],
            [2.25, 0.899],
            [2.25, 0.882],
            [2.26, 0.86],
            [2.28, 0.842],
            [2.29, 0.824],
            [2.31, 0.805],
            [2.32, 0.785],
            [2.33, 0.764],
            [2.34, 0.745],
            [2.35, 0.726],
            [2.37, 0.707],
            [2.38, 0.689],
            [2.39, 0.606],
            [2.43, 0.551],
            [2.44, 0.501],
            [2.47, 0.455],
            [2.48, 0.381],
            [2.46, 0.315],
        ],
        vec![
            [3.52, 0.938],
            [3.52, 0.958],
            [3.51, 0.976],
            [3.5, 0.994],
            [3.46, 1.05],
            [3.43, 1.13],
            [3.42, 1.14],
            [3.4, 1.15],
            [3.38, 1.16],
            [3.36, 1.17],
            [3.35, 1.19],
            [3.34, 1.2],
            [3.32, 1.22],
            [3.31, 1.23],
            [3.29, 1.24],
            [3.27, 1.25],
            [3.25, 1.26],
            [3.23, 1.27],
            [3.21, 1.28],
            [3.2, 1.29],
            [3.18, 1.31],
            [3.17, 1.32],
            [3.15, 1.33],
            [3.13, 1.34],
            [3.11, 1.34],
            [3.09, 1.35],
            [3.07, 1.35],
            [3.05, 1.36],
            [3.03, 1.36],
            [3.0, 1.37],
            [2.98, 1.38],
            [2.96, 1.38],
            [2.94, 1.39],
            [2.92, 1.39],
            [2.89, 1.39],
            [2.87, 1.39],
            [2.84, 1.39],
            [2.82, 1.4],
            [2.8, 1.39],
            [2.78, 1.38],
            [2.77, 1.37],
            [2.75, 1.36],
            [2.74, 1.35],
            [2.73, 1.33],
            [2.71, 1.32],
            [2.7, 1.3],
            [2.69, 1.28],
            [2.69, 1.26],
            [2.68, 1.24],
            [2.68, 1.19],
            [2.67, 1.16],
            [2.67, 1.14],
            [2.67, 1.11],
            [2.67, 1.09],
            [2.67, 1.07],
            [2.68, 1.05],
            [2.69, 1.03],
            [2.69, 1.01],
            [2.7, 0.986],
            [2.71, 0.965],
            [2.71, 0.944],
            [2.72, 0.924],
            [2.73, 0.903],
            [2.73, 0.883],
            [2.74, 0.862],
            [2.75, 0.842],
            [2.76, 0.822],
            [2.77, 0.802],
            [2.78, 0.783],
            [2.79, 0.763],
            [2.8, 0.744],
            [2.81, 0.725],
            [2.82, 0.703],
            [2.82, 0.682],
            [2.83, 0.662],
            [2.84, 0.643],
            [2.85, 0.623],
            [2.86, 0.607],
            [2.87, 0.591],
            [2.88, 0.575],
            [2.89, 0.557],
            [2.91, 0.541],
            [2.92, 0.525],
            [2.93, 0.508],
            [2.94, 0.491],
            [2.95, 0.473],
            [2.96, 0.453],
            [2.97, 0.431],
            [2.98, 0.409],
            [2.98, 0.39],
            [2.99, 0.372],
            [3.01, 0.353],
            [3.02, 0.335],
            [3.03, 0.318],
            [3.05, 0.3],
            [3.06, 0.282],
            [3.07, 0.264],
            [3.09, 0.244],
            [3.1, 0.223],
            [3.16, 0.17],
            [3.19, 0.197],
            [3.25, 0.186],
            [3.29, 0.246],
            [3.29, 0.267],
            [3.29, 0.29],
            [3.28, 0.315],
            [3.28, 0.341],
            [3.28, 0.366],
            [3.28, 0.391],
            [3.28, 0.414],
            [3.28, 0.434],
            [3.27, 0.458],
            [3.26, 0.475],
            [3.26, 0.492],
            [3.24, 0.514],
            [3.23, 0.598],
            [3.17, 0.646],
            [3.17, 0.713],
            [3.12, 0.766],
            [3.12, 0.784],
            [3.12, 0.803],
            [3.11, 0.822],
            [3.1, 0.839],
            [3.09, 0.861],
            [3.09, 0.877],
            [3.08, 0.894],
            [3.06, 0.913],
            [3.04, 0.929],
            [3.03, 0.946],
            [3.01, 0.964],
            [2.99, 0.983],
            [2.97, 1.0],
            [2.96, 1.02],
            [2.95, 1.03],
            [2.94, 1.05],
            [2.93, 1.06],
            [2.91, 1.09],
            [2.9, 1.1],
            [2.89, 1.11],
            [2.88, 1.13],
            [2.86, 1.14],
            [2.85, 1.15],
            [2.84, 1.17],
            [2.82, 1.18],
            [2.81, 1.19],
            [2.8, 1.21],
            [2.79, 1.22],
            [2.77, 1.23],
            [2.8, 1.28],
            [2.82, 1.29],
            [2.84, 1.3],
            [2.86, 1.3],
            [2.88, 1.31],
            [2.9, 1.31],
            [2.92, 1.32],
            [2.94, 1.31],
            [2.96, 1.31],
            [2.98, 1.3],
            [2.99, 1.3],
            [3.01, 1.29],
            [3.03, 1.29],
            [3.06, 1.28],
            [3.08, 1.27],
            [3.1, 1.26],
            [3.12, 1.24],
            [3.14, 1.23],
            [3.16, 1.22],
            [3.18, 1.21],
            [3.21, 1.19],
            [3.24, 1.17],
            [3.27, 1.15],
            [3.29, 1.13],
            [3.32, 1.11],
            [3.34, 1.1],
            [3.36, 1.08],
            [3.38, 1.06],
            [3.4, 1.04],
            [3.42, 1.02],
            [3.44, 1.0],
            [3.45, 0.987],
            [3.47, 0.971],
            [3.48, 0.957],
            [3.5, 0.942],
        ],
        vec![
            [3.16, 0.315],
            [3.14, 0.346],
            [3.12, 0.375],
            [3.1, 0.404],
            [3.08, 0.433],
            [3.06, 0.463],
            [3.05, 0.493],
            [3.04, 0.509],
            [3.03, 0.531],
            [3.02, 0.547],
            [3.01, 0.562],
            [3.0, 0.582],
            [2.98, 0.602],
            [2.97, 0.618],
            [2.96, 0.639],
            [2.95, 0.66],
            [2.95, 0.682],
            [2.94, 0.704],
            [2.93, 0.726],
            [2.92, 0.747],
            [2.91, 0.768],
            [2.9, 0.788],
            [2.88, 0.806],
            [2.87, 0.825],
            [2.86, 0.845],
            [2.84, 0.866],
            [2.82, 0.908],
            [2.81, 0.924],
            [2.81, 0.943],
            [2.81, 0.965],
            [2.8, 0.986],
            [2.8, 1.01],
            [2.79, 1.03],
            [2.78, 1.05],
            [2.77, 1.06],
            [2.77, 1.08],
            [2.76, 1.1],
            [2.78, 1.09],
            [2.79, 1.08],
            [2.81, 1.07],
            [2.82, 1.05],
            [2.84, 1.03],
            [2.85, 1.02],
            [2.87, 1.01],
            [2.89, 0.995],
            [2.91, 0.976],
            [2.92, 0.956],
            [2.93, 0.936],
            [2.95, 0.916],
            [2.95, 0.899],
            [2.96, 0.882],
            [2.97, 0.86],
            [2.98, 0.842],
            [3.0, 0.824],
            [3.01, 0.805],
            [3.02, 0.785],
            [3.03, 0.764],
            [3.05, 0.745],
            [3.06, 0.726],
            [3.07, 0.707],
            [3.09, 0.689],
            [3.09, 0.606],
            [3.14, 0.551],
            [3.15, 0.501],
            [3.18, 0.455],
            [3.18, 0.381],
            [3.16, 0.315],
        ],
        vec![
            [4.25, 0.957],
            [4.25, 0.975],
            [4.24, 0.995],
            [4.22, 1.01],
            [4.21, 1.02],
            [4.19, 1.03],
            [4.17, 1.04],
            [4.15, 1.05],
            [4.13, 1.06],
            [4.12, 1.06],
            [4.1, 1.06],
            [4.06, 1.07],
            [4.03, 1.07],
            [4.0, 1.08],
            [3.98, 1.08],
            [3.96, 1.08],
            [3.94, 1.09],
            [3.92, 1.09],
            [3.9, 1.08],
            [3.88, 1.08],
            [3.86, 1.08],
            [3.84, 1.09],
            [3.81, 1.09],
            [3.8, 1.11],
            [3.79, 1.13],
            [3.79, 1.15],
            [3.78, 1.18],
            [3.77, 1.2],
            [3.76, 1.21],
            [3.74, 1.22],
            [3.75, 1.24],
            [3.73, 1.25],
            [3.72, 1.26],
            [3.71, 1.28],
            [3.71, 1.3],
            [3.7, 1.32],
            [3.68, 1.33],
            [3.66, 1.33],
            [3.64, 1.35],
            [3.63, 1.36],
            [3.61, 1.37],
            [3.59, 1.38],
            [3.58, 1.39],
            [3.56, 1.4],
            [3.54, 1.41],
            [3.52, 1.42],
            [3.5, 1.43],
            [3.48, 1.43],
            [3.46, 1.44],
            [3.44, 1.44],
            [3.43, 1.43],
            [3.41, 1.42],
            [3.39, 1.41],
            [3.37, 1.4],
            [3.36, 1.39],
            [3.35, 1.37],
            [3.34, 1.34],
            [3.33, 1.32],
            [3.32, 1.3],
            [3.32, 1.27],
            [3.33, 1.25],
            [3.34, 1.23],
            [3.35, 1.21],
            [3.35, 1.19],
            [3.36, 1.17],
            [3.36, 1.15],
            [3.36, 1.13],
            [3.37, 1.11],
            [3.38, 1.1],
            [3.38, 1.08],
            [3.39, 1.06],
            [3.4, 1.05],
            [3.4, 1.02],
            [3.41, 1.0],
            [3.43, 0.99],
            [3.45, 0.978],
            [3.47, 0.965],
            [3.48, 0.95],
            [3.49, 0.929],
            [3.49, 0.909],
            [3.51, 0.899],
            [3.53, 0.89],
            [3.54, 0.882],
            [3.56, 0.87],
            [3.56, 0.851],
            [3.56, 0.832],
            [3.57, 0.814],
            [3.59, 0.808],
            [3.61, 0.804],
            [3.63, 0.797],
            [3.65, 0.786],
            [3.66, 0.776],
            [3.68, 0.766],
            [3.7, 0.756],
            [3.71, 0.749],
            [3.74, 0.742],
            [3.76, 0.744],
            [3.78, 0.752],
            [3.79, 0.766],
            [3.8, 0.786],
            [3.8, 0.804],
            [3.81, 0.822],
            [3.81, 0.84],
            [3.82, 0.858],
            [3.83, 0.877],
            [3.83, 0.895],
            [3.83, 0.914],
            [3.83, 0.935],
            [3.83, 0.956],
            [3.83, 0.975],
            [3.84, 0.995],
            [3.86, 1.01],
            [3.89, 1.01],
            [3.92, 1.02],
            [3.94, 1.01],
            [3.97, 1.0],
            [3.99, 1.0],
            [4.01, 1.0],
            [4.02, 0.996],
            [4.04, 0.986],
            [4.06, 0.983],
            [4.08, 0.997],
            [4.1, 0.99],
            [4.12, 0.982],
            [4.14, 0.979],
            [4.16, 0.977],
            [4.18, 0.976],
            [4.21, 0.974],
            [4.23, 0.968],
            [4.25, 0.957],
        ],
        vec![
            [3.74, 0.979],
            [3.74, 0.959],
            [3.75, 0.937],
            [3.75, 0.915],
            [3.76, 0.893],
            [3.76, 0.871],
            [3.76, 0.851],
            [3.75, 0.832],
            [3.73, 0.816],
            [3.71, 0.829],
            [3.7, 0.847],
            [3.69, 0.865],
            [3.67, 0.881],
            [3.66, 0.893],
            [3.63, 0.893],
            [3.63, 0.912],
            [3.64, 0.929],
            [3.66, 0.943],
            [3.67, 0.958],
            [3.67, 0.979],
            [3.69, 0.987],
            [3.71, 0.99],
            [3.73, 0.987],
        ],
        vec![
            [3.73, 1.08],
            [3.71, 1.08],
            [3.68, 1.07],
            [3.67, 1.06],
            [3.65, 1.04],
            [3.63, 1.03],
            [3.61, 1.01],
            [3.58, 1.0],
            [3.56, 0.997],
            [3.55, 1.01],
            [3.53, 1.03],
            [3.52, 1.04],
            [3.5, 1.06],
            [3.49, 1.07],
            [3.49, 1.09],
            [3.49, 1.11],
            [3.49, 1.13],
            [3.47, 1.14],
            [3.46, 1.16],
            [3.45, 1.17],
            [3.45, 1.19],
            [3.44, 1.21],
            [3.44, 1.23],
            [3.44, 1.25],
            [3.43, 1.28],
            [3.43, 1.3],
            [3.43, 1.32],
            [3.42, 1.34],
            [3.41, 1.35],
            [3.44, 1.33],
            [3.45, 1.35],
            [3.47, 1.34],
            [3.49, 1.33],
            [3.51, 1.33],
            [3.53, 1.32],
            [3.54, 1.32],
            [3.57, 1.32],
            [3.59, 1.32],
            [3.61, 1.32],
            [3.62, 1.31],
            [3.63, 1.29],
            [3.64, 1.27],
            [3.65, 1.25],
            [3.66, 1.23],
            [3.68, 1.22],
            [3.68, 1.2],
            [3.7, 1.18],
            [3.71, 1.16],
            [3.72, 1.14],
            [3.73, 1.12],
            [3.73, 1.1],
        ],
        vec![
            [5.48, 0.418],
            [5.46, 0.433],
            [5.44, 0.438],
            [5.42, 0.438],
            [5.4, 0.435],
            [5.37, 0.432],
            [5.35, 0.432],
            [5.33, 0.438],
            [5.32, 0.454],
            [5.3, 0.45],
            [5.28, 0.452],
            [5.25, 0.456],
            [5.23, 0.461],
            [5.21, 0.467],
            [5.18, 0.472],
            [5.16, 0.476],
            [5.13, 0.476],
            [5.11, 0.454],
            [5.1, 0.468],
            [5.08, 0.472],
            [5.05, 0.464],
            [5.04, 0.474],
            [5.02, 0.48],
            [5.0, 0.481],
            [4.98, 0.48],
            [4.96, 0.48],
            [4.94, 0.479],
            [4.91, 0.484],
            [4.89, 0.494],
            [4.87, 0.509],
            [4.86, 0.531],
            [4.85, 0.555],
            [4.84, 0.575],
            [4.84, 0.595],
            [4.83, 0.616],
            [4.82, 0.636],
            [4.82, 0.657],
            [4.8, 0.693],
            [4.79, 0.719],
            [4.78, 0.745],
            [4.77, 0.771],
            [4.76, 0.797],
            [4.75, 0.823],
            [4.74, 0.849],
            [4.73, 0.874],
            [4.72, 0.899],
            [4.71, 0.925],
            [4.7, 0.95],
            [4.69, 0.975],
            [4.69, 0.995],
            [4.69, 1.01],
            [4.69, 1.03],
            [4.69, 1.05],
            [4.69, 1.07],
            [4.69, 1.09],
            [4.69, 1.11],
            [4.69, 1.14],
            [4.7, 1.16],
            [4.7, 1.17],
            [4.7, 1.19],
            [4.7, 1.21],
            [4.7, 1.24],
            [4.7, 1.26],
            [4.71, 1.28],
            [4.71, 1.3],
            [4.73, 1.32],
            [4.74, 1.34],
            [4.77, 1.32],
            [4.8, 1.36],
            [4.81, 1.35],
            [4.83, 1.33],
            [4.85, 1.31],
            [4.87, 1.29],
            [4.89, 1.28],
            [4.91, 1.27],
            [4.93, 1.26],
            [4.95, 1.25],
            [4.97, 1.23],
            [4.98, 1.22],
            [5.0, 1.21],
            [5.02, 1.2],
            [5.04, 1.2],
            [5.06, 1.19],
            [5.09, 1.17],
            [5.11, 1.14],
            [5.13, 1.12],
            [5.15, 1.1],
            [5.18, 1.07],
            [5.2, 1.05],
            [5.22, 1.03],
            [5.25, 1.01],
            [5.27, 0.987],
            [5.3, 0.973],
            [5.33, 0.963],
            [5.32, 0.988],
            [5.31, 1.01],
            [5.29, 1.04],
            [5.28, 1.06],
            [5.27, 1.09],
            [5.25, 1.11],
            [5.23, 1.13],
            [5.22, 1.16],
            [5.2, 1.18],
            [5.18, 1.2],
            [5.16, 1.22],
            [5.14, 1.24],
            [5.12, 1.26],
            [5.1, 1.27],
            [5.08, 1.28],
            [5.07, 1.29],
            [5.04, 1.31],
            [5.02, 1.32],
            [5.0, 1.34],
            [4.98, 1.36],
            [4.96, 1.38],
            [4.94, 1.4],
            [4.92, 1.4],
            [4.9, 1.4],
            [4.88, 1.4],
            [4.86, 1.4],
            [4.84, 1.42],
            [4.81, 1.42],
            [4.79, 1.43],
            [4.76, 1.44],
            [4.74, 1.43],
            [4.72, 1.43],
            [4.71, 1.42],
            [4.69, 1.41],
            [4.66, 1.4],
            [4.65, 1.39],
            [4.64, 1.37],
            [4.62, 1.36],
            [4.61, 1.34],
            [4.6, 1.33],
            [4.6, 1.31],
            [4.59, 1.29],
            [4.58, 1.27],
            [4.58, 1.25],
            [4.58, 1.23],
            [4.58, 1.21],
            [4.58, 1.19],
            [4.59, 1.18],
            [4.59, 1.16],
            [4.59, 1.13],
            [4.58, 1.11],
            [4.58, 1.09],
            [4.58, 1.08],
            [4.58, 1.06],
            [4.58, 1.04],
            [4.59, 1.0],
            [4.6, 0.976],
            [4.6, 0.949],
            [4.61, 0.922],
            [4.62, 0.895],
            [4.62, 0.868],
            [4.63, 0.835],
            [4.64, 0.802],
            [4.65, 0.769],
            [4.66, 0.736],
            [4.67, 0.703],
            [4.68, 0.67],
            [4.7, 0.638],
            [4.71, 0.605],
            [4.72, 0.573],
            [4.73, 0.54],
            [4.74, 0.508],
            [4.76, 0.476],
            [4.74, 0.47],
            [4.71, 0.466],
            [4.69, 0.465],
            [4.67, 0.464],
            [4.65, 0.464],
            [4.63, 0.463],
            [4.61, 0.46],
            [4.59, 0.454],
            [4.57, 0.448],
            [4.55, 0.45],
            [4.53, 0.452],
            [4.51, 0.451],
            [4.49, 0.441],
            [4.49, 0.418],
            [4.5, 0.399],
            [4.52, 0.388],
            [4.54, 0.385],
            [4.56, 0.383],
            [4.57, 0.376],
            [4.6, 0.381],
            [4.62, 0.384],
            [4.64, 0.387],
            [4.66, 0.388],
            [4.69, 0.388],
            [4.71, 0.386],
            [4.73, 0.383],
            [4.75, 0.377],
            [4.77, 0.369],
            [4.79, 0.359],
            [4.81, 0.346],
            [4.83, 0.33],
            [4.84, 0.302],
            [4.85, 0.273],
            [4.86, 0.242],
            [4.87, 0.211],
            [4.87, 0.18],
            [4.88, 0.15],
            [4.89, 0.12],
            [4.9, 0.0924],
            [4.92, 0.0658],
            [4.94, 0.0413],
            [4.96, 0.0195],
            [4.99, 0.000585],
            [5.01, 0.000585],
            [5.03, 0.00852],
            [5.05, 0.0199],
            [5.05, 0.0417],
            [5.04, 0.0612],
            [5.04, 0.0809],
            [5.04, 0.101],
            [5.03, 0.12],
            [5.02, 0.139],
            [5.02, 0.157],
            [5.01, 0.173],
            [5.02, 0.183],
            [5.02, 0.202],
            [5.02, 0.219],
            [5.01, 0.235],
            [5.0, 0.252],
            [4.98, 0.261],
            [4.97, 0.276],
            [4.96, 0.294],
            [4.95, 0.311],
            [4.96, 0.33],
            [4.97, 0.345],
            [4.98, 0.36],
            [5.0, 0.367],
            [5.02, 0.363],
            [5.04, 0.356],
            [5.06, 0.357],
            [5.08, 0.355],
            [5.1, 0.354],
            [5.12, 0.359],
            [5.14, 0.363],
            [5.16, 0.358],
            [5.17, 0.343],
            [5.19, 0.338],
            [5.21, 0.342],
            [5.23, 0.349],
            [5.25, 0.357],
            [5.26, 0.358],
            [5.29, 0.355],
            [5.31, 0.355],
            [5.33, 0.354],
            [5.36, 0.351],
            [5.38, 0.347],
            [5.41, 0.342],
            [5.43, 0.336],
            [5.45, 0.331],
            [5.47, 0.337],
            [5.48, 0.351],
            [5.48, 0.369],
            [5.48, 0.39],
            [5.48, 0.412],
        ],
        vec![
            [6.01, 1.21],
            [6.0, 1.23],
            [5.99, 1.25],
            [5.98, 1.27],
            [5.96, 1.29],
            [5.95, 1.32],
            [5.93, 1.34],
            [5.91, 1.36],
            [5.89, 1.37],
            [5.86, 1.39],
            [5.84, 1.4],
            [5.81, 1.41],
            [5.78, 1.43],
            [5.76, 1.44],
            [5.74, 1.45],
            [5.71, 1.46],
            [5.69, 1.45],
            [5.66, 1.45],
            [5.64, 1.46],
            [5.62, 1.46],
            [5.59, 1.46],
            [5.57, 1.45],
            [5.55, 1.44],
            [5.53, 1.42],
            [5.51, 1.42],
            [5.51, 1.4],
            [5.51, 1.38],
            [5.49, 1.36],
            [5.5, 1.34],
            [5.5, 1.33],
            [5.51, 1.31],
            [5.51, 1.29],
            [5.51, 1.27],
            [5.52, 1.25],
            [5.53, 1.23],
            [5.52, 1.21],
            [5.52, 1.19],
            [5.53, 1.17],
            [5.54, 1.14],
            [5.55, 1.12],
            [5.56, 1.1],
            [5.57, 1.08],
            [5.57, 1.06],
            [5.56, 1.04],
            [5.54, 1.03],
            [5.52, 1.03],
            [5.5, 1.05],
            [5.48, 1.07],
            [5.45, 1.09],
            [5.43, 1.11],
            [5.41, 1.13],
            [5.38, 1.16],
            [5.36, 1.18],
            [5.34, 1.2],
            [5.32, 1.23],
            [5.3, 1.25],
            [5.28, 1.28],
            [5.26, 1.31],
            [5.24, 1.31],
            [5.23, 1.33],
            [5.22, 1.34],
            [5.21, 1.36],
            [5.2, 1.38],
            [5.19, 1.4],
            [5.18, 1.41],
            [5.16, 1.42],
            [5.15, 1.44],
            [5.14, 1.46],
            [5.14, 1.49],
            [5.13, 1.51],
            [5.11, 1.5],
            [5.09, 1.5],
            [5.07, 1.49],
            [5.06, 1.47],
            [5.05, 1.45],
            [5.05, 1.43],
            [5.06, 1.41],
            [5.07, 1.39],
            [5.08, 1.37],
            [5.09, 1.35],
            [5.1, 1.32],
            [5.11, 1.3],
            [5.12, 1.28],
            [5.14, 1.26],
            [5.15, 1.24],
            [5.15, 1.22],
            [5.16, 1.19],
            [5.17, 1.18],
            [5.19, 1.17],
            [5.19, 1.14],
            [5.2, 1.12],
            [5.21, 1.11],
            [5.22, 1.09],
            [5.22, 1.07],
            [5.22, 1.05],
            [5.23, 1.03],
            [5.24, 1.02],
            [5.25, 0.997],
            [5.26, 0.981],
            [5.26, 0.959],
            [5.27, 0.94],
            [5.27, 0.922],
            [5.28, 0.906],
            [5.29, 0.892],
            [5.31, 0.874],
            [5.32, 0.855],
            [5.33, 0.839],
            [5.34, 0.822],
            [5.35, 0.805],
            [5.35, 0.787],
            [5.36, 0.768],
            [5.36, 0.749],
            [5.37, 0.73],
            [5.37, 0.714],
            [5.39, 0.695],
            [5.41, 0.684],
            [5.41, 0.666],
            [5.42, 0.644],
            [5.42, 0.627],
            [5.43, 0.61],
            [5.44, 0.588],
            [5.44, 0.57],
            [5.46, 0.557],
            [5.47, 0.543],
            [5.48, 0.524],
            [5.48, 0.503],
            [5.47, 0.485],
            [5.48, 0.463],
            [5.48, 0.443],
            [5.49, 0.422],
            [5.51, 0.407],
            [5.52, 0.397],
            [5.54, 0.387],
            [5.55, 0.373],
            [5.55, 0.351],
            [5.56, 0.332],
            [5.57, 0.317],
            [5.58, 0.3],
            [5.59, 0.281],
            [5.6, 0.26],
            [5.61, 0.24],
            [5.62, 0.221],
            [5.64, 0.205],
            [5.65, 0.191],
            [5.66, 0.169],
            [5.67, 0.149],
            [5.68, 0.131],
            [5.7, 0.116],
            [5.71, 0.105],
            [5.73, 0.0952],
            [5.75, 0.0817],
            [5.76, 0.0677],
            [5.78, 0.0618],
            [5.81, 0.0568],
            [5.83, 0.0544],
            [5.85, 0.0559],
            [5.87, 0.0635],
            [5.89, 0.0756],
            [5.9, 0.0905],
            [5.91, 0.104],
            [5.92, 0.123],
            [5.92, 0.141],
            [5.92, 0.162],
            [5.92, 0.18],
            [5.93, 0.198],
            [5.93, 0.224],
            [5.93, 0.252],
            [5.93, 0.281],
            [5.93, 0.31],
            [5.93, 0.339],
            [5.93, 0.369],
            [5.93, 0.397],
            [5.92, 0.425],
            [5.92, 0.452],
            [5.91, 0.478],
            [5.89, 0.501],
            [5.88, 0.579],
            [5.86, 0.584],
            [5.85, 0.601],
            [5.84, 0.622],
            [5.84, 0.641],
            [5.83, 0.661],
            [5.82, 0.678],
            [5.81, 0.695],
            [5.8, 0.711],
            [5.79, 0.726],
            [5.77, 0.739],
            [5.76, 0.75],
            [5.74, 0.759],
            [5.72, 0.769],
            [5.71, 0.786],
            [5.7, 0.799],
            [5.68, 0.811],
            [5.67, 0.822],
            [5.65, 0.83],
            [5.63, 0.835],
            [5.61, 0.846],
            [5.6, 0.858],
            [5.58, 0.871],
            [5.56, 0.881],
            [5.54, 0.887],
            [5.52, 0.887],
            [5.5, 0.896],
            [5.48, 0.908],
            [5.46, 0.913],
            [5.44, 0.916],
            [5.42, 0.918],
            [5.4, 0.923],
            [5.38, 0.932],
            [5.37, 0.95],
            [5.36, 0.969],
            [5.36, 0.991],
            [5.36, 1.01],
            [5.36, 1.03],
            [5.37, 1.05],
            [5.4, 1.03],
            [5.43, 1.01],
            [5.46, 0.994],
            [5.49, 0.975],
            [5.52, 0.957],
            [5.55, 0.942],
            [5.57, 0.931],
            [5.59, 0.923],
            [5.61, 0.923],
            [5.63, 0.924],
            [5.65, 0.922],
            [5.67, 0.927],
            [5.68, 0.942],
            [5.69, 0.961],
            [5.7, 0.98],
            [5.68, 0.998],
            [5.67, 1.02],
            [5.66, 1.04],
            [5.66, 1.06],
            [5.65, 1.09],
            [5.64, 1.11],
            [5.63, 1.13],
            [5.62, 1.15],
            [5.63, 1.17],
            [5.62, 1.19],
            [5.63, 1.21],
            [5.62, 1.23],
            [5.62, 1.25],
            [5.61, 1.27],
            [5.6, 1.29],
            [5.59, 1.31],
            [5.6, 1.32],
            [5.6, 1.34],
            [5.61, 1.37],
            [5.63, 1.38],
            [5.65, 1.38],
            [5.67, 1.38],
            [5.69, 1.37],
            [5.71, 1.37],
            [5.73, 1.37],
            [5.75, 1.37],
            [5.76, 1.36],
            [5.78, 1.35],
            [5.8, 1.34],
            [5.82, 1.33],
            [5.84, 1.32],
            [5.86, 1.31],
            [5.87, 1.3],
            [5.89, 1.28],
            [5.91, 1.27],
            [5.93, 1.26],
            [5.95, 1.25],
            [5.97, 1.24],
            [5.99, 1.22],
            [6.01, 1.21],
        ],
        vec![
            [5.81, 0.138],
            [5.81, 0.155],
            [5.79, 0.17],
            [5.78, 0.183],
            [5.76, 0.195],
            [5.75, 0.214],
            [5.74, 0.227],
            [5.73, 0.244],
            [5.72, 0.262],
            [5.72, 0.28],
            [5.71, 0.297],
            [5.69, 0.31],
            [5.68, 0.333],
            [5.67, 0.347],
            [5.66, 0.362],
            [5.65, 0.378],
            [5.64, 0.395],
            [5.63, 0.413],
            [5.62, 0.431],
            [5.61, 0.449],
            [5.6, 0.467],
            [5.59, 0.485],
            [5.59, 0.503],
            [5.58, 0.521],
            [5.56, 0.542],
            [5.55, 0.565],
            [5.55, 0.589],
            [5.54, 0.614],
            [5.53, 0.637],
            [5.52, 0.66],
            [5.51, 0.679],
            [5.48, 0.693],
            [5.49, 0.71],
            [5.49, 0.73],
            [5.48, 0.748],
            [5.47, 0.766],
            [5.46, 0.783],
            [5.45, 0.803],
            [5.47, 0.81],
            [5.5, 0.806],
            [5.52, 0.798],
            [5.54, 0.787],
            [5.57, 0.774],
            [5.59, 0.759],
            [5.61, 0.744],
            [5.63, 0.731],
            [5.66, 0.729],
            [5.68, 0.717],
            [5.69, 0.704],
            [5.71, 0.691],
            [5.72, 0.676],
            [5.74, 0.661],
            [5.75, 0.645],
            [5.76, 0.629],
            [5.77, 0.612],
            [5.79, 0.595],
            [5.8, 0.577],
            [5.81, 0.559],
            [5.8, 0.543],
            [5.81, 0.523],
            [5.82, 0.495],
            [5.83, 0.469],
            [5.83, 0.445],
            [5.84, 0.41],
            [5.84, 0.386],
            [5.85, 0.363],
            [5.85, 0.339],
            [5.86, 0.315],
            [5.86, 0.291],
            [5.86, 0.267],
            [5.86, 0.243],
            [5.86, 0.219],
            [5.87, 0.195],
            [5.87, 0.171],
            [5.87, 0.147],
            [5.81, 0.138],
        ],
        vec![
            [6.93, 0.967],
            [6.92, 0.981],
            [6.9, 0.996],
            [6.89, 1.01],
            [6.88, 1.03],
            [6.86, 1.04],
            [6.85, 1.06],
            [6.84, 1.07],
            [6.81, 1.07],
            [6.8, 1.09],
            [6.79, 1.1],
            [6.77, 1.12],
            [6.75, 1.13],
            [6.73, 1.14],
            [6.71, 1.15],
            [6.69, 1.16],
            [6.67, 1.17],
            [6.65, 1.18],
            [6.63, 1.18],
            [6.49, 1.24],
            [6.48, 1.25],
            [6.46, 1.26],
            [6.44, 1.27],
            [6.42, 1.28],
            [6.4, 1.29],
            [6.37, 1.3],
            [6.35, 1.31],
            [6.33, 1.32],
            [6.33, 1.34],
            [6.32, 1.36],
            [6.3, 1.37],
            [6.28, 1.37],
            [6.26, 1.37],
            [6.24, 1.38],
            [6.22, 1.38],
            [6.2, 1.38],
            [6.18, 1.38],
            [6.16, 1.38],
            [6.13, 1.39],
            [6.11, 1.39],
            [6.09, 1.38],
            [6.07, 1.39],
            [6.05, 1.39],
            [6.03, 1.38],
            [6.01, 1.37],
            [6.0, 1.35],
            [5.99, 1.34],
            [5.98, 1.32],
            [5.96, 1.3],
            [5.95, 1.28],
            [5.94, 1.26],
            [5.93, 1.24],
            [5.92, 1.22],
            [5.92, 1.2],
            [5.91, 1.17],
            [5.92, 1.15],
            [5.93, 1.14],
            [5.94, 1.12],
            [5.94, 1.1],
            [5.95, 1.08],
            [5.95, 1.06],
            [5.95, 1.04],
            [5.97, 1.03],
            [5.98, 1.02],
            [5.99, 1.01],
            [5.99, 0.991],
            [5.99, 0.972],
            [6.01, 0.957],
            [6.02, 0.941],
            [6.02, 0.923],
            [6.02, 0.899],
            [6.04, 0.882],
            [6.05, 0.866],
            [6.06, 0.851],
            [6.08, 0.837],
            [6.09, 0.824],
            [6.11, 0.811],
            [6.12, 0.799],
            [6.14, 0.786],
            [6.16, 0.774],
            [6.17, 0.761],
            [6.19, 0.748],
            [6.21, 0.745],
            [6.23, 0.744],
            [6.25, 0.733],
            [6.26, 0.718],
            [6.28, 0.703],
            [6.3, 0.694],
            [6.32, 0.704],
            [6.35, 0.687],
            [6.37, 0.677],
            [6.39, 0.675],
            [6.42, 0.679],
            [6.44, 0.687],
            [6.46, 0.698],
            [6.47, 0.712],
            [6.48, 0.727],
            [6.49, 0.752],
            [6.49, 0.771],
            [6.48, 0.789],
            [6.49, 0.807],
            [6.49, 0.826],
            [6.48, 0.847],
            [6.47, 0.863],
            [6.46, 0.88],
            [6.45, 0.896],
            [6.44, 0.914],
            [6.43, 0.934],
            [6.42, 0.953],
            [6.41, 0.972],
            [6.4, 0.989],
            [6.38, 1.0],
            [6.37, 1.02],
            [6.35, 1.02],
            [6.33, 1.03],
            [6.32, 1.04],
            [6.32, 1.06],
            [6.3, 1.07],
            [6.28, 1.08],
            [6.27, 1.09],
            [6.25, 1.11],
            [6.23, 1.1],
            [6.21, 1.1],
            [6.2, 1.12],
            [6.19, 1.14],
            [6.18, 1.15],
            [6.16, 1.17],
            [6.15, 1.17],
            [6.13, 1.18],
            [6.11, 1.19],
            [6.09, 1.2],
            [6.07, 1.21],
            [6.05, 1.22],
            [6.03, 1.23],
            [6.02, 1.25],
            [6.03, 1.26],
            [6.05, 1.27],
            [6.07, 1.27],
            [6.09, 1.27],
            [6.11, 1.27],
            [6.13, 1.27],
            [6.15, 1.27],
            [6.17, 1.28],
            [6.19, 1.27],
            [6.21, 1.27],
            [6.23, 1.26],
            [6.25, 1.25],
            [6.27, 1.25],
            [6.29, 1.24],
            [6.31, 1.23],
            [6.33, 1.22],
            [6.35, 1.21],
            [6.37, 1.21],
            [6.39, 1.2],
            [6.41, 1.2],
            [6.43, 1.21],
            [6.44, 1.21],
            [6.46, 1.19],
            [6.46, 1.18],
            [6.48, 1.16],
            [6.49, 1.15],
            [6.51, 1.14],
            [6.53, 1.13],
            [6.55, 1.13],
            [6.58, 1.13],
            [6.59, 1.12],
            [6.6, 1.1],
            [6.62, 1.09],
            [6.64, 1.09],
            [6.66, 1.09],
            [6.68, 1.09],
            [6.7, 1.08],
            [6.71, 1.07],
            [6.73, 1.05],
            [6.75, 1.06],
            [6.76, 1.04],
            [6.78, 1.03],
            [6.79, 1.01],
            [6.81, 1.0],
            [6.83, 0.99],
            [6.85, 0.978],
            [6.87, 0.966],
            [6.89, 0.954],
            [6.91, 0.941],
            [6.92, 0.928],
            [6.93, 0.945],
            [6.93, 0.964],
        ],
        vec![
            [6.38, 0.802],
            [6.36, 0.792],
            [6.34, 0.786],
            [6.32, 0.787],
            [6.3, 0.797],
            [6.29, 0.808],
            [6.27, 0.822],
            [6.26, 0.836],
            [6.25, 0.849],
            [6.23, 0.86],
            [6.21, 0.868],
            [6.19, 0.871],
            [6.17, 0.867],
            [6.17, 0.888],
            [6.16, 0.908],
            [6.16, 0.929],
            [6.16, 0.95],
            [6.14, 0.96],
            [6.12, 0.972],
            [6.11, 0.988],
            [6.1, 1.0],
            [6.09, 1.02],
            [6.09, 1.04],
            [6.08, 1.06],
            [6.07, 1.08],
            [6.08, 1.09],
            [6.1, 1.08],
            [6.12, 1.07],
            [6.14, 1.06],
            [6.16, 1.05],
            [6.17, 1.04],
            [6.19, 1.03],
            [6.21, 1.02],
            [6.22, 1.01],
            [6.24, 1.0],
            [6.26, 0.983],
            [6.28, 0.964],
            [6.3, 0.951],
            [6.32, 0.932],
            [6.34, 0.909],
            [6.35, 0.885],
            [6.36, 0.86],
            [6.37, 0.841],
            [6.38, 0.824],
            [6.38, 0.805],
        ],
        vec![
            [7.69, 1.11],
            [7.67, 1.12],
            [7.66, 1.13],
            [7.65, 1.15],
            [7.64, 1.17],
            [7.63, 1.2],
            [7.62, 1.22],
            [7.61, 1.24],
            [7.6, 1.26],
            [7.58, 1.26],
            [7.57, 1.28],
            [7.56, 1.3],
            [7.54, 1.31],
            [7.52, 1.31],
            [7.5, 1.32],
            [7.48, 1.33],
            [7.47, 1.35],
            [7.46, 1.37],
            [7.44, 1.38],
            [7.43, 1.39],
            [7.41, 1.4],
            [7.38, 1.41],
            [7.36, 1.41],
            [7.34, 1.42],
            [7.32, 1.42],
            [7.3, 1.42],
            [7.27, 1.42],
            [7.25, 1.42],
            [7.23, 1.42],
            [7.21, 1.41],
            [7.19, 1.4],
            [7.17, 1.39],
            [7.16, 1.37],
            [7.14, 1.36],
            [7.13, 1.34],
            [7.12, 1.32],
            [7.11, 1.3],
            [7.1, 1.28],
            [7.1, 1.25],
            [7.09, 1.23],
            [7.09, 1.21],
            [7.14, 1.06],
            [7.11, 1.04],
            [7.09, 1.02],
            [7.08, 0.991],
            [7.06, 0.965],
            [7.04, 0.941],
            [7.02, 0.917],
            [7.0, 0.931],
            [6.99, 0.945],
            [7.01, 0.966],
            [6.99, 0.982],
            [6.98, 0.999],
            [6.96, 1.02],
            [6.95, 1.03],
            [6.93, 1.05],
            [6.92, 1.07],
            [6.9, 1.09],
            [6.89, 1.11],
            [6.88, 1.13],
            [6.87, 1.15],
            [6.86, 1.17],
            [6.85, 1.19],
            [6.82, 1.2],
            [6.81, 1.21],
            [6.8, 1.22],
            [6.79, 1.24],
            [6.78, 1.26],
            [6.77, 1.28],
            [6.75, 1.29],
            [6.73, 1.29],
            [6.72, 1.31],
            [6.71, 1.33],
            [6.7, 1.35],
            [6.69, 1.37],
            [6.67, 1.38],
            [6.64, 1.39],
            [6.63, 1.37],
            [6.63, 1.35],
            [6.64, 1.33],
            [6.65, 1.31],
            [6.66, 1.29],
            [6.66, 1.27],
            [6.68, 1.24],
            [6.69, 1.22],
            [6.71, 1.2],
            [6.73, 1.18],
            [6.75, 1.16],
            [6.77, 1.14],
            [6.78, 1.12],
            [6.8, 1.09],
            [6.81, 1.07],
            [6.81, 1.04],
            [6.81, 1.01],
            [6.83, 1.01],
            [6.85, 1.0],
            [6.87, 0.996],
            [6.88, 0.987],
            [6.9, 0.97],
            [6.91, 0.948],
            [6.91, 0.929],
            [6.9, 0.914],
            [6.9, 0.893],
            [6.92, 0.886],
            [6.93, 0.875],
            [6.94, 0.861],
            [6.96, 0.845],
            [6.97, 0.829],
            [6.98, 0.812],
            [6.99, 0.794],
            [7.0, 0.776],
            [7.0, 0.758],
            [7.01, 0.741],
            [7.03, 0.726],
            [7.04, 0.712],
            [7.06, 0.721],
            [7.08, 0.729],
            [7.1, 0.738],
            [7.11, 0.749],
            [7.1, 0.764],
            [7.1, 0.784],
            [7.1, 0.803],
            [7.1, 0.823],
            [7.1, 0.843],
            [7.09, 0.863],
            [7.1, 0.881],
            [7.11, 0.896],
            [7.12, 0.912],
            [7.13, 0.928],
            [7.15, 0.942],
            [7.16, 0.954],
            [7.18, 0.945],
            [7.2, 0.941],
            [7.22, 0.941],
            [7.24, 0.941],
            [7.26, 0.941],
            [7.28, 0.939],
            [7.29, 0.932],
            [7.31, 0.917],
            [7.32, 0.922],
            [7.35, 0.922],
            [7.36, 0.927],
            [7.37, 0.944],
            [7.37, 0.963],
            [7.36, 0.983],
            [7.35, 1.0],
            [7.34, 1.01],
            [7.32, 1.03],
            [7.31, 1.04],
            [7.29, 1.05],
            [7.28, 1.06],
            [7.27, 1.08],
            [7.27, 1.1],
            [7.25, 1.11],
            [7.23, 1.12],
            [7.22, 1.14],
            [7.21, 1.17],
            [7.21, 1.19],
            [7.21, 1.22],
            [7.21, 1.24],
            [7.2, 1.27],
            [7.21, 1.29],
            [7.23, 1.29],
            [7.25, 1.3],
            [7.27, 1.31],
            [7.3, 1.31],
            [7.32, 1.31],
            [7.33, 1.3],
            [7.35, 1.28],
            [7.37, 1.27],
            [7.39, 1.26],
            [7.41, 1.25],
            [7.43, 1.25],
            [7.45, 1.24],
            [7.47, 1.23],
            [7.49, 1.22],
            [7.5, 1.21],
            [7.52, 1.19],
            [7.55, 1.17],
            [7.57, 1.14],
            [7.6, 1.12],
            [7.63, 1.1],
            [7.65, 1.07],
            [7.68, 1.05],
            [7.69, 1.07],
            [7.69, 1.08],
            [7.69, 1.1],
        ],
        vec![
            [8.6, 0.967],
            [8.59, 0.981],
            [8.57, 0.996],
            [8.56, 1.01],
            [8.55, 1.03],
            [8.53, 1.04],
            [8.52, 1.06],
            [8.51, 1.07],
            [8.48, 1.07],
            [8.47, 1.09],
            [8.46, 1.1],
            [8.44, 1.12],
            [8.42, 1.13],
            [8.4, 1.14],
            [8.38, 1.15],
            [8.36, 1.16],
            [8.34, 1.17],
            [8.32, 1.18],
            [8.3, 1.18],
            [8.16, 1.24],
            [8.15, 1.25],
            [8.13, 1.26],
            [8.11, 1.27],
            [8.09, 1.28],
            [8.07, 1.29],
            [8.05, 1.3],
            [8.03, 1.31],
            [8.0, 1.32],
            [8.0, 1.34],
            [7.99, 1.36],
            [7.97, 1.37],
            [7.95, 1.37],
            [7.93, 1.37],
            [7.91, 1.38],
            [7.89, 1.38],
            [7.87, 1.38],
            [7.85, 1.38],
            [7.83, 1.38],
            [7.8, 1.39],
            [7.78, 1.39],
            [7.76, 1.38],
            [7.74, 1.39],
            [7.72, 1.39],
            [7.7, 1.38],
            [7.68, 1.37],
            [7.67, 1.35],
            [7.66, 1.34],
            [7.65, 1.32],
            [7.63, 1.3],
            [7.62, 1.28],
            [7.61, 1.26],
            [7.6, 1.24],
            [7.59, 1.22],
            [7.59, 1.2],
            [7.58, 1.17],
            [7.59, 1.15],
            [7.6, 1.14],
            [7.61, 1.12],
            [7.62, 1.1],
            [7.62, 1.08],
            [7.62, 1.06],
            [7.62, 1.04],
            [7.64, 1.03],
            [7.66, 1.02],
            [7.66, 1.01],
            [7.66, 0.991],
            [7.66, 0.972],
            [7.68, 0.957],
            [7.69, 0.941],
            [7.69, 0.923],
            [7.69, 0.899],
            [7.71, 0.882],
            [7.72, 0.866],
            [7.73, 0.851],
            [7.75, 0.837],
            [7.76, 0.824],
            [7.78, 0.811],
            [7.8, 0.799],
            [7.81, 0.786],
            [7.83, 0.774],
            [7.84, 0.761],
            [7.86, 0.748],
            [7.88, 0.745],
            [7.9, 0.744],
            [7.92, 0.733],
            [7.94, 0.718],
            [7.95, 0.703],
            [7.97, 0.694],
            [7.99, 0.704],
            [8.02, 0.687],
            [8.04, 0.677],
            [8.06, 0.675],
            [8.09, 0.679],
            [8.11, 0.687],
            [8.13, 0.698],
            [8.14, 0.712],
            [8.15, 0.727],
            [8.16, 0.752],
            [8.16, 0.771],
            [8.15, 0.789],
            [8.16, 0.807],
            [8.16, 0.826],
            [8.15, 0.847],
            [8.14, 0.863],
            [8.13, 0.88],
            [8.12, 0.896],
            [8.11, 0.914],
            [8.1, 0.934],
            [8.09, 0.953],
            [8.08, 0.972],
            [8.07, 0.989],
            [8.06, 1.0],
            [8.04, 1.02],
            [8.02, 1.02],
            [8.0, 1.03],
            [7.99, 1.04],
            [7.99, 1.06],
            [7.97, 1.07],
            [7.95, 1.08],
            [7.94, 1.09],
            [7.92, 1.11],
            [7.9, 1.1],
            [7.88, 1.1],
            [7.87, 1.12],
            [7.86, 1.14],
            [7.85, 1.15],
            [7.84, 1.17],
            [7.82, 1.17],
            [7.8, 1.18],
            [7.78, 1.19],
            [7.76, 1.2],
            [7.74, 1.21],
            [7.72, 1.22],
            [7.7, 1.23],
            [7.69, 1.25],
            [7.7, 1.26],
            [7.72, 1.27],
            [7.74, 1.27],
            [7.76, 1.27],
            [7.78, 1.27],
            [7.81, 1.27],
            [7.82, 1.27],
            [7.84, 1.28],
            [7.87, 1.27],
            [7.89, 1.27],
            [7.91, 1.26],
            [7.93, 1.25],
            [7.94, 1.25],
            [7.96, 1.24],
            [7.98, 1.23],
            [8.0, 1.22],
            [8.02, 1.21],
            [8.04, 1.21],
            [8.06, 1.2],
            [8.08, 1.2],
            [8.1, 1.21],
            [8.11, 1.21],
            [8.13, 1.19],
            [8.13, 1.18],
            [8.15, 1.16],
            [8.16, 1.15],
            [8.18, 1.14],
            [8.2, 1.13],
            [8.23, 1.13],
            [8.25, 1.13],
            [8.26, 1.12],
            [8.27, 1.1],
            [8.29, 1.09],
            [8.31, 1.09],
            [8.33, 1.09],
            [8.35, 1.09],
            [8.37, 1.08],
            [8.38, 1.07],
            [8.4, 1.05],
            [8.42, 1.06],
            [8.43, 1.04],
            [8.45, 1.03],
            [8.47, 1.01],
            [8.48, 1.0],
            [8.5, 0.99],
            [8.52, 0.978],
            [8.54, 0.966],
            [8.56, 0.954],
            [8.58, 0.941],
            [8.59, 0.928],
            [8.6, 0.945],
            [8.6, 0.964],
        ],
        vec![
            [8.05, 0.802],
            [8.03, 0.792],
            [8.01, 0.786],
            [7.99, 0.787],
            [7.97, 0.797],
            [7.96, 0.808],
            [7.94, 0.822],
            [7.93, 0.836],
            [7.92, 0.849],
            [7.9, 0.86],
            [7.88, 0.868],
            [7.86, 0.871],
            [7.84, 0.867],
            [7.84, 0.888],
            [7.83, 0.908],
            [7.83, 0.929],
            [7.83, 0.95],
            [7.81, 0.96],
            [7.79, 0.972],
            [7.78, 0.988],
            [7.77, 1.0],
            [7.76, 1.02],
            [7.76, 1.04],
            [7.75, 1.06],
            [7.74, 1.08],
            [7.75, 1.09],
            [7.77, 1.08],
            [7.79, 1.07],
            [7.81, 1.06],
            [7.83, 1.05],
            [7.84, 1.04],
            [7.86, 1.03],
            [7.88, 1.02],
            [7.89, 1.01],
            [7.91, 1.0],
            [7.93, 0.983],
            [7.95, 0.964],
            [7.97, 0.951],
            [7.99, 0.932],
            [8.01, 0.909],
            [8.02, 0.885],
            [8.03, 0.86],
            [8.04, 0.841],
            [8.05, 0.824],
            [8.05, 0.805],
        ],
    ]
}

fn handwritten_hi_there_paths() -> Vec<Vec<[f32; 2]>> {
    let mut paths = Vec::new();
    let mut hi = Vec::new();
    append_hi_path(&mut hi, 0.0);
    paths.push(hi);

    let mut there = Vec::new();
    append_there_path(&mut there, 2.10);
    paths.push(there);

    paths.push(vec![[4.35, 0.42], [5.05, 0.35]]);
    paths
}

fn handwritten_hey_there_paths() -> Vec<Vec<[f32; 2]>> {
    let mut paths = Vec::new();
    let mut hey = Vec::new();
    append_hey_path(&mut hey, 0.0);
    paths.push(hey);

    let mut there = Vec::new();
    append_there_path(&mut there, 3.40);
    paths.push(there);

    paths.push(vec![[5.65, 0.42], [6.35, 0.35]]);
    paths
}

#[allow(dead_code)]
fn append_hello_path(path: &mut Vec<[f32; 2]>, x: f32) {
    path.push([x + 0.05, 1.05]);
    append_cubic(
        path,
        [x + 0.05, 1.05],
        [x + 0.12, 0.64],
        [x + 0.10, 0.15],
        [x + 0.34, -0.08],
        10,
    );
    append_cubic(
        path,
        [x + 0.34, -0.08],
        [x + 0.66, 0.14],
        [x + 0.20, 0.88],
        [x + 0.26, 1.06],
        14,
    );
    append_cubic(
        path,
        [x + 0.26, 1.06],
        [x + 0.38, 0.58],
        [x + 0.82, 0.52],
        [x + 1.02, 1.02],
        12,
    );
    append_cubic(
        path,
        [x + 1.02, 1.02],
        [x + 1.20, 0.58],
        [x + 1.72, 0.45],
        [x + 1.58, 0.86],
        12,
    );
    append_cubic(
        path,
        [x + 1.58, 0.86],
        [x + 1.42, 1.18],
        [x + 2.02, 1.18],
        [x + 2.22, 0.92],
        12,
    );
    append_cubic(
        path,
        [x + 2.22, 0.92],
        [x + 2.62, 0.45],
        [x + 2.78, -0.10],
        [x + 2.96, -0.06],
        12,
    );
    append_cubic(
        path,
        [x + 2.96, -0.06],
        [x + 3.22, 0.02],
        [x + 2.76, 0.78],
        [x + 3.07, 1.02],
        12,
    );
    append_cubic(
        path,
        [x + 3.07, 1.02],
        [x + 3.48, 0.56],
        [x + 3.60, -0.08],
        [x + 3.82, -0.04],
        12,
    );
    append_cubic(
        path,
        [x + 3.82, -0.04],
        [x + 4.04, 0.04],
        [x + 3.66, 0.72],
        [x + 3.90, 1.00],
        12,
    );
    append_cubic(
        path,
        [x + 3.90, 1.00],
        [x + 4.22, 0.38],
        [x + 5.00, 0.44],
        [x + 4.88, 0.86],
        16,
    );
    append_cubic(
        path,
        [x + 4.88, 0.86],
        [x + 4.74, 1.28],
        [x + 4.02, 1.15],
        [x + 4.15, 0.72],
        16,
    );
    append_cubic(
        path,
        [x + 4.15, 0.72],
        [x + 4.38, 0.28],
        [x + 4.96, 0.92],
        [x + 5.20, 0.82],
        12,
    );
}

fn append_hi_path(path: &mut Vec<[f32; 2]>, x: f32) {
    path.push([x + 0.08, 1.10]);
    append_cubic(
        path,
        [x + 0.08, 1.10],
        [x + 0.14, 0.70],
        [x + 0.12, 0.20],
        [x + 0.36, -0.06],
        10,
    );
    append_cubic(
        path,
        [x + 0.36, -0.06],
        [x + 0.68, -0.34],
        [x + 0.70, 0.74],
        [x + 0.40, 0.70],
        12,
    );
    append_cubic(
        path,
        [x + 0.40, 0.70],
        [x + 0.18, 0.66],
        [x + 0.28, 0.36],
        [x + 0.55, 0.42],
        8,
    );
    append_cubic(
        path,
        [x + 0.55, 0.42],
        [x + 0.82, 0.52],
        [x + 0.74, 0.78],
        [x + 0.92, 0.78],
        6,
    );
    append_cubic(
        path,
        [x + 0.92, 0.78],
        [x + 1.08, 0.76],
        [x + 0.93, 0.30],
        [x + 1.12, 0.28],
        8,
    );
    path.push([x + 0.99, 0.08]);
    path.push([x + 1.00, 0.09]);
}

fn append_hey_path(path: &mut Vec<[f32; 2]>, x: f32) {
    path.push([x + 0.08, 1.10]);
    append_cubic(
        path,
        [x + 0.08, 1.10],
        [x + 0.14, 0.68],
        [x + 0.12, 0.18],
        [x + 0.36, -0.06],
        10,
    );
    append_cubic(
        path,
        [x + 0.36, -0.06],
        [x + 0.70, -0.30],
        [x + 0.70, 0.74],
        [x + 0.42, 0.70],
        12,
    );
    append_cubic(
        path,
        [x + 0.42, 0.70],
        [x + 0.20, 0.66],
        [x + 0.28, 0.36],
        [x + 0.56, 0.42],
        8,
    );
    append_cubic(
        path,
        [x + 0.56, 0.42],
        [x + 0.88, 0.56],
        [x + 0.98, 0.62],
        [x + 1.18, 0.58],
        8,
    );
    append_cubic(
        path,
        [x + 1.18, 0.58],
        [x + 0.88, 0.80],
        [x + 0.86, 0.24],
        [x + 1.28, 0.30],
        12,
    );
    append_cubic(
        path,
        [x + 1.28, 0.30],
        [x + 1.54, 0.34],
        [x + 1.64, 0.52],
        [x + 1.78, 0.68],
        8,
    );
    append_cubic(
        path,
        [x + 1.78, 0.68],
        [x + 1.62, 0.38],
        [x + 1.70, 0.10],
        [x + 1.98, 0.10],
        8,
    );
    append_cubic(
        path,
        [x + 1.98, 0.10],
        [x + 2.30, 0.10],
        [x + 2.24, 0.68],
        [x + 2.08, 0.56],
        8,
    );
    append_cubic(
        path,
        [x + 2.08, 0.56],
        [x + 1.96, 0.44],
        [x + 2.08, 0.18],
        [x + 2.34, 0.20],
        8,
    );
    append_cubic(
        path,
        [x + 2.34, 0.20],
        [x + 2.58, 0.26],
        [x + 2.54, 0.58],
        [x + 2.70, 0.60],
        8,
    );
}

fn append_there_path(path: &mut Vec<[f32; 2]>, x: f32) {
    path.push([x + 0.38, 0.08]);
    append_cubic(
        path,
        [x + 0.38, 0.08],
        [x + 0.24, 0.52],
        [x + 0.22, 0.92],
        [x + 0.40, 1.06],
        12,
    );
    append_cubic(
        path,
        [x + 0.40, 1.06],
        [x + 0.66, 1.22],
        [x + 0.98, 0.92],
        [x + 1.05, 0.82],
        10,
    );
    append_cubic(
        path,
        [x + 1.05, 0.82],
        [x + 1.12, 0.44],
        [x + 1.14, 0.04],
        [x + 1.36, -0.05],
        12,
    );
    append_cubic(
        path,
        [x + 1.36, -0.05],
        [x + 1.72, 0.16],
        [x + 1.26, 0.78],
        [x + 1.38, 1.04],
        14,
    );
    append_cubic(
        path,
        [x + 1.38, 1.04],
        [x + 1.58, 0.62],
        [x + 2.00, 0.50],
        [x + 2.18, 1.02],
        12,
    );
    append_cubic(
        path,
        [x + 2.18, 1.02],
        [x + 2.38, 0.56],
        [x + 2.90, 0.45],
        [x + 2.76, 0.86],
        12,
    );
    append_cubic(
        path,
        [x + 2.76, 0.86],
        [x + 2.60, 1.18],
        [x + 3.20, 1.18],
        [x + 3.40, 0.92],
        12,
    );
    append_cubic(
        path,
        [x + 3.40, 0.92],
        [x + 3.54, 0.54],
        [x + 3.86, 0.54],
        [x + 4.00, 0.80],
        10,
    );
    append_cubic(
        path,
        [x + 4.00, 0.80],
        [x + 4.10, 0.52],
        [x + 4.24, 0.48],
        [x + 4.40, 0.62],
        8,
    );
    append_cubic(
        path,
        [x + 4.40, 0.62],
        [x + 4.22, 0.80],
        [x + 4.14, 1.14],
        [x + 4.50, 1.04],
        10,
    );
    append_cubic(
        path,
        [x + 4.50, 1.04],
        [x + 4.82, 0.56],
        [x + 5.34, 0.45],
        [x + 5.20, 0.86],
        12,
    );
    append_cubic(
        path,
        [x + 5.20, 0.86],
        [x + 5.04, 1.18],
        [x + 5.66, 1.16],
        [x + 5.92, 0.88],
        12,
    );
}

fn append_cubic(
    path: &mut Vec<[f32; 2]>,
    p0: [f32; 2],
    p1: [f32; 2],
    p2: [f32; 2],
    p3: [f32; 2],
    steps: usize,
) {
    let steps = steps.saturating_mul(3).max(1);
    for step in 1..=steps {
        let t = step as f32 / steps as f32;
        let mt = 1.0 - t;
        path.push([
            mt.powi(3) * p0[0]
                + 3.0 * mt.powi(2) * t * p1[0]
                + 3.0 * mt * t.powi(2) * p2[0]
                + t.powi(3) * p3[0],
            mt.powi(3) * p0[1]
                + 3.0 * mt.powi(2) * t * p1[1]
                + 3.0 * mt * t.powi(2) * p2[1]
                + t.powi(3) * p3[1],
        ]);
    }
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

pub(crate) fn push_streaming_activity_cue(
    vertices: &mut Vec<Vertex>,
    app: &SingleSessionApp,
    size: PhysicalSize<u32>,
    tick: u64,
    viewport: Option<&SingleSessionBodyViewport>,
) {
    let typography = single_session_typography();
    let line_height = typography.body_size * typography.body_line_height;
    let body_top = single_session_body_top_for_app(app, size);
    let viewport = viewport
        .cloned()
        .unwrap_or_else(|| single_session_body_viewport_for_tick(app, size, tick, 0.0));
    let active_line_index = if app.streaming_response.is_empty() {
        None
    } else {
        viewport.lines.len().checked_sub(1)
    };

    let cue_y = active_line_index
        .map(|line_index| body_top + viewport.top_offset_pixels + line_index as f32 * line_height)
        .filter(|y| *y >= PANEL_BODY_TOP_PADDING && *y <= single_session_body_bottom(size))
        .unwrap_or_else(|| {
            single_session_draft_top_for_app(app, size) - typography.body_size * 0.82
        });
    let cue_x = PANEL_TITLE_LEFT_PADDING + typography.body_size * 0.08;
    let phase = (tick % 24) as f32 / 24.0;
    let pulse = 0.5 + 0.5 * (phase * std::f32::consts::TAU).sin();

    let mut beam_color = NATIVE_SPINNER_HEAD_COLOR;
    beam_color[3] = if app.streaming_response.is_empty() {
        0.22 + 0.24 * pulse
    } else {
        0.36 + 0.34 * pulse
    };
    push_rounded_rect(
        vertices,
        Rect {
            x: cue_x,
            y: cue_y + line_height * 0.16,
            width: 3.0,
            height: line_height * 0.68,
        },
        1.5,
        beam_color,
        size,
    );

    let dot_radius = (typography.body_size * 0.12).clamp(2.0, 3.5);
    let dot_y = cue_y + line_height * 0.50 - dot_radius;
    let dot_start_x = cue_x + typography.body_size * 0.55;
    for dot in 0..3 {
        let dot_phase = ((tick + dot as u64 * 3) % 12) as f32 / 12.0;
        let dot_pulse = 0.5 + 0.5 * (dot_phase * std::f32::consts::TAU).sin();
        let mut dot_color = if app.streaming_response.is_empty() {
            NATIVE_SPINNER_TRACK_COLOR
        } else {
            NATIVE_SPINNER_HEAD_COLOR
        };
        dot_color[3] = (0.30 + 0.58 * dot_pulse).clamp(0.24, 0.92);
        push_rounded_rect(
            vertices,
            Rect {
                x: dot_start_x + dot as f32 * dot_radius * 2.8,
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

#[cfg(test)]
#[derive(Clone, Copy, Debug)]
pub(crate) struct SingleSessionTranscriptCardGeometry {
    pub(crate) run: SingleSessionTranscriptCardRun,
    pub(crate) card_rect: Rect,
    pub(crate) text_left: f32,
    pub(crate) line_height: f32,
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
    );
}

fn push_single_session_transcript_cards_from_viewport(
    vertices: &mut Vec<Vertex>,
    app: &SingleSessionApp,
    size: PhysicalSize<u32>,
    viewport: &SingleSessionBodyViewport,
    total_lines: usize,
) {
    let typography = single_session_typography_for_scale(app.text_scale());
    let line_height = typography.body_size * typography.body_line_height;
    let width = (size.width as f32 - PANEL_TITLE_LEFT_PADDING * 2.0 + 12.0).max(1.0);
    let body_top = single_session_body_top_for_app(app, size);
    let body_bottom = single_session_body_bottom_for_total_lines(app, size, total_lines);

    for run in single_session_transcript_card_runs(&viewport.lines) {
        let Some(color) = single_session_line_card_color(run.style) else {
            continue;
        };
        let rect = Rect {
            x: PANEL_TITLE_LEFT_PADDING - 6.0,
            y: body_top + viewport.top_offset_pixels + run.line as f32 * line_height + 3.0,
            width,
            height: (run.line_count as f32 * line_height - 6.0).max(1.0),
        };
        let Some(rect) = clip_rect_to_vertical_bounds(rect, body_top, body_bottom) else {
            continue;
        };
        push_rounded_rect(vertices, rect, 7.0, color, size);
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
    let width = (size.width as f32 - PANEL_TITLE_LEFT_PADDING * 2.0 + 12.0).max(1.0);
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
    );
}

fn push_single_session_inline_code_cards_from_viewport(
    vertices: &mut Vec<Vertex>,
    app: &SingleSessionApp,
    size: PhysicalSize<u32>,
    viewport: &SingleSessionBodyViewport,
    total_lines: usize,
) {
    let text_scale = app.text_scale();
    let typography = single_session_typography_for_scale(text_scale);
    let line_height = typography.body_size * typography.body_line_height;
    let char_width = single_session_body_char_width_for_scale(text_scale);
    let body_top = single_session_body_top_for_app(app, size);
    let body_bottom = single_session_body_bottom_for_total_lines(app, size, total_lines);
    let right = (size.width as f32 - PANEL_TITLE_LEFT_PADDING + 3.0).max(PANEL_TITLE_LEFT_PADDING);
    let card_height = (typography.body_size * 1.10)
        .min(line_height - 5.0)
        .max(typography.body_size * 0.85);
    let radius = (5.0 * text_scale).clamp(4.0, 8.0);
    let horizontal_pad = (3.5 * text_scale).clamp(3.0, 6.0);

    for (line_index, line) in viewport.lines.iter().enumerate() {
        if !single_session_line_style_supports_inline_code_cards(line.style) {
            continue;
        }
        let line_y = body_top + viewport.top_offset_pixels + line_index as f32 * line_height;
        let code_runs = single_session_inline_code_runs_for_line(line);
        for run in &code_runs {
            let x =
                PANEL_TITLE_LEFT_PADDING + run.start_column as f32 * char_width - horizontal_pad;
            let width = run.column_count as f32 * char_width + horizontal_pad * 2.0;
            let clipped_right = (x + width).min(right);
            if clipped_right <= x {
                continue;
            }
            let rect = Rect {
                x,
                y: line_y + (line_height - card_height) * 0.5,
                width: clipped_right - x,
                height: card_height,
            };
            let Some(rect) = clip_rect_to_vertical_bounds(rect, body_top, body_bottom) else {
                continue;
            };
            push_rounded_rect(vertices, rect, radius, INLINE_CODE_BACKGROUND_COLOR, size);
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
            let clipped_right = (x + width).min(right);
            if clipped_right <= x {
                continue;
            }
            let rect = Rect {
                x,
                y: line_y + (line_height - card_height) * 0.5,
                width: clipped_right - x,
                height: card_height,
            };
            let Some(rect) = clip_rect_to_vertical_bounds(rect, body_top, body_bottom) else {
                continue;
            };
            push_rounded_rect(vertices, rect, radius, INLINE_MATH_BACKGROUND_COLOR, size);
        }
    }
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
    let right = (size.width as f32 - PANEL_TITLE_LEFT_PADDING + 3.0).max(left + 1.0);
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
) {
    let Some(metrics) = single_session_body_scroll_metrics(app, size, tick) else {
        return;
    };
    push_single_session_scrollbar_for_metrics(vertices, size, smooth_scroll_lines, metrics);
}

fn push_single_session_scrollbar_for_total_lines(
    vertices: &mut Vec<Vertex>,
    app: &SingleSessionApp,
    size: PhysicalSize<u32>,
    smooth_scroll_lines: f32,
    total_lines: usize,
) {
    let Some(metrics) = single_session_body_scroll_metrics_for_total_lines(app, size, total_lines)
    else {
        return;
    };
    push_single_session_scrollbar_for_metrics(vertices, size, smooth_scroll_lines, metrics);
}

fn push_single_session_scrollbar_for_metrics(
    vertices: &mut Vec<Vertex>,
    size: PhysicalSize<u32>,
    smooth_scroll_lines: f32,
    metrics: SingleSessionBodyScrollMetrics,
) {
    let track_top = PANEL_BODY_TOP_PADDING + 4.0;
    let track_bottom = single_session_body_bottom(size) - 4.0;
    let track_height = (track_bottom - track_top).max(1.0);
    let x = size.width as f32 - PANEL_TITLE_LEFT_PADDING - 4.0;
    let thumb_height = (metrics.visible_lines as f32 / metrics.total_lines as f32 * track_height)
        .clamp(28.0, track_height);
    let travel = (track_height - thumb_height).max(0.0);
    let smooth_scroll_lines =
        (metrics.scroll_lines + smooth_scroll_lines).clamp(0.0, metrics.max_scroll_lines as f32);
    let scroll_fraction = smooth_scroll_lines / metrics.max_scroll_lines.max(1) as f32;
    let thumb_y = track_top + (1.0 - scroll_fraction.clamp(0.0, 1.0)) * travel;

    push_rounded_rect(
        vertices,
        Rect {
            x,
            y: track_top,
            width: 3.0,
            height: track_height,
        },
        2.0,
        [0.040, 0.055, 0.090, 0.075],
        size,
    );
    push_rounded_rect(
        vertices,
        Rect {
            x: x - 0.5,
            y: thumb_y,
            width: 4.0,
            height: thumb_height,
        },
        2.0,
        [0.035, 0.065, 0.145, 0.34],
        size,
    );
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

fn single_session_line_card_color(style: SingleSessionLineStyle) -> Option<[f32; 4]> {
    match style {
        SingleSessionLineStyle::AssistantHeading => Some(MARKDOWN_HEADING_BACKGROUND_COLOR),
        SingleSessionLineStyle::CodeHeader | SingleSessionLineStyle::Code => {
            Some(CODE_BLOCK_BACKGROUND_COLOR)
        }
        SingleSessionLineStyle::AssistantQuote => Some(QUOTE_CARD_BACKGROUND_COLOR),
        SingleSessionLineStyle::AssistantTable => Some(TABLE_CARD_BACKGROUND_COLOR),
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

    let typography = single_session_typography();
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
    height: f32,
}

pub(crate) fn glyphon_draft_caret_position(
    app: &SingleSessionApp,
    draft_buffer: &Buffer,
    size: PhysicalSize<u32>,
) -> Option<CaretPosition> {
    let typography = single_session_typography();
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

fn approximate_draft_caret_position(
    app: &SingleSessionApp,
    size: PhysicalSize<u32>,
) -> CaretPosition {
    let typography = single_session_typography();
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
            .min((size.width as f32 - PANEL_TITLE_LEFT_PADDING * 2.0).max(0.0));
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
        if app.inline_widget_line_count() > 0 {
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
        if app.inline_widget_line_count() > 0 {
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
            vec![SingleSessionStyledLine::new(
                "Type a message to start. Ask me to build, debug, explain, or automate something.",
                SingleSessionLineStyle::Meta,
            )]
        } else {
            Vec::new()
        };
        (app.welcome_hero_text(), welcome_hint)
    } else if app.is_fresh_welcome_visible() && app.draft.is_empty() {
        (
            String::new(),
            vec![SingleSessionStyledLine::new(
                "Type a message to start. Ask me to build, debug, explain, or automate something.",
                SingleSessionLineStyle::Meta,
            )],
        )
    } else {
        (String::new(), Vec::new())
    };
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
        body,
        inline_widget: app.inline_widget_styled_lines(),
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
    let content_width = (size.width as f32 - PANEL_TITLE_LEFT_PADDING * 2.0).max(1.0);

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

    let exact_layout_compatible = previous_key.is_some_and(|previous| {
        previous.size == key.size && previous.text_scale_bits == key.text_scale_bits
    });
    let body_layout_compatible = previous_key.is_some_and(|previous| {
        previous.text_scale_bits == key.text_scale_bits
            && single_session_body_text_buffer_layout_bucket(previous.size, text_scale)
                == single_session_body_text_buffer_layout_bucket(key.size, text_scale)
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
        reuse_body_buffer || body_previous.is_some_and(|previous| previous.body == key.body),
    )
    .unwrap_or_else(|| {
        single_session_body_text_buffer_from_lines(font_system, &key.body, size, text_scale)
    });

    let inline_widget_width = if key.inline_widget.is_empty() {
        content_width
    } else {
        inline_widget_intrinsic_text_width(&key.inline_widget, size, text_scale)
            .max(1.0)
            .min(content_width)
    };
    let inline_widget_buffer = take_reusable(
        &mut old_buffers,
        4,
        exact_previous.is_some_and(|previous| previous.inline_widget == key.inline_widget),
    )
    .unwrap_or_else(|| {
        single_session_styled_text_buffer(
            font_system,
            &key.inline_widget,
            typography.body_size,
            typography.body_size * typography.body_line_height,
            inline_widget_width,
            prompt_height,
            Wrap::Word,
        )
    });

    let draft_buffer = take_reusable(
        &mut old_buffers,
        2,
        exact_previous.is_some_and(|previous| previous.draft == key.draft),
    )
    .unwrap_or_else(|| {
        single_session_text_buffer(
            font_system,
            &key.draft,
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
    ]
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
    let content_width = (size.width as f32 - PANEL_TITLE_LEFT_PADDING * 2.0).max(1.0);
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
    let height_lines = single_session_body_text_buffer_layout_lines(physical_size, text_scale)
        .min(u32::MAX as usize) as u32;
    (0, height_lines)
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
    let typography = single_session_typography_for_scale(app.text_scale());
    let line_height = typography.body_size * typography.body_line_height;
    let body_top = single_session_body_top_for_app(app, size);
    let total_lines = lines.len();
    let body_bottom = single_session_body_bottom_for_total_lines(app, size, total_lines);
    let available_height = (body_bottom - body_top).max(line_height);
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
        + single_session_wrapped_body_lines(
            app.streaming_response_styled_lines(),
            size,
            app.text_scale(),
        )
        .len()
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
    if line.text.is_empty() || !text_exceeds_columns(&line.text, max_columns) {
        wrapped.push(line);
        return;
    }
    push_wrapped_body_line_parts(
        wrapped,
        &line.text,
        &line.inline_spans,
        line.style,
        max_columns,
    );
}

fn push_wrapped_body_line_ref(
    wrapped: &mut Vec<SingleSessionStyledLine>,
    line: &SingleSessionStyledLine,
    max_columns: usize,
) {
    if line.text.is_empty() || !text_exceeds_columns(&line.text, max_columns) {
        wrapped.push(line.clone());
        return;
    }
    push_wrapped_body_line_parts(
        wrapped,
        &line.text,
        &line.inline_spans,
        line.style,
        max_columns,
    );
}

fn push_wrapped_body_line_parts(
    wrapped: &mut Vec<SingleSessionStyledLine>,
    text: &str,
    inline_spans: &[SingleSessionInlineSpan],
    style: SingleSessionLineStyle,
    max_columns: usize,
) {
    for (text, inline_spans) in wrap_body_line_text_with_spans(text, inline_spans, max_columns) {
        wrapped.push(SingleSessionStyledLine::with_inline_spans(
            text,
            style,
            inline_spans,
        ));
    }
}

fn single_session_body_max_columns(size: PhysicalSize<u32>, text_scale: f32) -> usize {
    let content_width = (size.width as f32 - PANEL_TITLE_LEFT_PADDING * 2.0).max(1.0);
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

fn inline_spans_for_wrapped_range(
    inline_spans: &[SingleSessionInlineSpan],
    start: usize,
    end: usize,
) -> Vec<SingleSessionInlineSpan> {
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
    text.chars().nth(max_columns.max(1)).is_some()
}

fn word_wrap_split_index(text: &str, max_columns: usize) -> usize {
    let hard_split = byte_index_at_char_limit(text, max_columns);
    text[..hard_split]
        .char_indices()
        .rev()
        .find_map(|(index, ch)| ch.is_whitespace().then_some(index))
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

fn single_session_body_bottom_for_app(app: &SingleSessionApp, size: PhysicalSize<u32>) -> f32 {
    (single_session_body_bottom_base_for_app(app, size) - inline_widget_reserved_height(app))
        .max(single_session_body_top_for_app(app, size))
}

fn single_session_body_bottom_base_for_total_lines(
    app: &SingleSessionApp,
    size: PhysicalSize<u32>,
    total_lines: usize,
) -> f32 {
    if app.is_welcome_timeline_visible() {
        return (welcome_timeline_draft_top_for_total_lines(app, size, total_lines)
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
    (single_session_body_bottom_base_for_total_lines(app, size, total_lines)
        - inline_widget_reserved_height(app))
    .max(single_session_body_top_for_app(app, size))
}

fn inline_widget_text_height(app: &SingleSessionApp) -> f32 {
    let lines = app.inline_widget_line_count();
    if lines == 0 {
        return 0.0;
    }
    let typography = single_session_typography_for_scale(app.text_scale());
    lines as f32 * typography.body_size * typography.body_line_height
}

fn inline_widget_reserved_height(app: &SingleSessionApp) -> f32 {
    if app.inline_widget_line_count() == 0 {
        0.0
    } else {
        (inline_widget_text_height(app)
            + INLINE_WIDGET_CARD_PADDING_Y * 2.0
            + INLINE_WIDGET_BODY_GAP)
            * app.inline_widget_reveal_progress().clamp(0.0, 1.0)
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
    let shaping = if segments
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
            assistant_inline_markdown_run_attrs(line.style, text, &active_kinds),
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
) -> Attrs<'static> {
    if kinds.iter().any(|kind| {
        matches!(
            kind,
            SingleSessionInlineSpanKind::Code | SingleSessionInlineSpanKind::Math
        )
    }) {
        return single_session_style_attrs(SingleSessionLineStyle::Code);
    }

    let mut attrs = single_session_style_attrs_for_text(style, text);
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
        RichLineStyle::Meta | RichLineStyle::MediaPlaceholder => SingleSessionLineStyle::Meta,
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
    let family = if is_ai_response_font_style(style) && text_contains_symbol_glyphs(text) {
        SINGLE_SESSION_FONT_FAMILY
    } else {
        single_session_font_family_for_style(style)
    };
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
    let family = if is_ai_response_font_style(style) && text_contains_symbol_glyphs(text) {
        SINGLE_SESSION_FONT_FAMILY
    } else {
        single_session_font_family_for_style(style)
    };
    single_session_style_attrs_for_family(style, family)
}

fn single_session_font_family_for_style(style: SingleSessionLineStyle) -> &'static str {
    if is_ai_response_font_style(style) {
        SINGLE_SESSION_ASSISTANT_FONT_FAMILY
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

fn is_ai_response_font_style(style: SingleSessionLineStyle) -> bool {
    matches!(
        style,
        SingleSessionLineStyle::Assistant
            | SingleSessionLineStyle::AssistantHeading
            | SingleSessionLineStyle::AssistantQuote
            | SingleSessionLineStyle::AssistantLink
    )
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
        SingleSessionLineStyle::AssistantLink => ASSISTANT_LINK_TEXT_COLOR,
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
    let inline_widget_lines = app.inline_widget_styled_lines();
    let inline_widget_text_width =
        inline_widget_intrinsic_text_width(&inline_widget_lines, size, app.text_scale());
    let body_top_offset_pixels =
        single_session_body_viewport_for_tick(app, size, tick, smooth_scroll_lines)
            .top_offset_pixels;
    let welcome_chrome_offset_pixels =
        welcome_timeline_visual_offset_pixels(app, size, smooth_scroll_lines);
    let welcome_chrome_visible =
        welcome_timeline_chrome_visible(app, size, welcome_chrome_offset_pixels);
    single_session_text_areas_for_state(
        buffers,
        size,
        welcome_chrome_visible,
        false,
        body_top_offset_pixels,
        single_session_body_top_for_app(app, size),
        text_bounds_bottom(single_session_body_bottom_for_app(app, size)),
        inline_widget_lines.len(),
        inline_widget_text_width,
        single_session_draft_top_for_app(app, size),
        welcome_chrome_offset_pixels,
        welcome_status_lane_visible(app),
        app.is_fresh_welcome_visible() && app.draft.is_empty(),
        app.text_scale(),
        welcome_hero_runtime_mask_supported(&app.welcome_hero_text()),
        1.0,
        app.inline_widget_reveal_progress(),
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
    let inline_widget_lines = app.inline_widget_styled_lines();
    let inline_widget_text_width =
        inline_widget_intrinsic_text_width(&inline_widget_lines, size, app.text_scale());
    let welcome_chrome_offset_pixels = welcome_timeline_visual_offset_pixels_for_total_lines(
        app,
        size,
        smooth_scroll_lines,
        viewport.total_lines,
    );
    let welcome_chrome_visible =
        welcome_timeline_chrome_visible(app, size, welcome_chrome_offset_pixels);
    single_session_text_areas_for_state(
        buffers,
        size,
        welcome_chrome_visible,
        false,
        viewport.top_offset_pixels,
        single_session_body_top_for_app(app, size),
        text_bounds_bottom(single_session_body_bottom_for_total_lines(
            app,
            size,
            viewport.total_lines,
        )),
        inline_widget_lines.len(),
        inline_widget_text_width,
        single_session_draft_top_for_total_lines(app, size, viewport.total_lines),
        welcome_chrome_offset_pixels,
        welcome_status_lane_visible(app),
        app.is_fresh_welcome_visible() && app.draft.is_empty(),
        app.text_scale(),
        welcome_hero_runtime_mask_supported(&app.welcome_hero_text()),
        welcome_hero_reveal_progress,
        app.inline_widget_reveal_progress(),
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
    let typography = single_session_typography_for_scale(app.text_scale());
    let line_height = typography.body_size * typography.body_line_height;
    let left = PANEL_TITLE_LEFT_PADDING;
    let right = size.width.saturating_sub(PANEL_TITLE_LEFT_PADDING as u32) as i32;
    let body_top = single_session_body_top_for_app(app, size);
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
            bottom: text_bounds_bottom(single_session_body_bottom_for_total_lines(
                app,
                size,
                viewport.total_lines,
            )),
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
        0.0,
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
    inline_widget_text_width: f32,
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
    let right = size.width.saturating_sub(PANEL_TITLE_LEFT_PADDING as u32) as i32;
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
        inline_widget_card_layout(
            size,
            &typography,
            inline_widget_line_count,
            inline_widget_text_width,
            target_top,
            inline_widget_reveal_progress,
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
        let inline_bounds_right = layout
            .visible_text_right
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
