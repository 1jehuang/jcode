//! Shared MCP Server Pool
//!
//! Manages a global pool of MCP server processes that are shared across
//! all jcode sessions. Instead of each session spawning its own set of
//! MCP servers (N sessions × M servers = N×M processes), sessions share
//! a single pool (M processes total).
//!
//! Sessions get lightweight `McpHandle` clones that can send concurrent
//! requests to shared server processes. Request/response correlation by
//! ID ensures no interference between sessions.

use super::client::{McpClient, McpHandle};
use super::protocol::{McpConfig, McpServerConfig, McpToolDef};
use anyhow::{Context, Result};
use std::collections::HashMap;
use std::sync::Arc;
use std::time::{Duration, Instant};
use tokio::sync::{Mutex, Notify, RwLock};

const FAILED_CONNECT_RETRY_COOLDOWN: Duration = Duration::from_secs(30);

include!("pool_attempt.rs");
include!("pool_state.rs");
include!("pool_handle_lease.rs");

impl SharedMcpPool {
    #[cfg(all(windows, feature = "test-support"))]
    pub fn test_wait_points(&self) -> [usize; 4] {
        self.test_wait_points
            .each_ref()
            .map(|counter| counter.load(std::sync::atomic::Ordering::SeqCst))
    }

    #[cfg(all(windows, feature = "test-support"))]
    pub async fn test_with_owned_job(&self, name: &str, check: impl FnOnce(usize)) {
        let owner = self.clients.lock().await.get(name).cloned().unwrap();
        let client = owner.lock().await;
        check(client.test_owned_job_handle().expect("native owned job"));
    }

    #[cfg(test)]
    pub(super) fn test_pending_drop_releases(&self) -> usize {
        self.released_references.lock().unwrap().len()
    }

