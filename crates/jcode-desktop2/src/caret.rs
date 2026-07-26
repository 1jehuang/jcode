//! Caret blink state.
//!
//! A normal input box shows an insert caret at all times: solid while you are
//! typing (so the caret never blinks out mid-keystroke and looks broken), then
//! blinking on a fixed phase once idle. Blink is derived from elapsed time
//! rather than a timer thread, so it is a pure function of
//! (last activity, now) and can be tested without sleeping.

use std::time::{Duration, Instant};

/// Full blink cycle: on for half, off for half.
pub const BLINK_PERIOD: Duration = Duration::from_millis(1060);
/// Grace period after a keystroke during which the caret stays solid.
///
/// Deliberately longer than half a blink period. If it were shorter or equal,
/// the grace period would be entirely covered by the blink's "on" phase and so
/// would have no observable effect: the caret would appear to stay solid while
/// typing purely by coincidence, and removing this rule would be undetectable.
/// A full period plus a margin means fast typists never see a blink.
pub const SOLID_AFTER_INPUT: Duration = Duration::from_millis(1200);

#[derive(Clone, Copy, Debug)]
pub struct Caret {
    last_activity: Instant,
    /// Pins visibility, ignoring the clock. State-space nodes use this so a
    /// captured frame is a pure function of the model: otherwise the caret
    /// blinks on wall-clock time and the same node renders differently
    /// depending on how long the GPU took.
    pinned: Option<bool>,
}

impl Default for Caret {
    fn default() -> Self {
        Self {
            last_activity: Instant::now(),
            pinned: None,
        }
    }
}

impl Caret {
    /// Call on every keystroke or cursor move: keeps the caret solid and
    /// restarts the blink phase from "visible".
    pub fn touch(&mut self) {
        self.last_activity = Instant::now();
        self.pinned = None;
    }

    /// Construct a caret with an explicit last-activity time, so blink phases
    /// are deterministic in state-space nodes and tests.
    pub fn with_activity(last_activity: Instant) -> Self {
        Self {
            last_activity,
            pinned: None,
        }
    }

    /// A caret pinned on or off, for deterministic rendering.
    pub fn pinned(visible: bool) -> Self {
        Self {
            last_activity: Instant::now(),
            pinned: Some(visible),
        }
    }

    /// Whether the caret is drawn at `now`.
    pub fn visible_at(&self, now: Instant) -> bool {
        if let Some(pinned) = self.pinned {
            return pinned;
        }
        let elapsed = now.saturating_duration_since(self.last_activity);
        if elapsed < SOLID_AFTER_INPUT {
            return true;
        }
        let phase = (elapsed.as_millis() % BLINK_PERIOD.as_millis()) as u64;
        phase < (BLINK_PERIOD.as_millis() as u64) / 2
    }

    pub fn visible(&self) -> bool {
        self.visible_at(Instant::now())
    }

