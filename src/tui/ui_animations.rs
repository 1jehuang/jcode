#![allow(dead_code)]

use super::dim_color;
use crate::tui::{STARTUP_ANIMATION_WINDOW, TuiState, color_support::rgb};
use ratatui::{prelude::*, widgets::Paragraph};
use std::cell::RefCell;
use std::collections::{HashSet, hash_map::DefaultHasher};
use std::hash::{Hash, Hasher};
use std::sync::OnceLock;

const STARTUP_ASCII_STATUS_FPS: f32 = 12.0;
const STARTUP_ASCII_STATUS_SPINNER: &[&str] = &["|", "/", "-", "\\"];
const LUMINANCE: &[u8] = b".,-~:;=!*#$@";

const STARTUP_VARIANTS: &[&str] = &["donut", "globe", "cube", "octahedron", "lorenz", "rabbit"];

const IDLE_VARIANTS: &[&str] = &["donut", "knot", "three_rings"];

struct RenderBuffers {
    output: Vec<Vec<u8>>,
    zbuffer: Vec<Vec<f32>>,
    width: usize,
    height: usize,
}

impl RenderBuffers {
    fn new() -> Self {
        Self {
            output: Vec::new(),
            zbuffer: Vec::new(),
            width: 0,
            height: 0,
        }
    }

    fn resize_and_clear(&mut self, width: usize, height: usize) {
        if self.width != width || self.height != height {
            self.output.resize_with(height, Vec::new);
            self.output.truncate(height);
            self.zbuffer.resize_with(height, Vec::new);
            self.zbuffer.truncate(height);
            for row in &mut self.output {
                row.resize(width, b' ');
            }
            for row in &mut self.zbuffer {
                row.resize(width, 0.0);
            }
            self.width = width;
            self.height = height;
        }
        for row in &mut self.output {
            row.fill(b' ');
        }
        for row in &mut self.zbuffer {
            row.fill(0.0);
        }
    }
}

thread_local! {
    static RENDER_BUF: RefCell<RenderBuffers> = RefCell::new(RenderBuffers::new());
}

fn with_render_buffers<F>(width: usize, height: usize, f: F) -> Vec<String>
where
    F: FnOnce(&mut Vec<Vec<u8>>, &mut Vec<Vec<f32>>),
{
    RENDER_BUF.with(|cell| {
        let mut buf = cell.borrow_mut();
        buf.resize_and_clear(width, height);
        let buf = &mut *buf;
        f(&mut buf.output, &mut buf.zbuffer);
        buf.output
            .iter()
            .map(|row| String::from_utf8_lossy(row).into_owned())
            .collect()
    })
}

fn render_donut(elapsed: f32, width: usize, height: usize) -> Vec<String> {
    let a = elapsed * 1.5;
    let b = elapsed * 0.8;
    let cos_a = a.cos();
    let sin_a = a.sin();
    let cos_b = b.cos();
    let sin_b = b.sin();

    with_render_buffers(width, height, |output, zbuffer| {
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

                let x = circle_x * (cos_b * cos_phi + sin_a * sin_b * sin_phi)
                    - circle_y * cos_a * sin_b;
                let y = circle_x * (sin_b * cos_phi - sin_a * cos_b * sin_phi)
                    + circle_y * cos_a * cos_b;
                let z = 5.0 + cos_a * circle_x * sin_phi + circle_y * sin_a;
                let ooz = 1.0 / z;

                let xp = (width as f32 / 2.0 + width as f32 * 0.35 * ooz * x) as isize;
                let yp = (height as f32 / 2.0 - height as f32 * 0.35 * ooz * y) as isize;

                let lum =
                    cos_phi * cos_theta * sin_b - cos_a * cos_theta * sin_phi - sin_a * sin_theta
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
    })
}

