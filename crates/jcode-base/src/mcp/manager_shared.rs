impl McpManager {
    async fn connect_shared_using<H: std::future::Future<Output = ()>>(
        &self,
        name: &str,
        config: &McpServerConfig,
        after_connect: H,
    ) -> Result<()> {
        let operation = {
            let mut operations = self
                .shared_operations
                .lock()
                .unwrap_or_else(|e| e.into_inner());
            Arc::clone(
                operations
                    .entry(name.to_string())
                    .or_insert_with(|| Arc::new(Mutex::new(()))),
            )
        };
        let _operation = operation.lock().await;
        let pool = self
            .pool
            .as_ref()
            .context("MCP shared pool is unavailable")?;
        let lease = pool
            .connect_server_handle(self.reference_id, name, config, after_connect)
            .await?;
        let mut handles = self.pool_handles.write().await;
        let handle = lease.handle();
        if handle.is_closed() {
            anyhow::bail!("MCP server '{name}' closed before handle publication");
        }
        handles.insert(name.to_string(), handle);
        lease.commit();
        crate::logging::info(&format!(
            "MCP manager '{}' acquired shared server '{name}'",
            self.session_id
        ));
        Ok(())
    }
}
