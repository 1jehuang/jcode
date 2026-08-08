//! Policy for the decorative idle-animation partial repaint.
//!
//! Split out of `run_shell.rs` so the pure "may this tick be served cheaply?"
//! decision and the targeted cell copy live next to the tests that pin them,
//! instead of inside the already-oversized run-loop module.

use ratatui::buffer::Buffer;
use ratatui::layout::Rect;
use std::time::Duration;

/// Cadence for chrome that is "live" but not animated: notification/status
/// lines, cache countdowns, and similar text. These change on the order of a
/// second, so repainting them at the idle rate is imperceptible, while the
/// decorative animation in between rides the cheap partial-repaint path.
pub(super) const IDLE_ANIMATION_CHROME_FULL_FRAME_INTERVAL: Duration = Duration::from_millis(250);

/// Whether an animation tick may be served by the partial repaint.
///
/// With nothing else live this is always allowed. When other chrome also wants
/// repaints, the naive answer ("defer to the full frame") is wrong in practice:
/// an idle screen almost always has some slow-moving chrome up (a notification
/// line, a status notice, a cache countdown), so deferring drags every single
/// animation tick back onto the full-frame path and reintroduces the lag this
/// exists to remove. Instead that chrome gets a full frame at its own
/// human-scale cadence, and the animation frames in between come from the
/// cheap path.
pub(super) fn idle_animation_partial_repaint_allowed(
    other_redraw_required: bool,
    since_last_full_frame: Option<Duration>,
) -> bool {
    if !other_redraw_required {
        return true;
    }
    since_last_full_frame.is_some_and(|elapsed| elapsed < IDLE_ANIMATION_CHROME_FULL_FRAME_INTERVAL)
}

/// Copy the cells of `area` from `src` into `dst`, leaving every other cell
/// untouched.
///
/// The targeted alternative to `Buffer::clone_from` for the animation-only
/// repaint, which only ever changes one rectangle.
pub(crate) fn copy_cells_in(src: &Buffer, dst: &mut Buffer, area: Rect) {
    let area = area.intersection(*src.area()).intersection(*dst.area());
    for y in area.top()..area.bottom() {
        for x in area.left()..area.right() {
            dst[(x, y)] = src[(x, y)].clone();
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    /// With nothing else live, every animation tick takes the cheap path.
    #[test]
    fn quiet_idle_screen_serves_every_animation_tick_from_the_partial_repaint() {
        for since in [None, Some(Duration::ZERO), Some(Duration::from_secs(60))] {
            assert!(
                idle_animation_partial_repaint_allowed(false, since),
                "a quiet idle screen must never pay for a full frame (since={since:?})"
            );
        }
    }

    #[test]
    fn slow_chrome_gets_periodic_full_frames_without_pacing_the_animation() {
        // Just after a full frame: chrome is already up to date, so the
        // animation rides the cheap path.
        assert!(idle_animation_partial_repaint_allowed(
            true,
            Some(Duration::ZERO)
        ));
        assert!(idle_animation_partial_repaint_allowed(
            true,
            Some(IDLE_ANIMATION_CHROME_FULL_FRAME_INTERVAL - Duration::from_millis(1))
        ));

        // Once the chrome interval lapses, it earns a full frame.
        assert!(!idle_animation_partial_repaint_allowed(
            true,
            Some(IDLE_ANIMATION_CHROME_FULL_FRAME_INTERVAL)
        ));
        assert!(!idle_animation_partial_repaint_allowed(
            true,
            Some(Duration::from_secs(5))
        ));

        // No full frame drawn yet: there is nothing to patch over.
        assert!(!idle_animation_partial_repaint_allowed(true, None));
    }

    /// The chrome cadence must stay far slower than animation FPS, otherwise the
    /// fast path saves nothing. At 60fps animation and a 250ms chrome interval,
    /// at most ~1 in 15 ticks is a full frame.
    #[test]
    fn chrome_full_frames_are_a_small_fraction_of_animation_ticks() {
        let animation_tick = Duration::from_millis(1000 / 60);
        let ticks_per_chrome_frame =
            IDLE_ANIMATION_CHROME_FULL_FRAME_INTERVAL.as_secs_f64() / animation_tick.as_secs_f64();
        assert!(
            ticks_per_chrome_frame >= 10.0,
            "chrome would repaint every {ticks_per_chrome_frame:.1} animation ticks, \
             which defeats the partial-repaint path"
        );
    }
}
