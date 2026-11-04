impl McpClient {
    fn new_handle(
        name: &str,
        config: &McpServerConfig,
        pending: Arc<Mutex<HashMap<u64, oneshot::Sender<JsonRpcResponse>>>>,
        closed: Arc<AtomicBool>,
        writer_tx: mpsc::Sender<String>,
    ) -> McpHandle {
        McpHandle {
            name: name.to_string(),
            request_id: Arc::new(AtomicU64::new(1)),
            pending,
            closed,
            writer_tx,
            server_info: Arc::new(std::sync::RwLock::new(None)),
            capabilities: Arc::new(std::sync::RwLock::new(ServerCapabilities::default())),
            tools: Arc::new(std::sync::RwLock::new(Vec::new())),
            request_timeout: request_timeout_for(config),
        }
    }

    /// Bound discovery, authentication refresh, initialization, fallback and
    /// tool listing together. `timeout_secs` can extend legitimate tool calls,
    /// but must not turn connecting to a remote server into an indefinite wait.
    async fn connect_remote(
        name: String,
        config: &McpServerConfig,
        transport: RemoteTransport,
    ) -> Result<Self> {
        tokio::time::timeout(
            REMOTE_MCP_HANDSHAKE_TIMEOUT,
            Self::connect_remote_handshake(&name, config, transport),
        )
        .await
        .with_context(|| format!("MCP server '{name}' connection timed out after 30s"))?
    }

    /// Connect to a remote (Streamable HTTP or legacy SSE) MCP server. A
    /// Streamable HTTP endpoint that rejects `initialize` with 400/404/405 is
    /// retried once as legacy SSE, mirroring the spec's backwards-compat rule.
    async fn connect_remote_handshake(
        name: &str,
        config: &McpServerConfig,
        transport: RemoteTransport,
    ) -> Result<Self> {
        crate::logging::info(&format!(
            "MCP: Connecting to '{}' ({})",
            name,
            config.display_target()
        ));
        match Self::connect_remote_once(name, config, transport).await {
            Ok(client) => Ok(client),
            Err((err, wants_sse)) if wants_sse && transport == RemoteTransport::StreamableHttp => {
                crate::logging::info(&format!(
                    "MCP: '{}' rejected Streamable HTTP ({:#}); retrying as legacy SSE",
                    name, err
                ));
                Self::connect_remote_once(name, config, RemoteTransport::LegacySse)
                    .await
                    .map_err(|(sse_err, _)| {
                        sse_err.context(format!(
                            "MCP server '{name}' failed over both Streamable HTTP ({err:#}) and SSE"
                        ))
                    })
            }
            Err((err, _)) => Err(err),
        }
    }

    async fn connect_remote_once(
        name: &str,
        config: &McpServerConfig,
        transport: RemoteTransport,
    ) -> std::result::Result<Self, (anyhow::Error, bool)> {
        let pending: Arc<Mutex<HashMap<u64, oneshot::Sender<JsonRpcResponse>>>> =
            Arc::new(Mutex::new(HashMap::new()));
        let closed = Arc::new(AtomicBool::new(false));
        let (writer_tx, writer_rx) = mpsc::channel::<String>(32);
        let connection = super::remote::start(
            name,
            config,
            transport,
            writer_rx,
            Arc::clone(&pending),
            Arc::clone(&closed),
        )
        .await
        .map_err(|err| (err, false))?;

        let handle = Self::new_handle(name, config, pending, closed, writer_tx);
        let mut client = Self {
            handle,
            backend: Backend::Remote(connection),
            shutdown_pending: false,
        };

        if let Err(err) = client.initialize().await {
            let wants_sse = matches!(&client.backend, Backend::Remote(c) if c.wants_sse_fallback());
            return Err((
                err.context(format!("MCP server '{name}' failed to initialize")),
                wants_sse,
            ));
        }
        client
            .handle
            .refresh_tools()
            .await
            .with_context(|| format!("MCP server '{name}' failed to list tools"))
            .map_err(|err| (err, false))?;

        crate::logging::info(&format!(
            "MCP: Connected to '{}' with {} tools",
            name,
            client.handle.tools().len()
        ));
        Ok(client)
    }
}
