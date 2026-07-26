use crate::tui::{TuiState, color_support::rgb};
use ratatui::prelude::*;
use std::cell::RefCell;
use std::collections::{HashSet, hash_map::DefaultHasher};
use std::hash::{Hash, Hasher};
use std::sync::OnceLock;

const IDLE_VARIANTS: &[&str] = &["donut", "orbit_rings"];

/// Where (if anywhere) the decorative idle animation rendered on the last full
/// frame, so the run loop can repaint only those rows on animation ticks.
///
/// Packed into one atomic (`None` is the all-ones sentinel, which no real Rect
/// can be) so publishing it costs nothing on the render path and reading it
/// cannot fail or block. Ratatui coordinates are `u16`, so all four fields fit.
#[cfg(not(test))]
static LAST_IDLE_ANIMATION_AREA: std::sync::atomic::AtomicU64 =
    std::sync::atomic::AtomicU64::new(u64::MAX);

fn pack_area(area: Option<Rect>) -> u64 {
    match area {
        None => u64::MAX,
        Some(area) => {
            u64::from(area.x) << 48
                | u64::from(area.y) << 32
                | u64::from(area.width) << 16
                | u64::from(area.height)
        }
    }
}

fn unpack_area(packed: u64) -> Option<Rect> {
    if packed == u64::MAX {
        return None;
    }
    Some(Rect::new(
        (packed >> 48) as u16,
        (packed >> 32) as u16,
        (packed >> 16) as u16,
        packed as u16,
    ))
}

/// Counts of how idle-animation ticks were served, so the partial-repaint fast
/// path is observable in `draw-stats` instead of having to be inferred from a
/// profile. A healthy idle screen shows `partial` climbing and `full` flat: a
/// regression that pushes animation ticks back onto the full frame pipeline
/// (~50x more expensive) shows up immediately as `full` climbing instead.
static IDLE_ANIMATION_PARTIAL_REPAINTS: std::sync::atomic::AtomicU64 =
    std::sync::atomic::AtomicU64::new(0);
static IDLE_ANIMATION_FULL_REPAINTS: std::sync::atomic::AtomicU64 =
    std::sync::atomic::AtomicU64::new(0);

pub(crate) fn note_idle_animation_partial_repaint() {
    IDLE_ANIMATION_PARTIAL_REPAINTS.fetch_add(1, std::sync::atomic::Ordering::Relaxed);
}

pub(crate) fn note_idle_animation_full_repaint() {
    IDLE_ANIMATION_FULL_REPAINTS.fetch_add(1, std::sync::atomic::Ordering::Relaxed);
}

/// Tally of why the animation-only repaint was refused, for `draw-stats`.
///
/// A tally rather than a "last reason" because the fast path is consulted many
/// times per second: a single latest value is ambiguous (it can be a rare case
/// that happened to fire last), while counts show which predicate actually
/// dominates.
static IDLE_ANIMATION_FAST_PATH_BLOCKED: std::sync::Mutex<
    Option<std::collections::BTreeMap<&'static str, u64>>,
> = std::sync::Mutex::new(None);

pub(crate) fn note_idle_animation_fast_path_blocked(reason: &'static str) {
    if let Ok(mut slot) = IDLE_ANIMATION_FAST_PATH_BLOCKED.lock() {
        *slot
            .get_or_insert_with(std::collections::BTreeMap::new)
            .entry(reason)
            .or_insert(0) += 1;
    }
}

pub(crate) fn idle_animation_fast_path_blocked_reasons() -> serde_json::Value {
    match IDLE_ANIMATION_FAST_PATH_BLOCKED.lock() {
        Ok(slot) => match slot.as_ref() {
            Some(counts) => serde_json::to_value(counts).unwrap_or(serde_json::Value::Null),
            None => serde_json::Value::Null,
        },
        Err(_) => serde_json::Value::Null,
    }
}

