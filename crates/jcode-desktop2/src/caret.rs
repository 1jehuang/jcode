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
pub const SOLID_AFTER_INPUT: Duration = Duration::from_millis(530);

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
        let caret = Caret::default();
        let now = Instant::now();
        // Sampled across the whole grace period, the caret never blinks out
        // while the user is actively typing.
        for ms in [0, 100, 250, 500] {
            assert!(
                caret.visible_at(now + Duration::from_millis(ms)),
                "caret blinked out {ms}ms after input"
            );
        }
    }

    #[test]
    fn caret_blinks_once_idle() {
        let start = Instant::now();
        let caret = Caret::with_activity(start);
        let at = |ms: u64| caret.visible_at(start + Duration::from_millis(ms));
        // After the grace period the caret alternates on a fixed phase.
        assert!(!at(600), "caret should be off in the first off-phase");
        assert!(at(1100), "caret should return in the next on-phase");
        assert!(!at(1650), "caret should blink off again");
    }

    #[test]
    fn typing_restarts_the_blink_so_the_caret_is_visible() {
        let start = Instant::now();
        let mut caret = Caret::with_activity(start);
        assert!(!caret.visible_at(start + Duration::from_millis(600)));
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
            toggle <= start + BLINK_PERIOD,
            "toggle should be within one blink period"
        );
    }

    #[test]
    fn scheduled_toggle_actually_flips_visibility() {
        // Guards against a wake time that fires before the state changes,
        // which would redraw forever without the caret ever toggling.
        let start = Instant::now();
        let caret = Caret::with_activity(start);
        let mut now = start;
        let mut visible = caret.visible_at(now);
        for _ in 0..4 {
            now = caret.next_toggle_at(now).expect("no toggle");
            let next = caret.visible_at(now);
            assert_ne!(visible, next, "visibility did not flip at the wake time");
            visible = next;
        }
    }
}
