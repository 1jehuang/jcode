//! Re-exports of the shared output-style helpers (emoji suppression, see #526).
//!
//! Items are re-exported explicitly rather than with a glob so the crate
//! boundary stays visible in the API surface.

pub use jcode_core::output_style::{
    emoji_enabled, replace_emoji_with_ascii, set_emoji_enabled, terminal_text,
    terminal_text_with_emoji,
};
