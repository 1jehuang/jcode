//! MCP Client - handles communication with a single MCP server

use super::protocol::*;
use anyhow::{Context, Result};
use serde_json::Value;
use std::collections::HashMap;
#[cfg(not(windows))]
use std::process::Stdio;
use std::sync::Arc;
use std::sync::atomic::{AtomicBool, AtomicU64, Ordering};
use tokio::io::{AsyncBufReadExt, AsyncWriteExt, BufReader};
#[cfg(not(windows))]
use tokio::process::{Child, Command};
#[cfg(not(windows))]
type StdioProcess = Child;
#[cfg(windows)]
type StdioProcess = super::windows_native_process::NativeProcess;
use tokio::sync::{Mutex, mpsc, oneshot};
#[cfg(all(windows, feature = "test-support"))]
type NativeObserver = fn(usize, usize, usize, u32, u32);
#[cfg(all(windows, feature = "test-support"))]
static NATIVE_OBSERVER: std::sync::Mutex<Option<NativeObserver>> = std::sync::Mutex::new(None);
#[cfg(all(windows, feature = "test-support"))]
static SHUTDOWN_OBSERVER: std::sync::Mutex<Option<fn()>> = std::sync::Mutex::new(None);

/// Shared communication handle for an MCP server.
/// Multiple sessions can hold clones of this and send concurrent requests.
/// Request/response correlation by ID ensures no interference.
#[derive(Clone)]
pub struct McpHandle {
    pub(crate) name: String,
    request_id: Arc<AtomicU64>,
    pending: Arc<Mutex<HashMap<u64, oneshot::Sender<JsonRpcResponse>>>>,
    /// Set by the reader on stdout EOF: no reply can arrive, so requests fail fast.
    closed: Arc<AtomicBool>,
    writer_tx: mpsc::Sender<String>,
    server_info: Arc<std::sync::RwLock<Option<ServerInfo>>>,
    capabilities: Arc<std::sync::RwLock<ServerCapabilities>>,
    tools: Arc<std::sync::RwLock<Vec<McpToolDef>>>,
    /// Reply timeout applied to every request on this server.
    request_timeout: std::time::Duration,
}

/// Removes waiters on timeout, send failure and cancellation of the caller.
struct PendingRequest {
    id: u64,
    pending: Arc<Mutex<HashMap<u64, oneshot::Sender<JsonRpcResponse>>>>,
}

impl Drop for PendingRequest {
    fn drop(&mut self) {
        if let Ok(mut pending) = self.pending.try_lock() {
            pending.remove(&self.id);
        } else if let Ok(runtime) = tokio::runtime::Handle::try_current() {
            let pending = Arc::clone(&self.pending);
            let id = self.id;
            runtime.spawn(async move {
                pending.lock().await.remove(&id);
            });
        }
    }
}

/// Default reply timeout when a server config does not set `timeout_secs`.
pub const DEFAULT_MCP_REQUEST_TIMEOUT: std::time::Duration = std::time::Duration::from_secs(30);

/// A separate budget for the complete remote connection, including OAuth refresh.
const REMOTE_MCP_HANDSHAKE_TIMEOUT: std::time::Duration = std::time::Duration::from_secs(30);

/// Resolve the per-request reply timeout for a server config.
pub fn request_timeout_for(config: &McpServerConfig) -> std::time::Duration {
    config
        .timeout_secs
        .filter(|secs| *secs > 0)
        .map(std::time::Duration::from_secs)
        .unwrap_or(DEFAULT_MCP_REQUEST_TIMEOUT)
}

impl McpHandle {
    #[cfg(feature = "test-support")]
    pub async fn test_pending_count(&self) -> usize {
        self.pending.lock().await.len()
    }
    #[cfg(feature = "test-support")]
    pub fn test_writer_remaining_capacity(&self) -> usize {
        self.writer_tx.capacity()
    }

