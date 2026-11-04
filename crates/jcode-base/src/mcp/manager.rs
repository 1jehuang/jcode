//! MCP Manager - manages MCP server connections for a single session.
//!
//! In daemon mode with a shared pool, servers marked `shared: true` (the default)
//! are managed by the pool and reused across sessions. Servers marked `shared: false`
//! (e.g., Playwright with browser state) are spawned per-session.

use super::client::{McpClient, McpHandle};
use super::pool::SharedMcpPool;
use super::protocol::{McpConfig, McpServerConfig, McpToolDef, ToolCallResult};
use anyhow::{Context, Result};
use serde::Serialize;
use std::collections::{BTreeMap, HashMap};
use std::sync::Arc;
use tokio::sync::{Mutex, RwLock, watch};

/// Bound on how long a tool call will wait for a not-yet-connected MCP server
/// to come up before failing with a clean tool error. Keeps a slow/hanging
/// server from blocking a single tool call forever (and never blocks spawn).
const CONNECT_ON_CALL_TIMEOUT: std::time::Duration = std::time::Duration::from_secs(30);

type OwnedConnectionResult = Option<std::result::Result<(), String>>;
type OwnedConnectionSender = watch::Sender<OwnedConnectionResult>;
type OwnedConnecting = Arc<std::sync::Mutex<HashMap<String, OwnedConnectionSender>>>;

/// Meter a completed tool call for partner-discovery provenance. No-op for
/// servers without discovery provenance (the overwhelmingly common case) and
/// whenever `sponsors.enabled` is false. Counts only; never content.
fn meter_provenance_call(server: &str, result: &Result<ToolCallResult>) {
    let is_error = match result {
        Ok(res) => res.is_error,
        Err(_) => true,
    };
    crate::sponsors::provenance::on_tool_call(server, is_error);
}

#[derive(Debug, Clone, Default, Serialize)]
pub struct McpManagerMemoryProfile {
    pub shared_pool_enabled: bool,
    pub configured_servers: usize,
    pub connected_servers: usize,
    pub pooled_handles: usize,
    pub owned_clients: usize,
    pub available_tools: usize,
    pub configured_json_bytes: usize,
    pub tool_schema_estimate_bytes: usize,
}

/// Manages MCP server connections for a session.
///
/// In daemon mode, shared servers delegate to the SharedMcpPool while
/// non-shared (stateful) servers are owned per-session.
pub struct McpManager {
    pool: Option<Arc<SharedMcpPool>>,
    /// Handles from the shared pool (shared servers)
    pool_handles: RwLock<HashMap<String, McpHandle>>,
    /// Per-session owned clients (non-shared / stateful servers)
    owned_clients: RwLock<HashMap<String, McpClient>>,
    /// In-flight owned-server handshakes, keyed by server name. This prevents
    /// background startup and a manual connect from spawning two processes for
    /// the same session server at the same time.
    connecting: OwnedConnecting,
    shared_operations: std::sync::Mutex<HashMap<String, Arc<Mutex<()>>>>,
    admission: RwLock<()>,
    cleanup_pending: std::sync::atomic::AtomicBool,
    config: McpConfig,
    session_id: String,
    reference_id: u64,
    /// Project directory used to resolve project-local MCP config. `None`
    /// loads only global config and never consults the process working directory.
    project_dir: Option<std::path::PathBuf>,
}

fn next_manager_reference_id() -> u64 {
    static NEXT: std::sync::atomic::AtomicU64 = std::sync::atomic::AtomicU64::new(1);
    NEXT.fetch_add(1, std::sync::atomic::Ordering::Relaxed)
}

struct OwnedConnectionAttemptGuard {
    connecting: OwnedConnecting,
    name: String,
    sender: OwnedConnectionSender,
    armed: bool,
}

impl OwnedConnectionAttemptGuard {
    fn new(connecting: OwnedConnecting, name: &str, sender: OwnedConnectionSender) -> Self {
        Self {
            connecting,
            name: name.to_string(),
            sender,
            armed: true,
        }
    }

    fn disarm(&mut self) {
        self.armed = false;
    }
}

impl Drop for OwnedConnectionAttemptGuard {
    fn drop(&mut self) {
        if !self.armed {
            return;
        }

        {
            let mut connecting = self.connecting.lock().unwrap_or_else(|e| e.into_inner());
            let is_current = connecting
                .get(&self.name)
                .is_some_and(|current| current.same_channel(&self.sender));
            if is_current {
                connecting.remove(&self.name);
            }
            self.sender.send_replace(Some(Err(format!(
                "MCP server '{}' connection attempt was cancelled",
                self.name
            ))));
        }
    }
}

include!("manager_connect.rs");
include!("manager_shared.rs");
include!("manager_lifecycle.rs");
include!("manager_tools.rs");

impl Default for McpManager {
    fn default() -> Self {
        Self::new()
    }
}

impl Drop for McpManager {
    fn drop(&mut self) {
        if let Some(pool) = &self.pool {
            pool.release_manager_on_drop(self.reference_id);
        }
    }
}

fn estimate_tool_bytes(tool: &McpToolDef) -> usize {
    tool.name.len()
        + tool
            .description
            .as_ref()
            .map(|value| value.len())
            .unwrap_or(0)
        + crate::process_memory::estimate_json_bytes(&tool.input_schema)
}

include!("manager_tests.rs");
include!("manager_admission_tests.rs");
