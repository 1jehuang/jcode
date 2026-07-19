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
use std::collections::HashMap;
use std::sync::Arc;
use tokio::sync::RwLock;

/// Bound on how long a tool call will wait for a not-yet-connected MCP server
/// to come up before failing with a clean tool error. Keeps a slow/hanging
/// server from blocking a single tool call forever (and never blocks spawn).
const CONNECT_ON_CALL_TIMEOUT: std::time::Duration = std::time::Duration::from_secs(30);

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
    config: McpConfig,
    session_id: String,
    /// Project directory used to resolve project-local MCP config. `None`
    /// loads only global config and never consults the process working directory.
    project_dir: Option<std::path::PathBuf>,
}

impl McpManager {
    /// Create a new manager in owned in-process mode (used by tests and local harnesses).
    pub fn new() -> Self {
        Self {
            pool: None,
            pool_handles: RwLock::new(HashMap::new()),
            owned_clients: RwLock::new(HashMap::new()),
            config: McpConfig::load(),
            session_id: "owned".to_string(),
            project_dir: None,
        }
    }

    /// Create a manager backed by a shared pool (daemon mode)
    pub fn with_shared_pool(pool: Arc<SharedMcpPool>, session_id: String) -> Self {
        Self::with_shared_pool_for_dir(pool, session_id, None)
    }

    /// Create a manager backed by a shared pool, resolving project-local MCP
    /// config against `project_dir` instead of the server process cwd
    /// (issue #420: remote/client sessions must use the session working dir).
    pub fn with_shared_pool_for_dir(
        pool: Arc<SharedMcpPool>,
        session_id: String,
        project_dir: Option<std::path::PathBuf>,
    ) -> Self {
        Self {
            pool: Some(pool),
            pool_handles: RwLock::new(HashMap::new()),
            owned_clients: RwLock::new(HashMap::new()),
            config: McpConfig::load_for_dir(project_dir.as_deref()),
            session_id,
            project_dir,
        }
    }

    /// Create manager with specific config (no sharing)
    pub fn with_config(config: McpConfig) -> Self {
        Self {
            pool: None,
            pool_handles: RwLock::new(HashMap::new()),
            owned_clients: RwLock::new(HashMap::new()),
            config,
            session_id: "owned".to_string(),
            project_dir: None,
        }
    }

    /// Whether this manager has a shared pool available
    pub fn is_shared(&self) -> bool {
        self.pool.is_some()
    }