fn render_startup_animation(
    elapsed: f32,
    width: usize,
    height: usize,
    variant: &str,
) -> Vec<String> {
    match variant {
        "donut" => render_donut(elapsed, width, height),
        "globe" => render_globe(elapsed, width, height),
        "cube" => render_cube(elapsed, width, height),
        "mobius" => render_mobius(elapsed, width, height),
        "octahedron" => render_octahedron(elapsed, width, height),
        "lorenz" => render_lorenz(elapsed, width, height),
        "rabbit" => render_rabbit(elapsed, width, height),
        "black_hole" => render_black_hole(elapsed, width, height),
        _ => render_donut(elapsed, width, height),
    }
}

fn render_black_hole(elapsed: f32, width: usize, height: usize) -> Vec<String> {
    with_render_buffers(width, height, |output, _zbuffer| {
        let cx = width as f32 / 2.0;
        let cy = height as f32 / 2.0;
        let aspect = 0.5f32;
        let disk_half_len = width as f32 * 0.36;
        let disk_half_thickness = (height as f32 * 0.055).max(0.8);
        let horizon_r = height.min(width / 3) as f32 * 0.16;
        let halo_r = horizon_r * 1.85;
        let shimmer = elapsed * 2.4;

        for y in 0..height {
            for x in 0..width {
                let dx = x as f32 - cx;
                let dy = (y as f32 - cy) / aspect;
                let r = (dx * dx + dy * dy).sqrt();

                let abs_x = dx.abs();
                let abs_y = dy.abs();

                let disk_falloff_x = (1.0 - abs_x / disk_half_len).clamp(0.0, 1.0);
                let disk_core = (1.0 - abs_y / disk_half_thickness).clamp(0.0, 1.0);
                let disk_glow =
                    (1.0 - abs_y / (disk_half_thickness * 3.8 + 1.0)).clamp(0.0, 1.0) * 0.42;
                let lens_band = (1.0
                    - ((abs_y - horizon_r * 0.72).abs() / (horizon_r * 0.55 + 0.1)))
                    .clamp(0.0, 1.0)
                    * (1.0 - abs_x / (halo_r * 1.5 + 1.0)).clamp(0.0, 1.0);
                let halo =
                    (1.0 - ((r - halo_r).abs() / (horizon_r * 0.95 + 0.1))).clamp(0.0, 1.0) * 0.38;

                let streak_phase = shimmer + dx * 0.33;
                let streaks = ((streak_phase.sin() * 0.5 + 0.5) * 0.55
                    + ((streak_phase * 0.47 + 1.7).sin() * 0.5 + 0.5) * 0.45)
                    * disk_falloff_x;
                let relativistic_beam = (1.0
                    - ((dx - disk_half_len * 0.34).abs() / (disk_half_len * 0.52 + 0.1)))
                    .clamp(0.0, 1.0)
                    * 0.32;

                let mut brightness = disk_core * (0.55 + 0.45 * streaks) * disk_falloff_x
                    + disk_glow * disk_falloff_x
                    + lens_band * 0.62
                    + halo * 0.28
                    + relativistic_beam * disk_core;

                if r <= horizon_r {
                    brightness = 0.0;
                }

                if abs_x <= horizon_r * 0.95 && abs_y <= disk_half_thickness * 1.2 {
                    brightness *= (abs_x / (horizon_r * 0.95 + 0.1)).clamp(0.0, 1.0);
                }

                brightness = brightness.clamp(0.0, 1.0);
                let idx = (brightness * (LUMINANCE.len() - 1) as f32) as usize;
                output[y][x] = LUMINANCE[idx.min(LUMINANCE.len() - 1)];
            }
        }
    })
}

fn render_globe(elapsed: f32, width: usize, height: usize) -> Vec<String> {
    with_render_buffers(width, height, |output, zbuffer| {
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
    })
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

#[allow(clippy::too_many_arguments)]
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
    with_render_buffers(width, height, |output, zbuffer| {
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
                output, zbuffer, x0, y0, z0, x1, y1, z1, width, height, cam_dist, b'#',
            );
        }
        for &(x, y, z) in &rotated {
            if let Some((xp, yp, _)) = project_3d(x, y, z, width, height, cam_dist) {
                if xp >= 0 && (xp as usize) < width && yp >= 0 && (yp as usize) < height {
                    output[yp as usize][xp as usize] = b'@';
                }
            }
        }
    })
}

