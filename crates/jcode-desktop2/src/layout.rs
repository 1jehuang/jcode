//! Pure layout geometry, in logical (device-independent) units.
//!
//! Layout is separated from drawing so the rules in
//! `docs/DESKTOP2_VISUAL_CHECKLIST.md` are machine-checkable: `Frame` is a
//! pure function of the window box, and the tests below assert the geometric
//! invariants (measure, gutters, no overlap, hairline crispness) across a
//! sweep of window sizes and scale factors instead of relying on eyeballing
//! screenshots.

/// Body copy measure cap. Long lines are the most common way a text UI
/// becomes unreadable, so the column never exceeds this.
pub const MEASURE: f64 = 720.0;
/// Body copy size and leading (style guide: 1.65).
pub const BODY_SIZE: f32 = 13.5;
pub const BODY_LEADING: f64 = 1.65;
/// Caption size for status/hints.
pub const CAPTION_SIZE: f32 = 10.5;
/// Composer well height for a single line, and the inner padding.
pub const COMPOSER_HEIGHT: f64 = 44.0;
/// Extra height per additional composer line.
pub const COMPOSER_LINE_HEIGHT: f64 = 20.0;
/// Composer lines shown before it stops growing and scrolls internally.
pub const COMPOSER_MAX_LINES: usize = 8;
pub const COMPOSER_PAD_X: f64 = 14.0;
pub const COMPOSER_RADIUS: f64 = 6.0;
/// Width reserved at the left of the well for the prompt marker. A field that
/// is only a grey slab is ambiguous about where typing starts; the marker
/// makes the text origin explicit and survives an empty composer.
pub const COMPOSER_MARKER_ADVANCE: f64 = 20.0;
/// Field border thickness, in logical units. Drawn as a stroke so the composer
/// reads as an input field rather than a block of shaded paper.
pub const COMPOSER_BORDER: f64 = 1.0;
/// Extra border thickness when the window has keyboard focus.
pub const COMPOSER_BORDER_FOCUS: f64 = 1.25;
/// Top of the prompt text inside the composer well. Derived so a single line
/// is vertically centred: hardcoding it left the text a pixel high.
pub const COMPOSER_TEXT_OFFSET: f64 = (COMPOSER_HEIGHT - COMPOSER_LINE_HEIGHT) / 2.0;
/// Insert caret: a thin vertical bar, like any normal text input.
pub const CARET_WIDTH: f64 = 1.5;
pub const CARET_HEIGHT: f64 = 18.0;
/// Caption row under the composer for notices and the scrollback indicator.
pub const FOOTNOTE_HEIGHT: f64 = 16.0;
pub const FOOTNOTE_GAP: f64 = 6.0;
/// Largest the hero donut gets, in logical units.
pub const DONUT_MAX_SIDE: f64 = 320.0;
/// Below this the halftone screen has too few dots to read as a donut, so it is
/// not drawn at all rather than degrading into speckle on a cramped window.
pub const DONUT_MIN_SIDE: f64 = 120.0;
/// Vertical breathing room between regions.
pub const SPACE_BEFORE_COMPOSER: f64 = 20.0;
/// Fraction of the page height the input box is centred on. 0.5 puts the
/// composer in the exact middle of the window; it only leaves that line when
/// the page is too short to keep a transcript line above it.
pub const COMPOSER_CENTER: f64 = 0.5;

/// Resolved geometry for one frame. All fields are logical pixels.
#[derive(Clone, Copy, Debug, PartialEq)]
pub struct Frame {
    pub width: f64,
    pub height: f64,
    pub scale: f64,
    /// Left edge of the measure column.
    pub left: f64,
    /// Right edge of the measure column.
    pub right: f64,
    /// Top of the transcript region.
    pub body_top: f64,
    /// Bottom of the transcript region.
    pub body_bottom: f64,
    pub composer_top: f64,
    pub composer_bottom: f64,
    /// Caption row under the composer. Reserved even when empty, so a notice
    /// appearing never shifts the composer or spills off-paper.
    pub footnote_top: f64,
    pub footnote_bottom: f64,
}

impl Frame {
    /// Resolve geometry for a surface of `size` physical pixels at `scale`,
    /// with a single-line composer.
    pub fn new(size: (u32, u32), scale: f64) -> Self {
        Self::with_composer_lines(size, scale, 1)
    }

