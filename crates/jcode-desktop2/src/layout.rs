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
/// Session strip: one row of bars at the very top, modelled on the waybar
/// `niri-workspaces` module. Fixed height, because it is chrome rather than
/// content and must not grow with the number of sessions.
pub const STRIP_HEIGHT: f64 = 14.0;
/// Gap between the strip and the transcript below it.
pub const STRIP_GAP: f64 = 10.0;
/// Width of one session block, and the gap between blocks in a group. Tuned to
/// read as the `|` ticks of the waybar module rather than as buttons.
pub const STRIP_BAR_WIDTH: f64 = 2.0;
/// The focused block is drawn wider, standing in for the module's `█` glyph.
pub const STRIP_BAR_FOCUS_WIDTH: f64 = 6.0;
pub const STRIP_BAR_GAP: f64 = 3.0;
/// Height of a block within the strip row.
pub const STRIP_BAR_HEIGHT: f64 = 8.0;
/// Padding between a group's outline and the blocks inside it. The outline is
/// what names the group now that no text does, so it needs enough air to read
/// as an enclosure rather than as a border on the first block. Blocks plus this
/// padding on both sides must stay inside `STRIP_HEIGHT`, or the enclosure
/// bleeds into the transcript's gap; `strip_owns_its_band` pins that.
pub const STRIP_FRAME_PAD: f64 = 2.0;
/// Corner radius of a group outline.
pub const STRIP_FRAME_RADIUS: f64 = 2.0;
/// Gap between one group and the next.
pub const STRIP_GROUP_GAP: f64 = 16.0;
/// The hero donut's side, in logical units. Matches the website's 360px hero
/// canvas, so the halftone screen has the same density there. It is one fixed
/// number rather than a cap: the donut is a mark, and a mark that breathes
/// with the window (or twitches when the top chrome row appears) reads as a
/// bug. A window that cannot hold it drops the hero instead of shrinking it.
pub const DONUT_SIDE: f64 = 360.0;
/// Hero wordmark over the donut, as on the website's landing section.
pub const HERO_WORDMARK_SIZE: f32 = 34.0;
/// Clear space under the wordmark, and under the donut before the tagline.
/// Measured from painted ink to painted ink, not from box to box, so it is the
/// gap the eye actually sees. Roughly half the wordmark's size, which is the
/// smallest gap that still reads as "separate thing" rather than "touching".
pub const HERO_GAP: f64 = 18.0;
/// Line height for hero text. Tight, because the hero stacks single lines
/// against a graphic: body leading would put invisible slack above each line
/// and make the measured gaps disagree with the optical ones.
pub const HERO_LINE_HEIGHT: f32 = 1.15;
/// Tagline under the donut: the one line that says what this is.
pub const HERO_TAGLINE_SIZE: f32 = 12.5;
/// Fraction of the donut's square its silhouette actually inks vertically, at
/// the *widest* point of the tilt wobble. The torus at this tilt does not reach
/// the square's top and bottom edges, so laying the stack out on the raw square
/// leaves a gap that looks like a mistake; the wordmark and tagline are spaced
/// against the *visible* disc instead. This is the worst case over the whole
/// animation, not the average: spacing against the average lets the disc grow
/// into the gap on the frames where it is tallest, which is exactly when the
/// text looks touched. Measured by `donut::tests::ink_extent_is_stable_across_
/// the_wobble`.
pub const DONUT_INK_FRACTION: f64 = 0.88;
/// The halftone screen paints a *disc* centred on each sampled cell, so the
/// painted silhouette reaches about one dot radius further than the cell grid
/// [`DONUT_INK_FRACTION`] measures. Counted as part of the donut so the gap the
/// eye sees is the gap the layout asked for. See `scene`'s `DOT_PITCH` and
/// `DOT_FILL`: 360/76 * 0.62 rounded up.
pub const DONUT_DOT_BLEED: f64 = 3.0;
/// Smallest square that still reads as a donut at [`DOT_PITCH`]. Nothing is
/// drawn between this and [`DONUT_SIDE`] now that the hero is fixed size, but
/// the renderer and the hit test keep it as their floor so a degenerate box
/// can never paint speckle.
pub const DONUT_MIN_SIDE: f64 = 100.0;
/// Vertical breathing room between regions.
pub const SPACE_BEFORE_COMPOSER: f64 = 20.0;
/// Fraction of the page height the input box is centred on. 0.5 puts the
/// composer in the exact middle of the window; it only leaves that line when
/// the page is too short to keep a transcript line above it.
pub const COMPOSER_CENTER: f64 = 0.5;