fn render_mobius(elapsed: f32, width: usize, height: usize) -> Vec<String> {
    with_render_buffers(width, height, |output, zbuffer| {
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
    })
}

fn render_octahedron(elapsed: f32, width: usize, height: usize) -> Vec<String> {
    with_render_buffers(width, height, |output, zbuffer| {
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
                output, zbuffer, x0, y0, z0, x1, y1, z1, width, height, cam_dist, b'=',
            );
        }
        for &(x, y, z) in &rotated {
            if let Some((xp, yp, _)) = project_3d(x, y, z, width, height, cam_dist) {
                if xp >= 0 && (xp as usize) < width && yp >= 0 && (yp as usize) < height {
                    output[yp as usize][xp as usize] = b'@';
                }
            }
        }
    })
}

fn render_lorenz(elapsed: f32, width: usize, height: usize) -> Vec<String> {
    with_render_buffers(width, height, |output, _zbuffer| {
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
        let total_steps = (4000 + (elapsed * 500.0) as usize).min(8000);
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
                let xp = (cx + rx * scale_x) as isize;
                let yp = (cy - z * scale_y) as isize;
                if xp >= 0 && (xp as usize) < width && yp >= 0 && (yp as usize) < height {
                    let age = (step - trail_start) as f32 / 3000.0;
                    let li = (age * (LUMINANCE.len() - 1) as f32) as usize;
                    let ch = LUMINANCE[li.min(LUMINANCE.len() - 1)];
                    if ch > output[yp as usize][xp as usize]
                        || output[yp as usize][xp as usize] == b' '
                    {
                        output[yp as usize][xp as usize] = ch;
                    }
                }
            }
        }
    })
}

