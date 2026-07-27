//! Streaming reveal: how a reply arrives on screen.
//!
//! Tokens do not arrive at a human reading rate. They arrive in bursts, a
//! sentence at a time, separated by stalls, and if the transcript simply
//! renders whatever has been received the reply *flickers* into existence:
//! chunks pop, the conversation jumps a line at a time as the last message
//! grows, and the eye is dragged to a new position on every delta. That is the
//! whole of the "streaming looks bad" problem, and it is two problems:
//!
//! 1. **Text appears discontinuously.** Fixed by a *reveal cursor*: a
//!    fractional character position that chases the received length with an
//!    exponential ease, so a 200-character burst becomes a smooth sweep and a
//!    trickle stays honest. The ~[`FADE_CHARS`] characters at the leading edge
//!    ramp their alpha in (and drift up by [`RISE`]) rather than switching on,
//!    so even a single character enters continuously.
//!
//! 2. **The page jumps.** The transcript is bottom-aligned, so every new line
//!    of reply shoves everything up by a line height at once. Fixed by
//!    [`Stream::glide`]: a growth in content height is absorbed as a temporary
//!    scroll offset that decays away, turning a jump into a short glide.
//!
//! Both are derived from (state, now) rather than driven by a timer thread, so
//! a frame stays a pure function of the model, exactly like [`crate::caret`]
//! and [`crate::activity`]. A default `Stream` reveals everything, so tests and
//! captures that never call [`Stream::advance`] see fully drawn text.

use std::time::{Duration, Instant};

/// Time constant of the reveal's exponential chase, in seconds.
///
/// The cursor closes ~63% of the remaining distance per tau. Short enough that
/// the text never feels like a slow typewriter reading itself aloud, long
/// enough that a burst is a sweep rather than a pop.
const TAU: f64 = 0.10;

/// Floor on reveal speed, in characters per second. Pure exponential decay is
/// asymptotic, so the last few characters would crawl forever; this makes the
/// tail land.
const MIN_CPS: f64 = 90.0;

/// Ceiling on reveal speed, in characters per second. Without it a huge
/// non-streamed insertion (history replay, a paste) would "sweep" at thousands
/// of characters per frame, which is a flash, not an animation. Above this the
/// reveal is imperceptible anyway.
const MAX_CPS: f64 = 2600.0;

/// Width of the soft leading edge, in characters. The last glyphs of the
/// revealed text are partially faded, so the reply has a gradient tip instead
/// of a hard cut.
pub const FADE_CHARS: f64 = 26.0;

/// How far, in logical pixels, an entering glyph drifts up as it fades in.
/// Small on purpose: motion here is a hint that text is arriving, not an
/// effect.
pub const RISE: f64 = 3.0;

/// Alpha an entering glyph starts at. Not zero: a glyph that appears from
/// nothing reads as a flicker at small sizes, where the outer few percent of
/// coverage is most of what the eye sees.
pub const MIN_ALPHA: f32 = 0.0;

/// Time constant of the scroll glide, in seconds. Slower than the reveal: this
/// one is absorbing a whole line of movement, and a fast decay would be the
/// jump it exists to remove.
const GLIDE_TAU: f64 = 0.13;

/// Below this many logical pixels the glide is finished; carrying a sub-pixel
/// offset forever would keep the window animating for nothing.
const GLIDE_EPSILON: f64 = 0.25;

/// Frame interval requested while the reveal or glide is running. 120Hz-ish, so
/// the animation is limited by the display rather than by us.
pub const FRAME: Duration = Duration::from_millis(8);

/// The reveal cursor and scroll glide for the streaming reply.
#[derive(Clone, Copy, Debug, PartialEq)]
pub struct Stream {
    /// Characters revealed so far, fractional. `f64::INFINITY` means "reveal
    /// everything", which is the resting state and what a default `Stream`
    /// does, so nothing that does not animate has to know this type exists.
    revealed: f64,
    /// Characters received for the message being revealed.
    target: f64,
    /// Last time [`Self::advance`] ran, for the timestep.
    last: Option<Instant>,
    /// Outstanding scroll offset, in logical pixels, absorbing recent content
    /// growth. Positive means the view is held slightly *above* the tail.
    glide: f64,
    /// Content height at the last observation, to detect growth.
    content: Option<f64>,
}

impl Default for Stream {
    fn default() -> Self {
        Self {
            revealed: f64::INFINITY,
            target: 0.0,
            last: None,
            glide: 0.0,
            content: None,
        }
    }
}

impl Stream {
    /// Note that the streaming message now holds `chars` characters.
    ///
    /// The first delta of a reply starts the cursor at zero so the reply sweeps
    /// in; later deltas only extend the target, because restarting would replay
    /// text the user has already read.
    pub fn extend_to(&mut self, chars: usize, now: Instant) {
        let chars = chars as f64;
        if !self.revealed.is_finite() || chars < self.target {
            // A new reply, or the transcript was replaced under us.
            self.revealed = 0.0;
            self.last = Some(now);
        }
        self.target = chars;
    }

