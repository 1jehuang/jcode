use crate::tui::{TuiState, color_support::rgb};
use ratatui::{prelude::*, widgets::Paragraph};
use std::cell::RefCell;
use std::collections::{HashSet, hash_map::DefaultHasher};
use std::hash::{Hash, Hasher};
use std::sync::OnceLock;

const IDLE_VARIANTS: &[&str] = &["donut", "orbit_rings"];

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
            "orbit_rings" => sample_orbit_rings(
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
    let scale_base = (sw as f32).min(sh as f32 / aspect) * 0.20;

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

fn sample_orbit_rings(
    elapsed: f32,
    sw: usize,
    sh: usize,
    hit: &mut [bool],
    lum_map: &mut [f32],
    z_buf: &mut [f32],
) {
    let rot_x = elapsed * 0.32 + (elapsed * 0.45).sin() * 0.30;
    let rot_y = elapsed * 0.56;
    let rot_z = elapsed * 0.22 + (elapsed * 0.38).cos() * 0.22;
    let cam_dist = 8.8f32;
    let aspect = 0.5;
    let scale_base = (sw as f32).min(sh as f32 / aspect) * 0.29;

    let rings = [
        (0u8, 2.35f32, 0.10f32, 0.32f32, 0.0f32),
        (1u8, 1.78f32, 0.11f32, 0.26f32, std::f32::consts::TAU / 3.0),
        (
            2u8,
            1.22f32,
            0.09f32,
            0.20f32,
            2.0 * std::f32::consts::TAU / 3.0,
        ),
        (1u8, 2.70f32, 0.08f32, 0.36f32, std::f32::consts::TAU / 6.0),
    ];

    for (ring_idx, &(axis, major_r, tube_r, orbit_r, phase_offset)) in rings.iter().enumerate() {
        let phase = elapsed * (0.30 + ring_idx as f32 * 0.10) + phase_offset;
        let center_x = orbit_r * phase.cos() * 0.55;
        let center_y = orbit_r * (phase * 0.7).sin() * 0.30;
        let center_z = orbit_r * phase.sin() * 0.50;
        let radius_pulse = 1.0 + 0.08 * (elapsed * 1.1 + phase_offset).sin();

        let mut u: f32 = 0.0;
        while u < std::f32::consts::TAU {
            let uu = u + phase * 0.7;
            let cu = uu.cos();
            let su = uu.sin();

            let mut v: f32 = 0.0;
            while v < std::f32::consts::TAU {
                let cv = v.cos();
                let sv = v.sin();
                let ring_r = major_r * radius_pulse + tube_r * cv;

                let (x, y, z, nx, ny, nz) = match axis {
                    0 => {
                        let x = center_x + tube_r * sv;
                        let y = center_y + ring_r * cu;
                        let z = center_z + ring_r * su;
                        let nx = sv;
                        let ny = cv * cu;
                        let nz = cv * su;
                        (x, y, z, nx, ny, nz)
                    }
                    1 => {
                        let x = center_x + ring_r * cu;
                        let y = center_y + tube_r * sv;
                        let z = center_z + ring_r * su;
                        let nx = cv * cu;
                        let ny = sv;
                        let nz = cv * su;
                        (x, y, z, nx, ny, nz)
                    }
                    _ => {
                        let x = center_x + ring_r * cu;
                        let y = center_y + ring_r * su;
                        let z = center_z + tube_r * sv;
                        let nx = cv * cu;
                        let ny = cv * su;
                        let nz = sv;
                        (x, y, z, nx, ny, nz)
                    }
                };

                let (rx, ry, rz) = rotate_xyz(x, y, z, rot_x, rot_y, rot_z);
                let d = cam_dist + rz;
                if d < 0.1 {
                    v += 0.22;
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
                        let glow = (phase.cos() * 0.10 + ring_idx as f32 * 0.03).clamp(-0.2, 0.2);
                        let lum =
                            (rnx * 0.42 + rny * 0.33 + rnz * 0.25 + 0.18 + glow).clamp(-1.0, 1.0);
                        lum_map[idx] = lum;
                        hit[idx] = true;
                    }
                }

                v += 0.22;
            }

            u += 0.032;
        }
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

    type IdleSampler = fn(f32, usize, usize, &mut [bool], &mut [f32], &mut [f32]);

    fn hit_bounds(hit: &[bool], sw: usize, sh: usize) -> Option<(usize, usize, usize, usize)> {
        let mut min_x = sw;
        let mut max_x = 0usize;
        let mut min_y = sh;
        let mut max_y = 0usize;
        let mut any = false;

        for y in 0..sh {
            for x in 0..sw {
                if hit[y * sw + x] {
                    any = true;
                    min_x = min_x.min(x);
                    max_x = max_x.max(x);
                    min_y = min_y.min(y);
                    max_y = max_y.max(y);
                }
            }
        }

        any.then_some((min_x, max_x, min_y, max_y))
    }

    fn assert_idle_sampler_avoids_heavy_border_clipping(name: &str, sampler: IdleSampler) {
        let sw = 120;
        let sh = 60;

        for &elapsed in &[0.0f32, 0.8, 1.6, 2.4] {
            let mut hit = vec![false; sw * sh];
            let mut lum_map = vec![0.0; sw * sh];
            let mut z_buf = vec![0.0; sw * sh];
            sampler(elapsed, sw, sh, &mut hit, &mut lum_map, &mut z_buf);

            let (_min_x, _max_x, _min_y, _max_y) =
                hit_bounds(&hit, sw, sh).unwrap_or_else(|| panic!("{name} should draw pixels"));
            let lit_pixels = hit.iter().filter(|&&value| value).count();
            let border_pixels = hit
                .iter()
                .enumerate()
                .filter(|(idx, value)| {
                    if !**value {
                        return false;
                    }
                    let x = idx % sw;
                    let y = idx / sw;
                    x == 0 || x + 1 == sw || y == 0 || y + 1 == sh
                })
                .count();

            assert!(
                border_pixels * 12 < lit_pixels.max(1),
                "{name} at t={elapsed} is too clipped at viewport border: border_pixels={border_pixels}, lit_pixels={lit_pixels}"
            );
        }
    }

    fn assert_idle_sampler_stays_off_border_on_small_viewports(name: &str, sampler: IdleSampler) {
        let sizes = [(90usize, 36usize), (108, 42), (120, 48)];

        for &(sw, sh) in &sizes {
            for &elapsed in &[0.0f32, 0.8, 1.6, 2.4] {
                let mut hit = vec![false; sw * sh];
                let mut lum_map = vec![0.0; sw * sh];
                let mut z_buf = vec![0.0; sw * sh];
                sampler(elapsed, sw, sh, &mut hit, &mut lum_map, &mut z_buf);

                let (min_x, max_x, min_y, max_y) =
                    hit_bounds(&hit, sw, sh).unwrap_or_else(|| panic!("{name} should draw pixels"));

                assert!(
                    min_x > 0 && max_x + 1 < sw && min_y > 0 && max_y + 1 < sh,
                    "{name} at t={elapsed} touches border on small viewport {sw}x{sh}: bounds=({min_x}..={max_x}, {min_y}..={max_y})"
                );
            }
        }
    }

    #[test]
    fn idle_variants_exclude_retired_variants() {
        assert!(!IDLE_VARIANTS.contains(&"knot"));
        assert!(!IDLE_VARIANTS.contains(&"black_hole"));
    }

    #[test]
    fn idle_variants_keep_normal_donut_and_exclude_cube() {
        assert!(IDLE_VARIANTS.contains(&"donut"));
        assert!(!IDLE_VARIANTS.contains(&"pulse_donut"));
        assert!(IDLE_VARIANTS.contains(&"orbit_rings"));
        assert!(!IDLE_VARIANTS.contains(&"three_rings"));
        assert!(!IDLE_VARIANTS.contains(&"cube"));
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

    #[test]
    fn idle_animation_samplers_avoid_heavy_border_clipping() {
        assert_idle_sampler_avoids_heavy_border_clipping("donut", sample_donut);
        assert_idle_sampler_avoids_heavy_border_clipping("three_rings", sample_gyroscope);
        assert_idle_sampler_avoids_heavy_border_clipping("orbit_rings", sample_orbit_rings);
    }

    #[test]
    fn three_rings_fit_small_viewports_without_touching_border() {
        assert_idle_sampler_stays_off_border_on_small_viewports("three_rings", sample_gyroscope);
    }
}
