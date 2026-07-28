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
    ///
    /// Rows are independent, so the march is split across worker threads: the
    /// result is bit-identical to the serial version (no accumulation across
    /// rows, no reduction order to change), which is what makes it safe to
    /// parallelise a function that captures and tests compare exactly.
    pub fn render(&mut self, t: f32, spin: f32) {
        let grid = self.grid;
        let view = View::new(grid, t, spin);
        let threads = worker_count(grid);
        if threads <= 1 {
            march_rows(&mut self.lum, 0, grid, &view);
            return;
        }
        // Split into contiguous row bands, one per worker. Bands rather than
        // interleaved rows so each thread writes one cache-contiguous slice.
        let rows_per = grid.div_ceil(threads);
        std::thread::scope(|scope| {
            let mut rest = self.lum.as_mut_slice();
            let mut row = 0;
            while row < grid {
                let take = rows_per.min(grid - row);
                let (band, tail) = rest.split_at_mut(take * grid);
                rest = tail;
                let view = &view;
                scope.spawn(move || march_rows(band, row, row + take, view));
                row += take;
            }
        });
    }

    /// Fraction of the field's square actually inked, as (horizontal,
    /// vertical) extents in `0..=1`. The torus at this tilt never reaches the
    /// square's edges, so layout spaces the hero text against these instead of
    /// the raw box, which is what stops the gap under the donut looking like a
    /// mistake.
    pub fn ink_extent(&self) -> (f32, f32) {
        let grid = self.grid;
        let (mut x0, mut x1, mut y0, mut y1) = (grid, 0usize, grid, 0usize);
        for y in 0..grid {
            for x in 0..grid {
                if self.lum[x + y * grid] > 0.04 {
                    x0 = x0.min(x);
                    x1 = x1.max(x);
                    y0 = y0.min(y);
                    y1 = y1.max(y);
                }
            }
        }
        if x1 < x0 || y1 < y0 {
            return (0.0, 0.0);
        }
        let n = grid as f32;
        ((x1 - x0 + 1) as f32 / n, (y1 - y0 + 1) as f32 / n)
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

/// Per-frame camera and orientation constants. Computed once per frame and
/// shared by every worker, so the trig is not repeated per row (which is what
/// made the original donut.c inner loop the expensive part).
struct View {
    grid: usize,
    k1: f32,
    cc: f32,
    oy: f32,
    oz: f32,
    m00: f32,
    m01: f32,
    m10: f32,
    m11: f32,
    m12: f32,
    m20: f32,
    m21: f32,
    m22: f32,
}

impl View {
    fn new(grid: usize, t: f32, spin: f32) -> Self {
        let a = 1.0 + (t * 0.4).sin() * 0.25;
        let b = t * 0.7 + spin;
        let (sa, ca) = a.sin_cos();
        let (sb, cb) = b.sin_cos();
        Self {
            grid,
            k1: (grid as f32 * K2 * 3.0) / (8.0 * (R1 + R2)),
            cc: K2 * K2 - BOUND * BOUND,
            // Eye at the world origin, torus centred at (0,0,K2): the eye in
            // torus-local space.
            oy: -ca * K2,
            oz: -sa * K2,
            // Rotation local<-world: the transpose of donut.c's world<-local
            // matrix, so orientation and lighting match the original exactly.
            m00: cb,
            m01: sb,
            m10: sa * sb,
            m11: -sa * cb,
            m12: ca,
            m20: -ca * sb,
            m21: ca * cb,
            m22: sa,
        }
    }
}

/// March rows `[from, to)` of the field into `out`, which is exactly those rows.
fn march_rows(out: &mut [f32], from: usize, to: usize, view: &View) {
    let grid = view.grid;
    let half = grid as f32 / 2.0;
    for yc in from..to {
        let pyw = -(yc as f32 + 0.5 - half) / view.k1;
        let row = (yc - from) * grid;
        for xc in 0..grid {
            let pxw = (xc as f32 + 0.5 - half) / view.k1;
            let inv = 1.0 / (pxw * pxw + pyw * pyw + 1.0).sqrt();
            let (dwx, dwy, dwz) = (pxw * inv, pyw * inv, inv);
            let dx = view.m00 * dwx + view.m01 * dwy;
            let dy = view.m10 * dwx + view.m11 * dwy + view.m12 * dwz;
            let dz = view.m20 * dwx + view.m21 * dwy + view.m22 * dwz;
            out[row + xc] = trace(dx, dy, dz, view);
        }
    }
}

/// Workers to march a `grid`-square field with. Small fields are not worth the
/// spawn cost, and the count is capped so a decorative animation never takes
/// the whole machine.
fn worker_count(grid: usize) -> usize {
    if grid < 48 {
        return 1;
    }
    let cpus = std::thread::available_parallelism()
        .map(std::num::NonZeroUsize::get)
        .unwrap_or(1);
    // Leave a core for the UI thread's own work (text layout, path building).
    cpus.saturating_sub(1).clamp(1, 8)
}

/// Signed distance to the torus in local space.
#[inline]
fn sdf(px: f32, py: f32, pz: f32) -> f32 {
    let q = (px * px + py * py).sqrt() - R2;
    (q * q + pz * pz).sqrt() - R1
}

/// Sphere-trace one ray and return its lit luminance in `0..=1`.
fn trace(dx: f32, dy: f32, dz: f32, view: &View) -> f32 {
    let View {
        k1,
        cc,
        oy,
        oz,
        m01,
        m11,
        m12,
        m21,
        m22,
        ..
    } = *view;
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

    /// The tilt wobbles, so the inked extent must stay in a narrow band: this
    /// is the number `layout::DONUT_INK_FRACTION` encodes, so a change to the
    /// pose that invalidates the hero's spacing fails here rather than looking
    /// subtly wrong.
    #[test]
    fn ink_extent_is_stable_across_the_wobble() {
        let mut donut = Donut::new(96);
        let (mut lo, mut hi) = (1.0f32, 0.0f32);
        // Vertical extent only: the hero's clearance problem is the donut
        // touching the wordmark above and the tagline below, and the torus is
        // wider than it is tall at this tilt, so folding width in would demand
        // more top/bottom padding than the shape ever needs.
        let mut wide = 0.0f32;
        for step in 0..40 {
            donut.render(step as f32 * 0.4, step as f32 * 0.7);
            let (w, h) = donut.ink_extent();
            wide = wide.max(w);
            lo = lo.min(h);
            hi = hi.max(h);
        }
        assert!(wide <= 1.0, "the donut overflowed its box sideways: {wide}");
        assert!(lo > 0.6, "the donut shrank out of its box: {lo}");
        assert!(hi <= 1.0, "the donut overflowed its box: {hi}");
        // Layout spaces the hero text against the *widest* the silhouette ever
        // gets, so the gap cannot close on the frames where the disc is
        // tallest. Anything smaller than `hi` would let the donut eat into the
        // wordmark's clearance mid-animation.
        assert!(
            (crate::layout::DONUT_INK_FRACTION as f32) >= hi - 1e-3,
            "DONUT_INK_FRACTION {} is under the measured maximum {hi}",
            crate::layout::DONUT_INK_FRACTION
        );
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