    /// A stream pinned mid-reveal, showing `fraction` of the arriving reply.
    /// State-space nodes and pixel tests use this so a partially revealed frame
    /// is a pure function of the model rather than of the clock.
    pub fn pinned(fraction: f64) -> Self {
        let target = 1000.0;
        Self {
            revealed: target * fraction.clamp(0.0, 1.0),
            target,
            last: None,
            glide: 0.0,
            content: None,
        }
    }

    /// Stop revealing: everything is shown from now on. Used when a turn is
    /// interrupted or a session is swapped, where continuing to animate text
    /// that is no longer arriving would just be a delay.
    pub fn reveal_all(&mut self) {
        self.revealed = f64::INFINITY;
        self.target = 0.0;
    }

    /// Whether a reveal is still in flight.
    pub fn is_revealing(&self) -> bool {
        self.revealed.is_finite() && self.revealed < self.target
    }

    /// Whether anything here wants another frame.
    pub fn is_animating(&self) -> bool {
        self.is_revealing() || self.glide.abs() >= GLIDE_EPSILON
    }

    /// Fraction of the streaming message that is revealed, in `0..=1`.
    ///
    /// A fraction rather than a character count because the cursor counts
    /// *markdown source* characters while the renderer draws *laid-out glyphs*,
    /// and the two differ by every `**` and backtick in the reply. Scaling by
    /// fraction keeps the leading edge in the right place without maintaining a
    /// source-to-glyph map that markdown makes ill-defined anyway.
    pub fn fraction(&self) -> f64 {
        if !self.is_revealing() || self.target <= 0.0 {
            return 1.0;
        }
        (self.revealed / self.target).clamp(0.0, 1.0)
    }

    /// Advance the reveal and decay the glide to `now`.
    pub fn advance(&mut self, now: Instant) {
        let dt = self
            .last
            .map(|last| now.saturating_duration_since(last).as_secs_f64())
            // Clamp: a stall, or a laptop waking from sleep, must not teleport
            // the reveal forward by a visible lurch.
            .map_or(0.0, |dt| dt.min(0.1));
        self.last = Some(now);
        if dt <= 0.0 {
            return;
        }
        if self.is_revealing() {
            let remaining = self.target - self.revealed;
            // Exponential chase, floored so the tail lands and capped so a
            // bulk insertion does not flash.
            let eased = remaining * (1.0 - (-dt / TAU).exp());
            let step = eased.max(MIN_CPS * dt).min(MAX_CPS * dt);
            self.revealed = (self.revealed + step).min(self.target);
        }
        // Same ease for the glide, in pixels.
        self.glide *= (-dt / GLIDE_TAU).exp();
        if self.glide.abs() < GLIDE_EPSILON {
            self.glide = 0.0;
        }
    }

    /// Observe the conversation's total height. Growth while the view is
    /// following the tail is absorbed into the glide, so the page slides up
    /// instead of jumping a line at a time.
    ///
    /// `at_tail` gates this: a user who has scrolled back is holding a
    /// position, and nudging it because the reply grew would be the app
    /// fighting them.
    pub fn observe_content(&mut self, height: f64, at_tail: bool) {
        let previous = self.content.replace(height);
        let Some(previous) = previous else { return };
        let growth = height - previous;
        // Only growth, and only plausible growth: a resize or a theme change
        // re-measures everything at once and must not be glided.
        if at_tail && growth > 0.0 && growth < MAX_GLIDE {
            self.glide = (self.glide + growth).min(MAX_GLIDE);
        } else if !at_tail || growth.abs() >= MAX_GLIDE {
            self.glide = 0.0;
        }
    }

    /// The scroll offset to add while gliding, in logical pixels.
    pub fn glide(&self) -> f64 {
        self.glide
    }

    /// When the loop must next wake for this animation, or `None` when it is at
    /// rest.
    pub fn next_frame_at(&self, now: Instant) -> Option<Instant> {
        self.is_animating().then(|| now + FRAME)
    }
}

/// Largest content growth absorbed as a glide, in logical pixels. Beyond about
/// this the movement is a relayout, not a reply growing, and sliding it would
/// be a long unrequested scroll.
const MAX_GLIDE: f64 = 90.0;

/// Alpha for a glyph at ordinal `index` when `revealed` glyphs are shown, or
/// `None` when the glyph is not yet on screen.
///
/// This is the leading-edge ramp: fully revealed text is opaque, the tip fades
/// out over [`FADE_CHARS`] glyphs, and anything past the cursor is absent.
pub fn glyph_alpha(index: f64, revealed: f64) -> Option<f32> {
    if !revealed.is_finite() {
        return Some(1.0);
    }
    let distance = revealed - index;
    if distance <= 0.0 {
        return None;
    }
    if distance >= FADE_CHARS {
        return Some(1.0);
    }
    let t = (distance / FADE_CHARS).clamp(0.0, 1.0);
    // Smoothstep: the ramp's ends are flat, so a glyph neither pops in at the
    // cursor nor visibly "arrives" at full opacity.
    let smooth = t * t * (3.0 - 2.0 * t);
    Some(MIN_ALPHA + (1.0 - MIN_ALPHA) * smooth as f32)
}

