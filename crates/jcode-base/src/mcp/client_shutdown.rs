impl McpClient {
    pub fn cleanup_pending(&self) -> bool {
        self.shutdown_pending
    }

    /// Shutdown the server.
    pub async fn shutdown(&mut self) -> Result<()> {
        self.shutdown_pending = true;
        self.handle.closed.store(true, Ordering::SeqCst);
        let result: Result<()> = match &mut self.backend {
            Backend::Stdio(child) => {
                #[cfg(windows)]
                let mut guard = child
                    .shutdown_guard()
                    .context("Cannot retain owned MCP Job for shutdown")?;
                #[cfg(not(windows))]
                if self
                    .handle
                    .writer_tx
                    .send("{\"jsonrpc\":\"2.0\",\"method\":\"shutdown\"}\n".to_string())
                    .await
                    .is_err()
                {
                    crate::logging::warn(&format!(
                        "MCP [{}]: shutdown notification could not be queued",
                        self.handle.name
                    ));
                }
                #[cfg(windows)]
                if !matches!(
                    tokio::time::timeout(
                        std::time::Duration::from_millis(100),
                        self.handle
                            .writer_tx
                            .send("{\"jsonrpc\":\"2.0\",\"method\":\"shutdown\"}\n".to_string())
                    )
                    .await,
                    Ok(Ok(()))
                ) {
                    crate::logging::warn(&format!(
                        "MCP [{}]: shutdown notification could not be queued",
                        self.handle.name
                    ));
                }
                #[cfg(all(windows, feature = "test-support"))]
                {
                    let observer = {
                        *SHUTDOWN_OBSERVER
                            .lock()
                            .unwrap_or_else(|error| error.into_inner())
                    };
                    if let Some(observer) = observer {
                        observer();
                    }
                }
                tokio::time::sleep(std::time::Duration::from_millis(100)).await;
                #[cfg(not(windows))]
                {
                    child
                        .kill()
                        .await
                        .context("MCP child shutdown did not complete")
                }
                #[cfg(windows)]
                {
                    if let Err(error) = child.terminate_owned_job(guard.job()) {
                        crate::logging::warn(&format!(
                            "MCP [{}]: Job termination request failed: {error}",
                            self.handle.name
                        ));
                    }
                    let drained = StdioProcess::wait_job_empty(
                        guard.job(),
                        std::time::Duration::from_secs(2),
                    )
                    .await
                    .with_context(|| {
                        format!(
                            "MCP '{}' shutdown unconfirmed: owned Job did not drain",
                            self.handle.name
                        )
                    });
                    guard.disarm();
                    drained
                }
            }
            Backend::Remote(connection) => {
                connection.shutdown().await;
                Ok(())
            }
        };
        if result.is_ok() {
            self.shutdown_pending = false;
        }
        result
    }
}