/// The hero block on an empty session: wordmark, donut, tagline. All fields are
/// logical pixels.
#[derive(Clone, Copy, Debug, PartialEq)]
pub struct Hero {
    /// Baseline box top of the wordmark, centred in the column.
    pub wordmark_top: f64,
    /// The donut's square.
    pub donut: vello::kurbo::Rect,
    /// Top of the tagline line under the donut.
    pub tagline_top: f64,
}

/// The empty band between one edge of the donut's square and the nearest
/// painted dot, at the tallest point of the wobble. The stack's gaps are laid
/// against this, so `HERO_GAP` is optical clearance rather than box padding.
pub fn donut_bleed(side: f64) -> f64 {
    (side * (1.0 - DONUT_INK_FRACTION) / 2.0 - DONUT_DOT_BLEED).max(0.0)
}

/// Height of the hero stack as laid on the page: the wordmark, the donut's
/// *inked* disc (the square's empty bleed bands are not demanded), and the
/// tagline, with their gaps. One definition shared by [`Frame::hero`]'s fit
/// check and [`Frame::resolve`]'s reservation, so the two can never disagree
/// about whether the stack fits.
fn hero_stack_height() -> f64 {
    let wordmark = f64::from(HERO_WORDMARK_SIZE * HERO_LINE_HEIGHT);
    let tagline = f64::from(HERO_TAGLINE_SIZE * HERO_LINE_HEIGHT);
    let bleed = donut_bleed(DONUT_SIDE);
    DONUT_SIDE - bleed * 2.0 + wordmark + tagline + HERO_GAP * 2.0
}

/// The transcript height [`Frame::resolve`] reserves for the hero on an empty
/// session. A hair over the stack itself, so floating-point placement cannot
/// round the available space to just under the stack and silently drop it.
fn hero_reservation() -> f64 {
    hero_stack_height() + 0.5
}

/// Slack allowed when judging whether the hero stack fits, in logical pixels.
///
/// The stack's top and bottom edges are whitespace (a gap and the disc's soft
/// silhouette), so a region a couple of pixels short of the stack loses
/// nothing visible. Without this, a page whose composer ceiling lands a
/// fraction of a pixel under the reservation drops the whole hero over slack
/// nobody can see.
const HERO_FIT_SLACK: f64 = 2.0;

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
    /// Top of the session strip row, when the strip is shown. `None` means
    /// there is no strip and nothing above was reserved for it.
    strip_top: Option<f64>,
}

impl Frame {
    /// Resolve geometry for a surface of `size` physical pixels at `scale`,
    /// with a single-line composer.
    pub fn new(size: (u32, u32), scale: f64) -> Self {
        Self::with_composer_lines(size, scale, 1)
    }

    /// Resolve geometry with a session strip reserved at the top.
    #[cfg_attr(not(test), allow(dead_code))]
    pub fn with_strip(size: (u32, u32), scale: f64, lines: usize, strip: bool) -> Self {
        Self::resolve(size, scale, lines, strip, 0.0)
    }

    /// Resolve geometry with a strip and a measured transcript height, so the
    /// composer can ride down the page as the conversation grows instead of
    /// staying pinned to the middle with a gap under the first reply.
    pub fn with_content(
        size: (u32, u32),
        scale: f64,
        lines: usize,
        strip: bool,
        content_height: f64,
    ) -> Self {
        Self::resolve(size, scale, lines, strip, content_height)
    }