/// Upward drift, in logical pixels, for a glyph with `alpha`. Fully revealed
/// glyphs sit exactly on their laid-out baseline, so the text the user reads is
/// never off-grid.
pub fn glyph_rise(alpha: f32) -> f64 {
    RISE * f64::from(1.0 - alpha)
}

#[cfg(test)]
mod tests {
    use super::*;

    fn at(secs: f64) -> Instant {
        // A fixed origin plus an offset: tests must not depend on wall time.
        Instant::now() + Duration::from_secs_f64(secs)
    }

    /// The resting state draws everything. A model that never streams (a
    /// capture, a test, a replayed history) must not be silently blank.
    #[test]
    fn a_default_stream_reveals_everything() {
        let stream = Stream::default();
        assert!(!stream.is_revealing());
        assert_eq!(stream.fraction(), 1.0);
        assert_eq!(glyph_alpha(10_000.0, f64::INFINITY), Some(1.0));
    }

    /// The reveal chases the received text and lands on it, rather than
    /// approaching asymptotically forever.
    #[test]
    fn the_reveal_catches_up_and_finishes() {
        let start = Instant::now();
        let mut stream = Stream::default();
        stream.extend_to(400, start);
        assert!(stream.is_revealing());
        let mut now = start;
        for _ in 0..200 {
            now += Duration::from_millis(8);
            stream.advance(now);
        }
        assert!(!stream.is_revealing(), "reveal never finished");
        assert_eq!(stream.fraction(), 1.0);
    }

    /// A later delta extends the reveal instead of restarting it: text the user
    /// has already read must never be replayed.
    #[test]
    fn a_later_delta_does_not_rewind_the_cursor() {
        let start = Instant::now();
        let mut stream = Stream::default();
        stream.extend_to(100, start);
        stream.advance(start + Duration::from_millis(60));
        let progressed = stream.revealed;
        assert!(progressed > 0.0);
        stream.extend_to(180, start + Duration::from_millis(60));
        assert_eq!(stream.revealed, progressed, "cursor rewound on a delta");
    }

    /// Content growth at the tail becomes a decaying offset, not a jump.
    #[test]
    fn growth_glides_and_then_settles() {
        let start = Instant::now();
        let mut stream = Stream::default();
        stream.observe_content(100.0, true);
        stream.observe_content(120.0, true);
        assert!((stream.glide() - 20.0).abs() < 1e-9);
        let mut now = start;
        for _ in 0..100 {
            now += Duration::from_millis(8);
            stream.advance(now);
        }
        assert_eq!(stream.glide(), 0.0, "glide never settled");
    }

    /// A user who has scrolled back is not moved.
    #[test]
    fn scrolled_back_content_growth_does_not_glide() {
        let mut stream = Stream::default();
        stream.observe_content(100.0, false);
        stream.observe_content(400.0, false);
        assert_eq!(stream.glide(), 0.0);
    }

    /// The leading edge is a ramp, monotonic in position, and nothing past the
    /// cursor is drawn.
    #[test]
    fn the_leading_edge_ramps_monotonically() {
        let revealed = 100.0;
        assert_eq!(glyph_alpha(100.0, revealed), None, "past the cursor");
        assert_eq!(glyph_alpha(10.0, revealed), Some(1.0), "well behind");
        let near = glyph_alpha(99.0, revealed).expect("at the tip");
        let mid = glyph_alpha(90.0, revealed).expect("mid ramp");
        assert!(near < mid && mid < 1.0, "{near} {mid} not a ramp");
        assert!(glyph_rise(near) > glyph_rise(mid), "rise must follow alpha");
        assert_eq!(
            glyph_rise(1.0),
            0.0,
            "settled text must sit on its baseline"
        );
    }

    /// A bulk insertion is capped, so it animates rather than flashing, but a
    /// trickle is not slowed below the floor.
    #[test]
    fn the_reveal_speed_is_bounded_at_both_ends() {
        let start = Instant::now();
        let mut bulk = Stream::default();
        bulk.extend_to(1_000_000, start);
        bulk.advance(start + Duration::from_millis(16));
        assert!(bulk.revealed <= MAX_CPS * 0.016 + 1.0, "{}", bulk.revealed);

        // The floor is what makes a trickle land promptly: three characters
        // must be fully shown well inside a tenth of a second, or a slow reply
        // would read as laggy rather than as streaming.
        let mut trickle = Stream::default();
        trickle.extend_to(3, start);
        trickle.advance(start + Duration::from_millis(60));
        assert!(!trickle.is_revealing(), "a 3-char delta lingered");
        let _ = at(0.0);
    }
}
