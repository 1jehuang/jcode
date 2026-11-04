impl McpClient {
    #[cfg(all(windows, feature = "test-support"))]
    pub fn test_set_native_observer(observer: Option<NativeObserver>) {
        *NATIVE_OBSERVER.lock().unwrap() = observer;
    }
    #[cfg(all(windows, feature = "test-support"))]
    pub fn test_set_spawn_window_observer(observer: Option<fn()>) {
        super::windows_native_launch::test_set_spawn_observer(observer);
    }
    #[cfg(all(windows, feature = "test-support"))]
    pub fn test_set_shutdown_observer(observer: Option<fn()>) {
        *SHUTDOWN_OBSERVER.lock().unwrap() = observer;
    }
    /// Connect to an MCP server, inheriting the current process working directory
    pub async fn connect(name: String, config: &McpServerConfig) -> Result<Self> {
        Self::connect_in_dir(name, config, None).await
    }

    /// Connect to an MCP server, optionally running it in `working_dir`.
    ///
    /// The working directory is only applied when it exists; otherwise the
    /// subprocess falls back to inheriting the current process cwd (issue #557).
    pub async fn connect_in_dir(
        name: String,
        config: &McpServerConfig,
        working_dir: Option<&std::path::Path>,
    ) -> Result<Self> {
        Self::connect_in_dir_mode(name, config, working_dir, false).await
    }

    #[cfg(all(windows, feature = "test-support"))]
    pub async fn connect_natural_test(name: String, config: &McpServerConfig) -> Result<Self> {
        Self::connect_in_dir_mode(name, config, None, true).await
    }

    #[cfg(all(windows, feature = "test-support"))]
    pub fn test_owned_job_handle(&self) -> Option<usize> {
        match &self.backend {
            Backend::Stdio(child) => child.test_job_handle(),
            _ => None,
        }
    }

    #[cfg(all(windows, feature = "test-support"))]
    pub fn test_owned_process_ids(&self) -> Result<(u32, u32)> {
        match &self.backend {
            Backend::Stdio(child) => Ok((child.id()?, child.broker_id()?)),
            _ => anyhow::bail!("no native MCP owner"),
        }
    }

    async fn connect_in_dir_mode(
        name: String,
        config: &McpServerConfig,
        working_dir: Option<&std::path::Path>,
        _natural_test: bool,
    ) -> Result<Self> {
        if let Some(transport) = config.remote_transport() {
            return Self::connect_remote(name, config, transport).await;
        }
        let working_dir = working_dir.filter(|dir| dir.is_dir());
        crate::logging::info(&format!(
            "MCP: Connecting to '{}' ({} {:?}) cwd={:?}",
            name, config.command, config.args, working_dir
        ));

        // Credentials must be opted into an MCP server explicitly through its
        // config. The long-lived jcode daemon contains provider credentials in
        // its process environment, and blindly inheriting them exposes those
        // credentials to every configured MCP executable (issue #771).
        let inherited = std::env::vars_os().collect();
        let env = mcp_child_env(inherited, &config.env);

        #[cfg(not(windows))]
        let (child, stdin, stdout, stderr) = {
            let mut command = Command::new(&config.command);
            configure_mcp_command_env(&mut command, &env);
            command
                .args(&config.args)
                .stdin(Stdio::piped())
                .stdout(Stdio::piped())
                .stderr(Stdio::piped());
            if let Some(dir) = working_dir {
                command.current_dir(dir);
            }
            let mut child = command
                .spawn()
                .with_context(|| format!("Failed to spawn MCP server: {}", config.command))?;
            let stdin = child.stdin.take().context("No stdin")?;
            let stdout = child.stdout.take().context("No stdout")?;
            let stderr = child.stderr.take().context("No stderr")?;
            (child, stdin, stdout, stderr)
        };
        #[cfg(windows)]
        let (child, stdin, stdout, stderr) = {
            #[cfg(feature = "test-support")]
            let mut spawned = if _natural_test {
                super::windows_native_command::spawn_natural_test(
                    &config.command,
                    &config.args,
                    &env,
                    working_dir,
                )
            } else {
                super::windows_native_command::spawn(
                    &config.command,
                    &config.args,
                    &env,
                    working_dir,
                )
            }
            .with_context(|| format!("Failed to spawn MCP server: {}", config.command))?;
            #[cfg(not(feature = "test-support"))]
            let spawned = super::windows_native_command::spawn(
                &config.command,
                &config.args,
                &env,
                working_dir,
            )
            .with_context(|| format!("Failed to spawn MCP server: {}", config.command))?;
            #[cfg(feature = "test-support")]
            if _natural_test {
                spawned.process.resume_test_broker()?;
            }
            #[cfg(feature = "test-support")]
            {
                let observer = { *NATIVE_OBSERVER.lock().unwrap() };
                if let Some(observer) = observer {
                    let (job, root, broker, root_id, broker_id) =
                        spawned.process.test_owned_handles()?;
                    observer(job, root, broker, root_id, broker_id);
                }
            }
            crate::logging::info(&format!(
                "MCP [{}]: owned native process={} broker={} natural_fixture={}",
                name,
                spawned.process.id()?,
                spawned.process.broker_id()?,
                _natural_test
            ));
            (
                spawned.process,
                spawned.stdin,
                spawned.stdout,
                spawned.stderr,
            )
        };
        // Spawn stderr reader
        let server_name = name.clone();
        tokio::spawn(async move {
            let mut reader = BufReader::new(stderr);
            let mut line = String::new();
            loop {
                line.clear();
                match reader.read_line(&mut line).await {
                    Ok(0) => break,
                    Ok(_) => {
                        let trimmed = line.trim();
                        if !trimmed.is_empty() {
                            crate::logging::warn(&format!(
                                "MCP [{}] stderr: {}",
                                server_name, trimmed
                            ));
                        }
                    }
                    Err(_) => break,
                }
            }
        });

        // Setup channels
        let pending: Arc<Mutex<HashMap<u64, oneshot::Sender<JsonRpcResponse>>>> =
            Arc::new(Mutex::new(HashMap::new()));
        let (writer_tx, mut writer_rx) = mpsc::channel::<String>(32);

        // Spawn writer task
        let mut stdin = stdin;
        tokio::spawn(async move {
            while let Some(msg) = writer_rx.recv().await {
                if stdin.write_all(msg.as_bytes()).await.is_err() {
                    break;
                }
                if stdin.flush().await.is_err() {
                    break;
                }
            }
        });

        // Spawn reader task
        let pending_clone = Arc::clone(&pending);
        let closed = Arc::new(AtomicBool::new(false));
        let closed_clone = Arc::clone(&closed);
        let reader_name = name.clone();
        let mut reader = BufReader::new(stdout);
        tokio::spawn(async move {
            let mut line = String::new();
            loop {
                line.clear();
                match reader.read_line(&mut line).await {
                    Ok(0) => {
                        crate::logging::debug(&format!("MCP [{}]: stdout EOF", reader_name));
                        break;
                    }
                    Ok(_) => {
                        if let Ok(response) = serde_json::from_str::<JsonRpcResponse>(&line) {
                            if let Some(id) = response.id {
                                let mut pending = pending_clone.lock().await;
                                if let Some(tx) = pending.remove(&id) {
                                    let _ = tx.send(response);
                                }
                            }
                        } else {
                            let trimmed = line.trim();
                            if !trimmed.is_empty() {
                                crate::logging::debug(&format!(
                                    "MCP [{}] non-JSON output: {}",
                                    reader_name, trimmed
                                ));
                            }
                        }
                    }
                    Err(e) => {
                        crate::logging::warn(&format!("MCP [{}] read error: {}", reader_name, e));
                        break;
                    }
                }
            }
            // Server can never reply now: drop in-flight senders, fail later requests fast.
            let mut pending = pending_clone.lock().await;
            closed_clone.store(true, Ordering::SeqCst);
            pending.clear();
        });

        let handle = McpHandle {
            name: name.clone(),
            request_id: Arc::new(AtomicU64::new(1)),
            pending,
            closed,
            writer_tx,
            server_info: Arc::new(std::sync::RwLock::new(None)),
            capabilities: Arc::new(std::sync::RwLock::new(ServerCapabilities::default())),
            tools: Arc::new(std::sync::RwLock::new(Vec::new())),
            request_timeout: request_timeout_for(config),
        };

        let mut client = Self {
            handle,
            backend: Backend::Stdio(child),
            shutdown_pending: false,
        };

        client
            .initialize()
            .await
            .with_context(|| format!("MCP server '{}' failed to initialize", name))?;

        client
            .handle
            .refresh_tools()
            .await
            .with_context(|| format!("MCP server '{}' failed to list tools", name))?;

        crate::logging::info(&format!(
            "MCP: Connected to '{}' with {} tools",
            name,
            client.handle.tools().len()
        ));

        Ok(client)
    }
}
