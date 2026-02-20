#![allow(dead_code)]

use super::info_widget;
use super::markdown;
use super::visual_debug::{
    self, FrameCaptureBuilder, ImageRegionCapture, InfoWidgetCapture, InfoWidgetSummary,
    MarginsCapture, MessageCapture, RenderTimingCapture, WidgetPlacementCapture,
};
use super::{is_unexpected_cache_miss, DisplayMessage, ProcessingStatus, TuiState};
use crate::message::ToolCall;
use ratatui::{
    prelude::*,
    widgets::{Block, Borders, Clear, Paragraph},
};
use std::collections::{hash_map::DefaultHasher, HashMap, VecDeque};
use std::hash::{Hash, Hasher};
use std::sync::atomic::{AtomicUsize, Ordering};
use std::sync::{Mutex, OnceLock};
use std::time::{Duration, Instant};

/// Last known max scroll value from the renderer. Updated each frame.
/// Scroll handlers use this to clamp scroll_offset and prevent overshoot.
static LAST_MAX_SCROLL: AtomicUsize = AtomicUsize::new(0);
/// Number of recovered panics while rendering the frame.
static DRAW_PANIC_COUNT: AtomicUsize = AtomicUsize::new(0);

/// Get the last known max scroll value (from the most recent render frame).
/// Returns 0 if no frame has been rendered yet.
pub fn last_max_scroll() -> usize {
    LAST_MAX_SCROLL.load(Ordering::Relaxed)
}

// Minimal color palette
const USER_COLOR: Color = Color::Rgb(138, 180, 248); // Soft blue (caret)
const AI_COLOR: Color = Color::Rgb(129, 199, 132); // Soft green (unused)
const TOOL_COLOR: Color = Color::Rgb(120, 120, 120); // Gray
const DIM_COLOR: Color = Color::Rgb(80, 80, 80); // Dimmer gray
const ACCENT_COLOR: Color = Color::Rgb(186, 139, 255); // Purple accent
const QUEUED_COLOR: Color = Color::Rgb(255, 193, 7); // Amber/yellow for queued
const ASAP_COLOR: Color = Color::Rgb(110, 210, 255); // Cyan for immediate send
const PENDING_COLOR: Color = Color::Rgb(180, 230, 140); // Light green for sent/awaiting injection
const USER_TEXT: Color = Color::Rgb(245, 245, 255); // Bright cool white (user messages)
const USER_BG: Color = Color::Rgb(35, 40, 50); // Subtle dark blue background for user
const AI_TEXT: Color = Color::Rgb(220, 220, 215); // Softer warm white (AI messages)
const HEADER_ICON_COLOR: Color = Color::Rgb(120, 210, 230); // Teal for session icon
const HEADER_NAME_COLOR: Color = Color::Rgb(190, 210, 235); // Soft blue-gray for JCode label
const HEADER_SESSION_COLOR: Color = Color::Rgb(255, 255, 255); // White for session name

// Spinner frames for animated status
const SPINNER_FRAMES: &[&str] = &["⠋", "⠙", "⠹", "⠸", "⠼", "⠴", "⠦", "⠧", "⠇", "⠏"];

const STARTUP_ASCII_STATUS_FPS: f32 = 12.0;
const STARTUP_ASCII_STATUS_SPINNER: &[&str] = &["|", "/", "-", "\\"];

const LUMINANCE: &[u8] = b".,-~:;=!*#$@";

fn render_donut(elapsed: f32, width: usize, height: usize) -> Vec<String> {
    let a = elapsed * 1.5;
    let b = elapsed * 0.8;
    let cos_a = a.cos();
    let sin_a = a.sin();
    let cos_b = b.cos();
    let sin_b = b.sin();

    let mut output = vec![vec![b' '; width]; height];
    let mut zbuffer = vec![vec![0.0f32; width]; height];

    let mut theta: f32 = 0.0;
    while theta < std::f32::consts::TAU {
        let cos_theta = theta.cos();
        let sin_theta = theta.sin();

        let mut phi: f32 = 0.0;
        while phi < std::f32::consts::TAU {
            let cos_phi = phi.cos();
            let sin_phi = phi.sin();

            let circle_x = 2.0 + cos_theta;
            let circle_y = sin_theta;

            let x =
                circle_x * (cos_b * cos_phi + sin_a * sin_b * sin_phi) - circle_y * cos_a * sin_b;
            let y =
                circle_x * (sin_b * cos_phi - sin_a * cos_b * sin_phi) + circle_y * cos_a * cos_b;
            let z = 5.0 + cos_a * circle_x * sin_phi + circle_y * sin_a;
            let ooz = 1.0 / z;

            let xp = (width as f32 / 2.0 + width as f32 * 0.35 * ooz * x) as isize;
            let yp = (height as f32 / 2.0 - height as f32 * 0.35 * ooz * y) as isize;

            let lum = cos_phi * cos_theta * sin_b - cos_a * cos_theta * sin_phi - sin_a * sin_theta
                + cos_b * (cos_a * sin_theta - cos_theta * sin_a * sin_phi);

            if xp >= 0
                && (xp as usize) < width
                && yp >= 0
                && (yp as usize) < height
                && ooz > zbuffer[yp as usize][xp as usize]
            {
                zbuffer[yp as usize][xp as usize] = ooz;
                let li = (lum * 8.0).max(0.0).min((LUMINANCE.len() - 1) as f32) as usize;
                output[yp as usize][xp as usize] = LUMINANCE[li];
            }

            phi += 0.02;
        }
        theta += 0.07;
    }

    output
        .into_iter()
        .map(|row| String::from_utf8(row).unwrap_or_default())
        .collect()
}

fn render_startup_animation(
    elapsed: f32,
    width: usize,
    height: usize,
    variant: usize,
) -> Vec<String> {
    match variant % 7 {
        0 => render_donut(elapsed, width, height),
        1 => render_globe(elapsed, width, height),
        2 => render_cube(elapsed, width, height),
        3 => render_mobius(elapsed, width, height),
        4 => render_octahedron(elapsed, width, height),
        5 => render_lorenz(elapsed, width, height),
        6 => render_dna_helix(elapsed, width, height),
        _ => render_donut(elapsed, width, height),
    }
}

fn render_globe(elapsed: f32, width: usize, height: usize) -> Vec<String> {
    let mut output = vec![vec![b' '; width]; height];
    let mut zbuffer = vec![vec![0.0f32; width]; height];
    let rot = elapsed * 0.8;
    let cos_r = rot.cos();
    let sin_r = rot.sin();
    let radius = (width.min(height * 2) as f32) * 0.35;
    let cx = width as f32 / 2.0;
    let cy = height as f32 / 2.0;

    let mut lat: f32 = -std::f32::consts::FRAC_PI_2;
    while lat < std::f32::consts::FRAC_PI_2 {
        let cos_lat = lat.cos();
        let sin_lat = lat.sin();
        let mut lon: f32 = 0.0;
        while lon < std::f32::consts::TAU {
            let cos_lon = lon.cos();
            let sin_lon = lon.sin();
            let x3 = cos_lat * sin_lon;
            let y3 = sin_lat;
            let z3 = cos_lat * cos_lon;
            let rx = x3 * cos_r + z3 * sin_r;
            let rz = -x3 * sin_r + z3 * cos_r;
            if rz < -0.1 {
                lon += 0.03;
                continue;
            }
            let xp = (cx + rx * radius) as isize;
            let yp = (cy - y3 * radius * 0.5) as isize;
            let lum = (rz + 1.0) * 0.5;
            if xp >= 0 && (xp as usize) < width && yp >= 0 && (yp as usize) < height {
                let depth = rz + 1.0;
                if depth > zbuffer[yp as usize][xp as usize] {
                    zbuffer[yp as usize][xp as usize] = depth;
                    let is_grid = (lat * 6.0).fract().abs() < 0.15
                        || ((lon + rot) * 6.0 / std::f32::consts::TAU).fract().abs() < 0.1;
                    if is_grid {
                        let li = (lum * (LUMINANCE.len() - 1) as f32)
                            .max(0.0)
                            .min((LUMINANCE.len() - 1) as f32)
                            as usize;
                        output[yp as usize][xp as usize] = LUMINANCE[li];
                    } else {
                        let li = (lum * 3.0).max(0.0).min(2.0) as usize;
                        output[yp as usize][xp as usize] = b".,:"[li];
                    }
                }
            }
            lon += 0.03;
        }
        lat += 0.03;
    }
    output
        .into_iter()
        .map(|row| String::from_utf8(row).unwrap_or_default())
        .collect()
}

fn rotate_xyz(x: f32, y: f32, z: f32, ax: f32, ay: f32, az: f32) -> (f32, f32, f32) {
    let (sx, cx) = ax.sin_cos();
    let (sy, cy) = ay.sin_cos();
    let (sz, cz) = az.sin_cos();
    let y1 = y * cx - z * sx;
    let z1 = y * sx + z * cx;
    let x1 = x * cy + z1 * sy;
    let z2 = -x * sy + z1 * cy;
    let x2 = x1 * cz - y1 * sz;
    let y2 = x1 * sz + y1 * cz;
    (x2, y2, z2)
}

fn project_3d(
    x: f32,
    y: f32,
    z: f32,
    width: usize,
    height: usize,
    cam_dist: f32,
) -> Option<(isize, isize, f32)> {
    let d = cam_dist + z;
    if d < 0.1 {
        return None;
    }
    let scale = cam_dist / d;
    let xp = (width as f32 / 2.0 + x * scale * height as f32 * 0.4) as isize;
    let yp = (height as f32 / 2.0 - y * scale * height as f32 * 0.4) as isize;
    Some((xp, yp, 1.0 / d))
}

fn draw_line_3d(
    output: &mut [Vec<u8>],
    zbuffer: &mut [Vec<f32>],
    x0: f32,
    y0: f32,
    z0: f32,
    x1: f32,
    y1: f32,
    z1: f32,
    width: usize,
    height: usize,
    cam_dist: f32,
    ch: u8,
) {
    let steps = 40;
    for i in 0..=steps {
        let t = i as f32 / steps as f32;
        let x = x0 + (x1 - x0) * t;
        let y = y0 + (y1 - y0) * t;
        let z = z0 + (z1 - z0) * t;
        if let Some((xp, yp, depth)) = project_3d(x, y, z, width, height, cam_dist) {
            if xp >= 0
                && (xp as usize) < width
                && yp >= 0
                && (yp as usize) < height
                && depth > zbuffer[yp as usize][xp as usize]
            {
                zbuffer[yp as usize][xp as usize] = depth;
                output[yp as usize][xp as usize] = ch;
            }
        }
    }
}

fn render_cube(elapsed: f32, width: usize, height: usize) -> Vec<String> {
    let mut output = vec![vec![b' '; width]; height];
    let mut zbuffer = vec![vec![0.0f32; width]; height];
    let ax = elapsed * 0.7;
    let ay = elapsed * 1.1;
    let az = elapsed * 0.3;
    let cam_dist = 5.0;
    let verts: [(f32, f32, f32); 8] = [
        (-1.0, -1.0, -1.0),
        (1.0, -1.0, -1.0),
        (1.0, 1.0, -1.0),
        (-1.0, 1.0, -1.0),
        (-1.0, -1.0, 1.0),
        (1.0, -1.0, 1.0),
        (1.0, 1.0, 1.0),
        (-1.0, 1.0, 1.0),
    ];
    let edges: [(usize, usize); 12] = [
        (0, 1),
        (1, 2),
        (2, 3),
        (3, 0),
        (4, 5),
        (5, 6),
        (6, 7),
        (7, 4),
        (0, 4),
        (1, 5),
        (2, 6),
        (3, 7),
    ];
    let rotated: Vec<(f32, f32, f32)> = verts
        .iter()
        .map(|&(x, y, z)| rotate_xyz(x, y, z, ax, ay, az))
        .collect();
    for &(a, b) in &edges {
        let (x0, y0, z0) = rotated[a];
        let (x1, y1, z1) = rotated[b];
        draw_line_3d(
            &mut output,
            &mut zbuffer,
            x0,
            y0,
            z0,
            x1,
            y1,
            z1,
            width,
            height,
            cam_dist,
            b'#',
        );
    }
    for &(x, y, z) in &rotated {
        if let Some((xp, yp, _)) = project_3d(x, y, z, width, height, cam_dist) {
            if xp >= 0 && (xp as usize) < width && yp >= 0 && (yp as usize) < height {
                output[yp as usize][xp as usize] = b'@';
            }
        }
    }
    output
        .into_iter()
        .map(|row| String::from_utf8(row).unwrap_or_default())
        .collect()
}

fn render_mobius(elapsed: f32, width: usize, height: usize) -> Vec<String> {
    let mut output = vec![vec![b' '; width]; height];
    let mut zbuffer = vec![vec![0.0f32; width]; height];
    let rot = elapsed * 0.6;
    let cam_dist = 6.0;
    let mut u: f32 = 0.0;
    while u < std::f32::consts::TAU {
        let mut v: f32 = -0.4;
        while v <= 0.4 {
            let half_u = u / 2.0;
            let x = (1.0 + v * half_u.cos()) * u.cos();
            let y = (1.0 + v * half_u.cos()) * u.sin();
            let z = v * half_u.sin();
            let (rx, ry, rz) = rotate_xyz(x, y, z, elapsed * 0.3, rot, 0.0);
            if let Some((xp, yp, depth)) = project_3d(rx, ry, rz, width, height, cam_dist) {
                if xp >= 0
                    && (xp as usize) < width
                    && yp >= 0
                    && (yp as usize) < height
                    && depth > zbuffer[yp as usize][xp as usize]
                {
                    zbuffer[yp as usize][xp as usize] = depth;
                    let nx = half_u.cos() * u.cos();
                    let ny = half_u.cos() * u.sin();
                    let nz = half_u.sin();
                    let (rnx, rny, _) = rotate_xyz(nx, ny, nz, elapsed * 0.3, rot, 0.0);
                    let lum = (rnx * 0.5 + rny * 0.5 + 0.5).clamp(0.0, 1.0);
                    let li = (lum * (LUMINANCE.len() - 1) as f32) as usize;
                    output[yp as usize][xp as usize] = LUMINANCE[li.min(LUMINANCE.len() - 1)];
                }
            }
            v += 0.04;
        }
        u += 0.03;
    }
    output
        .into_iter()
        .map(|row| String::from_utf8(row).unwrap_or_default())
        .collect()
}

fn render_octahedron(elapsed: f32, width: usize, height: usize) -> Vec<String> {
    let mut output = vec![vec![b' '; width]; height];
    let mut zbuffer = vec![vec![0.0f32; width]; height];
    let ax = elapsed * 0.9;
    let ay = elapsed * 0.6;
    let az = elapsed * 0.4;
    let cam_dist = 4.5;
    let s = 1.3;
    let verts: [(f32, f32, f32); 6] = [
        (s, 0.0, 0.0),
        (-s, 0.0, 0.0),
        (0.0, s, 0.0),
        (0.0, -s, 0.0),
        (0.0, 0.0, s),
        (0.0, 0.0, -s),
    ];
    let edges: [(usize, usize); 12] = [
        (0, 2),
        (0, 3),
        (0, 4),
        (0, 5),
        (1, 2),
        (1, 3),
        (1, 4),
        (1, 5),
        (2, 4),
        (4, 3),
        (3, 5),
        (5, 2),
    ];
    let rotated: Vec<(f32, f32, f32)> = verts
        .iter()
        .map(|&(x, y, z)| rotate_xyz(x, y, z, ax, ay, az))
        .collect();
    for &(a, b) in &edges {
        let (x0, y0, z0) = rotated[a];
        let (x1, y1, z1) = rotated[b];
        draw_line_3d(
            &mut output,
            &mut zbuffer,
            x0,
            y0,
            z0,
            x1,
            y1,
            z1,
            width,
            height,
            cam_dist,
            b'=',
        );
    }
    for &(x, y, z) in &rotated {
        if let Some((xp, yp, _)) = project_3d(x, y, z, width, height, cam_dist) {
            if xp >= 0 && (xp as usize) < width && yp >= 0 && (yp as usize) < height {
                output[yp as usize][xp as usize] = b'@';
            }
        }
    }
    output
        .into_iter()
        .map(|row| String::from_utf8(row).unwrap_or_default())
        .collect()
}

fn render_lorenz(elapsed: f32, width: usize, height: usize) -> Vec<String> {
    let mut output = vec![vec![b' '; width]; height];
    let sigma: f32 = 10.0;
    let rho: f32 = 28.0;
    let beta: f32 = 8.0 / 3.0;
    let dt: f32 = 0.005;
    let mut x: f32 = 0.1;
    let mut y: f32 = 0.0;
    let mut z: f32 = 0.0;
    let rot = elapsed * 0.3;
    let cos_r = rot.cos();
    let sin_r = rot.sin();
    let scale_x = width as f32 / 55.0;
    let scale_y = height as f32 / 55.0;
    let cx = width as f32 / 2.0;
    let cy = height as f32 * 0.65;
    let total_steps = 4000 + (elapsed * 500.0) as usize;
    let trail_start = total_steps.saturating_sub(3000);
    for step in 0..total_steps {
        let dx = sigma * (y - x);
        let dy = x * (rho - z) - y;
        let dz = x * y - beta * z;
        x += dx * dt;
        y += dy * dt;
        z += dz * dt;
        if step >= trail_start {
            let rx = x * cos_r - y * sin_r;
            let ry_unused = x * sin_r + y * cos_r;
            let _ = ry_unused;
            let xp = (cx + rx * scale_x) as isize;
            let yp = (cy - z * scale_y) as isize;
            if xp >= 0 && (xp as usize) < width && yp >= 0 && (yp as usize) < height {
                let age = (step - trail_start) as f32 / 3000.0;
                let li = (age * (LUMINANCE.len() - 1) as f32) as usize;
                let ch = LUMINANCE[li.min(LUMINANCE.len() - 1)];
                if ch > output[yp as usize][xp as usize] || output[yp as usize][xp as usize] == b' '
                {
                    output[yp as usize][xp as usize] = ch;
                }
            }
        }
    }
    output
        .into_iter()
        .map(|row| String::from_utf8(row).unwrap_or_default())
        .collect()
}

fn render_dna_helix(elapsed: f32, width: usize, height: usize) -> Vec<String> {
    let mut output = vec![vec![b' '; width]; height];
    let mut zbuffer = vec![vec![0.0f32; width]; height];
    let cx = width as f32 / 2.0;
    let radius = width as f32 * 0.2;
    let speed = elapsed * 2.0;
    for row in 0..height {
        let t = row as f32 / height as f32 * 4.0 * std::f32::consts::PI + speed;
        let x1 = t.cos();
        let z1 = t.sin();
        let x2 = (t + std::f32::consts::PI).cos();
        let z2 = (t + std::f32::consts::PI).sin();
        let xp1 = (cx + x1 * radius) as isize;
        let xp2 = (cx + x2 * radius) as isize;
        let d1 = z1 * 0.5 + 0.5;
        let d2 = z2 * 0.5 + 0.5;
        if xp1 >= 0 && (xp1 as usize) < width && d1 > zbuffer[row][xp1 as usize] {
            zbuffer[row][xp1 as usize] = d1;
            let li = (d1 * (LUMINANCE.len() - 1) as f32) as usize;
            output[row][xp1 as usize] = LUMINANCE[li.min(LUMINANCE.len() - 1)];
        }
        if xp2 >= 0 && (xp2 as usize) < width && d2 > zbuffer[row][xp2 as usize] {
            zbuffer[row][xp2 as usize] = d2;
            let li = (d2 * (LUMINANCE.len() - 1) as f32) as usize;
            output[row][xp2 as usize] = LUMINANCE[li.min(LUMINANCE.len() - 1)];
        }
        if (row % 3) == 0 {
            let left = xp1.min(xp2).max(0) as usize;
            let right = xp1.max(xp2).max(0) as usize;
            if left < width && right < width {
                for col in left..=right {
                    if output[row][col] == b' ' {
                        let frac = if right > left {
                            (col - left) as f32 / (right - left) as f32
                        } else {
                            0.5
                        };
                        let d = d1 + (d2 - d1) * frac;
                        if d > zbuffer[row][col] * 0.9 {
                            output[row][col] = b'-';
                        }
                    }
                }
            }
        }
    }
    output
        .into_iter()
        .map(|row| String::from_utf8(row).unwrap_or_default())
        .collect()
}

/// Duration of the startup fade-in animation in seconds
const HEADER_ANIM_DURATION: f32 = 1.5;

/// Speed of the continuous chroma wave (lower = slower)
const CHROMA_SPEED: f32 = 0.15;

/// Convert HSL to RGB (h in 0-360, s and l in 0-1)
fn hsl_to_rgb(h: f32, s: f32, l: f32) -> (u8, u8, u8) {
    let c = (1.0 - (2.0 * l - 1.0).abs()) * s;
    let h_prime = h / 60.0;
    let x = c * (1.0 - (h_prime % 2.0 - 1.0).abs());
    let m = l - c / 2.0;

    let (r1, g1, b1) = match h_prime as u32 {
        0 => (c, x, 0.0),
        1 => (x, c, 0.0),
        2 => (0.0, c, x),
        3 => (0.0, x, c),
        4 => (x, 0.0, c),
        _ => (c, 0.0, x),
    };

    (
        ((r1 + m) * 255.0).clamp(0.0, 255.0) as u8,
        ((g1 + m) * 255.0).clamp(0.0, 255.0) as u8,
        ((b1 + m) * 255.0).clamp(0.0, 255.0) as u8,
    )
}

/// Chroma color based on position and time - creates flowing rainbow wave
fn chroma_color(pos: f32, elapsed: f32, saturation: f32, lightness: f32) -> Color {
    // Hue shifts over time and varies by position
    // pos: 0.0-1.0 position in the text
    // Creates a wave that flows across the text
    let hue = ((pos * 60.0) + (elapsed * CHROMA_SPEED * 360.0)) % 360.0;
    let (r, g, b) = hsl_to_rgb(hue, saturation, lightness);
    Color::Rgb(r, g, b)
}

/// Calculate chroma color with fade-in from dim during startup
fn header_chroma_color(pos: f32, elapsed: f32) -> Color {
    let fade = ((elapsed / HEADER_ANIM_DURATION).clamp(0.0, 1.0)).powf(0.5);

    // During fade-in, transition from dim gray to full chroma
    let saturation = 0.75 * fade;
    let lightness = 0.3 + 0.35 * fade; // Start darker (0.3), end bright (0.65)

    chroma_color(pos, elapsed, saturation, lightness)
}

/// Calculate smooth animated color for the header (single color, no position)
fn header_animation_color(elapsed: f32) -> Color {
    header_chroma_color(0.5, elapsed)
}

fn header_fade_t(elapsed: f32, offset: f32) -> f32 {
    let t = ((elapsed - offset) / HEADER_ANIM_DURATION).clamp(0.0, 1.0);
    1.0 - (1.0 - t).powi(3)
}

fn header_fade_color(target: Color, elapsed: f32, offset: f32) -> Color {
    blend_color(DIM_COLOR, target, header_fade_t(elapsed, offset))
}

fn blend_color(from: Color, to: Color, t: f32) -> Color {
    let (fr, fg, fb) = match from {
        Color::Rgb(r, g, b) => (r as f32, g as f32, b as f32),
        _ => (80.0, 80.0, 80.0),
    };
    let (tr, tg, tb) = match to {
        Color::Rgb(r, g, b) => (r as f32, g as f32, b as f32),
        _ => (200.0, 200.0, 200.0),
    };
    let r = fr + (tr - fr) * t;
    let g = fg + (tg - fg) * t;
    let b = fb + (tb - fb) * t;
    Color::Rgb(
        r.clamp(0.0, 255.0) as u8,
        g.clamp(0.0, 255.0) as u8,
        b.clamp(0.0, 255.0) as u8,
    )
}

/// Chrome-style sweep highlight across header text.
fn header_chrome_color(base: Color, pos: f32, elapsed: f32, intensity: f32) -> Color {
    const HIGHLIGHT: Color = Color::Rgb(235, 245, 255);
    const SHADOW: Color = Color::Rgb(70, 80, 95);
    const SPEED: f32 = 0.12;
    const WIDTH: f32 = 0.22;

    let center = (elapsed * SPEED) % 1.0;
    let mut dist = (pos - center).abs();
    dist = dist.min(1.0 - dist);
    let shine = (1.0 - (dist / WIDTH).clamp(0.0, 1.0)).powf(2.4);

    let micro = ((pos * 12.0 + elapsed * 2.6).sin() * 0.5 + 0.5) * 0.12;
    let shimmer = (shine * 0.9 + micro).clamp(0.0, 1.0) * intensity;

    let shadow_center = (center + 0.5) % 1.0;
    let mut shadow_dist = (pos - shadow_center).abs();
    shadow_dist = shadow_dist.min(1.0 - shadow_dist);
    let shadow = (1.0 - (shadow_dist / (WIDTH * 1.2)).clamp(0.0, 1.0)).powf(2.0) * 0.16 * intensity;

    let darkened = blend_color(base, SHADOW, shadow);
    blend_color(darkened, HIGHLIGHT, shimmer)
}

/// Set alignment on a line only if it doesn't already have one set.
/// This allows markdown rendering to mark code blocks as left-aligned while
/// other content inherits the default alignment (e.g., centered mode).
pub(crate) fn align_if_unset(line: Line<'static>, align: Alignment) -> Line<'static> {
    if line.alignment.is_some() {
        line
    } else {
        line.alignment(align)
    }
}

/// Extract semantic version from full version string (e.g., "v0.1.0-dev (abc123)" -> "v0.1.0")
fn semver() -> &'static str {
    static SEMVER: OnceLock<String> = OnceLock::new();
    SEMVER.get_or_init(|| {
        let full = env!("JCODE_VERSION");
        // Extract just the version part (before any space or -dev suffix for display)
        if let Some(space_pos) = full.find(' ') {
            full[..space_pos].trim_end_matches("-dev").to_string()
        } else {
            full.trim_end_matches("-dev").to_string()
        }
    })
}

/// True when this process is running from the stable release binary path.
/// Only matches the explicit ~/.jcode/builds/stable/jcode path, NOT
/// ~/.local/bin/jcode (which is the latest build from source).
fn is_running_stable_release() -> bool {
    static IS_STABLE: OnceLock<bool> = OnceLock::new();
    *IS_STABLE.get_or_init(|| {
        // Use the raw symlink target (read_link), not canonicalize, to
        // distinguish ~/.local/bin/jcode -> target/release/jcode (latest)
        // from ~/.jcode/builds/stable/jcode -> builds/versions/XXX/jcode (release).
        let current_exe = match std::env::current_exe().ok() {
            Some(path) => path,
            None => return false,
        };

        // Check if we were launched via the stable symlink
        if let Ok(stable_path) = crate::build::stable_binary_path() {
            // Compare the symlink target (not canonical) to avoid
            // conflating target/release/jcode with the stable binary
            let stable_target =
                std::fs::read_link(&stable_path).unwrap_or_else(|_| stable_path.clone());
            let current_target =
                std::fs::read_link(&current_exe).unwrap_or_else(|_| current_exe.clone());
            if stable_target == current_target {
                return true;
            }
            // Also check canonical paths for when launched directly
            if let (Ok(stable_canon), Ok(current_canon)) = (
                std::fs::canonicalize(&stable_path),
                std::fs::canonicalize(&current_exe),
            ) {
                if stable_canon == current_canon
                    && !current_exe.to_string_lossy().contains("target/release")
                {
                    return true;
                }
            }
        }

        false
    })
}

/// Create a modern pill-style badge: ⟨ label ⟩
fn pill_badge(label: &str, color: Color) -> Vec<Span<'static>> {
    vec![
        Span::styled("  ", Style::default()),
        Span::styled("⟨ ", Style::default().fg(color)),
        Span::styled(label.to_string(), Style::default().fg(color)),
        Span::styled(" ⟩", Style::default().fg(color)),
    ]
}

/// Create a combined status badge with multiple colored items: ⟨item1·item2·item3⟩
fn multi_status_badge(items: &[(&str, Color)]) -> Vec<Span<'static>> {
    let mut spans = vec![
        Span::styled(" ", Style::default()),
        Span::styled("⟨", Style::default().fg(DIM_COLOR)),
    ];

    for (i, (label, color)) in items.iter().enumerate() {
        if i > 0 {
            spans.push(Span::styled("·", Style::default().fg(DIM_COLOR)));
        }
        spans.push(Span::styled(label.to_string(), Style::default().fg(*color)));
    }

    spans.push(Span::styled("⟩", Style::default().fg(DIM_COLOR)));
    spans
}

/// Create multi-color spans for the header line
fn header_spans(icon: &str, session: &str, model: &str, elapsed: f32) -> Vec<Span<'static>> {
    let segments = [
        (format!("{} ", icon), HEADER_ICON_COLOR, 0.00),
        ("JCode ".to_string(), HEADER_NAME_COLOR, 0.06),
        (
            format!("{} ", capitalize(session)),
            HEADER_SESSION_COLOR,
            0.12,
        ),
        ("· ".to_string(), DIM_COLOR, 0.18),
        (model.to_string(), header_animation_color(elapsed), 0.12),
    ];

    let total_chars: usize = segments
        .iter()
        .map(|(text, _, _)| text.chars().count())
        .sum();
    let total = total_chars.max(1);
    let mut spans = Vec::with_capacity(total_chars);
    let mut idx = 0usize;

    for (text, target, offset) in segments {
        let fade = header_fade_t(elapsed, offset);
        let base = header_fade_color(target, elapsed, offset);
        for ch in text.chars() {
            let pos = if total > 1 {
                idx as f32 / (total - 1) as f32
            } else {
                0.0
            };
            let color = header_chrome_color(base, pos, elapsed, fade);
            spans.push(Span::styled(ch.to_string(), Style::default().fg(color)));
            idx += 1;
        }
    }

    spans
}

