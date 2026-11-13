//! Shared MCP server toggle service.
//!
//! Both the `mcp` management tool (agent-facing) and the TUI `/mcp
//! enable|disable` command flip a server's enabled flag, persist the change to
//! `~/.jcode/mcp.json`, refresh the manager's in-memory config, and connect or
//! disconnect the server while (un)registering its tool proxies. That flow
//! used to live twice with divergent error handling; this module is the single
//! implementation. Call sites decide how to react to the result: both the
//! tool and the TUI command stop on a failed save and report it, so the
//! in-memory state never diverges from the config on disk.
//!
//! Registry access goes through [`ToolRegistrar`], implemented by the
//! app-core tool registry, so this module stays independent of that type.

use std::sync::Arc;

use async_trait::async_trait;
use jcode_tool_core::Tool;
use tokio::sync::RwLock;

use super::manager::McpManager;
use super::protocol::{McpConfig, McpServerConfig};

#[cfg(unix)]
use std::os::fd::AsRawFd;

/// Sink for the tool proxies a toggle adds or removes. Implemented by the
/// app-core registry; keeping the trait here lets the shared toggle flow live
/// in `jcode-base`.
#[async_trait]
pub trait ToolRegistrar: Send + Sync {
    async fn register_tool(&self, name: String, tool: Arc<dyn Tool>);
    async fn unregister_tools_for(&self, server_prefix: &str);
}

/// Result of persisting the toggled config, so callers can phrase their
/// user-facing text honestly. A `save_ok=false` result means the toggle must
/// be treated as not applied: callers check it before touching the manager
/// or registry (the tool and the TUI command both stop on save failure).
#[derive(Debug)]
pub struct TogglePersistence {
    /// Path the config was written to, when the jcode dir resolved.
    pub path: Option<std::path::PathBuf>,
    /// Whether the file write succeeded (false also when the jcode dir
    /// failed to resolve; the toggle is then not applied at all).
    pub save_ok: bool,
    /// Why the save failed, so callers that surface errors (the TUI) can show
    /// the cause instead of just "it failed".
    pub save_error: Option<String>,
}

impl TogglePersistence {
    /// Suffix describing the save outcome, for user-facing text. Only
    /// reachable with `save_ok=true` in current callers (both stop on
    /// failure); kept accurate for any future caller.
    pub fn describe(&self) -> String {
        match (&self.path, self.save_ok) {
            (Some(path), true) => format!("Config saved to {}.", path.display()),
            (Some(path), false) => format!(
                "Config NOT saved (write to {} failed); the toggle did not apply.",
                path.display()
            ),
            (None, _) => "Config NOT saved (could not resolve ~/.jcode).".to_string(),
        }
    }
}

/// Cross-process exclusive lock for the `mcp.json` read-modify-write in
/// [`persist_toggle`]: the agent tool and a TUI window can be two separate
/// processes, so an in-process mutex cannot serialize them.
///
/// `flock` on a sidecar lock file matches how the repo locks elsewhere
/// (selfdev build lock, harness API bridge). The lock must not sit on
/// `mcp.json` itself: `save_to_file` reopens the json path, so an fd locked
/// on the old inode would stop guarding the live file. `LOCK_EX` blocks
/// rather than skips because the write is fast and rare, and a dropped
/// toggle would be worse than a millisecond wait.
#[derive(Debug)]
pub(crate) struct McpConfigLock {
    #[cfg(unix)]
    /// Held for its flock; dropping it closes the fd and releases the lock.
    #[allow(dead_code)]
    file: std::fs::File,
}

impl McpConfigLock {
    #[cfg(unix)]
    fn acquire(jcode_dir: &std::path::Path) -> anyhow::Result<Self> {
        let lock_path = jcode_dir.join("mcp.json.lock");
        std::fs::create_dir_all(jcode_dir)?;
        let file = std::fs::OpenOptions::new()
            .create(true)
            .write(true)
            .truncate(false)
            .open(&lock_path)?;
        // SAFETY: flock on a valid fd; the kernel releases it on close, so a
        // crashed holder never wedges the config.
        let ret = unsafe { libc::flock(file.as_raw_fd(), libc::LOCK_EX) };
        if ret != 0 {
            return Err(std::io::Error::last_os_error().into());
        }
        Ok(McpConfigLock { file })
    }

    #[cfg(not(unix))]
    fn acquire(_jcode_dir: &std::path::Path) -> anyhow::Result<Self> {
        // No advisory file locking here; the write stays unsynchronized, as
        // it was before this lock existed.
        Ok(McpConfigLock {})
    }
}

