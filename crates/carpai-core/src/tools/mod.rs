//! Tool System Module
//!
//! This module provides the complete tool infrastructure for CarpAI, including:
//!
//! - **MCP Protocol** (`mcp`): Model Context Protocol implementation for tool discovery,
//!   JSON-RPC transport, server/client modes, and bidirectional bridging
//! - **Tool Registry** (`registry`): Dynamic tool registration, schema validation,
//!   execution routing, and context overflow protection
//! - **Slash Commands** (`slash_command`): CLI slash command system (/help, /clear, /model, etc.)
//!
//! ## Architecture
//!
//! ```text
//! +----------------------------------------------------------+
//! |                    Tool System (this module)              |
//! +----------------------------------------------------------+
//! |                                                          |
//! |  +-------------------+  +------------------+             |
//! |  | ToolRegistry      |  | SlashCommandRegistry|           |
//! |  | - Dynamic register|  | - /help          |             |
//! |  | - Schema validate |  | - /clear         |             |
//! |  | - Execute route   |  | - /model         |             |
//! |  | - Context guard   |  | - /mode          |             |
//! |  +--------+----------+  +------------------+             |
//! |           |                                              |
//! |           v                                              |
//! |  +---------------------------------------------------+  |
//! |  | MCP Layer                                          |  |
//! |  | - McpServer (expose tools via MCP)                 |  |
//! |  | - McpClient (connect to external MCP servers)      |  |
//! |  | - McpBridge (bidirectional server+client)          |  |
//! |  | - SharedMcpPool (process reuse across sessions)    |  |
//! |  +---------------------------------------------------+  |
//! |                                                          |
//! |  Uses: carpai_internal::{ToolExecutor, ToolRequest, ...}|
//! +----------------------------------------------------------+
//! ```
//!
//! ## Migration Notes (Phase 1D)
//!
//! Migrated from monolithic `src/tool/mod.rs`, `src/mcp/mod.rs`, and `src/tools.rs`.
//! All imports now use `crate::` paths (within carpai-core) or `carpai_internal::`
//! for shared trait types.

pub mod mcp;
pub mod registry;
pub mod slash_command;

// ========================================================================
// Re-exports — Public API surface
// ========================================================================

// --- From carpai-internal (shared types) ---
pub use carpai_internal::{
    tools::ToolDefinition,
    tools::ToolResult,
    tools::ToolError,
    tool_executor::ToolCategory,
};

// --- From this crate's modules ---
pub use registry::ToolRegistry;
pub use mcp::{
    McpServer,
    McpClient,
    McpManager,
    McpBridge,
    SharedMcpPool,
};
pub use slash_command::{
    SlashCommandRegistry,
    SlashCommand,
};
