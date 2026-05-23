//! # jcode-debate-core
//!
//! Multi-perspective debate orchestration system for AI-powered decision making.
//!
//! This crate provides infrastructure for running debates between multiple AI perspectives,
//! coordinating their interactions, and synthesizing their viewpoints into actionable decisions.
//!
//! ## Architecture
//!
//! - `Perspective`: Defines the three debate perspectives (Advocate, Critic, Synthesizer)
//! - `DebateSession`: Manages the state and history of a debate session
//! - `Coordinator`: Orchestrates the debate flow between perspectives
//! - `RateLimiter`: Prevents API rate limit collisions
//!
//! ## Example
//!
//! ```rust,ignore
//! use jcode_debate_core::{DebateSession, Coordinator, DebateConfig};
//!
//! let config = DebateConfig::default();
//! let mut session = DebateSession::new(config);
//! let coordinator = Coordinator::new();
//!
//! coordinator.add_perspective("Should we adopt Rust for our backend?").await?;
//! coordinator.run_debate().await?;
//!
//! let verdict = coordinator.final_verdict().await?;
//! ```

pub mod coordinator;
pub mod debate_session;
pub mod perspectives;
pub mod provider_adapter;
pub mod rate_limiter;

pub use coordinator::Coordinator;
pub use debate_session::{
    DebateConfig, DebatePhase, DebateSession, DebateVerdict, PerspectiveResponse,
};
pub use perspectives::{DebateTopic, Perspective, PerspectiveType};
pub use provider_adapter::{
    create_adapter_from_multi_provider, JcodeProviderAdapter, ProviderAdapterBuilder, ProviderType,
    RateLimitConfig, RateLimitStrategy,
};
pub use rate_limiter::RateLimiter;

use thiserror::Error;

/// Errors that can occur during debate operations
#[derive(Error, Debug)]
pub enum DebateError {
    #[error("Provider error: {0}")]
    Provider(String),

    #[error("Rate limit exceeded. Retry after {retry_after}s")]
    RateLimit { retry_after: u64 },

    #[error("Timeout waiting for perspective response: {0}")]
    Timeout(String),

    #[error("Invalid debate state: {0}")]
    InvalidState(String),

    #[error("Serialization error: {0}")]
    Serialization(#[from] serde_json::Error),

    #[error("Network error: {0}")]
    Network(String),

    #[error("Context window exceeded")]
    ContextExceeded,
}

impl From<reqwest::Error> for DebateError {
    fn from(err: reqwest::Error) -> Self {
        DebateError::Network(err.to_string())
    }
}

/// Result type for debate operations
pub type DebateResult<T> = Result<T, DebateError>;

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn debate_error_display() {
        let err = DebateError::RateLimit { retry_after: 30 };
        assert_eq!(err.to_string(), "Rate limit exceeded. Retry after 30s");

        let err = DebateError::Provider("test error".to_string());
        assert_eq!(err.to_string(), "Provider error: test error");

        let err = DebateError::Timeout("timeout".to_string());
        assert_eq!(
            err.to_string(),
            "Timeout waiting for perspective response: timeout"
        );
    }

    #[test]
    fn debate_error_from_network() {
        // Test that network errors are properly converted
        let network_err = DebateError::Network("connection refused".to_string());
        assert_eq!(network_err.to_string(), "Network error: connection refused");
    }
}
