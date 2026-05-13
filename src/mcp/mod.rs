//! MCP (Model Context Protocol) client implementation
//!
//! Connects to MCP servers that provide tools via JSON-RPC over stdio.
//! Supports shared server pools so multiple sessions reuse the same
//! MCP server processes instead of spawning duplicates.
//!
//! Enhanced features:
//! - Multiple transport types (StdIO, SSE, StreamableHTTP, WebSocket)
//! - OAuth authentication support
//! - Connection pooling and retry logic
//! - Session management with error recovery
//! - Progress reporting for tool calls
//! - Comprehensive error handling

pub mod client;
pub mod enhanced_client;
pub mod manager;
pub mod pool;
pub mod protocol;
pub mod tool;

pub use client::{McpClient, McpHandle};
pub use enhanced_client::{
    ConnectionState, EnhancedMcpClient, EnhancedMcpConfig, EnhancedMcpHandle,
    HealthStatus, McpError, ProgressStage, ToolCallProgress, TransportType,
};
pub use manager::McpManager;
pub use pool::{SharedMcpPool, get_shared_pool, init_shared_pool};
pub use protocol::*;
pub use tool::{McpTool, create_mcp_tools};
