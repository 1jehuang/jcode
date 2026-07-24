//! Design tokens from the Solo Systems style guide (~/jcode-website/STYLE.md).
//!
//! Print, not screen: two "colors" (ink and paper), grays are ink at lower
//! densities. Never introduce hue. Hierarchy from size and space alone.

use vello::peniko::Color;

pub const INK: Color = Color::from_rgb8(0x11, 0x11, 0x11);
pub const MUTED: Color = Color::from_rgb8(0x66, 0x66, 0x66);
pub const FAINT: Color = Color::from_rgb8(0x99, 0x99, 0x99);
pub const RULE: Color = Color::from_rgb8(0xcc, 0xcc, 0xcc);
pub const WASH: Color = Color::from_rgb8(0xf4, 0xf4, 0xf4);
pub const PAPER: Color = Color::from_rgb8(0xff, 0xff, 0xff);
