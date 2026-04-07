//! Server registry for multi-server architecture
//!
//! Tracks running servers in `~/.jcode/servers.json` for discovery by clients.

use anyhow::Result;
use serde::{Deserialize, Serialize};
use std::collections::HashMap;
use std::path::PathBuf;
use tokio::fs;

use crate::storage::jcode_dir;

/// Information about a running server
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct ServerInfo {
    /// Full server ID (e.g., "server_blazing_1705012345678")
    pub id: String,
    /// Short name (e.g., "blazing")
    pub name: String,
    /// Icon for display (e.g., "🔥")
    pub icon: String,
    /// Socket path
    pub socket: PathBuf,
    /// Debug socket path
    pub debug_socket: PathBuf,
    /// Git hash of the binary
    pub git_hash: String,
    /// Version string (e.g., "v0.1.123")
    pub version: String,
    /// Process ID
    pub pid: u32,
    /// When the server started (ISO 8601)
    pub started_at: String,
    /// Session names currently on this server
    #[serde(default)]
    pub sessions: Vec<String>,
}

impl ServerInfo {
    /// Display name with icon (e.g., "🔥 blazing")
    pub fn display_name(&self) -> String {
        format!("{} {}", self.icon, self.name)
    }
}

/// The server registry file
#[derive(Debug, Clone, Serialize, Deserialize, Default)]
pub struct ServerRegistry {
    /// Map from server name to server info
    #[serde(flatten)]
    pub servers: HashMap<String, ServerInfo>,
}

impl ServerRegistry {
    /// Load the registry from disk
    pub async fn load() -> Result<Self> {
        let path = registry_path()?;
        if !path.exists() {
            return Ok(Self::default());
        }

        let content = fs::read_to_string(&path).await?;
        let registry: Self = serde_json::from_str(&content)?;
        Ok(registry)
    }

    /// Save the registry to disk
    pub async fn save(&self) -> Result<()> {
        let path = registry_path()?;

        // Ensure parent directory exists
        if let Some(parent) = path.parent() {
            fs::create_dir_all(parent).await?;
            if let Err(e) = crate::platform::set_directory_permissions_owner_only(parent) {
                crate::logging::info(&format!(
                    "Registry save: failed to harden directory permissions for {}: {}",
                    parent.display(),
                    e
                ));
            }
        }

        let content = serde_json::to_string_pretty(self)?;
        fs::write(&path, content).await?;
        if let Err(e) = crate::platform::set_permissions_owner_only(&path) {
            crate::logging::info(&format!(
                "Registry save: failed to harden file permissions for {}: {}",
                path.display(),
                e
            ));
        }
        Ok(())
    }

    /// Register a server
    pub fn register(&mut self, info: ServerInfo) {
        self.servers.insert(info.name.clone(), info);
    }

    /// Unregister a server by name
    pub fn unregister(&mut self, name: &str) {
        self.servers.remove(name);
    }

    /// Find a server by name
    pub fn find_by_name(&self, name: &str) -> Option<&ServerInfo> {
        self.servers.get(name)
    }

    /// Get all servers sorted by started_at (newest first)
    pub fn servers_by_time(&self) -> Vec<&ServerInfo> {
        let mut servers: Vec<_> = self.servers.values().collect();
        servers.sort_by(|a, b| b.started_at.cmp(&a.started_at));
        servers
    }

