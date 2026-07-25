//! The website's hero donut, ported to the desktop.
//!
//! Same maths as `public/donut.js` on the jcode site: instead of splatting
//! parametric torus points into a grid (which scintillates as samples snap
//! between cells and z-fight), every cell of a luminance grid raymarches the
//! torus SDF for an exact, deterministic shade. There is zero temporal state,
//! so a frame is a pure function of `(time, spin)` and captures are
//! reproducible. The field is then screened as a classic 45-degree
//! rotated-dot halftone, which is what gives the newspaper look.
//!
//! Geometry, camera, and light match classic `donut.c` exactly, so the
//! desktop, the website, and the TUI animation are all the same object.

/// Torus minor radius, major radius, and eye distance, as in `donut.c`.
const R1: f32 = 1.0;
const R2: f32 = 2.0;
const K2: f32 = 5.0;
/// Bounding sphere around the torus, used to gate empty rays.
const BOUND: f32 = R1 + R2 + 0.02;
const EPS: f32 = 5e-4;
const MAX_STEPS: usize = 48;
/// Light direction (0,1,-1)/sqrt(2), as in `donut.c`.
const LY: f32 = std::f32::consts::FRAC_1_SQRT_2;
const LZ: f32 = -std::f32::consts::FRAC_1_SQRT_2;

/// A square luminance field, raymarched per cell.
pub struct Donut {
    grid: usize,
    lum: Vec<f32>,
}

impl Donut {
    pub fn new(grid: usize) -> Self {
        let grid = grid.max(8);
        Self {
            grid,
            lum: vec![0.0; grid * grid],
        }
    }

    pub fn grid(&self) -> usize {
        self.grid
    }

    /// Fill the luminance field for `t` seconds of animation plus `spin`
    /// radians of user drag. Drag only affects the yaw, so dragging never
    /// changes the flattering tilt.
    pub fn render(&mut self, t: f32, spin: f32) {
        let grid = self.grid;
        let k1 = (grid as f32 * K2 * 3.0) / (8.0 * (R1 + R2));
        let cc = K2 * K2 - BOUND * BOUND;

        let a = 1.0 + (t * 0.4).sin() * 0.25;
        let b = t * 0.7 + spin;
        let (sa, ca) = a.sin_cos();
        let (sb, cb) = b.sin_cos();

        // Rotation local<-world: the transpose of donut.c's world<-local
        // matrix, so orientation and lighting match the original exactly.
        let (m00, m01) = (cb, sb);
        let (m10, m11, m12) = (sa * sb, -sa * cb, ca);
        let (m20, m21, m22) = (-ca * sb, ca * cb, sa);

        // Eye at the world origin, torus centred at (0,0,K2): transform the
        // eye into torus-local space once per frame.
        let oy = -ca * K2;
        let oz = -sa * K2;

        let half = grid as f32 / 2.0;
        for yc in 0..grid {
            let pyw = -(yc as f32 + 0.5 - half) / k1;
            for xc in 0..grid {
                let pxw = (xc as f32 + 0.5 - half) / k1;
                let inv = 1.0 / (pxw * pxw + pyw * pyw + 1.0).sqrt();
                let (dwx, dwy, dwz) = (pxw * inv, pyw * inv, inv);

                let dx = m00 * dwx + m01 * dwy;
                let dy = m10 * dwx + m11 * dwy + m12 * dwz;
                let dz = m20 * dwx + m21 * dwy + m22 * dwz;

                self.lum[xc + yc * grid] =
                    trace(dx, dy, dz, oy, oz, cc, k1, m01, m11, m12, m21, m22);
            }
        }
    }

    /// Bilinear sample of the field at grid coordinates, so each halftone dot
    /// integrates its neighbourhood instead of point-sampling one cell.
    pub fn sample(&self, gx: f32, gy: f32) -> f32 {
        let grid = self.grid;
        if gx < 0.0 || gy < 0.0 || gx > grid as f32 - 2.0 || gy > grid as f32 - 2.0 {
            return 0.0;
        }
        let (x0, y0) = (gx.floor() as usize, gy.floor() as usize);
        let (fx, fy) = (gx - x0 as f32, gy - y0 as f32);
        let a = self.lum[x0 + y0 * grid];
        let b = self.lum[x0 + 1 + y0 * grid];
        let c = self.lum[x0 + (y0 + 1) * grid];
        let d = self.lum[x0 + 1 + (y0 + 1) * grid];
        a * (1.0 - fx) * (1.0 - fy) + b * fx * (1.0 - fy) + c * (1.0 - fx) * fy + d * fx * fy
    }
}