/// Capitalize first letter of a string
fn capitalize(s: &str) -> String {
    let mut chars = s.chars();
    match chars.next() {
        None => String::new(),
        Some(first) => first.to_uppercase().chain(chars).collect(),
    }
}

/// Format model name nicely (e.g., "claude4.5opus" -> "Claude 4.5 Opus")
fn format_model_name(short: &str) -> String {
    // Handle OpenRouter models (format: provider/model)
    if short.contains('/') {
        return format!("OpenRouter: {}", short);
    }
    if short.contains("opus") {
        if short.contains("4.5") {
            return "Claude 4.5 Opus".to_string();
        }
        return "Claude Opus".to_string();
    }
    if short.contains("sonnet") {
        if short.contains("3.5") {
            return "Claude 3.5 Sonnet".to_string();
        }
        return "Claude Sonnet".to_string();
    }
    if short.contains("haiku") {
        return "Claude Haiku".to_string();
    }
    if short.starts_with("gpt") {
        return format_gpt_name(short);
    }
    short.to_string()
}

/// Format GPT-style model names for display (e.g., "gpt5.2codex" -> "GPT-5.2 Codex")
fn format_gpt_name(short: &str) -> String {
    let rest = short.trim_start_matches("gpt");
    if rest.is_empty() {
        return "GPT".to_string();
    }

    if let Some(idx) = rest.find("codex") {
        let version = &rest[..idx];
        if version.is_empty() {
            return "GPT Codex".to_string();
        }
        return format!("GPT-{} Codex", version);
    }

    format!("GPT-{}", rest)
}

/// Build the auth status line with colored dots for each provider
fn build_auth_status_line(auth: &crate::auth::AuthStatus, max_width: usize) -> Line<'static> {
    use crate::auth::AuthState;

    const GREEN: Color = Color::Rgb(100, 200, 100); // Available
    const YELLOW: Color = Color::Rgb(255, 200, 100); // Expired (may work)
    const GRAY: Color = Color::Rgb(80, 80, 80); // Not configured

    fn dot_color(state: AuthState) -> Color {
        match state {
            AuthState::Available => GREEN,
            AuthState::Expired => YELLOW,
            AuthState::NotConfigured => GRAY,
        }
    }

    fn dot_char(state: AuthState) -> &'static str {
        match state {
            AuthState::Available => "●",
            AuthState::Expired => "◐",
            AuthState::NotConfigured => "○",
        }
    }

    // Build full-length labels
    let anthropic_label = if auth.anthropic.has_oauth && auth.anthropic.has_api_key {
        "anthropic(oauth+key)"
    } else if auth.anthropic.has_oauth {
        "anthropic(oauth)"
    } else if auth.anthropic.has_api_key {
        "anthropic(key)"
    } else {
        "anthropic"
    };

    let openai_label = if auth.openai_has_oauth && auth.openai_has_api_key {
        "openai(oauth+key)"
    } else if auth.openai_has_oauth {
        "openai(oauth)"
    } else if auth.openai_has_api_key {
        "openai(key)"
    } else {
        "openai"
    };

    // Estimate full width: "● label ● openrouter ● label"
    let full_width = 2 + anthropic_label.len() + 3 + "openrouter".len() + 3 + openai_label.len();

    // Choose short labels if full doesn't fit
    let (anth_lbl, or_lbl, oai_lbl) = if full_width <= max_width {
        (anthropic_label, "openrouter", openai_label)
    } else {
        // Short form: drop auth method details
        ("anthropic", "openrouter", "openai")
    };

    let mut spans = Vec::new();
    spans.push(Span::styled(
        dot_char(auth.anthropic.state),
        Style::default().fg(dot_color(auth.anthropic.state)),
    ));
    spans.push(Span::styled(
        format!(" {} ", anth_lbl),
        Style::default().fg(DIM_COLOR),
    ));
    spans.push(Span::styled(
        dot_char(auth.openrouter),
        Style::default().fg(dot_color(auth.openrouter)),
    ));
    spans.push(Span::styled(
        format!(" {} ", or_lbl),
        Style::default().fg(DIM_COLOR),
    ));
    spans.push(Span::styled(
        dot_char(auth.openai),
        Style::default().fg(dot_color(auth.openai)),
    ));
    spans.push(Span::styled(
        format!(" {}", oai_lbl),
        Style::default().fg(DIM_COLOR),
    ));

    Line::from(spans)
}

/// Render context window as vertical list with smart grouping
/// Items < 5% are grouped by category (docs, msgs, etc.)
fn render_context_bar(
    info: &crate::prompt::ContextInfo,
    max_width: usize,
    context_limit: usize,
) -> Vec<Line<'static>> {
    const SYS_COLOR: Color = Color::Rgb(100, 140, 200);
    const DOCS_COLOR: Color = Color::Rgb(200, 160, 100);
    const TOOLS_COLOR: Color = Color::Rgb(100, 200, 200);
    const MSGS_COLOR: Color = Color::Rgb(138, 180, 248);
    const TOOL_IO_COLOR: Color = Color::Rgb(255, 183, 77);
    const OTHER_COLOR: Color = Color::Rgb(150, 150, 150);
    const EMPTY_COLOR: Color = Color::Rgb(50, 50, 50);

    const THRESHOLD: f64 = 5.0;
    let limit = context_limit.max(1);

    // Collect raw: (icon, label, tokens, color, category)
    let mut raw: Vec<(&str, String, usize, Color, &str)> = Vec::new();

    let sys = info.system_prompt_chars / 4;
    if sys > 0 {
        raw.push(("⚙", "system".into(), sys, SYS_COLOR, "system"));
    }

    if info.has_project_agents_md {
        raw.push((
            "📋",
            "AGENTS.md".into(),
            info.project_agents_md_chars / 4,
            DOCS_COLOR,
            "docs",
        ));
    }
    if info.has_project_claude_md {
        raw.push((
            "📝",
            "CLAUDE.md".into(),
            info.project_claude_md_chars / 4,
            DOCS_COLOR,
            "docs",
        ));
    }
    if info.has_global_agents_md {
        raw.push((
            "📋",
            "~/.AGENTS".into(),
            info.global_agents_md_chars / 4,
            DOCS_COLOR,
            "docs",
        ));
    }
    if info.has_global_claude_md {
        raw.push((
            "📝",
            "~/.CLAUDE".into(),
            info.global_claude_md_chars / 4,
            DOCS_COLOR,
            "docs",
        ));
    }

    if info.env_context_chars > 0 {
        raw.push((
            "🌍",
            "env".into(),
            info.env_context_chars / 4,
            OTHER_COLOR,
            "other",
        ));
    }
    if info.skills_chars > 0 {
        raw.push((
            "🔧",
            "skills".into(),
            info.skills_chars / 4,
            OTHER_COLOR,
            "other",
        ));
    }
    if info.selfdev_chars > 0 {
        raw.push((
            "🛠",
            "selfdev".into(),
            info.selfdev_chars / 4,
            OTHER_COLOR,
            "other",
        ));
    }

    if info.tool_defs_chars > 0 {
        let lbl = if info.tool_defs_count > 0 {
            format!("tools ({})", info.tool_defs_count)
        } else {
            "tools".into()
        };
        raw.push(("🔨", lbl, info.tool_defs_chars / 4, TOOLS_COLOR, "tools"));
    }
    if info.user_messages_chars > 0 {
        let lbl = if info.user_messages_count > 0 {
            format!("user ({})", info.user_messages_count)
        } else {
            "user".into()
        };
        raw.push(("👤", lbl, info.user_messages_chars / 4, MSGS_COLOR, "msgs"));
    }
    if info.assistant_messages_chars > 0 {
        let lbl = if info.assistant_messages_count > 0 {
            format!("assistant ({})", info.assistant_messages_count)
        } else {
            "assistant".into()
        };
        raw.push((
            "🤖",
            lbl,
            info.assistant_messages_chars / 4,
            MSGS_COLOR,
            "msgs",
        ));
    }
    if info.tool_calls_chars > 0 {
        let lbl = if info.tool_calls_count > 0 {
            format!("calls ({})", info.tool_calls_count)
        } else {
            "calls".into()
        };
        raw.push((
            "⚡",
            lbl,
            info.tool_calls_chars / 4,
            TOOL_IO_COLOR,
            "tool_io",
        ));
    }
    if info.tool_results_chars > 0 {
        let lbl = if info.tool_results_count > 0 {
            format!("results ({})", info.tool_results_count)
        } else {
            "results".into()
        };
        raw.push((
            "📤",
            lbl,
            info.tool_results_chars / 4,
            TOOL_IO_COLOR,
            "tool_io",
        ));
    }

    // Smart grouping
    let mut final_segs: Vec<(String, String, usize, Color)> = Vec::new();
    let mut grouped: std::collections::HashMap<&str, (usize, Vec<String>)> =
        std::collections::HashMap::new();

    for (icon, label, tokens, color, cat) in &raw {
        let pct = (*tokens as f64 / limit as f64) * 100.0;
        if pct >= THRESHOLD || *cat == "system" {
            final_segs.push((icon.to_string(), label.clone(), *tokens, *color));
        } else {
            let e = grouped.entry(*cat).or_insert((0, Vec::new()));
            e.0 += tokens;
            e.1.push(label.clone());
        }
    }

    for (cat, icon, color) in [
        ("docs", "📄", DOCS_COLOR),
        ("msgs", "💬", MSGS_COLOR),
        ("tools", "🔨", TOOLS_COLOR),
        ("tool_io", "⚡", TOOL_IO_COLOR),
        ("other", "📦", OTHER_COLOR),
    ] {
        if let Some((tokens, items)) = grouped.get(cat) {
            if *tokens > 0 {
                let lbl = if items.len() == 1 {
                    items[0].clone()
                } else {
                    format!("{} ({})", cat, items.len())
                };
                final_segs.push((icon.to_string(), lbl, *tokens, color));
            }
        }
    }

    final_segs.sort_by(|a, b| b.2.cmp(&a.2));

    let mut lines: Vec<Line<'static>> = Vec::new();
    let total: usize = final_segs.iter().map(|(_, _, t, _)| *t).sum();

    // Summary bar (top)
    let total_str = if total >= 1000 {
        format!("{}k", total / 1000)
    } else {
        format!("{}", total)
    };
    let limit_str = if limit >= 1000 {
        format!("{}k", limit / 1000)
    } else {
        format!("{}", limit)
    };
    let tail = format!("{}/{}", total_str, limit_str);
    let tail_len = tail.chars().count();

    let max_bar = max_width.saturating_sub(tail_len + 3); // "[" + bar + "] " + tail
    let sum_w = 36.min(max_bar).max(10);
    let used_w = ((total as f64 / limit as f64) * sum_w as f64)
        .ceil()
        .max(if total > 0 { 1.0 } else { 0.0 })
        .min(sum_w as f64) as usize;
    let empty_w = sum_w.saturating_sub(used_w);

    let mut bar: Vec<Span<'static>> = vec![Span::styled("[", Style::default().fg(DIM_COLOR))];
    let mut rem = used_w;
    for (_, _, t, c) in &final_segs {
        if rem == 0 || total == 0 {
            break;
        }
        let w = ((*t as f64 / total as f64) * used_w as f64)
            .round()
            .min(rem as f64) as usize;
        if w > 0 {
            bar.push(Span::styled("█".repeat(w), Style::default().fg(*c)));
            rem -= w;
        }
    }
    if rem > 0 && !final_segs.is_empty() {
        bar.push(Span::styled(
            "█".repeat(rem),
            Style::default().fg(final_segs.last().unwrap().3),
        ));
    }
    if empty_w > 0 {
        bar.push(Span::styled(
            "░".repeat(empty_w),
            Style::default().fg(EMPTY_COLOR),
        ));
    }
    bar.push(Span::styled("] ", Style::default().fg(DIM_COLOR)));
    bar.push(Span::styled(tail, Style::default().fg(DIM_COLOR)));
    lines.push(Line::from(bar));

    // Detail list with dot leaders
    let max_label_len = final_segs
        .iter()
        .map(|(_, l, _, _)| l.chars().count())
        .max()
        .unwrap_or(8);
    let label_w = max_label_len.max(10).min(18);
    let line_w = max_width;

    for (icon, label, tokens, color) in &final_segs {
        let pct = (*tokens as f64 / limit as f64 * 100.0).round() as usize;
        let token_str = if *tokens >= 1000 {
            format!("{}k", tokens / 1000)
        } else {
            format!("{}", tokens)
        };
        let tail = format!("{}  {}%", token_str, pct);
        let label_text = format!("{} {}", icon, label);
        let label_len = label_text.chars().count();
        let pad = label_w.saturating_sub(label_len);
        let reserved = label_w + pad + tail.chars().count() + 2;
        let dots = line_w.saturating_sub(reserved).max(2);

        let mut spans: Vec<Span<'static>> = Vec::new();
        spans.push(Span::styled(label_text, Style::default().fg(*color)));
        if pad > 0 {
            spans.push(Span::raw(" ".repeat(pad)));
        }
        spans.push(Span::styled(
            "·".repeat(dots),
            Style::default().fg(DIM_COLOR),
        ));
        spans.push(Span::raw(" "));
        spans.push(Span::styled(tail, Style::default().fg(DIM_COLOR)));
        lines.push(Line::from(spans));
    }

    lines
}

fn render_rounded_box(
    title: &str,
    content: Vec<Line<'static>>,
    max_width: usize,
    border_style: Style,
) -> Vec<Line<'static>> {
    if content.is_empty() || max_width < 6 {
        return Vec::new();
    }

    let max_content_width = content
        .iter()
        .map(|line| line.width())
        .max()
        .unwrap_or(0)
        .min(max_width.saturating_sub(4));

    if max_content_width < 6 {
        return Vec::new();
    }

    let box_width = max_content_width + 4; // "│ " + content + " │"
    let title_text = format!(" {} ", title);
    let title_len = title_text.chars().count();
    let border_chars = box_width.saturating_sub(title_len + 2);
    let left_border = "─".repeat(border_chars / 2);
    let right_border = "─".repeat(border_chars - border_chars / 2);

    let mut lines: Vec<Line<'static>> = Vec::new();
    lines.push(Line::from(Span::styled(
        format!("╭{}{}{}╮", left_border, title_text, right_border),
        border_style,
    )));

    for line in content {
        let truncated = truncate_line_to_width(&line, max_content_width);
        let padding = max_content_width.saturating_sub(truncated.width());
        let mut spans: Vec<Span<'static>> = Vec::new();
        spans.push(Span::styled("│ ", border_style));
        spans.extend(truncated.spans);
        if padding > 0 {
            spans.push(Span::raw(" ".repeat(padding)));
        }
        spans.push(Span::styled(" │", border_style));
        lines.push(Line::from(spans));
    }

    let bottom_border = "─".repeat(box_width.saturating_sub(2));
    lines.push(Line::from(Span::styled(
        format!("╰{}╯", bottom_border),
        border_style,
    )));

    lines
}

fn truncate_line_to_width(line: &Line<'static>, width: usize) -> Line<'static> {
    if width == 0 {
        return Line::from("");
    }

    let mut spans: Vec<Span<'static>> = Vec::new();
    let mut remaining = width;
    for span in &line.spans {
        if remaining == 0 {
            break;
        }
        let text = span.content.as_ref();
        let len = text.chars().count();
        if len <= remaining {
            spans.push(span.clone());
            remaining -= len;
        } else {
            let clipped: String = text.chars().take(remaining).collect();
            spans.push(Span::styled(clipped, span.style));
            remaining = 0;
        }
    }

    if spans.is_empty() {
        Line::from("")
    } else {
        Line::from(spans)
    }
}

/// Calculate rainbow color for prompt index with exponential decay to gray.
/// `distance` is how many prompts back from the most recent (0 = most recent).
fn rainbow_prompt_color(distance: usize) -> Color {
    // Rainbow colors (hue progression): red -> orange -> yellow -> green -> cyan -> blue -> violet
    const RAINBOW: [(u8, u8, u8); 7] = [
        (255, 80, 80),   // Red (softened)
        (255, 160, 80),  // Orange
        (255, 230, 80),  // Yellow
        (80, 220, 100),  // Green
        (80, 200, 220),  // Cyan
        (100, 140, 255), // Blue
        (180, 100, 255), // Violet
    ];

    // Gray target (DIM_COLOR)
    const GRAY: (u8, u8, u8) = (80, 80, 80);

    // Exponential decay factor - how quickly we fade to gray
    // decay = e^(-distance * rate), rate of ~0.4 gives nice falloff
    let decay = (-0.4 * distance as f32).exp();

    // Select rainbow color based on distance (cycle through)
    let rainbow_idx = distance.min(RAINBOW.len() - 1);
    let (r, g, b) = RAINBOW[rainbow_idx];

    // Blend rainbow color with gray based on decay
    // At distance 0: 100% rainbow, as distance increases: approaches gray
    let blend = |rainbow: u8, gray: u8| -> u8 {
        (rainbow as f32 * decay + gray as f32 * (1.0 - decay)) as u8
    };

    Color::Rgb(blend(r, GRAY.0), blend(g, GRAY.1), blend(b, GRAY.2))
}

/// Generate an animated color that pulses between two colors
fn animated_tool_color(elapsed: f32) -> Color {
    // Cycle period of ~1.5 seconds
    let t = (elapsed * 2.0).sin() * 0.5 + 0.5; // 0.0 to 1.0

    // Interpolate between cyan and purple
    let r = (80.0 + t * 106.0) as u8; // 80 -> 186
    let g = (200.0 - t * 61.0) as u8; // 200 -> 139
    let b = (220.0 + t * 35.0) as u8; // 220 -> 255

    Color::Rgb(r, g, b)
}

/// Format seconds as a human-readable age string
fn format_age(secs: i64) -> String {
    if secs < 0 {
        "future?".to_string()
    } else if secs < 60 {
        "just now".to_string()
    } else if secs < 3600 {
        format!("{}m ago", secs / 60)
    } else if secs < 86400 {
        format!("{}h ago", secs / 3600)
    } else {
        format!("{}d ago", secs / 86400)
    }
}

/// Get how long ago the binary was built and when the code was committed
/// Shows both if they differ significantly, otherwise just the build time
fn binary_age() -> Option<String> {
    let build_time = env!("JCODE_BUILD_TIME");
    let git_date = env!("JCODE_GIT_DATE");

    let now = chrono::Utc::now();

    // Parse build time
    let build_date = chrono::DateTime::parse_from_str(build_time, "%Y-%m-%d %H:%M:%S %z").ok()?;
    let build_secs = now.signed_duration_since(build_date).num_seconds();

    // Parse git commit date
    let git_commit_date = chrono::DateTime::parse_from_str(git_date, "%Y-%m-%d %H:%M:%S %z").ok();
    let git_secs = git_commit_date.map(|d| now.signed_duration_since(d).num_seconds());

    let build_age = format_age(build_secs);

    // If git date is available and differs significantly (>5 min), show both
    if let Some(git_secs) = git_secs {
        let diff = (git_secs - build_secs).abs();
        if diff > 300 {
            // More than 5 minutes difference
            let git_age = format_age(git_secs);
            return Some(format!("{}, code {}", build_age, git_age));
        }
    }

    Some(build_age)
}

/// Get changelog entries the user hasn't seen yet.
/// Reads the last-seen commit hash from ~/.jcode/last_seen_changelog,
/// filters the embedded changelog to only new entries, then saves the latest hash.
/// Returns just the commit subjects (not the hashes).
fn get_unseen_changelog_entries() -> &'static Vec<String> {
    static ENTRIES: OnceLock<Vec<String>> = OnceLock::new();
    ENTRIES.get_or_init(|| {
        let changelog = env!("JCODE_CHANGELOG");
        if changelog.is_empty() {
            return Vec::new();
        }

        // Parse "hash:subject" lines
        let all_entries: Vec<(&str, &str)> = changelog
            .lines()
            .filter_map(|line| line.split_once(':'))
            .collect();

        if all_entries.is_empty() {
            return Vec::new();
        }

        // Read last-seen hash
        let state_file = dirs::home_dir()
            .map(|h| h.join(".jcode").join("last_seen_changelog"))
            .unwrap_or_else(|| std::path::PathBuf::from(".jcode/last_seen_changelog"));

        let last_seen_hash = std::fs::read_to_string(&state_file)
            .ok()
            .map(|s| s.trim().to_string())
            .unwrap_or_default();

        // Filter: take entries until we hit the last-seen hash
        let new_entries: Vec<String> = if last_seen_hash.is_empty() {
            // First time ever — show last 5 as a welcome
            all_entries
                .iter()
                .take(5)
                .map(|(_, subject)| subject.to_string())
                .collect()
        } else {
            all_entries
                .iter()
                .take_while(|(hash, _)| *hash != last_seen_hash)
                .map(|(_, subject)| subject.to_string())
                .collect()
        };

        // Save the latest hash so next session only shows new stuff
        if let Some((latest_hash, _)) = all_entries.first() {
            if let Some(parent) = state_file.parent() {
                let _ = std::fs::create_dir_all(parent);
            }
            let _ = std::fs::write(&state_file, latest_hash);
        }

        new_entries
    })
}

/// Shorten model name for display (e.g., "claude-opus-4-5-20251101" -> "claude4.5opus")
fn shorten_model_name(model: &str) -> String {
    // Handle OpenRouter models (format: provider/model-name)
    // Keep the full identifier for display
    if model.contains('/') {
        return model.to_string();
    }
    // Handle common Claude model patterns
    if model.contains("opus") {
        if model.contains("4-5") || model.contains("4.5") {
            return "claude4.5opus".to_string();
        }
        return "claudeopus".to_string();
    }
    if model.contains("sonnet") {
        if model.contains("3-5") || model.contains("3.5") {
            return "claude3.5sonnet".to_string();
        }
        return "claudesonnet".to_string();
    }
    if model.contains("haiku") {
        return "claudehaiku".to_string();
    }
    // Handle OpenAI models (gpt-5.2-codex -> gpt5.2codex)
    if model.starts_with("gpt-5") {
        // e.g., "gpt-5.2-codex" -> "gpt5.2codex"
        return model.replace("gpt-", "gpt").replace("-", "");
    }
    if model.starts_with("gpt-4") {
        return model.replace("gpt-", "").replace("-", "");
    }
    if model.starts_with("gpt-3") {
        return "gpt3.5".to_string();
    }
    // Fallback: remove common suffixes and dashes
    model.split('-').take(3).collect::<Vec<_>>().join("")
}

/// Calculate the number of visual lines an input string will occupy
/// when wrapped to a given width, accounting for explicit newlines.
fn calculate_input_lines(input: &str, line_width: usize) -> usize {
    if line_width == 0 {
        return 1;
    }
    if input.is_empty() {
        return 1;
    }

    let mut total_lines = 0;
    for line in input.split('\n') {
        let chars: Vec<char> = line.chars().collect();
        if chars.is_empty() {
            total_lines += 1;
        } else {
            // Calculate wrapped lines for this segment
            total_lines += (chars.len() + line_width - 1) / line_width;
        }
    }
    total_lines.max(1)
}

/// Format status line content for visual debug capture
fn format_status_for_debug(app: &dyn TuiState) -> String {
    match app.status() {
        ProcessingStatus::Idle => {
            if let Some(notice) = app.status_notice() {
                format!("Idle (notice: {})", notice)
            } else if let Some((input, output)) = app.total_session_tokens() {
                format!(
                    "Idle (session: {}k in, {}k out)",
                    input / 1000,
                    output / 1000
                )
            } else {
                "Idle".to_string()
            }
        }
        ProcessingStatus::Sending => "Sending...".to_string(),
        ProcessingStatus::Thinking(_start) => {
            let elapsed = app.elapsed().map(|d| d.as_secs_f32()).unwrap_or(0.0);
            format!("Thinking... ({:.1}s)", elapsed)
        }
        ProcessingStatus::Streaming => {
            let (input, output) = app.streaming_tokens();
            format!("Streaming (↑{} ↓{})", input, output)
        }
        ProcessingStatus::RunningTool(ref name) => {
            format!("Running tool: {}", name)
        }
    }
}

/// Pre-computed image region from line scanning
#[derive(Clone)]
struct ImageRegion {
    /// Absolute line index in wrapped_lines
    abs_line_idx: usize,
    /// Hash of the mermaid content (for cache lookup)
    hash: u64,
    /// Total height of the image placeholder in lines
    height: u16,
}

#[derive(Clone)]
struct PreparedMessages {
    wrapped_lines: Vec<Line<'static>>,
    wrapped_user_indices: Vec<usize>,
    /// Pre-scanned image regions (computed once, not every frame)
    image_regions: Vec<ImageRegion>,
}

#[derive(Debug, Clone, PartialEq, Eq, Hash)]
struct BodyCacheKey {
    width: u16,
    diff_mode: crate::config::DiffDisplayMode,
    messages_version: u64,
    diagram_mode: crate::config::DiagramDisplayMode,
}

#[derive(Default)]
struct BodyCacheState {
    key: Option<BodyCacheKey>,
    prepared: Option<PreparedMessages>,
}

static BODY_CACHE: OnceLock<Mutex<BodyCacheState>> = OnceLock::new();

fn body_cache() -> &'static Mutex<BodyCacheState> {
    BODY_CACHE.get_or_init(|| Mutex::new(BodyCacheState::default()))
}

#[derive(Debug, Clone, PartialEq, Eq, Hash)]
struct MessageCacheKey {
    width: u16,
    diff_mode: crate::config::DiffDisplayMode,
    message_hash: u64,
    content_len: usize,
    diagram_mode: crate::config::DiagramDisplayMode,
}

#[derive(Default)]
struct MessageCacheState {
    entries: HashMap<MessageCacheKey, Vec<Line<'static>>>,
    order: VecDeque<MessageCacheKey>,
}

impl MessageCacheState {
    fn touch(&mut self, key: &MessageCacheKey) {
        if let Some(pos) = self.order.iter().position(|k| k == key) {
            self.order.remove(pos);
        }
        self.order.push_back(key.clone());
    }

    fn get(&mut self, key: &MessageCacheKey) -> Option<Vec<Line<'static>>> {
        let lines = self.entries.get(key).cloned();
        if lines.is_some() {
            self.touch(key);
        }
        lines
    }

    fn insert(&mut self, key: MessageCacheKey, lines: Vec<Line<'static>>) {
        if self.entries.contains_key(&key) {
            self.entries.insert(key.clone(), lines);
            self.touch(&key);
            return;
        }

        self.entries.insert(key.clone(), lines);
        self.order.push_back(key);

        while self.order.len() > MESSAGE_CACHE_LIMIT {
            if let Some(oldest) = self.order.pop_front() {
                self.entries.remove(&oldest);
            }
        }
    }
}

static MESSAGE_CACHE: OnceLock<Mutex<MessageCacheState>> = OnceLock::new();

fn message_cache() -> &'static Mutex<MessageCacheState> {
    MESSAGE_CACHE.get_or_init(|| Mutex::new(MessageCacheState::default()))
}

const MESSAGE_CACHE_LIMIT: usize = 512;

#[derive(Default)]
struct RenderProfile {
    frames: u64,
    total: Duration,
    prepare: Duration,
    draw: Duration,
    last_log: Option<Instant>,
}

static PROFILE_STATE: OnceLock<Mutex<RenderProfile>> = OnceLock::new();

fn profile_state() -> &'static Mutex<RenderProfile> {
    PROFILE_STATE.get_or_init(|| Mutex::new(RenderProfile::default()))
}

#[derive(Clone, Copy, Debug)]
pub struct LayoutSnapshot {
    pub messages_area: Rect,
    pub diagram_area: Option<Rect>,
}

static LAST_LAYOUT: OnceLock<Mutex<Option<LayoutSnapshot>>> = OnceLock::new();

fn last_layout_state() -> &'static Mutex<Option<LayoutSnapshot>> {
    LAST_LAYOUT.get_or_init(|| Mutex::new(None))
}

pub fn record_layout_snapshot(messages_area: Rect, diagram_area: Option<Rect>) {
    if let Ok(mut snapshot) = last_layout_state().lock() {
        *snapshot = Some(LayoutSnapshot {
            messages_area,
            diagram_area,
        });
    }
}

pub fn last_layout_snapshot() -> Option<LayoutSnapshot> {
    last_layout_state()
        .lock()
        .ok()
        .and_then(|snapshot| *snapshot)
}

fn profile_enabled() -> bool {
    static ENABLED: OnceLock<bool> = OnceLock::new();
    *ENABLED.get_or_init(|| std::env::var("JCODE_TUI_PROFILE").is_ok())
}

