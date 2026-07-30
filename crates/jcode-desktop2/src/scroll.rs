//! Scroll smoothing and the scrollbar's visibility.
//!
//! The model's `scroll` is the *logical* position: where the user has asked the
//! conversation to sit. Moving it directly is correct but ugly, because a wheel
//! notch or a Page Up teleports the page by a chunk. This module holds the
//! difference between where the view is drawn and where it logically is, as a
//! `lag` that decays exponentially, so every scroll becomes a short glide and
//! the logical position stays exact (clamping, hit-testing and selection all
//! keep reading one unambiguous number).
//!
//! It also owns the scrollbar's alpha. A permanently visible bar is furniture;
//! one that appears while you scroll and fades out afterwards says the same
//! thing without competing with the text. Both are derived from (state, now),
//! like [`crate::caret`] and [`crate::stream`], so a frame stays a pure
//! function of the model.

use std::time::{Duration, Instant};

/// Time constant of the scroll ease, in seconds. One notch should land in
/// well under a tenth of a second: this is smoothing, not an animation the
/// user has to wait out.
const TAU: f64 = 0.055;

/// Time constant of the kinetic friction that bleeds a fling away, in seconds.
///
/// Chosen against [`crate::scroll_bench`] rather than against an impression:
/// paired with `MIN_VELOCITY` below it puts a flick's visible coast at about
/// 1.3s and roughly 8x the fingers' own travel, which is the range a browser
/// page fling lands in. The 0.18 this replaced measured 4x, and read as a page
/// that stops the moment you let go: a flick should cross a long reply, not
/// just finish the stroke.
const FRICTION_TAU: f64 = 0.32;

/// Below this speed, in logical pixels per second, a fling is over.
///
/// An exponential decay never reaches zero, so this cutoff, not the friction
/// alone, is what ends a coast. Too low and the page creeps for a second after
/// it has visibly stopped, repainting for motion nobody can see. 40px/s is
/// about a quarter of a line a second: slow enough that the tail of a fling
/// still drifts the way a browser's does, fast enough that the stop is not a
/// creep.
const MIN_VELOCITY: f64 = 60.0;

/// Ceiling on fling speed, in logical pixels per second. A frantic swipe should
/// travel far, not teleport past everything the user wanted to read.
const MAX_VELOCITY: f64 = 6_000.0;

/// A gesture is treated as still in progress while events keep arriving inside
/// this window; momentum only takes over once the fingers have left the pad.
///
/// This is a *fallback* for backends that do not report a gesture phase. Where
/// the phase is known (Wayland and X11 both send an axis stop, macOS sends
/// `Ended`), [`Smooth::end_gesture`] is what releases the fling, because a
/// timeout cannot tell a lifted finger from a slow drag: a user inching down a
/// long reply pauses for more than a frame all the time, and a timeout reads
/// every one of those pauses as a release and coasts on top of the finger's own
/// travel. So the fallback is generous, and the phase is authoritative.
const GESTURE_IDLE: Duration = Duration::from_millis(180);

/// A gesture held open this long with no events at all is treated as over,
/// whatever the backend claimed. Compositors do drop the axis stop, and a lost
/// stop must not pin the view's momentum for the rest of the session; half a
/// second is far longer than any real pause inside a moving drag.
const GESTURE_STALL: Duration = Duration::from_millis(500);

/// Velocity samples older than this belong to a previous gesture.
const SAMPLE_GAP: Duration = Duration::from_millis(120);

/// Shortest interval a single velocity sample may be measured over.
///
/// A gesture's speed is pixels over time, and the time has to be *real* time.
/// Two events can arrive with timestamps a few microseconds apart, either
/// because the compositor batched a burst into one wakeup or because libinput
/// coalesced them, and dividing one delta by that interval reports a speed the
/// finger never had: with a 1ms floor, an ordinary 12px event reads as
/// 12'000px/s and saturates `MAX_VELOCITY`. That is how a hand-speed drag
/// turned into a full-clamp fling on release, and it happened *more* on a 60Hz
/// display, because a longer frame batches more events per wakeup. So a sample
/// measured over less than this is accumulated rather than divided, and the
/// speed is taken once enough real time has passed to divide by.
const MIN_SAMPLE: Duration = Duration::from_millis(6);

/// Weight of the newest velocity sample in the running estimate. Low enough to
/// ignore one jittery frame, high enough to follow a real change of speed.
const VELOCITY_BLEND: f64 = 0.45;

/// Below this many logical pixels the ease is done. Sub-pixel lag would keep
/// the window repainting for something nobody can see.
const EPSILON: f64 = 0.2;