    /// Connect to all configured servers.
    /// Shared servers go to the pool, non-shared are spawned per-session.
    #[expect(
        clippy::collapsible_if,
        reason = "MCP connect flow keeps shared-pool and owned-server paths explicit"
    )]
    pub async fn connect_all(&self) -> Result<(usize, Vec<(String, String)>)> {
        let mut total_successes = 0;
        let mut total_failures = Vec::new();

        // Disabled servers stay in config (so they can be connected on demand
        // by name) but are never auto-spawned (issue #436).
        // Split the rest into shared vs owned.
        let (shared_servers, owned_servers): (Vec<_>, Vec<_>) = self
            .config
            .servers
            .iter()
            .filter(|(_, config)| config.is_enabled())
            .partition(|(_, config)| config.shared && self.pool.is_some());

        // Connect shared servers via pool
        if let Some(pool) = &self.pool {
            if !shared_servers.is_empty() {
                let (successes, failures) = pool.connect_all().await;
                total_successes += successes;
                total_failures.extend(failures);

                // Acquire handles for shared servers only
                let all_handles = pool.acquire_handles(&self.session_id).await;
                let shared_names: std::collections::HashSet<&String> =
                    shared_servers.iter().map(|(name, _)| *name).collect();
                let mut pool_handles = self.pool_handles.write().await;
                for (name, handle) in all_handles {
                    if shared_names.contains(&name) {
                        pool_handles.insert(name, handle);
                    }
                }

                // If pool already had servers connected, count those as successes
                if total_successes == 0 && !pool_handles.is_empty() {
                    total_successes = pool_handles.len();
                }
            }
        }

        // Connect non-shared servers per-session
        if !owned_servers.is_empty() {
            let mut spawn_handles = Vec::new();

            for (name, config) in owned_servers {
                let name = name.clone();
                let config = config.clone();
                let handle = tokio::spawn(async move {
                    let result = McpClient::connect(name.clone(), &config).await;
                    (name, result)
                });
                spawn_handles.push(handle);
            }

            for handle in spawn_handles {
                match handle.await {
                    Ok((name, Ok(client))) => {
                        let mut clients = self.owned_clients.write().await;
                        clients.insert(name, client);
                        total_successes += 1;
                    }
                    Ok((name, Err(e))) => {
                        let error_msg = format!("{:#}", e);
                        crate::logging::error(&format!(
                            "Failed to connect to MCP server '{}': {}",
                            name, error_msg
                        ));
                        total_failures.push((name, error_msg));
                    }
                    Err(e) => {
                        crate::logging::error(&format!("MCP connection task panicked: {}", e));
                    }
                }
            }
        }

        Ok((total_successes, total_failures))
    }

    /// Connect to a specific server
    #[expect(
        clippy::collapsible_if,
        reason = "MCP connect flow keeps shared-pool and owned-server paths explicit"
    )]
    pub async fn connect(&self, name: &str, config: &McpServerConfig) -> Result<()> {
        // Partner-discovery provenance: if this server's command matches a
        // setup the agent saw in a discover_tools listing, tag it so calls to
        // it are metered coarsely (counts only; see sponsors::provenance).
        if let Some(sponsor) =
            crate::sponsors::provenance::on_server_connected(name, &config.command, &config.args)
        {
            crate::logging::info(&format!(
                "MCP: '{name}' connected via partner discovery (partner: {sponsor}); \
                 coarse usage counts are shared per the disclosed policy"
            ));
        }
        if config.shared {
            if let Some(pool) = &self.pool {
                pool.connect_server(name, config).await?;
                if let Some(handle) = pool.get_handle(name).await {
                    self.pool_handles
                        .write()
                        .await
                        .insert(name.to_string(), handle);
                }
                return Ok(());
            }
        }

        // Owned (non-shared or no pool available)
        let client = McpClient::connect(name.to_string(), config)
            .await
            .with_context(|| format!("Failed to connect to MCP server '{}'", name))?;

        self.owned_clients
            .write()
            .await
            .insert(name.to_string(), client);
        Ok(())
    }

    /// Disconnect from a server
    pub async fn disconnect(&self, name: &str) -> Result<()> {
        // Check if it's a pool handle
        {
            let mut handles = self.pool_handles.write().await;
            if handles.remove(name).is_some() {
                if let Some(pool) = &self.pool {
                    pool.release_handles(&self.session_id, &[name.to_string()])
                        .await;
                }
                return Ok(());
            }
        }

        // Otherwise it's owned
        let mut clients = self.owned_clients.write().await;
        if let Some(mut client) = clients.remove(name) {
            client.shutdown().await;
        }
        Ok(())
    }

    /// Disconnect from all servers
    pub async fn disconnect_all(&self) {
        // Session is ending: flush any pending partner-discovery usage
        // aggregates (best effort) so short sessions still report.
        crate::sponsors::provenance::flush_now();
        // Release pool handles
        {
            let mut handles = self.pool_handles.write().await;
            let names: Vec<String> = handles.keys().cloned().collect();
            handles.clear();
            if let Some(pool) = &self.pool {
                pool.release_handles(&self.session_id, &names).await;
            }
        }

        // Shutdown owned clients
        {
            let mut clients = self.owned_clients.write().await;
            for (_, mut client) in clients.drain() {
                client.shutdown().await;
            }
        }
    }

    /// Get list of connected server names
    pub async fn connected_servers(&self) -> Vec<String> {
        let mut names: Vec<String> = self.pool_handles.read().await.keys().cloned().collect();
        names.extend(self.owned_clients.read().await.keys().cloned());
        names
    }

    /// Get all available tools from all connected servers
    pub async fn all_tools(&self) -> Vec<(String, McpToolDef)> {
        let mut tools = Vec::new();

        // Pool handles
        for (server_name, handle) in self.pool_handles.read().await.iter() {
            for tool in handle.tools() {
                tools.push((server_name.clone(), tool));
            }
        }

        // Owned clients
        for (server_name, client) in self.owned_clients.read().await.iter() {
            for tool in client.tools() {
                tools.push((server_name.clone(), tool));
            }
        }

        tools
    }

    /// Call a tool on a specific server.
    ///
    /// Connect-on-first-call: if the server is configured but not yet connected
    /// (e.g. because we advertised its tools early from the on-disk schema cache
    /// while the background connection was still settling), this connects it
    /// first, bounded by `CONNECT_ON_CALL_TIMEOUT`. This is the latency we
    /// deliberately deferred from spawn — paid only when a tool is actually
    /// used, never blocking startup.
    pub async fn call_tool(
        &self,
        server: &str,
        tool: &str,
        arguments: serde_json::Value,
    ) -> Result<ToolCallResult> {
        // Fast path: already connected via pool handle.
        {
            let handles = self.pool_handles.read().await;
            if let Some(handle) = handles.get(server) {
                let result = handle.call_tool(tool, arguments).await;
                meter_provenance_call(server, &result);
                return result;
            }
        }
        // Fast path: already connected via owned client.
        {
            let clients = self.owned_clients.read().await;
            if let Some(client) = clients.get(server) {
                let result = client.call_tool(tool, arguments).await;
                meter_provenance_call(server, &result);
                return result;
            }
        }

        // Not connected yet. If the server is configured, connect-on-first-call.
        if let Some(config) = self.config.servers.get(server).cloned() {
            crate::logging::info(&format!(
                "MCP: connecting to '{server}' on first tool call (connect-on-first-call)"
            ));
            let connect = self.connect(server, &config);
            match tokio::time::timeout(CONNECT_ON_CALL_TIMEOUT, connect).await {
                Ok(Ok(())) => {
                    // Retry once now that we should be connected.
                    {
                        let handles = self.pool_handles.read().await;
                        if let Some(handle) = handles.get(server) {
                            let result = handle.call_tool(tool, arguments).await;
                            meter_provenance_call(server, &result);
                            return result;
                        }
                    }
                    let clients = self.owned_clients.read().await;
                    if let Some(client) = clients.get(server) {
                        let result = client.call_tool(tool, arguments).await;
                        meter_provenance_call(server, &result);
                        return result;
                    }
                    anyhow::bail!(
                        "MCP server '{server}' connected but exposed no handle for tool '{tool}'"
                    );
                }
                Ok(Err(err)) => {
                    anyhow::bail!("MCP server '{server}' failed to connect: {err:#}");
                }
                Err(_) => {
                    anyhow::bail!(
                        "MCP server '{server}' did not connect within {}s; tool '{tool}' is \
                         unavailable right now",
                        CONNECT_ON_CALL_TIMEOUT.as_secs()
                    );
                }
            }
        }

        anyhow::bail!("MCP server '{}' not connected", server)
    }

    /// Ensure a configured server is connected, bounded by `timeout`. No-op if
    /// already connected or not configured. Used to warm a server proactively.
    pub async fn ensure_server_connected(
        &self,
        server: &str,
        timeout: std::time::Duration,
    ) -> Result<()> {
        if self.connected_servers().await.iter().any(|s| s == server) {
            return Ok(());
        }
        let Some(config) = self.config.servers.get(server).cloned() else {
            anyhow::bail!("MCP server '{server}' is not configured");
        };
        match tokio::time::timeout(timeout, self.connect(server, &config)).await {
            Ok(result) => result,
            Err(_) => anyhow::bail!(
                "MCP server '{server}' did not connect within {}s",
                timeout.as_secs()
            ),
        }
    }

    /// Reload config and reconnect to servers
    pub async fn reload(&mut self) -> Result<(usize, Vec<(String, String)>)> {
        // Disconnect all (releases pool handles, shuts down owned)
        self.disconnect_all().await;

        // Reload config
        self.config = McpConfig::load_for_dir(self.project_dir.as_deref());

        // If we have a pool, reload it too (reconnects shared servers)
        if let Some(pool) = &self.pool {
            pool.reload().await;
        }

        // Reconnect everything
        self.connect_all().await
    }

    /// Get config
    pub fn config(&self) -> &McpConfig {
        &self.config
    }

    /// Load a fresh copy of the config from disk, resolved against this
    /// manager's project directory (or the process cwd when unset).
    pub fn load_fresh_config(&self) -> McpConfig {
        McpConfig::load_for_dir(self.project_dir.as_deref())
    }

    pub fn debug_memory_profile(&self) -> McpManagerMemoryProfile {
        let pooled_handles = self
            .pool_handles
            .try_read()
            .map(|handles| handles.len())
            .unwrap_or(0);
        let owned_clients = self
            .owned_clients
            .try_read()
            .map(|clients| clients.len())
            .unwrap_or(0);

        let mut available_tools = 0usize;
        let mut tool_schema_estimate_bytes = 0usize;

        if let Ok(handles) = self.pool_handles.try_read() {
            for handle in handles.values() {
                for tool in handle.tools() {
                    available_tools += 1;
                    tool_schema_estimate_bytes += estimate_tool_bytes(&tool);
                }
            }
        }

        if let Ok(clients) = self.owned_clients.try_read() {
            for client in clients.values() {
                for tool in client.tools() {
                    available_tools += 1;
                    tool_schema_estimate_bytes += estimate_tool_bytes(&tool);
                }
            }
        }

        McpManagerMemoryProfile {
            shared_pool_enabled: self.pool.is_some(),
            configured_servers: self.config.servers.len(),
            connected_servers: pooled_handles + owned_clients,
            pooled_handles,
            owned_clients,
            available_tools,
            configured_json_bytes: crate::process_memory::estimate_json_bytes(&self.config),
            tool_schema_estimate_bytes,
        }
    }

    /// Check if any servers are connected
    pub async fn has_connections(&self) -> bool {
        !self.pool_handles.read().await.is_empty() || !self.owned_clients.read().await.is_empty()
    }
}

