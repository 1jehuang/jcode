//! Git Integration - Business Logic Layer (Layer 1)
//!
//! This module provides Git integration for version control:
//! - Git workflow management
//! - Version tracking
//! - Branch and commit operations

// --- Git Operations ---
pub mod git_workflow;
pub mod version_manager;

// Re-export key types
pub use git_workflow::GitWorkflow;

/// Stub VersionManager (full implementation pending)
#[derive(Debug, Default)]
pub struct VersionManager {
    _private: (),
}

impl VersionManager {
    pub fn new() -> Self { Self::default() }
}
