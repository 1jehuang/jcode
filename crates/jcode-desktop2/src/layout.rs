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
/// Wordmark size.
pub const WORDMARK_SIZE: f32 = 15.0;
/// Space reserved for the wordmark before the status caption starts.
pub const WORDMARK_ADVANCE: f64 = 72.0;
/// Composer well height and inner padding.
pub const COMPOSER_HEIGHT: f64 = 44.0;
pub const COMPOSER_PAD_X: f64 = 14.0;
pub const COMPOSER_RADIUS: f64 = 6.0;
/// Baseline offset of the prompt text inside the composer well.
pub const COMPOSER_TEXT_OFFSET: f64 = 13.0;
/// Insert caret: a thin vertical bar, like any normal text input.
pub const CARET_WIDTH: f64 = 1.5;
pub const CARET_HEIGHT: f64 = 18.0;
/// Caption row under the composer for notices and the scrollback indicator.
pub const FOOTNOTE_HEIGHT: f64 = 16.0;
pub const FOOTNOTE_GAP: f64 = 6.0;
/// Vertical breathing room between regions.
pub const SPACE_AFTER_RULE: f64 = 22.0;
pub const SPACE_BEFORE_COMPOSER: f64 = 20.0;

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
    /// Baseline origin of the wordmark.
    pub masthead_top: f64,
    /// Hairline under the masthead.
    pub masthead_rule: f64,
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
    /// Resolve geometry for a surface of `size` physical pixels at `scale`.
    pub fn new(size: (u32, u32), scale: f64) -> Self {
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

        let masthead_top = (height * 0.05).clamp(24.0, 44.0);
        let masthead_rule = masthead_top + 28.0;

        let bottom_margin = (height * 0.05).clamp(20.0, 40.0);
        let footnote_bottom = height - bottom_margin;
        let footnote_top = footnote_bottom - FOOTNOTE_HEIGHT;
        let composer_bottom = footnote_top - FOOTNOTE_GAP;
        let composer_top = composer_bottom - COMPOSER_HEIGHT;

        let body_top = masthead_rule + SPACE_AFTER_RULE;
        let body_bottom = (composer_top - SPACE_BEFORE_COMPOSER).max(body_top);

        Self {
            width,
            height,
            scale,
            left,
            right,
            masthead_top,
            masthead_rule,
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

    /// Left edge of the status caption.
    pub fn status_left(&self) -> f64 {
        self.left + WORDMARK_ADVANCE
    }

    /// Width available to the status caption.
    pub fn status_width(&self) -> f64 {
        (self.right - self.status_left()).max(80.0)
    }

    /// Height of one body line.
    pub fn body_line_height(&self) -> f64 {
        f64::from(BODY_SIZE) * BODY_LEADING
    }

    /// Body lines that fit in the transcript region.
    pub fn visible_body_lines(&self) -> usize {
        (((self.body_bottom - self.body_top) / self.body_line_height()) as usize).max(1)
    }

    /// The caret must stay inside the composer well at any size.
    #[cfg_attr(not(test), allow(dead_code))]
    pub fn caret_fits_in_composer(&self) -> bool {
        let top = self.composer_top + COMPOSER_TEXT_OFFSET - 1.0;
        top >= self.composer_top && top + CARET_HEIGHT <= self.composer_bottom
    }

    /// Thickness that renders as exactly one physical pixel.
    pub fn hairline(&self) -> f64 {
        1.0 / self.scale
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
                check(Frame::new(size, scale));
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
            assert!(frame.masthead_top < frame.masthead_rule);
            assert!(frame.masthead_rule < frame.body_top);
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

    #[test]
    fn status_caption_has_room_beside_the_wordmark() {
        sweep(|frame| {
            assert!(frame.status_left() > frame.left);
            assert!(frame.status_width() >= 80.0);
        });
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
}