    fn resolve(
        size: (u32, u32),
        scale: f64,
        lines: usize,
        strip: bool,
        content_height: f64,
    ) -> Self {
        // The strip's row comes out of the transcript's top margin, so the
        // content's own floor has to know about it before the composer is
        // placed. Resolve it on a probe frame first, then rebuild with the
        // real top so a growing transcript pushes the well down correctly.
        let strip_offset = if strip { STRIP_HEIGHT + STRIP_GAP } else { 0.0 };
        let build = |content: f64| {
            let mut frame = Self::with_composer_lines_and_content(size, scale, lines, content);
            // The strip takes its row out of the transcript's top margin, which
            // is dead space anyway, and only when there is something to show.
            // Nothing is reserved otherwise, so a single-session window is
            // byte-identical to one built before the strip existed.
            if strip {
                frame.strip_top = Some(frame.body_top);
                frame.body_top = (frame.body_top + STRIP_HEIGHT + STRIP_GAP).min(frame.body_bottom);
            }
            frame
        };
        if content_height > 0.0 {
            return build(content_height + strip_offset);
        }
        // An empty session is the hero's page, so reserve its stack the same
        // way a transcript reserves its height: the composer sits just under
        // the wordmark/donut/tagline column, exactly like the input under the
        // website's landing hero. Centring the composer instead starved the
        // hero of room on ordinary laptop windows (a 720-tall page has ~280
        // logical pixels above a centred well; the stack needs ~377), which
        // silently dropped the donut at the default window size.
        //
        // Checked against the built frame's own `hero()` rather than by
        // repeating its fit arithmetic here: the two can then never disagree.
        // A window too short or too narrow for the stack falls back to the
        // centred composer with no hero, as before.
        let reserved = build(hero_reservation() + strip_offset);
        if reserved.hero().is_some() {
            return reserved;
        }
        build(strip_offset)
    }

    /// Resolve geometry with a composer sized for `lines` of input. The
    /// composer grows upward so the transcript shrinks instead of the input
    /// being clipped, and stops growing at [`COMPOSER_MAX_LINES`] so a long
    /// paste can never push the transcript off the page.
    pub fn with_composer_lines(size: (u32, u32), scale: f64, lines: usize) -> Self {
        Self::resolve(size, scale, lines, false, 0.0)
    }

