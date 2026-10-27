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

#[derive(Clone)]
struct FailedConnectRecord {
    message: String,
    failed_at: Instant,
}

enum ConnectAttempt {
    Connected,
    Leader(Arc<Notify>),
    Wait(Arc<Notify>),
}

type Connecting = Arc<Mutex<HashMap<String, Arc<Notify>>>>;

/// Clears a leader's `connecting` entry when its future is dropped mid-connect
/// (e.g. by an outer timeout), so waiters wake and later sessions can retry.
struct LeaderGuard {
    connecting: Connecting,
    name: String,
    notify: Option<Arc<Notify>>,
}

impl Drop for LeaderGuard {
    fn drop(&mut self) {
        let Some(notify) = self.notify.take() else {
            return;
        };
        let Ok(runtime) = tokio::runtime::Handle::try_current() else {
            return;
        };
        let connecting = Arc::clone(&self.connecting);
        let name = std::mem::take(&mut self.name);
        runtime.spawn(async move {
            let mut map = connecting.lock().await;
            if map.get(&name).is_some_and(|current| Arc::ptr_eq(current, &notify)) {
                map.remove(&name);
            }
            drop(map);
            notify.notify_waiters();
        });
    }
}

/// Global shared pool of MCP server processes.
///
/// Only one pool exists per jcode daemon. It owns the child processes
/// and hands out cheap `McpHandle` clones to sessions.
pub struct SharedMcpPool {
    clients: Mutex<HashMap<String, McpClient>>,
    handles: RwLock<HashMap<String, McpHandle>>,
    config: RwLock<McpConfig>,
    /// Directory against which the pool's default config was resolved. Keep it
    /// stable across reloads because the daemon may serve sessions in many dirs.
    config_dir: Option<std::path::PathBuf>,
    ref_counts: Mutex<HashMap<String, usize>>,
    connecting: Connecting,
    last_errors: RwLock<HashMap<String, FailedConnectRecord>>,
}

impl SharedMcpPool {
    /// Create a new shared pool with the given config
    pub fn new(config: McpConfig) -> Self {
        Self::new_for_dir(config, std::env::current_dir().ok())
    }