fn record_profile(prepare: Duration, draw: Duration, total: Duration) {
    let mut state = match profile_state().lock() {
        Ok(s) => s,
        Err(poisoned) => poisoned.into_inner(),
    };
    state.frames += 1;
    state.prepare += prepare;
    state.draw += draw;
    state.total += total;

    let now = Instant::now();
    let should_log = match state.last_log {
        Some(last) => now.duration_since(last) >= Duration::from_secs(1),
        None => true,
    };
    if should_log && state.frames > 0 {
        let frames = state.frames as f64;
        let avg_prepare = state.prepare.as_secs_f64() * 1000.0 / frames;
        let avg_draw = state.draw.as_secs_f64() * 1000.0 / frames;
        let avg_total = state.total.as_secs_f64() * 1000.0 / frames;
        crate::logging::info(&format!(
            "TUI perf: {:.1} fps | prepare {:.2}ms | draw {:.2}ms | total {:.2}ms",
            frames, avg_prepare, avg_draw, avg_total
        ));
        state.frames = 0;
        state.prepare = Duration::from_secs(0);
        state.draw = Duration::from_secs(0);
        state.total = Duration::from_secs(0);
        state.last_log = Some(now);
    }
}

pub fn draw(frame: &mut Frame, app: &dyn TuiState) {
    match std::panic::catch_unwind(std::panic::AssertUnwindSafe(|| draw_inner(frame, app))) {
        Ok(()) => {}
        Err(payload) => {
            let panic_count = DRAW_PANIC_COUNT.fetch_add(1, Ordering::Relaxed) + 1;
            let msg = panic_payload_to_string(&payload);
            if panic_count <= 3 || panic_count % 50 == 0 {
                crate::logging::error(&format!(
                    "Recovered TUI draw panic #{}: {}",
                    panic_count, msg
                ));
            }
            let area = frame.area().intersection(*frame.buffer_mut().area());
            if area.width == 0 || area.height == 0 {
                return;
            }
            frame.render_widget(Clear, area);
            let lines = vec![
                Line::from(Span::styled(
                    "rendering error recovered",
                    Style::default().fg(Color::Red),
                )),
                Line::from(Span::styled(
                    "continuing with a safe fallback frame",
                    Style::default().fg(DIM_COLOR),
                )),
            ];
            frame.render_widget(Paragraph::new(lines), area);
        }
    }
}

fn panic_payload_to_string(payload: &(dyn std::any::Any + Send)) -> String {
    if let Some(s) = payload.downcast_ref::<&str>() {
        (*s).to_string()
    } else if let Some(s) = payload.downcast_ref::<String>() {
        s.clone()
    } else {
        "unknown panic payload".to_string()
    }
}

fn div_ceil_u32(value: u32, divisor: u32) -> u32 {
    if divisor == 0 {
        return value;
    }
    value.saturating_add(divisor - 1) / divisor
}

fn estimate_pinned_diagram_pane_width_with_font(
    diagram: &info_widget::DiagramInfo,
    pane_height: u16,
    min_width: u16,
    font_size: Option<(u16, u16)>,
) -> u16 {
    const PANE_BORDER_WIDTH: u32 = 2;
    let inner_height = pane_height.saturating_sub(PANE_BORDER_WIDTH as u16).max(1) as u32;
    let (cell_w, cell_h) = font_size.unwrap_or((8, 16));
    let cell_w = cell_w.max(1) as u32;
    let cell_h = cell_h.max(1) as u32;

    let image_w_cells = div_ceil_u32(diagram.width.max(1), cell_w);
    let image_h_cells = div_ceil_u32(diagram.height.max(1), cell_h);
    let fit_w_cells = if image_h_cells > inner_height {
        div_ceil_u32(image_w_cells.saturating_mul(inner_height), image_h_cells)
    } else {
        image_w_cells
    }
    .max(1);

    let pane_width = fit_w_cells.saturating_add(PANE_BORDER_WIDTH);
    pane_width.max(min_width as u32).min(u16::MAX as u32) as u16
}

fn estimate_pinned_diagram_pane_width(
    diagram: &info_widget::DiagramInfo,
    pane_height: u16,
    min_width: u16,
) -> u16 {
    estimate_pinned_diagram_pane_width_with_font(
        diagram,
        pane_height,
        min_width,
        super::mermaid::get_font_size(),
    )
}

fn estimate_pinned_diagram_pane_height(
    diagram: &info_widget::DiagramInfo,
    pane_width: u16,
    min_height: u16,
) -> u16 {
    const PANE_BORDER: u32 = 2;
    let inner_width = pane_width.saturating_sub(PANE_BORDER as u16).max(1) as u32;
    let (cell_w, cell_h) = super::mermaid::get_font_size().unwrap_or((8, 16));
    let cell_w = cell_w.max(1) as u32;
    let cell_h = cell_h.max(1) as u32;

    let image_w_cells = div_ceil_u32(diagram.width.max(1), cell_w);
    let image_h_cells = div_ceil_u32(diagram.height.max(1), cell_h);
    let fit_h_cells = if image_w_cells > inner_width {
        div_ceil_u32(image_h_cells.saturating_mul(inner_width), image_w_cells)
    } else {
        image_h_cells
    }
    .max(1);

    let pane_height = fit_h_cells.saturating_add(PANE_BORDER);
    pane_height.max(min_height as u32).min(u16::MAX as u32) as u16
}

fn draw_inner(frame: &mut Frame, app: &dyn TuiState) {
    let area = frame.area().intersection(*frame.buffer_mut().area());
    if area.width == 0 || area.height == 0 {
        return;
    }

    // Initialize visual debug capture if enabled
    let mut debug_capture = if visual_debug::is_enabled() {
        Some(FrameCaptureBuilder::new(area.width, area.height))
    } else {
        None
    };

    // Check diagram display mode and get active diagrams early so we can
    // determine the horizontal split before computing input width etc.
    let diagram_mode = app.diagram_mode();
    let diagrams = super::mermaid::get_active_diagrams();
    let diagram_count = diagrams.len();
    let selected_index = if diagram_count > 0 {
        app.diagram_index().min(diagram_count - 1)
    } else {
        0
    };
    let pane_enabled = app.diagram_pane_enabled();
    let pane_position = app.diagram_pane_position();
    let pinned_diagram =
        if diagram_mode == crate::config::DiagramDisplayMode::Pinned && pane_enabled {
            diagrams.get(selected_index).cloned()
        } else {
            None
        };
    let diagram_focus = app.diagram_focus();
    let (diagram_scroll_x, diagram_scroll_y) = app.diagram_scroll();

    // Compute layout depending on pane position (Side = right column, Top = above chat).
    let mut has_pinned_area = false;
    let (chat_area, diagram_area) = if let Some(diagram) = pinned_diagram.as_ref() {
        match pane_position {
            crate::config::DiagramPanePosition::Side => {
                const MIN_DIAGRAM_WIDTH: u16 = 24;
                const MIN_CHAT_WIDTH: u16 = 20;
                let max_diagram = area.width.saturating_sub(MIN_CHAT_WIDTH);
                if max_diagram >= MIN_DIAGRAM_WIDTH {
                    let ratio = app.diagram_pane_ratio().clamp(25, 70) as u32;
                    let ratio_cap = ((area.width as u32 * ratio) / 100) as u16;
                    let needed = estimate_pinned_diagram_pane_width(
                        diagram,
                        area.height,
                        MIN_DIAGRAM_WIDTH,
                    );
                    let diagram_width = needed
                        .min(ratio_cap)
                        .max(MIN_DIAGRAM_WIDTH)
                        .min(max_diagram);
                    let chat_width = area.width.saturating_sub(diagram_width);
                    has_pinned_area = diagram_width > 0 && chat_width > 0;
                    if has_pinned_area {
                        let chat = Rect { x: area.x, y: area.y, width: chat_width, height: area.height };
                        let diag = Rect { x: area.x + chat_width, y: area.y, width: diagram_width, height: area.height };
                        (chat, Some(diag))
                    } else {
                        (area, None)
                    }
                } else {
                    (area, None)
                }
            }
            crate::config::DiagramPanePosition::Top => {
                const MIN_DIAGRAM_HEIGHT: u16 = 6;
                const MIN_CHAT_HEIGHT: u16 = 8;
                let max_diagram = area.height.saturating_sub(MIN_CHAT_HEIGHT);
                if max_diagram >= MIN_DIAGRAM_HEIGHT {
                    let ratio = app.diagram_pane_ratio().clamp(20, 60) as u32;
                    let ratio_cap = ((area.height as u32 * ratio) / 100) as u16;
                    let needed = estimate_pinned_diagram_pane_height(
                        diagram,
                        area.width,
                        MIN_DIAGRAM_HEIGHT,
                    );
                    let diagram_height = needed
                        .min(ratio_cap)
                        .max(MIN_DIAGRAM_HEIGHT)
                        .min(max_diagram);
                    let chat_height = area.height.saturating_sub(diagram_height);
                    has_pinned_area = diagram_height > 0 && chat_height > 0;
                    if has_pinned_area {
                        let diag = Rect { x: area.x, y: area.y, width: area.width, height: diagram_height };
                        let chat = Rect { x: area.x, y: area.y + diagram_height, width: area.width, height: chat_height };
                        (chat, Some(diag))
                    } else {
                        (area, None)
                    }
                } else {
                    (area, None)
                }
            }
        }
    } else {
        (area, None)
    };

    let diff_mode = app.diff_mode();
    let pin_images = app.pin_images();
    let collect_diffs = diff_mode.is_pinned();
    let pinned_content = if collect_diffs || pin_images {
        collect_pinned_content(app.display_messages(), collect_diffs, pin_images)
    } else {
        Vec::new()
    };

    let (chat_area, diff_pane_area) = if !pinned_content.is_empty() {
        const MIN_DIFF_WIDTH: u16 = 30;
        const MIN_CHAT_WIDTH: u16 = 20;
        let max_diff = chat_area.width.saturating_sub(MIN_CHAT_WIDTH);
        if max_diff >= MIN_DIFF_WIDTH {
            let diff_width = (chat_area.width * 35 / 100).max(MIN_DIFF_WIDTH).min(max_diff);
            let new_chat_width = chat_area.width.saturating_sub(diff_width);
            let chat = Rect { x: chat_area.x, y: chat_area.y, width: new_chat_width, height: chat_area.height };
            let diff = Rect { x: chat_area.x + new_chat_width, y: chat_area.y, width: diff_width, height: chat_area.height };
            (chat, Some(diff))
        } else {
            (chat_area, None)
        }
    } else {
        (chat_area, None)
    };

    // Calculate pending messages (queued + interleave) for numbering and layout
    let pending_count = pending_prompt_count(app);
    let queued_height = pending_count.min(3) as u16;

    // Calculate input height based on content (max 10 lines visible, scrolls if more)
    let reserved_width = send_mode_reserved_width(app) as u16;
    let available_width = chat_area.width.saturating_sub(3 + reserved_width) as usize;
    let base_input_height = calculate_input_lines(app.input(), available_width).min(10) as u16;
    // Add 1 line for command suggestions when typing /, or for Shift+Enter hint when processing
    let suggestions = app.command_suggestions();
    let hint_line_height = if !suggestions.is_empty() && !app.is_processing() {
        1 // Command suggestions
    } else if app.is_processing() && !app.input().is_empty() {
        1 // Shift+Enter hint
    } else {
        0
    };
    let picker_height: u16 = if app.picker_state().is_some() { 8 } else { 0 };
    let input_height = base_input_height + hint_line_height;

    // Count user messages to show next prompt number
    let user_count = app
        .display_messages()
        .iter()
        .filter(|m| m.role == "user")
        .count();

    let total_start = Instant::now();
    if let Some(ref mut capture) = debug_capture {
        capture.render_order.push("prepare_messages".to_string());
    }
    let prep_start = Instant::now();
    let prepared = prepare_messages(app, chat_area.width, chat_area.height);
    if let Some(ref mut capture) = debug_capture {
        capture.image_regions = prepared
            .image_regions
            .iter()
            .map(|region| ImageRegionCapture {
                hash: format!("{:016x}", region.hash),
                abs_line_idx: region.abs_line_idx,
                height: region.height,
            })
            .collect();
    }
    let prep_elapsed = prep_start.elapsed();
    let content_height = prepared.wrapped_lines.len().max(1) as u16;
    let show_donut = crate::config::config().display.idle_animation
        && app.display_messages().is_empty()
        && !app.is_processing()
        && app.streaming_text().is_empty()
        && app.queued_messages().is_empty();
    let donut_height: u16 = if show_donut { 14 } else { 0 };
    let fixed_height = 1 + queued_height + picker_height + input_height + donut_height; // status + queued + picker + input + donut
    let available_height = chat_area.height;

    // Use packed layout when content fits, scrolling layout otherwise
    let use_packed = content_height + fixed_height <= available_height;

    // Layout: messages (includes header), queued, status, picker, input, donut
    // All vertical chunks are within the chat_area (left column).
    let chunks = Layout::default()
        .direction(Direction::Vertical)
        .constraints(if use_packed {
            vec![
                Constraint::Length(content_height.max(1)), // Messages (exact height)
                Constraint::Length(queued_height),         // Queued messages (above status)
                Constraint::Length(1),                     // Status line
                Constraint::Length(picker_height),         // Picker (0 or 1 line)
                Constraint::Length(input_height),          // Input
                Constraint::Length(donut_height),          // Donut animation
            ]
        } else {
            vec![
                Constraint::Min(3),                // Messages (scrollable)
                Constraint::Length(queued_height), // Queued messages (above status)
                Constraint::Length(1),             // Status line
                Constraint::Length(picker_height), // Picker (0 or 1 line)
                Constraint::Length(input_height),  // Input
                Constraint::Length(donut_height),  // Donut animation
            ]
        })
        .split(chat_area);

    // Capture layout info for visual debug
    if let Some(ref mut capture) = debug_capture {
        capture.layout.use_packed = use_packed;
        capture.layout.estimated_content_height = content_height as usize;
        capture.layout.messages_area = Some(chunks[0].into());
        if queued_height > 0 {
            capture.layout.queued_area = Some(chunks[1].into());
        }
        capture.layout.status_area = Some(chunks[2].into());
        capture.layout.input_area = Some(chunks[4].into());
        capture.layout.input_lines_raw = app.input().lines().count().max(1);
        capture.layout.input_lines_wrapped = base_input_height as usize;

        // Capture state snapshot
        capture.state.is_processing = app.is_processing();
        capture.state.input_len = app.input().len();
        capture.state.input_preview = app.input().chars().take(100).collect();
        capture.state.cursor_pos = app.cursor_pos();
        capture.state.scroll_offset = app.scroll_offset();
        capture.state.queued_count = pending_count;
        capture.state.message_count = app.display_messages().len();
        capture.state.streaming_text_len = app.streaming_text().len();
        capture.state.has_suggestions = !suggestions.is_empty();
        capture.state.status = format!("{:?}", app.status());
        capture.state.diagram_mode = Some(format!("{:?}", diagram_mode));
        capture.state.diagram_focus = diagram_focus;
        capture.state.diagram_index = selected_index;
        capture.state.diagram_count = diagram_count;
        capture.state.diagram_scroll_x = diagram_scroll_x;
        capture.state.diagram_scroll_y = diagram_scroll_y;
        capture.state.diagram_pane_ratio = app.diagram_pane_ratio();
        capture.state.diagram_pane_enabled = app.diagram_pane_enabled();
        capture.state.diagram_pane_position = Some(format!("{:?}", app.diagram_pane_position()));
        capture.state.diagram_zoom = app.diagram_zoom();

        // Capture rendered content
        // Queued messages
        capture.rendered_text.queued_messages = pending_queue_preview(app);

        // Recent display messages (last 5 for context)
        capture.rendered_text.recent_messages = app
            .display_messages()
            .iter()
            .rev()
            .take(5)
            .map(|m| MessageCapture {
                role: m.role.clone(),
                content_preview: m.content.chars().take(200).collect(),
                content_len: m.content.len(),
            })
            .collect();

        // Streaming text preview
        let streaming = app.streaming_text();
        if !streaming.is_empty() {
            capture.rendered_text.streaming_text_preview = streaming.chars().take(500).collect();
        }

        // Status line content
        capture.rendered_text.status_line = format_status_for_debug(app);
    }

    if let Some(ref mut capture) = debug_capture {
        capture.render_order.push("draw_messages".to_string());
    }
    let draw_start = Instant::now();

    // Messages area is chunks[0] within the chat column (already excludes diagram).
    let messages_area = chunks[0];

    if let Some(ref mut capture) = debug_capture {
        capture.layout.messages_area = Some(messages_area.into());
        capture.layout.diagram_area = diagram_area.map(|r| r.into());
    }
    record_layout_snapshot(messages_area, diagram_area);

    let margins = draw_messages(frame, app, messages_area, &prepared);

    // Render pinned diagram if we have one
    if let (Some(diagram_info), Some(area)) = (&pinned_diagram, diagram_area) {
        if let Some(ref mut capture) = debug_capture {
            capture.render_order.push("draw_pinned_diagram".to_string());
        }
        draw_pinned_diagram(
            frame,
            diagram_info,
            area,
            selected_index,
            diagram_count,
            diagram_focus,
            diagram_scroll_x,
            diagram_scroll_y,
            app.diagram_zoom(),
            pane_position,
        );
    }

    if let Some(diff_area) = diff_pane_area {
        if !pinned_content.is_empty() {
            if let Some(ref mut capture) = debug_capture {
                capture.render_order.push("draw_pinned_content".to_string());
            }
            draw_pinned_content(frame, diff_area, &pinned_content, app.diff_pane_scroll());
        }
    }

    let messages_draw = draw_start.elapsed();

    if let Some(ref mut capture) = debug_capture {
        capture.layout.margins = Some(MarginsCapture {
            left_widths: margins.left_widths.clone(),
            right_widths: margins.right_widths.clone(),
            centered: margins.centered,
        });
    }
    if queued_height > 0 {
        if let Some(ref mut capture) = debug_capture {
            capture.render_order.push("draw_queued".to_string());
        }
        draw_queued(frame, app, chunks[1], user_count + 1);
    }
    if let Some(ref mut capture) = debug_capture {
        capture.render_order.push("draw_status".to_string());
    }
    draw_status(frame, app, chunks[2], pending_count);
    if let Some(ref mut capture) = debug_capture {
        capture.render_order.push("draw_input".to_string());
    }
    // Draw picker line if active
    if picker_height > 0 {
        draw_picker_line(frame, app, chunks[3]);
    }

    draw_input(
        frame,
        app,
        chunks[4],
        user_count + pending_count + 1,
        &mut debug_capture,
    );

    if donut_height > 0 {
        draw_idle_animation(frame, app, chunks[5]);
    }

    // Draw info widget overlays (skip during idle animation - they look out of place)
    let widget_data = app.info_widget_data();
    let mut widget_render_ms: Option<f32> = None;
    let mut placements: Vec<info_widget::WidgetPlacement> = Vec::new();
    let widget_bounds = messages_area;
    if !widget_data.is_empty() && !show_donut {
        if let Some(ref mut capture) = debug_capture {
            capture.render_order.push("render_info_widgets".to_string());
        }
        placements = info_widget::calculate_placements(widget_bounds, &margins, &widget_data);

        if let Some(ref mut capture) = debug_capture {
            let placement_captures = capture_widget_placements(&placements);
            capture.layout.widget_placements = placement_captures.clone();
            capture.info_widgets = Some(InfoWidgetCapture {
                summary: build_info_widget_summary(&widget_data),
                placements: placement_captures,
            });

            // Detect overlaps with message area
            for placement in &placements {
                if rects_overlap(placement.rect, widget_bounds) {
                    capture.anomaly(format!(
                        "Info widget {:?} overlaps messages area",
                        placement.kind
                    ));
                }
                if !rect_within_bounds(placement.rect, area) {
                    capture.anomaly(format!(
                        "Info widget {:?} out of bounds {:?}",
                        placement.kind, placement.rect
                    ));
                }
                if let Some(diagram_area) = diagram_area {
                    if rects_overlap(placement.rect, diagram_area) {
                        capture.anomaly(format!(
                            "Info widget {:?} overlaps diagram area",
                            placement.kind
                        ));
                    }
                }
            }
            for i in 0..placements.len() {
                for j in (i + 1)..placements.len() {
                    if rects_overlap(placements[i].rect, placements[j].rect) {
                        capture.anomaly(format!(
                            "Info widgets overlap: {:?} and {:?}",
                            placements[i].kind, placements[j].kind
                        ));
                    }
                }
            }
        }

        let widget_start = Instant::now();
        info_widget::render_all(frame, &placements, &widget_data);
        widget_render_ms = Some(widget_start.elapsed().as_secs_f32() * 1000.0);

        // Optional visual overlay for placements
    } else if let Some(ref mut capture) = debug_capture {
        capture.info_widgets = Some(InfoWidgetCapture {
            summary: build_info_widget_summary(&widget_data),
            placements: Vec::new(),
        });
    }
    if visual_debug::overlay_enabled() {
        draw_debug_overlay(frame, &placements, &chunks);
    }

    // Record the frame capture if enabled
    if let Some(capture) = debug_capture {
        let total_draw = draw_start.elapsed();
        let render_timing = RenderTimingCapture {
            prepare_ms: prep_elapsed.as_secs_f32() * 1000.0,
            draw_ms: total_draw.as_secs_f32() * 1000.0,
            total_ms: total_start.elapsed().as_secs_f32() * 1000.0,
            messages_ms: Some(messages_draw.as_secs_f32() * 1000.0),
            widgets_ms: widget_render_ms,
        };

        let mut capture = capture;
        capture.render_timing = Some(render_timing);
        capture.mermaid = crate::tui::mermaid::debug_stats_json();
        capture.markdown = crate::tui::markdown::debug_stats_json();
        capture.theme = debug_palette_json();
        visual_debug::record_frame(capture.build());
    }

    if profile_enabled() {
        let total_draw = draw_start.elapsed();
        record_profile(prep_elapsed, total_draw, total_start.elapsed());
    }
}

fn prepare_messages(app: &dyn TuiState, width: u16, height: u16) -> PreparedMessages {
    // Build the top header (chroma animated name/model/badges)
    let mut all_header_lines = build_persistent_header(app, width);
    // Add the rest of the header (model ID, changelog, MCPs, etc.)
    all_header_lines.extend(build_header_lines(app, width));
    let header_prepared = wrap_lines(all_header_lines, &[], width);
    let startup_active = super::startup_animation_active(app);
    let startup_prepared = if startup_active {
        wrap_lines(build_startup_animation_lines(app, width), &[], width)
    } else {
        PreparedMessages {
            wrapped_lines: Vec::new(),
            wrapped_user_indices: Vec::new(),
            image_regions: Vec::new(),
        }
    };

    let body_prepared = prepare_body_cached(app, width);
    let has_streaming = app.is_processing() && !app.streaming_text().is_empty();
    let stream_prefix_blank = has_streaming && !body_prepared.wrapped_lines.is_empty();
    let streaming_prepared = if has_streaming {
        prepare_streaming_cached(app, width, stream_prefix_blank)
    } else {
        PreparedMessages {
            wrapped_lines: Vec::new(),
            wrapped_user_indices: Vec::new(),
            image_regions: Vec::new(),
        }
    };

    let mut wrapped_lines: Vec<Line<'static>>;
    let mut wrapped_user_indices;
    let mut image_regions;

    if startup_active {
        let elapsed = app.animation_elapsed();
        let anim_duration = super::STARTUP_ANIMATION_WINDOW.as_secs_f32();
        let morph_t = (elapsed / anim_duration).clamp(0.0, 1.0);

        let anim_lines = &startup_prepared.wrapped_lines;
        let header_lines = &header_prepared.wrapped_lines;

        let content_lines: Vec<Line<'static>> = if morph_t < 0.6 {
            anim_lines.clone()
        } else {
            morph_lines_to_header(anim_lines, header_lines, morph_t, width)
        };

        let content_height = content_lines.len();
        let input_reserve = 4;
        let available = (height as usize).saturating_sub(input_reserve);
        let centered_pad = available.saturating_sub(content_height) / 2;

        // As the animation morphs into the header, compute the target
        // centering pad for the header so we can smoothly converge to it
        // instead of jumping when the animation ends.
        let header_height = header_prepared.wrapped_lines.len();
        let header_pad = available.saturating_sub(header_height) / 2;

        let slide_t = if morph_t > 0.85 {
            ((morph_t - 0.85) / 0.15).clamp(0.0, 1.0)
        } else {
            0.0
        };
        let slide_ease = slide_t * slide_t * (3.0 - 2.0 * slide_t);
        // Slide from animation-centered pad toward header-centered pad
        let pad_top = (centered_pad as f32
            + (header_pad as f32 - centered_pad as f32) * slide_ease)
            as usize;

        wrapped_lines = Vec::with_capacity(pad_top + content_height);
        for _ in 0..pad_top {
            wrapped_lines.push(Line::from(""));
        }
        wrapped_lines.extend(content_lines);
        wrapped_user_indices = Vec::new();
        image_regions = Vec::new();
    } else {
        let is_initial_empty = app.display_messages().is_empty()
            && !app.is_processing()
            && app.streaming_text().is_empty();

        wrapped_lines = header_prepared.wrapped_lines;

        if is_initial_empty {
            let content_height = wrapped_lines.len();
            let input_reserve = 4;
            let available = (height as usize).saturating_sub(input_reserve);
            let pad_top = available.saturating_sub(content_height) / 2;
            let mut centered = Vec::with_capacity(pad_top + content_height);
            for _ in 0..pad_top {
                centered.push(Line::from(""));
            }
            centered.extend(wrapped_lines);
            wrapped_lines = centered;
        }

        let header_len = wrapped_lines.len();
        let startup_len = startup_prepared.wrapped_lines.len();
        wrapped_lines.extend(startup_prepared.wrapped_lines);
        let body_len = body_prepared.wrapped_lines.len();
        wrapped_lines.extend(body_prepared.wrapped_lines);
        wrapped_lines.extend(streaming_prepared.wrapped_lines);

        wrapped_user_indices = body_prepared.wrapped_user_indices;
        for idx in &mut wrapped_user_indices {
            *idx += header_len + startup_len;
        }

        image_regions = Vec::new();
        for mut region in body_prepared.image_regions {
            region.abs_line_idx += header_len + startup_len;
            image_regions.push(region);
        }
        for mut region in streaming_prepared.image_regions {
            region.abs_line_idx += header_len + startup_len + body_len;
            image_regions.push(region);
        }
    }

    PreparedMessages {
        wrapped_lines,
        wrapped_user_indices,
        image_regions,
    }
}

fn extract_line_text(line: &Line) -> String {
    line.spans.iter().map(|s| s.content.as_ref()).collect()
}

fn extract_line_styled_chars(line: &Line) -> Vec<(char, Style)> {
    let mut chars = Vec::new();
    for span in &line.spans {
        for ch in span.content.chars() {
            chars.push((ch, span.style));
        }
    }
    chars
}

