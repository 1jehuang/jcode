//! Plugin registry changes are observed only at a new prompt boundary.
use super::*;
use std::path::PathBuf;

pub(super) struct PluginMcpRefresh {
    manager: Arc<RwLock<crate::mcp::McpManager>>,
    registry_path: Option<PathBuf>,
    // Exact bytes avoid mtime granularity races and work across CLI/TUI processes.
    // None also forces reconciliation on the first prompt after async startup.
    observed: Option<Vec<u8>>,
}

#[cfg(test)]
mod tests {
    use super::*;
    use serde_json::json;

    struct HomeGuard(Option<std::ffi::OsString>);
    impl Drop for HomeGuard {
        fn drop(&mut self) {
            if let Some(previous) = self.0.take() {
                crate::env::set_var("JCODE_HOME", previous);
            } else {
                crate::env::remove_var("JCODE_HOME");
            }
        }
    }

    fn write_json(path: &std::path::Path, value: serde_json::Value) {
        std::fs::create_dir_all(path.parent().unwrap()).unwrap();
        std::fs::write(path, serde_json::to_vec(&value).unwrap()).unwrap();
    }

    async fn cache_proxy(
        registry: &Registry,
        manager: &Arc<RwLock<crate::mcp::McpManager>>,
        server: &str,
    ) {
        let definition =
            serde_json::from_value(json!({"name":"ping", "inputSchema":{"type":"object"}}))
                .unwrap();
        for (name, tool) in
            crate::mcp::create_mcp_tools_from_cached(server, &[definition], Arc::clone(manager))
        {
            registry.register(name, tool).await;
        }
    }

    #[tokio::test]
    async fn external_install_disable_refreshes_only_plugin_proxies_at_prompt_boundary() {
        let _lock = crate::storage::lock_test_env();
        let home = tempfile::tempdir().unwrap();
        let project = tempfile::tempdir().unwrap();
        let market = tempfile::tempdir().unwrap();
        let _guard = HomeGuard(std::env::var_os("JCODE_HOME"));
        crate::env::set_var("JCODE_HOME", home.path());
        write_json(
            &market.path().join(".claude-plugin/marketplace.json"),
            json!({
                "name":"boundary-market", "plugins":[{"name":"boundary-plugin", "source":"./demo"}]
            }),
        );
        write_json(
            &market.path().join("demo/.claude-plugin/plugin.json"),
            json!({"name":"boundary-plugin"}),
        );
        // Disabled server fixtures exercise discovery without starting subprocesses.
        write_json(
            &market.path().join("demo/.mcp.json"),
            json!({"mcpServers":{
                "echo":{"command":"never-start-this-server", "enabled":false}
            }}),
        );
        let registry = Registry::empty();
        let manager = Arc::new(RwLock::new(crate::mcp::McpManager::for_dir(Some(
            project.path().into(),
        ))));
        *registry.plugin_mcp_refresh.write().await =
            Some(PluginMcpRefresh::new(Arc::clone(&manager)));
        assert!(registry.refresh_plugin_mcp_before_prompt().await.unwrap());

        let external =
            jcode_base::plugin::PluginManager::new(home.path().into(), project.path().into());
        external
            .add_marketplace(market.path().to_str().unwrap())
            .unwrap();
        external
            .install(
                "boundary-plugin@boundary-market",
                jcode_base::plugin::PluginScope::User,
            )
            .unwrap();
        assert!(
            !manager
                .read()
                .await
                .config()
                .servers
                .keys()
                .any(|name| name.starts_with("plugin:")),
            "install must not mutate a live session mid-turn"
        );
        assert!(registry.refresh_plugin_mcp_before_prompt().await.unwrap());
        let plugin = "plugin:boundary-plugin@boundary-market:echo";
        assert!(manager.read().await.config().servers.contains_key(plugin));
        assert!(
            !registry.refresh_plugin_mcp_before_prompt().await.unwrap(),
            "unchanged disk snapshot must be a no-op"
        );

        cache_proxy(&registry, &manager, plugin).await;
        cache_proxy(&registry, &manager, "unrelated-offline").await;
        external
            .disable(
                "boundary-plugin@boundary-market",
                jcode_base::plugin::PluginScope::User,
            )
            .unwrap();
        assert!(registry.tools.read().await.values().any(|tool| {
            tool.mcp_identity()
                .is_some_and(|(server, _)| server == plugin)
        }));
        assert!(registry.refresh_plugin_mcp_before_prompt().await.unwrap());
        let tools = registry.tools.read().await;
        assert!(!tools.values().any(|tool| {
            tool.mcp_identity()
                .is_some_and(|(server, _)| server == plugin)
        }));
        assert!(
            tools.values().any(|tool| tool
                .mcp_identity()
                .is_some_and(|(server, _)| server == "unrelated-offline")),
            "unrelated cached MCP tools survive plugin disable"
        );
    }

