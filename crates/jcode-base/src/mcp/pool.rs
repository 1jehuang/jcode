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
//!
//! # Project scoping
//!
//! Entries are keyed by [`PoolKey`], which pairs the server name with the
//! **project** the server was defined for. Keying by name alone would let a
//! session in project B inherit the already-running process that project A
//! started under the same server name, so B's agent would silently talk to A's
//! backend (see P1.1 in `docs/plans/PROJECT_ISOLATION_HARDENING_PLAN.md`).
//!
//! Two entries may share a process only when their resolved
//! [`McpServerConfig`]s are identical *and* they came from the same scope,
//! which is what lets a second session in the same project reuse a handle
//! while a different project always gets its own.

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

/// Identity of one pooled MCP server entry.
///
/// The scope is a stable project key (see [`crate::project_scope`]), never a
/// raw path string, so that two spellings of one project share a bucket and two
/// different projects cannot.
#[derive(Debug, Clone)]
struct PoolKey {
    scope: String,
    name: String,
    /// Directory this entry belongs to, used as the child process cwd. Two
    /// entries match when their scope and name match, so this never affects
    /// identity; it is carried so the process can be started in the right place.
    working_dir: Option<std::path::PathBuf>,
}

impl PoolKey {
    fn new(scope: String, name: impl Into<String>) -> Self {
        Self { scope, name: name.into(), working_dir: None }
    }

    /// Build a key that also remembers the directory the child should run in.
    fn with_dir(
        scope: String,
        name: impl Into<String>,
        working_dir: Option<&std::path::Path>,
    ) -> Self {
        Self {
            scope,
            name: name.into(),
            working_dir: working_dir.map(std::path::Path::to_path_buf),
        }
    }
}

impl PartialEq for PoolKey {
    fn eq(&self, other: &Self) -> bool {
        self.scope == other.scope && self.name == other.name
    }
}

impl Eq for PoolKey {}

impl std::hash::Hash for PoolKey {
    fn hash<H: std::hash::Hasher>(&self, state: &mut H) {
        self.scope.hash(state);
        self.name.hash(state);
    }
}

