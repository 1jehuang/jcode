//! The boot-up reveal: black paper, a donut growing into existence, then the
//! rest of the window.
//!
//! A window that snaps into existence fully drawn gives the eye nowhere to
//! start: the wordmark, the donut, the composer, and the chrome all arrive in
//! the same frame and compete. The boot sequence gives the frame an order to be
//! read in. It is three overlapping ramps:
//!
//! 1. **Paper.** The surface starts pure black and fades to the theme's
//!    background. The donut's ink fades the other way (white to the theme's
//!    text colour) along the same curve, so the torus is visible on black
//!    *and* on paper without a frame where it matches its background.
//! 2. **Donut.** Grows from a point at the centre of its box to full size,
//!    fading in, over [`DONUT_IN`].
//! 3. **Chrome.** Everything else (wordmark, tagline, session strip, composer,
//!    captions) fades in as one group once the donut has arrived, so the input
//!    box reads as being *created by* the donut rather than as a second thing
//!    that happened to appear.
//!
//! Like [`crate::stream`] and [`crate::caret`], the phase is derived from
//! (started, now) rather than driven by a timer thread, so a frame stays a pure
//! function of the model. A default `Boot` is *finished*, so every capture,
//! test, and state-space node renders the settled window and nothing has to
//! know this type exists.

use std::time::{Duration, Instant};
use vello::peniko::Color;

/// How long the donut takes to grow in. The whole point is that this is short:
/// long enough to be an entrance, short enough that it is over before anyone
/// who wanted to type has finished reaching for the keyboard.
pub const DONUT_IN: f64 = 0.30;
/// When the paper starts turning from black to the theme background. Slightly
/// after the donut starts, so the first thing on screen is genuinely a donut in
/// the dark.
pub const PAPER_AT: f64 = 0.10;
/// How long the paper takes to reach the theme background.
pub const PAPER_IN: f64 = 0.26;
/// When the rest of the window starts arriving: just before the donut lands, so
/// the two read as one motion rather than as a pause.
pub const CHROME_AT: f64 = 0.24;
/// How long the rest of the window takes to fade in.
pub const CHROME_IN: f64 = 0.26;

/// Total length of the sequence.
pub const TOTAL: f64 = CHROME_AT + CHROME_IN;

/// Frame interval requested while the sequence runs. Display-paced: this is the
/// first thing the user ever sees, so it is the last place to save power.
pub const FRAME: Duration = Duration::from_millis(8);

/// Smallest alpha the chrome is drawn at. Below this the layer is skipped
/// entirely, because Vello still pays for a layer that paints nothing.
const ALPHA_FLOOR: f64 = 0.004;

/// The boot reveal's clock.
#[derive(Clone, Copy, Debug, PartialEq)]
pub struct Boot {
    /// When the window first painted, or `None` once the sequence is over (or
    /// was never started, which is the resting state).
    started: Option<Instant>,
    /// Seconds since [`Self::started`], as of the last [`Self::advance`].
    /// Cached here rather than read from a clock in the renderer so a frame
    /// stays a pure function of the model.
    t: f64,
}

impl Default for Boot {
    /// Finished: nothing is mid-reveal, so a default `Boot` draws the settled
    /// window.
    fn default() -> Self {
        Self {
            started: None,
            t: f64::INFINITY,
        }
    }
}

impl Boot {
    /// Begin the sequence at `now`.
    pub fn start(now: Instant) -> Self {
        Self {
            started: Some(now),
            t: 0.0,
        }
    }

    /// A pinned phase, for captures and tests: `t` seconds into the sequence,
    /// with no clock behind it, so the frame is reproducible.
    pub fn pinned(t: f64) -> Self {
        Self { started: None, t }
    }

    /// Advance to `now`. Returns true while the sequence is still running, so
    /// the caller knows the frame is not the last one.
    pub fn advance(&mut self, now: Instant) -> bool {
        let Some(started) = self.started else {
            return false;
        };
        let t = now.saturating_duration_since(started).as_secs_f64();
        if t >= TOTAL {
            *self = Self::default();
            return false;
        }
        self.t = t;
        true
    }

    /// When the next frame is wanted, or `None` when the sequence is over.
    pub fn next_frame_at(&self, now: Instant) -> Option<Instant> {
        self.started.map(|_| now + FRAME)
    }

    /// Whether anything is still being revealed.
    pub fn is_running(&self) -> bool {
        self.started.is_some()
    }

    /// The donut's size and ink fraction, 0 at the start and 1 once it has
    /// arrived.
    pub fn donut(&self) -> f64 {
        ramp(self.t, 0.0, DONUT_IN)
    }

    /// How far the paper has turned from black to the theme background.
    pub fn paper(&self) -> f64 {
        ramp(self.t, PAPER_AT, PAPER_IN)
    }

    /// Alpha for everything that is not the donut.
    pub fn chrome(&self) -> f64 {
        ramp(self.t, CHROME_AT, CHROME_IN)
    }

    /// The page background for this frame: black at the start, the theme's own
    /// background once the paper has arrived.
    pub fn paper_color(&self, background: Color) -> Color {
        mix(Color::BLACK, background, self.paper())
    }