/// Largest lag carried, in logical pixels. A jump to the top of a long history
/// should still be immediate-ish rather than a long cinematic sweep.
const MAX_LAG: f64 = 260.0;

/// How long the scrollbar stays at full strength after the last scroll.
const HOLD: Duration = Duration::from_millis(650);

/// Time constant of the scrollbar's fade-out, in seconds.
const FADE_TAU: f64 = 0.22;

/// Below this the bar is gone.
const ALPHA_EPSILON: f64 = 0.01;

/// Frame interval requested while a scroll ease or a bar fade is running.
pub const FRAME: Duration = Duration::from_millis(8);

/// Smoothing state for the transcript scroll.
#[derive(Clone, Copy, Debug, Default, PartialEq)]
pub struct Smooth {
    /// Pixels the drawn view is behind the logical position. Positive means
    /// the view still has to travel up (toward older content).
    lag: f64,
    /// Scrollbar opacity in `0..=1`.
    alpha: f64,
    /// When the bar may start fading.
    hold_until: Option<Instant>,
    last: Option<Instant>,
    /// Estimated gesture speed in logical pixels per second, in the same sign
    /// convention as a scroll delta (positive travels toward older content).
    velocity: f64,
    /// When the most recent gesture event arrived.
    last_event: Option<Instant>,
    /// Start of the interval the pending velocity sample is accumulating over,
    /// and the travel accumulated in it. Present so a burst of events delivered
    /// in one wakeup is measured as one interval of real time rather than
    /// several impossibly short ones. See [`MIN_SAMPLE`].
    sample_since: Option<Instant>,
    sample_travel: f64,
    /// Whether the backend says the fingers are still on the surface.
    holding_gesture: bool,
    /// Whether the current input carries a usable gesture phase.
    ///
    /// Backends disagree, and the disagreement is not announced. A trackpad's
    /// pixel deltas come with a real end (Wayland's axis stop, macOS's
    /// `Ended`), so the phase can be believed. A discrete wheel's line deltas
    /// come with a hard-coded `Moved` that never ends, so believing the phase
    /// there would pin the view in a gesture that never closes. Only the
    /// pixel-delta path sets this.
    phase_known: bool,
    /// Momentum travel accumulated since the caller last collected it.
    pending: f64,
}

impl Smooth {
    /// Note a logical scroll of `delta` pixels (sign irrelevant): the view
    /// keeps its old position and catches up, and the scrollbar lights up.
    pub fn nudge(&mut self, delta: f64, now: Instant) {
        if delta != 0.0 {
            self.lag = (self.lag + delta).clamp(-MAX_LAG, MAX_LAG);
            self.last.get_or_insert(now);
        }
        self.show(now);
    }

    /// Note a continuous gesture of `delta` logical pixels, as a trackpad or a
    /// high-resolution wheel produces. The caller applies `delta` itself; this
    /// records how fast the surface is moving so the scroll keeps coasting
    /// after the fingers lift, the way a browser does.
    pub fn glide_from(&mut self, delta: f64, now: Instant) {
        if delta != 0.0 {
            // A gap this long means the previous gesture is over, so nothing
            // accumulated for it may be measured against this event.
            let stale = self
                .last_event
                .is_none_or(|prev| now.saturating_duration_since(prev) >= SAMPLE_GAP);
            if stale {
                self.sample_since = None;
                self.sample_travel = 0.0;
            }
            // Accumulate into the open interval, and only take a speed once
            // that interval is long enough to divide by. See `MIN_SAMPLE`.
            let opened = *self.sample_since.get_or_insert(now);
            self.sample_travel += delta;
            let span = now.saturating_duration_since(opened);
            let sample = (span >= MIN_SAMPLE).then(|| {
                let speed = self.sample_travel / span.as_secs_f64();
                self.sample_since = Some(now);
                self.sample_travel = 0.0;
                speed
            });
            self.velocity = match sample {
                Some(sample) if self.velocity.signum() == sample.signum() => {
                    (self.velocity * (1.0 - VELOCITY_BLEND) + sample * VELOCITY_BLEND)
                        .clamp(-MAX_VELOCITY, MAX_VELOCITY)
                }
                // A reversal is a new intent, not something to average with.
                Some(sample) => sample.clamp(-MAX_VELOCITY, MAX_VELOCITY),
                // Not enough real time yet to say how fast this is. The
                // previous estimate stands (it was measured over a full
                // interval of this same gesture); zeroing it here would throw
                // the speed away every time a burst arrived in one wakeup,
                // which is what a release then has to guess at.
                None if stale => 0.0,
                None => self.velocity,
            };
            self.last_event = Some(now);
            self.last.get_or_insert(now);
            // A new gesture event means anything still coasting from the last
            // one is stale: the user has taken hold of the page again, so the
            // old fling must not keep adding travel underneath them.
            self.pending = 0.0;
        }
        self.show(now);
    }