fn morph_lines_to_header(
    anim_lines: &[Line<'static>],
    header_lines: &[Line<'static>],
    morph_t: f32,
    width: u16,
) -> Vec<Line<'static>> {
    let blend = ((morph_t - 0.6) / 0.35).clamp(0.0, 1.0);
    let max_rows = anim_lines.len().max(header_lines.len());
    let w = width as usize;

    let mut result = Vec::with_capacity(max_rows);

    let anim_row_count = anim_lines.len();
    let header_row_count = header_lines.len();
    let row_blend = blend * blend;
    let target_rows =
        anim_row_count as f32 + (header_row_count as f32 - anim_row_count as f32) * row_blend;
    let output_rows = target_rows.round() as usize;

    for out_row in 0..output_rows {
        let anim_row_f = if output_rows > 1 {
            out_row as f32 / (output_rows - 1) as f32 * (anim_row_count.max(1) - 1) as f32
        } else {
            0.0
        };
        let header_row_f = if output_rows > 1 {
            out_row as f32 / (output_rows - 1) as f32 * (header_row_count.max(1) - 1) as f32
        } else {
            0.0
        };

        let anim_idx = (anim_row_f.round() as usize).min(anim_row_count.saturating_sub(1));
        let header_idx = (header_row_f.round() as usize).min(header_row_count.saturating_sub(1));

        let anim_chars: Vec<(char, Style)> = if anim_idx < anim_row_count {
            extract_line_styled_chars(&anim_lines[anim_idx])
        } else {
            Vec::new()
        };
        let header_chars: Vec<(char, Style)> = if header_idx < header_row_count {
            extract_line_styled_chars(&header_lines[header_idx])
        } else {
            Vec::new()
        };

        let anim_text: String = anim_chars.iter().map(|(c, _)| *c).collect();
        let header_text: String = header_chars.iter().map(|(c, _)| *c).collect();
        let anim_trimmed = anim_text.trim();
        let header_trimmed = header_text.trim();

        let anim_start = anim_text.find(anim_trimmed).unwrap_or(0);
        let header_start = header_text.find(header_trimmed).unwrap_or(0);

        let anim_center = if !anim_trimmed.is_empty() {
            anim_start as f32 + anim_trimmed.len() as f32 / 2.0
        } else {
            w as f32 / 2.0
        };
        let header_center = if !header_trimmed.is_empty() {
            header_start as f32 + header_trimmed.len() as f32 / 2.0
        } else {
            w as f32 / 2.0
        };

        let center = anim_center + (header_center - anim_center) * blend;
        let max_col = anim_chars.len().max(header_chars.len()).max(w);

        let mut spans: Vec<Span<'static>> = Vec::new();

        for col in 0..max_col {
            let anim_ch = anim_chars.get(col).map(|(c, _)| *c).unwrap_or(' ');
            let anim_style = anim_chars.get(col).map(|(_, s)| *s).unwrap_or_default();
            let header_ch = header_chars.get(col).map(|(c, _)| *c).unwrap_or(' ');
            let header_style = header_chars.get(col).map(|(_, s)| *s).unwrap_or_default();

            let dist_from_center = ((col as f32) - center).abs() / (w as f32 / 2.0).max(1.0);
            let flip_hash = {
                let mut h = DefaultHasher::new();
                out_row.hash(&mut h);
                col.hash(&mut h);
                (std::hash::Hasher::finish(&h) % 1000) as f32 / 1000.0
            };
            let flip_threshold = (0.3 + dist_from_center * 0.4 + flip_hash * 0.3).clamp(0.0, 1.0);

            let (ch, style) = if blend >= flip_threshold {
                let style_blend = ((blend - flip_threshold) / 0.15).clamp(0.0, 1.0);
                if style_blend < 0.3 {
                    let glitch_chars = b"@#$%&*!?~=+<>";
                    let gi = {
                        let mut h = DefaultHasher::new();
                        out_row.hash(&mut h);
                        col.hash(&mut h);
                        ((blend * 100.0) as u32).hash(&mut h);
                        (std::hash::Hasher::finish(&h) % glitch_chars.len() as u64) as usize
                    };
                    let gc = glitch_chars[gi] as char;
                    (gc, lerp_style(anim_style, header_style, style_blend))
                } else {
                    (header_ch, lerp_style(anim_style, header_style, style_blend))
                }
            } else {
                let fade = (1.0 - blend / flip_threshold.max(0.01)).clamp(0.0, 1.0);
                let mut s = anim_style;
                if let Some(Color::Rgb(r, g, b)) = s.fg {
                    s.fg = Some(Color::Rgb(
                        (r as f32 * fade) as u8,
                        (g as f32 * fade) as u8,
                        (b as f32 * fade) as u8,
                    ));
                }
                (anim_ch, s)
            };

            spans.push(Span::styled(ch.to_string(), style));
        }

        let align = header_lines
            .get(header_idx)
            .and_then(|l| l.alignment)
            .or_else(|| anim_lines.get(anim_idx).and_then(|l| l.alignment))
            .unwrap_or(ratatui::layout::Alignment::Center);

        result.push(Line::from(spans).alignment(align));
    }

    result
}

fn lerp_style(from: Style, to: Style, t: f32) -> Style {
    let fg = match (from.fg, to.fg) {
        (Some(Color::Rgb(r1, g1, b1)), Some(Color::Rgb(r2, g2, b2))) => Some(Color::Rgb(
            (r1 as f32 + (r2 as f32 - r1 as f32) * t) as u8,
            (g1 as f32 + (g2 as f32 - g1 as f32) * t) as u8,
            (b1 as f32 + (b2 as f32 - b1 as f32) * t) as u8,
        )),
        (Some(Color::Rgb(r, g, b)), _) => {
            let dim = 1.0 - t;
            Some(Color::Rgb(
                (r as f32 * dim) as u8,
                (g as f32 * dim) as u8,
                (b as f32 * dim) as u8,
            ))
        }
        (_, Some(Color::Rgb(r, g, b))) => Some(Color::Rgb(
            (r as f32 * t) as u8,
            (g as f32 * t) as u8,
            (b as f32 * t) as u8,
        )),
        (_, to_fg) => to_fg,
    };
    let mut s = to;
    s.fg = fg;
    s
}

fn startup_animation_variant() -> usize {
    use std::sync::OnceLock;
    static VARIANT: OnceLock<usize> = OnceLock::new();
    *VARIANT.get_or_init(|| {
        let mut hasher = DefaultHasher::new();
        std::time::SystemTime::now().hash(&mut hasher);
        std::process::id().hash(&mut hasher);
        (std::hash::Hasher::finish(&hasher) % 7) as usize
    })
}

fn build_startup_animation_lines(app: &dyn TuiState, term_width: u16) -> Vec<Line<'static>> {
    let elapsed = app.animation_elapsed();
    let status_idx =
        ((elapsed * STARTUP_ASCII_STATUS_FPS) as usize) % STARTUP_ASCII_STATUS_SPINNER.len();
    let status_spinner = STARTUP_ASCII_STATUS_SPINNER[status_idx];
    let progress = (elapsed / super::STARTUP_ANIMATION_WINDOW.as_secs_f32()).clamp(0.0, 1.0);
    let fade_in = (progress / 0.2).clamp(0.0, 1.0);
    let fade_out = ((1.0 - progress) / 0.25).clamp(0.0, 1.0);
    let envelope = fade_in.min(fade_out);
    let boost = (envelope * 120.0) as u8;
    let base = 80u8.saturating_add(boost);
    let art_color = Color::Rgb(base, base.saturating_add(16), base.saturating_add(30));
    let centered = app.centered_mode();
    let align = if centered {
        ratatui::layout::Alignment::Center
    } else {
        ratatui::layout::Alignment::Left
    };

    let max_w = (term_width as usize).min(80);
    let max_h = max_w / 2;
    let variant = startup_animation_variant();
    let anim_lines = render_startup_animation(elapsed, max_w, max_h, variant);

    let mut lines = Vec::new();
    lines.push(Line::from(""));
    for line in &anim_lines {
        lines.push(Line::from(Span::styled(
            line.clone(),
            Style::default().fg(art_color),
        )));
    }
    lines.push(Line::from(""));
    lines.push(Line::from(Span::styled(
        format!("{} booting jcode interface...", status_spinner),
        Style::default().fg(DIM_COLOR),
    )));
    lines.push(Line::from(Span::styled(
        "waiting for your first prompt",
        Style::default().fg(DIM_COLOR),
    )));

    lines
        .into_iter()
        .map(|line| line.alignment(align))
        .collect()
}

/// Build chroma-colored text (each character gets a different hue in the rainbow wave)
fn chroma_spans(text: &str, elapsed: f32, offset: f32, bold: bool) -> Vec<Span<'static>> {
    let chars: Vec<char> = text.chars().collect();
    let len = chars.len().max(1) as f32;

    chars
        .into_iter()
        .enumerate()
        .map(|(i, ch)| {
            let pos = offset + (i as f32 / len) * 0.3; // Spread across 0.3 of the spectrum
            let color = header_chroma_color(pos, elapsed);
            let mut style = Style::default().fg(color);
            if bold {
                style = style.add_modifier(ratatui::style::Modifier::BOLD);
            }
            Span::styled(ch.to_string(), style)
        })
        .collect()
}

/// Build the top header (chroma animated)
/// Line 1: Status badges (client, dev, updates)
/// Line 2: Session name with icon (e.g., "🦋 Moth")
/// Abbreviate a path by replacing the home directory prefix with `~`
fn abbreviate_home(path: &str) -> String {
    if let Some(home) = dirs::home_dir() {
        let home_str = home.display().to_string();
        if path == home_str {
            return "~".to_string();
        }
        if let Some(rest) = path.strip_prefix(&home_str) {
            return format!("~{}", rest);
        }
    }
    path.to_string()
}

/// Line 3: Model name (e.g., "Claude 4.5 Opus")
/// Line 4: Version and build info
fn build_persistent_header(app: &dyn TuiState, width: u16) -> Vec<Line<'static>> {
    let model = app.provider_model();
    let session_name = app.session_display_name().unwrap_or_default();
    let server_name = app.server_display_name();
    let short_model = shorten_model_name(&model);
    let icon = crate::id::session_icon(&session_name);
    let nice_model = format_model_name(&short_model);
    let build_info = binary_age().unwrap_or_else(|| "unknown".to_string());
    let centered = app.centered_mode();
    let align = if centered {
        ratatui::layout::Alignment::Center
    } else {
        ratatui::layout::Alignment::Left
    };

    let mut lines: Vec<Line> = Vec::new();

    // Line 1: Status badges (chroma colored)
    let is_canary = app.is_canary();
    let is_remote = app.is_remote_mode();
    let server_update = app.server_update_available() == Some(true);
    let client_update = app.client_update_available();
    let mut status_items: Vec<&str> = Vec::new();
    if app.is_replay() {
        status_items.push("replay");
    } else if is_remote {
        status_items.push("client");
    }
    if is_canary {
        status_items.push("dev");
    }
    if server_update {
        status_items.push("srv↑");
    }
    if client_update {
        status_items.push("cli↑");
    }

    if !status_items.is_empty() {
        let badge_text = format!("⟨{}⟩", status_items.join("·"));
        lines.push(
            Line::from(Span::styled(badge_text, Style::default().fg(DIM_COLOR))).alignment(align),
        );
    } else if centered {
        lines.push(Line::from("")); // Empty line if no badges (only in centered mode)
    }

    // Line 2: "<ServerName|JCode> <icon> <SessionName>" (chroma)
    if !session_name.is_empty() {
        let title_prefix = server_name
            .as_deref()
            .map(capitalize)
            .unwrap_or_else(|| "JCode".to_string());
        let full_name = format!("{} {} {}", title_prefix, icon, capitalize(&session_name));
        lines.push(
            Line::from(Span::styled(
                full_name,
                Style::default().fg(HEADER_NAME_COLOR),
            ))
            .alignment(align),
        );
    } else {
        let title_prefix = server_name
            .as_deref()
            .map(capitalize)
            .unwrap_or_else(|| "JCode".to_string());
        lines.push(
            Line::from(Span::styled(
                title_prefix,
                Style::default().fg(HEADER_NAME_COLOR),
            ))
            .alignment(align),
        );
    }

    // Line 3: Model name (chroma)
    lines.push(
        Line::from(Span::styled(
            nice_model,
            Style::default().fg(HEADER_SESSION_COLOR),
        ))
        .alignment(align),
    );

    // Line 4: Version and build info (dim, no chroma)
    let w = width as usize;
    let version_text = if is_running_stable_release() {
        let tag = env!("JCODE_GIT_TAG");
        if tag.is_empty() || tag.contains('-') {
            let full = format!("{} · release · built {}", semver(), build_info);
            if full.chars().count() <= w { full } else { format!("{} · release", semver()) }
        } else {
            let full = format!("{} · release {} · built {}", semver(), tag, build_info);
            if full.chars().count() <= w { full } else { format!("{} · {}", semver(), tag) }
        }
    } else {
        let full = format!("{} · built {}", semver(), build_info);
        if full.chars().count() <= w { full } else { semver().to_string() }
    };
    let version_line =
        Line::from(Span::styled(version_text, Style::default().fg(DIM_COLOR))).alignment(align);
    lines.push(version_line);

    if let Some(dir) = app.working_dir() {
        let display_dir = abbreviate_home(&dir);
        lines.push(
            Line::from(Span::styled(display_dir, Style::default().fg(DIM_COLOR))).alignment(align),
        );
    }

    lines
}

/// Badge without leading space (for centered display)
fn multi_status_badge_no_leading_space(items: &[(&str, Color)]) -> Vec<Span<'static>> {
    let mut spans = vec![Span::styled("⟨", Style::default().fg(DIM_COLOR))];

    for (i, (label, color)) in items.iter().enumerate() {
        if i > 0 {
            spans.push(Span::styled("·", Style::default().fg(DIM_COLOR)));
        }
        spans.push(Span::styled(label.to_string(), Style::default().fg(*color)));
    }

    spans.push(Span::styled("⟩", Style::default().fg(DIM_COLOR)));
    spans
}

fn build_header_lines(app: &dyn TuiState, width: u16) -> Vec<Line<'static>> {
    let mut lines: Vec<Line> = Vec::new();
    let centered = app.centered_mode();
    let align = if centered {
        ratatui::layout::Alignment::Center
    } else {
        ratatui::layout::Alignment::Left
    };

    let model = app.provider_model();
    let provider_name = app.provider_name();
    let upstream = app.upstream_provider();
    let provider_label = {
        let trimmed = provider_name.trim();
        if trimmed.is_empty() {
            "unknown".to_string()
        } else {
            trimmed.to_lowercase()
        }
    };

    // Line: provider + model + upstream provider if available + hint to switch
    let w = width as usize;
    let model_info = if let Some(ref provider) = upstream {
        let full = format!("{} · {} via {} · /model to switch", provider_label, model, provider);
        if full.chars().count() <= w {
            full
        } else {
            let short = format!("{} · {} via {}", provider_label, model, provider);
            if short.chars().count() <= w { short } else { format!("{} · {}", provider_label, model) }
        }
    } else {
        let full = format!("{} · {} · /model to switch", provider_label, model);
        if full.chars().count() <= w {
            full
        } else {
            format!("{} · {}", provider_label, model)
        }
    };
    lines.push(
        Line::from(Span::styled(model_info, Style::default().fg(DIM_COLOR))).alignment(align),
    );

    // Line: Auth status indicators (colored dots for each provider)
    let auth = app.auth_status();
    let auth_line = build_auth_status_line(&auth, w);
    if !auth_line.spans.is_empty() {
        lines.push(auth_line.alignment(align));
    }

    // Line 3+: Recent changes in a box (from git log, embedded at build time)
    // Each line is "hash:subject". We filter to only show commits since the user last saw updates.
    let new_entries = get_unseen_changelog_entries();
    let term_width = width as usize;
    if !new_entries.is_empty() && term_width > 20 {
        const MAX_LINES: usize = 8;
        let available_width = term_width.saturating_sub(2);
        let display_count = new_entries.len().min(MAX_LINES);
        let has_more = new_entries.len() > MAX_LINES;

        let mut content: Vec<Line> = Vec::new();
        for entry in new_entries.iter().take(display_count) {
            content.push(
                Line::from(Span::styled(
                    format!("• {}", entry),
                    Style::default().fg(DIM_COLOR),
                ))
                .alignment(align),
            );
        }
        if has_more {
            content.push(
                Line::from(Span::styled(
                    format!("  …{} more", new_entries.len() - MAX_LINES),
                    Style::default().fg(DIM_COLOR),
                ))
                .alignment(align),
            );
        }

        let boxed = render_rounded_box(
            "Updates",
            content,
            available_width,
            Style::default().fg(DIM_COLOR),
        );
        for line in boxed {
            lines.push(line.alignment(align));
        }
    }

    // Line 4: MCPs - show server names with tool counts, or (none)
    let mcps = app.mcp_servers();
    let mcp_text = if mcps.is_empty() {
        "mcp: (none)".to_string()
    } else {
        let full_parts: Vec<String> = mcps
            .iter()
            .map(|(name, count)| {
                if *count > 0 {
                    format!("{} ({} tools)", name, count)
                } else {
                    format!("{} (...)", name)
                }
            })
            .collect();
        let full = format!("mcp: {}", full_parts.join(", "));
        if full.chars().count() <= w {
            full
        } else {
            // Try shorter: just names with counts
            let short_parts: Vec<String> = mcps
                .iter()
                .map(|(name, count)| {
                    if *count > 0 { format!("{}({})", name, count) }
                    else { format!("{}(…)", name) }
                })
                .collect();
            let short = format!("mcp: {}", short_parts.join(" "));
            if short.chars().count() <= w {
                short
            } else {
                // Just count
                format!("mcp: {} servers", mcps.len())
            }
        }
    };
    lines.push(Line::from(Span::styled(mcp_text, Style::default().fg(DIM_COLOR))).alignment(align));

    // Line 4: Skills (if any)
    let skills = app.available_skills();
    if !skills.is_empty() {
        let full = format!(
            "skills: {}",
            skills.iter().map(|s| format!("/{}", s)).collect::<Vec<_>>().join(" ")
        );
        let skills_text = if full.chars().count() <= w {
            full
        } else {
            format!("skills: {} loaded", skills.len())
        };
        lines.push(
            Line::from(Span::styled(skills_text, Style::default().fg(DIM_COLOR)))
                .alignment(align),
        );
    }

    // Line 5: Server stats (if running as server with clients)
    let client_count = app.connected_clients().unwrap_or(0);
    let session_count = app.server_sessions().len();
    if client_count > 0 || session_count > 1 {
        let mut parts = Vec::new();
        if client_count > 0 {
            parts.push(format!(
                "{} client{}",
                client_count,
                if client_count == 1 { "" } else { "s" }
            ));
        }
        if session_count > 1 {
            parts.push(format!("{} sessions", session_count));
        }
        lines.push(
            Line::from(Span::styled(
                format!("server: {}", parts.join(", ")),
                Style::default().fg(DIM_COLOR),
            ))
            .alignment(align),
        );
    }

    // Context window info (at the end of header) - DISABLED
    // let context_info = app.context_info();
    // if context_info.total_chars > 0 {
    //     let context_width = width.saturating_sub(4) as usize;
    //     let context_limit = app
    //         .context_limit()
    //         .unwrap_or(crate::provider::DEFAULT_CONTEXT_LIMIT);
    //     let context_lines = render_context_bar(&context_info, context_width, context_limit);
    //     if !context_lines.is_empty() {
    //         let boxed = render_rounded_box(
    //             "Context",
    //             context_lines,
    //             width as usize,
    //             Style::default().fg(DIM_COLOR),
    //         );
    //         for line in boxed {
    //             lines.push(line.alignment(align));
    //         }
    //     }
    // }

    // Blank line after header
    lines.push(Line::from(""));

    lines
}

fn prepare_body_cached(app: &dyn TuiState, width: u16) -> PreparedMessages {
    let key = BodyCacheKey {
        width,
        diff_mode: app.diff_mode(),
        messages_version: app.display_messages_version(),
        diagram_mode: app.diagram_mode(),
    };

    let mut cache = match body_cache().lock() {
        Ok(c) => c,
        Err(poisoned) => {
            let mut c = poisoned.into_inner();
            c.key = None;
            c.prepared = None;
            c
        }
    };
    if cache.key.as_ref() == Some(&key) {
        if let Some(prepared) = cache.prepared.clone() {
            return prepared;
        }
    }

    let prepared = prepare_body(app, width, false);
    cache.key = Some(key);
    cache.prepared = Some(prepared.clone());
    prepared
}

fn prepare_streaming_cached(
    app: &dyn TuiState,
    width: u16,
    prefix_blank: bool,
) -> PreparedMessages {
    let streaming = app.streaming_text();
    if streaming.is_empty() {
        return PreparedMessages {
            wrapped_lines: Vec::new(),
            wrapped_user_indices: Vec::new(),
            image_regions: Vec::new(),
        };
    }

    // Use incremental markdown rendering for streaming text
    // This is efficient because render_streaming_markdown uses internal caching
    let content_width = width.saturating_sub(4) as usize;
    let md_lines = app.render_streaming_markdown(content_width);

    // Apply alignment based on centered mode
    let centered = app.centered_mode();
    let align = if centered {
        ratatui::layout::Alignment::Center
    } else {
        ratatui::layout::Alignment::Left
    };

    let mut lines: Vec<Line<'static>> = Vec::new();
    if prefix_blank {
        lines.push(Line::from(""));
    }
    for line in md_lines {
        lines.push(align_if_unset(line, align));
    }

    wrap_lines(lines, &[], width)
}

fn prepare_body(app: &dyn TuiState, width: u16, include_streaming: bool) -> PreparedMessages {
    let mut lines: Vec<Line> = Vec::new();
    let mut user_line_indices: Vec<usize> = Vec::new();
    let centered = app.centered_mode();
    let align = if centered {
        ratatui::layout::Alignment::Center
    } else {
        ratatui::layout::Alignment::Left
    };

    let mut prompt_num = 0usize;
    // Count total user prompts and pending messages for rainbow coloring
    // The input prompt is distance 0, pending messages are 1..pending_count,
    // existing messages continue from there
    let total_prompts = app
        .display_messages()
        .iter()
        .filter(|m| m.role == "user")
        .count();
    let pending_count = pending_prompt_count(app);
    // Input prompt number is total_prompts + pending_count + 1, so distance for
    // existing prompt N is: (total_prompts + pending_count + 1) - N

    for msg in app.display_messages() {
        // Add spacing between messages
        if !lines.is_empty() && msg.role != "tool" && msg.role != "meta" {
            lines.push(Line::from(""));
        }

        match msg.role.as_str() {
            "user" => {
                prompt_num += 1;
                user_line_indices.push(lines.len()); // Track this line index
                                                     // Calculate distance from input prompt (distance 0)
                let distance = total_prompts + pending_count + 1 - prompt_num;
                let num_color = rainbow_prompt_color(distance);
                // User messages: rainbow number, blue caret, bright text
                lines.push(
                    Line::from(vec![
                        Span::styled(format!("{}", prompt_num), Style::default().fg(num_color)),
                        Span::styled("› ", Style::default().fg(USER_COLOR)),
                        Span::styled(msg.content.clone(), Style::default().fg(USER_TEXT)),
                    ])
                    .alignment(align),
                );
            }
            "assistant" => {
                // AI messages: render markdown
                // Pass width for table rendering (leave some margin)
                let content_width = width.saturating_sub(4);
                let cached = get_cached_message_lines(
                    msg,
                    content_width,
                    app.diff_mode(),
                    render_assistant_message,
                );
                for line in cached {
                    lines.push(align_if_unset(line, align));
                }
            }
            "meta" => {
                lines.push(
                    Line::from(vec![
                        Span::raw(if centered { "" } else { "  " }),
                        Span::styled(msg.content.clone(), Style::default().fg(DIM_COLOR)),
                    ])
                    .alignment(align),
                );
            }
            "tool" => {
                let cached =
                    get_cached_message_lines(msg, width, app.diff_mode(), render_tool_message);
                for line in cached {
                    lines.push(align_if_unset(line, align));
                }
            }
            "system" => {
                let should_render_markdown = msg.content.contains('\n')
                    || msg.content.contains("```")
                    || msg.content.contains("# ")
                    || msg.content.contains("- ");

                if should_render_markdown {
                    let content_width = width.saturating_sub(4) as usize;
                    let rendered =
                        markdown::render_markdown_with_width(&msg.content, Some(content_width));
                    for line in rendered {
                        lines.push(align_if_unset(line, align));
                    }
                } else {
                    lines.push(
                        Line::from(vec![
                            Span::styled(if centered { "" } else { "  " }, Style::default()),
                            Span::styled(
                                msg.content.clone(),
                                Style::default().fg(ACCENT_COLOR).italic(),
                            ),
                        ])
                        .alignment(align),
                    );
                }
            }
            "usage" => {
                lines.push(
                    Line::from(vec![
                        Span::styled(if centered { "" } else { "  " }, Style::default()),
                        Span::styled(msg.content.clone(), Style::default().fg(DIM_COLOR)),
                    ])
                    .alignment(align),
                );
            }
            "error" => {
                lines.push(
                    Line::from(vec![
                        Span::styled(
                            if centered { "✗ " } else { "  ✗ " },
                            Style::default().fg(Color::Red),
                        ),
                        Span::styled(msg.content.clone(), Style::default().fg(Color::Red)),
                    ])
                    .alignment(align),
                );
            }
            _ => {}
        }
    }

    // Streaming text - render with markdown for consistent formatting
    if include_streaming && app.is_processing() {
        if !app.streaming_text().is_empty() {
            if !lines.is_empty() {
                lines.push(Line::from(""));
            }
            // Use incremental markdown rendering for better streaming performance
            let content_width = width.saturating_sub(4) as usize;
            let md_lines = app.render_streaming_markdown(content_width);
            for line in md_lines {
                lines.push(align_if_unset(line, align));
            }
        }
        // Tool calls are now shown inline in display_messages
    }

    wrap_lines(lines, &user_line_indices, width)
}

fn get_cached_message_lines<F>(
    msg: &DisplayMessage,
    width: u16,
    diff_mode: crate::config::DiffDisplayMode,
    render: F,
) -> Vec<Line<'static>>
where
    F: FnOnce(&DisplayMessage, u16, crate::config::DiffDisplayMode) -> Vec<Line<'static>>,
{
    let key = MessageCacheKey {
        width,
        diff_mode,
        message_hash: hash_display_message(msg),
        content_len: msg.content.len(),
        diagram_mode: crate::config::config().display.diagram_mode,
    };

    let mut cache = match message_cache().lock() {
        Ok(c) => c,
        Err(poisoned) => poisoned.into_inner(),
    };
    if let Some(lines) = cache.get(&key) {
        return lines;
    }

    let lines = render(msg, width, diff_mode);
    cache.insert(key, lines.clone());
    lines
}

pub(crate) fn render_assistant_message(
    msg: &DisplayMessage,
    width: u16,
    _diff_mode: crate::config::DiffDisplayMode,
) -> Vec<Line<'static>> {
    let content_width = width as usize;
    let mut lines = markdown::render_markdown_with_width(&msg.content, Some(content_width));
    if !msg.tool_calls.is_empty() {
        lines.push(Line::from(vec![
            Span::raw("  "),
            Span::styled(
                msg.tool_calls.join(" "),
                Style::default().fg(ACCENT_COLOR).dim(),
            ),
        ]));
    }
    lines
}

pub(crate) fn render_tool_message(
    msg: &DisplayMessage,
    width: u16,
    diff_mode: crate::config::DiffDisplayMode,
) -> Vec<Line<'static>> {
    let mut lines: Vec<Line<'static>> = Vec::new();
    let Some(ref tc) = msg.tool_data else {
        return lines;
    };

    let summary = get_tool_summary(tc);

    // Determine status: error if content starts with error prefix
    // Be specific to avoid false positives (e.g., "No matches found" is not an error)
    let is_error = msg.content.starts_with("Error:")
        || msg.content.starts_with("error:")
        || msg.content.starts_with("Failed:");

    let (icon, icon_color) = if is_error {
        ("✗", Color::Rgb(220, 100, 100)) // Red for errors
    } else {
        ("✓", Color::Rgb(100, 180, 100)) // Green for success
    };

    // For edit tools, count line changes
    let is_edit_tool = matches!(tc.name.as_str(), "edit" | "Edit" | "write" | "multiedit");
    let (additions, deletions) = if is_edit_tool {
        diff_change_counts_for_tool(tc, &msg.content)
    } else {
        (0, 0)
    };

    let mut tool_line = vec![
        Span::styled(format!("  {} ", icon), Style::default().fg(icon_color)),
        Span::styled(tc.name.clone(), Style::default().fg(TOOL_COLOR)),
        Span::styled(format!(" {}", summary), Style::default().fg(DIM_COLOR)),
    ];
    if is_edit_tool {
        tool_line.push(Span::styled(" (", Style::default().fg(DIM_COLOR)));
        tool_line.push(Span::styled(
            format!("+{}", additions),
            Style::default().fg(DIFF_ADD_COLOR),
        ));
        tool_line.push(Span::styled(" ", Style::default().fg(DIM_COLOR)));
        tool_line.push(Span::styled(
            format!("-{}", deletions),
            Style::default().fg(DIFF_DEL_COLOR),
        ));
        tool_line.push(Span::styled(")", Style::default().fg(DIM_COLOR)));
    }

    lines.push(Line::from(tool_line));

    // Expand batch sub-calls as individual tool lines
    if tc.name == "batch" {
        if let Some(calls) = tc.input.get("tool_calls").and_then(|v| v.as_array()) {
            // Parse the result content to determine per-sub-call success/error
            let sub_results = parse_batch_sub_results(&msg.content);

            for (i, call) in calls.iter().enumerate() {
                let raw_name = call
                    .get("tool")
                    .and_then(|v| v.as_str())
                    .unwrap_or("?");
                let display_name = resolve_display_tool_name(raw_name);
                let params = call
                    .get("parameters")
                    .cloned()
                    .unwrap_or(serde_json::Value::Object(serde_json::Map::new()));

                let sub_tc = ToolCall {
                    id: String::new(),
                    name: display_name.to_string(),
                    input: params,
                    intent: None,
                };
                let sub_summary = get_tool_summary(&sub_tc);

                let sub_errored = sub_results
                    .get(i)
                    .copied()
                    .unwrap_or(false);
                let (sub_icon, sub_icon_color) = if sub_errored {
                    ("✗", Color::Rgb(220, 100, 100))
                } else {
                    ("✓", Color::Rgb(100, 180, 100))
                };

                lines.push(Line::from(vec![
                    Span::styled(format!("    {} ", sub_icon), Style::default().fg(sub_icon_color)),
                    Span::styled(display_name.to_string(), Style::default().fg(TOOL_COLOR)),
                    Span::styled(format!(" {}", sub_summary), Style::default().fg(DIM_COLOR)),
                ]));
            }
        }
    }

    // Show diff output for editing tools with syntax highlighting
    if diff_mode.is_inline() && is_edit_tool {
        // Extract file extension for syntax highlighting
        let file_ext = tc
            .input
            .get("file_path")
            .and_then(|v| v.as_str())
            .and_then(|p| std::path::Path::new(p).extension())
            .and_then(|e| e.to_str());

        // Collect only actual change lines (+ and -)
        // First try parsing from content, then fall back to tool input if empty
        let change_lines = {
            let from_content = collect_diff_lines(&msg.content);
            if !from_content.is_empty() {
                from_content
            } else {
                // Fall back to generating diff lines from tool input
                generate_diff_lines_from_tool_input(tc)
            }
        };

        const MAX_DIFF_LINES: usize = 12;
        let total_changes = change_lines.len();

        // Count additions and deletions for summary
        let additions = change_lines
            .iter()
            .filter(|line| line.kind == DiffLineKind::Add)
            .count();
        let deletions = change_lines
            .iter()
            .filter(|line| line.kind == DiffLineKind::Del)
            .count();

        // Determine which lines to show
        let (display_lines, truncated): (Vec<&ParsedDiffLine>, bool) =
            if total_changes <= MAX_DIFF_LINES {
                (change_lines.iter().collect(), false)
            } else {
                // Show first half and last half, with truncation indicator
                let half = MAX_DIFF_LINES / 2;
                let mut result: Vec<&ParsedDiffLine> = change_lines.iter().take(half).collect();
                result.extend(change_lines.iter().skip(total_changes - half));
                (result, true)
            };

        // Calculate max line width for centering (like code blocks)
        let max_content_width = display_lines
            .iter()
            .map(|l| l.prefix.chars().count() + l.content.chars().count())
            .max()
            .unwrap_or(0);
        let header_width = 5; // "┌─ " + some label space
        let block_width = header_width.max(max_content_width + 2).max(10); // +2 for "│ " prefix

        // Calculate padding to center the block
        let max_width = width as usize;
        let padding = if block_width < max_width {
            (max_width - block_width) / 2
        } else {
            0
        };
        let pad_str: String = " ".repeat(padding);

        // Add diff block header
        lines.push(
            Line::from(Span::styled(
                format!("{}┌─ diff", pad_str),
                Style::default().fg(DIM_COLOR),
            ))
            .left_aligned(),
        );

        let mut shown_truncation = false;
        let half_point = if truncated {
            MAX_DIFF_LINES / 2
        } else {
            usize::MAX
        };

        for (i, line) in display_lines.iter().enumerate() {
            // Show truncation marker at the midpoint
            if truncated && !shown_truncation && i >= half_point {
                let skipped = total_changes - MAX_DIFF_LINES;
                lines.push(
                    Line::from(Span::styled(
                        format!("{}│ ... {} more changes ...", pad_str, skipped),
                        Style::default().fg(DIM_COLOR),
                    ))
                    .left_aligned(),
                );
                shown_truncation = true;
            }

            let base_color = if line.kind == DiffLineKind::Add {
                DIFF_ADD_COLOR
            } else {
                DIFF_DEL_COLOR
            };

            // Build the line with syntax-highlighted content
            // Start with padding and box border
            let mut spans: Vec<Span<'static>> = vec![
                Span::styled(format!("{}│ ", pad_str), Style::default().fg(DIM_COLOR)),
                Span::styled(line.prefix.clone(), Style::default().fg(base_color)),
            ];

            // Apply syntax highlighting to content
            if !line.content.is_empty() {
                let highlighted = markdown::highlight_line(line.content.as_str(), file_ext);
                for span in highlighted {
                    let tinted = tint_span_with_diff_color(span, base_color);
                    spans.push(tinted);
                }
            }

            lines.push(Line::from(spans).left_aligned());
        }

        // Add diff block footer
        let footer = if total_changes > 0 && truncated {
            format!("{}└─ (+{} -{} total)", pad_str, additions, deletions)
        } else {
            format!("{}└─", pad_str)
        };
        lines.push(Line::from(Span::styled(footer, Style::default().fg(DIM_COLOR))).left_aligned());
    }

    lines
}

