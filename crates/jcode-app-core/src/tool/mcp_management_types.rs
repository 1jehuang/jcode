#[derive(Debug, Deserialize)]
struct McpToolInput {
    action: String,
    #[serde(default)]
    server: Option<String>,
    #[serde(default)]
    command: Option<String>,
    #[serde(default)]
    args: Option<Vec<String>>,
    #[serde(default)]
    env: Option<HashMap<String, String>>,
    #[serde(default)]
    url: Option<String>,
    #[serde(default)]
    headers: Option<HashMap<String, String>>,
    #[serde(default)]
    transport: Option<String>,
}

fn connection_config(
    params: &McpToolInput,
    configured: Option<McpServerConfig>,
) -> Result<McpServerConfig> {
    if params.command.is_some() && params.url.is_some() {
        anyhow::bail!("Supply either 'command' for stdio or 'url' for remote MCP, not both");
    }
    if params.command.is_none() && params.url.is_none() {
        if params.args.is_some()
            || params.env.is_some()
            || params.headers.is_some()
            || params.transport.is_some()
        {
            anyhow::bail!(
                "Connection overrides require an explicit 'command' or 'url'; omit them to use the configured server"
            );
        }
        return configured.ok_or_else(|| anyhow::anyhow!("'command' or 'url' is required for connect action (server is not in the MCP config)"));
    }
    if let Some(url) = params.url.as_deref() {
        if params.args.is_some() || params.env.is_some() {
            anyhow::bail!("'args' and 'env' are only supported for stdio MCP servers");
        }
        let transport = params.transport.as_deref().unwrap_or("http");
        if !matches!(transport, "http" | "streamable-http" | "sse") {
            anyhow::bail!("Remote 'transport' must be 'http', 'streamable-http', or 'sse'");
        }
        validate_remote_url(url)?;
    } else {
        if params
            .command
            .as_deref()
            .is_none_or(|command| command.trim().is_empty())
        {
            anyhow::bail!("'command' must not be empty");
        }
        if params.headers.is_some()
            || params
                .transport
                .as_deref()
                .is_some_and(|transport| transport != "stdio")
        {
            anyhow::bail!("'headers' and remote transports require 'url'");
        }
    }
    Ok(McpServerConfig {
        command: params
            .command
            .as_ref()
            .map_or_else(String::new, Clone::clone),
        args: params.args.as_ref().map_or_else(Vec::new, Clone::clone),
        env: params
            .env
            .as_ref()
            .map_or_else(Default::default, Clone::clone),
        shared: true,
        transport: params.transport.clone(),
        url: params.url.clone(),
        headers: params
            .headers
            .as_ref()
            .map_or_else(Default::default, Clone::clone),
        enabled: None,
        disabled: None,
        timeout_secs: None,
    })
}

fn validate_remote_url(value: &str) -> Result<()> {
    let url = url::Url::parse(value).map_err(|_| anyhow::anyhow!("Invalid remote MCP URL"))?;
    if !matches!(url.scheme(), "http" | "https")
        || url.host_str().is_none()
        || !url.username().is_empty()
        || url.password().is_some()
        || url.fragment().is_some()
    {
        anyhow::bail!(
            "Remote MCP URL must be HTTP(S), without embedded user credentials or a fragment"
        );
    }
    Ok(())
}

pub struct McpManagementTool {
    manager: Arc<RwLock<McpManager>>,
    registry: Option<crate::tool::WeakRegistry>,
}

impl McpManagementTool {
    pub fn new(manager: Arc<RwLock<McpManager>>) -> Self {
        Self {
            manager,
            registry: None,
        }
    }

    pub fn with_registry(mut self, registry: crate::tool::Registry) -> Self {
        self.registry = Some(registry.downgrade());
        self
    }
}