/// Signed distance to the torus in local space.
#[inline]
fn sdf(px: f32, py: f32, pz: f32) -> f32 {
    let q = (px * px + py * py).sqrt() - R2;
    (q * q + pz * pz).sqrt() - R1
}

/// Sphere-trace one ray and return its lit luminance in `0..=1`.
#[allow(clippy::too_many_arguments)]
fn trace(
    dx: f32,
    dy: f32,
    dz: f32,
    oy: f32,
    oz: f32,
    cc: f32,
    k1: f32,
    m01: f32,
    m11: f32,
    m12: f32,
    m21: f32,
    m22: f32,
) -> f32 {
    // Analytic bounding-sphere gate: skip empty cells instantly and bound the
    // march to [entry, exit].
    let b = oy * dy + oz * dz;
    let disc = b * b - cc;
    if disc <= 0.0 {
        return 0.0;
    }
    let sq = disc.sqrt();
    let s_exit = -b + sq;
    if s_exit <= 0.0 {
        return 0.0;
    }
    let mut s = (-b - sq).max(0.0);

    // March, tracking the closest approach for silhouette coverage AA and
    // keeping a bracket (prev, min, after) around the minimum for refinement.
    let mut min_d = f32::MAX;
    let mut s_min = s;
    let mut hit = false;
    let mut s_prev = s;
    let mut s_after = -1.0f32;
    let mut last_s = s;
    for _ in 0..MAX_STEPS {
        let d = sdf(dx * s, oy + dy * s, oz + dz * s);
        if d < min_d {
            min_d = d;
            s_prev = last_s;
            s_min = s;
            s_after = -1.0;
        } else if s_after < 0.0 {
            s_after = s;
        }
        if d < EPS {
            hit = true;
            break;
        }
        last_s = s;
        s += d;
        if s > s_exit {
            break;
        }
    }

    // For near-miss rays the sampled `min_d` is a noisy overestimate of the
    // true closest approach (march points land arbitrarily far from it, and
    // differently each frame, which flickers the AA band). Ternary-search the
    // bracket to converge it.
    if !hit && min_d < 0.5 {
        let mut lo = s_prev;
        let mut hi = if s_after > 0.0 {
            s_after
        } else {
            s_min + (s_min - s_prev)
        };
        for _ in 0..12 {
            let m1 = lo + (hi - lo) / 3.0;
            let m2 = hi - (hi - lo) / 3.0;
            let d1 = sdf(dx * m1, oy + dy * m1, oz + dz * m1);
            let d2 = sdf(dx * m2, oy + dy * m2, oz + dz * m2);
            if d1 < d2 {
                hi = m2;
                if d1 < min_d {
                    min_d = d1;
                    s_min = m1;
                }
            } else {
                lo = m1;
                if d2 < min_d {
                    min_d = d2;
                    s_min = m2;
                }
            }
        }
    }

    let sp = if hit { s } else { s_min };
    // AA band: ~1.5 halftone cells, in local units at depth `sp`. Wider than
    // strictly needed, on purpose: the gradual coverage fade lets edge dots
    // taper off, which suits the soft halftone look.
    let aa = (1.5 * sp) / k1;
    if !hit && min_d >= aa {
        return 0.0;
    }

    // Exact torus normal at (or nearest to) the surface point.
    let (px, py, pz) = (dx * sp, oy + dy * sp, oz + dz * sp);
    let len = (px * px + py * py).sqrt();
    let k = R2 / len;
    let (mut nx, mut ny, mut nz) = (px * (1.0 - k), py * (1.0 - k), pz);
    let nlen = (nx * nx + ny * ny + nz * nz).sqrt();
    let nlen = if nlen == 0.0 { 1.0 } else { nlen };
    nx /= nlen;
    ny /= nlen;
    nz /= nlen;

    // Rotate the normal back to world space and dot it with the light.
    let nwy = m01 * nx + m11 * ny + m21 * nz;
    let nwz = m12 * ny + m22 * nz;
    let mut v = nwy * LY + nwz * LZ;
    if v <= 0.0 {
        return 0.0;
    }
    if !hit {
        v *= 1.0 - min_d / aa; // silhouette coverage fade
    }
    v
}

