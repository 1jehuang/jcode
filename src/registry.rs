//! Server registry for multi-server architecture
//!
//! Tracks running servers in `~/.jcode/servers.json` for discovery by clients.

use anyhow::Result;
use serde::{Deserialize, Serialize};
use std::collections::HashMap;
use std::path::PathBuf;
use tokio::fs;

use crate::id::{new_memorable_server_id, server_icon};
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
    /// Create a new ServerInfo with generated ID and name
    pub fn new(git_hash: &str, version: &str) -> Result<Self> {
        let (id, name) = new_memorable_server_id();
        let icon = server_icon(&name).to_string();
        let socket = server_socket_path(&name);
        let debug_socket = server_debug_socket_path(&name);
        let pid = std::process::id();
        let started_at = chrono::Utc::now().to_rfc3339();

        Ok(Self {
            id,
            name,
            icon,
            socket,
            debug_socket,
            git_hash: git_hash.to_string(),
            version: version.to_string(),
            pid,
            started_at,
            sessions: Vec::new(),
        })
    }

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

    /// Find a server by git hash
    pub fn find_by_hash(&self, git_hash: &str) -> Option<&ServerInfo> {
        self.servers.values().find(|s| s.git_hash == git_hash)
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
    // On Unix, we can send signal 0 to check if process exists
    #[cfg(unix)]
    {
        // Signal 0 doesn't actually send a signal, just checks if process exists
        unsafe { libc::kill(pid as i32, 0) == 0 }
    }

    #[cfg(not(unix))]
    {
        // On other platforms, just assume it's running
        let _ = pid;
        true
    }
}

/// Register the current server in the registry
pub async fn register_server(git_hash: &str, version: &str) -> Result<ServerInfo> {
    let mut registry = ServerRegistry::load().await?;

    // Clean up stale entries first
    registry.cleanup_stale().await?;

    // Create new server info
    let info = ServerInfo::new(git_hash, version)?;

    // Ensure socket directory exists
    if let Ok(dir) = socket_dir() {
        fs::create_dir_all(&dir).await?;
    }

    // Register and save
    registry.register(info.clone());
    registry.save().await?;

    Ok(info)
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

/// Find a server for the given git hash, or return None if a new one should be spawned
pub async fn find_server_for_hash(git_hash: &str) -> Result<Option<ServerInfo>> {
    let mut registry = ServerRegistry::load().await?;

    // Clean up stale entries first
    registry.cleanup_stale().await?;

    Ok(registry.find_by_hash(git_hash).cloned())
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

    #[test]
    fn test_server_info_new() {
        let info = ServerInfo::new("abc1234", "v0.1.123").unwrap();
        assert!(info.id.starts_with("server_"));
        assert!(!info.name.is_empty());
        assert!(!info.icon.is_empty());
        assert!(info.socket.to_string_lossy().contains(&info.name));
    }

    #[test]
    fn test_server_info_display_name() {
        let mut info = ServerInfo::new("abc1234", "v0.1.123").unwrap();
        info.name = "blazing".to_string();
        info.icon = "🔥".to_string();
        assert_eq!(info.display_name(), "🔥 blazing");
    }

    #[test]
    fn test_registry_find_by_hash() {
        let mut registry = ServerRegistry::default();

        let mut info = ServerInfo::new("abc1234", "v0.1.123").unwrap();
        info.name = "blazing".to_string();
        registry.register(info);

        assert!(registry.find_by_hash("abc1234").is_some());
        assert!(registry.find_by_hash("xyz9999").is_none());
    }
}
