impl McpManager {
    /// Get all available tools from all connected servers
    pub async fn all_tools(&self) -> Vec<(String, McpToolDef)> {
        let Ok(_admission) = self.admit().await else {
            return Vec::new();
        };
        self.all_tools_inner().await
    }

    async fn all_tools_inner(&self) -> Vec<(String, McpToolDef)> {
        let mut tools = Vec::new();

        // Pool handles
        for (server_name, handle) in self.pool_handles.read().await.iter() {
            if handle.is_closed() {
                continue;
            }
            for tool in handle.tools() {
                tools.push((server_name.clone(), tool));
            }
        }

        // Owned clients
        for (server_name, client) in self.owned_clients.read().await.iter() {
            if client.handle().is_closed() {
                continue;
            }
            for tool in client.tools() {
                tools.push((server_name.clone(), tool));
            }
        }

        tools
    }

    /// Get the best available MCP tool catalog for deferred discovery.
    ///
    /// Live definitions win, while the schema cache fills in enabled servers
    /// that are still connecting. This preserves advertise-early behavior for
    /// the fixed `mcp_search` surface without changing `all_tools()`, whose
    /// callers intentionally operate on connected servers only.
    pub async fn searchable_tools(&self) -> Vec<(String, McpToolDef)> {
        let Ok(_admission) = self.admit().await else {
            return Vec::new();
        };
        let mut tools: BTreeMap<(String, String), McpToolDef> = BTreeMap::new();
        let schema_cache = super::McpSchemaCache::load();

        for (server, config) in &self.config.servers {
            if !config.is_enabled() {
                continue;
            }
            if let Some(cached) = schema_cache.tools_for(server, config) {
                for tool in cached {
                    tools.insert((server.clone(), tool.name.clone()), tool.clone());
                }
            }
        }

        // Insert live definitions last so schema changes discovered during this
        // session replace stale cache entries immediately.
        for (server, tool) in self.all_tools_inner().await {
            tools.insert((server.clone(), tool.name.clone()), tool);
        }

        tools
            .into_iter()
            .map(|((server, _), tool)| (server, tool))
            .collect()
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
        if let Some(handle) = self.live_handle(server).await? {
            let result = handle.call_tool(tool, arguments).await;
            meter_provenance_call(server, &result);
            return result;
        }
        let Some(config) = self.config.servers.get(server).cloned() else {
            anyhow::bail!("MCP server '{server}' not connected");
        };
        match tokio::time::timeout(CONNECT_ON_CALL_TIMEOUT, self.connect(server, &config)).await {
            Ok(result) => {
                result.with_context(|| format!("MCP server '{server}' failed to connect"))?
            }
            Err(_) => anyhow::bail!(
                "MCP server '{server}' did not connect within {}s; tool '{tool}' is unavailable right now",
                CONNECT_ON_CALL_TIMEOUT.as_secs()
            ),
        }
        let handle = self.live_handle(server).await?.with_context(|| {
            format!("MCP server '{server}' connected but exposed no live handle for tool '{tool}'")
        })?;
        let result = handle.call_tool(tool, arguments).await;
        meter_provenance_call(server, &result);
        result
    }

    async fn live_handle(&self, server: &str) -> Result<Option<McpHandle>> {
        let _admission = self.admit().await?;
        if let Some(handle) = self
            .pool_handles
            .read()
            .await
            .get(server)
            .filter(|handle| !handle.is_closed())
        {
            return Ok(Some(handle.clone()));
        }
        Ok(self
            .owned_clients
            .read()
            .await
            .get(server)
            .filter(|client| !client.cleanup_pending() && !client.handle().is_closed())
            .map(McpClient::handle))
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
        let pool = self.pool.clone();
        let project_dir = self.project_dir.clone();
        self.reload_using(
            move || McpConfig::load_for_dir(project_dir.as_deref()),
            async move {
                if let Some(pool) = pool {
                    pool.reload().await
                } else {
                    Ok((0, Vec::new()))
                }
            },
        )
        .await
    }

    async fn reload_using<L, P>(
        &mut self,
        load_config: L,
        pool_reload: P,
    ) -> Result<(usize, Vec<(String, String)>)>
    where
        L: FnOnce() -> McpConfig,
        P: std::future::Future<Output = Result<(usize, Vec<(String, String)>)>>,
    {
        let _admission = self.admission.write().await;
        self.cleanup_pending
            .store(true, std::sync::atomic::Ordering::SeqCst);
        self.disconnect_all_inner(std::future::ready(())).await?;
        let new_config = load_config();
        let pool_failures = pool_reload.await?.1;
        self.config = new_config;
        let (successes, mut failures) = self.connect_all_inner().await?;
        for failure in pool_failures {
            if !failures.iter().any(|current| current.0 == failure.0) {
                failures.push(failure);
            }
        }
        self.cleanup_pending
            .store(false, std::sync::atomic::Ordering::SeqCst);
        Ok((successes, failures))
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
        let mut connected_servers = 0usize;
        let admission = self.admission.try_read();
        let live = admission.is_ok() && !self.cleanup_pending();

        if let Ok(handles) = self.pool_handles.try_read() {
            for handle in handles.values() {
                if !live || handle.is_closed() {
                    continue;
                }
                connected_servers += 1;
                for tool in handle.tools() {
                    available_tools += 1;
                    tool_schema_estimate_bytes += estimate_tool_bytes(&tool);
                }
            }
        }

        if let Ok(clients) = self.owned_clients.try_read() {
            for client in clients.values() {
                if !live || client.cleanup_pending() || client.handle().is_closed() {
                    continue;
                }
                connected_servers += 1;
                for tool in client.tools() {
                    available_tools += 1;
                    tool_schema_estimate_bytes += estimate_tool_bytes(&tool);
                }
            }
        }

        McpManagerMemoryProfile {
            shared_pool_enabled: self.pool.is_some(),
            configured_servers: self.config.servers.len(),
            connected_servers,
            pooled_handles,
            owned_clients,
            available_tools,
            configured_json_bytes: crate::process_memory::estimate_json_bytes(&self.config),
            tool_schema_estimate_bytes,
        }
    }

    /// Check if any servers are connected
    pub async fn has_connections(&self) -> bool {
        !self.connected_servers().await.is_empty()
    }
}
