//! # jcode-render-core
//!
//! Backend-neutral document/render model shared by jcode's front-ends (the
//! ratatui TUI and the desktop GPU UI).
//!
//! The pipeline is split at the seam where backends actually differ:
//!
//! ```text
//!   text ─▶ parse_markdown ─▶ Document (neutral blocks/spans) ─▶ wrap ─▶ adapter ─▶ backend draw
//! ```
//!
//! Everything up to and including wrapping is shared here. Each front-end owns
//! only a thin adapter: it resolves [`model::StyleRole`] to concrete colors and
//! turns [`model::StyledLine`]s into its own draw primitives (`ratatui::Line`
//! for the TUI, glyph runs for the desktop), and supplies a
//! [`wrap::WidthMeasure`] for its width units.
//!
//! This model is extracted from the *working* TUI markdown renderer
//! (`jcode-tui-markdown`); that renderer remains authoritative until this core
//! reaches parity, after which front-ends migrate onto it.

pub mod markdown;
pub mod model;
pub mod preprocess;
pub mod reasoning;
pub mod wrap;

pub use markdown::parse_markdown;
pub use model::{
    Alignment, Block, BlockKind, Document, FillRole, StyleRole, StyledLine, StyledSpan, TextAttrs,
};
pub use preprocess::escape_currency_dollars;
pub use reasoning::{
    REASONING_SENTINEL, reasoning_line_markup, reasoning_partial_markup,
    reasoning_summary_line_markup,
};
pub use wrap::{ColumnWidth, WidthMeasure, wrap_line, wrap_lines};

#[cfg(test)]
mod tests;