    /// Report the backend's own view of whether the fingers are still on the
    /// surface. `held` holds momentum off however long the user pauses, and
    /// clearing it releases the fling immediately, which is what the idle
    /// timeout can only guess at.
    ///
    /// Call this only where the phase is meaningful: see [`Self::phase_known`].
    pub fn gesture_held(&mut self, held: bool) {
        self.holding_gesture = held;
        self.phase_known = true;
    }

    /// Momentum travel owed to the logical scroll since the last call, in
    /// logical pixels. The caller applies it with its own clamping and reports
    /// a short fall via [`Smooth::stop`] when an edge swallowed it.
    pub fn take_momentum(&mut self) -> f64 {
        std::mem::take(&mut self.pending)
    }

    /// Whether a fling still owes the view travel.
    pub fn has_momentum(&self) -> bool {
        self.pending != 0.0
    }

    /// Kill the fling: the view has hit the top or the tail, and coasting into
    /// a wall keeps the window repainting for no visible movement.
    pub fn stop(&mut self) {
        self.velocity = 0.0;
        self.pending = 0.0;
    }

    /// Light the scrollbar without moving anything, e.g. while a drag holds a
    /// position at the edge.
    pub fn show(&mut self, now: Instant) {
        self.alpha = 1.0;
        self.hold_until = Some(now + HOLD);
    }

    /// A settled view with the scrollbar at full strength. Captures and pixel
    /// tests use this so the bar is a pure function of the model rather than
    /// of how recently a clock said the user scrolled.
    pub fn lit() -> Self {
        Self {
            alpha: 1.0,
            ..Self::default()
        }
    }

    /// Land immediately: used where a jump is the point (attaching to another
    /// session, clearing the transcript) and easing would replay history.
    pub fn settle(&mut self) {
        self.lag = 0.0;
        self.stop();
        self.holding_gesture = false;
        self.last_event = None;
        self.sample_since = None;
        self.sample_travel = 0.0;
    }

    /// Offset to subtract from the logical scroll when drawing.
    pub fn lag(&self) -> f64 {
        self.lag
    }

    /// Scrollbar opacity in `0..=1`.
    pub fn alpha(&self) -> f64 {
        self.alpha
    }

    pub fn is_animating(&self) -> bool {
        self.lag.abs() >= EPSILON
            || self.alpha > ALPHA_EPSILON
            || self.velocity.abs() >= MIN_VELOCITY
            || self.pending != 0.0
    }

    /// Decay the lag and the bar to `now`.
    pub fn advance(&mut self, now: Instant) {
        let dt = self
            .last
            .map(|last| now.saturating_duration_since(last).as_secs_f64())
            // A stall or a wake from sleep must not teleport the ease.
            .map_or(0.0, |dt| dt.min(0.1));
        self.last = Some(now);
        if dt <= 0.0 {
            return;
        }
        self.lag *= (-dt / TAU).exp();
        if self.lag.abs() < EPSILON {
            self.lag = 0.0;
        }
        // While the gesture is still under the user's fingers, their own deltas
        // move the view; integrating the estimate too would double the travel.
        let idle = |limit: Duration| {
            self.last_event
                .is_some_and(|at| now.saturating_duration_since(at) < limit)
        };
        // A known phase is authoritative in both directions: held means held
        // however long the pause, released means released even if the last
        // delta arrived a microsecond ago. The stall check is only a guard
        // against a dropped end, not a second opinion about the finger.
        let gesturing = if self.phase_known {
            self.holding_gesture && idle(GESTURE_STALL)
        } else {
            idle(GESTURE_IDLE)
        };
        if !gesturing && self.velocity != 0.0 {
            self.velocity *= (-dt / FRICTION_TAU).exp();
            if self.velocity.abs() < MIN_VELOCITY {
                self.stop();
            } else {
                self.pending += self.velocity * dt;
            }
        }
        let holding = self.hold_until.is_some_and(|until| now < until);
        if !holding && self.alpha > 0.0 {
            self.alpha *= (-dt / FADE_TAU).exp();
            if self.alpha < ALPHA_EPSILON {
                self.alpha = 0.0;
            }
        }
    }