/// Diagnostic snapshot of how idle-animation ticks are being served, embedded
/// in `draw-stats`. Assembled here so the animation owns its own reporting and
/// the generic frame-metrics module stays agnostic.
pub(crate) fn idle_animation_debug_json() -> serde_json::Value {
    serde_json::json!({
        "partial_repaints": IDLE_ANIMATION_PARTIAL_REPAINTS
            .load(std::sync::atomic::Ordering::Relaxed),
        "full_repaints": IDLE_ANIMATION_FULL_REPAINTS
            .load(std::sync::atomic::Ordering::Relaxed),
        "last_full_frame_reason": crate::tui::last_full_frame_redraw_reason(),
        "fast_path_blocked": idle_animation_fast_path_blocked_reasons(),
    })
}

/// Publish the animated rectangle for the animation-only partial repaint.
/// `None` means the last frame drew no animation, disabling the fast path.
pub(crate) fn record_idle_animation_area(area: Option<Rect>) {
    #[cfg(test)]
    {
        TEST_LAST_IDLE_ANIMATION_AREA.with(|snapshot| {
            *snapshot.borrow_mut() = area;
        });
    }
    #[cfg(not(test))]
    {
        LAST_IDLE_ANIMATION_AREA.store(pack_area(area), std::sync::atomic::Ordering::Relaxed);
    }
}

pub(crate) fn last_idle_animation_area() -> Option<Rect> {
    #[cfg(test)]
    {
        return TEST_LAST_IDLE_ANIMATION_AREA.with(|snapshot| *snapshot.borrow());
    }
    #[cfg(not(test))]
    {
        unpack_area(LAST_IDLE_ANIMATION_AREA.load(std::sync::atomic::Ordering::Relaxed))
    }
}

/// Repaint just the idle-animation rows of an already-rendered frame buffer.
///
/// The surrounding cells were already theme/emoji adapted when the full frame
/// was drawn, so adaptation is applied to the freshly written animation cells
/// only: `adapt_buffer_for_theme` inverts colors on light terminals and is not
/// idempotent, so re-running it over the reused buffer would flip everything
/// back. The animation writes plain box/shade glyphs and a foreground color, so
/// per-cell foreground adaptation is the complete equivalent here.
pub(crate) fn render_idle_animation_into(buf: &mut Buffer, area: Rect, elapsed: f32) {
    let area = area.intersection(*buf.area());
    // A full frame clears the whole surface before drawing, so the animation
    // blits over reset cells and leaves its blank cells styleless. Here the rows
    // still hold the previous tick's glyphs and colors, so reset them first to
    // reproduce that starting state exactly.
    for y in area.top()..area.bottom() {
        for x in area.left()..area.right() {
            buf[(x, y)].reset();
        }
    }
    render_idle_animation(buf, area, elapsed);
    for y in area.top()..area.bottom() {
        for x in area.left()..area.right() {
            let cell = &mut buf[(x, y)];
            cell.fg = jcode_tui_style::adapt_color_for_theme(cell.fg);
        }
    }
}

// Pure math kernels (3D samplers, glyph chooser, HSV->RGB) live in the
// dependency-free `jcode-tui-anim` crate, which is pinned to opt-level = 3 in
// all profiles so these trig-heavy loops stay optimized even in debug/selfdev
// builds. They are imported under their original names so the call sites and
// tests below are unchanged.
use jcode_tui_anim::{
    hsv_to_rgb, sample_black_hole, sample_donut, sample_gyroscope, sample_orbit_rings,
    shape_char_3x3,
};

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

#[cfg(test)]
thread_local! {
    static TEST_LAST_IDLE_ANIMATION_AREA: RefCell<Option<Rect>> = const { RefCell::new(None) };
}

thread_local! {
    static IDLE_BUF: RefCell<IdleBuffers> = RefCell::new(IdleBuffers::new());
}

/// Rows reserved below the input for the decorative idle donut.
///
/// The donut only shows on an (effectively) empty idle screen, which means it
/// is pure negative space. When the composer grows past its resting one-row
/// height (multi-line input, or the `/` command menu adding suggestion rows),
/// take that growth out of the donut's reservation instead of shrinking the
/// transcript above: this keeps the header/tips text and info widgets
/// perfectly still when the slash menu opens on a fresh session. The donut
/// simply renders shorter for as long as the composer is expanded.
pub(crate) fn idle_donut_reserved_height(show_donut: bool, input_height: u16) -> u16 {
    const IDLE_DONUT_HEIGHT: u16 = 14;
    if show_donut {
        let composer_growth = input_height.saturating_sub(1);
        IDLE_DONUT_HEIGHT.saturating_sub(composer_growth)
    } else {
        0
    }
}