fn render_rabbit(elapsed: f32, width: usize, height: usize) -> Vec<String> {
    with_render_buffers(width, height, |output, zbuffer| {
        let ax = -0.25;
        let ay = elapsed * 0.4;
        let az = 0.0;
        let cam_dist = 5.0;

        let sample = |output: &mut Vec<Vec<u8>>,
                      zbuffer: &mut Vec<Vec<f32>>,
                      cx: f32,
                      cy: f32,
                      cz: f32,
                      rx: f32,
                      ry: f32,
                      rz: f32| {
            let max_r = rx.max(ry).max(rz);
            let step = (0.05 / max_r).clamp(0.03, 0.12);
            let mut theta: f32 = 0.0;
            while theta < std::f32::consts::TAU {
                let ct = theta.cos();
                let st = theta.sin();
                let mut phi: f32 = -std::f32::consts::FRAC_PI_2;
                while phi < std::f32::consts::FRAC_PI_2 {
                    let cp = phi.cos();
                    let sp = phi.sin();
                    let lx = rx * cp * ct;
                    let ly = ry * sp;
                    let lz = rz * cp * st;
                    let nx = lx / (rx * rx);
                    let ny = ly / (ry * ry);
                    let nz = lz / (rz * rz);
                    let nm = (nx * nx + ny * ny + nz * nz).sqrt();
                    let px = lx + cx;
                    let py = ly + cy;
                    let pz = lz + cz;
                    let (rpx, rpy, rpz) = rotate_xyz(px, py, pz, ax, ay, az);
                    let (rnx, rny, rnz) = rotate_xyz(nx / nm, ny / nm, nz / nm, ax, ay, az);
                    let lum = rnx * 0.3 + rny * 0.5 + rnz * 0.7;
                    if lum > -0.2 {
                        if let Some((xp, yp, depth)) =
                            project_3d(rpx, rpy, rpz, width, height, cam_dist)
                        {
                            if xp >= 0
                                && (xp as usize) < width
                                && yp >= 0
                                && (yp as usize) < height
                                && depth > zbuffer[yp as usize][xp as usize]
                            {
                                zbuffer[yp as usize][xp as usize] = depth;
                                let li = (lum.max(0.0) * (LUMINANCE.len() - 1) as f32) as usize;
                                output[yp as usize][xp as usize] =
                                    LUMINANCE[li.min(LUMINANCE.len() - 1)];
                            }
                        }
                    }
                    phi += step;
                }
                theta += step;
            }
        };

        let eye = |output: &mut Vec<Vec<u8>>,
                   zbuffer: &mut Vec<Vec<f32>>,
                   cx: f32,
                   cy: f32,
                   cz: f32,
                   radius: f32| {
            let step = 0.12;
            let mut theta: f32 = 0.0;
            while theta < std::f32::consts::TAU {
                let mut phi: f32 = -std::f32::consts::FRAC_PI_2;
                while phi < std::f32::consts::FRAC_PI_2 {
                    let cp = phi.cos();
                    let sp = phi.sin();
                    let ct = theta.cos();
                    let px = radius * cp * ct + cx;
                    let py = radius * sp + cy;
                    let pz = radius * cp * theta.sin() + cz;
                    let (rpx, rpy, rpz) = rotate_xyz(px, py, pz, ax, ay, az);
                    if let Some((xp, yp, depth)) =
                        project_3d(rpx, rpy, rpz, width, height, cam_dist)
                    {
                        if xp >= 0
                            && (xp as usize) < width
                            && yp >= 0
                            && (yp as usize) < height
                            && depth > zbuffer[yp as usize][xp as usize]
                        {
                            zbuffer[yp as usize][xp as usize] = depth;
                            output[yp as usize][xp as usize] = b'@';
                        }
                    }
                    phi += step;
                }
                theta += step;
            }
        };

        sample(output, zbuffer, 0.0, -0.2, 0.0, 1.3, 0.85, 0.85);
        sample(output, zbuffer, 0.0, 0.7, 0.7, 0.55, 0.5, 0.5);
        sample(output, zbuffer, -0.25, 1.85, 0.5, 0.13, 0.7, 0.08);
        sample(output, zbuffer, 0.25, 1.85, 0.5, 0.13, 0.7, 0.08);
        sample(output, zbuffer, 0.0, -0.1, -1.2, 0.25, 0.25, 0.25);
        sample(output, zbuffer, -0.45, -0.9, 0.5, 0.2, 0.15, 0.35);
        sample(output, zbuffer, 0.45, -0.9, 0.5, 0.2, 0.15, 0.35);
        sample(output, zbuffer, -0.55, -0.95, -0.4, 0.25, 0.2, 0.45);
        sample(output, zbuffer, 0.55, -0.95, -0.4, 0.25, 0.2, 0.45);
        eye(output, zbuffer, -0.2, 0.85, 1.1, 0.08);
        eye(output, zbuffer, 0.2, 0.85, 1.1, 0.08);
        eye(output, zbuffer, 0.0, 0.65, 1.15, 0.06);
    })
}

fn render_dna_helix(elapsed: f32, width: usize, height: usize) -> Vec<String> {
    with_render_buffers(width, height, |output, zbuffer| {
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
    })
}

fn animation_seed() -> u64 {
    static SEED: OnceLock<u64> = OnceLock::new();
    *SEED.get_or_init(|| {
        let mut hasher = DefaultHasher::new();
        std::time::SystemTime::now().hash(&mut hasher);
        std::process::id().hash(&mut hasher);
        hasher.finish()
    })
}

fn normalized_animation_name(name: &str) -> String {
    name.trim().to_lowercase().replace(['-', ' '], "_")
}

fn expand_disabled_animation_names<I>(names: I) -> HashSet<String>
where
    I: IntoIterator,
    I::Item: AsRef<str>,
{
    let mut disabled: HashSet<String> = names
        .into_iter()
        .map(|name| normalized_animation_name(name.as_ref()))
        .collect();

    if disabled.contains("mobius") {
        disabled.insert("knot".to_string());
    }
    if disabled.contains("knot") {
        disabled.insert("mobius".to_string());
    }
    if disabled.contains("three_rings") || disabled.contains("three-rings") {
        disabled.insert("three_rings".to_string());
        disabled.insert("gyroscope".to_string());
    }
    if disabled.contains("gyroscope") {
        disabled.insert("three_rings".to_string());
    }

    disabled
}