    fn new_for_dir(config: McpConfig, config_dir: Option<std::path::PathBuf>) -> Self {
        Self {
            clients: Mutex::new(HashMap::new()),
            handles: RwLock::new(HashMap::new()),
            config: RwLock::new(config),
            config_dir,
            ref_counts: Mutex::new(HashMap::new()),
            connecting: Arc::default(),
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
            connect_futures.push(async move {
                let result = self.ensure_connected(name.clone(), server_config).await;
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

    /// Connect to a specific server by name and config
    pub async fn connect_server(&self, name: &str, config: &McpServerConfig) -> Result<()> {
        self.ensure_connected(name.to_string(), config.clone())
            .await
            .map(|_| ())
            .map_err(|error_msg| anyhow::anyhow!(error_msg))
            .with_context(|| format!("Failed to connect to MCP server '{}'", name))
    }

    /// Disconnect a specific server
    pub async fn disconnect_server(&self, name: &str) {
        {
            let mut handles = self.handles.write().await;
            handles.remove(name);
        }
        {
            let mut clients = self.clients.lock().await;
            if let Some(mut client) = clients.remove(name) {
                client.shutdown().await;
            }
        }
        {
            let mut refs = self.ref_counts.lock().await;
            refs.remove(name);
        }
        {
            let mut errors = self.last_errors.write().await;
            errors.remove(name);
        }
    }

    /// Disconnect all servers
    pub async fn disconnect_all(&self) {
        {
            let mut handles = self.handles.write().await;
            handles.clear();
        }
        {
            let mut clients = self.clients.lock().await;
            for (_, mut client) in clients.drain() {
                client.shutdown().await;
            }
        }
        {
            let mut refs = self.ref_counts.lock().await;
            refs.clear();
        }
        {
            let mut errors = self.last_errors.write().await;
            errors.clear();
        }
    }

    /// Get handles for all connected servers (for a new session).
    /// Increments reference counts.
    pub async fn acquire_handles(&self, session_id: &str) -> HashMap<String, McpHandle> {
        let handles = self.handles.read().await;
        let result: HashMap<String, McpHandle> = handles
            .iter()
            .filter(|(_, handle)| !handle.is_closed())
            .map(|(name, handle)| (name.clone(), handle.clone()))
            .collect();

        let mut refs = self.ref_counts.lock().await;
        for name in result.keys() {
            *refs.entry(name.clone()).or_insert(0) += 1;
        }

        if !result.is_empty() {
            crate::logging::info(&format!(
                "MCP pool: session '{}' acquired {} server handle(s)",
                session_id,
                result.len()
            ));
        }

        result
    }

    /// Release handles when a session disconnects.
    /// Decrements reference counts.
    pub async fn release_handles(&self, session_id: &str, server_names: &[String]) {
        let mut refs = self.ref_counts.lock().await;
        for name in server_names {
            if let Some(count) = refs.get_mut(name) {
                *count = count.saturating_sub(1);
            }
        }

        if !server_names.is_empty() {
            crate::logging::info(&format!(
                "MCP pool: session '{}' released {} server handle(s)",
                session_id,
                server_names.len()
            ));
        }
    }

    /// Get a handle for a specific server
    pub async fn get_handle(&self, name: &str) -> Option<McpHandle> {
        let handles = self.handles.read().await;
        handles
            .get(name)
            .filter(|handle| !handle.is_closed())
            .cloned()
    }

    /// Get all available tools from all connected servers
    pub async fn all_tools(&self) -> Vec<(String, McpToolDef)> {
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
        let handles = self.handles.read().await;
        let handle = handles
            .get(server)
            .filter(|handle| !handle.is_closed())
            .with_context(|| format!("MCP server '{}' not connected", server))?;
        handle.call_tool(tool, arguments).await
    }

    /// Reload config and reconnect all servers
    pub async fn reload(&self) -> (usize, Vec<(String, String)>) {
        self.disconnect_all().await;
        *self.config.write().await = McpConfig::load_for_dir(self.config_dir.as_deref());
        self.connect_all().await
    }

    /// Get current config
    pub async fn config(&self) -> McpConfig {
        self.config.read().await.clone()
    }

    /// Check if any servers are connected
    pub async fn has_connections(&self) -> bool {
        let handles = self.handles.read().await;
        handles.values().any(|handle| !handle.is_closed())
    }

    /// Get reference counts (for debugging)
    pub async fn ref_counts(&self) -> HashMap<String, usize> {
        self.ref_counts.lock().await.clone()
    }

    async fn begin_connect(&self, name: &str) -> ConnectAttempt {
        let mut connecting = self.connecting.lock().await;
        if let Some(notify) = connecting.get(name) {
            return ConnectAttempt::Wait(Arc::clone(notify));
        }

        {
            let mut handles = self.handles.write().await;
            if handles.get(name).is_some_and(|handle| !handle.is_closed()) {
                return ConnectAttempt::Connected;
            }
            handles.remove(name);
        }
        // Preserve reference counts held by existing sessions, but release the
        // dead transport before starting a fresh initialization handshake.
        self.clients.lock().await.remove(name);

        let notify = Arc::new(Notify::new());
        connecting.insert(name.to_string(), Arc::clone(&notify));
        ConnectAttempt::Leader(notify)
    }

    async fn finish_connect(&self, name: &str, notify: Arc<Notify>, result: Result<McpClient>) {
        match result {
            Ok(client) => {
                let handle = client.handle();
                {
                    let mut handles = self.handles.write().await;
                    handles.insert(name.to_string(), handle);
                }
                {
                    let mut clients = self.clients.lock().await;
                    clients.insert(name.to_string(), client);
                }
                {
                    let mut errors = self.last_errors.write().await;
                    errors.remove(name);
                }
            }
            Err(error) => {
                let mut errors = self.last_errors.write().await;
                errors.insert(
                    name.to_string(),
                    FailedConnectRecord {
                        message: format!("{:#}", error),
                        failed_at: Instant::now(),
                    },
                );
            }
        }

        {
            let mut connecting = self.connecting.lock().await;
            if connecting
                .get(name)
                .map(|current| Arc::ptr_eq(current, &notify))
                .unwrap_or(false)
            {
                connecting.remove(name);
            }
        }

        notify.notify_waiters();
    }

    async fn ensure_connected(
        &self,
        name: String,
        config: McpServerConfig,
    ) -> std::result::Result<bool, String> {
        if let Some(record) = self.recent_failure(&name).await {
            let retry_after = FAILED_CONNECT_RETRY_COOLDOWN
                .saturating_sub(record.failed_at.elapsed())
                .as_secs()
                .max(1);
            crate::logging::info(&format!(
                "MCP: Skipping reconnect to '{}' for {}s after recent failure",
                name, retry_after
            ));
            return Err(format!(
                "{} (retry suppressed for ~{}s after recent failure)",
                record.message, retry_after
            ));
        }

        match self.begin_connect(&name).await {
            ConnectAttempt::Connected => Ok(false),
            ConnectAttempt::Wait(notify) => {
                // Another session leads this handshake; never wait longer than
                // this server's own request budget for it.
                let limit = super::client::request_timeout_for(&config);
                if tokio::time::timeout(limit, self.wait_for_connect(&name, &notify))
                    .await
                    .is_err()
                {
                    return Err(format!(
                        "MCP server '{name}' is still connecting after {}s",
                        limit.as_secs()
                    ));
                }
                if self
                    .handles
                    .read()
                    .await
                    .get(&name)
                    .is_some_and(|handle| !handle.is_closed())
                {
                    Ok(false)
                } else {
                    let error = self
                        .last_errors
                        .read()
                        .await
                        .get(&name)
                        .map(|record| record.message.clone())
                        .unwrap_or_else(|| {
                            "Connection attempt did not produce a handle".to_string()
                        });
                    Err(error)
                }
            }
            ConnectAttempt::Leader(notify) => {
                let mut guard = LeaderGuard {
                    connecting: Arc::clone(&self.connecting),
                    name: name.clone(),
                    notify: Some(Arc::clone(&notify)),
                };
                let result = McpClient::connect(name.clone(), &config).await;
                let outcome = match &result {
                    Ok(_) => Ok(true),
                    Err(error) => Err(format!("{:#}", error)),
                };
                self.finish_connect(&name, notify, result).await;
                guard.notify = None;
                outcome
            }
        }
    }

    async fn wait_for_connect(&self, name: &str, notify: &Arc<Notify>) {
        let notified = notify.notified();
        tokio::pin!(notified);
        notified.as_mut().enable();
        // Registration must precede this check because notify_waiters does
        // not retain a permit when the leader finishes before we start waiting.
        if self
            .connecting
            .lock()
            .await
            .get(name)
            .is_some_and(|current| Arc::ptr_eq(current, notify))
        {
            notified.await;
        }
    }

    async fn recent_failure(&self, name: &str) -> Option<FailedConnectRecord> {
        if self
            .handles
            .read()
            .await
            .get(name)
            .is_some_and(|handle| !handle.is_closed())
        {
            return None;
        }

        self.last_errors
            .read()
            .await
            .get(name)
            .filter(|record| record.failed_at.elapsed() < FAILED_CONNECT_RETRY_COOLDOWN)
            .cloned()
    }
}

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

#[cfg(test)]
mod tests {
    use super::{ConnectAttempt, SharedMcpPool};
    use crate::mcp::protocol::McpConfig;
    use std::sync::Arc;

    #[tokio::test]
    async fn issue_790_reload_reuses_default_config_directory() {
        let _guard = crate::storage::lock_test_env();
        let original_cwd = std::env::current_dir().expect("current cwd");
        let previous_home = std::env::var_os("JCODE_HOME");
        let home = tempfile::tempdir().expect("home tempdir");
        let first_project = tempfile::tempdir().expect("first project tempdir");
        let second_project = tempfile::tempdir().expect("second project tempdir");
        crate::env::set_var("JCODE_HOME", home.path());
        std::fs::write(
            first_project.path().join(".mcp.json"),
            r#"{"mcpServers":{"first":{"command":"first-server","shared":false}}}"#,
        )
        .expect("write first project config");
        std::fs::write(
            second_project.path().join(".mcp.json"),
            r#"{"mcpServers":{"second":{"command":"second-server","shared":false}}}"#,
        )
        .expect("write second project config");

        std::env::set_current_dir(first_project.path()).expect("set first project cwd");
        let pool = SharedMcpPool::from_default_config();
        let initially_loaded_first = pool.config().await.servers.contains_key("first");

        std::fs::write(
            first_project.path().join(".mcp.json"),
            r#"{"mcpServers":{"first-reloaded":{"command":"first-reloaded-server","shared":false}}}"#,
        )
        .expect("update first project config");

        std::env::set_current_dir(second_project.path()).expect("set second project cwd");
        let _ = pool.reload().await;
        let reloaded = pool.config().await;

        std::env::set_current_dir(original_cwd).expect("restore cwd");
        if let Some(previous_home) = previous_home {
            crate::env::set_var("JCODE_HOME", previous_home);
        } else {
            crate::env::remove_var("JCODE_HOME");
        }

        assert!(initially_loaded_first);
        assert!(!reloaded.servers.contains_key("first"));
        assert!(reloaded.servers.contains_key("first-reloaded"));
        assert!(!reloaded.servers.contains_key("second"));
    }

    #[tokio::test]
    async fn begin_connect_deduplicates_concurrent_attempts() {
        let pool = Arc::new(SharedMcpPool::new(McpConfig::default()));

        let first = pool.begin_connect("demo").await;
        let second = pool.begin_connect("demo").await;

        let first_notify = match first {
            ConnectAttempt::Leader(notify) => notify,
            _ => panic!("first attempt should lead"),
        };
        let second_notify = match second {
            ConnectAttempt::Wait(notify) => notify,
            _ => panic!("second attempt should wait"),
        };

        assert!(Arc::ptr_eq(&first_notify, &second_notify));
    }

    #[tokio::test]
    async fn reconnect_waiter_observes_leader_completion_before_registration() {
        let pool = SharedMcpPool::new(McpConfig::default());
        let ConnectAttempt::Leader(leader) = pool.begin_connect("early").await else {
            panic!("leader");
        };
        let ConnectAttempt::Wait(waiter) = pool.begin_connect("early").await else {
            panic!("waiter");
        };
        pool.finish_connect("early", leader, Err(anyhow::anyhow!("fixture failure")))
            .await;
        tokio::time::timeout(
            std::time::Duration::from_millis(100),
            pool.wait_for_connect("early", &waiter),
        )
        .await
        .expect("an already-completed attempt must not wait for a lost notification");
    }

    fn sh_config(script: &str, timeout_secs: u64) -> crate::mcp::protocol::McpServerConfig {
        serde_json::from_value(serde_json::json!({
            "command": "/bin/sh", "args": ["-c", script], "timeout_secs": timeout_secs
        }))
        .unwrap()
    }

    #[tokio::test]
    async fn waiter_gives_up_on_a_leader_that_never_finishes() {
        let pool = SharedMcpPool::new(McpConfig::default());
        let ConnectAttempt::Leader(_stuck) = pool.begin_connect("stuck").await else {
            panic!("leader");
        };
        let result = tokio::time::timeout(
            std::time::Duration::from_secs(5),
            pool.ensure_connected("stuck".to_string(), sh_config("true", 1)),
        )
        .await
        .expect("a waiter must be bounded by the server's request timeout");
        assert!(result.unwrap_err().contains("still connecting"));
    }

    #[tokio::test]
    async fn dropped_leader_clears_its_connecting_entry() {
        let pool = SharedMcpPool::new(McpConfig::default());
        // An outer timeout (connect-on-call) drops the leader mid-handshake.
        let attempt = pool.ensure_connected("hang".to_string(), sh_config("exec sleep 5", 600));
        assert!(
            tokio::time::timeout(std::time::Duration::from_millis(200), attempt)
                .await
                .is_err()
        );
        tokio::time::timeout(std::time::Duration::from_secs(2), async {
            while pool.connecting.lock().await.contains_key("hang") {
                tokio::task::yield_now().await;
            }
        })
        .await
        .expect("a dropped leader must not wedge later sessions");
        assert!(matches!(
            pool.begin_connect("hang").await,
            ConnectAttempt::Leader(_)
        ));
    }

    #[tokio::test]
    async fn connect_all_skips_non_shared_servers() {
        // Issue #557: shared:false servers are owned per-session and must not
        // be spawned in the daemon-global pool. If the pool tried to connect
        // this nonexistent command it would show up as a failure.
        let mut config = McpConfig::default();
        config.servers.insert(
            "owned-only".to_string(),
            crate::mcp::protocol::McpServerConfig {
                command: "/nonexistent/jcode-test-mcp-557".to_string(),
                args: vec![],
                env: Default::default(),
                shared: false,
                transport: None,
                url: None,
                headers: std::collections::HashMap::new(),
                enabled: None,
                disabled: None,
                timeout_secs: None,
            },
        );
        let pool = SharedMcpPool::new(config);

        let (successes, failures) = pool.connect_all().await;
        assert_eq!(successes, 0);
        assert!(
            failures.is_empty(),
            "non-shared server must be skipped, got failures: {failures:?}"
        );
    }
}
