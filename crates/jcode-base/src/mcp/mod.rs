//! MCP (Model Context Protocol) client implementation
//!
//! Connects to MCP servers that provide tools via JSON-RPC over stdio.
//! Supports shared server pools so multiple sessions reuse the same
//! MCP server processes instead of spawning duplicates.

mod client;
mod manager;
pub mod pool;
mod protocol;
pub mod schema_cache;
mod tool;

pub use client::{McpClient, McpHandle};
pub use manager::McpManager;
pub use pool::{SharedMcpPool, get_shared_pool, init_shared_pool};
pub use protocol::*;
pub use schema_cache::{McpSchemaCache, fingerprint_config};
pub use tool::{McpTool, create_mcp_tools, create_mcp_tools_from_cached};