/// Flip a server's enabled flag and persist the new config to
/// `~/.jcode/mcp.json`. Sync and lock-free so both the async tool path and the
/// sync TUI command can call it before doing their async work. The caller
/// decides whether a save failure is fatal; the event log gets a
/// `toggle_save_failed` warn entry either way.
///
/// The write is serialized across processes with [`McpConfigLock`], and the
/// file is re-read *inside* the lock: a snapshot taken before acquiring could
/// clobber a toggle another process persisted in between. The caller's
/// in-memory config is untouched, matching its pre-lock behavior.
pub fn persist_toggle(
    config: &McpConfig,
    server_name: &str,
    enable: bool,
) -> anyhow::Result<(McpServerConfig, TogglePersistence)> {
    let mut server_config = config.servers.get(server_name).cloned().ok_or_else(|| {
        anyhow::anyhow!(
            "Server '{}' not found in MCP config. Add it to ~/.jcode/mcp.json first.",
            server_name
        )
    })?;

    server_config.disabled = Some(!enable);
    server_config.enabled = Some(enable);

    let persistence = match crate::storage::jcode_dir() {
        Ok(jcode_dir) => {
            let mcp_path = jcode_dir.join("mcp.json");
            let mut saved_config = config.clone();
            let lock_result = McpConfigLock::acquire(&jcode_dir)
                .and_then(|_lock| {
                    // Merge on top of the freshest on-disk state: `config`
                    // may be older than what a concurrent process just wrote.
                    if let Ok(disk) = McpConfig::load_from_file(&mcp_path) {
                        saved_config = disk;
                    }
                    saved_config
                        .servers
                        .insert(server_name.to_string(), server_config.clone());
                    saved_config.save_to_file(&mcp_path)
                })
                .map_err(|e: anyhow::Error| e.to_string());
            match lock_result {
                Ok(()) => TogglePersistence {
                    path: Some(mcp_path),
                    save_ok: true,
                    save_error: None,
                },
                Err(e) => {
                    crate::logging::event_warn(
                        "MCP_LIFECYCLE",
                        vec![
                            ("phase", "toggle_save_failed".to_string()),
                            ("server", server_name.to_string()),
                            ("error", e.clone()),
                        ],
                    );
                    TogglePersistence {
                        path: Some(mcp_path),
                        save_ok: false,
                        save_error: Some(e),
                    }
                }
            }
        }
        Err(e) => TogglePersistence {
            path: None,
            save_ok: false,
            save_error: Some(e.to_string()),
        },
    };

    Ok((server_config, persistence))
}

/// Connect the (just enabled) server and register its tool proxies in the
/// optional registrar. Returns the number of tools the server exposes and any
/// connection error; on error nothing is registered.
pub async fn connect_and_register(
    manager: &Arc<RwLock<McpManager>>,
    server_name: &str,
    server_config: &McpServerConfig,
    registrar: Option<&dyn ToolRegistrar>,
) -> (usize, Option<String>) {
    let mgr = manager.read().await;
    if let Err(e) = mgr.connect(server_name, server_config).await {
        return (0, Some(format!("{:#}", e)));
    }
    let tools = mgr.all_tools().await;
    let tool_count = tools.iter().filter(|(s, _)| s == server_name).count();
    drop(mgr);

    if let Some(registrar) = registrar {
        let mcp_tools = super::create_mcp_tools_for_server(Arc::clone(manager), server_name).await;
        for (name, tool) in mcp_tools {
            registrar.register_tool(name, tool).await;
        }
    }

    (tool_count, None)
}

#[cfg(test)]
#[path = "mcp_toggle_tests.rs"]
mod mcp_toggle_tests;

/// Disconnect the (just disabled) server and unregister its tool proxies from
/// the optional registrar. Connection errors are logged, not returned: the
/// flag flip already succeeded and the config no longer spawns the server.
pub async fn disconnect_and_unregister(
    manager: &Arc<RwLock<McpManager>>,
    server_name: &str,
    registrar: Option<&dyn ToolRegistrar>,
) {
    let mgr = manager.read().await;
    let connected = mgr.connected_servers().await;
    if connected.iter().any(|name| name == server_name)
        && let Err(error) = mgr.disconnect(server_name).await
    {
        crate::logging::event_error(
            "mcp_toggle_disconnect_failed",
            vec![
                ("server", server_name.to_string()),
                ("error", error.to_string()),
            ],
        );
    }
    drop(mgr);

    if let Some(registrar) = registrar {
        registrar
            .unregister_tools_for(&super::dispatch_name(server_name, ""))
            .await;
    }
}
