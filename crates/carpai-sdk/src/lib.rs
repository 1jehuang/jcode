//! # CarpAI SDK - Unified Client SDK
//!
//! A comprehensive client library for interacting with CarpAI services.
//! Provides a consistent API across different IDEs and platforms with:
//!
//! - **Unified API**: Single interface for all CarpAI services
//! - **Multi-protocol support**: gRPC, REST, SSE streaming
//! - **Intelligent caching**: Reduces latency and API calls
//! - **Offline mode**: Graceful degradation when offline
//! - **Error handling**: Rich error types with recovery suggestions
//! - **IDE integration**: Ready-to-use adapters for VS Code, JetBrains, etc.
//!
//! ## Quick Start
//!
//! ```rust,no_run
//! use carpai_sdk::{CarpAiClient, CarpAiConfig};
//!
//! #[tokio::main]
//! async fn main() -> Result<(), Box<dyn std::error::Error>> {
//!     let config = CarpAiConfig::default();
//!     let client = CarpAiClient::new(config).await?;
//!
//!     let response = client.complete("Explain Rust's ownership system").await?;
//!     println!("{}", response.text);
//!
//!     Ok(())
//! }
//! ```

pub mod cache;
pub mod client;
pub mod config;
pub mod error;
pub mod ide;
mod metrics;
pub mod protocol;
pub mod streaming;
pub mod types;

#[cfg(test)]
mod cache_tests;

#[cfg(test)]
mod types_tests;

#[cfg(test)]
mod error_tests;

// Re-export main types for convenience
pub use cache::{CacheConfig, CacheManager};
pub use client::{CarpAiClient, ClientBuilder};
pub use config::CarpAiConfig;
pub use error::{CarpAiError, Result};
pub use ide::{IdeAdapter, IdeType};
pub use protocol::{GrpcAdapter, RestAdapter, ProtocolAdapter};
pub use streaming::{StreamEvent, StreamHandler};
pub use types::*;

/// SDK version
pub const VERSION: &str = env!("CARGO_PKG_VERSION");

/// Initialize logging for the SDK
pub fn init_logging() {
    tracing_subscriber::fmt()
        .with_env_filter(
            tracing_subscriber::EnvFilter::try_from_default_env()
                .unwrap_or_else(|_| tracing_subscriber::EnvFilter::new("info")),
        )
        .init();
}
