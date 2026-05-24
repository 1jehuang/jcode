//! CarpAI Client SDK
//!
//! Provides a consistent API for interacting with CarpAI services
//! across different IDEs and platforms.
//!
//! ## Features
//! - Unified API for IDE integration
//! - MCP client for connecting to MCP servers
//! - Response caching with LRU
//! - Retry logic with exponential backoff
//! - Config management
//! - OpenAI-compatible Chat Completions API
//! - Session CRUD operations

pub mod client;
pub mod cache;
pub mod config;
pub mod error;
pub mod types;
pub mod mcp;
pub mod streaming;
pub mod ide;
pub mod protocol;
pub mod session_api;

// WASM bindings (optional, for browser/VSCode webview)
#[cfg(feature = "wasm")]
pub mod wasm;

// Re-export most commonly used items
pub use cache::ResponseCache;
pub use client::CarpAiClient;
pub use config::SdkConfig;
pub use error::SdkError;
pub use mcp::{
    McpClient,
    McpClientManager,
    McpClientError,
    McpConnectionStatus,
    McpServerConfig,
    McpServerInfo,
    McpToolDefinition,
    McpTransport,
    HttpMcpClient,
};
pub use types::*;

// Re-export Session API types
pub use session_api::{
    SessionCreateRequest,
    SessionResponse,
    SessionListRequest,
    SessionListResponse,
    MessageAppendRequest,
    GetMessagesRequest,
    GetMessagesResponse,
    DeleteSessionResponse,
};
