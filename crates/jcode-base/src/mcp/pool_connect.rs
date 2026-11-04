impl SharedMcpPool {
    fn lock_connecting(
        &self,
    ) -> std::sync::MutexGuard<'_, HashMap<String, Arc<SharedConnectionAttempt>>> {
        self.connecting
            .lock()
            .unwrap_or_else(|poisoned| poisoned.into_inner())
    }
    fn complete_attempt(
        &self,
        name: &str,
        attempt: &Arc<SharedConnectionAttempt>,
        result: std::result::Result<(), String>,
    ) {
        let mut connecting = self.lock_connecting();
        if connecting
            .get(name)
            .is_some_and(|current| Arc::ptr_eq(current, attempt))
        {
            connecting.remove(name);
        }
        attempt.complete(result);
    }
    async fn begin_connect(&self, name: &str) -> ConnectAttempt {
        let attempt = {
            let mut connecting = self.lock_connecting();
            if let Some(current) = connecting.get(name) {
                return ConnectAttempt::Wait(Arc::clone(current));
            }
            let attempt = Arc::new(SharedConnectionAttempt::new(SharedOperation::Connect));
            connecting.insert(name.to_string(), Arc::clone(&attempt));
            attempt
        };
        let mut guard = SharedConnectionAttemptGuard::new(
            Arc::clone(&self.connecting),
            name,
            Arc::clone(&attempt),
        );
        let owner = self.clients.lock().await.get(name).cloned();
        if let Some(owner) = owner {
            let mut client = owner.lock().await;
            if client.cleanup_pending() {
                let error = format!(
                    "MCP server '{name}' cleanup is unconfirmed; retry disconnect before reconnecting"
                );
                self.complete_attempt(name, &attempt, Err(error.clone()));
                guard.disarm();
                return ConnectAttempt::Failed(error);
            }
            if !client.handle().is_closed() {
                self.complete_attempt(name, &attempt, Ok(()));
                guard.disarm();
                return ConnectAttempt::Connected;
            }
            if let Err(error) = client.shutdown().await {
                let error = format!("MCP stale owner '{name}' cleanup failed: {error:#}");
                self.complete_attempt(name, &attempt, Err(error.clone()));
                guard.disarm();
                return ConnectAttempt::Failed(error);
            }
            let mut clients = self.clients.lock().await;
            let mut handles = self.handles.write().await;
            let registry = self.lock_connecting();
            let current_generation = registry
                .get(name)
                .is_some_and(|current| Arc::ptr_eq(current, &attempt));
            let current_owner = clients
                .get(name)
                .is_some_and(|current| Arc::ptr_eq(current, &owner));
            if !current_generation || !current_owner {
                drop(registry);
                drop(handles);
                drop(clients);
                let error = format!("MCP stale owner '{name}' cleanup generation was superseded");
                self.complete_attempt(name, &attempt, Err(error.clone()));
                guard.disarm();
                return ConnectAttempt::Failed(error);
            }
            clients.remove(name);
            handles.remove(name);
        }
        guard.disarm();
        ConnectAttempt::Leader(attempt)
    }

    async fn finish_connect(
        &self,
        name: &str,
        notify: &Arc<SharedConnectionAttempt>,
        result: Result<McpClient>,
    ) -> std::result::Result<bool, String> {
        let mut clients = self.clients.lock().await;
        let mut handles = self.handles.write().await;
        let mut errors = self.last_errors.write().await;
        let mut connecting = self.lock_connecting();
        if !connecting
            .get(name)
            .is_some_and(|current| Arc::ptr_eq(current, notify))
        {
            let error = format!("MCP server '{name}' connection generation was superseded");
            notify.complete(Err(error.clone()));
            return Err(error);
        }
        let outcome = match result {
            Ok(client) => {
                let handle = client.handle();
                clients.insert(name.to_string(), Arc::new(Mutex::new(client)));
                handles.insert(name.to_string(), handle);
                errors.remove(name);
                Ok(true)
            }
            Err(error) => {
                let message = format!("{error:#}");
                errors.insert(
                    name.to_string(),
                    FailedConnectRecord {
                        message: message.clone(),
                        failed_at: Instant::now(),
                    },
                );
                Err(message)
            }
        };
        connecting.remove(name);
        notify.complete(outcome.clone().map(|_| ()));
        outcome
    }

    async fn connect_as_leader<F>(
        &self,
        name: &str,
        notify: Arc<SharedConnectionAttempt>,
        connection: F,
    ) -> std::result::Result<bool, String>
    where
        F: std::future::Future<Output = Result<McpClient>>,
    {
        let mut guard = SharedConnectionAttemptGuard::new(
            Arc::clone(&self.connecting),
            name,
            Arc::clone(&notify),
        );
        let result = connection.await;
        let outcome = self.finish_connect(name, &notify, result).await;
        guard.disarm();
        outcome
    }

    async fn ensure_connected(
        &self,
        name: String,
        config: McpServerConfig,
    ) -> std::result::Result<bool, String> {
        self.ensure_connected_using(name, config, |name, config| async move {
            McpClient::connect(name, &config).await
        })
        .await
    }
    async fn ensure_connected_using<C, F>(
        &self,
        name: String,
        config: McpServerConfig,
        connect: C,
    ) -> std::result::Result<bool, String>
    where
        C: Fn(String, McpServerConfig) -> F,
        F: std::future::Future<Output = Result<McpClient>>,
    {
        loop {
            if let Some(record) = self.recent_failure(&name).await {
                let retry_after = FAILED_CONNECT_RETRY_COOLDOWN
                    .saturating_sub(record.failed_at.elapsed())
                    .as_secs()
                    .max(1);
                return Err(format!(
                    "{} (retry suppressed for ~{}s after recent failure)",
                    record.message, retry_after
                ));
            }
            match self.begin_connect(&name).await {
                ConnectAttempt::Failed(error) => return Err(error),
                ConnectAttempt::Connected => return Ok(false),
                ConnectAttempt::Wait(attempt) => {
                    self.wait_for_connect(&name, &attempt).await;
                    let result = attempt.result().unwrap_or_else(|| {
                        Err(format!(
                            "MCP server '{name}' lifecycle operation did not finish"
                        ))
                    });
                    if attempt.operation == SharedOperation::Disconnect && result.is_ok() {
                        continue;
                    }
                    return result.map(|()| false);
                }
                ConnectAttempt::Leader(attempt) => {
                    return self
                        .connect_as_leader(&name, attempt, connect(name.clone(), config.clone()))
                        .await;
                }
            }
        }
    }

    async fn commit_disconnect(
        &self,
        name: &str,
        attempt: &Arc<SharedConnectionAttempt>,
        owner: Option<&Arc<Mutex<McpClient>>>,
    ) -> Result<()> {
        let mut clients = self.clients.lock().await;
        let mut handles = self.handles.write().await;
        let mut refs = self.ref_counts.lock().await;
        let mut errors = self.last_errors.write().await;
        self.flush_released_references(&mut refs);
        let mut registry = self.lock_connecting();
        if !registry
            .get(name)
            .is_some_and(|current| Arc::ptr_eq(current, attempt))
        {
            let error = format!("MCP disconnect '{name}' generation was superseded");
            attempt.complete(Err(error.clone()));
            anyhow::bail!(error);
        }
        let same_owner = match (clients.get(name), owner) {
            (None, None) => true,
            (Some(current), Some(expected)) => Arc::ptr_eq(current, expected),
            _ => false,
        };
        if !same_owner {
            let error = format!("MCP disconnect '{name}' owner was superseded");
            registry.remove(name);
            attempt.complete(Err(error.clone()));
            anyhow::bail!(error);
        }
        clients.remove(name);
        handles.remove(name);
        refs.remove(name);
        self.session_refs
            .lock()
            .unwrap_or_else(|e| e.into_inner())
            .retain(|_, names| {
                names.remove(name);
                !names.is_empty()
            });
        errors.remove(name);
        registry.remove(name);
        attempt.complete(Ok(()));
        Ok(())
    }

    async fn wait_for_connect(&self, _name: &str, attempt: &Arc<SharedConnectionAttempt>) {
        let notified = attempt.notify.notified();
        tokio::pin!(notified);
        notified.as_mut().enable();
        if attempt.result().is_none() {
            notified.await;
        }
    }

    async fn recent_failure(&self, name: &str) -> Option<FailedConnectRecord> {
        if self
            .handles
            .read()
            .await
            .get(name)
            .is_some_and(|handle| !handle.is_closed())
        {
            return None;
        }

        self.last_errors
            .read()
            .await
            .get(name)
            .filter(|record| record.failed_at.elapsed() < FAILED_CONNECT_RETRY_COOLDOWN)
            .cloned()
    }
}