impl Default for McpManager {
    fn default() -> Self {
        Self::new()
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

#[cfg(test)]
mod tests {
    use super::*;
    use std::time::Duration;

    fn empty_config() -> McpConfig {
        McpConfig::default()
    }

    #[tokio::test]
    async fn call_tool_unconfigured_server_bails_cleanly() {
        let manager = McpManager::with_config(empty_config());
        let err = manager
            .call_tool("ghost", "do_thing", serde_json::json!({}))
            .await
            .expect_err("calling an unknown server must error");
        assert!(
            err.to_string().contains("ghost"),
            "error should name the missing server: {err}"
        );
    }

    #[tokio::test]
    async fn ensure_server_connected_unconfigured_errors() {
        let manager = McpManager::with_config(empty_config());
        let err = manager
            .ensure_server_connected("ghost", Duration::from_millis(50))
            .await
            .expect_err("ensuring an unconfigured server must error");
        assert!(err.to_string().contains("not configured"), "{err}");
    }

    #[tokio::test]
    async fn connect_all_skips_disabled_servers() {
        // Issue #436: disabled servers stay in config but are never
        // auto-spawned. This config's command would fail to connect (and thus
        // produce a failure entry) if it were attempted at all.
        let mut config = McpConfig::default();
        config.servers.insert(
            "off".to_string(),
            McpServerConfig {
                command: "true".to_string(),
                args: vec![],
                env: HashMap::new(),
                shared: false,
                transport: None,
                url: None,
                enabled: Some(false),
                disabled: None,
            },
        );
        let manager = McpManager::with_config(config);
        let (successes, failures) = manager.connect_all().await.expect("connect_all");
        assert_eq!(successes, 0, "disabled server must not be spawned");
        assert!(
            failures.is_empty(),
            "disabled server must not be attempted: {failures:?}"
        );
        assert!(manager.connected_servers().await.is_empty());
        // Still present in config so it can be connected on demand by name.
        assert!(manager.config().servers.contains_key("off"));
    }

    #[tokio::test]
    async fn connect_on_first_call_fails_cleanly_for_broken_server() {
        // A configured server whose command exits immediately and never speaks
        // MCP. connect-on-first-call must surface a clean, bounded tool error
        // (connection failure) rather than hanging or panicking.
        let mut config = McpConfig::default();
        config.servers.insert(
            "broken".to_string(),
            McpServerConfig {
                // `true` exits 0 immediately: the stdio handshake gets EOF, so
                // connect fails fast instead of waiting on the initialize bound.
                command: "true".to_string(),
                args: vec![],
                env: HashMap::new(),
                shared: false,
                transport: None,
                url: None,
                enabled: None,
                disabled: None,
            },
        );
        let manager = McpManager::with_config(config);

        let started = std::time::Instant::now();
        let result = tokio::time::timeout(
            Duration::from_secs(40),
            manager.call_tool("broken", "anything", serde_json::json!({})),
        )
        .await;
        let inner = result.expect("call_tool must return, not hang");
        assert!(inner.is_err(), "broken server must yield a tool error");
        let msg = inner.unwrap_err().to_string();
        assert!(
            msg.contains("broken"),
            "tool error should name the server: {msg}"
        );
        assert!(
            started.elapsed() < Duration::from_secs(35),
            "connect-on-first-call must be bounded"
        );
    }
}

#[cfg(all(test, unix))]
mod provenance_integration_tests {
    use super::*;
    use std::io::Write;

    /// Write a minimal stdio MCP server as a shell script: answers
    /// initialize, tools/list, and tools/call with canned JSON-RPC replies.
    fn write_fake_mcp_server(dir: &std::path::Path) -> std::path::PathBuf {
        let path = dir.join("fake-mcp-server.sh");
        let script = r##"#!/bin/bash
while IFS= read -r line; do
  id=$(echo "$line" | grep -o '"id":[0-9]*' | grep -o '[0-9]*' | head -1)
  case "$line" in
    *'"initialize"'*)
      echo '{"jsonrpc":"2.0","id":'"$id"',"result":{"protocolVersion":"2024-11-05","capabilities":{"tools":{}},"serverInfo":{"name":"fake","version":"0.0.1"}}}'
      ;;
    *'"tools/list"'*)
      echo '{"jsonrpc":"2.0","id":'"$id"',"result":{"tools":[{"name":"create_card","description":"fake card","inputSchema":{"type":"object"}}]}}'
      ;;
    *'"tools/call"'*)
      echo '{"jsonrpc":"2.0","id":'"$id"',"result":{"content":[{"type":"text","text":"card created"}],"isError":false}}'
      ;;
    *'"shutdown"'*)
      exit 0
      ;;
  esac
