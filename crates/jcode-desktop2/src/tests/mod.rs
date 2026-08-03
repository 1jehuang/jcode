//! Behavioral tests for the app shell.
//!
//! Split by concern so no file grows unbounded: `actions` covers keyboard and
//! pointer dispatch, `scheduling` covers the event loop's animation wakeups,
//! `visual` covers pixel-level invariants rendered offscreen, `hero_visual`
//! covers the hero block's optical rules, and
//! `strip_visual` covers the session strip's own pixel rules. See
//! `docs/DESKTOP2_VISUAL_CHECKLIST.md` for the rules enforced.

mod actions;
mod delivery;
mod editor_selection;
mod failures;
mod hero_visual;
mod model_picker;
mod overview_gesture;
mod overview_visual;
mod page_bands;
mod progress;
mod scheduling;
mod selection_visual;
mod settings;
mod strip_visual;
mod transcript_selection;
mod visual;
