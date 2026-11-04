impl SharedMcpPool {
    /// Connect to a specific server by name and config
    pub(super) async fn connect_server_handle<H: std::future::Future<Output = ()>>(
        &self,
        reference_id: u64,
        name: &str,
        config: &McpServerConfig,
        after_connect: H,
    ) -> Result<SharedMcpHandleLease<'_>> {
        let admission = self.admission.read().await;
        if self.cleanup_pending() {
            anyhow::bail!("MCP pool cleanup is unconfirmed; retry disconnect_all or reload");
        }
        self.ensure_connected(name.to_string(), config.clone())
            .await
            .map_err(anyhow::Error::msg)?;
        after_connect.await;
        let handles = self.handles.read().await;
        let handle = handles
            .get(name)
            .filter(|handle| !handle.is_closed())
            .cloned()
            .with_context(|| format!("MCP server '{name}' connected without a live handle"))?;
        let mut refs = self.ref_counts.lock().await;
        let new_reference = self.record_references(
            PoolReferenceOwner::Manager(reference_id),
            std::iter::once(name),
            &mut refs,
        ) != 0;
        Ok(SharedMcpHandleLease {
            pool: self,
            reference_id,
            name: name.to_string(),
            handle,
            new_reference,
            _admission: admission,
        })
    }

    pub async fn connect_server(&self, name: &str, config: &McpServerConfig) -> Result<()> {
        let _admission = self.admission.read().await;
        if self.cleanup_pending() {
            anyhow::bail!("MCP pool cleanup is unconfirmed; retry disconnect_all or reload");
        }
        self.ensure_connected(name.to_string(), config.clone())
            .await
            .map(|_| ())
            .map_err(|error_msg| anyhow::anyhow!(error_msg))
            .with_context(|| format!("Failed to connect to MCP server '{}'", name))
    }

    /// Get handles for all connected servers (for a new session).
    /// Increments reference counts.
    pub async fn acquire_handles(&self, session_id: &str) -> HashMap<String, McpHandle> {
        let _admission = self.admission.read().await;
        if self.cleanup_pending() {
            return HashMap::new();
        }
        let handles = self.handles.read().await;
        let result: HashMap<String, McpHandle> = handles
            .iter()
            .filter(|(_, handle)| !handle.is_closed())
            .map(|(name, handle)| (name.clone(), handle.clone()))
            .collect();

        let mut refs = self.ref_counts.lock().await;
        self.record_references(
            PoolReferenceOwner::Session(session_id.to_string()),
            result.keys().map(String::as_str),
            &mut refs,
        );

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
        self.release_reference_handles(
            PoolReferenceOwner::Session(session_id.to_string()),
            server_names,
        )
        .await;
    }

    pub(super) async fn release_manager_handles(&self, reference_id: u64, server_names: &[String]) {
        self.release_reference_handles(PoolReferenceOwner::Manager(reference_id), server_names)
            .await;
    }

    async fn release_reference_handles(&self, owner: PoolReferenceOwner, server_names: &[String]) {
        let mut refs = self.ref_counts.lock().await;
        self.flush_released_references(&mut refs);
        let mut sessions = self.session_refs.lock().unwrap_or_else(|e| e.into_inner());
        for name in server_names {
            if sessions
                .get_mut(&owner)
                .is_some_and(|names| names.remove(name))
                && let Some(count) = refs.get_mut(name)
            {
                *count = count.saturating_sub(1);
            }
        }
        if sessions.get(&owner).is_some_and(|names| names.is_empty()) {
            sessions.remove(&owner);
        }
    }

    fn record_references<'a>(
        &self,
        owner: PoolReferenceOwner,
        names: impl Iterator<Item = &'a str>,
        refs: &mut HashMap<String, usize>,
    ) -> usize {
        self.flush_released_references(refs);
        let mut sessions = self.session_refs.lock().unwrap_or_else(|e| e.into_inner());
        let owned = sessions.entry(owner).or_default();
        let mut added = 0;
        for name in names {
            if owned.insert(name.to_string()) {
                *refs.entry(name.to_string()).or_default() += 1;
                added += 1;
            }
        }
        added
    }

    pub(super) fn release_manager_on_drop(&self, reference_id: u64) {
        self.release_reference_on_drop(DeferredReferenceRelease::Manager(reference_id));
    }

    fn release_reference_on_drop(&self, release: DeferredReferenceRelease) {
        self.released_references
            .lock()
            .unwrap_or_else(|e| e.into_inner())
            .push(release);
        if let Ok(mut refs) = self.ref_counts.try_lock() {
            self.flush_released_references(&mut refs);
        }
    }

    fn flush_released_references(&self, refs: &mut HashMap<String, usize>) {
        let released = std::mem::take(
            &mut *self
                .released_references
                .lock()
                .unwrap_or_else(|e| e.into_inner()),
        );
        if released.is_empty() {
            return;
        }
        let mut sessions = self.session_refs.lock().unwrap_or_else(|e| e.into_inner());
        for release in released {
            match release {
                DeferredReferenceRelease::Manager(reference_id) => {
                    if let Some(names) = sessions.remove(&PoolReferenceOwner::Manager(reference_id))
                    {
                        for name in names {
                            if let Some(count) = refs.get_mut(&name) {
                                *count = count.saturating_sub(1);
                            }
                        }
                    }
                }
                DeferredReferenceRelease::Claim(reference_id, name) => {
                    let owner = PoolReferenceOwner::Manager(reference_id);
                    if sessions
                        .get_mut(&owner)
                        .is_some_and(|names| names.remove(&name))
                        && let Some(count) = refs.get_mut(&name)
                    {
                        *count = count.saturating_sub(1);
                    }
                    if sessions.get(&owner).is_some_and(|names| names.is_empty()) {
                        sessions.remove(&owner);
                    }
                }
            }
        }
    }
    /// Get a handle for a specific server
    pub async fn get_handle(&self, name: &str) -> Option<McpHandle> {
        let _admission = self.admission.read().await;
        if self.cleanup_pending() {
            return None;
        }
        let handles = self.handles.read().await;
        handles
            .get(name)
            .filter(|handle| !handle.is_closed())
            .cloned()
    }
}
