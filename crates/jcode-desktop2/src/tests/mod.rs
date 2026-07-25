//! Behavioral tests for the app shell.
//!
//! Split by concern so no file grows unbounded: `actions` covers keyboard and
//! pointer dispatch, `scheduling` covers the event loop's animation wakeups,
//! and `visual` covers pixel-level invariants rendered offscreen. See
//! `docs/DESKTOP2_VISUAL_CHECKLIST.md` for the rules enforced.

mod actions;
mod scheduling;
mod visual;