    /// Clean up stale entries (servers that are no longer running or have been superseded).
    ///
    /// Socket path ownership is managed by the server process itself. Registry
    /// cleanup must not unlink those paths because a new live server can reuse
    /// the same published socket after a reboot or reload while an older
    /// registry entry still references it.
    pub async fn cleanup_stale(&mut self) -> Result<Vec<String>> {
        let mut removed = Vec::new();

        // First pass: remove entries whose process is dead
        let names: Vec<_> = self.servers.keys().cloned().collect();
        for name in &names {
            if let Some(info) = self.servers.get(name) {
                let pid = info.pid;
                if !is_process_running(pid) {
                    removed.push(name.clone());
                    self.servers.remove(name);
                }
            }
        }

        // Second pass: if multiple entries share the same socket path (happens
        // after server exec/reload), keep only the newest one.
        let remaining: Vec<_> = self.servers.keys().cloned().collect();
        let mut socket_to_newest: std::collections::HashMap<PathBuf, (String, String)> =
            std::collections::HashMap::new();
        for name in &remaining {
            if let Some(info) = self.servers.get(name) {
                let entry = socket_to_newest
                    .entry(info.socket.clone())
                    .or_insert_with(|| (name.clone(), info.started_at.clone()));
                if info.started_at > entry.1 {
                    *entry = (name.clone(), info.started_at.clone());
                }
            }
        }
        for name in &remaining {
            if let Some(info) = self.servers.get(name) {
                if let Some((newest_name, _)) = socket_to_newest.get(&info.socket) {
                    if newest_name != name {
                        removed.push(name.clone());
                        self.servers.remove(name);
                    }
                }
            }
        }

        if !removed.is_empty() {
            self.save().await?;
        }

        Ok(removed)
    }

    /// Add a session to a server
    pub fn add_session(&mut self, server_name: &str, session_name: &str) {
        if let Some(info) = self.servers.get_mut(server_name) {
            if !info.sessions.contains(&session_name.to_string()) {
                info.sessions.push(session_name.to_string());
            }
        }
    }

    /// Remove a session from a server
    pub fn remove_session(&mut self, server_name: &str, session_name: &str) {
        if let Some(info) = self.servers.get_mut(server_name) {
            info.sessions.retain(|s| s != session_name);
        }
    }
}

/// Get the path to the registry file
pub fn registry_path() -> Result<PathBuf> {
    Ok(jcode_dir()?.join("servers.json"))
}

/// Get the socket directory path
pub fn socket_dir() -> Result<PathBuf> {
    Ok(crate::storage::runtime_dir().join("jcode"))
}

/// Get the socket path for a named server
pub fn server_socket_path(name: &str) -> PathBuf {
    socket_dir()
        .map(|d| d.join(format!("{}.sock", name)))
        .unwrap_or_else(|_| std::env::temp_dir().join(format!("jcode-{}.sock", name)))
}

/// Get the debug socket path for a named server
pub fn server_debug_socket_path(name: &str) -> PathBuf {
    socket_dir()
        .map(|d| d.join(format!("{}-debug.sock", name)))
        .unwrap_or_else(|_| std::env::temp_dir().join(format!("jcode-{}-debug.sock", name)))
}

/// Check if a process is still running
fn is_process_running(pid: u32) -> bool {
    crate::platform::is_process_running(pid)
}

/// Unregister a server from the registry
pub async fn unregister_server(name: &str) -> Result<()> {
    let mut registry = ServerRegistry::load().await?;
    registry.unregister(name);
    registry.save().await?;
    Ok(())
}

/// List all running servers
pub async fn list_servers() -> Result<Vec<ServerInfo>> {
    let mut registry = ServerRegistry::load().await?;
    registry.cleanup_stale().await?;
    Ok(registry.servers_by_time().into_iter().cloned().collect())
}