fn wrap_lines(
    lines: Vec<Line<'static>>,
    user_line_indices: &[usize],
    width: u16,
) -> PreparedMessages {
    // Wrap lines and track which wrapped indices correspond to user lines
    let full_width = width as usize;
    let user_width = width.saturating_sub(2) as usize; // Leave margin for right bar
    let mut wrapped_user_indices: Vec<usize> = Vec::new();
    let mut user_line_mask = vec![false; lines.len()];
    for &idx in user_line_indices {
        if idx < user_line_mask.len() {
            user_line_mask[idx] = true;
        }
    }
    let mut wrapped_idx = 0usize;

    let mut wrapped_lines: Vec<Line> = Vec::new();
    for (orig_idx, line) in lines.into_iter().enumerate() {
        let is_user_line = user_line_mask.get(orig_idx).copied().unwrap_or(false);
        // User lines need margin for bar, AI lines use full width
        let wrap_width = if is_user_line { user_width } else { full_width };
        let new_lines = markdown::wrap_line(line, wrap_width);
        let count = new_lines.len();

        if is_user_line {
            // All wrapped lines from a user message get the right bar
            for i in 0..count {
                wrapped_user_indices.push(wrapped_idx + i);
            }
        }

        wrapped_lines.extend(new_lines);
        wrapped_idx += count;
    }

    // Scan for mermaid image placeholders (once during preparation, not every frame)
    let mut image_regions = Vec::new();
    for (idx, line) in wrapped_lines.iter().enumerate() {
        if let Some(hash) = super::mermaid::parse_image_placeholder(line) {
            // Count consecutive empty lines for image height
            let mut height = 1u16;
            for subsequent in wrapped_lines.iter().skip(idx + 1) {
                if subsequent.spans.is_empty()
                    || (subsequent.spans.len() == 1 && subsequent.spans[0].content.is_empty())
                {
                    height += 1;
                } else {
                    break;
                }
            }
            image_regions.push(ImageRegion {
                abs_line_idx: idx,
                hash,
                height,
            });
        }
    }

    PreparedMessages {
        wrapped_lines,
        wrapped_user_indices,
        image_regions,
    }
}

fn hash_display_message(msg: &DisplayMessage) -> u64 {
    let mut hasher = DefaultHasher::new();
    msg.role.hash(&mut hasher);
    msg.content.hash(&mut hasher);
    msg.tool_calls.hash(&mut hasher);
    msg.title.hash(&mut hasher);
    if let Some(tool) = &msg.tool_data {
        tool.id.hash(&mut hasher);
        tool.name.hash(&mut hasher);
        tool.input.to_string().hash(&mut hasher);
    }
    hasher.finish()
}

fn compute_visible_margins(
    lines: &[Line],
    user_line_indices: &[usize],
    scroll: usize,
    area: Rect,
    centered: bool,
) -> info_widget::Margins {
    let visible_height = area.height as usize;
    let mut mask = vec![false; lines.len()];
    for &idx in user_line_indices {
        if idx < mask.len() {
            mask[idx] = true;
        }
    }

    let mut right_widths = Vec::with_capacity(visible_height);
    let mut left_widths = Vec::with_capacity(visible_height);

    for row in 0..visible_height {
        let line_idx = scroll + row;
        if line_idx < lines.len() {
            let mut used = lines[line_idx].width().min(area.width as usize) as u16;
            if mask[line_idx] && area.width > 0 {
                // User lines have a bar on the right, so add 1 to used width
                used = used.saturating_add(1).min(area.width);
            }

            if centered {
                // Respect each line's effective alignment. Some lines (e.g. code/diff blocks)
                // are explicitly left-aligned even in centered mode.
                let total_margin = area.width.saturating_sub(used);
                let effective_alignment = lines[line_idx].alignment.unwrap_or(Alignment::Center);
                let (left_margin, right_margin) = match effective_alignment {
                    Alignment::Left => (0, total_margin),
                    Alignment::Center => {
                        let left = total_margin / 2;
                        let right = total_margin.saturating_sub(left);
                        (left, right)
                    }
                    Alignment::Right => (total_margin, 0),
                };
                left_widths.push(left_margin);
                right_widths.push(right_margin);
            } else {
                // Left-aligned: all free space is on the right
                left_widths.push(0);
                right_widths.push(area.width.saturating_sub(used));
            }
        } else {
            // Empty lines - full width available
            if centered {
                let half = area.width / 2;
                left_widths.push(half);
                right_widths.push(area.width.saturating_sub(half));
            } else {
                left_widths.push(0);
                right_widths.push(area.width);
            }
        }
    }

    info_widget::Margins {
        right_widths,
        left_widths,
        centered,
    }
}

fn capture_widget_placements(
    placements: &[info_widget::WidgetPlacement],
) -> Vec<WidgetPlacementCapture> {
    placements
        .iter()
        .map(|p| WidgetPlacementCapture {
            kind: p.kind.as_str().to_string(),
            side: p.side.as_str().to_string(),
            rect: p.rect.into(),
        })
        .collect()
}

fn build_info_widget_summary(data: &info_widget::InfoWidgetData) -> InfoWidgetSummary {
    let todos_total = data.todos.len();
    let todos_done = data
        .todos
        .iter()
        .filter(|t| t.status == "completed")
        .count();

    let context_total_chars = data.context_info.as_ref().map(|c| c.total_chars);
    let context_limit = data.context_limit;

    let memory_total = data.memory_info.as_ref().map(|m| m.total_count);
    let memory_project = data.memory_info.as_ref().map(|m| m.project_count);
    let memory_global = data.memory_info.as_ref().map(|m| m.global_count);
    let memory_activity = data.memory_info.as_ref().map(|m| m.activity.is_some());

    let swarm_session_count = data.swarm_info.as_ref().map(|s| s.session_count);
    let swarm_member_count = data.swarm_info.as_ref().map(|s| s.members.len());
    let swarm_subagent_status = data
        .swarm_info
        .as_ref()
        .and_then(|s| s.subagent_status.clone());

    let background_running = data.background_info.as_ref().map(|b| b.running_count);
    let background_tasks = data.background_info.as_ref().map(|b| b.running_tasks.len());

    let usage_available = data.usage_info.as_ref().map(|u| u.available);
    let usage_provider = data
        .usage_info
        .as_ref()
        .map(|u| format!("{:?}", u.provider));

    InfoWidgetSummary {
        todos_total,
        todos_done,
        context_total_chars,
        context_limit,
        queue_mode: data.queue_mode,
        model: data.model.clone(),
        reasoning_effort: data.reasoning_effort.clone(),
        session_count: data.session_count,
        client_count: data.client_count,
        memory_total,
        memory_project,
        memory_global,
        memory_activity,
        swarm_session_count,
        swarm_member_count,
        swarm_subagent_status,
        background_running,
        background_tasks,
        usage_available,
        usage_provider,
        tokens_per_second: data.tokens_per_second,
        auth_method: Some(format!("{:?}", data.auth_method)),
        upstream_provider: data.upstream_provider.clone(),
    }
}

fn rects_overlap(a: Rect, b: Rect) -> bool {
    if a.width == 0 || a.height == 0 || b.width == 0 || b.height == 0 {
        return false;
    }
    let a_right = a.x.saturating_add(a.width);
    let a_bottom = a.y.saturating_add(a.height);
    let b_right = b.x.saturating_add(b.width);
    let b_bottom = b.y.saturating_add(b.height);
    a.x < b_right && a_right > b.x && a.y < b_bottom && a_bottom > b.y
}

fn rect_within_bounds(rect: Rect, bounds: Rect) -> bool {
    let right = rect.x.saturating_add(rect.width);
    let bottom = rect.y.saturating_add(rect.height);
    let bounds_right = bounds.x.saturating_add(bounds.width);
    let bounds_bottom = bounds.y.saturating_add(bounds.height);
    rect.x >= bounds.x && rect.y >= bounds.y && right <= bounds_right && bottom <= bounds_bottom
}

fn draw_debug_overlay(
    frame: &mut Frame,
    placements: &[info_widget::WidgetPlacement],
    chunks: &[Rect],
) {
    if chunks.len() < 5 {
        return;
    }
    render_overlay_box(frame, chunks[0], "messages", Color::Red);
    render_overlay_box(frame, chunks[1], "queued", Color::Yellow);
    render_overlay_box(frame, chunks[2], "status", Color::Cyan);
    render_overlay_box(frame, chunks[3], "picker", Color::Magenta);
    render_overlay_box(frame, chunks[4], "input", Color::Green);
    if chunks.len() > 5 && chunks[5].height > 0 {
        render_overlay_box(frame, chunks[5], "donut", Color::Blue);
    }

    for placement in placements {
        let title = format!("widget:{}", placement.kind.as_str());
        render_overlay_box(frame, placement.rect, &title, Color::Magenta);
    }
}

fn render_overlay_box(frame: &mut Frame, area: Rect, title: &str, color: Color) {
    if area.width == 0 || area.height == 0 {
        return;
    }
    let block = Block::default()
        .borders(Borders::ALL)
        .border_style(Style::default().fg(color))
        .title(Span::styled(title.to_string(), Style::default().fg(color)));
    frame.render_widget(block, area);
}

fn debug_palette_json() -> Option<serde_json::Value> {
    Some(serde_json::json!({
        "USER_COLOR": color_to_rgb(USER_COLOR),
        "AI_COLOR": color_to_rgb(AI_COLOR),
        "TOOL_COLOR": color_to_rgb(TOOL_COLOR),
        "DIM_COLOR": color_to_rgb(DIM_COLOR),
        "ACCENT_COLOR": color_to_rgb(ACCENT_COLOR),
        "QUEUED_COLOR": color_to_rgb(QUEUED_COLOR),
        "ASAP_COLOR": color_to_rgb(ASAP_COLOR),
        "PENDING_COLOR": color_to_rgb(PENDING_COLOR),
        "USER_TEXT": color_to_rgb(USER_TEXT),
        "USER_BG": color_to_rgb(USER_BG),
        "AI_TEXT": color_to_rgb(AI_TEXT),
        "HEADER_ICON_COLOR": color_to_rgb(HEADER_ICON_COLOR),
        "HEADER_NAME_COLOR": color_to_rgb(HEADER_NAME_COLOR),
        "HEADER_SESSION_COLOR": color_to_rgb(HEADER_SESSION_COLOR),
    }))
}

fn color_to_rgb(color: Color) -> Option<[u8; 3]> {
    match color {
        Color::Rgb(r, g, b) => Some([r, g, b]),
        _ => None,
    }
}

/// Compute a vertically-centered sub-area for a fitted image.
///
/// Given the available terminal `area` and the source image pixel dimensions,
/// this calculates how many rows the image will occupy after aspect-ratio
/// scaling and returns a `Rect` offset so the image is vertically centered.
fn vcenter_fitted_image(area: Rect, img_w_px: u32, img_h_px: u32) -> Rect {
    if area.width == 0 || area.height == 0 || img_w_px == 0 || img_h_px == 0 {
        return area;
    }
    let (font_w, font_h) = match super::mermaid::get_font_size() {
        Some(fs) => (fs.0 as f64, fs.1 as f64),
        None => return area,
    };

    let area_w_px = area.width as f64 * font_w;
    let area_h_px = area.height as f64 * font_h;
    let scale = (area_w_px / img_w_px as f64).min(area_h_px / img_h_px as f64);
    let fitted_h_cells = ((img_h_px as f64 * scale) / font_h).ceil() as u16;
    let fitted_h_cells = fitted_h_cells.min(area.height);

    if fitted_h_cells >= area.height {
        return area;
    }
    let y_offset = (area.height - fitted_h_cells) / 2;
    Rect {
        x: area.x,
        y: area.y + y_offset,
        width: area.width,
        height: fitted_h_cells,
    }
}

/// Check if a diagram is a poor fit for the current pane position.
/// Returns true when the aspect ratio makes the diagram poorly utilized.
fn is_diagram_poor_fit(
    diagram: &info_widget::DiagramInfo,
    area: Rect,
    position: crate::config::DiagramPanePosition,
) -> bool {
    if diagram.width == 0 || diagram.height == 0 || area.width < 5 || area.height < 3 {
        return false;
    }
    let (cell_w, cell_h) = super::mermaid::get_font_size().unwrap_or((8, 16));
    let cell_w = cell_w.max(1) as f64;
    let cell_h = cell_h.max(1) as f64;
    let inner_w = area.width.saturating_sub(2).max(1) as f64 * cell_w;
    let inner_h = area.height.saturating_sub(2).max(1) as f64 * cell_h;
    let img_w = diagram.width as f64;
    let img_h = diagram.height as f64;
    let aspect = img_w / img_h.max(1.0);

    match position {
        crate::config::DiagramPanePosition::Side => {
            let scale = (inner_w / img_w).min(inner_h / img_h);
            let used_w = img_w * scale;
            let used_h = img_h * scale;
            let utilization = (used_w * used_h) / (inner_w * inner_h);
            aspect > 2.0 && utilization < 0.35
        }
        crate::config::DiagramPanePosition::Top => {
            let scale = (inner_w / img_w).min(inner_h / img_h);
            let used_w = img_w * scale;
            let used_h = img_h * scale;
            let utilization = (used_w * used_h) / (inner_w * inner_h);
            aspect < 0.5 && utilization < 0.35
        }
    }
}

/// Draw a pinned diagram in a dedicated pane
fn draw_pinned_diagram(
    frame: &mut Frame,
    diagram: &info_widget::DiagramInfo,
    area: Rect,
    index: usize,
    total: usize,
    focused: bool,
    scroll_x: i32,
    scroll_y: i32,
    zoom_percent: u8,
    pane_position: crate::config::DiagramPanePosition,
) {
    use ratatui::widgets::{BorderType, Clear, Paragraph, Wrap};

    if area.width < 5 || area.height < 3 {
        return;
    }

    let border_color = if focused { ACCENT_COLOR } else { DIM_COLOR };
    let mut title_parts = vec![Span::styled(" pinned ", Style::default().fg(TOOL_COLOR))];
    if total > 0 {
        title_parts.push(Span::styled(
            format!("{}/{}", index + 1, total),
            Style::default().fg(TOOL_COLOR),
        ));
    }
    let mode_label = if focused { " pan " } else { " fit " };
    title_parts.push(Span::styled(
        mode_label,
        Style::default().fg(if focused { ACCENT_COLOR } else { DIM_COLOR }),
    ));
    if focused || zoom_percent != 100 {
        title_parts.push(Span::styled(
            format!(" zoom {}%", zoom_percent),
            Style::default().fg(if focused { ACCENT_COLOR } else { DIM_COLOR }),
        ));
    }
    if total > 1 {
        title_parts.push(Span::styled(
            " Ctrl+1-9",
            Style::default().fg(DIM_COLOR),
        ));
    }
    title_parts.push(Span::styled(
        " Ctrl+H/L focus",
        Style::default().fg(DIM_COLOR),
    ));
    title_parts.push(Span::styled(
        " Alt+M toggle",
        Style::default().fg(DIM_COLOR),
    ));

    let poor_fit = is_diagram_poor_fit(diagram, area, pane_position);
    if poor_fit {
        let hint = match pane_position {
            crate::config::DiagramPanePosition::Side => " Alt+T \u{21c4} top",
            crate::config::DiagramPanePosition::Top => " Alt+T \u{21c4} side",
        };
        title_parts.push(Span::styled(
            hint,
            Style::default().fg(ACCENT_COLOR).add_modifier(ratatui::style::Modifier::BOLD),
        ));
    }

    // Draw border with title
    let block = Block::default()
        .borders(Borders::ALL)
        .border_type(BorderType::Rounded)
        .border_style(Style::default().fg(border_color))
        .title(Line::from(title_parts));

    let inner = block.inner(area);
    frame.render_widget(block, area);

    // Render the diagram image inside the border
    if inner.width > 0 && inner.height > 0 {
        let mut rendered = 0u16;
        if super::mermaid::protocol_type().is_some() {
            if focused {
                rendered = super::mermaid::render_image_widget_viewport(
                    diagram.hash,
                    inner,
                    frame.buffer_mut(),
                    scroll_x,
                    scroll_y,
                    zoom_percent,
                    false,
                );
            } else {
                let render_area =
                    vcenter_fitted_image(inner, diagram.width, diagram.height);
                rendered = super::mermaid::render_image_widget_fit(
                    diagram.hash,
                    render_area,
                    frame.buffer_mut(),
                    false,
                    false,
                );
            }
        }

        if rendered > 0 && super::mermaid::is_video_export_mode() {
            super::mermaid::write_video_export_marker(diagram.hash, inner, frame.buffer_mut());
        } else if rendered == 0 {
            frame.render_widget(Clear, inner);
            let placeholder =
                super::mermaid::diagram_placeholder_lines(diagram.width, diagram.height);
            let paragraph = Paragraph::new(placeholder).wrap(Wrap { trim: true });
            frame.render_widget(paragraph, inner);
        }
    }
}

fn draw_messages(
    frame: &mut Frame,
    app: &dyn TuiState,
    area: Rect,
    prepared: &PreparedMessages,
) -> info_widget::Margins {
    let wrapped_lines = &prepared.wrapped_lines;
    let wrapped_user_indices = &prepared.wrapped_user_indices;

    // Calculate scroll position
    let total_lines = wrapped_lines.len();
    let visible_height = area.height as usize;
    let max_scroll = total_lines.saturating_sub(visible_height);

    // Publish max_scroll so scroll handlers can clamp without overshoot
    LAST_MAX_SCROLL.store(max_scroll, Ordering::Relaxed);

    let user_scroll = app.scroll_offset().min(max_scroll);

    // scroll_offset semantics:
    // - When auto_scroll_paused: scroll_offset is absolute line from top
    // - When !auto_scroll_paused: scroll_offset should be 0 (at bottom)
    let scroll = if app.auto_scroll_paused() {
        user_scroll.min(max_scroll)
    } else {
        max_scroll
    };

    let margins = compute_visible_margins(
        wrapped_lines,
        wrapped_user_indices,
        scroll,
        area,
        app.centered_mode(),
    );

    let visible_end = (scroll + visible_height).min(wrapped_lines.len());
    let mut visible_lines = if scroll < visible_end {
        wrapped_lines[scroll..visible_end].to_vec()
    } else {
        Vec::new()
    };
    if visible_lines.len() < visible_height {
        visible_lines
            .extend(std::iter::repeat(Line::from("")).take(visible_height - visible_lines.len()));
    }

    // Clear message pane before repainting to prevent stale glyph artifacts
    // during streaming/incremental markdown updates.
    frame.render_widget(Clear, area);

    // Render text first
    let paragraph = Paragraph::new(visible_lines);
    frame.render_widget(paragraph, area);

    // Use pre-computed image regions (scanned once during preparation, not every frame)
    let centered = app.centered_mode();
    let diagram_mode = app.diagram_mode();
    if diagram_mode != crate::config::DiagramDisplayMode::Pinned {
        for region in &prepared.image_regions {
            let abs_idx = region.abs_line_idx;
            let hash = region.hash;
            let total_height = region.height;
            let image_end = abs_idx + total_height as usize;

            // Check if this image overlaps the visible area at all
            if image_end > scroll && abs_idx < visible_end {
                // Image overlaps visible area
                let marker_visible = abs_idx >= scroll && abs_idx < visible_end;

                if marker_visible {
                    // Marker is visible - render the image
                    let screen_y = (abs_idx - scroll) as u16;
                    let available_height = (visible_height as u16).saturating_sub(screen_y);
                    let render_height = (total_height as u16).min(available_height);

                    if render_height > 0 {
                        let image_area = Rect {
                            x: area.x,
                            y: area.y + screen_y,
                            width: area.width,
                            height: render_height,
                        };
                        let rows = super::mermaid::render_image_widget(
                            hash,
                            image_area,
                            frame.buffer_mut(),
                            centered,
                            false,
                        );
                        if rows == 0 {
                            frame.render_widget(
                                Paragraph::new(Line::from(Span::styled(
                                    "↗ mermaid diagram unavailable",
                                    Style::default().fg(DIM_COLOR),
                                ))),
                                image_area,
                            );
                        }
                    }
                } else {
                    // Marker is off-screen but image would overlap - render the visible portion
                    let visible_start = scroll.max(abs_idx);
                    let visible_end_img = visible_end.min(image_end);
                    let screen_y = (visible_start - scroll) as u16;
                    let render_height = (visible_end_img - visible_start) as u16;

                    if render_height > 0 {
                        let image_area = Rect {
                            x: area.x,
                            y: area.y + screen_y,
                            width: area.width,
                            height: render_height,
                        };
                        super::mermaid::render_image_widget(
                            hash,
                            image_area,
                            frame.buffer_mut(),
                            centered,
                            true,
                        );
                    }
                }
            }
        }
    }

    // Draw right bar for visible user lines
    let right_x = area.x + area.width.saturating_sub(1);
    for &line_idx in wrapped_user_indices {
        // Check if this line is visible after scroll
        if line_idx >= scroll && line_idx < scroll + visible_height {
            let screen_y = area.y + (line_idx - scroll) as u16;
            let bar_area = Rect {
                x: right_x,
                y: screen_y,
                width: 1,
                height: 1,
            };
            let bar = Paragraph::new(Span::styled("│", Style::default().fg(USER_COLOR)));
            frame.render_widget(bar, bar_area);
        }
    }

    // Content above indicator (top-right) when user has scrolled up
    if scroll > 0 {
        let indicator = format!("↑{}", scroll);
        let indicator_area = Rect {
            x: area.x + area.width.saturating_sub(indicator.len() as u16 + 2),
            y: area.y,
            width: indicator.len() as u16,
            height: 1,
        };
        let indicator_widget = Paragraph::new(Line::from(vec![Span::styled(
            indicator,
            Style::default().fg(DIM_COLOR),
        )]));
        frame.render_widget(indicator_widget, indicator_area);
    }

    // Content below indicator (bottom-right) when user has scrolled up
    if app.auto_scroll_paused() && scroll < max_scroll {
        let indicator = format!("↓{}", max_scroll - scroll);
        let indicator_area = Rect {
            x: area.x + area.width.saturating_sub(indicator.len() as u16 + 2),
            y: area.y + area.height.saturating_sub(1),
            width: indicator.len() as u16,
            height: 1,
        };
        let indicator_widget = Paragraph::new(Line::from(vec![Span::styled(
            indicator,
            Style::default().fg(QUEUED_COLOR),
        )]));
        frame.render_widget(indicator_widget, indicator_area);
    }

    margins
}

/// Format elapsed time in a human-readable way
fn format_elapsed(secs: f32) -> String {
    if secs >= 3600.0 {
        let hours = (secs / 3600.0) as u32;
        let mins = ((secs % 3600.0) / 60.0) as u32;
        format!("{}h {}m", hours, mins)
    } else if secs >= 60.0 {
        let mins = (secs / 60.0) as u32;
        let s = (secs % 60.0) as u32;
        format!("{}m {}s", mins, s)
    } else {
        format!("{:.1}s", secs)
    }
}

/// Draw the inline model/provider picker line
fn draw_picker_line(frame: &mut Frame, app: &dyn TuiState, area: Rect) {
    let picker = match app.picker_state() {
        Some(p) => p,
        None => return,
    };

    let height = area.height as usize;
    let width = area.width as usize;
    if height == 0 {
        return;
    }

    let selected = picker.selected;
    let total = picker.models.len();
    let filtered_count = picker.filtered.len();
    let col = picker.column;

    // Column labels
    let col_names = ["MODEL", "PROVIDER", "VIA"];
    let col_focus_style = Style::default().fg(Color::White).bold().underlined();
    let col_dim_style = Style::default().fg(DIM_COLOR);

    // Header line: column headers + filter + count
    let mut header_spans: Vec<Span> = Vec::new();
    header_spans.push(Span::styled(" ", Style::default()));

    // Filter display
    if picker.filter.is_empty() {
        header_spans.push(Span::styled(
            "filter: ",
            Style::default().fg(Color::Rgb(80, 80, 100)),
        ));
    } else {
        header_spans.push(Span::styled("filter: ", Style::default().fg(DIM_COLOR)));
        header_spans.push(Span::styled(
            picker.filter.clone(),
            Style::default().fg(Color::White).bold(),
        ));
        header_spans.push(Span::styled(" ", Style::default()));
    }

    // Count
    let count_str = if filtered_count == total {
        format!("({})", total)
    } else {
        format!("({}/{})", filtered_count, total)
    };
    header_spans.push(Span::styled(
        format!("{}  ", count_str),
        Style::default().fg(DIM_COLOR),
    ));

    // Column headers with focus indicator
    for (i, name) in col_names.iter().enumerate() {
        if i > 0 {
            header_spans.push(Span::styled("  ", Style::default()));
        }
        header_spans.push(Span::styled(
            name.to_string(),
            if i == col {
                col_focus_style
            } else {
                col_dim_style
            },
        ));
    }

    if picker.preview {
        header_spans.push(Span::styled(
            "  press Enter to open",
            Style::default().fg(Color::Rgb(60, 60, 80)).italic(),
        ));
    } else {
        header_spans.push(Span::styled(
            "  ↑↓ ←→ Enter Esc",
            Style::default().fg(Color::Rgb(60, 60, 80)),
        ));
    }

    let mut lines: Vec<Line> = Vec::new();
    lines.push(Line::from(header_spans));

    // Handle empty results
    if picker.filtered.is_empty() {
        lines.push(Line::from(Span::styled(
            "   no matches",
            Style::default().fg(DIM_COLOR).italic(),
        )));
        let paragraph = Paragraph::new(lines);
        frame.render_widget(paragraph, area);
        return;
    }

    // Calculate column widths based on content
    // Model column: flexible, Provider: ~20, Via: ~12
    let via_width = 12usize;
    let provider_width = 20usize;
    let model_width = width.saturating_sub(3 + provider_width + via_width + 4); // 3 for marker, 4 for gaps

    // Vertical list
    let list_height = height.saturating_sub(1);
    if list_height == 0 {
        let paragraph = Paragraph::new(lines);
        frame.render_widget(paragraph, area);
        return;
    }

    // Scroll window
    let half = list_height / 2;
    let start = if selected <= half {
        0
    } else if selected + list_height - half > filtered_count {
        filtered_count.saturating_sub(list_height)
    } else {
        selected - half
    };
    let end = (start + list_height).min(filtered_count);

    for vi in start..end {
        let model_idx = picker.filtered[vi];
        let entry = &picker.models[model_idx];
        let is_row_selected = vi == selected;
        let route = entry.routes.get(entry.selected_route);

        let marker = if is_row_selected { "▸" } else { " " };

        let mut spans: Vec<Span> = Vec::new();
        spans.push(Span::styled(
            format!(" {} ", marker),
            if is_row_selected {
                Style::default().fg(Color::White).bold()
            } else {
                Style::default().fg(DIM_COLOR)
            },
        ));

        let unavailable = route.map(|r| !r.available).unwrap_or(true);

        // Model column
        let rec_suffix = if entry.recommended && !entry.is_current {
            " ★"
        } else {
            ""
        };
        let display_name = format!("{}{}", entry.name, rec_suffix);
        let model_text = if display_name.len() > model_width {
            format!("{:<w$}", &display_name[..model_width], w = model_width)
        } else {
            format!("{:<w$}", display_name, w = model_width)
        };
        let model_style = if unavailable {
            Style::default().fg(Color::Rgb(80, 80, 80))
        } else if is_row_selected && col == 0 {
            Style::default()
                .fg(Color::White)
                .bg(Color::Rgb(60, 60, 80))
                .bold()
        } else if entry.is_current {
            Style::default().fg(ACCENT_COLOR)
        } else if entry.recommended {
            Style::default().fg(Color::Rgb(255, 220, 120))
        } else {
            Style::default().fg(Color::Rgb(200, 200, 220))
        };
        spans.push(Span::styled(model_text, model_style));

        // Provider column
        let route_count = entry.routes.len();
        let provider_text = route.map(|r| r.provider.as_str()).unwrap_or("—");
        // When on model column, show route count hint instead of full provider
        let provider_display = if col == 0 && route_count > 1 {
            let label = format!("{} ({})", provider_text, route_count);
            if label.len() > provider_width {
                format!(" {:<w$}", &label[..provider_width], w = provider_width)
            } else {
                format!(" {:<w$}", label, w = provider_width)
            }
        } else if provider_text.len() > provider_width {
            format!(
                " {:<w$}",
                &provider_text[..provider_width],
                w = provider_width
            )
        } else {
            format!(" {:<w$}", provider_text, w = provider_width)
        };
        let provider_style = if unavailable {
            Style::default().fg(Color::Rgb(80, 80, 80))
        } else if is_row_selected && col == 1 {
            Style::default()
                .fg(Color::White)
                .bg(Color::Rgb(60, 60, 80))
                .bold()
        } else {
            Style::default().fg(Color::Rgb(140, 180, 255))
        };
        spans.push(Span::styled(provider_display, provider_style));

        // Via/API column
        let via_text = route.map(|r| r.api_method.as_str()).unwrap_or("—");
        let via_display = format!(" {:<w$}", via_text, w = via_width);
        let via_style = if unavailable {
            Style::default().fg(Color::Rgb(80, 80, 80))
        } else if is_row_selected && col == 2 {
            Style::default()
                .fg(Color::White)
                .bg(Color::Rgb(60, 60, 80))
                .bold()
        } else {
            Style::default().fg(Color::Rgb(220, 190, 120))
        };
        spans.push(Span::styled(via_display, via_style));

        // Detail (pricing etc) after columns
        if let Some(route) = route {
            if !route.detail.is_empty() {
                spans.push(Span::styled(
                    format!("  {}", route.detail),
                    if unavailable {
                        Style::default().fg(Color::Rgb(80, 80, 80))
                    } else {
                        Style::default().fg(DIM_COLOR)
                    },
                ));
            }
        }

        lines.push(Line::from(spans));
    }

    let paragraph = Paragraph::new(lines);
    frame.render_widget(paragraph, area);
}