    /// Send a request and wait for response
    pub async fn request(&self, method: &str, params: Option<Value>) -> Result<JsonRpcResponse> {
        let id = self.request_id.fetch_add(1, Ordering::SeqCst);
        let request = JsonRpcRequest::new(id, method, params);

        let msg = serde_json::to_string(&request)? + "\n";
        let (tx, rx) = oneshot::channel();
        {
            let mut pending = self.pending.lock().await;
            if self.closed.load(Ordering::SeqCst) {
                anyhow::bail!("MCP server '{}' exited (connection closed)", self.name);
            }
            pending.insert(id, tx);
        }
        let _cleanup = PendingRequest {
            id,
            pending: Arc::clone(&self.pending),
        };
        // Include channel backpressure in the deadline, not just the reply.
        let response = tokio::time::timeout(self.request_timeout, async {
            self.writer_tx.send(msg).await.context("Failed to send request")?;
            rx.await.with_context(|| format!("MCP server '{}' exited (connection closed)", self.name))
        })
        .await
        .with_context(|| format!(
            "Request timeout after {}s (raise `timeout_secs` for MCP server '{}' if its tools legitimately run longer)",
            self.request_timeout.as_secs(), self.name
        ))??;

        if let Some(err) = &response.error {
            anyhow::bail!("MCP error {}: {}", err.code, err.message);
        }

        Ok(response)
    }

    /// Call a tool
    pub async fn call_tool(&self, name: &str, arguments: Value) -> Result<ToolCallResult> {
        let arguments = if arguments.is_null() {
            Value::Object(serde_json::Map::new())
        } else {
            arguments
        };
        let params = ToolCallParams {
            name: name.to_string(),
            arguments,
        };

        let response = self
            .request("tools/call", Some(serde_json::to_value(params)?))
            .await?;

        let result = response.result.context("No result from tool call")?;
        let tool_result: ToolCallResult = serde_json::from_value(result)?;

        Ok(tool_result)
    }

    /// Whether the transport can no longer deliver replies. Closed handles
    /// must be replaced by a fresh connection, never used to replay a call.
    pub fn is_closed(&self) -> bool {
        self.closed.load(Ordering::SeqCst) || self.writer_tx.is_closed()
    }

    /// Get the server name
    pub fn name(&self) -> &str {
        &self.name
    }

    /// Get server info
    pub fn server_info(&self) -> Option<ServerInfo> {
        self.server_info
            .read()
            .unwrap_or_else(|poisoned| poisoned.into_inner())
            .clone()
    }

    /// Get available tools
    pub fn tools(&self) -> Vec<McpToolDef> {
        self.tools
            .read()
            .unwrap_or_else(|poisoned| poisoned.into_inner())
            .clone()
    }

    /// Refresh the list of available tools
    pub async fn refresh_tools(&self) -> Result<()> {
        let response = self.request("tools/list", None).await?;

        if let Some(result) = response.result {
            let tools_result: ToolsListResult = serde_json::from_value(result)?;
            *self
                .tools
                .write()
                .unwrap_or_else(|poisoned| poisoned.into_inner()) = tools_result.tools;
        }

        Ok(())
    }
}

/// MCP Client - owns the child process and provides shared handles.
/// Only one McpClient exists per MCP server process, but many McpHandle
/// clones can be distributed to different sessions.
pub struct McpClient {
    handle: McpHandle,
    backend: Backend,
    shutdown_pending: bool,
}

/// What actually carries the JSON-RPC messages for a client.
enum Backend {
    /// Local child process speaking newline-delimited JSON over stdio.
    Stdio(StdioProcess),
    /// Remote server over Streamable HTTP or legacy SSE.
    Remote(super::remote::RemoteConnection),
}

include!("client_connect.rs");
include!("client_remote.rs");
include!("client_shutdown.rs");

impl McpClient {
    /// Get a shareable handle to this client
    pub fn handle(&self) -> McpHandle {
        self.handle.clone()
    }

