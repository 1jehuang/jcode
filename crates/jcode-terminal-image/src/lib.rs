//! Low-level terminal image primitives for jcode.
//!
//! This crate has no dependency on the rest of jcode; it provides:
//! - [`display`]: terminal image rendering (Kitty / iTerm2 / Sixel) with a
//!   graceful no-protocol fallback.
//! - [`metadata`]: small pure helpers for formatting image dimensions, byte
//!   counts, formats, and estimating base64-decoded sizes.
//!
//! `display` is behind the `display` feature so only terminal clients pull in
//! crossterm. The shared runtime uses `metadata` alone.

#[cfg(feature = "display")]
pub mod display;
pub mod metadata;

#[cfg(feature = "display")]
pub use display::{ImageDisplayParams, ImageProtocol, display_image};
