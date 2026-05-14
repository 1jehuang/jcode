//! MCP (Model Context Protocol) implementation
//!
//! ## Architecture
//! ```text
//! ┌─────────────────────────────────────────────────────────┐
//! │                    McpBridge                            │
//! │   (bidirectional — Server + Client in one)              │
//! ├─────────────────────────────────────────────────────────┤
//! │  ┌──────────────┐    ┌──────────────────────────────┐   │
//! │  │  MCP Server  │    │  MCP Client (McpManager)    │   │
//! │  │ (server.rs)  │    │  - Basic McpClient          │   │
//! │  │ - tools/list │    │  - EnhancedMcpClient        │   │
//! │  │ - tools/call │    │  - SharedMcpPool            │   │
//! │  │ - resources  │    │  - SSE/HTTP/WS transports   │   │
//! │  │ - prompts    │    └──────────────────────────────┘   │
//! │  └──────┬───────┘                                       │
//! │         │                                                │
//! │         ▼                                                │
//! │  ┌──────────────────────────────────────────────────┐    │
//! │  │           Tool Registry + MCP Tool wrapper       │    │
//! │  └──────────────────────────────────────────────────┘    │
//! └─────────────────────────────────────────────────────────┘
//! ```
//!
//! ## Server mode (other tools connect TO CarpAI)
//! Run `carpai mcp serve` to start CarpAI as an MCP server.
//! External tools (IDEs, agents) can call CarpAI's tools
//! via the MCP protocol over stdin/stdout JSON-RPC.
//!
//! ## Client mode (connecting TO an MCP server)
//! Connect to MCP servers that provide tools via JSON-RPC over stdio.
//! Supports shared server pools so multiple sessions reuse the same
//! MCP server processes instead of spawning duplicates.
//!
//! ## Bidirectional mode
//! Run `carpai mcp bridge` to start both server and client simultaneously.

pub mod bridge;
pub mod client;
pub mod enhanced_client;
pub mod manager;
pub mod pool;
pub mod protocol;
pub mod server;
pub mod tool;

pub use bridge::{
    BridgeCapabilities, BridgeStatus, McpBridge, McpBridgeConfig,
};
pub use client::{McpClient, McpHandle};
pub use enhanced_client::{
    ConnectionState, EnhancedMcpClient, EnhancedMcpConfig, EnhancedMcpHandle,
    HealthStatus, McpError, ProgressStage, ToolCallProgress, TransportType,
};
pub use manager::McpManager;
pub use pool::{SharedMcpPool, get_shared_pool, init_shared_pool};
pub use protocol::*;
pub use server::{McpServer, McpServerConfig, ExtraToolDef};
pub use tool::{McpTool, create_mcp_tools};
