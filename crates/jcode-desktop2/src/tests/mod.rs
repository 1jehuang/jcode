//! Behavioral tests for the app shell.
//!
//! Split by concern so no file grows unbounded: `actions` covers keyboard and
//! pointer dispatch, `scheduling` covers the event loop's animation wakeups,
//! `visual` covers pixel-level invariants rendered offscreen, and
//! `strip_visual` covers the session strip's own pixel rules. See
//! `docs/DESKTOP2_VISUAL_CHECKLIST.md` for the rules enforced.

mod actions;
mod scheduling;
mod selection_visual;
mod strip_visual;
mod transcript_selection;
mod visual;
