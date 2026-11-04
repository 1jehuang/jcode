//! MCP management tool - stdio/remote connections and noninteractive OAuth management

#[cfg(test)]
#[path = "mcp_remote_management_tests.rs"]
mod remote_management_tests;

use crate::mcp::{ContentBlock, McpManager, McpServerConfig, dispatch_name};
use crate::tool::{Tool, ToolContext, ToolOutput};
use anyhow::Result;
use async_trait::async_trait;
use serde::{Deserialize, Serialize};
use serde_json::{Value, json};
use std::collections::HashMap;
use std::sync::Arc;
use tokio::sync::RwLock;

include!("mcp_search_impl.rs");
include!("mcp_call_impl.rs");
include!("mcp_management_types.rs");
include!("mcp_management_dispatch.rs");
include!("mcp_management_lifecycle.rs");
#[cfg(test)]
#[path = "mcp_tests.rs"]
mod tests;
