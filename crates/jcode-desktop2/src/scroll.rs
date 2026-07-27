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
        self.lag.abs() >= EPSILON || self.alpha > ALPHA_EPSILON
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
}
