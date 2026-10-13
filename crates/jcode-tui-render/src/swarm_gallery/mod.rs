//! Shared presentation logic for the inline swarm gallery.
//!
//! This is the single source of truth for how swarm-agent viewports look:
//! status accent colors, role glyphs, age formatting, the header line, member
//! sorting, and the gallery [`SwarmGalleryConfig`]. Both the live TUI adapter
//! (`jcode-tui`) and the `swarm_gallery_live` demo map their own data into
//! [`GalleryMember`] and call [`render_gallery`], so the demo renders identical
//! output to production and the two cannot drift.

mod card;
mod dock;
mod hover;
mod panel;
mod render;
mod strip;
mod strip_vertical;
pub(crate) mod types;
pub(crate) mod util;

// Re-export all public items so external callers can use
// `jcode_tui_render::swarm_gallery::*` unchanged.
pub use self::card::{render_swarm_chat_cards, render_swarm_live_card};
pub use self::dock::render_swarm_dock;
pub use self::panel::{render_swarm_compact, render_swarm_panel};
pub use self::render::{gallery_header, members_to_tiles, render_gallery};
pub use self::strip::{SwarmStripHint, render_swarm_strip};
pub use self::strip_vertical::render_swarm_strip_vertical;
pub use self::types::{GalleryMember, GalleryTodo, GalleryToolIntent};
pub use self::util::{
    STRIP_SPINNER_FPS, STRIP_SPINNER_FRAME_MS, STRIP_SPINNER_FRAMES, display_order, format_cost,
    format_elapsed_short, format_tokens, humanize_age, is_active_status, role_color, role_glyph,
    status_accent, status_glyph, summary_line, summary_line_from_members,
};

#[cfg(test)]
mod tests;
