//! Behavioral tests for the app shell.
//!
//! Split by concern so neither file grows unbounded: `actions` covers keyboard
//! and pointer dispatch, `visual` covers pixel-level invariants rendered
//! offscreen. See `docs/DESKTOP2_VISUAL_CHECKLIST.md` for the rules enforced.

mod actions;
mod visual;
