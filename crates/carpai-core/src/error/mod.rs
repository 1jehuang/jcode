//! Error Handling & Recovery - Business Logic Layer (Layer 1)
//!
//! This module provides comprehensive error handling:
//! - Error recovery strategies
//! - Network retry logic with exponential backoff
//! - Error type definitions
//! - Allowlist management for safe operations

// --- Error Types & Recovery ---
pub mod error_types;
pub mod error_recovery;

// --- Network ---
pub mod network_retry;

// --- Safety ---
pub mod allowlist;

// Re-export key types
pub use error_types::{ProviderError as CarpaiError, ToolExecuteError, ConfigError, SessionError as SessionErr, FileError};
pub use error_recovery::{ErrorSeverity, ClassifiedError, RetryStrategy as RetryPolicy};
pub use network_retry::{NetworkWaitPlan, wait_until_probably_online};
pub use allowlist::AllowlistManager;