fn disabled_animation_names() -> HashSet<String> {
    expand_disabled_animation_names(crate::config::config().display.disabled_animations.iter())
}

fn choose_animation_variant_from_disabled<'a>(
    variants: &'a [&'a str],
    salt: u64,
    disabled: &HashSet<String>,
) -> &'a str {
    let available: Vec<&str> = variants
        .iter()
        .copied()
        .filter(|name| !disabled.contains(&normalized_animation_name(name)))
        .collect();

    let pool = if available.is_empty() {
        variants
    } else {
        &available
    };
    let idx = ((animation_seed() ^ salt) as usize) % pool.len();
    pool[idx]
}

fn choose_animation_variant<'a>(variants: &'a [&'a str], salt: u64) -> &'a str {
    let disabled = disabled_animation_names();
    choose_animation_variant_from_disabled(variants, salt, &disabled)
}

fn startup_animation_variant() -> &'static str {
    choose_animation_variant(STARTUP_VARIANTS, 0x0053_5441_5254_5550)
}

pub(super) fn build_startup_animation_lines(
    app: &dyn TuiState,
    term_width: u16,
) -> Vec<Line<'static>> {
    let elapsed = app.animation_elapsed();
    let status_idx =
        ((elapsed * STARTUP_ASCII_STATUS_FPS) as usize) % STARTUP_ASCII_STATUS_SPINNER.len();
    let status_spinner = STARTUP_ASCII_STATUS_SPINNER[status_idx];
    let progress = (elapsed / STARTUP_ANIMATION_WINDOW.as_secs_f32()).clamp(0.0, 1.0);
    let fade_in = (progress / 0.2).clamp(0.0, 1.0);
    let fade_out = ((1.0 - progress) / 0.25).clamp(0.0, 1.0);
    let envelope = fade_in.min(fade_out);
    let boost = (envelope * 120.0) as u8;
    let base = 80u8.saturating_add(boost);
    let art_color = rgb(base, base.saturating_add(16), base.saturating_add(30));
    let centered = app.centered_mode();
    let align = if centered {
        Alignment::Center
    } else {
        Alignment::Left
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
        Style::default().fg(dim_color()),
    )));
    lines.push(Line::from(Span::styled(
        "waiting for your first prompt",
        Style::default().fg(dim_color()),
    )));

    lines
        .into_iter()
        .map(|line| line.alignment(align))
        .collect()
}

struct IdleBuffers {
    hit: Vec<bool>,
    lum_map: Vec<f32>,
    z_buf: Vec<f32>,
    size: usize,
}

impl IdleBuffers {
    fn new() -> Self {
        Self {
            hit: Vec::new(),
            lum_map: Vec::new(),
            z_buf: Vec::new(),
            size: 0,
        }
    }

    fn resize_and_clear(&mut self, len: usize) {
        if self.size != len {
            self.hit.resize(len, false);
            self.lum_map.resize(len, 0.0);
            self.z_buf.resize(len, 0.0);
            self.size = len;
        }
        self.hit.fill(false);
        self.lum_map.fill(0.0);
        self.z_buf.fill(0.0);
    }
}

thread_local! {
    static IDLE_BUF: RefCell<IdleBuffers> = RefCell::new(IdleBuffers::new());
}

