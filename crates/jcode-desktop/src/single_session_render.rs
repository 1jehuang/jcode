use super::*;

#[derive(Clone, Debug, Eq, PartialEq)]
pub(crate) struct SingleSessionTextKey {
    pub(crate) size: (u32, u32),
    pub(crate) title: String,
    pub(crate) version: String,
    pub(crate) welcome_hero: String,
    pub(crate) welcome_hint: Vec<SingleSessionStyledLine>,
    pub(crate) activity_active: bool,
    pub(crate) body: Vec<SingleSessionStyledLine>,
    pub(crate) draft: String,
    pub(crate) status: String,
}

pub(crate) fn build_single_session_vertices(
    app: &SingleSessionApp,
    size: PhysicalSize<u32>,
    focus_pulse: f32,
    spinner_tick: u64,
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
    push_surface(
        &mut vertices,
        rect,
        surface.color_index,
        true,
        focus_pulse,
        size,
    );

    if app.is_empty_fresh_session() {
        push_fresh_welcome_ambient(&mut vertices, size, spinner_tick);
        push_handwritten_welcome_hero(&mut vertices, size, app.welcome_reveal_progress());
    }

    push_single_session_composer_card(&mut vertices, size);
    if app.has_activity_indicator() {
        push_native_activity_spinner(&mut vertices, size, spinner_tick);
    }
    push_single_session_transcript_cards(&mut vertices, app, size);
    push_single_session_streaming_shimmer(&mut vertices, app, size, spinner_tick);
    push_single_session_selection(&mut vertices, app, size);
    push_single_session_scrollbar(&mut vertices, app, size, spinner_tick);

    vertices
}

fn push_fresh_welcome_ambient(vertices: &mut Vec<Vertex>, size: PhysicalSize<u32>, tick: u64) {
    let draft_top = single_session_draft_top(size);
    let usable_height = (draft_top - PANEL_BODY_TOP_PADDING).max(180.0);
    let t = tick as f32 * 0.055;

    push_aurora_ribbon(
        vertices,
        size,
        PANEL_BODY_TOP_PADDING + usable_height * 0.18 + (t * 0.60).sin() * 18.0,
        usable_height * 0.30,
        t * 0.85,
        WELCOME_AURORA_BLUE,
        WELCOME_AURORA_VIOLET,
    );
    push_aurora_ribbon(
        vertices,
        size,
        PANEL_BODY_TOP_PADDING + usable_height * 0.39 + (t * 0.47).cos() * 24.0,
        usable_height * 0.34,
        t * -0.72 + 1.8,
        WELCOME_AURORA_MINT,
        WELCOME_AURORA_BLUE,
    );
    push_aurora_ribbon(
        vertices,
        size,
        PANEL_BODY_TOP_PADDING + usable_height * 0.58 + (t * 0.52).sin() * 16.0,
        usable_height * 0.24,
        t * 0.64 + 3.2,
        WELCOME_AURORA_WARM,
        WELCOME_AURORA_MINT,
    );
}