done
"##;
        let mut file = std::fs::File::create(&path).unwrap();
        file.write_all(script.as_bytes()).unwrap();
        drop(file);
        let mut perms = std::fs::metadata(&path).unwrap().permissions();
        use std::os::unix::fs::PermissionsExt;
        perms.set_mode(0o755);
        std::fs::set_permissions(&path, perms).unwrap();
        path
    }

    /// Full loop: discovery records a setup, connecting a matching server
    /// tags provenance and counts the connect, real MCP tool calls through
    /// the manager are metered, non-matching servers are not.
    #[tokio::test]
    // The test-env mutex must intentionally stay held across awaits so the
    // JCODE_HOME/config mutation stays serialized for the whole test.
    #[allow(clippy::await_holding_lock)]
    async fn discovery_provenance_end_to_end_with_real_mcp_server() {
        let env_guard = crate::storage::lock_test_env();
        let temp = tempfile::tempdir().unwrap();
        crate::env::set_var("JCODE_HOME", temp.path());
        std::fs::write(
            temp.path().join("config.toml"),
            "[sponsors]\nenabled = true\n",
        )
        .unwrap();
        crate::config::Config::invalidate_cache();
        crate::sponsors::provenance::reset_for_tests();

        let server_path = write_fake_mcp_server(temp.path());
        let command = server_path.to_string_lossy().to_string();

        // 1. Discovery listing recorded this setup.
        crate::sponsors::provenance::record_discovered_setups(vec![
            crate::sponsors::provenance::DiscoveredSetup {
                sponsor: "agentcard".into(),
                command: command.clone(),
                args: vec![],
            },
        ]);

        // 2. Agent connects the matching server through the real manager.
        let mut config = McpConfig::default();
        config.servers.insert(
            "agentcard".to_string(),
            McpServerConfig {
                command: command.clone(),
                args: vec![],
                env: HashMap::new(),
                shared: false,
                transport: None,
                url: None,
                enabled: None,
                disabled: None,
            },
        );
        let manager = McpManager::with_config(config.clone());
        let server_config = config.servers.get("agentcard").unwrap().clone();
        manager
            .connect("agentcard", &server_config)
            .await
            .expect("fake MCP server must connect");
        assert!(crate::sponsors::provenance::is_tagged("agentcard"));

        // 3. Real tool calls through the manager are metered.
        let result = manager
            .call_tool("agentcard", "create_card", serde_json::json!({}))
            .await
            .expect("tool call through fake server");
        assert!(!result.is_error);

        // 4. A second, non-discovered server with a different command is
        // never tagged.
        assert!(!crate::sponsors::provenance::is_tagged("other"));

        // 5. Pending aggregates hold exactly the connect + the call.
        let reports = crate::sponsors::provenance::drain_pending_for_tests();
        assert_eq!(reports.len(), 1);
        assert_eq!(reports[0].sponsor, "agentcard");
        assert_eq!(reports[0].connects, 1);
        assert_eq!(reports[0].calls, 1);
        assert_eq!(reports[0].errors, 0);

        manager.disconnect_all().await;
        drop(env_guard);
    }
}
