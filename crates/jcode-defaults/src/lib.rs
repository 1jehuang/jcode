//! jcode-defaults: Zero-Configuration Startup System
//!
//! This crate provides intelligent defaults and auto-discovery mechanisms
//! for jcode, enabling true "out-of-the-box" experience like Cursor.
//!
//! ## Design Philosophy
//!
//! **Cursor's Success Formula**:
//! - Download → Install → Open VS Code → Works Immediately
//! - No config files needed for basic usage
//! - Smart defaults that work for 80% of users
//! - Progressive disclosure of advanced options
//!
//! **jcode's Approach**:
//! - Same zero-config startup
//! - Auto-detect environment (API keys, models, paths)
//! - Sensible defaults from industry best practices
//! - Easy customization when needed

pub mod config;
pub mod discovery;
pub mod presets;
pub mod validation;

pub use config::{JcodeConfig, ConfigProfile};
pub use discovery::{EnvironmentDetector, SystemCapabilities};
pub use presets::{QuickStartPreset, OptimizationPreset};
pub use validation::{ConfigValidator, ValidationWarning};