pub(crate) fn push_handwritten_welcome_hero(
    vertices: &mut Vec<Vertex>,
    size: PhysicalSize<u32>,
    reveal_progress: f32,
) {
    let paths = handwritten_welcome_paths();
    let total_length = stroke_paths_length(&paths);
    if total_length <= 0.0 {
        return;
    }

    let draft_top = single_session_draft_top(size);
    let target_width = size.width as f32 * 0.52;
    let (source_min, source_max) = stroke_paths_bounds(&paths);
    let source_width = (source_max[0] - source_min[0]).max(1.0);
    let scale = target_width / source_width;
    let source_left = source_min[0];
    let source_top = source_min[1];
    let left = (size.width as f32 - target_width) * 0.5;
    let top = PANEL_BODY_TOP_PADDING + (draft_top - PANEL_BODY_TOP_PADDING) * 0.31;
    let origin = [left - source_left * scale, top - source_top * scale];
    let thickness = (scale * 0.075).clamp(3.6, 8.5);
    let mut remaining = total_length * reveal_progress.clamp(0.0, 1.0);
    let mut lead = None;

    for path in &paths {
        for pair in path.windows(2) {
            let a = pair[0];
            let b = pair[1];
            let segment_length = distance(a, b);
            if segment_length <= 0.001 {
                continue;
            }
            if remaining <= 0.0 {
                break;
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
        && (0.01..0.995).contains(&reveal_progress)
    {
        push_stroke_dot(
            vertices,
            point,
            thickness * 1.65,
            WELCOME_HANDWRITING_HIGHLIGHT_COLOR,
            size,
        );
    }
}

fn handwritten_welcome_paths() -> Vec<Vec<[f32; 2]>> {
    let mut paths = Vec::new();
    let mut word = Vec::new();
    append_hello_path(&mut word, 0.0);
    paths.push(word);

    let mut there = Vec::new();
    append_there_path(&mut there, 5.55);
    paths.push(there);

    paths.push(vec![[5.80, 0.42], [6.50, 0.35]]);
    paths
}

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

fn stroke_paths_length(paths: &[Vec<[f32; 2]>]) -> f32 {
    paths
        .iter()
        .flat_map(|path| path.windows(2).map(|pair| distance(pair[0], pair[1])))
        .sum()
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
    let length = distance(a, b);
    if length <= 0.01 {
        return;
    }
    let nx = -(b[1] - a[1]) / length * thickness * 0.5;
    let ny = (b[0] - a[0]) / length * thickness * 0.5;
    push_pixel_triangle(
        vertices,
        [a[0] + nx, a[1] + ny],
        [a[0] - nx, a[1] - ny],
        [b[0] - nx, b[1] - ny],
        color,
        size,
    );
    push_pixel_triangle(
        vertices,
        [a[0] + nx, a[1] + ny],
        [b[0] - nx, b[1] - ny],
        [b[0] + nx, b[1] + ny],
        color,
        size,
    );
    push_stroke_dot(vertices, b, thickness * 0.52, color, size);
}

fn push_stroke_dot(
    vertices: &mut Vec<Vertex>,
    center: [f32; 2],
    radius: f32,
    color: [f32; 4],
    size: PhysicalSize<u32>,
) {
    let segments = 14;
    for segment in 0..segments {
        let start = segment as f32 / segments as f32 * std::f32::consts::TAU;
        let end = (segment + 1) as f32 / segments as f32 * std::f32::consts::TAU;
        push_pixel_triangle(
            vertices,
            center,
            [
                center[0] + radius * start.cos(),
                center[1] + radius * start.sin(),
            ],
            [
                center[0] + radius * end.cos(),
                center[1] + radius * end.sin(),
            ],
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

fn push_single_session_composer_card(vertices: &mut Vec<Vertex>, size: PhysicalSize<u32>) {
    let draft_top = single_session_draft_top(size);
    let typography = single_session_typography();
    let line_y = draft_top + typography.code_size * typography.code_line_height + 7.0;
    push_rect(
        vertices,
        Rect {
            x: PANEL_TITLE_LEFT_PADDING,
            y: line_y,
            width: (size.width as f32 - PANEL_TITLE_LEFT_PADDING * 2.0).max(1.0),
            height: 1.5,
        },
        COMPOSER_LINE_COLOR,
        size,
    );
}

pub(crate) fn push_native_activity_spinner(
    vertices: &mut Vec<Vertex>,
    size: PhysicalSize<u32>,
    tick: u64,
) {
    let typography = single_session_typography();
    let draft_top = single_session_draft_top(size);
    let center = [
        size.width as f32 - PANEL_TITLE_LEFT_PADDING - 12.0,
        draft_top - SINGLE_SESSION_STATUS_GAP + 7.0,
    ];
    let radius = (typography.meta_size * 0.54).clamp(5.0, 9.0);
    let thickness = 2.4;
    let segments = 12;
    let phase = (tick as usize) % segments;
    for segment in 0..segments {
        let age = (segment + segments - phase) % segments;
        let alpha_scale = if age == 0 {
            1.0
        } else {
            0.18 + (segments - age) as f32 / segments as f32 * 0.52
        };
        let mut color = if age == 0 {
            NATIVE_SPINNER_HEAD_COLOR
        } else {
            NATIVE_SPINNER_TRACK_COLOR
        };
        color[3] = (color[3] * alpha_scale).clamp(0.08, 1.0);
        let start =
            -std::f32::consts::FRAC_PI_2 + segment as f32 / segments as f32 * std::f32::consts::TAU;
        let end = start + std::f32::consts::TAU / segments as f32 * 0.64;
        push_spinner_segment(vertices, center, radius, thickness, start, end, color, size);
    }
}

fn push_spinner_segment(
    vertices: &mut Vec<Vertex>,
    center: [f32; 2],
    radius: f32,
    thickness: f32,
    start: f32,
    end: f32,
    color: [f32; 4],
    size: PhysicalSize<u32>,
) {
    let inner_radius = (radius - thickness).max(1.0);
    let outer_start = [
        center[0] + radius * start.cos(),
        center[1] + radius * start.sin(),
    ];
    let outer_end = [
        center[0] + radius * end.cos(),
        center[1] + radius * end.sin(),
    ];
    let inner_start = [
        center[0] + inner_radius * start.cos(),
        center[1] + inner_radius * start.sin(),
    ];
    let inner_end = [
        center[0] + inner_radius * end.cos(),
        center[1] + inner_radius * end.sin(),
    ];
    push_pixel_triangle(vertices, outer_start, outer_end, inner_end, color, size);
    push_pixel_triangle(vertices, outer_start, inner_end, inner_start, color, size);
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub(crate) struct SingleSessionTranscriptCardRun {
    pub(crate) line: usize,
    pub(crate) line_count: usize,
    pub(crate) style: SingleSessionLineStyle,
}

fn push_single_session_transcript_cards(
    vertices: &mut Vec<Vertex>,
    app: &SingleSessionApp,
    size: PhysicalSize<u32>,
) {
    let typography = single_session_typography();
    let line_height = typography.body_size * typography.body_line_height;
    let visible_lines = single_session_visible_styled_body(app, size);
    let width = (size.width as f32 - PANEL_TITLE_LEFT_PADDING * 2.0 + 12.0).max(1.0);

    for run in single_session_transcript_card_runs(&visible_lines) {
        let Some(color) = single_session_line_card_color(run.style) else {
            continue;
        };
        push_rounded_rect(
            vertices,
            Rect {
                x: PANEL_TITLE_LEFT_PADDING - 6.0,
                y: PANEL_BODY_TOP_PADDING + run.line as f32 * line_height + 3.0,
                width,
                height: (run.line_count as f32 * line_height - 6.0).max(1.0),
            },
            7.0,
            color,
            size,
        );
    }
}

pub(crate) fn push_single_session_streaming_shimmer(
    vertices: &mut Vec<Vertex>,
    app: &SingleSessionApp,
    size: PhysicalSize<u32>,
    tick: u64,
) {
    let Some(shimmer) = single_session_streaming_shimmer(app, size, tick) else {
        return;
    };

    push_rect(
        vertices,
        shimmer.soft_rect,
        STREAMING_SHIMMER_SOFT_COLOR,
        size,
    );
    push_rect(
        vertices,
        shimmer.core_rect,
        STREAMING_SHIMMER_CORE_COLOR,
        size,
    );
}

#[derive(Clone, Copy)]
pub(crate) struct SingleSessionStreamingShimmer {
    pub(crate) soft_rect: Rect,
    pub(crate) core_rect: Rect,
}

pub(crate) fn single_session_streaming_shimmer(
    app: &SingleSessionApp,
    size: PhysicalSize<u32>,
    tick: u64,
) -> Option<SingleSessionStreamingShimmer> {
    if app.streaming_response.trim().is_empty() {
        return None;
    }

    let visible_lines = single_session_visible_styled_body(app, size);
    let line_index = visible_lines.iter().rposition(is_shimmer_anchor_line)?;

    let typography = single_session_typography();
    let line_height = typography.body_size * typography.body_line_height;
    let text_columns = visible_lines[line_index].text.chars().count().max(8) as f32;
    let text_width = (text_columns * single_session_body_char_width())
        .min((size.width as f32 - PANEL_TITLE_LEFT_PADDING * 2.0).max(1.0));
    let lane_width = (text_width + 56.0).max(120.0);
    let shimmer_width = lane_width.min(180.0).max(72.0);
    let phase = (tick % 48) as f32 / 48.0;
    let travel = lane_width + shimmer_width;
    let head_x = PANEL_TITLE_LEFT_PADDING - shimmer_width + phase * travel;
    let y = PANEL_BODY_TOP_PADDING + line_index as f32 * line_height + line_height * 0.12;
    let height = line_height * 0.76;

    let soft_rect = Rect {
        x: head_x,
        y,
        width: shimmer_width,
        height,
    };
    let core_rect = Rect {
        x: head_x + shimmer_width * 0.34,
        y,
        width: shimmer_width * 0.32,
        height,
    };
    Some(SingleSessionStreamingShimmer {
        soft_rect,
        core_rect,
    })
}

fn push_single_session_scrollbar(
    vertices: &mut Vec<Vertex>,
    app: &SingleSessionApp,
    size: PhysicalSize<u32>,
    tick: u64,
) {
    let Some(metrics) = single_session_body_scroll_metrics(app, size, tick) else {
        return;
    };
    let track_top = PANEL_BODY_TOP_PADDING + 4.0;
    let track_bottom = single_session_body_bottom(size) - 4.0;
    let track_height = (track_bottom - track_top).max(1.0);
    let x = size.width as f32 - PANEL_TITLE_LEFT_PADDING - 4.0;
    let thumb_height = (metrics.visible_lines as f32 / metrics.total_lines as f32 * track_height)
        .clamp(28.0, track_height);
    let travel = (track_height - thumb_height).max(0.0);
    let scroll_fraction = metrics.scroll_lines as f32 / metrics.max_scroll_lines.max(1) as f32;
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

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub(crate) struct SingleSessionBodyScrollMetrics {
    pub(crate) total_lines: usize,
    pub(crate) visible_lines: usize,
    pub(crate) scroll_lines: usize,
    pub(crate) max_scroll_lines: usize,
}

pub(crate) fn single_session_body_scroll_metrics(
    app: &SingleSessionApp,
    size: PhysicalSize<u32>,
    tick: u64,
) -> Option<SingleSessionBodyScrollMetrics> {
    if app.is_empty_fresh_session() {
        return None;
    }
    let typography = single_session_typography();
    let line_height = typography.body_size * typography.body_line_height;
    let available_height =
        (single_session_body_bottom(size) - PANEL_BODY_TOP_PADDING).max(line_height);
    let visible_lines = ((available_height / line_height).floor() as usize).max(1);
    let total_lines = app.body_styled_lines_for_tick(tick).len();
    let max_scroll_lines = total_lines.saturating_sub(visible_lines);
    (max_scroll_lines > 0).then_some(SingleSessionBodyScrollMetrics {
        total_lines,
        visible_lines,
        scroll_lines: app.body_scroll_lines.min(max_scroll_lines),
        max_scroll_lines,
    })
}

fn is_shimmer_anchor_line(line: &SingleSessionStyledLine) -> bool {
    !line.text.trim().is_empty() && is_assistant_rendered_style(line.style)
}

fn is_assistant_rendered_style(style: SingleSessionLineStyle) -> bool {
    matches!(
        style,
        SingleSessionLineStyle::Assistant
            | SingleSessionLineStyle::AssistantHeading
            | SingleSessionLineStyle::AssistantQuote
            | SingleSessionLineStyle::AssistantTable
            | SingleSessionLineStyle::AssistantLink
            | SingleSessionLineStyle::Code
    )
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
            Some(run) if run.style == styled_line.style && run.line + run.line_count == line => {
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
        SingleSessionLineStyle::Code => Some(CODE_BLOCK_BACKGROUND_COLOR),
        SingleSessionLineStyle::AssistantQuote => Some(QUOTE_CARD_BACKGROUND_COLOR),
        SingleSessionLineStyle::AssistantTable => Some(TABLE_CARD_BACKGROUND_COLOR),
        SingleSessionLineStyle::Tool => Some(TOOL_CARD_BACKGROUND_COLOR),
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
    let typography = single_session_typography();
    let line_height = typography.body_size * typography.body_line_height;
    let char_width = single_session_body_char_width();
    let visible_lines = single_session_visible_body(app, size);
    for segment in app.selection_segments(&visible_lines) {
        let selected_columns = segment
            .end_column
            .saturating_sub(segment.start_column)
            .max(1);
        push_rect(
            vertices,
            Rect {
                x: PANEL_TITLE_LEFT_PADDING - 2.0 + segment.start_column as f32 * char_width,
                y: PANEL_BODY_TOP_PADDING + segment.line as f32 * line_height,
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
        let y = single_session_draft_top(size) + run.line_top;
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
    let draft_top = single_session_draft_top(size);
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

pub(crate) fn single_session_draft_top(size: PhysicalSize<u32>) -> f32 {
    (size.height as f32 - SINGLE_SESSION_DRAFT_TOP_OFFSET).max(112.0)
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

pub(crate) fn single_session_text_key_for_tick(
    app: &SingleSessionApp,
    size: PhysicalSize<u32>,
    tick: u64,
) -> SingleSessionTextKey {
    let hide_startup_chrome = app.is_empty_fresh_session();
    let body = single_session_visible_styled_body_for_tick(app, size, tick);
    let (welcome_hero, welcome_hint, body) = if hide_startup_chrome {
        split_welcome_hero_lines(body)
    } else {
        (String::new(), Vec::new(), body)
    };
    SingleSessionTextKey {
        size: (size.width, size.height),
        title: if hide_startup_chrome {
            String::new()
        } else {
            app.header_title()
        },
        version: if hide_startup_chrome {
            String::new()
        } else {
            desktop_header_version_label()
        },
        welcome_hero,
        welcome_hint,
        activity_active: app.has_activity_indicator(),
        body,
        draft: visualize_composer_whitespace(&app.composer_text()),
        status: app.composer_status_line_for_tick(tick),
    }
}

fn split_welcome_hero_lines(
    lines: Vec<SingleSessionStyledLine>,
) -> (
    String,
    Vec<SingleSessionStyledLine>,
    Vec<SingleSessionStyledLine>,
) {
    let hero = lines
        .into_iter()
        .find(|line| !line.text.trim().is_empty())
        .map(|line| line.text)
        .unwrap_or_default();
    (hero, Vec::new(), Vec::new())
}

pub(crate) fn single_session_text_buffers_from_key(
    key: &SingleSessionTextKey,
    size: PhysicalSize<u32>,
    font_system: &mut FontSystem,
) -> Vec<Buffer> {
    let typography = single_session_typography();
    let content_width = (size.width as f32 - PANEL_TITLE_LEFT_PADDING * 2.0).max(1.0);

    let prompt_height =
        (size.height as f32 - single_session_draft_top(size) - SINGLE_SESSION_STATUS_GAP - 18.0)
            .max(typography.code_size * typography.code_line_height * 2.0);
    let hero_font_size = welcome_hero_font_size(&key.welcome_hero, size);

    vec![
        single_session_text_buffer(
            font_system,
            &key.title,
            typography.title_size,
            typography.title_size * typography.meta_line_height,
            content_width,
            48.0,
        ),
        single_session_styled_text_buffer(
            font_system,
            &key.body,
            typography.body_size,
            typography.body_size * typography.body_line_height,
            content_width,
            (size.height as f32 - 150.0).max(1.0),
        ),
        single_session_text_buffer(
            font_system,
            &key.draft,
            typography.code_size,
            typography.code_size * typography.code_line_height,
            content_width,
            prompt_height,
        ),
        single_session_text_buffer(
            font_system,
            &key.status,
            typography.meta_size,
            typography.meta_size * typography.meta_line_height,
            content_width,
            28.0,
        ),
        single_session_text_buffer(
            font_system,
            &key.version,
            typography.meta_size,
            typography.meta_size * typography.meta_line_height,
            content_width,
            24.0,
        ),
        single_session_nowrap_text_buffer(
            font_system,
            key.welcome_hero.trim(),
            hero_font_size,
            hero_font_size * 1.08,
            size.width as f32 * 0.64,
            hero_font_size * 1.25,
        ),
    ]
}

fn welcome_hero_font_size(hero: &str, size: PhysicalSize<u32>) -> f32 {
    let width = size.width as f32;
    let height = size.height as f32;
    let chars = hero.trim().chars().count().max(1) as f32;
    let target_width = width * 0.50;
    (target_width / (chars * 0.56)).clamp(42.0, height * 0.18)
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
    let typography = single_session_typography();
    let line_height = typography.body_size * typography.body_line_height;
    let available_height =
        (single_session_body_bottom(size) - PANEL_BODY_TOP_PADDING).max(line_height);
    let visible_lines = ((available_height / line_height).floor() as usize).max(1);
    let mut lines = app.body_styled_lines_for_tick(tick);
    if app.is_empty_fresh_session() {
        lines = center_fresh_startup_lines(lines, size, visible_lines);
    }
    if lines.len() <= visible_lines {
        return lines;
    }

    let max_scroll = lines.len().saturating_sub(visible_lines);
    let scroll = app.body_scroll_lines.min(max_scroll);
    let end = lines.len().saturating_sub(scroll);
    let start = end.saturating_sub(visible_lines);
    lines[start..end].to_vec()
}

fn center_fresh_startup_lines(
    lines: Vec<SingleSessionStyledLine>,
    size: PhysicalSize<u32>,
    visible_lines: usize,
) -> Vec<SingleSessionStyledLine> {
    let top_padding = visible_lines.saturating_sub(lines.len()) / 3;
    let indent = fresh_startup_indent(size);
    let mut centered = Vec::with_capacity(top_padding + lines.len());
    centered.extend((0..top_padding).map(|_| SingleSessionStyledLine {
        text: String::new(),
        style: SingleSessionLineStyle::Blank,
    }));
    centered.extend(lines.into_iter().map(|mut line| {
        if !line.text.is_empty() {
            line.text = format!("{indent}{}", line.text);
        }
        line
    }));
    centered
}

fn fresh_startup_indent(size: PhysicalSize<u32>) -> String {
    let typography = single_session_typography();
    let approximate_char_width = typography.body_size * 0.58;
    let content_width = (size.width as f32 - PANEL_TITLE_LEFT_PADDING * 2.0).max(1.0);
    let columns = (content_width / approximate_char_width).floor().max(0.0) as usize;
    let target_text_width = 36;
    " ".repeat(columns.saturating_sub(target_text_width) / 2)
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
    let typography = single_session_typography();
    typography.body_size * 0.58
}

pub(crate) fn single_session_body_bottom(size: PhysicalSize<u32>) -> f32 {
    single_session_draft_top(size) - SINGLE_SESSION_STATUS_GAP - 12.0
}

fn single_session_text_buffer(
    font_system: &mut FontSystem,
    text: &str,
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
        Attrs::new().family(Family::Name(SINGLE_SESSION_FONT_FAMILY)),
        Shaping::Basic,
    );
    buffer.shape_until_scroll(font_system);
    buffer
}

fn single_session_nowrap_text_buffer(
    font_system: &mut FontSystem,
    text: &str,
    font_size: f32,
    line_height: f32,
    width: f32,
    height: f32,
) -> Buffer {
    let mut buffer = Buffer::new(font_system, Metrics::new(font_size, line_height));
    buffer.set_size(font_system, width, height);
    buffer.set_wrap(font_system, Wrap::None);
    buffer.set_text(
        font_system,
        text,
        Attrs::new().family(Family::Name(SINGLE_SESSION_FONT_FAMILY)),
        Shaping::Basic,
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
) -> Buffer {
    let mut buffer = Buffer::new(font_system, Metrics::new(font_size, line_height));
    buffer.set_size(font_system, width, height);
    let segments = single_session_styled_text_segments(lines);
    buffer.set_rich_text(
        font_system,
        segments
            .iter()
            .map(|(text, color)| (text.as_str(), single_session_color_attrs(*color))),
        Shaping::Basic,
    );
    buffer.shape_until_scroll(font_system);
    buffer
}

pub(crate) fn single_session_styled_text_segments(
    lines: &[SingleSessionStyledLine],
) -> Vec<(String, TextColor)> {
    let mut segments = Vec::new();
    for (index, line) in lines.iter().enumerate() {
        if !line.text.is_empty() {
            if line.style == SingleSessionLineStyle::User {
                push_user_prompt_segments(&mut segments, &line.text);
            } else {
                segments.push((line.text.clone(), single_session_line_color(line.style)));
            }
        }
        if index + 1 < lines.len() {
            segments.push((
                "\n".to_string(),
                single_session_line_color(SingleSessionLineStyle::Blank),
            ));
        }
    }
    if segments.is_empty() {
        segments.push((
            String::new(),
            single_session_line_color(SingleSessionLineStyle::Blank),
        ));
    }
    segments
}

fn push_user_prompt_segments(segments: &mut Vec<(String, TextColor)>, line: &str) {
    let Some((number, text)) = line.split_once("  ") else {
        segments.push((
            line.to_string(),
            single_session_line_color(SingleSessionLineStyle::User),
        ));
        return;
    };
    let Ok(turn) = number.parse::<usize>() else {
        segments.push((
            line.to_string(),
            single_session_line_color(SingleSessionLineStyle::User),
        ));
        return;
    };

    segments.push((number.to_string(), user_prompt_number_color(turn)));
    segments.push(("› ".to_string(), text_color(USER_PROMPT_ACCENT_COLOR)));
    segments.push((
        text.to_string(),
        single_session_line_color(SingleSessionLineStyle::User),
    ));
}

fn single_session_color_attrs(color: TextColor) -> Attrs<'static> {
    Attrs::new()
        .family(Family::Name(SINGLE_SESSION_FONT_FAMILY))
        .color(color)
}

pub(crate) fn user_prompt_number_color(turn: usize) -> TextColor {
    let index = turn.saturating_sub(1) % USER_PROMPT_NUMBER_COLORS.len();
    text_color(USER_PROMPT_NUMBER_COLORS[index])
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
    if buffers.len() < 5 {
        return Vec::new();
    }

    let left = PANEL_TITLE_LEFT_PADDING;
    let right = size.width.saturating_sub(PANEL_TITLE_LEFT_PADDING as u32) as i32;
    let bottom = size.height.saturating_sub(PANEL_TITLE_TOP_PADDING as u32) as i32;
    let draft_top = single_session_draft_top(size);
    let version_left = (size.width as f32 * 0.42).max(left + 220.0);

    vec![
        TextArea {
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
        },
        TextArea {
            buffer: &buffers[4],
            left: version_left,
            top: PANEL_TITLE_TOP_PADDING + 3.0,
            scale: 1.0,
            bounds: TextBounds {
                left: 0,
                top: 0,
                right,
                bottom: 64,
            },
            default_color: text_color(META_TEXT_COLOR),
        },
        TextArea {
            buffer: &buffers[1],
            left,
            top: PANEL_BODY_TOP_PADDING,
            scale: 1.0,
            bounds: TextBounds {
                left: 0,
                top: 0,
                right,
                bottom: single_session_body_bottom(size) as i32,
            },
            default_color: text_color(ASSISTANT_TEXT_COLOR),
        },
        TextArea {
            buffer: &buffers[3],
            left,
            top: draft_top - SINGLE_SESSION_STATUS_GAP,
            scale: 1.0,
            bounds: TextBounds {
                left: 0,
                top: (draft_top - SINGLE_SESSION_STATUS_GAP) as i32,
                right,
                bottom: draft_top as i32,
            },
            default_color: text_color(PANEL_SECTION_COLOR),
        },
        TextArea {
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
        },
    ]
}

fn visualize_composer_whitespace(text: &str) -> String {
    text.to_string()
}

pub(crate) fn desktop_header_version_label() -> String {
    let version = option_env!("JCODE_DESKTOP_VERSION").unwrap_or(env!("CARGO_PKG_VERSION"));
    let binary = std::env::current_exe()
        .ok()
        .map(|path| path.display().to_string())
        .unwrap_or_else(|| "unknown binary".to_string());
    format!("{binary} · {version}")
}

pub(crate) fn text_color(color: [f32; 4]) -> TextColor {
    TextColor::rgba(
        (color[0].clamp(0.0, 1.0) * 255.0).round() as u8,
        (color[1].clamp(0.0, 1.0) * 255.0).round() as u8,
        (color[2].clamp(0.0, 1.0) * 255.0).round() as u8,
        (color[3].clamp(0.0, 1.0) * 255.0).round() as u8,
    )
}
