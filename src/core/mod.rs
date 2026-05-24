//! Core types, traits, and utilities for CarpAI
//!
//! This module consolidates foundational components that were previously
//! scattered across many small top-level modules.

pub mod error;
pub mod types;
pub mod traits;
pub mod id;
pub mod util;
pub mod platform;

// Re-export commonly used items
pub use error::{CarpAiError, Result};
pub use types::*;
pub use jcode_core::id::*;
pub use jcode_core::util::*;