/// Best-effort sync lookup for a server by socket path.
///
/// This is used by client-side window title code before the async runtime is fully
/// established or in synchronous spawn helpers.
pub fn find_server_by_socket_sync(socket: &std::path::Path) -> Option<ServerInfo> {
    let path = registry_path().ok()?;
    let content = std::fs::read_to_string(path).ok()?;
    let registry: ServerRegistry = serde_json::from_str(&content).ok()?;
    registry
        .servers
        .values()
        .find(|info| info.socket == socket)
        .cloned()
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::storage::lock_test_env;
    use crate::transport::Listener;
    use std::ffi::OsString;

    fn test_server_info(name: &str) -> ServerInfo {
        ServerInfo {
            id: format!("server_{}_123", name),
            name: name.to_string(),
            icon: "🔥".to_string(),
            socket: PathBuf::from(format!("/tmp/{}.sock", name)),
            debug_socket: PathBuf::from(format!("/tmp/{}-debug.sock", name)),
            git_hash: "abc1234".to_string(),
            version: "v0.1.123".to_string(),
            pid: std::process::id(),
            started_at: "2025-01-01T00:00:00Z".to_string(),
            sessions: Vec::new(),
        }
    }

    #[test]
    fn test_server_info_display_name() {
        let info = test_server_info("blazing");
        assert_eq!(info.display_name(), "🔥 blazing");
    }

    #[test]
    fn test_registry_find_by_name() {
        let mut registry = ServerRegistry::default();
        let info = test_server_info("blazing");
        registry.register(info);

        assert!(registry.find_by_name("blazing").is_some());
        assert!(registry.find_by_name("frozen").is_none());
    }

    #[test]
    fn find_server_by_socket_sync_returns_matching_server() {
        let _guard = lock_test_env();
        let temp_home = tempfile::tempdir().expect("temp home");
        let prev_home: Option<OsString> = std::env::var_os("JCODE_HOME");
        crate::env::set_var("JCODE_HOME", temp_home.path());

        let socket = PathBuf::from("/tmp/blazing.sock");
        let mut registry = ServerRegistry::default();
        let mut info = test_server_info("blazing");
        info.socket = socket.clone();
        registry.register(info.clone());
        std::fs::create_dir_all(temp_home.path()).expect("create temp home");
        std::fs::write(
            registry_path().expect("registry path"),
            serde_json::to_string(&registry).expect("serialize registry"),
        )
        .expect("write registry");

        let found = find_server_by_socket_sync(&socket).expect("find server by socket");
        assert_eq!(found.name, info.name);
        assert_eq!(found.icon, info.icon);

        if let Some(prev_home) = prev_home {
            crate::env::set_var("JCODE_HOME", prev_home);
        } else {
            crate::env::remove_var("JCODE_HOME");
        }
    }

    #[cfg(unix)]
    #[tokio::test]
    async fn cleanup_stale_preserves_live_socket_paths() {
        let _guard = lock_test_env();
        let temp_home = tempfile::tempdir().expect("temp home");
        let prev_home: Option<OsString> = std::env::var_os("JCODE_HOME");
        crate::env::set_var("JCODE_HOME", temp_home.path());

        let temp_runtime = tempfile::tempdir().expect("temp runtime");
        let socket = temp_runtime.path().join("jcode.sock");
        let debug_socket = temp_runtime.path().join("jcode-debug.sock");
        let _listener = Listener::bind(&socket).expect("bind live socket");
        let _debug_listener = Listener::bind(&debug_socket).expect("bind live debug socket");
        let dead_pid = {
            let mut child = std::process::Command::new("sh")
                .args(["-c", "exit 0"])
                .spawn()
                .expect("spawn short-lived child");
            let pid = child.id();
            let _ = child.wait().expect("wait for short-lived child");
            pid
        };

        let mut registry = ServerRegistry::default();
        registry.register(ServerInfo {
            id: "server_old_1".to_string(),
            name: "old".to_string(),
            icon: "🪦".to_string(),
            socket: socket.clone(),
            debug_socket: debug_socket.clone(),
            git_hash: "deadbeef".to_string(),
            version: "v0.0.0".to_string(),
            pid: dead_pid,
            started_at: "2026-01-01T00:00:00Z".to_string(),
            sessions: Vec::new(),
        });

        let removed = registry.cleanup_stale().await.expect("cleanup stale");
        assert_eq!(removed, vec!["old".to_string()]);
        assert!(
            socket.exists(),
            "cleanup_stale must not unlink a live server socket path"
        );
        assert!(
            debug_socket.exists(),
            "cleanup_stale must not unlink a live debug socket path"
        );

        if let Some(prev_home) = prev_home {
            crate::env::set_var("JCODE_HOME", prev_home);
        } else {
            crate::env::remove_var("JCODE_HOME");
        }
    }
}
