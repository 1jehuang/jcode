//! Lucide icon vector drawing for single_session_render.

use super::{Rect, Vertex, push_stroke_segment};
use winit::dpi::PhysicalSize;

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub(super) enum LucideIcon {
    Bot,
    BookmarkCheck,
    CircleCheck,
    CirclePlay,
    CircleX,
    MessageSquare,
    Package,
    RefreshCw,
}

pub(super) fn push_lucide_icon(
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
