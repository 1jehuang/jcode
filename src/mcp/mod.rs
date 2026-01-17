//! MCP (Model Context Protocol) client implementation
//!
//! Connects to MCP servers that provide tools via JSON-RPC over stdio.

#![allow(dead_code)]
#![allow(unused_imports)]

mod client;
mod manager;
mod protocol;
mod tool;

pub use client::McpClient;
pub use manager::McpManager;
pub use protocol::*;
pub use tool::{create_mcp_tools, McpTool};
