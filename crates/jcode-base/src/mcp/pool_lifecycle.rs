impl SharedMcpPool {
    /// Disconnect a specific server
    pub async fn disconnect_server(&self, name: &str) -> Result<()> {
        let _admission = self.admission.read().await;
        self.disconnect_server_inner(name).await
    }
    async fn disconnect_server_inner(&self, name: &str) -> Result<()> {
        let attempt = {
            let mut registry = self.lock_connecting();
            if registry.contains_key(name) {
                anyhow::bail!(
                    "MCP server '{name}' lifecycle operation is in progress; disconnect not confirmed"
                );
            }
            let attempt = Arc::new(SharedConnectionAttempt::new(SharedOperation::Disconnect));
            registry.insert(name.to_string(), Arc::clone(&attempt));
            attempt
        };
        let mut guard = SharedConnectionAttemptGuard::new(
            Arc::clone(&self.connecting),
            name,
            Arc::clone(&attempt),
        );
        let owner = self.clients.lock().await.get(name).cloned();
        if let Some(owner) = owner.as_ref() {
            let mut client = owner.lock().await;
            if let Err(error) = client
                .shutdown()
                .await
                .with_context(|| format!("Shared MCP server '{name}' disconnect failed"))
            {
                self.complete_attempt(name, &attempt, Err(format!("{error:#}")));
                guard.disarm();
                return Err(error);
            }
            self.commit_disconnect(name, &attempt, Some(owner)).await?;
        } else {
            self.commit_disconnect(name, &attempt, None).await?;
        }
        guard.disarm();
        Ok(())
    }

    /// Disconnect all servers
    pub fn cleanup_pending(&self) -> bool {
        self.cleanup_pending
            .load(std::sync::atomic::Ordering::SeqCst)
    }
    pub async fn disconnect_all(&self) -> Result<()> {
        let _admission = self.admission.write().await;
        self.cleanup_pending
            .store(true, std::sync::atomic::Ordering::SeqCst);
        self.disconnect_all_inner(std::future::ready(())).await?;
        self.cleanup_pending
            .store(false, std::sync::atomic::Ordering::SeqCst);
        Ok(())
    }
    async fn disconnect_all_inner<F: std::future::Future<Output = ()>>(
        &self,
        after_snapshot: F,
    ) -> Result<()> {
        let names: Vec<String> = self.clients.lock().await.keys().cloned().collect();
        after_snapshot.await;
        let mut failures = Vec::new();
        for name in names {
            if let Err(error) = self.disconnect_server_inner(&name).await {
                failures.push(format!("{name}: {error:#}"));
            }
        }
        for name in self.clients.lock().await.keys() {
            failures.push(format!("{name}: owned client remains"));
        }
        for name in self.lock_connecting().keys() {
            failures.push(format!("{name}: lifecycle operation remains"));
        }
        {
            let mut refs = self.ref_counts.lock().await;
            self.flush_released_references(&mut refs);
        }
        if !failures.is_empty() {
            anyhow::bail!("Shared MCP shutdown incomplete: {}", failures.join("; "));
        }
        Ok(())
    }
}
