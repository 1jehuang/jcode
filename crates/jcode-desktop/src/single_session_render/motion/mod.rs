//! Motion/animation subsystem for single_session_render.
//!
//! Each submodule owns one animated surface's motion state machine
//! (visual snapshot, per-frame interpolation, and a registry keyed by a
//! stable content hash). Extracted from the parent render module.

mod scrollbar;
mod streaming_cue;

pub(crate) use scrollbar::*;
pub(crate) use streaming_cue::*;