fn draw_status(frame: &mut Frame, app: &dyn TuiState, area: Rect, pending_count: usize) {
    let elapsed = app.elapsed().map(|d| d.as_secs_f32()).unwrap_or(0.0);
    let stale_secs = app.time_since_activity().map(|d| d.as_secs_f32());

    // Check for unexpected cache miss (cache write on turn 3+)
    let (cache_read, cache_creation) = app.streaming_cache_tokens();
    let user_turn_count = app
        .display_messages()
        .iter()
        .filter(|m| m.role == "user")
        .count();
    let unexpected_cache_miss =
        is_unexpected_cache_miss(user_turn_count, cache_read, cache_creation);

    // Helper to append queued count indicator
    let queued_suffix = if pending_count > 0 {
        format!(" · +{} queued", pending_count)
    } else {
        String::new()
    };

    let mut line = if let Some(build_progress) = crate::build::read_build_progress() {
        // Show build progress when compiling
        let spinner_idx = (elapsed * 12.5) as usize % SPINNER_FRAMES.len();
        let spinner = SPINNER_FRAMES[spinner_idx];
        Line::from(vec![
            Span::styled(spinner, Style::default().fg(Color::Rgb(255, 193, 7))),
            Span::styled(
                format!(" {}", build_progress),
                Style::default().fg(Color::Rgb(255, 193, 7)),
            ),
        ])
    } else if let Some(remaining) = app.rate_limit_remaining() {
        // Rate limit countdown - show animated spinner and time remaining
        let secs = remaining.as_secs();
        let spinner_idx = (elapsed * 4.0) as usize % SPINNER_FRAMES.len();
        let spinner = SPINNER_FRAMES[spinner_idx];
        // Format time remaining in a human-readable way
        let time_str = if secs >= 3600 {
            let hours = secs / 3600;
            let mins = (secs % 3600) / 60;
            format!("{}h {}m", hours, mins)
        } else if secs >= 60 {
            let mins = secs / 60;
            let s = secs % 60;
            format!("{}m {}s", mins, s)
        } else {
            format!("{}s", secs)
        };
        Line::from(vec![
            Span::styled(spinner, Style::default().fg(Color::Rgb(255, 193, 7))),
            Span::styled(
                format!(
                    " Rate limited. Auto-retry in {}...{}",
                    time_str, queued_suffix
                ),
                Style::default().fg(Color::Rgb(255, 193, 7)),
            ),
        ])
    } else if app.is_processing() {
        // Animated spinner based on elapsed time (cycles every 80ms per frame)
        let spinner_idx = (elapsed * 12.5) as usize % SPINNER_FRAMES.len();
        let spinner = SPINNER_FRAMES[spinner_idx];

        match app.status() {
            ProcessingStatus::Idle => Line::from(""),
            ProcessingStatus::Sending => {
                let mut spans = vec![
                    Span::styled(spinner, Style::default().fg(AI_COLOR)),
                    Span::styled(
                        format!(" sending… {}", format_elapsed(elapsed)),
                        Style::default().fg(DIM_COLOR),
                    ),
                ];
                if !queued_suffix.is_empty() {
                    spans.push(Span::styled(
                        queued_suffix.clone(),
                        Style::default().fg(QUEUED_COLOR),
                    ));
                }
                Line::from(spans)
            }
            ProcessingStatus::Thinking(_start) => {
                let thinking_elapsed = elapsed;
                let mut spans = vec![
                    Span::styled(spinner, Style::default().fg(AI_COLOR)),
                    Span::styled(
                        format!(" thinking… {:.1}s", thinking_elapsed),
                        Style::default().fg(DIM_COLOR),
                    ),
                ];
                if !queued_suffix.is_empty() {
                    spans.push(Span::styled(
                        queued_suffix.clone(),
                        Style::default().fg(QUEUED_COLOR),
                    ));
                }
                Line::from(spans)
            }
            ProcessingStatus::Streaming => {
                // Show stale indicator if no activity for >2s
                let time_str = format_elapsed(elapsed);
                let mut status_text = match stale_secs {
                    Some(s) if s > 2.0 => format!("(idle {:.0}s) · {}", s, time_str),
                    _ => time_str,
                };
                // Add TPS if available
                if let Some(tps) = app.output_tps() {
                    status_text = format!("{} · {:.1} tps", status_text, tps);
                }
                if unexpected_cache_miss {
                    let miss_tokens = cache_creation.unwrap_or(0);
                    let miss_str = if miss_tokens >= 1000 {
                        format!("{}k", miss_tokens / 1000)
                    } else {
                        format!("{}", miss_tokens)
                    };
                    status_text = format!("⚠ {} cache miss · {}", miss_str, status_text);
                }
                let mut spans = vec![
                    Span::styled(spinner, Style::default().fg(AI_COLOR)),
                    Span::styled(
                        format!(" {}", status_text),
                        Style::default().fg(if unexpected_cache_miss {
                            Color::Rgb(255, 193, 7)
                        } else {
                            DIM_COLOR
                        }),
                    ),
                ];
                if !queued_suffix.is_empty() {
                    spans.push(Span::styled(
                        queued_suffix.clone(),
                        Style::default().fg(QUEUED_COLOR),
                    ));
                }
                Line::from(spans)
            }
            ProcessingStatus::RunningTool(ref name) => {
                // Animated progress dots - surrounds tool name only
                let half_width = 3;
                let progress = ((elapsed * 2.0) % 1.0) as f32; // Cycle every 0.5s
                let filled_pos = ((progress * half_width as f32) as usize) % half_width;
                let left_bar: String = (0..half_width)
                    .map(|i| if i == filled_pos { '●' } else { '·' })
                    .collect();
                let right_bar: String = (0..half_width)
                    .map(|i| {
                        if i == (half_width - 1 - filled_pos) {
                            '●'
                        } else {
                            '·'
                        }
                    })
                    .collect();

                let anim_color = animated_tool_color(elapsed);

                // Get tool details (command, file path, etc.)
                let tool_detail = app
                    .streaming_tool_calls()
                    .last()
                    .map(|tc| get_tool_summary(tc))
                    .filter(|s| !s.is_empty());

                // Subagent status (only for task_runner)
                let subagent = app.subagent_status();

                // Build the line: animation · tool · animation · detail · (status) · time · ⚠ cache
                let mut spans = vec![
                    Span::styled(left_bar, Style::default().fg(anim_color)),
                    Span::styled(" ", Style::default()),
                    Span::styled(name.to_string(), Style::default().fg(anim_color).bold()),
                    Span::styled(" ", Style::default()),
                    Span::styled(right_bar, Style::default().fg(anim_color)),
                ];

                if let Some(detail) = tool_detail {
                    spans.push(Span::styled(
                        format!(" · {}", detail),
                        Style::default().fg(DIM_COLOR),
                    ));
                }

                if let Some(status) = subagent {
                    spans.push(Span::styled(
                        format!(" ({})", status),
                        Style::default().fg(DIM_COLOR),
                    ));
                }

                spans.push(Span::styled(
                    format!(" · {}", format_elapsed(elapsed)),
                    Style::default().fg(DIM_COLOR),
                ));

                if unexpected_cache_miss {
                    let miss_tokens = cache_creation.unwrap_or(0);
                    let miss_str = if miss_tokens >= 1000 {
                        format!("{}k", miss_tokens / 1000)
                    } else {
                        format!("{}", miss_tokens)
                    };
                    spans.push(Span::styled(
                        format!(" · ⚠ {} cache miss", miss_str),
                        Style::default().fg(Color::Rgb(255, 193, 7)),
                    ));
                }

                spans.push(Span::styled(
                    " · Alt+B bg",
                    Style::default().fg(Color::Rgb(100, 100, 100)),
                ));

                if !queued_suffix.is_empty() {
                    spans.push(Span::styled(
                        queued_suffix.clone(),
                        Style::default().fg(QUEUED_COLOR),
                    ));
                }

                Line::from(spans)
            }
        }
    } else {
        // Idle - show token warning if high usage, otherwise usage limits
        if let Some((total_in, total_out)) = app.total_session_tokens() {
            let total = total_in + total_out;
            if total > 100_000 {
                // High usage warning (>100k tokens)
                let warning_color = if total > 150_000 {
                    Color::Rgb(255, 100, 100) // Red for very high
                } else {
                    Color::Rgb(255, 193, 7) // Amber for high
                };
                Line::from(vec![
                    Span::styled("⚠ ", Style::default().fg(warning_color)),
                    Span::styled(
                        format!("Session: {}k tokens ", total / 1000),
                        Style::default().fg(warning_color),
                    ),
                    Span::styled(
                        "(consider /clear for fresh context)",
                        Style::default().fg(DIM_COLOR),
                    ),
                ])
            } else {
                // Show usage limits when idle (subscription providers)
                build_idle_usage_line(app)
            }
        } else {
            // Show usage limits when idle (subscription providers)
            build_idle_usage_line(app)
        }
    };

    if let Some(notice) = app.status_notice() {
        if !line.spans.is_empty() {
            line.spans
                .push(Span::styled(" · ", Style::default().fg(DIM_COLOR)));
        }
        line.spans
            .push(Span::styled(notice, Style::default().fg(ACCENT_COLOR)));
    }

    let aligned_line = if app.centered_mode() {
        line.alignment(ratatui::layout::Alignment::Center)
    } else {
        line
    };
    let paragraph = Paragraph::new(aligned_line);
    frame.render_widget(paragraph, area);
}

/// Build usage line for idle state (shows subscription limits or cost)
fn build_idle_usage_line(app: &dyn TuiState) -> Line<'static> {
    use super::info_widget::UsageProvider;

    let widget_data = app.info_widget_data();
    let Some(usage) = &widget_data.usage_info else {
        return Line::from("");
    };
    if !usage.available {
        return Line::from("");
    }

    match usage.provider {
        UsageProvider::CostBased => {
            // Show cost for API-key providers
            let cost_str = format!("${:.4}", usage.total_cost);
            let tokens_str = format!(
                "{}↑ {}↓",
                format_tokens_compact(usage.input_tokens),
                format_tokens_compact(usage.output_tokens)
            );
            Line::from(vec![
                Span::styled("💰 ", Style::default().fg(DIM_COLOR)),
                Span::styled(cost_str, Style::default().fg(Color::Rgb(140, 180, 255))),
                Span::styled(format!(" ({})", tokens_str), Style::default().fg(DIM_COLOR)),
            ])
        }
        _ => {
            // Show subscription usage bars inline
            let five_hr = (usage.five_hour * 100.0).round() as u8;
            let seven_day = (usage.seven_day * 100.0).round() as u8;

            let five_hr_color = usage_color(five_hr);
            let seven_day_color = usage_color(seven_day);

            Line::from(vec![
                Span::styled("5hr:", Style::default().fg(DIM_COLOR)),
                Span::styled(format!("{}%", five_hr), Style::default().fg(five_hr_color)),
                Span::styled(" · 7d:", Style::default().fg(DIM_COLOR)),
                Span::styled(
                    format!("{}%", seven_day),
                    Style::default().fg(seven_day_color),
                ),
            ])
        }
    }
}

/// Color for usage percentage (green < 50, yellow 50-80, red > 80)
fn usage_color(pct: u8) -> Color {
    if pct >= 80 {
        Color::Rgb(255, 100, 100) // Red
    } else if pct >= 50 {
        Color::Rgb(255, 200, 100) // Yellow
    } else {
        Color::Rgb(100, 200, 100) // Green
    }
}

/// Format tokens compactly (1.2M, 45K, 123)
fn format_tokens_compact(tokens: u64) -> String {
    if tokens >= 1_000_000 {
        format!("{:.1}M", tokens as f64 / 1_000_000.0)
    } else if tokens >= 1_000 {
        format!("{:.0}K", tokens as f64 / 1_000.0)
    } else {
        format!("{}", tokens)
    }
}

fn format_usage_line(tokens_str: String, cache_status: Option<String>) -> String {
    let mut parts = Vec::new();
    if !tokens_str.is_empty() {
        parts.push(tokens_str);
    }
    if let Some(cache) = cache_status {
        parts.push(cache);
    }
    if parts.is_empty() {
        String::new()
    } else {
        parts.join(" • ")
    }
}

fn format_cache_status(
    cache_read_tokens: Option<u64>,
    cache_creation_tokens: Option<u64>,
) -> Option<String> {
    match (cache_read_tokens, cache_creation_tokens) {
        (Some(read), _) if read > 0 => {
            // Cache hit - show how many tokens were read from cache
            let k = read / 1000;
            if k > 0 {
                Some(format!("⚡{}k cached", k))
            } else {
                Some(format!("⚡{} cached", read))
            }
        }
        (_, Some(created)) if created > 0 => {
            // Cache write - show how many tokens were cached
            let k = created / 1000;
            if k > 0 {
                Some(format!("💾{}k stored", k))
            } else {
                Some(format!("💾{} stored", created))
            }
        }
        _ => None,
    }
}

fn send_mode_indicator(app: &dyn TuiState) -> (&'static str, Color) {
    if app.queue_mode() {
        ("⏳", QUEUED_COLOR)
    } else {
        ("⚡", ASAP_COLOR)
    }
}

fn send_mode_reserved_width(app: &dyn TuiState) -> usize {
    let (icon, _) = send_mode_indicator(app);
    if icon.is_empty() {
        0
    } else {
        2 // Reserve a small gutter on the right for the icon
    }
}

fn draw_send_mode_indicator(frame: &mut Frame, app: &dyn TuiState, area: Rect) {
    let (icon, color) = send_mode_indicator(app);
    if icon.is_empty() || area.width == 0 || area.height == 0 {
        return;
    }
    let indicator_area = Rect {
        x: area.x,
        y: area.y + area.height.saturating_sub(1),
        width: area.width,
        height: 1,
    };
    let line = Line::from(Span::styled(icon, Style::default().fg(color)));
    let paragraph = Paragraph::new(line).alignment(Alignment::Right);
    frame.render_widget(paragraph, indicator_area);
}

fn pending_prompt_count(app: &dyn TuiState) -> usize {
    let pending_soft_interrupt = app.is_processing()
        && app
            .pending_soft_interrupt()
            .map(|msg| !msg.is_empty())
            .unwrap_or(false);
    let interleave = app.is_processing()
        && app
            .interleave_message()
            .map(|msg| !msg.is_empty())
            .unwrap_or(false);
    app.queued_messages().len()
        + if pending_soft_interrupt { 1 } else { 0 }
        + if interleave { 1 } else { 0 }
}

fn pending_queue_preview(app: &dyn TuiState) -> Vec<String> {
    let mut previews = Vec::new();
    if app.is_processing() {
        // Show pending soft interrupt (sent to server, awaiting injection)
        if let Some(msg) = app.pending_soft_interrupt() {
            if !msg.is_empty() {
                previews.push(format!("↻ {}", msg.chars().take(100).collect::<String>()));
            }
        }
        // Show interleave message (in buffer, ready to send)
        if let Some(msg) = app.interleave_message() {
            if !msg.is_empty() {
                previews.push(format!("⚡ {}", msg.chars().take(100).collect::<String>()));
            }
        }
    }
    for msg in app.queued_messages() {
        previews.push(format!("⏳ {}", msg.chars().take(100).collect::<String>()));
    }
    previews
}

/// Types of queued/pending messages
#[derive(Clone, Copy)]
enum QueuedMsgType {
    Pending,    // Sent to server, awaiting injection (↻)
    Interleave, // In buffer, ready to send immediately (⚡)
    Queued,     // Waiting for processing to finish (⏳)
}

fn draw_queued(frame: &mut Frame, app: &dyn TuiState, area: Rect, start_num: usize) {
    let mut items: Vec<(QueuedMsgType, &str)> = Vec::new();
    if app.is_processing() {
        // Pending soft interrupt (sent to server, awaiting injection)
        if let Some(msg) = app.pending_soft_interrupt() {
            if !msg.is_empty() {
                items.push((QueuedMsgType::Pending, msg));
            }
        }
        // Interleave message (in buffer, ready to send)
        if let Some(msg) = app.interleave_message() {
            if !msg.is_empty() {
                items.push((QueuedMsgType::Interleave, msg));
            }
        }
    }
    // Queued messages (waiting for processing to finish)
    for msg in app.queued_messages() {
        items.push((QueuedMsgType::Queued, msg.as_str()));
    }

    let pending_count = items.len();
    let lines: Vec<Line> = items
        .iter()
        .take(3)
        .enumerate()
        .map(|(i, (msg_type, msg))| {
            // Distance from input prompt: pending_count - i (first pending is furthest from input)
            // +1 because the input prompt itself is distance 0
            let distance = pending_count.saturating_sub(i);
            let num_color = rainbow_prompt_color(distance);
            let (indicator, indicator_color, msg_color, dim) = match msg_type {
                QueuedMsgType::Pending => ("↻", PENDING_COLOR, PENDING_COLOR, false),
                QueuedMsgType::Interleave => ("⚡", ASAP_COLOR, ASAP_COLOR, false),
                QueuedMsgType::Queued => ("⏳", QUEUED_COLOR, QUEUED_COLOR, true),
            };
            let mut msg_style = Style::default().fg(msg_color);
            if dim {
                msg_style = msg_style.dim();
            }
            Line::from(vec![
                Span::styled(format!("{}", start_num + i), Style::default().fg(num_color)),
                Span::raw(" "),
                Span::styled(indicator, Style::default().fg(indicator_color)),
                Span::raw(" "),
                Span::styled(*msg, msg_style),
            ])
        })
        .collect();

    let paragraph = if app.centered_mode() {
        Paragraph::new(
            lines
                .iter()
                .map(|line| line.clone().alignment(Alignment::Center))
                .collect::<Vec<_>>(),
        )
    } else {
        Paragraph::new(lines)
    };
    frame.render_widget(paragraph, area);
}

fn draw_idle_animation(frame: &mut Frame, app: &dyn TuiState, area: Rect) {
    if area.width < 4 || area.height < 2 {
        return;
    }

    let elapsed = app.animation_elapsed();
    let cw = area.width as usize;
    let ch = area.height as usize;

    const SUB_X: usize = 3;
    const SUB_Y: usize = 3;
    let sw = cw * SUB_X;
    let sh = ch * SUB_Y;

    let mut hit = vec![false; sw * sh];
    let mut lum_map = vec![0.0f32; sw * sh];
    let mut z_buf = vec![0.0f32; sw * sh];

    let variant = idle_animation_variant();
    match variant {
        0 => sample_donut(elapsed, sw, sh, &mut hit, &mut lum_map, &mut z_buf),
        1 => sample_knot(elapsed, sw, sh, &mut hit, &mut lum_map, &mut z_buf),
        _ => sample_dna(elapsed, sw, sh, &mut hit, &mut lum_map, &mut z_buf),
    }

    let time_hue = elapsed * 40.0;
    let centered = app.centered_mode();
    let align = if centered {
        ratatui::layout::Alignment::Center
    } else {
        ratatui::layout::Alignment::Left
    };

    let lines: Vec<Line<'static>> = (0..ch)
        .map(|row| {
            let spans: Vec<Span<'static>> = (0..cw)
                .map(|col| {
                    let mut pattern = 0u16;
                    let mut total_lum = 0.0f32;
                    let mut hit_count = 0u32;

                    for sy in 0..SUB_Y {
                        for sx in 0..SUB_X {
                            let px = col * SUB_X + sx;
                            let py = row * SUB_Y + sy;
                            let idx = py * sw + px;
                            if hit[idx] {
                                pattern |= 1 << (sy * SUB_X + sx);
                                total_lum += lum_map[idx];
                                hit_count += 1;
                            }
                        }
                    }

                    if hit_count == 0 {
                        Span::raw(" ")
                    } else {
                        let avg_lum = total_lum / hit_count as f32;
                        let coverage = hit_count as f32 / (SUB_X * SUB_Y) as f32;
                        let t = (avg_lum + 1.0) * 0.5;
                        let ch = shape_char_3x3(pattern, t);

                        let hue = (time_hue + t * 160.0) % 360.0;
                        let hue = if hue < 0.0 { hue + 360.0 } else { hue };

                        let sat = 0.5 + t * 0.4;
                        let val = (0.10 + t * t * 0.90) * (0.55 + coverage * 0.45);
                        let (r, g, b) = hsv_to_rgb(hue, sat, val);
                        Span::styled(
                            String::from(ch),
                            Style::default().fg(Color::Rgb(r, g, b)),
                        )
                    }
                })
                .collect();
            Line::from(spans).alignment(align)
        })
        .collect();

    frame.render_widget(Paragraph::new(lines), area);
}

fn sample_donut(
    elapsed: f32,
    sw: usize,
    sh: usize,
    hit: &mut [bool],
    lum_map: &mut [f32],
    z_buf: &mut [f32],
) {
    let a_rot = elapsed * 1.0;
    let b_rot = elapsed * 0.5;
    let cos_a = a_rot.cos();
    let sin_a = a_rot.sin();
    let cos_b = b_rot.cos();
    let sin_b = b_rot.sin();

    let aspect = 0.5;
    let r1 = 1.0f32;
    let r2 = 2.0f32;
    let k2 = 5.0f32;
    let k1 = (sw as f32).min(sh as f32 / aspect) * k2 * 0.35 / (r1 + r2);

    let mut theta: f32 = 0.0;
    while theta < std::f32::consts::TAU {
        let ct = theta.cos();
        let st = theta.sin();

        let mut phi: f32 = 0.0;
        while phi < std::f32::consts::TAU {
            let cp = phi.cos();
            let sp = phi.sin();

            let cx = r2 + r1 * ct;
            let cy = r1 * st;

            let x = cx * (cos_b * cp + sin_a * sin_b * sp) - cy * cos_a * sin_b;
            let y = cx * (sin_b * cp - sin_a * cos_b * sp) + cy * cos_a * cos_b;
            let z = k2 + cos_a * cx * sp + cy * sin_a;
            let ooz = 1.0 / z;

            let xp = (sw as f32 / 2.0 + k1 * ooz * x) as isize;
            let yp = (sh as f32 / 2.0 - k1 * ooz * y * aspect) as isize;

            let lum = cp * ct * sin_b - cos_a * ct * sp - sin_a * st
                + cos_b * (cos_a * st - ct * sin_a * sp);

            if xp >= 0 && (xp as usize) < sw && yp >= 0 && (yp as usize) < sh {
                let idx = yp as usize * sw + xp as usize;
                if ooz > z_buf[idx] {
                    z_buf[idx] = ooz;
                    lum_map[idx] = lum;
                    hit[idx] = true;
                }
            }

            phi += 0.007;
        }
        theta += 0.02;
    }
}

fn idle_animation_variant() -> usize {
    use std::sync::OnceLock;
    static VARIANT: OnceLock<usize> = OnceLock::new();
    *VARIANT.get_or_init(|| {
        let mut hasher = DefaultHasher::new();
        std::time::SystemTime::now().hash(&mut hasher);
        std::process::id().hash(&mut hasher);
        (std::hash::Hasher::finish(&hasher) % 3) as usize
    })
}

fn sample_dna(
    elapsed: f32,
    sw: usize,
    sh: usize,
    hit: &mut [bool],
    lum_map: &mut [f32],
    z_buf: &mut [f32],
) {
    let rot_y = elapsed * 0.4;
    let rot_x = elapsed * 0.15;
    let cam_dist = 8.0f32;
    let aspect = 0.5;
    let scale_base = (sw as f32).min(sh as f32 / aspect) * 0.22;
    let tube_r = 0.2f32;
    let helix_r = 1.0f32;
    let twist = 2.5f32;
    let stretch = 3.5f32;

    for strand in 0..2 {
        let phase = strand as f32 * std::f32::consts::PI;
        let mut t: f32 = -std::f32::consts::PI * twist;
        let t_end = std::f32::consts::PI * twist;
        while t < t_end {
            let angle = t + phase;
            let hx = helix_r * angle.cos();
            let hy = (t / (twist * std::f32::consts::PI)) * stretch;
            let hz = helix_r * angle.sin();

            let dt = 0.01f32;
            let t2 = t + dt;
            let angle2 = t2 + phase;
            let dx = helix_r * angle2.cos() - hx;
            let dy = (t2 / (twist * std::f32::consts::PI)) * stretch - hy;
            let dz = helix_r * angle2.sin() - hz;
            let dl = (dx * dx + dy * dy + dz * dz).sqrt().max(0.001);
            let ttx = dx / dl;
            let tty = dy / dl;
            let ttz = dz / dl;

            let (bx, by, bz) = {
                let up = if ttx.abs() < 0.9 { (1.0f32, 0.0, 0.0) } else { (0.0, 1.0, 0.0) };
                let bx = tty * up.2 - ttz * up.1;
                let by = ttz * up.0 - ttx * up.2;
                let bz = ttx * up.1 - tty * up.0;
                let bl = (bx * bx + by * by + bz * bz).sqrt().max(0.001);
                (bx / bl, by / bl, bz / bl)
            };
            let nx = by * ttz - bz * tty;
            let ny = bz * ttx - bx * ttz;
            let nz = bx * tty - by * ttx;

            let mut phi: f32 = 0.0;
            while phi < std::f32::consts::TAU {
                let cp = phi.cos();
                let sp = phi.sin();
                let px = hx + tube_r * (cp * nx + sp * bx);
                let py = hy + tube_r * (cp * ny + sp * by);
                let pz = hz + tube_r * (cp * nz + sp * bz);

                let sn_x = cp * nx + sp * bx;
                let sn_y = cp * ny + sp * by;
                let sn_z = cp * nz + sp * bz;

                let (rx, ry, rz) = rotate_xyz(px, py, pz, rot_x, rot_y, 0.0);
                let d = cam_dist + rz;
                if d < 0.1 {
                    phi += 0.12;
                    continue;
                }
                let proj = cam_dist / d;
                let xp = (sw as f32 / 2.0 + rx * proj * scale_base) as isize;
                let yp = (sh as f32 / 2.0 - ry * proj * scale_base * aspect) as isize;
                let depth = 1.0 / d;

                if xp >= 0 && (xp as usize) < sw && yp >= 0 && (yp as usize) < sh {
                    let idx = yp as usize * sw + xp as usize;
                    if depth > z_buf[idx] {
                        z_buf[idx] = depth;
                        let (rnx, rny, _) = rotate_xyz(sn_x, sn_y, sn_z, rot_x, rot_y, 0.0);
                        let lum = (rnx * 0.4 + rny * 0.5 + 0.3).clamp(-1.0, 1.0);
                        lum_map[idx] = lum;
                        hit[idx] = true;
                    }
                }
                phi += 0.12;
            }
            t += 0.012;
        }
    }

    // Rungs connecting the two strands
    let rung_step = std::f32::consts::PI * 0.4;
    let mut t: f32 = -std::f32::consts::PI * twist + rung_step * 0.5;
    let t_end = std::f32::consts::PI * twist;
    let rung_r = 0.1f32;
    while t < t_end {
        let a1 = t;
        let a2 = t + std::f32::consts::PI;
        let y_pos = (t / (twist * std::f32::consts::PI)) * stretch;

        let p1x = helix_r * a1.cos();
        let p1z = helix_r * a1.sin();
        let p2x = helix_r * a2.cos();
        let p2z = helix_r * a2.sin();

        let steps = 20;
        for i in 0..=steps {
            let frac = i as f32 / steps as f32;
            let rx_pos = p1x + (p2x - p1x) * frac;
            let rz_pos = p1z + (p2z - p1z) * frac;

            let mut phi: f32 = 0.0;
            while phi < std::f32::consts::TAU {
                let cp = phi.cos();
                let sp = phi.sin();
                let px = rx_pos + rung_r * cp;
                let py = y_pos + rung_r * sp;
                let pz = rz_pos;

                let (rx, ry, rz) = rotate_xyz(px, py, pz, rot_x, rot_y, 0.0);
                let d = cam_dist + rz;
                if d < 0.1 {
                    phi += 0.3;
                    continue;
                }
                let proj = cam_dist / d;
                let xp = (sw as f32 / 2.0 + rx * proj * scale_base) as isize;
                let yp = (sh as f32 / 2.0 - ry * proj * scale_base * aspect) as isize;
                let depth = 1.0 / d;

                if xp >= 0 && (xp as usize) < sw && yp >= 0 && (yp as usize) < sh {
                    let idx = yp as usize * sw + xp as usize;
                    if depth > z_buf[idx] {
                        z_buf[idx] = depth;
                        lum_map[idx] = 0.2;
                        hit[idx] = true;
                    }
                }
                phi += 0.3;
            }
        }
        t += rung_step;
    }
}