    /// As [`Self::with_composer_lines`], with the measured height of the
    /// transcript. A conversation taller than the space above the centre line
    /// pushes the composer down, so the input always sits just under the last
    /// reply rather than leaving a hole between them.
    pub fn with_composer_lines_and_content(
        size: (u32, u32),
        scale: f64,
        lines: usize,
        content_height: f64,
    ) -> Self {
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
        // ...until the conversation is taller than that. Then the well rides
        // down with the last reply, the way a chat log fills a page top-down,
        // and stops at the floor once the transcript fills the window.
        let content = content_height.max(0.0);
        let followed = if content > 0.0 {
            centred.max(body_top + content + SPACE_BEFORE_COMPOSER)
        } else {
            centred
        };
        let ceiling = (slot_bottom - composer_height).max(body_top);
        let composer_top = followed.clamp(min_top.min(ceiling), ceiling);
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
            strip_top: None,
        }
    }

    /// Width of the measure column.
    pub fn column(&self) -> f64 {
        self.right - self.left
    }

    /// The session strip's row, in logical units, or `None` when no strip is
    /// shown. Returned as a band rather than a top so callers cannot invent
    /// their own height and drift from the reserved space.
    pub fn strip(&self) -> Option<(f64, f64)> {
        self.strip_top.map(|top| (top, top + STRIP_HEIGHT))
    }

    /// Height of one body line.
    pub fn body_line_height(&self) -> f64 {
        f64::from(BODY_SIZE) * BODY_LEADING
    }

    /// Width available to composer text, inside the well's padding. This is
    /// the wrap width handed to Parley, so the text wraps exactly where the
    /// well ends rather than at an estimated character count.
    pub fn composer_text_width(&self) -> f64 {
        (self.right - COMPOSER_PAD_X - self.composer_text_left()).max(1.0)
    }

    /// Left edge of composer text: inside the well's padding. The single source
    /// of truth for the text origin, so drawing, caret geometry, and click
    /// hit-testing cannot drift apart.
    ///
    /// There is no prompt marker: the outlined field already says where typing
    /// goes, so a `>` chevron was decoration that pushed the text off the
    /// field's own optical left edge.
    pub fn composer_text_left(&self) -> f64 {
        self.left + COMPOSER_PAD_X
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

    /// The hero block shown on an empty session: the wordmark, the donut, and
    /// the tagline, stacked and centred exactly like the website's landing
    /// section. Returns `None` when the window is too short to hold it, so a
    /// cramped frame degrades to a plain composer instead of a squashed hero.
    ///
    /// The hero borrows the transcript's dead space and reserves nothing, so
    /// nothing else in the frame moves when it appears or stands down.
    pub fn hero(&self) -> Option<Hero> {
        let available = self.body_bottom - self.body_top;
        let wordmark_height = f64::from(HERO_WORDMARK_SIZE * HERO_LINE_HEIGHT);
        // Fixed side: the hero either fits at its one true size or is not
        // drawn. Fitting it by scaling made the donut jump whenever the frame
        // changed height, e.g. when the top chrome row appeared.
        let side = DONUT_SIDE;
        // Space the text against the inked disc, not the square, then centre
        // the whole stack in the region like the website's flexbox.
        let bleed = donut_bleed(side);
        // Fit is judged on the inked stack, not the raw square: the square's
        // top and bottom `bleed` bands are empty, so demanding room for them
        // would drop the hero on ordinary laptop windows for the sake of
        // whitespace.
        let total = hero_stack_height();
        if total > available + HERO_FIT_SLACK || self.column() < side {
            return None;
        }
        let top = self.body_top + (available - total) / 2.0;
        let centre_x = (self.left + self.right) / 2.0;
        let donut_top = top + wordmark_height + HERO_GAP - bleed;
        Some(Hero {
            wordmark_top: top,
            donut: vello::kurbo::Rect::new(
                centre_x - side / 2.0,
                donut_top,
                centre_x + side / 2.0,
                donut_top + side,
            ),
            tagline_top: donut_top + side - bleed + HERO_GAP,
        })
    }

    /// Whether a logical point is inside the donut, used for drag hit-testing.
    /// Circular, not the bounding box, so clicks in the corners still reach
    /// whatever is behind it.
    pub fn hits_donut(&self, x: f64, y: f64) -> bool {
        let Some(box_) = self.hero().map(|hero| hero.donut) else {
            return false;
        };
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

    /// The same sweep, with the session strip shown.
    fn sweep_with_strip(mut check: impl FnMut(Frame)) {
        for &size in SIZES {
            for &scale in SCALES {
                for lines in [1usize, 2, 4, COMPOSER_MAX_LINES] {
                    check(Frame::with_strip(size, scale, lines, true));
                }
            }
        }
    }

    /// R1: the strip owns a band of its own at the top and can never collide
    /// with the transcript or the composer, at any window size or DPI.
    #[test]
    fn strip_band_never_overlaps_body_or_composer() {
        sweep_with_strip(|frame| {
            let (top, bottom) = frame.strip().expect("strip was requested but absent");
            assert!(top >= 0.0, "strip started off-paper at {top}");
            assert!(bottom > top, "strip band inverted");
            assert!(
                bottom <= frame.body_top + 1e-9,
                "strip ({bottom}) ran into the transcript ({})",
                frame.body_top
            );
            assert!(
                bottom < frame.composer_top,
                "strip ran into the composer at {}x{}",
                frame.width,
                frame.height
            );
        });
    }

    /// R2: with nothing worth showing, the strip must not merely be invisible
    /// but absent, leaving the rest of the frame bit-for-bit as it was.
    #[test]
    fn strip_is_absent_when_not_requested() {
        for &size in SIZES {
            for &scale in SCALES {
                let without = Frame::with_strip(size, scale, 1, false);
                let before = Frame::new(size, scale);
                assert_eq!(without.strip(), None);
                assert_eq!(
                    without.body_top, before.body_top,
                    "a hidden strip still stole space"
                );
                assert_eq!(without.body_bottom, before.body_bottom);
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
        // A window too short for the hero keeps the original centred layout:
        // the well grows symmetrically about the page's middle line.
        let size = (1100, 480);
        let one = Frame::with_composer_lines(size, 1.0, 1);
        assert!(one.hero().is_none(), "this size must not hold a hero");
        let three = Frame::with_composer_lines(size, 1.0, 3);
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

        // On a hero page the well's top is seated under the stack, so growth
        // goes downward: the hero must not be squeezed by a longer input.
        let one = Frame::with_composer_lines((1100, 720), 1.0, 1);
        assert!(one.hero().is_some(), "the default window must hold a hero");
        let three = Frame::with_composer_lines((1100, 720), 1.0, 3);
        assert_eq!(
            one.composer_top, three.composer_top,
            "a longer input moved the hero page's composer top"
        );
        assert!(
            three.composer_bottom > one.composer_bottom,
            "the composer did not grow for more lines on the hero page"
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
                    // An empty session seats the well under the hero stack when
                    // the stack does not fit above the centre line; that only
                    // ever moves the well *down*, like a transcript would.
                    let hero_seated = frame.hero().is_some() && center > page;
                    assert!(
                        (center - page).abs() < 0.001 || clamped_low || clamped_high || hero_seated,
                        "composer centre {center} left the page middle {page} unclamped at {}x{}",
                        frame.width,
                        frame.height
                    );
                }
            }
        }
    }

    /// The composer follows the conversation down the page: with an empty
    /// transcript it is centred, and as content accumulates it sits just under
    /// the last message rather than leaving a hole above itself.
    #[test]
    fn the_composer_follows_a_growing_transcript_down_the_page() {
        let size = (1400u32, 1000u32);
        let empty = Frame::with_content(size, 1.0, 1, false, 0.0);
        let mut last = empty.composer_top;
        for content in [0.0f64, 100.0, 300.0, 500.0, 900.0] {
            let frame = Frame::with_content(size, 1.0, 1, false, content);
            assert!(
                frame.composer_top >= last - 0.001,
                "composer moved back up as the transcript grew: \
                 {content} gave {} after {last}",
                frame.composer_top
            );
            last = frame.composer_top;
        }
        assert!(
            last > empty.composer_top,
            "a tall transcript never pushed the composer below centre"
        );
    }

    /// A short transcript must not move the well at all: the centred hero
    /// layout is the resting state, and only content that would collide with
    /// the well is allowed to displace it.
    #[test]
    fn a_short_transcript_leaves_the_composer_centred() {
        let size = (1400u32, 1000u32);
        let empty = Frame::with_content(size, 1.0, 1, false, 0.0);
        let short = Frame::with_content(size, 1.0, 1, false, 40.0);
        assert_eq!(empty.composer_top, short.composer_top);
    }

    /// However tall the conversation, the composer stops at its floor and the
    /// frame keeps every ordering invariant.
    #[test]
    fn a_huge_transcript_never_pushes_the_composer_off_the_page() {
        for &size in SIZES {
            for &scale in SCALES {
                for strip in [false, true] {
                    for lines in [1usize, 4, COMPOSER_MAX_LINES] {
                        let frame = Frame::with_content(size, scale, lines, strip, 100_000.0);
                        assert!(frame.body_top <= frame.body_bottom);
                        assert!(frame.body_bottom <= frame.composer_top + 0.001);
                        assert!(frame.composer_bottom <= frame.footnote_top + 0.001);
                        assert!(
                            frame.footnote_bottom <= frame.height + 0.001,
                            "a tall transcript pushed the caption row off-paper at {}x{}",
                            frame.width,
                            frame.height
                        );
                        if let Some((_, bottom)) = frame.strip() {
                            assert!(bottom <= frame.body_top + 1e-9);
                        }
                    }
                }
            }
        }
    }

    #[test]
    fn a_roomy_page_centres_the_composer_exactly() {
        for &size in &[(1100u32, 720u32), (1440, 900), (1920, 1080), (2560, 1440)] {
            let frame = Frame::with_composer_lines(size, 1.0, 1);
            let center = (frame.composer_top + frame.composer_bottom) / 2.0;
            let centred = (center - frame.height / 2.0).abs() < 0.001;
            // A roomy page is also a hero page: when the stack does not fit
            // above the centre line, the well is seated exactly under the
            // reserved stack instead. Anything else is drift.
            let seated = (frame.composer_top
                - (frame.body_top + hero_reservation() + SPACE_BEFORE_COMPOSER))
                .abs()
                < 0.001;
            assert!(
                centred || seated,
                "composer neither centred nor seated under the hero at {}x{}: {center}",
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

    /// The transcript region must always have room for at least one line of
    /// body copy, or a reply has nowhere to go at any window size.
    #[test]
    fn transcript_always_shows_at_least_one_line() {
        sweep(|frame| {
            assert!(frame.body_bottom - frame.body_top >= frame.body_line_height());
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
            // A window too small for the hero draws no donut at all, and an
            // empty rect has no position to check. Asserting on it instead
            // tested the placeholder rather than the layout.
            let Some(box_) = frame.hero().map(|hero| hero.donut) else {
                return;
            };
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
            let Some(box_) = frame.hero().map(|hero| hero.donut) else {
                return;
            };
            assert!(
                (box_.width() - box_.height()).abs() < 1e-9,
                "donut must be square"
            );
            assert!(
                (box_.width() - DONUT_SIDE).abs() < 1e-9,
                "the donut must always be DONUT_SIDE"
            );
            assert!(box_.width() >= 0.0);
        });
    }

    #[test]
    fn donut_is_centred_in_the_column() {
        let mut checked = 0;
        sweep(|frame| {
            let Some(box_) = frame.hero().map(|hero| hero.donut) else {
                return;
            };
            let centre = (box_.x0 + box_.x1) / 2.0;
            assert!(
                (centre - (frame.left + frame.right) / 2.0).abs() < 1e-9,
                "donut off-centre at {}x{} scale {}",
                frame.width,
                frame.height,
                frame.scale
            );
            checked += 1;
        });
        // Without this the test would pass vacuously if the hero stopped
        // fitting at every size in the sweep.
        assert!(checked > 0, "the sweep never produced a hero to check");
    }

    /// The hero is optional by design, but it must appear at ordinary desktop
    /// window sizes: silently losing it everywhere would be invisible to the
    /// tests above, which skip frames without one.
    ///
    /// Sizes here are *logical*, scaled up to physical per scale factor, so the
    /// same window is tested on 1x and HiDPI rather than a 1x window being
    /// shrunk by the scale factor.
    #[test]
    fn the_hero_fits_at_ordinary_window_sizes() {
        for &(w, h) in &[(1100.0f64, 720.0f64), (1440.0, 900.0), (1920.0, 1080.0)] {
            for &scale in SCALES {
                let size = ((w * scale) as u32, (h * scale) as u32);
                let frame = Frame::with_composer_lines(size, scale, 1);
                assert!(
                    frame.hero().is_some(),
                    "no hero in a {w}x{h} logical window at scale {scale}"
                );
            }
        }
    }

    #[test]
    fn donut_hit_test_matches_its_circle() {
        let frame = Frame::new((1100, 720), 1.0);
        let box_ = frame.hero().expect("a hero at 1100x720").donut;
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