/// Interaction state: drag adds spin, which decays like the website's.
#[derive(Clone, Copy, Debug, PartialEq)]
pub struct Spin {
    /// Accumulated yaw offset from dragging, in radians.
    pub offset: f32,
    /// Current drag velocity, decayed each frame.
    pub velocity: f32,
    /// Animation clock, in seconds. Advanced by the event loop so captures can
    /// pin it and stay deterministic.
    pub time: f32,
    /// True while the pointer is held on the donut.
    pub dragging: bool,
    /// Last pointer x, in logical units, for drag deltas.
    pub last_x: f64,
}

impl Default for Spin {
    fn default() -> Self {
        Self {
            offset: 0.0,
            velocity: 0.0,
            time: 0.0,
            dragging: false,
            last_x: 0.0,
        }
    }
}

/// Per-frame momentum decay, matching the website's feel.
const DECAY: f32 = 0.92;
/// Radians of spin per logical pixel of drag.
const DRAG_GAIN: f32 = 0.008;

impl Spin {
    pub fn press(&mut self, x: f64) {
        self.dragging = true;
        self.last_x = x;
    }

    pub fn drag_to(&mut self, x: f64) {
        if !self.dragging {
            return;
        }
        self.velocity += (x - self.last_x) as f32 * DRAG_GAIN;
        self.last_x = x;
    }

    pub fn release(&mut self) {
        self.dragging = false;
    }

    /// Advance one frame: apply momentum and the animation clock.
    pub fn advance(&mut self, dt: f32) {
        self.offset += self.velocity;
        self.velocity *= DECAY;
        if self.velocity.abs() < 1e-4 {
            self.velocity = 0.0;
        }
        self.time += dt;
    }

    /// Whether the donut still needs frames: it always idles, but this says
    /// whether user momentum is in play (used only for diagnostics).
    pub fn coasting(&self) -> bool {
        self.dragging || self.velocity != 0.0
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn field(t: f32, spin: f32) -> Vec<f32> {
        let mut donut = Donut::new(48);
        donut.render(t, spin);
        donut.lum.clone()
    }

    #[test]
    fn field_is_lit_and_has_a_hole() {
        let lum = field(0.8, 0.0);
        let lit = lum.iter().filter(|v| **v > 0.05).count();
        // A torus at this tilt covers a healthy but not overwhelming fraction.
        assert!(lit > 200, "expected a lit donut, got {lit} lit cells");
        assert!(lit < lum.len() / 2, "donut should not fill the frame");
        // The hole: the field centre is unlit at this tilt.
        let grid = 48;
        assert_eq!(lum[grid / 2 + (grid / 2) * grid], 0.0);
    }

    #[test]
    fn render_is_deterministic() {
        assert_eq!(field(1.25, 0.4), field(1.25, 0.4));
    }

    #[test]
    fn luminance_stays_in_unit_range() {
        for step in 0..20 {
            for v in field(step as f32 * 0.3, 0.0) {
                assert!((0.0..=1.0).contains(&v), "luminance out of range: {v}");
            }
        }
    }

    #[test]
    fn spin_changes_the_image_and_tilt_is_drag_independent() {
        // Dragging rotates the donut.
        assert_ne!(field(1.0, 0.0), field(1.0, 1.0));
        // A full turn of drag is the same image, so drag only affects yaw.
        let full = field(1.0, std::f32::consts::TAU);
        let base = field(1.0, 0.0);
        let max_diff = base
            .iter()
            .zip(&full)
            .map(|(a, b)| (a - b).abs())
            .fold(0.0f32, f32::max);
        assert!(max_diff < 1e-3, "tau of drag should be a no-op: {max_diff}");
    }

    #[test]
    fn drag_momentum_decays_to_rest() {
        let mut spin = Spin::default();
        spin.press(100.0);
        spin.drag_to(160.0);
        assert!(spin.velocity > 0.0);
        spin.release();
        for _ in 0..400 {
            spin.advance(1.0 / 60.0);
        }
        assert_eq!(spin.velocity, 0.0, "momentum must come to rest");
        assert!(spin.offset > 0.0, "drag should have moved the donut");
    }

    #[test]
    fn drag_without_press_is_ignored() {
        let mut spin = Spin::default();
        spin.drag_to(500.0);
        assert_eq!(spin.velocity, 0.0);
    }

    #[test]
    fn sample_is_zero_outside_the_field() {
        let mut donut = Donut::new(32);
        donut.render(0.5, 0.0);
        assert_eq!(donut.sample(-1.0, 5.0), 0.0);
        assert_eq!(donut.sample(5.0, 40.0), 0.0);
    }
}