    /// When the loop must next wake for this, or `None` when at rest.
    pub fn next_frame_at(&self, now: Instant) -> Option<Instant> {
        self.is_animating().then(|| now + FRAME)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    /// A burst of gesture events delivered in one compositor wakeup must
    /// estimate the hand's real speed, not one delta divided by the microseconds
    /// between two timestamps.
    ///
    /// This is the bug that made scrolling feel wrong on a 60Hz display while
    /// every headless number looked fine: a longer frame batches more events per
    /// wakeup, each near-zero interval saturated `MAX_VELOCITY`, and the release
    /// then flung the page at the clamp instead of at hand speed.
    #[test]
    fn a_batched_burst_is_not_read_as_a_flick() {
        let start = Instant::now();
        // Same gesture, same total travel, same wall-clock duration: 120px over
        // 60ms. Once delivered evenly, once as bursts of three events sharing a
        // wakeup. The estimated speed must be about the same, because the hand
        // moved identically.
        let even = {
            let mut smooth = Smooth::default();
            for step in 0..12 {
                smooth.gesture_held(true);
                smooth.glide_from(10.0, start + Duration::from_millis(step * 5));
            }
            smooth.velocity
        };
        let bursty = {
            let mut smooth = Smooth::default();
            for group in 0..4 {
                let at = start + Duration::from_millis(group * 15);
                for offset in 0..3 {
                    smooth.gesture_held(true);
                    // Microseconds apart, as a coalesced burst arrives.
                    smooth.glide_from(10.0, at + Duration::from_micros(offset * 20));
                }
            }
            smooth.velocity
        };
        // The real speed is 120px / 60ms = 2000px/s.
        assert!(
            (even - 2_000.0).abs() < 700.0,
            "evenly delivered gesture misread its own speed: {even:.0}px/s"
        );
        assert!(
            bursty < MAX_VELOCITY,
            "a batched burst saturated the clamp at {bursty:.0}px/s"
        );
        assert!(
            (bursty - even).abs() < 0.5 * even.abs().max(1.0),
            "the same gesture read as {even:.0}px/s evenly and {bursty:.0}px/s in bursts"
        );
    }

    /// The estimate must still follow a genuinely fast flick: the fix above
    /// must not have turned every gesture into a slow one.
    #[test]
    fn a_real_flick_still_reads_fast() {
        let start = Instant::now();
        let mut smooth = Smooth::default();
        for step in 0..8 {
            smooth.gesture_held(true);
            smooth.glide_from(60.0, start + Duration::from_millis(step * 8));
        }
        // 60px per 8ms is 7500px/s, above the clamp, so the estimate should sit
        // at or near it rather than be dragged down by the smoothing.
        assert!(
            smooth.velocity > 4_000.0,
            "a fast flick read as only {:.0}px/s",
            smooth.velocity
        );
    }

    #[test]
    fn a_scroll_lags_and_then_lands() {
        let start = Instant::now();
        let mut smooth = Smooth::default();
        smooth.nudge(80.0, start);
        assert!((smooth.lag() - 80.0).abs() < 1e-9);
        let mut now = start;
        for _ in 0..200 {
            now += FRAME;
            smooth.advance(now);
        }
        assert_eq!(smooth.lag(), 0.0, "scroll ease never settled");
    }

    #[test]
    fn the_bar_holds_then_fades() {
        let start = Instant::now();
        let mut smooth = Smooth::default();
        smooth.nudge(10.0, start);
        smooth.advance(start + Duration::from_millis(100));
        assert_eq!(smooth.alpha(), 1.0, "bar faded during the hold");
        let mut now = start;
        for _ in 0..400 {
            now += FRAME;
            smooth.advance(now);
        }
        assert_eq!(smooth.alpha(), 0.0, "bar never faded out");
        assert!(!smooth.is_animating());
    }

    #[test]
    fn a_huge_jump_is_not_a_long_sweep() {
        let start = Instant::now();
        let mut smooth = Smooth::default();
        smooth.nudge(10_000.0, start);
        assert!(smooth.lag() <= MAX_LAG);
    }

    /// A flick keeps travelling after the fingers leave the pad, and stops.
    #[test]
    fn a_flick_coasts_and_then_stops() {
        let start = Instant::now();
        let mut smooth = Smooth::default();
        let mut now = start;
        smooth.gesture_held(true);
        for _ in 0..6 {
            now += Duration::from_millis(8);
            smooth.glide_from(20.0, now);
        }
        assert_eq!(smooth.take_momentum(), 0.0, "coasted during the gesture");
        smooth.gesture_held(false);
        let mut coasted = 0.0;
        for _ in 0..400 {
            now += FRAME;
            smooth.advance(now);
            coasted += smooth.take_momentum();
        }
        assert!(coasted > 200.0, "flick barely coasted: {coasted}");
        assert!(!smooth.is_animating(), "fling never came to rest");
    }

    /// A finger resting on the pad mid-drag must not fling. This is the bug
    /// that made trackpad scrolling feel like it was fighting the hand: the
    /// release used to be guessed from event timing, so every pause inside a
    /// slow drag started a coast that added travel on top of the finger's own.
    #[test]
    fn a_paused_finger_does_not_fling() {
        let start = Instant::now();
        let mut smooth = Smooth::default();
        let mut now = start;
        smooth.gesture_held(true);
        for _ in 0..6 {
            now += Duration::from_millis(8);
            smooth.glide_from(30.0, now);
        }
        // The hand stops moving but stays down for a third of a second.
        let mut coasted = 0.0;
        for _ in 0..40 {
            now += FRAME;
            smooth.advance(now);
            coasted += smooth.take_momentum();
        }
        assert_eq!(coasted, 0.0, "a held finger flung the view by {coasted}");
    }

    /// Taking hold of the page again cancels the previous fling, rather than
    /// letting stale momentum add travel under the new gesture.
    #[test]
    fn a_new_gesture_cancels_the_old_fling() {
        let start = Instant::now();
        let mut smooth = Smooth::default();
        let mut now = start;
        smooth.gesture_held(true);
        for _ in 0..6 {
            now += Duration::from_millis(8);
            smooth.glide_from(40.0, now);
        }
        smooth.gesture_held(false);
        now += FRAME;
        smooth.advance(now);
        assert!(smooth.has_momentum(), "flick did not coast at all");
        now += FRAME;
        smooth.gesture_held(true);
        smooth.glide_from(-40.0, now);
        assert_eq!(
            smooth.take_momentum(),
            0.0,
            "the old fling survived into the new gesture"
        );
    }

    /// A single event carries no measurable speed, so it must not fling.
    #[test]
    fn one_event_does_not_fling() {
        let start = Instant::now();
        let mut smooth = Smooth::default();
        smooth.gesture_held(true);
        smooth.glide_from(20.0, start);
        smooth.gesture_held(false);
        let mut now = start;
        let mut coasted = 0.0;
        for _ in 0..40 {
            now += FRAME;
            smooth.advance(now);
            coasted += smooth.take_momentum();
        }
        assert_eq!(coasted, 0.0, "a lone event flung the view");
    }

    /// Hitting an edge ends the fling instead of grinding against the clamp.
    #[test]
    fn an_edge_ends_the_fling() {
        let start = Instant::now();
        let mut smooth = Smooth::default();
        let mut now = start;
        smooth.gesture_held(true);
        for _ in 0..6 {
            now += Duration::from_millis(8);
            smooth.glide_from(40.0, now);
        }
        smooth.gesture_held(false);
        now += Duration::from_millis(60);
        smooth.advance(now);
        assert!(smooth.take_momentum() > 0.0);
        smooth.stop();
        now += FRAME;
        smooth.advance(now);
        assert_eq!(smooth.take_momentum(), 0.0);
    }

    /// A backend that reports no phase at all still flings, via the timeout.
    /// Nothing on this desktop takes that path today, but the fallback is the
    /// only thing standing between a phase-less backend and a dead coast, so
    /// it is worth a test rather than a comment.
    #[test]
    fn a_phaseless_backend_still_flings() {
        let start = Instant::now();
        let mut smooth = Smooth::default();
        let mut now = start;
        for _ in 0..6 {
            now += Duration::from_millis(8);
            smooth.glide_from(40.0, now);
        }
        now += GESTURE_IDLE + FRAME;
        smooth.advance(now);
        assert!(smooth.take_momentum() > 0.0, "no fallback fling");
    }

    /// A dropped gesture end must not pin the view's momentum forever. The
    /// stall guard closes the gesture, so a compositor that swallows the axis
    /// stop costs the user a late fling rather than a dead one.
    #[test]
    fn a_dropped_gesture_end_still_releases() {
        let start = Instant::now();
        let mut smooth = Smooth::default();
        let mut now = start;
        smooth.gesture_held(true);
        for _ in 0..6 {
            now += Duration::from_millis(8);
            smooth.glide_from(40.0, now);
        }
        // No `gesture_held(false)` ever arrives.
        now += GESTURE_STALL + FRAME;
        smooth.advance(now);
        assert!(smooth.take_momentum() > 0.0, "a lost stop killed the fling");
    }
}