pub(super) fn draw_idle_animation(frame: &mut Frame, app: &dyn TuiState, area: Rect) {
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

    IDLE_BUF.with(|cell| {
        let mut bufs = cell.borrow_mut();
        bufs.resize_and_clear(sw * sh);
        let bufs = &mut *bufs;

        let variant = idle_animation_variant();
        match variant {
            "donut" => sample_donut(
                elapsed,
                sw,
                sh,
                &mut bufs.hit,
                &mut bufs.lum_map,
                &mut bufs.z_buf,
            ),
            "knot" => sample_knot(
                elapsed,
                sw,
                sh,
                &mut bufs.hit,
                &mut bufs.lum_map,
                &mut bufs.z_buf,
            ),
            "black_hole" => sample_black_hole(
                elapsed,
                sw,
                sh,
                &mut bufs.hit,
                &mut bufs.lum_map,
                &mut bufs.z_buf,
            ),
            _ => sample_gyroscope(
                elapsed,
                sw,
                sh,
                &mut bufs.hit,
                &mut bufs.lum_map,
                &mut bufs.z_buf,
            ),
        }

        let hit = &bufs.hit;
        let lum_map = &bufs.lum_map;

        let time_hue = elapsed * 40.0;
        let centered = app.centered_mode();
        let align = if centered {
            Alignment::Center
        } else {
            Alignment::Left
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
                            Span::styled(String::from(ch), Style::default().fg(rgb(r, g, b)))
                        }
                    })
                    .collect();
                Line::from(spans).alignment(align)
            })
            .collect();

        frame.render_widget(Paragraph::new(lines), area);
    });
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

            phi += 0.014;
        }
        theta += 0.04;
    }
}

fn idle_animation_variant() -> &'static str {
    choose_animation_variant(IDLE_VARIANTS, 0x4944_4c45_414e_494d)
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
                let up = if ttx.abs() < 0.9 {
                    (1.0f32, 0.0, 0.0)
                } else {
                    (0.0, 1.0, 0.0)
                };
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

fn sample_black_hole(
    elapsed: f32,
    sw: usize,
    sh: usize,
    hit: &mut [bool],
    lum_map: &mut [f32],
    z_buf: &mut [f32],
) {
    let cx = sw as f32 / 2.0;
    let cy = sh as f32 / 2.0;
    let aspect = 0.5f32;
    let disk_half_len = sw as f32 * 0.35;
    let disk_half_thickness = (sh as f32 * 0.052).max(1.0);
    let horizon_r = (sh as f32).min(sw as f32 / 3.2) * 0.16;
    let halo_r = horizon_r * 1.8;
    let shimmer = elapsed * 2.35;

    for y in 0..sh {
        for x in 0..sw {
            let dx = x as f32 - cx;
            let dy = (y as f32 - cy) / aspect;
            let r = (dx * dx + dy * dy).sqrt();
            let idx = y * sw + x;

            let abs_x = dx.abs();
            let abs_y = dy.abs();

            let disk_falloff_x = (1.0 - abs_x / disk_half_len).clamp(0.0, 1.0);
            let disk_core = (1.0 - abs_y / disk_half_thickness).clamp(0.0, 1.0);
            let disk_glow =
                (1.0 - abs_y / (disk_half_thickness * 3.8 + 1.0)).clamp(0.0, 1.0) * 0.42;
            let lens_band = (1.0 - ((abs_y - horizon_r * 0.72).abs() / (horizon_r * 0.55 + 0.1)))
                .clamp(0.0, 1.0)
                * (1.0 - abs_x / (halo_r * 1.5 + 1.0)).clamp(0.0, 1.0);
            let halo =
                (1.0 - ((r - halo_r).abs() / (horizon_r * 0.95 + 0.1))).clamp(0.0, 1.0) * 0.38;

            let streak_phase = shimmer + dx * 0.33;
            let streaks = ((streak_phase.sin() * 0.5 + 0.5) * 0.55
                + ((streak_phase * 0.47 + 1.7).sin() * 0.5 + 0.5) * 0.45)
                * disk_falloff_x;
            let relativistic_beam = (1.0
                - ((dx - disk_half_len * 0.34).abs() / (disk_half_len * 0.52 + 0.1)))
                .clamp(0.0, 1.0)
                * 0.32;

            let mut brightness = disk_core * (0.55 + 0.45 * streaks) * disk_falloff_x
                + disk_glow * disk_falloff_x
                + lens_band * 0.62
                + halo * 0.28
                + relativistic_beam * disk_core;

            if r <= horizon_r {
                brightness = 0.0;
            }

            if abs_x <= horizon_r * 0.95 && abs_y <= disk_half_thickness * 1.2 {
                brightness *= (abs_x / (horizon_r * 0.95 + 0.1)).clamp(0.0, 1.0);
            }

            brightness = brightness.clamp(0.0, 1.0);
            if brightness > 0.06 {
                hit[idx] = true;
                lum_map[idx] = brightness * 2.0 - 1.0;
                z_buf[idx] = brightness + lens_band * 0.2;
            } else {
                hit[idx] = false;
                lum_map[idx] = -1.0;
                z_buf[idx] = 0.0;
            }
        }
    }
}