    #[cfg(any(test, all(windows, feature = "test-support")))]
    pub async fn test_lock_ref_counts(
        &self,
    ) -> tokio::sync::MutexGuard<'_, HashMap<String, usize>> {
        self.ref_counts.lock().await
    }

    #[cfg(test)]
    pub(super) fn test_manager_reference(&self, reference_id: u64) -> bool {
        self.session_refs
            .lock()
            .unwrap()
            .contains_key(&PoolReferenceOwner::Manager(reference_id))
    }

    #[cfg(all(windows, feature = "test-support"))]
    pub async fn test_adopt_natural_client(&self, name: String, client: McpClient) {
        assert!(!self.clients.lock().await.contains_key(&name));
        self.handles
            .write()
            .await
            .insert(name.clone(), client.handle());
        self.clients
            .lock()
            .await
            .insert(name, Arc::new(Mutex::new(client)));
    }

    /// Create a new shared pool with the given config
    pub fn new(config: McpConfig) -> Self {
        Self::new_for_dir(config, std::env::current_dir().ok())
    }

    fn new_for_dir(config: McpConfig, config_dir: Option<std::path::PathBuf>) -> Self {
        Self {
            #[cfg(all(windows, feature = "test-support"))]
            test_wait_points: std::array::from_fn(|_| std::sync::atomic::AtomicUsize::new(0)),
            admission: RwLock::new(()),
            cleanup_pending: std::sync::atomic::AtomicBool::new(false),
            clients: Mutex::new(HashMap::new()),
            handles: RwLock::new(HashMap::new()),
            config: RwLock::new(config),
            config_dir,
            ref_counts: Mutex::new(HashMap::new()),
            session_refs: std::sync::Mutex::new(HashMap::new()),
            released_references: std::sync::Mutex::new(Vec::new()),
            connecting: Arc::new(std::sync::Mutex::new(HashMap::new())),
            last_errors: RwLock::new(HashMap::new()),
        }
    }

    /// Create pool loading config from default locations
    pub fn from_default_config() -> Self {
        let config_dir = std::env::current_dir().ok();
        let config = McpConfig::load_for_dir(config_dir.as_deref());
        Self::new_for_dir(config, config_dir)
    }

    /// Connect to all configured servers.
    /// Returns (successes, failures).
    pub async fn connect_all(&self) -> (usize, Vec<(String, String)>) {
        let _admission = self.admission.read().await;
        if self.cleanup_pending() {
            return (
                0,
                vec![(
                    "pool".into(),
                    "MCP pool cleanup is unconfirmed; retry disconnect_all or reload".into(),
                )],
            );
        }
        self.connect_all_inner().await
    }
    async fn connect_all_inner(&self) -> (usize, Vec<(String, String)>) {
        self.connect_all_using(
            |name, config| async move { McpClient::connect(name, &config).await },
        )
        .await
    }
    async fn connect_all_using<C, F>(&self, connect: C) -> (usize, Vec<(String, String)>)
    where
        C: Fn(String, McpServerConfig) -> F + Clone,
        F: std::future::Future<Output = Result<McpClient>>,
    {
        let config = self.config.read().await;
        let mut connect_futures = Vec::new();

        for (name, server_config) in &config.servers {
            // Disabled servers stay configured but are never auto-spawned
            // (issue #436); they can still be connected on demand by name.
            if !server_config.is_enabled() {
                continue;
            }
            // Non-shared servers are owned per-session (with the session cwd)
            // and must never be spawned in the daemon-global pool (issue #557).
            if !server_config.shared {
                continue;
            }
            let name = name.clone();
            let server_config = server_config.clone();
            let connect = connect.clone();
            connect_futures.push(async move {
                let result = self
                    .ensure_connected_using(name.clone(), server_config, connect)
                    .await;
                (name, result)
            });
        }
        drop(config);

        let mut successes = 0;
        let mut failures = Vec::new();

        for (name, result) in futures::future::join_all(connect_futures).await {
            match result {
                Ok(new_connection) => {
                    if new_connection {
                        successes += 1;
                    }
                }
                Err(error_msg) => {
                    crate::logging::error(&format!(
                        "Failed to connect to MCP server '{}': {}",
                        name, error_msg
                    ));
                    failures.push((name, error_msg));
                }
            }
        }

        if successes == 0 {
            successes = self
                .handles
                .read()
                .await
                .values()
                .filter(|handle| !handle.is_closed())
                .count();
        }

        (successes, failures)
    }

    /// Get all available tools from all connected servers
    pub async fn all_tools(&self) -> Vec<(String, McpToolDef)> {
        let _admission = self.admission.read().await;
        if self.cleanup_pending() {
            return Vec::new();
        }
        let handles = self.handles.read().await;
        let mut tools = Vec::new();
        for (server_name, handle) in handles.iter() {
            if handle.is_closed() {
                continue;
            }
            for tool in handle.tools() {
                tools.push((server_name.clone(), tool));
            }
        }
        tools
    }

    /// Get list of connected server names
    pub async fn connected_servers(&self) -> Vec<String> {
        let _admission = self.admission.read().await;
        if self.cleanup_pending() {
            return Vec::new();
        }
        let handles = self.handles.read().await;
        handles
            .iter()
            .filter(|(_, handle)| !handle.is_closed())
            .map(|(name, _)| name.clone())
            .collect()
    }

    /// Call a tool on a specific server
    pub async fn call_tool(
        &self,
        server: &str,
        tool: &str,
        arguments: serde_json::Value,
    ) -> Result<super::protocol::ToolCallResult> {
        let handle = {
            let _admission = self.admission.read().await;
            if self.cleanup_pending() {
                anyhow::bail!("MCP pool cleanup is unconfirmed; retry disconnect_all or reload");
            }
            self.handles
                .read()
                .await
                .get(server)
                .filter(|handle| !handle.is_closed())
                .cloned()
                .with_context(|| format!("MCP server '{}' not connected", server))?
        };
        handle.call_tool(tool, arguments).await
    }

    /// Reload config and reconnect all servers.
    /// Cancellation after the swap retains the new config and blocks admission
    /// until a successful disconnect_all or reload retry confirms cleanup.
    pub async fn reload(&self) -> Result<(usize, Vec<(String, String)>)> {
        self.reload_with(
            || McpConfig::load_for_dir(self.config_dir.as_deref()),
            std::future::ready(()),
        )
        .await
    }
    async fn reload_with<L, H>(
        &self,
        load_config: L,
        after_swap: H,
    ) -> Result<(usize, Vec<(String, String)>)>
    where
        L: FnOnce() -> McpConfig,
        H: std::future::Future<Output = ()>,
    {
        self.reload_using(load_config, after_swap, |name, config| async move {
            McpClient::connect(name, &config).await
        })
        .await
    }
    async fn reload_using<L, H, C, F>(
        &self,
        load_config: L,
        after_swap: H,
        connect: C,
    ) -> Result<(usize, Vec<(String, String)>)>
    where
        L: FnOnce() -> McpConfig,
        H: std::future::Future<Output = ()>,
        C: Fn(String, McpServerConfig) -> F + Clone,
        F: std::future::Future<Output = Result<McpClient>>,
    {
        let _admission = self.admission.write().await;
        self.cleanup_pending
            .store(true, std::sync::atomic::Ordering::SeqCst);
        self.disconnect_all_inner(std::future::ready(())).await?;
        *self.config.write().await = load_config();
        after_swap.await;
        let result = self.connect_all_using(connect).await;
        self.cleanup_pending
            .store(false, std::sync::atomic::Ordering::SeqCst);
        Ok(result)
    }

    /// Get current config
    pub async fn config(&self) -> McpConfig {
        self.config.read().await.clone()
    }

    /// Check if any servers are connected
    pub async fn has_connections(&self) -> bool {
        let _admission = self.admission.read().await;
        if self.cleanup_pending() {
            return false;
        }
        let handles = self.handles.read().await;
        handles.values().any(|handle| !handle.is_closed())
    }

    /// Get reference counts (for debugging)
    pub async fn ref_counts(&self) -> HashMap<String, usize> {
        let mut refs = self.ref_counts.lock().await;
        self.flush_released_references(&mut refs);
        refs.clone()
    }
}

include!("pool_connect.rs");

/// Global pool singleton
static SHARED_POOL: tokio::sync::OnceCell<Arc<SharedMcpPool>> = tokio::sync::OnceCell::const_new();

/// Initialize the global shared MCP pool. Call once at daemon startup.
pub async fn init_shared_pool() -> Arc<SharedMcpPool> {
    SHARED_POOL
        .get_or_init(|| async {
            let pool = SharedMcpPool::from_default_config();
            Arc::new(pool)
        })
        .await
        .clone()
}

/// Get the global shared pool, if initialized.
pub fn get_shared_pool() -> Option<Arc<SharedMcpPool>> {
    SHARED_POOL.get().cloned()
}

include!("pool_tests.rs");

include!("pool_cancellation_tests.rs");

include!("pool_disconnect_tests.rs");

include!("pool_admission_tests.rs");
include!("pool_handle_tests.rs");

include!("pool_lifecycle.rs");

include!("pool_handles.rs");