    /// Resolve geometry with a composer sized for `lines` of input. The
    /// composer grows upward so the transcript shrinks instead of the input
    /// being clipped, and stops growing at [`COMPOSER_MAX_LINES`] so a long
    /// paste can never push the transcript off the page.
    pub fn with_composer_lines(size: (u32, u32), scale: f64, lines: usize) -> Self {
        let scale = if scale.is_finite() && scale > 0.0 {
            scale
        } else {
            1.0
        };
        // Guard against degenerate surfaces (minimized/zero-sized windows).
        let width = (f64::from(size.0) / scale).max(240.0);
        let height = (f64::from(size.1) / scale).max(200.0);

        let gutter = (width * 0.06).clamp(20.0, 64.0);
        let column = (width - gutter * 2.0).clamp(120.0, MEASURE);
        let left = ((width - column) / 2.0).max(gutter.min((width - column).max(0.0)));
        let right = left + column;

        // No masthead: the transcript starts at the top margin. The window
        // chrome already says which app this is, so a wordmark, a status
        // caption, and a build-identity row only stole reading space from the
        // one thing the user came for.
        let top_margin = (height * 0.05).clamp(20.0, 40.0);
        let bottom_margin = (height * 0.05).clamp(20.0, 40.0);
        let body_top = top_margin;
        // Hard floor: the composer must leave room for its own caption row
        // above the bottom margin.
        let slot_bottom = height - bottom_margin - FOOTNOTE_HEIGHT - FOOTNOTE_GAP;
        // Soft floor: keep at least one transcript line visible above the well.
        let min_top = body_top + SPACE_BEFORE_COMPOSER + f64::from(BODY_SIZE) * BODY_LEADING;

        let extra_lines = lines.clamp(1, COMPOSER_MAX_LINES) - 1;
        let wanted = COMPOSER_HEIGHT + extra_lines as f64 * COMPOSER_LINE_HEIGHT;
        let composer_height = wanted.min((slot_bottom - min_top).max(COMPOSER_HEIGHT));
        // The input box sits on the middle of the page and grows symmetrically
        // about that line as the text wraps, clamped so it never crosses the
        // top margin or its own caption row.
        let centred = height * COMPOSER_CENTER - composer_height * 0.5;
        let ceiling = (slot_bottom - composer_height).max(body_top);
        let composer_top = centred.clamp(min_top.min(ceiling), ceiling);
        let composer_bottom = composer_top + composer_height;
        let footnote_top = composer_bottom + FOOTNOTE_GAP;
        let footnote_bottom = footnote_top + FOOTNOTE_HEIGHT;

        let body_bottom = (composer_top - SPACE_BEFORE_COMPOSER).max(body_top);

        Self {
            width,
            height,
            scale,
            left,
            right,
            body_top,
            body_bottom,
            composer_top,
            composer_bottom,
            footnote_top,
            footnote_bottom,
        }
    }

    /// Width of the measure column.
    pub fn column(&self) -> f64 {
        self.right - self.left
    }

    /// Height of one body line.
    pub fn body_line_height(&self) -> f64 {
        f64::from(BODY_SIZE) * BODY_LEADING
    }

    /// Body lines that fit in the transcript region.
    pub fn visible_body_lines(&self) -> usize {
        (((self.body_bottom - self.body_top) / self.body_line_height()) as usize).max(1)
    }

    /// Width available to composer text, inside the well's padding. This is
    /// the wrap width handed to Parley, so the text wraps exactly where the
    /// well ends rather than at an estimated character count.
    pub fn composer_text_width(&self) -> f64 {
        (self.right - COMPOSER_PAD_X - self.composer_text_left()).max(1.0)
    }

    /// Left edge of composer text: inside the padding and past the prompt
    /// marker. The single source of truth for the text origin, so drawing,
    /// caret geometry, and click hit-testing cannot drift apart.
    pub fn composer_text_left(&self) -> f64 {
        self.left + COMPOSER_PAD_X + COMPOSER_MARKER_ADVANCE
    }

    /// Composer lines this frame was built for.
    pub fn composer_lines(&self) -> usize {
        let extra = (self.composer_bottom - self.composer_top - COMPOSER_HEIGHT).max(0.0);
        1 + (extra / COMPOSER_LINE_HEIGHT).round() as usize
    }

    /// The caret must stay inside the composer well at any size.
    #[cfg_attr(not(test), allow(dead_code))]
    pub fn caret_fits_in_composer(&self) -> bool {
        let top = self.composer_top + COMPOSER_TEXT_OFFSET - 1.0;
        top >= self.composer_top && top + CARET_HEIGHT <= self.composer_bottom
    }