fn sample_gyroscope(
    elapsed: f32,
    sw: usize,
    sh: usize,
    hit: &mut [bool],
    lum_map: &mut [f32],
    z_buf: &mut [f32],
) {
    let rot_x = elapsed * 0.45 + (elapsed * 0.7).sin() * 0.25;
    let rot_y = elapsed * 0.70;
    let rot_z = elapsed * 0.28 + (elapsed * 0.5).cos() * 0.18;
    let cam_dist = 8.5f32;
    let aspect = 0.5;
    let scale_base = (sw as f32).min(sh as f32 / aspect) * 0.30;

    let rings = [(0u8, 2.0f32, 0.17f32), (1, 1.45, 0.15), (2, 0.95, 0.13)];

    for (ring_idx, &(axis, major_r, tube_r)) in rings.iter().enumerate() {
        let phase = elapsed * (0.35 + ring_idx as f32 * 0.15);
        let mut u: f32 = 0.0;
        while u < std::f32::consts::TAU {
            let uu = u + phase;
            let cu = uu.cos();
            let su = uu.sin();

            let mut v: f32 = 0.0;
            while v < std::f32::consts::TAU {
                let cv = v.cos();
                let sv = v.sin();
                let ring_r = major_r + tube_r * cv;

                let (x, y, z, nx, ny, nz) = match axis {
                    0 => {
                        let x = tube_r * sv;
                        let y = ring_r * cu;
                        let z = ring_r * su;
                        let nx = sv;
                        let ny = cv * cu;
                        let nz = cv * su;
                        (x, y, z, nx, ny, nz)
                    }
                    1 => {
                        let x = ring_r * cu;
                        let y = tube_r * sv;
                        let z = ring_r * su;
                        let nx = cv * cu;
                        let ny = sv;
                        let nz = cv * su;
                        (x, y, z, nx, ny, nz)
                    }
                    _ => {
                        let x = ring_r * cu;
                        let y = ring_r * su;
                        let z = tube_r * sv;
                        let nx = cv * cu;
                        let ny = cv * su;
                        let nz = sv;
                        (x, y, z, nx, ny, nz)
                    }
                };

                let (rx, ry, rz) = rotate_xyz(x, y, z, rot_x, rot_y, rot_z);
                let d = cam_dist + rz;
                if d < 0.1 {
                    v += 0.24;
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
                        let (rnx, rny, rnz) = rotate_xyz(nx, ny, nz, rot_x, rot_y, rot_z);
                        let lum = (rnx * 0.45 + rny * 0.35 + rnz * 0.20 + 0.20).clamp(-1.0, 1.0);
                        lum_map[idx] = lum;
                        hit[idx] = true;
                    }
                }

                v += 0.24;
            }

            u += 0.035;
        }
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
        t += 0.016;
    }
}