fn sample_knot(
    elapsed: f32,
    sw: usize,
    sh: usize,
    hit: &mut [bool],
    lum_map: &mut [f32],
    z_buf: &mut [f32],
) {
    let rot_y = elapsed * 0.4;
    let rot_x = elapsed * 0.2;
    let cam_dist = 6.0f32;
    let aspect = 0.5;
    let scale_base = (sw as f32).min(sh as f32 / aspect) * 0.28;
    let tube_r = 0.35f32;

    let mut t: f32 = 0.0;
    while t < std::f32::consts::TAU {
        let kx = (2.0 + (2.0 * t).cos()) * t.cos();
        let ky = (2.0 + (2.0 * t).cos()) * t.sin();
        let kz = (2.0 * t).sin();

        let dt = 0.01f32;
        let t2 = t + dt;
        let dx = (2.0 + (2.0 * t2).cos()) * t2.cos() - kx;
        let dy = (2.0 + (2.0 * t2).cos()) * t2.sin() - ky;
        let dz = (2.0 * t2).sin() - kz;
        let dl = (dx * dx + dy * dy + dz * dz).sqrt().max(0.001);
        let tx = dx / dl;
        let ty = dy / dl;
        let tz = dz / dl;

        let up_x = 0.0f32;
        let up_y = 0.0f32;
        let up_z = 1.0f32;
        let bx = ty * up_z - tz * up_y;
        let by = tz * up_x - tx * up_z;
        let bz = tx * up_y - ty * up_x;
        let bl = (bx * bx + by * by + bz * bz).sqrt().max(0.001);
        let bx = bx / bl;
        let by = by / bl;
        let bz = bz / bl;
        let nx = by * tz - bz * ty;
        let ny = bz * tx - bx * tz;
        let nz = bx * ty - by * tx;

        let mut phi: f32 = 0.0;
        while phi < std::f32::consts::TAU {
            let cp = phi.cos();
            let sp = phi.sin();
            let px = kx + tube_r * (cp * nx + sp * bx);
            let py = ky + tube_r * (cp * ny + sp * by);
            let pz = kz + tube_r * (cp * nz + sp * bz);

            let sn_x = cp * nx + sp * bx;
            let sn_y = cp * ny + sp * by;
            let sn_z = cp * nz + sp * bz;

            let (rx, ry, rz) = rotate_xyz(px, py, pz, rot_x, rot_y, 0.0);
            let d = cam_dist + rz;
            if d < 0.1 {
                phi += 0.08;
                continue;
            }
            let proj = cam_dist / d;
            let xp = (sw as f32 / 2.0 + rx * proj * scale_base) as isize;
            let yp = (sh as f32 / 2.0 - ry * proj * scale_base * aspect) as isize;
            let depth = 1.0 / d;

            if xp >= 0 && (xp as usize) < sw && yp >= 0 && (yp as usize) < sh {
                let idx = yp as usize * sw + xp as usize;
                if depth > z_buf[idx] {
                    z_buf[idx] = depth;
                    let (rnx, rny, _) = rotate_xyz(sn_x, sn_y, sn_z, rot_x, rot_y, 0.0);
                    let lum = (rnx * 0.4 + rny * 0.5 + 0.3).clamp(-1.0, 1.0);
                    lum_map[idx] = lum;
                    hit[idx] = true;
                }
            }
            phi += 0.08;
        }
        t += 0.008;
    }
}

fn shape_char_3x3(pattern: u16, brightness: f32) -> char {
    if pattern == 0 {
        return ' ';
    }

    // 3x3 grid bit layout:
    // 0 1 2   (top row)
    // 3 4 5   (middle row)
    // 6 7 8   (bottom row)
    let top_l = pattern & 1 != 0;
    let top_c = pattern & 2 != 0;
    let top_r = pattern & 4 != 0;
    let mid_l = pattern & 8 != 0;
    let mid_c = pattern & 16 != 0;
    let mid_r = pattern & 32 != 0;
    let bot_l = pattern & 64 != 0;
    let bot_c = pattern & 128 != 0;
    let bot_r = pattern & 256 != 0;

    let count = pattern.count_ones();
    let top = (top_l as u8) + (top_c as u8) + (top_r as u8);
    let mid = (mid_l as u8) + (mid_c as u8) + (mid_r as u8);
    let bot = (bot_l as u8) + (bot_c as u8) + (bot_r as u8);
    let left = (top_l as u8) + (mid_l as u8) + (bot_l as u8);
    let center = (top_c as u8) + (mid_c as u8) + (bot_c as u8);
    let right = (top_r as u8) + (mid_r as u8) + (bot_r as u8);

    // brightness 0.0=dark .. 1.0=bright, quantize to 3 levels
    let bl = if brightness > 0.65 { 2u8 } else if brightness > 0.35 { 1u8 } else { 0u8 };

    // Full/near-full coverage — density ramp
    if count >= 8 {
        return match bl { 2 => '@', 1 => '#', _ => '%' };
    }
    if count >= 7 {
        return match bl { 2 => '#', 1 => '%', _ => '*' };
    }

    // Diagonal: top-left to bottom-right
    if top_l && mid_c && bot_r && !top_r && !bot_l {
        return match bl { 2 => '\\', 1 => '\\', _ => '.' };
    }
    // Diagonal: top-right to bottom-left
    if top_r && mid_c && bot_l && !top_l && !bot_r {
        return match bl { 2 => '/', 1 => '/', _ => '.' };
    }

    // Horizontal line (middle row dominant)
    if mid >= 2 && top <= 1 && bot <= 1 && mid > top && mid > bot {
        return match bl { 2 => '=', 1 => '-', _ => '~' };
    }
    // Top edge
    if top >= 2 && mid <= 1 && bot == 0 {
        return match bl { 2 => '=', 1 => '-', _ => '~' };
    }
    // Bottom edge
    if bot >= 2 && mid <= 1 && top == 0 {
        return match bl { 2 => '=', 1 => '_', _ => '.' };
    }

    // Vertical line (center column dominant)
    if center >= 2 && left <= 1 && right <= 1 && center > left && center > right {
        return match bl { 2 => '|', 1 => '|', _ => ':' };
    }
    // Left edge
    if left >= 2 && center <= 1 && right == 0 {
        return match bl { 2 => '|', 1 => '|', _ => ':' };
    }
    // Right edge
    if right >= 2 && center <= 1 && left == 0 {
        return match bl { 2 => '|', 1 => '|', _ => ':' };
    }

    // Top-heavy shapes
    if top >= 2 && bot == 0 {
        return match bl { 2 => '"', 1 => '^', _ => '\'' };
    }
    // Bottom-heavy
    if bot >= 2 && top == 0 {
        return match bl { 2 => ',', 1 => '.', _ => '.' };
    }

    // Left-heavy (curved)
    if left >= 2 && right == 0 {
        return match bl { 2 => '(', 1 => '(', _ => ':' };
    }
    // Right-heavy (curved)
    if right >= 2 && left == 0 {
        return match bl { 2 => ')', 1 => ')', _ => ':' };
    }

    // Mostly full
    if count >= 6 {
        return match bl { 2 => '%', 1 => '*', _ => '+' };
    }
    if count >= 5 {
        return match bl { 2 => '*', 1 => '+', _ => ':' };
    }

    // Center blob
    if mid_c && count <= 3 {
        return match bl { 2 => 'o', 1 => '*', _ => '.' };
    }

    // Sparse diagonal hints
    if top_r && bot_l && count <= 3 {
        return match bl { 2 => '/', 1 => '/', _ => '.' };
    }
    if top_l && bot_r && count <= 3 {
        return match bl { 2 => '\\', 1 => '\\', _ => '.' };
    }

    // Corner dots
    if count == 1 {
        if bot_c || bot_l || bot_r { return match bl { 2 => '.', _ => '.' }; }
        if top_c || top_l || top_r { return match bl { 2 => '\'', 1 => '\'', _ => '.' }; }
        return match bl { 2 => ':', 1 => '.', _ => '.' };
    }

    if count <= 3 {
        return match bl { 2 => ':', 1 => ':', _ => '.' };
    }

    match bl { 2 => '+', 1 => ':', _ => '.' }
}

fn hsv_to_rgb(h: f32, s: f32, v: f32) -> (u8, u8, u8) {
    let c = v * s;
    let h2 = h / 60.0;
    let x = c * (1.0 - (h2 % 2.0 - 1.0).abs());
    let (r1, g1, b1) = match h2 as u32 {
        0 => (c, x, 0.0),
        1 => (x, c, 0.0),
        2 => (0.0, c, x),
        3 => (0.0, x, c),
        4 => (x, 0.0, c),
        _ => (c, 0.0, x),
    };
    let m = v - c;
    (
        ((r1 + m) * 255.0) as u8,
        ((g1 + m) * 255.0) as u8,
        ((b1 + m) * 255.0) as u8,
    )
}

fn draw_input(
    frame: &mut Frame,
    app: &dyn TuiState,
    area: Rect,
    next_prompt: usize,
    debug_capture: &mut Option<FrameCaptureBuilder>,
) {
    let input_text = app.input();
    let cursor_pos = app.cursor_pos();

    // Check for command suggestions
    let suggestions = app.command_suggestions();
    let has_suggestions = !suggestions.is_empty() && !app.is_processing();

    // Build prompt parts: number (dim) + caret (colored) + space
    let (prompt_char, caret_color) = if app.is_processing() {
        ("… ", QUEUED_COLOR)
    } else if app.active_skill().is_some() {
        ("» ", ACCENT_COLOR)
    } else {
        ("> ", USER_COLOR)
    };
    let num_str = format!("{}", next_prompt);
    // Use char count, not byte count (ellipsis is 3 bytes but 1 char)
    let prompt_len = num_str.chars().count() + prompt_char.chars().count();
    let reserved_width = send_mode_reserved_width(app);

    let line_width = (area.width as usize).saturating_sub(prompt_len + reserved_width);

    if line_width == 0 {
        return;
    }

    // Build all wrapped lines with cursor tracking
    let (all_lines, cursor_line, cursor_col) = wrap_input_text(
        input_text,
        cursor_pos,
        line_width,
        &num_str,
        prompt_char,
        caret_color,
        prompt_len,
    );

    // Show command suggestions if available (prepended to lines)
    let mut lines: Vec<Line> = Vec::new();
    let mut hint_shown = false;
    let mut hint_line: Option<String> = None;
    if has_suggestions {
        // Limit suggestions and add Tab hint
        let max_suggestions = 5;
        let limited: Vec<_> = suggestions.iter().take(max_suggestions).collect();
        let more_count = suggestions.len().saturating_sub(max_suggestions);

        let mut spans = vec![Span::styled("  Tab: ", Style::default().fg(DIM_COLOR))];
        for (i, (cmd, _desc)) in limited.iter().enumerate() {
            if i > 0 {
                spans.push(Span::styled(" │ ", Style::default().fg(DIM_COLOR)));
            }
            spans.push(Span::styled(
                cmd.to_string(),
                Style::default().fg(Color::Rgb(138, 180, 248)), // USER_COLOR - soft blue
            ));
        }
        if more_count > 0 {
            spans.push(Span::styled(
                format!(" (+{})", more_count),
                Style::default().fg(DIM_COLOR),
            ));
        }
        lines.push(Line::from(spans));
    } else if app.is_processing() && !input_text.is_empty() {
        // Show hint for Shift+Enter when processing and user has typed something
        hint_shown = true;
        let hint = if app.queue_mode() {
            "  Shift+Enter to send now"
        } else {
            "  Shift+Enter to queue"
        };
        hint_line = Some(hint.trim().to_string());
        lines.push(Line::from(Span::styled(
            hint,
            Style::default().fg(DIM_COLOR),
        )));
    }

    // Visual debug: check for shift-enter hint anomalies
    if let Some(ref mut capture) = debug_capture {
        capture.rendered_text.input_area = input_text.to_string();
        if let Some(hint) = &hint_line {
            capture.rendered_text.input_hint = Some(hint.clone());
        }
        visual_debug::check_shift_enter_anomaly(
            capture,
            app.is_processing(),
            input_text,
            hint_shown,
        );
    }

    let suggestions_offset = lines.len();
    let total_input_lines = all_lines.len();
    let visible_height = area.height as usize;

    // Calculate scroll offset to keep cursor visible
    // The cursor_line is relative to input lines (0-indexed)
    let scroll_offset = if total_input_lines + suggestions_offset <= visible_height {
        // Everything fits, no scrolling needed
        0
    } else {
        // Need to scroll - ensure cursor line is visible
        let available_for_input = visible_height.saturating_sub(suggestions_offset);
        if cursor_line < available_for_input {
            0
        } else {
            // Scroll so cursor is near the bottom of visible area
            cursor_line.saturating_sub(available_for_input.saturating_sub(1))
        }
    };

    // Add visible input lines (after scroll offset)
    for line in all_lines.into_iter().skip(scroll_offset) {
        lines.push(line);
        if lines.len() >= visible_height {
            break;
        }
    }

    let centered = app.centered_mode();
    let paragraph = if centered {
        Paragraph::new(
            lines
                .iter()
                .map(|l| l.clone().alignment(ratatui::layout::Alignment::Center))
                .collect::<Vec<_>>(),
        )
    } else {
        Paragraph::new(lines.clone())
    };
    frame.render_widget(paragraph, area);

    // Calculate cursor screen position
    let cursor_screen_line = cursor_line.saturating_sub(scroll_offset) + suggestions_offset;
    let cursor_y = area.y + (cursor_screen_line as u16).min(area.height.saturating_sub(1));

    // For centered mode, calculate the offset to center the line
    let cursor_x = if centered {
        // Get the actual line width from the rendered line (not the full input)
        let actual_line_width = lines
            .get(cursor_screen_line)
            .map(|l| l.width())
            .unwrap_or(prompt_len);
        // Center offset = (area_width - line_width) / 2
        let center_offset = (area.width as usize).saturating_sub(actual_line_width) / 2;
        // For continuation lines, cursor_col is already relative to content start
        // For first line, we need to account for prompt
        let cursor_offset = if cursor_line == 0 {
            prompt_len + cursor_col
        } else {
            // Continuation lines have indent padding, cursor_col is relative to content
            let indent_len = prompt_len; // Same indent as prompt length
            indent_len + cursor_col
        };
        area.x + center_offset as u16 + cursor_offset as u16
    } else {
        area.x + prompt_len as u16 + cursor_col as u16
    };

    frame.set_cursor_position(Position::new(cursor_x, cursor_y));

    draw_send_mode_indicator(frame, app, area);
}

/// Wrap input text into lines, handling explicit newlines and tracking cursor position.
/// Returns (lines, cursor_line, cursor_col) where cursor_line/col are in wrapped coordinates.
fn wrap_input_text<'a>(
    input: &str,
    cursor_pos: usize,
    line_width: usize,
    num_str: &str,
    prompt_char: &'a str,
    caret_color: Color,
    prompt_len: usize,
) -> (Vec<Line<'a>>, usize, usize) {
    let mut lines: Vec<Line> = Vec::new();
    let mut cursor_line = 0;
    let mut cursor_col = 0;
    let mut char_count = 0;
    let mut found_cursor = false;

    let chars: Vec<char> = input.chars().collect();

    // Handle empty input
    if chars.is_empty() {
        let num_color = rainbow_prompt_color(0);
        lines.push(Line::from(vec![
            Span::styled(num_str.to_string(), Style::default().fg(num_color)),
            Span::styled(prompt_char.to_string(), Style::default().fg(caret_color)),
        ]));
        return (lines, 0, 0);
    }

    // Split by newlines first, then wrap each segment
    let mut pos = 0;
    while pos <= chars.len() {
        // Find next newline or end
        let newline_pos = chars[pos..].iter().position(|&c| c == '\n');
        let segment_end = match newline_pos {
            Some(rel_pos) => pos + rel_pos,
            None => chars.len(),
        };

        let segment: Vec<char> = chars[pos..segment_end].to_vec();

        // Wrap this segment
        let mut seg_pos = 0;
        loop {
            let end = (seg_pos + line_width).min(segment.len());
            let line_text: String = segment[seg_pos..end].iter().collect();

            // Track cursor position
            let line_start_char = char_count;
            let line_end_char = char_count + (end - seg_pos);

            if !found_cursor && cursor_pos >= line_start_char && cursor_pos <= line_end_char {
                cursor_line = lines.len();
                cursor_col = cursor_pos - line_start_char;
                found_cursor = true;
            }
            char_count = line_end_char;

            if lines.is_empty() {
                // First line has prompt
                let num_color = rainbow_prompt_color(0);
                lines.push(Line::from(vec![
                    Span::styled(num_str.to_string(), Style::default().fg(num_color)),
                    Span::styled(prompt_char.to_string(), Style::default().fg(caret_color)),
                    Span::raw(line_text),
                ]));
            } else {
                // Continuation lines
                lines.push(Line::from(vec![
                    Span::raw(" ".repeat(prompt_len)),
                    Span::raw(line_text),
                ]));
            }

            if end >= segment.len() {
                break;
            }
            seg_pos = end;
        }

        // Account for the newline character itself in cursor tracking
        if newline_pos.is_some() {
            if !found_cursor && cursor_pos == char_count {
                cursor_line = lines.len().saturating_sub(1);
                cursor_col = lines
                    .last()
                    .map(|l| {
                        l.spans
                            .iter()
                            .skip(1)
                            .map(|s| s.content.chars().count())
                            .sum::<usize>()
                    })
                    .unwrap_or(0);
                found_cursor = true;
            }
            char_count += 1; // newline char
            pos = segment_end + 1;
        } else {
            break;
        }
    }

    // Handle cursor at very end
    if !found_cursor {
        cursor_line = lines.len().saturating_sub(1);
        cursor_col = lines
            .last()
            .map(|l| {
                // Skip the prompt spans and count content
                l.spans
                    .iter()
                    .skip(if cursor_line == 0 { 2 } else { 1 })
                    .map(|s| s.content.chars().count())
                    .sum::<usize>()
            })
            .unwrap_or(0);
    }

    (lines, cursor_line, cursor_col)
}

// Colors for diff display (classic green/red)
const DIFF_ADD_COLOR: Color = Color::Rgb(100, 200, 100); // Green for additions
const DIFF_DEL_COLOR: Color = Color::Rgb(200, 100, 100); // Red for deletions
const DIFF_HIGHLIGHT_ADD: Color = Color::Rgb(150, 255, 150); // Brighter green highlight
const DIFF_HIGHLIGHT_DEL: Color = Color::Rgb(255, 130, 130); // Brighter red highlight

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
enum DiffLineKind {
    Add,
    Del,
}

#[derive(Clone, Debug)]
struct ParsedDiffLine {
    kind: DiffLineKind,
    prefix: String,
    content: String,
}

fn diff_change_counts(content: &str) -> (usize, usize) {
    let lines = collect_diff_lines(content);
    let additions = lines
        .iter()
        .filter(|line| line.kind == DiffLineKind::Add)
        .count();
    let deletions = lines
        .iter()
        .filter(|line| line.kind == DiffLineKind::Del)
        .count();
    (additions, deletions)
}

fn diff_change_counts_for_tool(tool: &ToolCall, content: &str) -> (usize, usize) {
    let (additions, deletions) = diff_change_counts(content);
    if additions > 0 || deletions > 0 {
        return (additions, deletions);
    }

    match tool.name.as_str() {
        "edit" | "Edit" => {
            diff_counts_from_input_pair(&tool.input, "old_string", "new_string").unwrap_or((0, 0))
        }
        "multiedit" => diff_counts_from_multiedit(&tool.input).unwrap_or((0, 0)),
        _ => (additions, deletions),
    }
}

fn diff_counts_from_input_pair(
    input: &serde_json::Value,
    old_key: &str,
    new_key: &str,
) -> Option<(usize, usize)> {
    let old = input.get(old_key)?.as_str()?;
    let new = input.get(new_key)?.as_str()?;
    Some(diff_counts_from_strings(old, new))
}

fn diff_counts_from_multiedit(input: &serde_json::Value) -> Option<(usize, usize)> {
    let edits = input.get("edits")?.as_array()?;
    let mut additions = 0usize;
    let mut deletions = 0usize;

    for edit in edits {
        let old = edit
            .get("old_string")
            .and_then(|v| v.as_str())
            .unwrap_or("");
        let new = edit
            .get("new_string")
            .and_then(|v| v.as_str())
            .unwrap_or("");
        if old.is_empty() && new.is_empty() {
            continue;
        }
        let (add, del) = diff_counts_from_strings(old, new);
        additions += add;
        deletions += del;
    }

    Some((additions, deletions))
}

fn diff_counts_from_strings(old: &str, new: &str) -> (usize, usize) {
    use similar::ChangeTag;
    let diff = similar::TextDiff::from_lines(old, new);
    let mut additions = 0usize;
    let mut deletions = 0usize;
    for change in diff.iter_all_changes() {
        match change.tag() {
            ChangeTag::Insert => additions += 1,
            ChangeTag::Delete => deletions += 1,
            ChangeTag::Equal => {}
        }
    }
    (additions, deletions)
}

/// Generate diff lines from tool input (old_string/new_string) when content doesn't have them.
/// This is needed when the SDK executes tools and returns results in a different format.
fn generate_diff_lines_from_tool_input(tool: &ToolCall) -> Vec<ParsedDiffLine> {
    match tool.name.as_str() {
        "edit" | "Edit" => {
            let old = tool
                .input
                .get("old_string")
                .and_then(|v| v.as_str())
                .unwrap_or("");
            let new = tool
                .input
                .get("new_string")
                .and_then(|v| v.as_str())
                .unwrap_or("");
            generate_diff_lines_from_strings(old, new)
        }
        "multiedit" => {
            let Some(edits) = tool.input.get("edits").and_then(|v| v.as_array()) else {
                return Vec::new();
            };
            let mut all_lines = Vec::new();
            for edit in edits {
                let old = edit
                    .get("old_string")
                    .and_then(|v| v.as_str())
                    .unwrap_or("");
                let new = edit
                    .get("new_string")
                    .and_then(|v| v.as_str())
                    .unwrap_or("");
                all_lines.extend(generate_diff_lines_from_strings(old, new));
            }
            all_lines
        }
        "write" => {
            // For write, show the new content as additions
            let content = tool
                .input
                .get("content")
                .and_then(|v| v.as_str())
                .unwrap_or("");
            generate_diff_lines_from_strings("", content)
        }
        _ => Vec::new(),
    }
}

/// Generate ParsedDiffLine entries from old/new strings
fn generate_diff_lines_from_strings(old: &str, new: &str) -> Vec<ParsedDiffLine> {
    use similar::ChangeTag;

    let diff = similar::TextDiff::from_lines(old, new);
    let mut lines = Vec::new();
    let mut line_num = 1usize;

    for change in diff.iter_all_changes() {
        let content = change.value().trim();
        if content.is_empty() {
            if change.tag() != ChangeTag::Equal {
                line_num += 1;
            }
            continue;
        }

        match change.tag() {
            ChangeTag::Delete => {
                lines.push(ParsedDiffLine {
                    kind: DiffLineKind::Del,
                    prefix: format!("{}- ", line_num),
                    content: content.to_string(),
                });
                line_num += 1;
            }
            ChangeTag::Insert => {
                lines.push(ParsedDiffLine {
                    kind: DiffLineKind::Add,
                    prefix: format!("{}+ ", line_num),
                    content: content.to_string(),
                });
                line_num += 1;
            }
            ChangeTag::Equal => {
                line_num += 1;
            }
        }
    }

    lines
}

fn collect_diff_lines(content: &str) -> Vec<ParsedDiffLine> {
    content.lines().filter_map(parse_diff_line).collect()
}

fn parse_diff_line(raw_line: &str) -> Option<ParsedDiffLine> {
    let trimmed = raw_line.trim();
    if trimmed.is_empty() || trimmed == "..." {
        return None;
    }
    if trimmed.starts_with("diff --git ")
        || trimmed.starts_with("index ")
        || trimmed.starts_with("--- ")
        || trimmed.starts_with("+++ ")
        || trimmed.starts_with("@@ ")
        || trimmed.starts_with("\\ No newline")
    {
        return None;
    }

    // Compact diff format: "42- old" / "42+ new"
    if let Some(pos) = trimmed.find("- ") {
        let (prefix, content) = trimmed.split_at(pos + 2);
        if !prefix.is_empty() && prefix[..pos].chars().all(|c| c.is_ascii_digit()) {
            return Some(ParsedDiffLine {
                kind: DiffLineKind::Del,
                prefix: prefix.to_string(),
                content: trim_diff_content(content),
            });
        }
    }
    if let Some(pos) = trimmed.find("+ ") {
        let (prefix, content) = trimmed.split_at(pos + 2);
        if !prefix.is_empty() && prefix[..pos].chars().all(|c| c.is_ascii_digit()) {
            return Some(ParsedDiffLine {
                kind: DiffLineKind::Add,
                prefix: prefix.to_string(),
                content: trim_diff_content(content),
            });
        }
    }

    // Unified diff format: "+added" / "-removed"
    if let Some(rest) = raw_line.strip_prefix('+') {
        return Some(ParsedDiffLine {
            kind: DiffLineKind::Add,
            prefix: "+".to_string(),
            content: trim_diff_content(rest),
        });
    }
    if let Some(rest) = raw_line.strip_prefix('-') {
        return Some(ParsedDiffLine {
            kind: DiffLineKind::Del,
            prefix: "-".to_string(),
            content: trim_diff_content(rest),
        });
    }

    None
}

fn trim_diff_content(content: &str) -> String {
    content
        .trim_start_matches(|c| c == ' ' || c == '\t')
        .to_string()
}

/// Extract prefix (line number + sign) and content from diff line
/// "42- content" -> ("42- ", "content")
fn extract_diff_prefix_and_content(line: &str) -> (&str, &str) {
    // Format is "42- content" or "42+ content"
    if let Some(pos) = line.find("- ") {
        (&line[..pos + 2], &line[pos + 2..])
    } else if let Some(pos) = line.find("+ ") {
        (&line[..pos + 2], &line[pos + 2..])
    } else {
        (line, "")
    }
}

/// Tint a syntax-highlighted span with a diff color (green/red)
/// Blends the syntax color with the diff color for a subtle tint
fn tint_span_with_diff_color(span: Span<'static>, diff_color: Color) -> Span<'static> {
    let (dr, dg, db) = match diff_color {
        Color::Rgb(r, g, b) => (r, g, b),
        _ => return span,
    };

    // Get the span's foreground color
    let fg = span.style.fg.unwrap_or(Color::White);
    let (sr, sg, sb) = match fg {
        Color::Rgb(r, g, b) => (r, g, b),
        Color::White => (255, 255, 255),
        Color::Black => (0, 0, 0),
        _ => return span, // Can't tint indexed colors easily
    };

    // Blend: 70% syntax color + 30% diff color
    let blend = |s: u8, d: u8| -> u8 { ((s as u16 * 70 + d as u16 * 30) / 100) as u8 };

    let tinted = Color::Rgb(blend(sr, dr), blend(sg, dg), blend(sb, db));
    Span::styled(span.content, span.style.fg(tinted))
}

/// Render a diff line with word-level highlighting for changed parts
fn render_diff_line_with_highlights(
    full_line: &str,
    this_content: &str,
    other_content: &str,
    is_deletion: bool,
) -> Line<'static> {
    use similar::{ChangeTag, TextDiff};

    let (base_color, highlight_color) = if is_deletion {
        (DIFF_DEL_COLOR, DIFF_HIGHLIGHT_DEL)
    } else {
        (DIFF_ADD_COLOR, DIFF_HIGHLIGHT_ADD)
    };

    // Get prefix (line number and +/-)
    let prefix = if let Some(pos) = full_line.find(if is_deletion { "- " } else { "+ " }) {
        &full_line[..pos + 2]
    } else {
        ""
    };

    // Do word-level diff
    let diff = TextDiff::from_words(
        if is_deletion {
            this_content
        } else {
            other_content
        },
        if is_deletion {
            other_content
        } else {
            this_content
        },
    );

    let mut spans: Vec<Span<'static>> = vec![
        Span::styled("    ".to_string(), Style::default()),
        Span::styled(prefix.to_string(), Style::default().fg(base_color)),
    ];

    // Build spans with highlighting for changed words
    for change in diff.iter_all_changes() {
        let text = change.value().to_string();
        let style = match change.tag() {
            ChangeTag::Equal => Style::default().fg(base_color),
            ChangeTag::Insert if !is_deletion => {
                // This is a new word in the addition - highlight it
                Style::default().fg(highlight_color).bold()
            }
            ChangeTag::Delete if is_deletion => {
                // This is a removed word in the deletion - highlight it
                Style::default().fg(highlight_color).bold()
            }
            _ => Style::default().fg(base_color),
        };
        spans.push(Span::styled(text, style));
    }

    Line::from(spans)
}

