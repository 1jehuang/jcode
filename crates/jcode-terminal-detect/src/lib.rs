//! Terminal emulator detection and escape sequence helpers.
//!
//! Detects which terminal emulator is running and what capabilities it
//! supports by reading environment variables. Provides escape-sequence
//! helpers so callers do not need to embed raw CSI/OSC strings.

mod capabilities;
mod detect;
mod emulator;
mod sequences;

pub use capabilities::{FeatureSet, ShellIntegrationLevel};
pub use detect::detect;
pub use emulator::TerminalEmulator;
pub use sequences::*;
