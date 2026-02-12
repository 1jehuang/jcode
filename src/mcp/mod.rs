//! MCP (Model Context Protocol) client implementation
//!
//! Connects to MCP servers that provide tools via JSON-RPC over stdio.
//! Supports shared server pools so multiple sessions reuse the same
//! MCP server processes instead of spawning duplicates.

#![allow(dead_code)]
#![allow(unused_imports)]

mod client;
mod manager;
pub mod pool;
mod protocol;
mod tool;

pub use client::{McpClient, McpHandle};
pub use manager::McpManager;
pub use pool::{get_shared_pool, init_shared_pool, SharedMcpPool};
pub use protocol::*;
pub use tool::{create_mcp_tools, McpTool};
