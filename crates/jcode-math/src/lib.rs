//! TeX-style math typesetting for jcode front-ends.
//!
//! `jcode-math` turns LaTeX math source into positioned glyphs and rules using
//! the OpenType MATH table of a real math font (STIX Two Math, vendored). It
//! deliberately stops at *positions*: it hands back a [`layout::LayoutBox`] of
//! glyph ids, sizes, and rectangles, and knows nothing about any renderer.
//!
//! The pipeline is the classical one:
//!
//! ```text
//! LaTeX  ->  parse (atom tree, TeX atom classes)
//!        ->  layout (boxes positioned with MATH constants)
//!        ->  glyphs + rules for a front-end to draw
//! ```
//!
//! ```
//! let engine = jcode_math::MathLayoutEngine::stix();
//! let formula = engine.layout_display(r"\frac{a + b}{c}", 20.0);
//! assert!(formula.width > 0.0);
//! ```

pub mod font;
pub mod layout;
pub mod parse;
pub mod symbols;

pub use font::{MathConstants, MathFontFace, STIX_TWO_MATH};
pub use layout::{Item, LayoutBox, MathLayoutEngine, MathLines, PlacedGlyph, PlacedRule, Style};
pub use parse::{Node, parse};