    /// When the caret next changes state, so the event loop can schedule a
    /// redraw instead of spinning. `None` means no redraw is needed.
    pub fn next_toggle_at(&self, now: Instant) -> Option<Instant> {
        if self.pinned.is_some() {
            return None;
        }
        let elapsed = now.saturating_duration_since(self.last_activity);
        if elapsed < SOLID_AFTER_INPUT {
            return Some(self.last_activity + SOLID_AFTER_INPUT);
        }
        let half = BLINK_PERIOD.as_millis() / 2;
        let phase = elapsed.as_millis() % BLINK_PERIOD.as_millis();
        let remaining = if phase < half {
            half - phase
        } else {
            BLINK_PERIOD.as_millis() - phase
        };
        Some(now + Duration::from_millis(remaining as u64 + 1))
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn caret_is_solid_immediately_after_typing() {
        // Anchor to an explicit activity time so the samples are exact rather
        // than racing the clock: a sampling gap let a real bug through once.
        let start = Instant::now();
        let caret = Caret::with_activity(start);
        let step = 10u64;
        let grace = SOLID_AFTER_INPUT.as_millis() as u64;
        let mut ms = 0;
        while ms < grace {
            assert!(
                caret.visible_at(start + Duration::from_millis(ms)),
                "caret blinked out {ms}ms after input, inside the {grace}ms grace period"
            );
            ms += step;
        }
    }

    /// The grace period must be what actually keeps the caret solid, not the
    /// blink phase happening to be "on". Without this, removing the grace
    /// check entirely goes unnoticed.
    #[test]
    fn the_grace_period_is_what_holds_the_caret_solid() {
        let start = Instant::now();
        let caret = Caret::with_activity(start);
        let half = BLINK_PERIOD.as_millis() as u64 / 2;
        // Pick a moment inside the grace period that falls in the blink's
        // OFF phase: only the grace period can make it visible.
        let off_phase_moment = half + 5;
        assert!(
            off_phase_moment < SOLID_AFTER_INPUT.as_millis() as u64,
            "the grace period ({}ms) must outlast half a blink period ({half}ms), \
             or it has no observable effect",
            SOLID_AFTER_INPUT.as_millis()
        );
        assert!(
            caret.visible_at(start + Duration::from_millis(off_phase_moment)),
            "the caret went dark {off_phase_moment}ms after typing: the grace \
             period is not holding it solid"
        );
    }

    #[test]
    fn caret_blinks_once_idle() {
        // Derived from the constants rather than hardcoded, so tuning the blink
        // timing cannot silently invalidate this test.
        let start = Instant::now();
        let caret = Caret::with_activity(start);
        let at = |ms: u64| caret.visible_at(start + Duration::from_millis(ms));
        let grace = SOLID_AFTER_INPUT.as_millis() as u64;
        let period = BLINK_PERIOD.as_millis() as u64;
        let half = period / 2;
        // Walk several cycles past the grace period and confirm it alternates.
        let mut seen_on = false;
        let mut seen_off = false;
        for cycle in 0..3 {
            let base = grace + cycle * period;
            // Find the phase at `base`, then sample this cycle's two halves.
            let phase = base % period;
            let on_at = base + ((period - phase) % period) + 5;
            let off_at = on_at + half;
            assert!(at(on_at), "caret should be visible at {on_at}ms");
            assert!(!at(off_at), "caret should be dark at {off_at}ms");
            seen_on = true;
            seen_off = true;
        }
        assert!(seen_on && seen_off, "never observed a full blink cycle");
    }

    #[test]
    fn typing_restarts_the_blink_so_the_caret_is_visible() {
        let start = Instant::now();
        let mut caret = Caret::with_activity(start);
        // A moment well past the grace period, in the blink's off phase.
        let dark = SOLID_AFTER_INPUT.as_millis() as u64 + BLINK_PERIOD.as_millis() as u64 / 2 + 5;
        assert!(
            !caret.visible_at(start + Duration::from_millis(dark)),
            "expected the caret to be dark at {dark}ms"
        );
        caret.touch();
        assert!(caret.visible(), "caret was not visible right after typing");
    }

    #[test]
    fn a_pinned_caret_ignores_the_clock() {
        let start = Instant::now();
        for visible in [true, false] {
            let caret = Caret::pinned(visible);
            for ms in [0, 600, 1200, 5000] {
                assert_eq!(
                    caret.visible_at(start + Duration::from_millis(ms)),
                    visible,
                    "pinned caret changed at {ms}ms"
                );
            }
            assert_eq!(
                caret.next_toggle_at(start),
                None,
                "a pinned caret must not schedule redraws"
            );
        }
    }

    #[test]
    fn typing_unpins_the_caret_so_it_blinks_again() {
        let mut caret = Caret::pinned(false);
        caret.touch();
        assert!(caret.visible(), "caret stayed pinned off after typing");
        assert!(caret.next_toggle_at(Instant::now()).is_some());
    }

    #[test]
    fn blink_is_scheduled_rather_than_polled() {
        // The event loop needs a wake time; without one, blinking would
        // require a busy redraw loop.
        let start = Instant::now();
        let caret = Caret::with_activity(start);
        let toggle = caret.next_toggle_at(start).expect("no toggle scheduled");
        assert!(toggle > start, "toggle must be in the future");
        assert!(
            toggle <= start + SOLID_AFTER_INPUT + BLINK_PERIOD,
            "the next toggle should be within the grace period plus one blink"
        );
    }

    #[test]
    fn scheduled_toggle_actually_flips_visibility() {
        // Guards against a wake time that fires before the state changes,
        // which would redraw forever without the caret ever toggling.
        let start = Instant::now();
        let caret = Caret::with_activity(start);
        // Start past the grace period, where the caret is actually blinking.
        let mut now = start + SOLID_AFTER_INPUT;
        let mut visible = caret.visible_at(now);
        for _ in 0..4 {
            now = caret.next_toggle_at(now).expect("no toggle");
            let next = caret.visible_at(now);
            assert_ne!(visible, next, "visibility did not flip at the wake time");
            visible = next;
        }
    }
}
