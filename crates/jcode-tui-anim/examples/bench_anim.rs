// Throughput benchmark: old "trig every iteration" samplers vs the new
// precomputed-angle-table samplers. Run with:
//   cargo run --profile selfdev --example bench_anim -p jcode-tui-anim
// The jcode-tui-anim lib is pinned to opt-level=3 in every profile, so this
// measures the table speedup at the optimization level the TUI actually uses.

use std::time::Instant;

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

// Original donut: cos/sin recomputed every theta/phi iteration.
fn old_donut(e: f32, sw: usize, sh: usize, hit: &mut [bool], lum: &mut [f32], z: &mut [f32]) {
    let a = e * 1.0;
    let b = e * 0.5;
    let (ca, sa, cb, sb) = (a.cos(), a.sin(), b.cos(), b.sin());
    let aspect = 0.5;
    let (r1, r2, k2) = (1.0f32, 2.0f32, 5.0f32);
    let k1 = (sw as f32).min(sh as f32 / aspect) * k2 * 0.35 / (r1 + r2);
    let mut theta = 0.0f32;
    while theta < std::f32::consts::TAU {
        let (ct, st) = (theta.cos(), theta.sin());
        let mut phi = 0.0f32;
        while phi < std::f32::consts::TAU {
            let (cp, sp) = (phi.cos(), phi.sin());
            let cx = r2 + r1 * ct;
            let cy = r1 * st;
            let x = cx * (cb * cp + sa * sb * sp) - cy * ca * sb;
            let y = cx * (sb * cp - sa * cb * sp) + cy * ca * cb;
            let zz = k2 + ca * cx * sp + cy * sa;
            let ooz = 1.0 / zz;
            let xp = (sw as f32 / 2.0 + k1 * ooz * x) as isize;
            let yp = (sh as f32 / 2.0 - k1 * ooz * y * aspect) as isize;
            let l = cp * ct * sb - ca * ct * sp - sa * st + cb * (ca * st - ct * sa * sp);
            if xp >= 0 && (xp as usize) < sw && yp >= 0 && (yp as usize) < sh {
                let i = yp as usize * sw + xp as usize;
                if ooz > z[i] {
                    z[i] = ooz;
                    lum[i] = l;
                    hit[i] = true;
                }
            }
            phi += 0.014;
        }
        theta += 0.04;
    }
}

fn old_orbit(e: f32, sw: usize, sh: usize, hit: &mut [bool], lum: &mut [f32], z: &mut [f32]) {
    let rot_x = e * 0.32 + (e * 0.45).sin() * 0.30;
    let rot_y = e * 0.56;
    let rot_z = e * 0.22 + (e * 0.38).cos() * 0.22;
    let cam = 8.8f32;
    let aspect = 0.5;
    let sb = (sw as f32).min(sh as f32 / aspect) * 0.29;
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
    for (ri, &(axis, major, tube, orbit, po)) in rings.iter().enumerate() {
        let phase = e * (0.30 + ri as f32 * 0.10) + po;
        let cxm = orbit * phase.cos() * 0.55;
        let cym = orbit * (phase * 0.7).sin() * 0.30;
        let czm = orbit * phase.sin() * 0.50;
        let pulse = 1.0 + 0.08 * (e * 1.1 + po).sin();
        let mut u = 0.0f32;
        while u < std::f32::consts::TAU {
            let uu = u + phase * 0.7;
            let (cu, su) = (uu.cos(), uu.sin());
            let mut v = 0.0f32;
            while v < std::f32::consts::TAU {
                let (cv, sv) = (v.cos(), v.sin());
                let rr = major * pulse + tube * cv;
                let (x, y, zz, nx, ny, nz) = match axis {
                    0 => (
                        cxm + tube * sv,
                        cym + rr * cu,
                        czm + rr * su,
                        sv,
                        cv * cu,
                        cv * su,
                    ),
                    1 => (
                        cxm + rr * cu,
                        cym + tube * sv,
                        czm + rr * su,
                        cv * cu,
                        sv,
                        cv * su,
                    ),
                    _ => (
                        cxm + rr * cu,
                        cym + rr * su,
                        czm + tube * sv,
                        cv * cu,
                        cv * su,
                        sv,
                    ),
                };
                let (rx, ry, rz) = rotate_xyz(x, y, zz, rot_x, rot_y, rot_z);
                let d = cam + rz;
                if d < 0.1 {
                    v += 0.22;
                    continue;
                }
                let proj = cam / d;
                let xp = (sw as f32 / 2.0 + rx * proj * sb) as isize;
                let yp = (sh as f32 / 2.0 - ry * proj * sb * aspect) as isize;
                let depth = 1.0 / d;
                if xp >= 0 && (xp as usize) < sw && yp >= 0 && (yp as usize) < sh {
                    let i = yp as usize * sw + xp as usize;
                    if depth > z[i] {
                        z[i] = depth;
                        let (rnx, rny, rnz) = rotate_xyz(nx, ny, nz, rot_x, rot_y, rot_z);
                        let glow = (phase.cos() * 0.10 + ri as f32 * 0.03).clamp(-0.2, 0.2);
                        lum[i] =
                            (rnx * 0.42 + rny * 0.33 + rnz * 0.25 + 0.18 + glow).clamp(-1.0, 1.0);
                        hit[i] = true;
                    }
                }
                v += 0.22;
            }
            u += 0.032;
        }
    }
}

type S = fn(f32, usize, usize, &mut [bool], &mut [f32], &mut [f32]);

fn time(name: &str, f: S, frames: usize, sw: usize, sh: usize) -> f64 {
    let n = sw * sh;
    let (mut h, mut l, mut z) = (vec![false; n], vec![0.0f32; n], vec![0.0f32; n]);
    // warmup
    for i in 0..50 {
        f(i as f32 * 0.05, sw, sh, &mut h, &mut l, &mut z);
        h.fill(false);
        l.fill(0.0);
        z.fill(0.0);
    }
    let t = Instant::now();
    let mut sink = 0u64;
    for i in 0..frames {
        h.fill(false);
        l.fill(0.0);
        z.fill(0.0);
        f(i as f32 * 0.016, sw, sh, &mut h, &mut l, &mut z);
        sink = sink.wrapping_add(h.iter().filter(|&&b| b).count() as u64);
    }
    let dt = t.elapsed().as_secs_f64();
    let per = dt / frames as f64 * 1e6;
    std::hint::black_box(sink);
    println!(
        "  {name:<22} {per:8.2} us/frame   ({:.0} frames/s)",
        1.0 / (dt / frames as f64)
    );
    per
}

fn main() {
    // Typical idle viewport: ~120 cols x ~40 rows, 3x subpixels -> 360 x 120.
    let (sw, sh) = (360usize, 120usize);
    let frames = 2000;
    println!("donut  @ {sw}x{sh}, {frames} frames:");
    let od = time("old (trig/iter)", old_donut, frames, sw, sh);
    let nd = time(
        "new (angle table)",
        jcode_tui_anim::sample_donut,
        frames,
        sw,
        sh,
    );
    println!("  -> {:.2}x faster\n", od / nd);
    println!("orbit_rings @ {sw}x{sh}, {frames} frames:");
    let oo = time("old (trig/iter)", old_orbit, frames, sw, sh);
    let no = time(
        "new (angle table)",
        jcode_tui_anim::sample_orbit_rings,
        frames,
        sw,
        sh,
    );
    println!("  -> {:.2}x faster", oo / no);
}
