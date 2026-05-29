//! Low-level terminal image primitives for jcode.
//!
//! This crate has no dependency on the rest of jcode; it provides:
//! - [`display`]: terminal image rendering (Kitty / iTerm2 / Sixel) with a
//!   graceful no-protocol fallback.
//! - [`metadata`]: small pure helpers for formatting image dimensions, byte
//!   counts, formats, and estimating base64-decoded sizes.
//!
//! It is shared by the TUI and the `read` tool so neither has to depend on
//! the other just to display or describe an image.

pub mod display;
pub mod metadata;

pub use display::{ImageDisplayParams, ImageProtocol, display_image};