    /// Initialize the MCP connection
    async fn initialize(&mut self) -> Result<()> {
        // Remote servers get the newest revision we speak; older servers
        // answer with the version they support, which is fine because jcode
        // only uses methods common to every revision.
        let protocol_version = match &self.backend {
            Backend::Stdio(_) => "2024-11-05",
            Backend::Remote(connection) if connection.is_legacy() => "2024-11-05",
            Backend::Remote(_) => "2025-06-18",
        };
        let params = InitializeParams {
            protocol_version: protocol_version.to_string(),
            capabilities: ClientCapabilities::default(),
            client_info: ClientInfo {
                name: "jcode".to_string(),
                version: jcode_build_meta::pkg_version().to_string(),
            },
        };

        let response = self
            .handle
            .request("initialize", Some(serde_json::to_value(params)?))
            .await?;

        {
            let result = response
                .result
                .context("MCP initialize response has no result")?;
            let init_result: InitializeResult = serde_json::from_value(result)?;
            if let Backend::Remote(connection) = &self.backend {
                if !matches!(
                    init_result.protocol_version.as_str(),
                    "2024-11-05" | "2025-03-26" | "2025-06-18"
                ) {
                    anyhow::bail!(
                        "MCP server negotiated unsupported protocol version: {}",
                        init_result.protocol_version
                    );
                }
                connection.set_protocol_version(&init_result.protocol_version);
            }
            *self
                .handle
                .server_info
                .write()
                .unwrap_or_else(|poisoned| poisoned.into_inner()) = init_result.server_info;
            *self
                .handle
                .capabilities
                .write()
                .unwrap_or_else(|poisoned| poisoned.into_inner()) = init_result.capabilities;
        }

        // Send initialized notification
        let notif = JsonRpcNotification::new("notifications/initialized", None);
        let msg = serde_json::to_string(&notif)? + "\n";
        self.handle.writer_tx.send(msg).await?;

        Ok(())
    }

    /// Check if server is still running
    pub fn is_running(&mut self) -> bool {
        match &mut self.backend {
            Backend::Stdio(child) => matches!(child.try_wait(), Ok(None)),
            Backend::Remote(connection) => connection.is_running(),
        }
    }

    /// Whether this client talks to a remote server.
    pub fn is_remote(&self) -> bool {
        matches!(self.backend, Backend::Remote(_))
    }

    // === Legacy compatibility methods that delegate to handle ===

    pub fn name(&self) -> &str {
        &self.handle.name
    }

    pub fn server_info(&self) -> Option<ServerInfo> {
        self.handle.server_info()
    }

    pub fn tools(&self) -> Vec<McpToolDef> {
        self.handle.tools()
    }

    pub async fn call_tool(&self, name: &str, arguments: Value) -> Result<ToolCallResult> {
        self.handle.call_tool(name, arguments).await
    }

    pub async fn refresh_tools(&self) -> Result<()> {
        self.handle.refresh_tools().await
    }
}

/// Secrets that an MCP child must not receive merely because jcode has them.
///
/// This intentionally applies only to inherited values. A server can still be
/// given any of these names through `McpServerConfig::env`.
fn is_sensitive_inherited_env_key(key: &str) -> bool {
    let key = key.to_ascii_uppercase();
    key.ends_with("_API_KEY")
        || key.ends_with("_ACCESS_TOKEN")
        || key.ends_with("_AUTH_TOKEN")
        || matches!(
            key.as_str(),
            "AWS_ACCESS_KEY_ID"
                | "AWS_SECRET_ACCESS_KEY"
                | "AWS_SESSION_TOKEN"
                | "AZURE_CLIENT_SECRET"
                | "GOOGLE_APPLICATION_CREDENTIALS"
        )
}

fn mcp_child_env(
    mut inherited: HashMap<std::ffi::OsString, std::ffi::OsString>,
    explicit: &HashMap<String, String>,
) -> Result<HashMap<std::ffi::OsString, std::ffi::OsString>> {
    inherited.retain(|key, _| !is_sensitive_inherited_env_key(&key.to_string_lossy()));
    #[cfg(windows)]
    {
        let mut entries: Vec<_> = explicit.iter().collect();
        entries.sort_by(|a, b| a.0.cmp(b.0));
        for (key, value) in entries {
            let mut replaced = Vec::new();
            for existing in inherited.keys() {
                if super::windows_native_env::same_key(existing, key.as_ref())? {
                    replaced.push(existing.clone());
                }
            }
            for existing in replaced {
                drop(inherited.remove(&existing));
            }
            inherited.insert(key.into(), value.into());
        }
        Ok(inherited)
    }
    #[cfg(not(windows))]
    {
        inherited.extend(
            explicit
                .iter()
                .map(|(key, value)| (key.into(), value.into())),
        );
        Ok(inherited)
    }
}

#[cfg(not(windows))]
fn configure_mcp_command_env(
    command: &mut Command,
    env: &HashMap<std::ffi::OsString, std::ffi::OsString>,
) {
    command.env_clear().envs(env);
}

impl Drop for McpClient {
    fn drop(&mut self) {
        if let Backend::Stdio(child) = &mut self.backend {
            let _ = child.start_kill();
        }
    }
}

include!("client_tests.rs");
