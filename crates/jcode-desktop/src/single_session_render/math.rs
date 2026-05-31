//! Pure math/color helpers for single_session_render.
//! Extracted leaf functions with no rendering-state dependencies.

use super::Rect;

pub(super) fn ease_in_out_cubic(t: f32) -> f32 {
    if t < 0.5 {
        4.0 * t * t * t
    } else {
        1.0 - (-2.0 * t + 2.0).powi(3) / 2.0
    }
}

pub(super) fn scaled_rect(rect: Rect, scale: f32) -> Rect {
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

pub(super) fn mix_rgba(left: [f32; 4], right: [f32; 4], amount: f32) -> [f32; 4] {
    let amount = amount.clamp(0.0, 1.0);
    [
        left[0] + (right[0] - left[0]) * amount,
        left[1] + (right[1] - left[1]) * amount,
        left[2] + (right[2] - left[2]) * amount,
        left[3] + (right[3] - left[3]) * amount,
    ]
}

pub(super) fn distance(a: [f32; 2], b: [f32; 2]) -> f32 {
    ((b[0] - a[0]).powi(2) + (b[1] - a[1]).powi(2)).sqrt()
}

pub(super) fn lerp_point(a: [f32; 2], b: [f32; 2], t: f32) -> [f32; 2] {
    [a[0] + (b[0] - a[0]) * t, a[1] + (b[1] - a[1]) * t]
}

pub(super) fn mix_color(a: [f32; 4], b: [f32; 4], t: f32) -> [f32; 4] {
    [
        a[0] + (b[0] - a[0]) * t,
        a[1] + (b[1] - a[1]) * t,
        a[2] + (b[2] - a[2]) * t,
        a[3] + (b[3] - a[3]) * t,
    ]
}

pub(super) fn transparent(mut color: [f32; 4]) -> [f32; 4] {
    color[3] = 0.0;
    color
}

pub(super) fn lerp_f32(start: f32, end: f32, progress: f32) -> f32 {
    start + (end - start) * progress
}

pub(super) fn ease_out_cubic_local(progress: f32) -> f32 {
    1.0 - (1.0 - progress.clamp(0.0, 1.0)).powi(3)
}