/// Global shared pool of MCP server processes.
///
/// Only one pool exists per jcode daemon. It owns the child processes
/// and hands out cheap `McpHandle` clones to sessions.
pub struct SharedMcpPool {
    clients: Mutex<HashMap<PoolKey, McpClient>>,
    handles: RwLock<HashMap<PoolKey, McpHandle>>,
    config: RwLock<McpConfig>,
    /// Directory against which the pool's default config was resolved. Keep it
    /// stable across reloads because the daemon may serve sessions in many dirs.
    config_dir: Option<std::path::PathBuf>,
    ref_counts: Mutex<HashMap<PoolKey, usize>>,
    connecting: Mutex<HashMap<PoolKey, Arc<Notify>>>,
    last_errors: RwLock<HashMap<PoolKey, FailedConnectRecord>>,
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
            connecting: Mutex::new(HashMap::new()),
            last_errors: RwLock::new(HashMap::new()),
        }
    }

    /// Create pool loading config from default locations.
    ///
    /// This bakes in whatever directory the *daemon* happens to run in, which
    /// is one arbitrary project. Sessions in other projects must not reuse that
    /// scope; their manager calls the `_scoped` entry points with its own scope
    /// instead.
    pub fn from_default_config() -> Self {
        let config_dir = std::env::current_dir().ok();
        let config = McpConfig::load_for_dir(config_dir.as_deref());
        Self::new_for_dir(config, config_dir)
    }

    /// The scope key this pool resolves its own default config under.
    fn default_scope(&self) -> String {
        crate::project_scope::optional_project_key(self.config_dir.as_deref())
    }

    /// Connect to all configured servers under this pool's own default scope.
    ///
    /// Prefer [`Self::connect_all_for_dir`] from a session path: it scopes
    /// entries to the session's project instead of the daemon's cwd.
    pub async fn connect_all(&self) -> (usize, Vec<(String, String)>) {
        self.connect_all_scoped(&self.default_scope()).await
    }


    /// Connect to all configured servers, filing every entry under `scope`.
    pub async fn connect_all_scoped(&self, scope: &str) -> (usize, Vec<(String, String)>) {
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
            // The pool's own config was resolved from `config_dir`, so that is
            // the only directory a config-only entry can be spawned in.
            let key = PoolKey::with_dir(
                scope.to_string(),
                name.clone(),
                self.config_dir.as_deref(),
            );
            connect_futures.push(async move {
                let result = self.ensure_connected(key.clone(), server_config).await;
                (key, result)
            });
        }
        drop(config);

        let mut successes = 0;
        let mut failures = Vec::new();

        for (key, result) in futures::future::join_all(connect_futures).await {
            match result {
                Ok(new_connection) => {
                    if new_connection {
                        successes += 1;
                    }
                }
                Err(error_msg) => {
                    crate::logging::error(&format!(
                        "Failed to connect to MCP server '{}': {}",
                        key.name, error_msg
                    ));
                    failures.push((key.name, error_msg));
                }
            }
        }

        if successes == 0 {
            successes = self.handles.read().await.keys().filter(|k| k.scope == scope).count();
        }

        (successes, failures)
    }

    /// Connect to a specific server by name and config, in this pool's default
    /// scope.
    ///
    /// Prefer [`Self::connect_server_scoped`] from a session path.
    pub async fn connect_server(&self, name: &str, config: &McpServerConfig) -> Result<()> {
        self.connect_server_scoped(&self.default_scope(), name, config).await
    }


    /// Connect a specific server, filing the entry under `scope`.
    pub async fn connect_server_scoped(
        &self,
        scope: &str,
        name: &str,
        config: &McpServerConfig,
    ) -> Result<()> {
        let key = PoolKey::new(scope.to_string(), name);
        self.ensure_connected(key.clone(), config.clone())
            .await
            .map(|_| ())
            .map_err(|error_msg| anyhow::anyhow!(error_msg))
            .with_context(|| format!("Failed to connect to MCP server '{}'", key.name))
    }

    /// Disconnect a specific server in this pool's default scope.
    pub async fn disconnect_server(&self, name: &str) {
        let scope = self.default_scope();
        self.disconnect_server_scoped(&scope, name).await;
    }

    /// Disconnect a specific server in `scope`.
    ///
    /// Scoped on purpose: tearing down a session's handle must never shut down a
    /// process that another project's sessions are still using.
    pub async fn disconnect_server_scoped(&self, scope: &str, name: &str) {
        let key = PoolKey::new(scope.to_string(), name);
        {
            let mut handles = self.handles.write().await;
            handles.remove(&key);
        }
        {
            let mut clients = self.clients.lock().await;
            if let Some(mut client) = clients.remove(&key) {
                client.shutdown().await;
            }
        }
        {
            let mut refs = self.ref_counts.lock().await;
            refs.remove(&key);
        }
        {
            let mut errors = self.last_errors.write().await;
            errors.remove(&key);
        }
    }

    /// Disconnect all servers in every scope.
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
        {
            let mut connecting = self.connecting.lock().await;
            connecting.clear();
        }
    }


    /// Disconnect only the servers connected under `scope`.
    pub async fn disconnect_all_scoped(&self, scope: &str) {
        let doomed: Vec<PoolKey> = {
            let handles = self.handles.read().await;
            handles.keys().filter(|key| key.scope == scope).cloned().collect()
        };
        for key in doomed {
            self.disconnect_server_scoped(scope, &key.name).await;
        }
    }

    /// Get handles for all connected servers in this pool's default scope.
    pub async fn acquire_handles(&self, session_id: &str) -> HashMap<String, McpHandle> {
        let scope = self.default_scope();
        self.acquire_handles_scoped(session_id, &scope).await
    }

    /// Get handles for every server connected under `scope`.
    ///
    /// Servers another project connected are excluded, which is what stops a
    /// session from being handed a process it has no business talking to.
    pub async fn acquire_handles_scoped(
        &self,
        session_id: &str,
        scope: &str,
    ) -> HashMap<String, McpHandle> {
        let result: HashMap<String, McpHandle> = {
            let handles = self.handles.read().await;
            handles
                .iter()
                .filter(|(key, _)| key.scope == scope)
                .map(|(key, handle)| (key.name.clone(), handle.clone()))
                .collect()
        };

        let mut refs = self.ref_counts.lock().await;
        for name in result.keys() {
            *refs
                .entry(PoolKey::new(scope.to_string(), name.clone()))
                .or_insert(0) += 1;
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
        let scope = self.default_scope();
        self.release_handles_in_dir(session_id, server_names, &scope).await;
    }

    /// Release handles counted against `scope`.
    pub async fn release_handles_scoped(
        &self,
        session_id: &str,
        server_names: &[String],
        scope: &str,
    ) {
        self.release_handles_in_dir(session_id, server_names, scope).await;
    }

    async fn release_handles_in_dir(
        &self,
        session_id: &str,
        server_names: &[String],
        scope: &str,
    ) {
        let mut refs = self.ref_counts.lock().await;
        for name in server_names {
            if let Some(count) = refs.get_mut(&PoolKey::new(scope.to_string(), name.clone())) {
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

    /// Get a handle for a specific server in this pool's default scope.
    pub async fn get_handle(&self, name: &str) -> Option<McpHandle> {
        let scope = self.default_scope();
        self.handles.read().await.get(&PoolKey::new(scope, name.to_string())).cloned()
    }

    /// Get a handle for a specific server in `scope`.
    pub async fn get_handle_scoped(&self, scope: &str, name: &str) -> Option<McpHandle> {
        self.handles.read().await.get(&PoolKey::new(scope.to_string(), name.to_string())).cloned()
    }

    /// Get all available tools from all connected servers
    pub async fn all_tools(&self) -> Vec<(String, McpToolDef)> {
        let handles = self.handles.read().await;
        let mut tools = Vec::new();
        for (key, handle) in handles.iter() {
            for tool in handle.tools() {
                tools.push((key.name.clone(), tool));
            }
        }
        tools
    }

    /// Get list of connected server names, across every scope.
    pub async fn connected_servers(&self) -> Vec<String> {
        let handles = self.handles.read().await;
        handles.keys().map(|key| key.name.clone()).collect()
    }

    /// Get list of connected server names within `scope`.
    pub async fn connected_servers_scoped(&self, scope: &str) -> Vec<String> {
        let handles = self.handles.read().await;
        handles.keys().filter(|key| key.scope == scope).map(|key| key.name.clone()).collect()
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
            .get(&PoolKey::new(self.default_scope(), server.to_string()))
            .with_context(|| format!("MCP server '{}' not connected", server))?;
        handle.call_tool(tool, arguments).await
    }

    /// Call a tool on a server within `scope`.
    ///
    /// Refuses rather than falling back to another project's entry: a silent
    /// fallback here is exactly the cross-project leak P1.1 is about.
    pub async fn call_tool_scoped(
        &self,
        scope: &str,
        server: &str,
        tool: &str,
        arguments: serde_json::Value,
    ) -> Result<super::protocol::ToolCallResult> {
        let handles = self.handles.read().await;
        let handle = handles
            .get(&PoolKey::new(scope.to_string(), server.to_string()))
            .with_context(|| format!("MCP server '{}' not connected", server))?;
        handle.call_tool(tool, arguments).await
    }

    /// Reload config and reconnect all servers in every scope.
    ///
    /// Config files are re-read once from the pool's own dir. Any scope that was
    /// populated from a *different* project keeps its own key; see
    /// [`Self::reload_for_dir`] for the session-facing path.
    pub async fn reload(&self) -> (usize, Vec<(String, String)>) {
        self.disconnect_all().await;
        *self.config.write().await = McpConfig::load_for_dir(self.config_dir.as_deref());
        self.connect_all().await
    }

    /// Reload config and reconnect only the servers in `scope`.
    pub async fn reload_scoped(&self, scope: &str) -> (usize, Vec<(String, String)>) {
        self.disconnect_all_scoped(scope).await;
        *self.config.write().await = McpConfig::load_for_dir(self.config_dir.as_deref());
        self.connect_all_scoped(scope).await
    }

    /// Get current config
    pub async fn config(&self) -> McpConfig {
        self.config.read().await.clone()
    }

    /// Check if any servers are connected
    pub async fn has_connections(&self) -> bool {
        let handles = self.handles.read().await;
        !handles.is_empty()
    }

    /// Get reference counts (for debugging), keyed by `"scope/name"`.
    ///
    /// Server names are no longer unique on their own, so the scope is included
    /// in the label; without it this view would merge two projects' counters.
    pub async fn ref_counts(&self) -> HashMap<String, usize> {
        self.ref_counts
            .lock()
            .await
            .iter()
            .map(|(key, count)| (format!("{}/{}", key.scope, key.name), *count))
            .collect()
    }

    /// Claim the right to connect `key`, or report that someone else already has.
    ///
    /// The comparison is on the whole [`PoolKey`], not the bare server name.
    /// Comparing names alone is the P1.1 bug: a session in project B would be
    /// told "already connected" about the process project A started.
    async fn begin_connect(&self, key: &PoolKey) -> ConnectAttempt {
        let mut connecting = self.connecting.lock().await;
        if let Some(notify) = connecting.get(key) {
            return ConnectAttempt::Wait(Arc::clone(notify));
        }

        if self.handles.read().await.contains_key(key) {
            return ConnectAttempt::Connected;
        }

        let notify = Arc::new(Notify::new());
        connecting.insert(key.clone(), Arc::clone(&notify));
        ConnectAttempt::Leader(notify)
    }

    async fn finish_connect(&self, key: PoolKey, notify: Arc<Notify>, result: Result<McpClient>) {
        match result {
            Ok(client) => {
                let handle = client.handle();
                {
                    let mut handles = self.handles.write().await;
                    handles.insert(key.clone(), handle);
                }
                {
                    let mut clients = self.clients.lock().await;
                    clients.insert(key.clone(), client);
                }
                {
                    let mut errors = self.last_errors.write().await;
                    errors.remove(&key);
                }
            }
            Err(error) => {
                let mut errors = self.last_errors.write().await;
                errors.insert(
                    key.clone(),
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
                .get(&key)
                .map(|current| Arc::ptr_eq(current, &notify))
                .unwrap_or(false)
            {
                connecting.remove(&key);
            }
        }

        notify.notify_waiters();
    }

    async fn ensure_connected(
        &self,
        key: PoolKey,
        config: McpServerConfig,
    ) -> std::result::Result<bool, String> {
        if let Some(record) = self.recent_failure(&key).await {
            let retry_after = FAILED_CONNECT_RETRY_COOLDOWN
                .saturating_sub(record.failed_at.elapsed())
                .as_secs()
                .max(1);
            crate::logging::info(&format!(
                "MCP: Skipping reconnect to '{}' (scope {}) for {}s after recent failure",
                key.name, key.scope, retry_after
            ));
            return Err(format!(
                "{} (retry suppressed for ~{}s after recent failure)",
                record.message, retry_after
            ));
        }

        match self.begin_connect(&key).await {
            ConnectAttempt::Connected => Ok(false),
            ConnectAttempt::Wait(notify) => {
                notify.notified().await;
                if self.handles.read().await.contains_key(&key) {
                    Ok(false)
                } else {
                    let error = self
                        .last_errors
                        .read()
                        .await
                        .get(&key)
                        .map(|record| record.message.clone())
                        .unwrap_or_else(|| {
                            "Connection attempt did not produce a handle".to_string()
                        });
                    Err(error)
                }
            }
            ConnectAttempt::Leader(notify) => {
                // Run the child in the directory this entry belongs to, not the
                // daemon's cwd: a project-local server that resolves paths
                // relative to its cwd must see its own project (P1.1).
                let result = McpClient::connect_in_dir(
                    key.name.clone(),
                    &config,
                    key.working_dir.as_deref(),
                )
                .await;
                let outcome = match &result {
                    Ok(_) => Ok(true),
                    Err(error) => Err(format!("{:#}", error)),
                };
                self.finish_connect(key, notify, result).await;
                outcome
            }
        }
    }

    async fn recent_failure(&self, key: &PoolKey) -> Option<FailedConnectRecord> {
        if self.handles.read().await.contains_key(key) {
            return None;
        }

        self.last_errors
            .read()
            .await
            .get(key)
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
    use super::{ConnectAttempt, PoolKey, SharedMcpPool};
    use crate::mcp::protocol::{McpConfig, McpServerConfig};
    use std::collections::HashMap;
    use std::sync::Arc;

    /// A stdio MCP server that reports its own cwd and pid as its identity.
    ///
    /// Two projects sharing a server *name* must end up with two processes; the
    /// only way to prove that is to have each process say which one it is. The
    /// `whereami` tool returns that identity so a test can call it and compare.
    const IDENTITY_SERVER: &str = r#"
import json, os, sys
for line in sys.stdin:
    req = json.loads(line)
    if req.get('method') == 'shutdown':
        break
    if 'id' not in req:
        continue
    method = req.get('method')
    if method == 'initialize':
        result = {'protocolVersion': '2024-11-05', 'capabilities': {'tools': {}},
                  'serverInfo': {'name': os.getcwd(), 'version': '1'}}
    elif method == 'tools/list':
        result = {'tools': [{'name': 'whereami', 'description': 'cwd and pid',
                             'inputSchema': {'type': 'object', 'properties': {}}}]}
    elif method == 'tools/call':
        text = '%s:%s' % (os.getcwd(), os.getpid())
        result = {'content': [{'type': 'text', 'text': text}], 'isError': False}
    else:
        continue
    print(json.dumps({'jsonrpc': '2.0', 'id': req['id'], 'result': result}), flush=True)
"#;

    fn identity_server() -> McpServerConfig {
        McpServerConfig {
            command: "python3".to_string(),
            args: vec![
                "-I".to_string(),
                "-S".to_string(),
                "-u".to_string(),
                "-c".to_string(),
                IDENTITY_SERVER.to_string(),
            ],
            env: Default::default(),
            shared: true,
            transport: None,
            url: None,
            headers: Default::default(),
            enabled: None,
            disabled: None,
            timeout_secs: Some(10),
        }
    }

    /// True when python3 can run the identity fixture.
    fn python_available() -> bool {
        std::process::Command::new("python3")
            .arg("--version")
            .stdout(std::process::Stdio::null())
            .stderr(std::process::Stdio::null())
            .status()
            .is_ok_and(|status| status.success())
    }

    /// Call the fixture's `whereami` tool and return its reported `cwd:pid`.
    async fn ask_identity(handle: &crate::mcp::McpHandle) -> String {
        let result = handle
            .call_tool("whereami", serde_json::json!({}))
            .await
            .expect("whereami call should succeed");
        assert!(!result.is_error, "whereami reported an error: {result:?}");
        result
            .content
            .iter()
            .filter_map(|block| match block {
                crate::mcp::protocol::ContentBlock::Text { text } => Some(text.clone()),
                _ => None,
            })
            .collect::<Vec<_>>()
            .join("")
    }

    fn pool_with_identity_server() -> Arc<SharedMcpPool> {
        let mut config = McpConfig::default();
        config.servers.insert("db".to_string(), identity_server());
        Arc::new(SharedMcpPool::new(config))
    }

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

        let first = pool.begin_connect(&PoolKey::new("scope".to_string(), "demo")).await;
        let second = pool.begin_connect(&PoolKey::new("scope".to_string(), "demo")).await;

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

    #[tokio::test]
    async fn same_server_name_in_two_projects_gets_two_separate_processes() {
        if !python_available() {
            eprintln!("SKIP: python3 unavailable for real MCP fixtures");
            return;
        }
        let pool = pool_with_identity_server();
        let project_a = tempfile::tempdir().expect("project a");
        let project_b = tempfile::tempdir().expect("project b");
        let scope_a = crate::project_scope::project_key(project_a.path());
        let scope_b = crate::project_scope::project_key(project_b.path());

        for scope in [&scope_a, &scope_b] {
            let (successes, failures) = pool.connect_all_scoped(scope).await;
            assert!(failures.is_empty(), "connect failed: {failures:?}");
            assert_eq!(successes, 1, "one server should connect for scope {scope}");
        }

        // Each project must receive a handle, and the two handles must be backed
        // by different processes reporting different cwds.
        let handles_a = pool.acquire_handles_scoped("session-a", &scope_a).await;
        let handles_b = pool.acquire_handles_scoped("session-b", &scope_b).await;

        let handle_a = handles_a
            .get("db")
            .expect("project A should have a db handle")
            .clone();
        let handle_b = handles_b
            .get("db")
            .expect("project B should have a db handle")
            .clone();

        let identity_a = ask_identity(&handle_a).await;
        let identity_b = ask_identity(&handle_b).await;

        assert_ne!(
            identity_a, identity_b,
            "project B inherited project A's process for server 'db'"
        );

        pool.disconnect_all().await;
    }

    #[tokio::test]
    async fn a_session_cannot_acquire_another_projects_server_handle() {
        if !python_available() {
            eprintln!("SKIP: python3 unavailable for real MCP fixtures");
            return;
        }
        let pool = pool_with_identity_server();
        let project_a = tempfile::tempdir().expect("project a");
        let project_b = tempfile::tempdir().expect("project b");
        let scope_a = crate::project_scope::project_key(project_a.path());
        let scope_b = crate::project_scope::project_key(project_b.path());

        let (successes, failures) = pool.connect_all_scoped(&scope_a).await;
        assert!(failures.is_empty(), "connect failed: {failures:?}");
        assert_eq!(successes, 1);

        // Project B has never connected anything, so it must be handed no
        // handle at all rather than the running process from project A.
        let handles_b = pool.acquire_handles_scoped("session-b", &scope_b).await;
        assert!(
            handles_b.is_empty(),
            "project B acquired project A's server: {:?}",
            handles_b.keys().collect::<Vec<_>>()
        );

        pool.disconnect_all().await;
    }

    #[tokio::test]
    async fn sessions_in_the_same_project_share_one_process() {
        if !python_available() {
            eprintln!("SKIP: python3 unavailable for real MCP fixtures");
            return;
        }
        let pool = pool_with_identity_server();
        let project = tempfile::tempdir().expect("project");
        let scope = crate::project_scope::project_key(project.path());

        let (successes, failures) = pool.connect_all_scoped(&scope).await;
        assert!(failures.is_empty(), "connect failed: {failures:?}");
        assert_eq!(successes, 1);

        // A second connect in the same project must reuse the entry, not spawn
        // a duplicate: sharing within a project is the pool's whole purpose.
        let (second_successes, second_failures) = pool.connect_all_scoped(&scope).await;
        assert!(second_failures.is_empty(), "second connect failed: {second_failures:?}");
        assert_eq!(
            second_successes, 1,
            "the pool must still report the server as available"
        );

        let first = pool
            .acquire_handles_scoped("session-1", &scope)
            .await
            .remove("db")
            .expect("first session should get a handle");
        let second = pool
            .acquire_handles_scoped("session-2", &scope)
            .await
            .remove("db")
            .expect("second session should get a handle");

        assert_eq!(
            ask_identity(&first).await,
            ask_identity(&second).await,
            "two sessions in one project should share one process"
        );

        pool.disconnect_all().await;
    }

    #[tokio::test]
    async fn two_spellings_of_one_project_share_a_pool_entry() {
        // Canonicalization must happen before keying, or `/repo` and `/repo/.`
        // would spawn two processes for one project.
        let project = tempfile::tempdir().expect("project");
        let dotted = project.path().join(".");
        let scope_direct = crate::project_scope::project_key(project.path());
        let scope_dotted = crate::project_scope::project_key(&dotted);

        assert_eq!(scope_direct, scope_dotted);
    }

    #[tokio::test]
    async fn ref_counts_separate_projects_that_share_a_server_name() {
        let pool = SharedMcpPool::new(McpConfig::default());
        {
            let mut refs = pool.ref_counts.lock().await;
            refs.insert(PoolKey::new("scope-a".to_string(), "db"), 2);
            refs.insert(PoolKey::new("scope-b".to_string(), "db"), 1);
        }

        let counts: HashMap<String, usize> = pool.ref_counts().await;

        assert_eq!(counts.get("scope-a/db"), Some(&2));
        assert_eq!(counts.get("scope-b/db"), Some(&1));
        assert_eq!(
            counts.len(),
            2,
            "same-named servers in two projects must not merge: {counts:?}"
        );
    }

    #[tokio::test]
    async fn disconnecting_one_project_leaves_another_projects_process_running() {
        if !python_available() {
            eprintln!("SKIP: python3 unavailable for real MCP fixtures");
            return;
        }
        let pool = pool_with_identity_server();
        let project_a = tempfile::tempdir().expect("project a");
        let project_b = tempfile::tempdir().expect("project b");
        let scope_a = crate::project_scope::project_key(project_a.path());
        let scope_b = crate::project_scope::project_key(project_b.path());

        for scope in [&scope_a, &scope_b] {
            pool.connect_all_scoped(scope).await;
        }

        pool.disconnect_all_scoped(&scope_a).await;

        assert!(
            pool.acquire_handles_scoped("session-a", &scope_a).await.is_empty(),
            "project A's server should be gone"
        );
        let survivors = pool.acquire_handles_scoped("session-b", &scope_b).await;
        assert!(
            survivors.contains_key("db"),
            "project A's teardown must not take project B's process down"
        );

        pool.disconnect_all().await;
    }
}