pub(super) fn draw_idle_animation(frame: &mut Frame, app: &dyn TuiState, area: Rect) {
    // Remember the animated rows so the run loop can repaint only them between
    // full frames. Sizes the animation rejects are recorded as "no animation".
    record_idle_animation_area((area.width >= 4 && area.height >= 2).then_some(area));
    render_idle_animation(frame.buffer_mut(), area, app.animation_elapsed());
}

/// Render the decorative idle animation into `buf` over `area`.
///
/// Split out from [`draw_idle_animation`] so the run loop can repaint just the
/// animation rows between full frames: the animation is the only thing that
/// changes on an idle screen, and re-rendering the whole frame at animation FPS
/// was costing ~50ms per tick (dominated by transcript/chrome work that
/// produced byte-identical cells).
pub(crate) fn render_idle_animation(buf: &mut Buffer, area: Rect, elapsed: f32) {
    if area.width < 4 || area.height < 2 {
        return;
    }

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

        // Render glyphs directly into the frame buffer. Each idle line is exactly
        // `area.width` cells wide, so Paragraph's Center/Left alignment offset is
        // always 0; writing cells in place produces byte-identical output to the
        // old `Paragraph::new(Vec<Line<Vec<Span>>>)` path (proven by
        // `direct_blit_matches_paragraph` below) while avoiding a per-cell String
        // plus a Vec<Span>/Vec<Line>/Paragraph allocation on every animation
        // frame (60 FPS) -- the dominant idle-render cost once the samplers were
        // optimized.
        blit_idle(buf, area, hit, lum_map, sw, time_hue);
    });
}

/// Composite the subpixel `hit`/`lum_map` grids into terminal cells, writing
/// directly into `buf` over `area`. Factored out so it can be diffed against the
/// original Paragraph-based renderer in tests.
fn blit_idle(
    buf: &mut Buffer,
    area: Rect,
    hit: &[bool],
    lum_map: &[f32],
    sw: usize,
    time_hue: f32,
) {
    const SUB_X: usize = 3;
    const SUB_Y: usize = 3;
    let cw = area.width as usize;
    let ch = area.height as usize;

    for row in 0..ch {
        let y = area.y + row as u16;
        for col in 0..cw {
            let x = area.x + col as u16;

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

            let cell = &mut buf[(x, y)];
            if hit_count == 0 {
                // Matches the old `Span::raw(" ")`: symbol becomes a space and
                // the (empty) default style patches nothing onto the cell.
                cell.set_char(' ');
            } else {
                let avg_lum = total_lum / hit_count as f32;
                let coverage = hit_count as f32 / (SUB_X * SUB_Y) as f32;
                let t = (avg_lum + 1.0) * 0.5;
                let glyph = shape_char_3x3(pattern, t);

                let hue = (time_hue + t * 160.0) % 360.0;
                let hue = if hue < 0.0 { hue + 360.0 } else { hue };

                let sat = 0.5 + t * 0.4;
                let val = (0.10 + t * t * 0.90) * (0.55 + coverage * 0.45);
                let (r, g, b) = hsv_to_rgb(hue, sat, val);
                // Matches the old `Span::styled(.., Style::default().fg(..))`:
                // set the symbol and patch only the foreground color.
                cell.set_char(glyph).set_fg(rgb(r, g, b));
            }
        }
    }
}

fn idle_animation_variant() -> &'static str {
    choose_animation_variant(IDLE_VARIANTS, 0x4944_4c45_414e_494d)
}

#[cfg(test)]
mod tests {
    use super::*;
    use ratatui::widgets::{Paragraph, Widget};

    type IdleSampler = fn(f32, usize, usize, &mut [bool], &mut [f32], &mut [f32]);

