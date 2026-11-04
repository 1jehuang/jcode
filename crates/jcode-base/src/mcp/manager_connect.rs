impl McpManager {
    #[cfg(all(windows, feature = "test-support"))]
    pub async fn test_adopt_natural_client(&self, name: String, client: McpClient) {
        assert!(!self.owned_clients.read().await.contains_key(&name));
        self.owned_clients.write().await.insert(name, client);
    }

    /// Create a new manager in owned in-process mode (used by tests and local harnesses).
    pub fn new() -> Self {
        let project_dir = std::env::current_dir().ok();
        Self {
            pool: None,
            pool_handles: RwLock::new(HashMap::new()),
            owned_clients: RwLock::new(HashMap::new()),
            connecting: Arc::new(std::sync::Mutex::new(HashMap::new())),
            shared_operations: std::sync::Mutex::new(HashMap::new()),
            admission: RwLock::new(()),
            cleanup_pending: std::sync::atomic::AtomicBool::new(false),
            config: McpConfig::load_for_dir(project_dir.as_deref()),
            session_id: "owned".to_string(),
            reference_id: next_manager_reference_id(),
            project_dir,
        }
    }

    /// Create a manager backed by a shared pool (daemon mode)
    pub fn with_shared_pool(pool: Arc<SharedMcpPool>, session_id: String) -> Self {
        Self::with_shared_pool_for_dir(pool, session_id, None)
    }

    /// Create a manager backed by a shared pool, resolving project-local MCP
    /// config against `project_dir` instead of the server process cwd
    /// (issue #420: remote/client sessions must use the session working dir).
    pub fn with_shared_pool_for_dir(
        pool: Arc<SharedMcpPool>,
        session_id: String,
        project_dir: Option<std::path::PathBuf>,
    ) -> Self {
        Self {
            pool: Some(pool),
            pool_handles: RwLock::new(HashMap::new()),
            owned_clients: RwLock::new(HashMap::new()),
            connecting: Arc::new(std::sync::Mutex::new(HashMap::new())),
            shared_operations: std::sync::Mutex::new(HashMap::new()),
            admission: RwLock::new(()),
            cleanup_pending: std::sync::atomic::AtomicBool::new(false),
            config: McpConfig::load_for_dir(project_dir.as_deref()),
            session_id,
            reference_id: next_manager_reference_id(),
            project_dir,
        }
    }

    /// Create manager with specific config (no sharing)
    pub fn with_config(config: McpConfig) -> Self {
        Self {
            pool: None,
            pool_handles: RwLock::new(HashMap::new()),
            owned_clients: RwLock::new(HashMap::new()),
            connecting: Arc::new(std::sync::Mutex::new(HashMap::new())),
            shared_operations: std::sync::Mutex::new(HashMap::new()),
            admission: RwLock::new(()),
            cleanup_pending: std::sync::atomic::AtomicBool::new(false),
            config,
            session_id: "owned".to_string(),
            reference_id: next_manager_reference_id(),
            project_dir: None,
        }
    }

    /// Whether this manager has a shared pool available
    pub fn is_shared(&self) -> bool {
        self.pool.is_some()
    }

    /// Connect to all configured servers.
    /// Shared servers go to the pool, non-shared are spawned per-session.
    #[expect(
        clippy::collapsible_if,
        reason = "MCP connect flow keeps shared-pool and owned-server paths explicit"
    )]
    pub async fn connect_all(&self) -> Result<(usize, Vec<(String, String)>)> {
        let _admission = self.admit().await?;
        self.connect_all_inner().await
    }

    async fn connect_all_inner(&self) -> Result<(usize, Vec<(String, String)>)> {
        let futures =
            self.config
                .servers
                .iter()
                .filter(|(_, config)| config.is_enabled())
                .map(|(name, config)| async move {
                    (name.clone(), self.connect_inner(name, config).await)
                });
        let mut successes = 0;
        let mut failures = Vec::new();
        for (name, result) in futures::future::join_all(futures).await {
            match result {
                Ok(()) => successes += 1,
                Err(error) => failures.push((name, format!("{error:#}"))),
            }
        }
        Ok((successes, failures))
    }
    /// Connect to a specific server
    #[expect(
        clippy::collapsible_if,
        reason = "MCP connect flow keeps shared-pool and owned-server paths explicit"
    )]
    pub async fn connect(&self, name: &str, config: &McpServerConfig) -> Result<()> {
        let _admission = self.admit().await?;
        self.connect_inner(name, config).await
    }

    async fn connect_inner(&self, name: &str, config: &McpServerConfig) -> Result<()> {
        if self
            .owned_clients
            .read()
            .await
            .get(name)
            .is_some_and(McpClient::cleanup_pending)
        {
            anyhow::bail!(
                "MCP server '{name}' cleanup is unconfirmed; retry disconnect before reconnecting"
            );
        }
        // Partner-discovery provenance: if this server's command matches a
        // setup the agent saw in a discover_tools listing, tag it so calls to
        // it are metered coarsely (counts only; see sponsors::provenance).
        if let Some(sponsor) =
            crate::sponsors::provenance::on_server_connected(name, &config.command, &config.args)
        {
            crate::logging::info(&format!(
                "MCP: '{name}' connected via integration discovery (provider: {sponsor}); \
                 coarse usage counts are shared per the disclosed policy"
            ));
        }
        if config.shared {
            if self.pool.is_some() {
                return self
                    .connect_shared_using(name, config, std::future::ready(()))
                    .await;
            }
        }

        // Owned (non-shared or no pool available). Coordinate concurrent
        // callers so background startup and a manual connect share one
        // handshake instead of spawning duplicate child processes.
        self.connect_owned(name, config).await
    }

    #[allow(clippy::await_holding_lock)]
    async fn owned_connection_ready(&self, name: &str) -> Result<bool> {
        let clients = self.owned_clients.read().await;
        if let Some(client) = clients.get(name) {
            if client.cleanup_pending() {
                anyhow::bail!(
                    "MCP server '{name}' cleanup is unconfirmed; retry disconnect before reconnecting"
                );
            }
            return Ok(!client.handle().is_closed());
        }
        Ok(false)
    }

    async fn connect_owned(&self, name: &str, config: &McpServerConfig) -> Result<()> {
        if self.owned_connection_ready(name).await? {
            return Ok(());
        }

        let (mut waiter, leader, sender) = {
            let clients = self.owned_clients.read().await;
            if let Some(client) = clients.get(name) {
                if client.cleanup_pending() {
                    anyhow::bail!("MCP server '{name}' cleanup is unconfirmed");
                }
                if !client.handle().is_closed() {
                    return Ok(());
                }
            }
            let mut connecting = self.connecting.lock().unwrap_or_else(|e| e.into_inner());
            if let Some(sender) = connecting.get(name) {
                (Some(sender.subscribe()), false, None)
            } else {
                let (sender, _receiver) = watch::channel(None);
                connecting.insert(name.to_string(), sender.clone());
                (None, true, Some(sender))
            }
        };

        if !leader {
            let receiver = waiter
                .as_mut()
                .expect("owned MCP connection waiter must have a receiver");
            receiver.changed().await.with_context(|| {
                format!("MCP server '{}' connection attempt was cancelled", name)
            })?;
            return receiver
                .borrow()
                .clone()
                .unwrap_or_else(|| Err(format!("MCP server '{}' connection did not finish", name)))
                .map_err(anyhow::Error::msg);
        }

        let sender = sender.expect("owned MCP connection leader must have a sender");
        let mut attempt_guard =
            OwnedConnectionAttemptGuard::new(Arc::clone(&self.connecting), name, sender.clone());
        let connection = async {
            {
                let mut clients = self.owned_clients.write().await;
                if let Some(client) = clients.get_mut(name) {
                    client
                        .shutdown()
                        .await
                        .with_context(|| format!("MCP stale owner '{name}' cleanup failed"))?;
                }
                clients.remove(name);
            }
            McpClient::connect_in_dir(name.to_string(), config, self.project_dir.as_deref()).await
        }
        .await;

        let mut clients = self.owned_clients.write().await;
        let mut connecting = self.connecting.lock().unwrap_or_else(|e| e.into_inner());
        let result = match connection {
            Ok(client) => {
                clients.insert(name.to_string(), client);
                Ok(())
            }
            Err(error) => Err(format!("{:#}", error)),
        };

        if connecting
            .get(name)
            .is_some_and(|current| current.same_channel(&sender))
        {
            connecting.remove(name);
            let _ = sender.send(Some(result.clone()));
        }
        attempt_guard.disarm();

        result.map_err(anyhow::Error::msg)
    }
}