    /// The donut's ink for this frame: whichever of white and the theme's text
    /// colour contrasts better with the paper *behind it right now*. Alpha
    /// carries the donut's own fade-in.
    ///
    /// A cross-fade between the two would be the obvious implementation and is
    /// wrong: the two ends meet in the middle, so there is a stretch where ink
    /// and paper are the same mid grey and the donut disappears exactly when it
    /// is fully grown. Switching instead keeps it legible on every frame, and
    /// the switch is invisible because it lands on the frame where the two are
    /// equally visible anyway. Guarded by
    /// `ink_stays_visible_against_the_paper_behind_it`.
    pub fn donut_ink(&self, text: Color, background: Color) -> Color {
        // Squared, so the fade lags the growth a little: a full-alpha dot
        // lattice at 20% size reads as a small solid blob rather than as a
        // donut on its way in.
        let alpha = self.donut().powi(2) as f32 * text.components[3];
        let paper = luma(self.paper_color(background));
        let ink = if (luma(text) - paper).abs() >= (luma(Color::WHITE) - paper).abs() {
            text
        } else {
            Color::WHITE
        };
        ink.with_alpha(alpha)
    }

    /// Chrome alpha as a layer alpha, or `None` when no layer is needed
    /// (settled window) or nothing would be painted (opening frames).
    ///
    /// `Some(None)` is deliberately not in the vocabulary: the caller either
    /// draws normally, draws inside a layer, or skips.
    pub fn chrome_layer(&self) -> ChromeReveal {
        let alpha = self.chrome();
        if alpha <= ALPHA_FLOOR {
            ChromeReveal::Hidden
        } else if alpha >= 1.0 - ALPHA_FLOOR {
            ChromeReveal::Solid
        } else {
            ChromeReveal::Fading(alpha as f32)
        }
    }
}

/// How the non-donut half of the window should be drawn this frame.
#[derive(Clone, Copy, Debug, PartialEq)]
pub enum ChromeReveal {
    /// Not on screen yet: skip it.
    Hidden,
    /// Mid-fade: draw inside a layer at this alpha.
    Fading(f32),
    /// Settled: draw normally, no layer.
    Solid,
}

/// Rough perceptual brightness of a colour, for picking the ink that stands out
/// against it. Rec. 709 weights on the sRGB components: exact luminance would be
/// a gamma round trip for a choice between two colours that are at opposite ends
/// of the range.
fn luma(color: Color) -> f32 {
    let [r, g, b, _] = color.components;
    0.2126 * r + 0.7152 * g + 0.0722 * b
}

/// A smoothstep ramp from 0 to 1 over `[at, at + span)`.
fn ramp(t: f64, at: f64, span: f64) -> f64 {
    if !t.is_finite() {
        return 1.0;
    }
    let x = ((t - at) / span).clamp(0.0, 1.0);
    x * x * (3.0 - 2.0 * x)
}

/// Linear blend between two colours in sRGB space. Good enough here: both ends
/// of every blend are near-neutral, where a gamma-correct mix and a naive one
/// differ by less than the display can show.
fn mix(from: Color, to: Color, k: f64) -> Color {
    let k = k.clamp(0.0, 1.0) as f32;
    let a = from.components;
    let b = to.components;
    Color::new([
        a[0] + (b[0] - a[0]) * k,
        a[1] + (b[1] - a[1]) * k,
        a[2] + (b[2] - a[2]) * k,
        a[3] + (b[3] - a[3]) * k,
    ])
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn default_boot_is_finished() {
        let boot = Boot::default();
        assert!(!boot.is_running());
        assert_eq!(boot.donut(), 1.0);
        assert_eq!(boot.chrome(), 1.0);
        assert_eq!(boot.chrome_layer(), ChromeReveal::Solid);
        let paper = Color::from_rgb8(0xff, 0xff, 0xff);
        assert_eq!(boot.paper_color(paper), paper);
    }

    #[test]
    fn opens_black_with_a_growing_donut() {
        let now = Instant::now();
        let mut boot = Boot::start(now);
        assert!(boot.advance(now));
        // First frame: black paper, no donut yet, no chrome.
        let paper = boot.paper_color(Color::from_rgb8(0xff, 0xff, 0xff));
        assert!(paper.components[0] < 0.01, "opening frame must be black");
        assert_eq!(boot.donut(), 0.0);
        assert_eq!(boot.chrome_layer(), ChromeReveal::Hidden);

        // Mid-donut: the torus is on its way in, the chrome still is not.
        assert!(boot.advance(now + Duration::from_millis(150)));
        let grow = boot.donut();
        assert!(grow > 0.3 && grow < 1.0, "donut mid-growth, got {grow}");
        assert!(boot.chrome() < 0.1, "chrome must wait for the donut");

        // The donut lands before the chrome finishes arriving.
        assert!(boot.advance(now + Duration::from_millis(300)));
        assert_eq!(boot.donut(), 1.0);
        assert!(matches!(
            boot.chrome_layer(),
            ChromeReveal::Fading(_) | ChromeReveal::Solid
        ));
    }

    #[test]
    fn settles_and_stops_asking_for_frames() {
        let now = Instant::now();
        let mut boot = Boot::start(now);
        assert!(boot.next_frame_at(now).is_some());
        assert!(!boot.advance(now + Duration::from_secs_f64(TOTAL)));
        assert_eq!(boot, Boot::default());
        assert!(boot.next_frame_at(now).is_none());
    }

    #[test]
    fn ink_stays_visible_against_the_paper_behind_it() {
        // The failure this guards: ink blended to the theme's dark text while
        // the paper is still black, i.e. a donut that disappears mid-reveal.
        let now = Instant::now();
        let mut boot = Boot::start(now);
        let text = Color::from_rgb8(0x11, 0x11, 0x11);
        let bg = Color::from_rgb8(0xff, 0xff, 0xff);
        for ms in (0..=(TOTAL * 1000.0) as u64).step_by(8) {
            boot.advance(now + Duration::from_millis(ms));
            let ink = boot.donut_ink(text, bg);
            let paper = boot.paper_color(bg);
            let contrast = (luma(ink) - luma(paper)).abs();
            assert!(
                contrast > 0.25,
                "donut vanished into the paper at {ms}ms: contrast {contrast}"
            );
        }
    }
}