    /// The original Paragraph-based renderer, kept here verbatim so we can prove
    /// the in-place `blit_idle` produces a byte-identical `Buffer`.
    fn render_idle_via_paragraph(
        buf: &mut Buffer,
        area: Rect,
        hit: &[bool],
        lum_map: &[f32],
        sw: usize,
        time_hue: f32,
        centered: bool,
    ) {
        const SUB_X: usize = 3;
        const SUB_Y: usize = 3;
        let cw = area.width as usize;
        let ch = area.height as usize;
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
                            let glyph = shape_char_3x3(pattern, t);

                            let hue = (time_hue + t * 160.0) % 360.0;
                            let hue = if hue < 0.0 { hue + 360.0 } else { hue };

                            let sat = 0.5 + t * 0.4;
                            let val = (0.10 + t * t * 0.90) * (0.55 + coverage * 0.45);
                            let (r, g, b) = hsv_to_rgb(hue, sat, val);
                            Span::styled(String::from(glyph), Style::default().fg(rgb(r, g, b)))
                        }
                    })
                    .collect();
                Line::from(spans).alignment(align)
            })
            .collect();

        Paragraph::new(lines).render(area, buf);
    }

    /// The direct-buffer renderer must produce a Buffer byte-identical to the
    /// old Paragraph path, for both Center and Left "alignment" (the idle lines
    /// are full width, so the offset is always 0), across several sizes/times
    /// and all idle variants.
    #[test]
    fn direct_blit_matches_paragraph() {
        type Sampler = fn(f32, usize, usize, &mut [bool], &mut [f32], &mut [f32]);
        let samplers: &[(&str, Sampler)] = &[
            ("donut", sample_donut),
            ("orbit_rings", sample_orbit_rings),
            ("gyroscope", sample_gyroscope),
            ("black_hole", sample_black_hole),
        ];

        for &(cw, chh) in &[(40u16, 16u16), (80, 30), (120, 40), (57, 23)] {
            const SUB_X: usize = 3;
            const SUB_Y: usize = 3;
            let sw = cw as usize * SUB_X;
            let sh = chh as usize * SUB_Y;
            let area = Rect::new(2, 1, cw, chh);
            let buf_area = Rect::new(0, 0, cw + 4, chh + 2);

            for &(name, sampler) in samplers {
                for &elapsed in &[0.0f32, 0.4, 1.6, 3.3, 7.1] {
                    let n = sw * sh;
                    let (mut hit, mut lum, mut z) =
                        (vec![false; n], vec![0.0f32; n], vec![0.0f32; n]);
                    sampler(elapsed, sw, sh, &mut hit, &mut lum, &mut z);
                    let time_hue = elapsed * 40.0;

                    for &centered in &[false, true] {
                        let mut buf_ref = Buffer::empty(buf_area);
                        let mut buf_new = Buffer::empty(buf_area);
                        render_idle_via_paragraph(
                            &mut buf_ref,
                            area,
                            &hit,
                            &lum,
                            sw,
                            time_hue,
                            centered,
                        );
                        blit_idle(&mut buf_new, area, &hit, &lum, sw, time_hue);

                        assert_eq!(
                            buf_ref, buf_new,
                            "{name}: buffer mismatch at {cw}x{chh} t={elapsed} centered={centered}"
                        );
                    }
                }
            }
        }
    }

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

    /// The published animation rectangle is packed into a single atomic word so
    /// the render path can publish it without a lock. `None` uses an all-ones
    /// sentinel, which no real `Rect` can produce because a full-`u16` Rect is
    /// not a valid terminal area. Verify the encoding is lossless.
    #[test]
    fn animation_area_packing_round_trips() {
        for area in [
            Rect::new(0, 0, 0, 0),
            Rect::new(0, 0, 4, 2),
            Rect::new(7, 26, 94, 14),
            Rect::new(u16::MAX - 1, u16::MAX - 1, 1, 1),
            Rect::new(1, 1, u16::MAX, u16::MAX),
        ] {
            let packed = pack_area(Some(area));
            assert_ne!(packed, u64::MAX, "{area:?} collided with the None sentinel");
            assert_eq!(
                unpack_area(packed),
                Some(area),
                "lossy packing for {area:?}"
            );
        }

        assert_eq!(pack_area(None), u64::MAX);
        assert_eq!(unpack_area(pack_area(None)), None);
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
