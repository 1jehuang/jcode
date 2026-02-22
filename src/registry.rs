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
        }

        let content = serde_json::to_string_pretty(self)?;
        fs::write(&path, content).await?;
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

    /// Clean up stale entries (servers that are no longer running)
    pub async fn cleanup_stale(&mut self) -> Result<Vec<String>> {
        let mut removed = Vec::new();

        let names: Vec<_> = self.servers.keys().cloned().collect();
        for name in names {
            if let Some(info) = self.servers.get(&name) {
                // Check if process is still running
                let pid = info.pid;
                if !is_process_running(pid) {
                    // Also clean up socket files
                    let _ = fs::remove_file(&info.socket).await;
                    let _ = fs::remove_file(&info.debug_socket).await;
                    removed.push(name.clone());
                    self.servers.remove(&name);
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
        .unwrap_or_else(|_| PathBuf::from(format!("/tmp/jcode-{}.sock", name)))
}

/// Get the debug socket path for a named server
pub fn server_debug_socket_path(name: &str) -> PathBuf {
    socket_dir()
        .map(|d| d.join(format!("{}-debug.sock", name)))
        .unwrap_or_else(|_| PathBuf::from(format!("/tmp/jcode-{}-debug.sock", name)))
}

/// Check if a process is still running
fn is_process_running(pid: u32) -> bool {
    crate::platform::is_process_running(pid)
}


/// Unregister a server from the registry
pub async fn unregister_server(name: &str) -> Result<()> {
    let mut registry = ServerRegistry::load().await?;

    // Clean up socket files
    if let Some(info) = registry.find_by_name(name) {
        let _ = fs::remove_file(&info.socket).await;
        let _ = fs::remove_file(&info.debug_socket).await;
    }

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

#[cfg(test)]
mod tests {
    use super::*;

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
}
