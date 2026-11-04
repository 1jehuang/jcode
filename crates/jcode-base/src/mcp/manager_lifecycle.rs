impl McpManager {
    pub fn cleanup_pending(&self) -> bool {
        self.cleanup_pending
            .load(std::sync::atomic::Ordering::SeqCst)
    }

    async fn admit(&self) -> Result<tokio::sync::RwLockReadGuard<'_, ()>> {
        let admission = self.admission.read().await;
        if self.cleanup_pending() {
            anyhow::bail!("MCP manager cleanup is unconfirmed; retry disconnect_all or reload");
        }
        Ok(admission)
    }

    /// Disconnect from a server
    pub async fn disconnect(&self, name: &str) -> Result<()> {
        let _admission = self.admission.write().await;
        self.disconnect_inner(name).await
    }

    async fn disconnect_inner(&self, name: &str) -> Result<()> {
        if self
            .connecting
            .lock()
            .unwrap_or_else(|e| e.into_inner())
            .contains_key(name)
        {
            anyhow::bail!(
                "MCP server '{name}' connection is in progress; disconnect not confirmed"
            );
        }
        // Check if it's a pool handle
        {
            let mut handles = self.pool_handles.write().await;
            if handles.contains_key(name) {
                if let Some(pool) = &self.pool {
                    pool.release_manager_handles(self.reference_id, &[name.to_string()])
                        .await;
                }
                handles.remove(name);
                return Ok(());
            }
        }

        // Otherwise it's owned
        let mut clients = self.owned_clients.write().await;
        if let Some(client) = clients.get_mut(name) {
            client
                .shutdown()
                .await
                .with_context(|| format!("MCP server '{name}' disconnect failed"))?;
            clients.remove(name);
        }
        Ok(())
    }

    /// Disconnect from all servers
    pub async fn disconnect_all(&self) -> Result<()> {
        self.disconnect_all_with(std::future::ready(())).await
    }

    async fn disconnect_all_with<F: std::future::Future<Output = ()>>(
        &self,
        after_snapshot: F,
    ) -> Result<()> {
        let _admission = self.admission.write().await;
        self.cleanup_pending
            .store(true, std::sync::atomic::Ordering::SeqCst);
        self.disconnect_all_inner(after_snapshot).await?;
        self.cleanup_pending
            .store(false, std::sync::atomic::Ordering::SeqCst);
        Ok(())
    }

    async fn disconnect_all_inner<F: std::future::Future<Output = ()>>(
        &self,
        after_snapshot: F,
    ) -> Result<()> {
        // Session is ending: flush any pending partner-discovery usage
        // aggregates (best effort) so short sessions still report.
        crate::sponsors::provenance::flush_now();
        // Release pool handles
        {
            let mut handles = self.pool_handles.write().await;
            let names: Vec<String> = handles.keys().cloned().collect();
            if let Some(pool) = &self.pool {
                pool.release_manager_handles(self.reference_id, &names)
                    .await;
            }
            handles.clear();
        }

        // Shutdown owned clients
        let names: Vec<String> = self.owned_clients.read().await.keys().cloned().collect();
        after_snapshot.await;
        let mut failures = Vec::new();
        for name in names {
            if let Err(error) = self.disconnect_inner(&name).await {
                failures.push(format!("{name}: {error:#}"));
            }
        }
        for name in self.owned_clients.read().await.keys() {
            failures.push(format!("{name}: owned client remains"));
        }
        for name in self
            .connecting
            .lock()
            .unwrap_or_else(|e| e.into_inner())
            .keys()
        {
            failures.push(format!("{name}: connection still in progress"));
        }
        if !failures.is_empty() {
            anyhow::bail!("MCP shutdown incomplete: {}", failures.join("; "));
        }
        Ok(())
    }

    /// Get list of connected server names
    pub async fn connected_servers(&self) -> Vec<String> {
        let Ok(_admission) = self.admit().await else {
            return Vec::new();
        };
        let mut names: Vec<String> = self
            .pool_handles
            .read()
            .await
            .iter()
            .filter(|(_, handle)| !handle.is_closed())
            .map(|(name, _)| name.clone())
            .collect();
        names.extend(
            self.owned_clients
                .read()
                .await
                .iter()
                .filter(|(_, client)| !client.handle().is_closed())
                .map(|(name, _)| name.clone()),
        );
        names
    }

    /// Tool definitions for one connected server, or `None` when it is not
    /// connected. Cheaper than [`Self::all_tools`] when reacting to a single
    /// server's connection.
    pub async fn server_tools(&self, server_name: &str) -> Option<Vec<McpToolDef>> {
        let Ok(_admission) = self.admit().await else {
            return None;
        };
        if let Some(handle) = self.pool_handles.read().await.get(server_name)
            && !handle.is_closed()
        {
            return Some(handle.tools());
        }
        if let Some(client) = self.owned_clients.read().await.get(server_name)
            && !client.handle().is_closed()
        {
            return Some(client.tools());
        }
        None
    }
}