    /// Thickness that renders as exactly one physical pixel. Kept as the
    /// single definition of crispness for any future rule or border.
    #[cfg_attr(not(test), allow(dead_code))]
    pub fn hairline(&self) -> f64 {
        1.0 / self.scale
    }

    /// Square box for the hero donut, centred in the transcript region under
    /// the placeholder line. It is only drawn on an empty session, so it
    /// borrows dead space rather than reserving any: nothing else in the frame
    /// moves when it appears or stands down.
    pub fn donut_box(&self) -> vello::kurbo::Rect {
        let top = (self.body_top + self.body_line_height() * 2.0).min(self.body_bottom);
        let available = (self.body_bottom - top).max(0.0);
        // Capped so the donut stays a motif on tall windows rather than a
        // poster, and so it never crowds the composer.
        let side = available.min(self.column()).min(DONUT_MAX_SIDE);
        let cx = (self.left + self.right) / 2.0;
        let cy = top + available / 2.0;
        vello::kurbo::Rect::new(
            cx - side / 2.0,
            cy - side / 2.0,
            cx + side / 2.0,
            cy + side / 2.0,
        )
    }

    /// Whether a logical point is inside the donut, used for drag hit-testing.
    /// Circular, not the bounding box, so clicks in the corners still reach
    /// whatever is behind it.
    pub fn hits_donut(&self, x: f64, y: f64) -> bool {
        let box_ = self.donut_box();
        let radius = box_.width().min(box_.height()) / 2.0;
        if radius < DONUT_MIN_SIDE / 2.0 {
            return false;
        }
        let cx = box_.x0 + box_.width() / 2.0;
        let cy = box_.y0 + box_.height() / 2.0;
        (x - cx).powi(2) + (y - cy).powi(2) <= radius * radius
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    /// Window boxes to sweep: tiny, phone-ish, laptop, wide, and tall.
    const SIZES: &[(u32, u32)] = &[
        (320, 240),
        (640, 480),
        (800, 600),
        (1100, 720),
        (1440, 900),
        (1920, 1080),
        (2560, 1440),
        (3840, 2160),
        (600, 1400),
    ];
    const SCALES: &[f64] = &[1.0, 1.25, 1.5, 1.75, 2.0, 3.0];

    fn sweep(mut check: impl FnMut(Frame)) {
        for &size in SIZES {
            for &scale in SCALES {
                // Every invariant must hold for a composer of any height, not
                // just a single line.
                for lines in [1usize, 2, 4, COMPOSER_MAX_LINES, COMPOSER_MAX_LINES + 20] {
                    check(Frame::with_composer_lines(size, scale, lines));
                }
            }
        }
    }

    #[test]
    fn column_never_exceeds_measure() {
        sweep(|frame| {
            assert!(
                frame.column() <= MEASURE + 0.001,
                "column {} exceeded measure at {}x{}",
                frame.column(),
                frame.width,
                frame.height
            );
        });
    }

    #[test]
    fn column_stays_inside_the_window() {
        sweep(|frame| {
            assert!(frame.left >= 0.0, "column started off-paper");
            assert!(
                frame.right <= frame.width + 0.001,
                "column right {} overflowed width {}",
                frame.right,
                frame.width
            );
            assert!(frame.column() > 0.0, "column collapsed");
        });
    }

    #[test]
    fn column_is_horizontally_balanced() {
        sweep(|frame| {
            let leading = frame.left;
            let trailing = frame.width - frame.right;
            assert!(
                (leading - trailing).abs() < 1.0 || leading <= trailing,
                "asymmetric gutters: {leading} vs {trailing}"
            );
        });
    }

    #[test]
    fn regions_are_ordered_and_never_overlap() {
        sweep(|frame| {
            assert!(frame.body_top <= frame.body_bottom);
            assert!(
                frame.body_bottom <= frame.composer_top,
                "transcript overlapped the composer"
            );
            assert!(frame.composer_top < frame.composer_bottom);
            assert!(
                frame.composer_bottom <= frame.footnote_top,
                "composer overlapped the footnote row"
            );
            assert!(frame.footnote_top < frame.footnote_bottom);
            assert!(
                frame.footnote_bottom <= frame.height + 0.001,
                "footnote row fell off the bottom"
            );
        });
    }

    /// Nothing may be drawn above the transcript: the top of the page is
    /// deliberately clear, and a reintroduced masthead would show up as a
    /// body region that no longer starts at the top margin.
    #[test]
    fn the_top_of_the_page_is_clear() {
        sweep(|frame| {
            let margin = (frame.height * 0.05).clamp(20.0, 40.0);
            assert!(
                (frame.body_top - margin).abs() < 0.001,
                "body_top {} is not the top margin {margin}: something was \
                 added above the transcript",
                frame.body_top
            );
        });
    }

    #[test]
    fn the_composer_grows_with_its_line_count() {
        let one = Frame::with_composer_lines((1100, 720), 1.0, 1);
        let three = Frame::with_composer_lines((1100, 720), 1.0, 3);
        assert!(
            three.composer_top < one.composer_top,
            "the composer did not grow for more lines"
        );
        assert!(
            three.composer_bottom > one.composer_bottom,
            "the composer must grow downward too, staying centred"
        );
        let one_center = (one.composer_top + one.composer_bottom) / 2.0;
        let three_center = (three.composer_top + three.composer_bottom) / 2.0;
        assert!(
            (one_center - three_center).abs() < 0.001,
            "growth moved the composer off its centre line: {one_center} vs {three_center}"
        );
        assert!(
            three.body_bottom < one.body_bottom,
            "the transcript did not yield space to the composer"
        );
    }

    #[test]
    fn the_composer_sits_on_the_middle_of_the_page() {
        // On any window with room for it, the input box is centred vertically:
        // this is the whole point of the layout.
        for &size in SIZES {
            for &scale in SCALES {
                for lines in [1usize, 2, 4, COMPOSER_MAX_LINES] {
                    let frame = Frame::with_composer_lines(size, scale, lines);
                    let center = (frame.composer_top + frame.composer_bottom) / 2.0;
                    let page = frame.height * COMPOSER_CENTER;
                    // Either the well is centred, or the page was too short and
                    // it was clamped against the transcript above or the caption
                    // row below. Nothing else is allowed to move it.
                    // Pushed down to keep one transcript line visible.
                    let clamped_low =
                        frame.body_bottom - frame.body_top <= frame.body_line_height() + 0.001;
                    let clamped_high = frame.footnote_bottom >= frame.height - 40.0 - 0.001;
                    assert!(
                        (center - page).abs() < 0.001 || clamped_low || clamped_high,
                        "composer centre {center} left the page middle {page} unclamped at {}x{}",
                        frame.width,
                        frame.height
                    );
                }
            }
        }
    }

    #[test]
    fn a_roomy_page_centres_the_composer_exactly() {
        for &size in &[(1100u32, 720u32), (1440, 900), (1920, 1080), (2560, 1440)] {
            let frame = Frame::with_composer_lines(size, 1.0, 1);
            let center = (frame.composer_top + frame.composer_bottom) / 2.0;
            assert!(
                (center - frame.height / 2.0).abs() < 0.001,
                "composer was not centred at {}x{}: {center}",
                frame.width,
                frame.height
            );
        }
    }

    #[test]
    fn the_footnote_row_follows_the_composer() {
        sweep(|frame| {
            assert!(
                (frame.footnote_top - (frame.composer_bottom + FOOTNOTE_GAP)).abs() < 0.001,
                "the caption row detached from the composer"
            );
        });
    }

    #[test]
    fn the_composer_stops_growing_at_the_line_cap() {
        let capped = Frame::with_composer_lines((1100, 720), 1.0, COMPOSER_MAX_LINES);
        let over = Frame::with_composer_lines((1100, 720), 1.0, COMPOSER_MAX_LINES + 50);
        assert_eq!(
            capped.composer_top, over.composer_top,
            "a huge paste grew the composer past its cap"
        );
    }

    #[test]
    fn a_tall_composer_never_eats_the_whole_page() {
        // On a short window, a multi-line composer must still leave a readable
        // transcript rather than covering it.
        for &size in SIZES {
            let frame = Frame::with_composer_lines(size, 1.75, COMPOSER_MAX_LINES);
            assert!(
                frame.body_bottom > frame.body_top,
                "the transcript collapsed at {}x{}",
                frame.width,
                frame.height
            );
            assert!(frame.composer_top > frame.body_top);
        }
    }

    #[test]
    fn composer_lines_round_trips() {
        for lines in 1..=COMPOSER_MAX_LINES {
            let frame = Frame::with_composer_lines((1400, 1000), 1.0, lines);
            assert_eq!(frame.composer_lines(), lines);
        }
    }

    #[test]
    fn the_caret_always_fits_inside_the_composer() {
        sweep(|frame| {
            assert!(
                frame.caret_fits_in_composer(),
                "caret escaped the composer well at {}x{}",
                frame.width,
                frame.height
            );
        });
    }

    #[test]
    fn hairlines_are_one_physical_pixel() {
        sweep(|frame| {
            let physical = frame.hairline() * frame.scale;
            assert!(
                (physical - 1.0).abs() < 1e-9,
                "hairline rendered {physical} physical pixels"
            );
        });
    }

    #[test]
    fn layout_is_scale_independent_in_logical_units() {
        // The same logical window must lay out identically at any DPI: this is
        // the bug that made the first cut look cramped on a 1.75x display.
        let base = Frame::new((1100, 720), 1.0);
        for &scale in SCALES {
            let scaled = Frame::new(
                (
                    (1100.0 * scale).round() as u32,
                    (720.0 * scale).round() as u32,
                ),
                scale,
            );
            for (name, a, b) in [
                ("left", base.left, scaled.left),
                ("right", base.right, scaled.right),
                ("body_top", base.body_top, scaled.body_top),
                ("body_bottom", base.body_bottom, scaled.body_bottom),
                ("composer_top", base.composer_top, scaled.composer_top),
                ("footnote_top", base.footnote_top, scaled.footnote_top),
            ] {
                assert!(
                    (a - b).abs() < 1.0,
                    "{name} drifted with scale {scale}: {a} vs {b}"
                );
            }
        }
    }

    #[test]
    fn transcript_always_shows_at_least_one_line() {
        sweep(|frame| {
            assert!(frame.visible_body_lines() >= 1);
        });
    }

    #[test]
    fn degenerate_sizes_do_not_panic_or_invert() {
        for size in [(0, 0), (1, 1), (10, 4000), (4000, 10)] {
            let frame = Frame::new(size, 1.75);
            assert!(frame.column() > 0.0);
            assert!(frame.body_top <= frame.body_bottom);
            assert!(frame.composer_top < frame.composer_bottom);
        }
    }

    #[test]
    fn donut_stays_inside_the_transcript_region() {
        sweep(|frame| {
            let box_ = frame.donut_box();
            assert!(
                box_.y0 >= frame.body_top - 0.001,
                "donut crossed the top margin at {}x{}",
                frame.width,
                frame.height
            );
            assert!(
                box_.y1 <= frame.composer_top + 0.001,
                "donut overlapped the composer at {}x{}",
                frame.width,
                frame.height
            );
            assert!(box_.x0 >= frame.left - 0.001 && box_.x1 <= frame.right + 0.001);
        });
    }

    #[test]
    fn donut_is_square_and_bounded() {
        sweep(|frame| {
            let box_ = frame.donut_box();
            assert!(
                (box_.width() - box_.height()).abs() < 1e-9,
                "donut must be square"
            );
            assert!(box_.width() <= DONUT_MAX_SIDE + 1e-9);
            assert!(box_.width() >= 0.0);
        });
    }

    #[test]
    fn donut_is_centred_in_the_column() {
        sweep(|frame| {
            let box_ = frame.donut_box();
            let centre = (box_.x0 + box_.x1) / 2.0;
            assert!((centre - (frame.left + frame.right) / 2.0).abs() < 1e-9);
        });
    }

    #[test]
    fn donut_hit_test_matches_its_circle() {
        let frame = Frame::new((1100, 720), 1.0);
        let box_ = frame.donut_box();
        let cx = (box_.x0 + box_.x1) / 2.0;
        let cy = (box_.y0 + box_.y1) / 2.0;
        let radius = box_.width() / 2.0;
        assert!(frame.hits_donut(cx, cy), "centre must hit");
        // Corners of the bounding box fall outside the circle, so clicks there
        // are not stolen from whatever sits behind the donut.
        assert!(!frame.hits_donut(box_.x0 + 1.0, box_.y0 + 1.0));
        assert!(!frame.hits_donut(cx + radius + 2.0, cy));
        assert!(frame.hits_donut(cx + radius - 1.0, cy));
    }

    #[test]
    fn donut_never_hit_tests_outside_the_frame() {
        sweep(|frame| {
            assert!(!frame.hits_donut(-10.0, -10.0));
            assert!(!frame.hits_donut(frame.width + 10.0, frame.height + 10.0));
            // Never steals a press meant for the composer.
            let mid_well = (frame.composer_top + frame.composer_bottom) / 2.0;
            assert!(
                !frame.hits_donut((frame.left + frame.right) / 2.0, mid_well),
                "donut must not overlap the composer at {}x{}",
                frame.width,
                frame.height
            );
        });
    }
}