fn shape_char_3x3(pattern: u16, brightness: f32) -> char {
    if pattern == 0 {
        return ' ';
    }

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

    let bl = if brightness > 0.65 {
        2u8
    } else if brightness > 0.35 {
        1u8
    } else {
        0u8
    };

    if count >= 8 {
        return match bl {
            2 => '@',
            1 => '#',
            _ => '%',
        };
    }
    if count >= 7 {
        return match bl {
            2 => '#',
            1 => '%',
            _ => '*',
        };
    }

    if top_l && mid_c && bot_r && !top_r && !bot_l {
        return match bl {
            2 => '\\',
            1 => '\\',
            _ => '.',
        };
    }
    if top_r && mid_c && bot_l && !top_l && !bot_r {
        return match bl {
            2 => '/',
            1 => '/',
            _ => '.',
        };
    }

    if mid >= 2 && top <= 1 && bot <= 1 && mid > top && mid > bot {
        return match bl {
            2 => '=',
            1 => '-',
            _ => '~',
        };
    }
    if top >= 2 && mid <= 1 && bot == 0 {
        return match bl {
            2 => '=',
            1 => '-',
            _ => '~',
        };
    }
    if bot >= 2 && mid <= 1 && top == 0 {
        return match bl {
            2 => '=',
            1 => '_',
            _ => '.',
        };
    }

    if center >= 2 && left <= 1 && right <= 1 && center > left && center > right {
        return match bl {
            2 => '|',
            1 => '|',
            _ => ':',
        };
    }
    if left >= 2 && center <= 1 && right == 0 {
        return match bl {
            2 => '|',
            1 => '|',
            _ => ':',
        };
    }
    if right >= 2 && center <= 1 && left == 0 {
        return match bl {
            2 => '|',
            1 => '|',
            _ => ':',
        };
    }

    if top >= 2 && bot == 0 {
        return match bl {
            2 => '"',
            1 => '^',
            _ => '\'',
        };
    }
    if bot >= 2 && top == 0 {
        return match bl {
            2 => ',',
            1 => '.',
            _ => '.',
        };
    }

    if left >= 2 && right == 0 {
        return match bl {
            2 => '(',
            1 => '(',
            _ => ':',
        };
    }
    if right >= 2 && left == 0 {
        return match bl {
            2 => ')',
            1 => ')',
            _ => ':',
        };
    }

    if count >= 6 {
        return match bl {
            2 => '%',
            1 => '*',
            _ => '+',
        };
    }
    if count >= 5 {
        return match bl {
            2 => '*',
            1 => '+',
            _ => ':',
        };
    }

    if mid_c && count <= 3 {
        return match bl {
            2 => 'o',
            1 => '*',
            _ => '.',
        };
    }

    if top_r && bot_l && count <= 3 {
        return match bl {
            2 => '/',
            1 => '/',
            _ => '.',
        };
    }
    if top_l && bot_r && count <= 3 {
        return match bl {
            2 => '\\',
            1 => '\\',
            _ => '.',
        };
    }

    if count == 1 {
        if bot_c || bot_l || bot_r {
            return match bl {
                2 => '.',
                _ => '.',
            };
        }
        if top_c || top_l || top_r {
            return match bl {
                2 => '\'',
                1 => '\'',
                _ => '.',
            };
        }
        return match bl {
            2 => ':',
            1 => '.',
            _ => '.',
        };
    }

    if count <= 3 {
        return match bl {
            2 => ':',
            1 => ':',
            _ => '.',
        };
    }

    match bl {
        2 => '+',
        1 => ':',
        _ => '.',
    }
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

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn startup_variants_exclude_mobius_and_black_hole() {
        assert!(!STARTUP_VARIANTS.contains(&"mobius"));
        assert!(!STARTUP_VARIANTS.contains(&"black_hole"));
    }

    #[test]
    fn idle_variants_exclude_black_hole() {
        assert!(!IDLE_VARIANTS.contains(&"black_hole"));
    }

    #[test]
    fn disabling_mobius_also_disables_knot_alias() {
        let disabled = expand_disabled_animation_names(["mobius"]);
        assert!(disabled.contains("mobius"));
        assert!(disabled.contains("knot"));
    }

    #[test]
    fn disabling_three_rings_also_disables_gyroscope_alias() {
        let disabled = expand_disabled_animation_names(["three_rings"]);
        assert!(disabled.contains("three_rings"));
        assert!(disabled.contains("gyroscope"));
    }

    #[test]
    fn variant_selection_avoids_disabled_entries_when_possible() {
        let disabled = expand_disabled_animation_names(["donut", "three_rings"]);
        let variant = choose_animation_variant_from_disabled(IDLE_VARIANTS, 7, &disabled);
        assert_ne!(variant, "donut");
        assert_ne!(variant, "three_rings");
    }
}