/// Map provider-side tool names to internal display names.
/// Mirrors `Registry::resolve_tool_name` so the TUI shows friendly names.
fn resolve_display_tool_name(name: &str) -> &str {
    match name {
        "task" | "task_runner" => "subagent",
        "shell_exec" => "bash",
        "file_read" => "read",
        "file_write" => "write",
        "file_edit" => "edit",
        "file_glob" => "glob",
        "file_grep" => "grep",
        "todo_read" => "todoread",
        "todo_write" => "todowrite",
        other => other,
    }
}

/// Parse batch result content to determine per-sub-call success/error.
/// Returns a Vec<bool> where `true` means that sub-call errored.
/// The batch output format is:
///   --- [1] tool_name ---
///   <output or Error: ...>
///   --- [2] tool_name ---
///   ...
fn parse_batch_sub_results(content: &str) -> Vec<bool> {
    let mut results = Vec::new();
    let mut current_errored = false;
    let mut in_section = false;

    for line in content.lines() {
        if line.starts_with("--- [") && line.ends_with(" ---") {
            if in_section {
                results.push(current_errored);
            }
            in_section = true;
            current_errored = false;
        } else if in_section
            && (line.starts_with("Error:") || line.starts_with("error:") || line.starts_with("Failed:"))
        {
            current_errored = true;
        }
    }
    if in_section {
        results.push(current_errored);
    }
    results
}

/// Extract a brief summary from a tool call input (file path, command, etc.)
fn get_tool_summary(tool: &ToolCall) -> String {
    let truncate = |s: &str, max_chars: usize| {
        match s.char_indices().nth(max_chars) {
            Some((byte_idx, _)) => format!("{}...", &s[..byte_idx]),
            None => s.to_string(),
        }
    };

    match tool.name.as_str() {
        "bash" => tool
            .input
            .get("command")
            .and_then(|v| v.as_str())
            .map(|cmd| format!("$ {}", truncate(cmd, 50)))
            .unwrap_or_default(),
        "read" => {
            let path = tool
                .input
                .get("file_path")
                .and_then(|v| v.as_str())
                .unwrap_or("");
            let offset = tool.input.get("offset").and_then(|v| v.as_u64());
            let limit = tool.input.get("limit").and_then(|v| v.as_u64());
            match (offset, limit) {
                (Some(o), Some(l)) => format!("{}:{}-{}", path, o, o + l),
                (Some(o), None) => format!("{}:{}", path, o),
                _ => path.to_string(),
            }
        }
        "write" | "edit" => tool
            .input
            .get("file_path")
            .and_then(|v| v.as_str())
            .map(|p| p.to_string())
            .unwrap_or_default(),
        "multiedit" => {
            let path = tool
                .input
                .get("file_path")
                .and_then(|v| v.as_str())
                .unwrap_or("");
            let count = tool
                .input
                .get("edits")
                .and_then(|v| v.as_array())
                .map(|a| a.len())
                .unwrap_or(0);
            format!("{} ({} edits)", path, count)
        }
        "glob" => tool
            .input
            .get("pattern")
            .and_then(|v| v.as_str())
            .map(|p| format!("'{}'", p))
            .unwrap_or_default(),
        "grep" => {
            let pattern = tool
                .input
                .get("pattern")
                .and_then(|v| v.as_str())
                .unwrap_or("");
            let path = tool.input.get("path").and_then(|v| v.as_str());
            if let Some(p) = path {
                format!("'{}' in {}", truncate(pattern, 30), p)
            } else {
                format!("'{}'", truncate(pattern, 40))
            }
        }
        "ls" => tool
            .input
            .get("path")
            .and_then(|v| v.as_str())
            .unwrap_or(".")
            .to_string(),
        "task" => {
            let desc = tool
                .input
                .get("description")
                .and_then(|v| v.as_str())
                .unwrap_or("task");
            let agent_type = tool
                .input
                .get("subagent_type")
                .and_then(|v| v.as_str())
                .unwrap_or("agent");
            format!("{} ({})", desc, agent_type)
        }
        "patch" | "apply_patch" => tool
            .input
            .get("patch_text")
            .and_then(|v| v.as_str())
            .map(|p| {
                let lines = p.lines().count();
                let first_file = p
                    .lines()
                    .find(|l| {
                        l.starts_with("--- ") || l.starts_with("+++ ") || l.starts_with("*** ")
                    })
                    .and_then(|l| {
                        let rest = l
                            .trim_start_matches("--- ")
                            .trim_start_matches("+++ ")
                            .trim_start_matches("*** ");
                        let path = rest.split_whitespace().next().unwrap_or("");
                        let path = path
                            .strip_prefix("a/")
                            .or(path.strip_prefix("b/"))
                            .unwrap_or(path);
                        if path.is_empty() || path == "/dev/null" {
                            None
                        } else {
                            Some(path.to_string())
                        }
                    });
                if let Some(file) = first_file {
                    format!("{} ({} lines)", file, lines)
                } else {
                    format!("({} lines)", lines)
                }
            })
            .unwrap_or_default(),
        "webfetch" => tool
            .input
            .get("url")
            .and_then(|v| v.as_str())
            .map(|u| truncate(u, 50))
            .unwrap_or_default(),
        "websearch" => tool
            .input
            .get("query")
            .and_then(|v| v.as_str())
            .map(|q| format!("'{}'", truncate(q, 40)))
            .unwrap_or_default(),
        "mcp" => {
            let action = tool
                .input
                .get("action")
                .and_then(|v| v.as_str())
                .unwrap_or("");
            let server = tool.input.get("server_name").and_then(|v| v.as_str());
            if let Some(s) = server {
                format!("{} {}", action, s)
            } else {
                action.to_string()
            }
        }
        "todoread" => "todos".to_string(),
        "todowrite" => {
            let count = tool
                .input
                .get("todos")
                .and_then(|v| v.as_array())
                .map(|a| a.len())
                .unwrap_or(0);
            format!("{} items", count)
        }
        "skill" => tool
            .input
            .get("skill")
            .and_then(|v| v.as_str())
            .map(|s| format!("/{}", s))
            .unwrap_or_default(),
        "codesearch" => tool
            .input
            .get("query")
            .and_then(|v| v.as_str())
            .map(|q| format!("'{}'", truncate(q, 40)))
            .unwrap_or_default(),
        "memory" => {
            let action = tool
                .input
                .get("action")
                .and_then(|v| v.as_str())
                .unwrap_or("?");
            match action {
                "remember" => {
                    let content = tool
                        .input
                        .get("content")
                        .and_then(|v| v.as_str())
                        .unwrap_or("");
                    format!("remember: {}", truncate(content, 35))
                }
                "recall" => {
                    let query = tool.input.get("query").and_then(|v| v.as_str());
                    if let Some(q) = query {
                        format!("recall '{}'", truncate(q, 35))
                    } else {
                        "recall (recent)".to_string()
                    }
                }
                "search" => {
                    let query = tool
                        .input
                        .get("query")
                        .and_then(|v| v.as_str())
                        .unwrap_or("");
                    format!("search '{}'", truncate(query, 35))
                }
                "forget" => {
                    let id = tool.input.get("id").and_then(|v| v.as_str()).unwrap_or("?");
                    format!("forget {}", truncate(id, 30))
                }
                "tag" => {
                    let id = tool.input.get("id").and_then(|v| v.as_str()).unwrap_or("?");
                    format!("tag {}", truncate(id, 30))
                }
                "link" => "link".to_string(),
                "related" => {
                    let id = tool.input.get("id").and_then(|v| v.as_str()).unwrap_or("?");
                    format!("related {}", truncate(id, 30))
                }
                _ => action.to_string(),
            }
        }
        "remember" => {
            let action = tool
                .input
                .get("action")
                .and_then(|v| v.as_str())
                .unwrap_or("store");
            match action {
                "store" => {
                    let content = tool
                        .input
                        .get("content")
                        .and_then(|v| v.as_str())
                        .unwrap_or("");
                    format!("store: {}", truncate(content, 40))
                }
                "search" => {
                    let query = tool
                        .input
                        .get("query")
                        .and_then(|v| v.as_str())
                        .unwrap_or("");
                    format!("search '{}'", truncate(query, 35))
                }
                _ => action.to_string(),
            }
        }
        "selfdev" => {
            let action = tool
                .input
                .get("action")
                .and_then(|v| v.as_str())
                .unwrap_or("?");
            action.to_string()
        }
        "communicate" => {
            let action = tool
                .input
                .get("action")
                .and_then(|v| v.as_str())
                .unwrap_or("?");
            let target = tool
                .input
                .get("to_session")
                .or_else(|| tool.input.get("target_session"))
                .or_else(|| tool.input.get("channel"))
                .and_then(|v| v.as_str());
            if let Some(t) = target {
                format!("{} → {}", action, truncate(t, 25))
            } else {
                action.to_string()
            }
        }
        "session_search" => tool
            .input
            .get("query")
            .and_then(|v| v.as_str())
            .map(|q| format!("'{}'", truncate(q, 40)))
            .unwrap_or_default(),
        "conversation_search" => {
            if let Some(q) = tool.input.get("query").and_then(|v| v.as_str()) {
                format!("'{}'", truncate(q, 40))
            } else if tool
                .input
                .get("stats")
                .and_then(|v| v.as_bool())
                .unwrap_or(false)
            {
                "stats".to_string()
            } else {
                "history".to_string()
            }
        }
        "lsp" => {
            let op = tool
                .input
                .get("operation")
                .and_then(|v| v.as_str())
                .unwrap_or("?");
            let file = tool
                .input
                .get("filePath")
                .and_then(|v| v.as_str())
                .unwrap_or("");
            let short_file = file.rsplit('/').next().unwrap_or(file);
            let line = tool.input.get("line").and_then(|v| v.as_u64()).unwrap_or(0);
            format!("{} {}:{}", op, short_file, line)
        }
        "bg" => {
            let action = tool
                .input
                .get("action")
                .and_then(|v| v.as_str())
                .unwrap_or("?");
            let task_id = tool.input.get("task_id").and_then(|v| v.as_str());
            if let Some(id) = task_id {
                format!("{} {}", action, truncate(id, 20))
            } else {
                action.to_string()
            }
        }
        "batch" => {
            let count = tool
                .input
                .get("tool_calls")
                .and_then(|v| v.as_array())
                .map(|a| a.len())
                .unwrap_or(0);
            format!("{} calls", count)
        }
        "subagent" => {
            let desc = tool
                .input
                .get("description")
                .and_then(|v| v.as_str())
                .unwrap_or("task");
            let agent_type = tool
                .input
                .get("subagent_type")
                .and_then(|v| v.as_str())
                .unwrap_or("agent");
            format!("{} ({})", desc, agent_type)
        }
        "debug_socket" => {
            let cmd = tool
                .input
                .get("command")
                .and_then(|v| v.as_str())
                .unwrap_or("?");
            truncate(cmd, 40)
        }
        // MCP tools (prefixed with mcp__)
        name if name.starts_with("mcp__") => {
            // Show first string parameter as summary
            tool.input
                .as_object()
                .and_then(|obj| obj.iter().find(|(_, v)| v.is_string()))
                .and_then(|(_, v)| v.as_str())
                .map(|s| truncate(s, 40))
                .unwrap_or_default()
        }
        _ => String::new(),
    }
}

// ─── Pinned content pane (diffs + images) ───────────────────────────────────

enum PinnedContentEntry {
    Diff {
        file_path: String,
        lines: Vec<ParsedDiffLine>,
        additions: usize,
        deletions: usize,
    },
    Image {
        file_path: String,
        hash: u64,
        width: u32,
        height: u32,
    },
}

fn collect_pinned_content(
    messages: &[DisplayMessage],
    collect_diffs: bool,
    collect_images: bool,
) -> Vec<PinnedContentEntry> {
    let mut entries = Vec::new();
    for msg in messages {
        if msg.role != "tool" {
            continue;
        }
        let Some(ref tc) = msg.tool_data else {
            continue;
        };

        if collect_images && matches!(tc.name.as_str(), "read" | "Read") {
            let file_path = tc
                .input
                .get("file_path")
                .and_then(|v| v.as_str())
                .unwrap_or("")
                .to_string();
            let path = std::path::Path::new(&file_path);
            if is_supported_image_ext(path) && path.exists() {
                if let Some((w, h)) = get_image_dimensions_from_path(path) {
                    let hash = super::mermaid::register_external_image(path, w, h);
                    entries.push(PinnedContentEntry::Image {
                        file_path,
                        hash,
                        width: w,
                        height: h,
                    });
                }
            }
            continue;
        }

        if !collect_diffs {
            continue;
        }

        if !matches!(tc.name.as_str(), "edit" | "Edit" | "write" | "multiedit") {
            continue;
        }

        let file_path = tc
            .input
            .get("file_path")
            .and_then(|v| v.as_str())
            .unwrap_or("unknown")
            .to_string();

        let change_lines = {
            let from_content = collect_diff_lines(&msg.content);
            if !from_content.is_empty() {
                from_content
            } else {
                generate_diff_lines_from_tool_input(tc)
            }
        };
        if change_lines.is_empty() {
            continue;
        }

        let additions = change_lines
            .iter()
            .filter(|l| l.kind == DiffLineKind::Add)
            .count();
        let deletions = change_lines
            .iter()
            .filter(|l| l.kind == DiffLineKind::Del)
            .count();

        entries.push(PinnedContentEntry::Diff {
            file_path,
            lines: change_lines,
            additions,
            deletions,
        });
    }
    entries
}

fn is_supported_image_ext(path: &std::path::Path) -> bool {
    path.extension()
        .and_then(|e| e.to_str())
        .map(|ext| {
            matches!(
                ext.to_lowercase().as_str(),
                "png" | "jpg" | "jpeg" | "gif" | "webp" | "bmp"
            )
        })
        .unwrap_or(false)
}

fn get_image_dimensions_from_path(path: &std::path::Path) -> Option<(u32, u32)> {
    let data = std::fs::read(path).ok()?;
    // PNG
    if data.len() > 24 && &data[0..8] == b"\x89PNG\r\n\x1a\n" {
        let w = u32::from_be_bytes([data[16], data[17], data[18], data[19]]);
        let h = u32::from_be_bytes([data[20], data[21], data[22], data[23]]);
        return Some((w, h));
    }
    // JPEG: search for SOF0 marker
    if data.len() > 2 && data[0] == 0xFF && data[1] == 0xD8 {
        let mut i = 2;
        while i + 9 < data.len() {
            if data[i] == 0xFF {
                let marker = data[i + 1];
                if marker == 0xC0 || marker == 0xC2 {
                    let h = u16::from_be_bytes([data[i + 5], data[i + 6]]) as u32;
                    let w = u16::from_be_bytes([data[i + 7], data[i + 8]]) as u32;
                    return Some((w, h));
                }
                if marker == 0xD9 || marker == 0xDA {
                    break;
                }
                let len = u16::from_be_bytes([data[i + 2], data[i + 3]]) as usize;
                i += 2 + len;
            } else {
                i += 1;
            }
        }
    }
    // GIF
    if data.len() > 10 && (&data[0..4] == b"GIF8") {
        let w = u16::from_le_bytes([data[6], data[7]]) as u32;
        let h = u16::from_le_bytes([data[8], data[9]]) as u32;
        return Some((w, h));
    }
    None
}

fn draw_pinned_content(
    frame: &mut Frame,
    area: Rect,
    entries: &[PinnedContentEntry],
    scroll: usize,
) {
    use ratatui::widgets::{Block, BorderType, Borders, Paragraph, Wrap};

    if area.width < 10 || area.height < 3 {
        return;
    }

    let total_diffs = entries
        .iter()
        .filter(|e| matches!(e, PinnedContentEntry::Diff { .. }))
        .count();
    let total_images = entries
        .iter()
        .filter(|e| matches!(e, PinnedContentEntry::Image { .. }))
        .count();
    let total_additions: usize = entries
        .iter()
        .map(|e| match e {
            PinnedContentEntry::Diff { additions, .. } => *additions,
            _ => 0,
        })
        .sum();
    let total_deletions: usize = entries
        .iter()
        .map(|e| match e {
            PinnedContentEntry::Diff { deletions, .. } => *deletions,
            _ => 0,
        })
        .sum();

    let mut title_parts = vec![Span::styled(" pinned ", Style::default().fg(TOOL_COLOR))];
    if total_diffs > 0 {
        title_parts.push(Span::styled(
            format!("+{}", total_additions),
            Style::default().fg(DIFF_ADD_COLOR),
        ));
        title_parts.push(Span::styled(" ", Style::default().fg(DIM_COLOR)));
        title_parts.push(Span::styled(
            format!("-{}", total_deletions),
            Style::default().fg(DIFF_DEL_COLOR),
        ));
        title_parts.push(Span::styled(
            format!(" {}f", total_diffs),
            Style::default().fg(DIM_COLOR),
        ));
    }
    if total_images > 0 {
        if total_diffs > 0 {
            title_parts.push(Span::styled(" ", Style::default().fg(DIM_COLOR)));
        }
        title_parts.push(Span::styled(
            format!("📷{}", total_images),
            Style::default().fg(DIM_COLOR),
        ));
    }
    title_parts.push(Span::styled(" ", Style::default().fg(DIM_COLOR)));

    let block = Block::default()
        .borders(Borders::ALL)
        .border_type(BorderType::Rounded)
        .border_style(Style::default().fg(DIM_COLOR))
        .title(Line::from(title_parts));

    let inner = block.inner(area);
    frame.render_widget(block, area);

    if inner.width == 0 || inner.height == 0 {
        return;
    }

    let mut text_lines: Vec<Line<'static>> = Vec::new();

    struct ImagePlacement {
        after_text_line: usize,
        hash: u64,
        rows: u16,
    }
    let mut image_placements: Vec<ImagePlacement> = Vec::new();

    let has_protocol = super::mermaid::protocol_type().is_some();

    for (i, entry) in entries.iter().enumerate() {
        if i > 0 {
            text_lines.push(Line::from(""));
        }

        match entry {
            PinnedContentEntry::Diff {
                file_path,
                lines: diff_lines,
                additions,
                deletions,
            } => {
                let short_path = file_path
                    .rsplit('/')
                    .take(2)
                    .collect::<Vec<_>>()
                    .into_iter()
                    .rev()
                    .collect::<Vec<_>>()
                    .join("/");

                let file_ext = std::path::Path::new(file_path)
                    .extension()
                    .and_then(|e| e.to_str());

                text_lines.push(Line::from(vec![
                    Span::styled("── ", Style::default().fg(DIM_COLOR)),
                    Span::styled(
                        short_path,
                        Style::default()
                            .fg(Color::Rgb(180, 200, 255))
                            .add_modifier(ratatui::style::Modifier::BOLD),
                    ),
                    Span::styled(" (", Style::default().fg(DIM_COLOR)),
                    Span::styled(
                        format!("+{}", additions),
                        Style::default().fg(DIFF_ADD_COLOR),
                    ),
                    Span::styled(" ", Style::default().fg(DIM_COLOR)),
                    Span::styled(
                        format!("-{}", deletions),
                        Style::default().fg(DIFF_DEL_COLOR),
                    ),
                    Span::styled(")", Style::default().fg(DIM_COLOR)),
                ]));

                for line in diff_lines {
                    let base_color = if line.kind == DiffLineKind::Add {
                        DIFF_ADD_COLOR
                    } else {
                        DIFF_DEL_COLOR
                    };

                    let mut spans: Vec<Span<'static>> = vec![Span::styled(
                        line.prefix.clone(),
                        Style::default().fg(base_color),
                    )];

                    if !line.content.is_empty() {
                        let highlighted =
                            markdown::highlight_line(line.content.as_str(), file_ext);
                        for span in highlighted {
                            let tinted = tint_span_with_diff_color(span, base_color);
                            spans.push(tinted);
                        }
                    }

                    text_lines.push(Line::from(spans));
                }
            }
            PinnedContentEntry::Image {
                file_path,
                hash,
                width: img_w,
                height: img_h,
            } => {
                let short_path = file_path
                    .rsplit('/')
                    .take(2)
                    .collect::<Vec<_>>()
                    .into_iter()
                    .rev()
                    .collect::<Vec<_>>()
                    .join("/");

                text_lines.push(Line::from(vec![
                    Span::styled("── 📷 ", Style::default().fg(DIM_COLOR)),
                    Span::styled(
                        short_path,
                        Style::default()
                            .fg(Color::Rgb(180, 200, 255))
                            .add_modifier(ratatui::style::Modifier::BOLD),
                    ),
                    Span::styled(
                        format!(" {}×{}", img_w, img_h),
                        Style::default().fg(DIM_COLOR),
                    ),
                ]));

                if has_protocol {
                    let img_rows = inner.height.min(12).max(4);
                    image_placements.push(ImagePlacement {
                        after_text_line: text_lines.len(),
                        hash: *hash,
                        rows: img_rows,
                    });
                    for _ in 0..img_rows {
                        text_lines.push(Line::from(""));
                    }
                }
            }
        }
    }

    if text_lines.is_empty() {
        text_lines.push(Line::from(Span::styled(
            "No content yet",
            Style::default().fg(DIM_COLOR),
        )));
    }

    let visible_lines: Vec<Line<'static>> = text_lines.into_iter().skip(scroll).collect();

    let paragraph = Paragraph::new(visible_lines).wrap(Wrap { trim: false });
    frame.render_widget(paragraph, inner);

    if has_protocol {
        for placement in &image_placements {
            let text_y = placement.after_text_line as u16;
            if text_y < scroll as u16 {
                continue;
            }
            let y_in_inner = text_y.saturating_sub(scroll as u16);
            if y_in_inner >= inner.height {
                continue;
            }
            let avail_rows = inner.height.saturating_sub(y_in_inner).min(placement.rows);
            if avail_rows < 2 {
                continue;
            }
            let img_area = Rect {
                x: inner.x,
                y: inner.y + y_in_inner,
                width: inner.width,
                height: avail_rows,
            };
            super::mermaid::render_image_widget_fit(
                placement.hash,
                img_area,
                frame.buffer_mut(),
                false,
                false,
            );
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_calculate_input_lines_empty() {
        assert_eq!(calculate_input_lines("", 80), 1);
    }

    #[test]
    fn test_calculate_input_lines_single_line() {
        assert_eq!(calculate_input_lines("hello", 80), 1);
        assert_eq!(calculate_input_lines("hello world", 80), 1);
    }

    #[test]
    fn test_calculate_input_lines_wrapped() {
        // 10 chars with width 5 = 2 lines
        assert_eq!(calculate_input_lines("aaaaaaaaaa", 5), 2);
        // 15 chars with width 5 = 3 lines
        assert_eq!(calculate_input_lines("aaaaaaaaaaaaaaa", 5), 3);
    }

    #[test]
    fn test_calculate_input_lines_with_newlines() {
        // Two lines separated by newline
        assert_eq!(calculate_input_lines("hello\nworld", 80), 2);
        // Three lines
        assert_eq!(calculate_input_lines("a\nb\nc", 80), 3);
        // Trailing newline
        assert_eq!(calculate_input_lines("hello\n", 80), 2);
    }

    #[test]
    fn test_calculate_input_lines_newlines_and_wrapping() {
        // First line wraps (10 chars / 5 = 2), second line is short (1)
        assert_eq!(calculate_input_lines("aaaaaaaaaa\nb", 5), 3);
    }

    #[test]
    fn test_calculate_input_lines_zero_width() {
        assert_eq!(calculate_input_lines("hello", 0), 1);
    }

    #[test]
    fn test_wrap_input_text_empty() {
        let (lines, cursor_line, cursor_col) = wrap_input_text("", 0, 80, "1", "> ", USER_COLOR, 3);
        assert_eq!(lines.len(), 1);
        assert_eq!(cursor_line, 0);
        assert_eq!(cursor_col, 0);
    }

    #[test]
    fn test_wrap_input_text_simple() {
        let (lines, cursor_line, cursor_col) =
            wrap_input_text("hello", 5, 80, "1", "> ", USER_COLOR, 3);
        assert_eq!(lines.len(), 1);
        assert_eq!(cursor_line, 0);
        assert_eq!(cursor_col, 5); // cursor at end
    }

    #[test]
    fn test_wrap_input_text_cursor_middle() {
        let (lines, cursor_line, cursor_col) =
            wrap_input_text("hello world", 6, 80, "1", "> ", USER_COLOR, 3);
        assert_eq!(lines.len(), 1);
        assert_eq!(cursor_line, 0);
        assert_eq!(cursor_col, 6); // cursor at 'w'
    }

    #[test]
    fn test_wrap_input_text_wrapping() {
        // 10 chars with width 5 = 2 lines
        let (lines, cursor_line, cursor_col) =
            wrap_input_text("aaaaaaaaaa", 7, 5, "1", "> ", USER_COLOR, 3);
        assert_eq!(lines.len(), 2);
        assert_eq!(cursor_line, 1); // second line
        assert_eq!(cursor_col, 2); // 7 - 5 = 2
    }

    #[test]
    fn test_wrap_input_text_with_newlines() {
        let (lines, cursor_line, cursor_col) =
            wrap_input_text("hello\nworld", 6, 80, "1", "> ", USER_COLOR, 3);
        assert_eq!(lines.len(), 2);
        assert_eq!(cursor_line, 1); // second line (after newline)
        assert_eq!(cursor_col, 0); // at start of 'world'
    }

    #[test]
    fn test_wrap_input_text_cursor_at_end_of_wrapped() {
        // 10 chars with width 5, cursor at position 10 (end)
        let (lines, cursor_line, cursor_col) =
            wrap_input_text("aaaaaaaaaa", 10, 5, "1", "> ", USER_COLOR, 3);
        assert_eq!(lines.len(), 2);
        assert_eq!(cursor_line, 1);
        assert_eq!(cursor_col, 5);
    }

    #[test]
    fn test_wrap_input_text_many_lines() {
        // Create text that spans 15 lines when wrapped to width 10
        let text = "a".repeat(150);
        let (lines, cursor_line, cursor_col) =
            wrap_input_text(&text, 145, 10, "1", "> ", USER_COLOR, 3);
        assert_eq!(lines.len(), 15);
        assert_eq!(cursor_line, 14); // last line
        assert_eq!(cursor_col, 5); // 145 % 10 = 5
    }

    #[test]
    fn test_wrap_input_text_multiple_newlines() {
        let (lines, cursor_line, cursor_col) =
            wrap_input_text("a\nb\nc\nd", 6, 80, "1", "> ", USER_COLOR, 3);
        assert_eq!(lines.len(), 4);
        assert_eq!(cursor_line, 3); // on 'd' line
        assert_eq!(cursor_col, 0);
    }

    #[test]
    fn test_compute_visible_margins_centered_respects_line_alignment() {
        let lines = vec![
            ratatui::text::Line::from("centered").centered(),
            ratatui::text::Line::from("left block").left_aligned(),
            ratatui::text::Line::from("right").right_aligned(),
        ];
        let area = Rect::new(0, 0, 20, 3);
        let margins = compute_visible_margins(&lines, &[], 0, area, true);

        // centered: used=8 => total_margin=12 => 6/6 split
        assert_eq!(margins.left_widths[0], 6);
        assert_eq!(margins.right_widths[0], 6);

        // left-aligned: used=10 => left=0, right=10
        assert_eq!(margins.left_widths[1], 0);
        assert_eq!(margins.right_widths[1], 10);

        // right-aligned: used=5 => left=15, right=0
        assert_eq!(margins.left_widths[2], 15);
        assert_eq!(margins.right_widths[2], 0);
    }

    #[test]
    fn test_estimate_pinned_diagram_pane_width_scales_to_height() {
        let diagram = info_widget::DiagramInfo {
            hash: 1,
            width: 800,
            height: 600,
            label: None,
        };
        let width = estimate_pinned_diagram_pane_width_with_font(&diagram, 20, 24, Some((8, 16)));
        assert_eq!(width, 50);
    }

    #[test]
    fn test_estimate_pinned_diagram_pane_width_respects_minimum() {
        let diagram = info_widget::DiagramInfo {
            hash: 2,
            width: 120,
            height: 120,
            label: None,
        };
        let width = estimate_pinned_diagram_pane_width_with_font(&diagram, 10, 24, Some((8, 16)));
        assert_eq!(width, 24);
    }
}