    #[tokio::test]
    async fn in_flight_mcp_work_never_blocks_the_prompt_boundary() {
        let _lock = crate::storage::lock_test_env();
        let home = tempfile::tempdir().unwrap();
        let _guard = HomeGuard(std::env::var_os("JCODE_HOME"));
        crate::env::set_var("JCODE_HOME", home.path());
        let registry = Registry::empty();
        let manager = Arc::new(RwLock::new(crate::mcp::McpManager::for_dir(None)));
        *registry.plugin_mcp_refresh.write().await =
            Some(PluginMcpRefresh::new(Arc::clone(&manager)));

        // Startup connects and MCP tool calls hold this across their await.
        let in_flight_connect = manager.read().await;
        let refreshed = tokio::time::timeout(
            std::time::Duration::from_secs(1),
            registry.refresh_plugin_mcp_before_prompt(),
        )
        .await
        .expect("a pending MCP connection must not hold the turn");
        assert!(!refreshed.unwrap(), "busy manager defers the refresh");
        // A writer must not have been queued ahead of later readers.
        assert!(manager.try_read().is_ok());

        drop(in_flight_connect);
        assert!(
            registry.refresh_plugin_mcp_before_prompt().await.unwrap(),
            "deferred refresh runs at the next boundary"
        );
    }

    #[cfg(unix)]
    #[test]
    fn disk_snapshot_observes_separate_process_same_length_replacement() {
        let _lock = crate::storage::lock_test_env();
        let temp = tempfile::tempdir().unwrap();
        let registry_path = temp.path().join("registry.json");
        std::fs::write(&registry_path, b"old").unwrap();
        let state = PluginMcpRefresh {
            manager: Arc::new(RwLock::new(crate::mcp::McpManager::with_config(
                Default::default(),
            ))),
            registry_path: Some(registry_path.clone()),
            observed: Some(b"old".to_vec()),
        };
        let generation = jcode_base::plugin::generation();
        let replacement = temp.path().join("replacement.json");
        std::fs::write(&replacement, b"new").unwrap();
        assert!(
            std::process::Command::new("/bin/cp")
                .arg(&replacement)
                .arg(&registry_path)
                .status()
                .unwrap()
                .success()
        );
        assert_eq!(
            jcode_base::plugin::generation(),
            generation,
            "a separate process cannot update this process's generation"
        );
        assert_ne!(
            state.observed.as_ref().unwrap(),
            &state.read_snapshot().unwrap()
        );
    }
}

impl PluginMcpRefresh {
    pub(super) fn new(manager: Arc<RwLock<crate::mcp::McpManager>>) -> Self {
        Self {
            manager,
            registry_path: crate::storage::jcode_dir()
                .ok()
                .map(|home| home.join("plugins/registry.json")),
            observed: None,
        }
    }

    fn read_snapshot(&self) -> std::io::Result<Vec<u8>> {
        let Some(path) = &self.registry_path else {
            return Ok(Vec::new());
        };
        match std::fs::read(path) {
            Ok(bytes) => Ok(bytes),
            Err(error) if error.kind() == std::io::ErrorKind::NotFound => Ok(Vec::new()),
            Err(error) => Err(error),
        }
    }
}

impl Registry {
    /// Call before a new user prompt, never from the model/tool execution loop.
    /// Reconcile only plugin-owned connections; ad-hoc and configured MCPs survive.
    pub(crate) async fn refresh_plugin_mcp_before_prompt(&self) -> anyhow::Result<bool> {
        // Never wait here: MCP connects and tool calls hold the manager read
        // lock for their whole duration, and a queued writer would also stall
        // new readers. On contention `observed` stays stale, so the next
        // prompt boundary retries.
        let Ok(mut state) = self.plugin_mcp_refresh.try_write() else {
            return Ok(false);
        };
        let Some(state) = state.as_mut() else {
            return Ok(false);
        };
        let snapshot = state.read_snapshot()?;
        if state.observed.as_ref() == Some(&snapshot) {
            return Ok(false);
        }
        let Ok(mut manager) = state.manager.try_write() else {
            return Ok(false);
        };
        let old_config = manager.config().clone();
        let mut plugin_servers: std::collections::BTreeSet<String> = manager
            .config()
            .sources
            .iter()
            .filter(|(_, source)| crate::mcp::is_plugin_mcp_source(source))
            .map(|(name, _)| name.clone())
            .collect();
        let (_, failures) = manager.reload_plugin_sources().await?;
        plugin_servers.extend(
            manager
                .config()
                .sources
                .iter()
                .filter(|(_, source)| crate::mcp::is_plugin_mcp_source(source))
                .map(|(name, _)| name.clone()),
        );
        plugin_servers.retain(|name| {
            serde_json::to_value(old_config.servers.get(name)).ok()
                != serde_json::to_value(manager.config().servers.get(name)).ok()
                || old_config.sources.get(name) != manager.config().sources.get(name)
                || old_config.pending_approval.contains(name)
                    != manager.config().pending_approval.contains(name)
        });
        drop(manager);
        for (server, error) in failures {
            crate::logging::warn(&format!("Plugin MCP refresh: {server}: {error}"));
        }
        // Replace only plugin proxies. In particular, unrelated offline servers
        // may have cached proxies which create_mcp_tools cannot recreate.
        if !plugin_servers.is_empty() {
            let fresh = crate::mcp::create_mcp_tools(Arc::clone(&state.manager))
                .await
                .into_iter()
                .filter(|(_, tool)| {
                    tool.mcp_identity()
                        .is_some_and(|(server, _)| plugin_servers.contains(server))
                })
                .collect();
            self.refresh_mcp_tools(fresh, &plugin_servers.iter().cloned().collect::<Vec<_>>())
                .await;
        }
        state.observed = Some(snapshot);
        Ok(true)
    }
}
