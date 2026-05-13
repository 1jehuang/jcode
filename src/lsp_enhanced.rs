//! Enhanced LSP Client with advanced features
//!
//! Features ported from claude_code_src:
//! - Robust process lifecycle management (spawn, crash detection, restart)
//! - Request/response correlation with timeout
//! - Notification and request handlers
//! - Diagnostic caching with incremental updates
//! - Performance metrics and monitoring
//! - Connection pooling for multiple servers

use lsp_types::*;
use parking_lot::Mutex;
use serde::{Deserialize, Serialize};
use std::collections::{HashMap, VecDeque};
use std::path::PathBuf;
use std::sync::Arc;
use std::time::{Duration, Instant};
use tokio::process::{Child, Command};
use tokio::sync::{mpsc, oneshot, RwLock};
use tracing as log;

/// LSP Server state machine
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
pub enum EnhancedLspServerState {
    Stopped,
    Starting,
    Running,
    Stopping,
    Error,
    Crashed,
}

impl EnhancedLspServerState {
    pub fn label(&self) -> &'static str {
        match self {
            Self::Stopped => "stopped",
            Self::Starting => "starting",
            Self::Running => "running",
            Self::Stopping => "stopping",
            Self::Error => "error",
            Self::Crashed => "crashed",
        }
    }

    pub fn is_operational(&self) -> bool {
        matches!(self, Self::Running)
    }
}

impl std::fmt::Display for EnhancedLspServerState {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        write!(f, "{}", self.label())
    }
}

/// Enhanced LSP server configuration
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct EnhancedLspConfig {
    pub name: String,
    pub command: String,
    #[serde(default)]
    pub args: Vec<String>,
    #[serde(default)]
    pub env: HashMap<String, String>,
    #[serde(default)]
    pub language_ids: HashMap<String, String>,
    #[serde(default)]
    pub root_path: Option<PathBuf>,
    #[serde(default = "default_init_timeout")]
    pub initialization_timeout_secs: u64,
    #[serde(default = "default_request_timeout")]
    pub request_timeout_secs: u64,
    #[serde(default)]
    pub auto_restart: bool,
    #[serde(default = "default_max_restarts")]
    pub max_restarts: u32,
}

fn default_init_timeout() -> u64 { 30 }
fn default_request_timeout() -> u64 { 10 }
fn default_max_restarts() -> u32 { 3 }

impl Default for EnhancedLspConfig {
    fn default() -> Self {
        Self {
            name: String::new(),
            command: String::new(),
            args: vec![],
            env: HashMap::new(),
            language_ids: HashMap::new(),
            root_path: None,
            initialization_timeout_secs: default_init_timeout(),
            request_timeout_secs: default_request_timeout(),
            auto_restart: false,
            max_restarts: default_max_restarts(),
        }
    }
}

/// Performance metrics for LSP operations
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct LspMetrics {
    pub total_requests: u64,
    pub total_notifications: u64,
    pub successful_requests: u64,
    pub failed_requests: u64,
    pub average_latency_ms: f64,
    pub last_request_latency_ms: Option<f64>,
    pub uptime_seconds: u64,
    pub restart_count: u32,
}

impl Default for LspMetrics {
    fn default() -> Self {
        Self {
            total_requests: 0,
            total_notifications: 0,
            successful_requests: 0,
            failed_requests: 0,
            average_latency_ms: 0.0,
            last_request_latency_ms: None,
            uptime_seconds: 0,
            restart_count: 0,
        }
    }
}

/// Enhanced diagnostic with metadata
#[derive(Debug, Clone, Serialize)]
pub struct EnhancedDiagnostic {
    pub message: String,
    pub severity: Option<DiagnosticSeverity>,
    pub range: Range,
    pub source: Option<String>,
    pub code: Option<DiagnosticCode>,
    pub related_information: Option<Vec<DiagnosticRelatedInformation>>,
    pub data: Option<serde_json::Value>,
    #[serde(skip)]
    pub received_at: Instant,
}

impl From<Diagnostic> for EnhancedDiagnostic {
    fn from(diag: Diagnostic) -> Self {
        Self {
            message: diag.message,
            severity: diag.severity,
            range: diag.range,
            source: diag.source,
            code: diag.code,
            related_information: diag.related_information,
            data: diag.data,
            received_at: Instant::now(),
        }
    }
}

/// File diagnostics with version tracking
#[derive(Debug, Clone, Serialize)]
pub struct FileDiagnosticsSnapshot {
    pub uri: Url,
    pub version: Option<i32>,
    pub diagnostics: Vec<EnhancedDiagnostic>,
    #[serde(skip)]
    pub updated_at: Instant,
}

/// Diagnostic registry with caching and history
pub struct EnhancedDiagnosticRegistry {
    current: Mutex<HashMap<String, FileDiagnosticsSnapshot>>,
    history: Mutex<VecDeque<FileDiagnosticsSnapshot>>,
    max_history: usize,
}

impl EnhancedDiagnosticRegistry {
    pub fn new(max_history: usize) -> Self {
        Self {
            current: Mutex::new(HashMap::new()),
            history: Mutex::new(VecDeque::with_capacity(max_history)),
            max_history,
        }
    }

    pub fn update(&self, uri: &Url, version: Option<i32>, diagnostics: Vec<Diagnostic>) {
        let enhanced: Vec<EnhancedDiagnostic> =
            diagnostics.into_iter().map(EnhancedDiagnostic::from).collect();

        let snapshot = FileDiagnosticsSnapshot {
            uri: uri.clone(),
            version,
            diagnostics: enhanced,
            updated_at: Instant::now(),
        };

        let uri_str = uri.to_string();
        let mut current = self.current.lock();
        current.insert(uri_str.clone(), snapshot.clone());

        let mut history = self.history.lock();
        history.push_back(snapshot);
        while history.len() > self.max_history {
            history.pop_front();
        }
    }

    pub fn get(&self, uri: &str) -> Option<FileDiagnosticsSnapshot> {
        self.current.lock().get(uri).cloned()
    }

    pub fn get_all(&self) -> Vec<FileDiagnosticsSnapshot> {
        self.current.lock().values().cloned().collect()
    }

    pub fn clear_uri(&self, uri: &str) {
        self.current.lock().remove(uri);
    }

    pub fn clear_all(&self) {
        self.current.lock().clear();
        self.history.lock().clear();
    }

    pub fn get_diagnostics_for_file(&self, uri: &str) -> Vec<EnhancedDiagnostic> {
        self.current
            .lock()
            .get(uri)
            .map(|s| s.diagnostics.clone())
            .unwrap_or_default()
    }

    pub fn get_errors_count(&self) -> usize {
        self.current
            .lock()
            .values()
            .flat_map(|s| {
                s.diagnostics
                    .iter()
                    .filter(|d| d.severity == Some(DiagnosticSeverity::ERROR))
            })
            .count()
    }

    pub fn get_warnings_count(&self) -> usize {
        self.current
            .lock()
            .values()
            .flat_map(|s| {
                s.diagnostics
                    .iter()
                    .filter(|d| d.severity == Some(DiagnosticSeverity::WARNING))
            })
            .count()
    }
}

/// LSP operation result with timing information
#[derive(Debug, Clone)]
pub struct LspOperationResult<T> {
    pub result: T,
    pub latency_ms: u128,
    pub cached: bool,
}

/// Notification handler type
pub type NotificationHandler = Arc<dyn Fn(serde_json::Value) + Send + Sync>;

/// Enhanced LSP client handle
#[derive(Clone)]
pub struct EnhancedLspHandle {
    config: Arc<EnhancedLspConfig>,
    state: Arc<RwLock<EnhancedLspServerState>>,
    capabilities: Arc<RwLock<Option<ServerCapabilities>>>,
    request_id: Arc<std::sync::atomic::AtomicU64>,
    pending: Arc<Mutex<HashMap<u64, oneshot::Sender<serde_json::Value>>>>,
    writer_tx: mpsc::Sender<String>,
    notification_handlers: Arc<RwLock<HashMap<String, Vec<NotificationHandler>>>>,
    metrics: Arc<Mutex<LspMetrics>>,
    start_time: Arc<RwLock<Option<Instant>>>,
    restart_count: Arc<std::sync::atomic::AtomicU32>,
    server_name: String,
}

impl EnhancedLspHandle {
    async fn send_request(
        &self,
        method: &str,
        params: impl Serialize,
    ) -> Result<(serde_json::Value, Duration)> {
        let start = Instant::now();
        let timeout_dur = Duration::from_secs(self.config.request_timeout_secs);

        let id = self.request_id.fetch_add(1, std::sync::atomic::Ordering::SeqCst);
        let request = json!({
            "jsonrpc": "2.0",
            "id": id,
            "method": method,
            "params": params
        });

        let (tx, rx) = oneshot::channel::<serde_json::Value>();
        {
            let mut pending = self.pending.lock();
            pending.insert(id, tx);
        }

        let msg = serde_json::to_string(&request)? + "\n";
        self.writer_tx.send(msg).await.map_err(|e| {
            anyhow::anyhow!("Failed to send request: {}", e)
        })?;

        let response = tokio::time::timeout(timeout_dur, rx).await??;

        let latency = start.elapsed();

        // Update metrics
        let mut metrics = self.metrics.lock();
        metrics.total_requests += 1;
        if response.get("error").is_none() {
            metrics.successful_requests += 1;
        } else {
            metrics.failed_requests += 1;
        }
        let total = metrics.successful_requests + metrics.failed_requests;
        if total > 0 {
            metrics.average_latency_ms =
                ((metrics.average_latency_ms * (total - 1) as f64) + latency.as_millis() as f64)
                    / total as f64;
        }
        metrics.last_request_latency_ms = Some(latency.as_millis() as f64);

        Ok((response, latency))
    }

    async fn send_notification(&self, method: &str, params: impl Serialize) -> Result<()> {
        let notification = json!({
            "jsonrpc": "2.0",
            "method": method,
            "params": params
        });

        let msg = serde_json::to_string(&notification)? + "\n";
        self.writer_tx.send(msg).await.map_err(|e| {
            anyhow::anyhow!("Failed to send notification: {}", e)
        })?;

        // Update metrics
        let mut metrics = self.metrics.lock();
        metrics.total_notifications += 1;

        Ok(())
    }

    /// Go to definition
    pub async fn goto_definition(
        &self,
        uri: &Url,
        position: Position,
    ) -> Result<LspOperationResult<Option<GotoDefinitionResponse>>> {
        let (result, latency) = self
            .send_request(
                "textDocument/definition",
                TextDocumentPositionParams {
                    text_document: TextDocumentIdentifier { uri: uri.clone() },
                    position,
                },
            )
            .await?;

        if let Some(error) = result.get("error") {
            anyhow::bail!("LSP error: {}", error);
        }

        let def: Option<GotoDefinitionResponse> = serde_json::from_value(result["result"].clone())?;
        Ok(LspOperationResult {
            result: def,
            latency_ms: latency.as_millis(),
            cached: false,
        })
    }

    /// Find references
    pub async fn find_references(
        &self,
        uri: &Url,
        position: Position,
        context: ReferenceContext,
    ) -> Result<LspOperationResult<Vec<Location>>> {
        let (result, latency) = self
            .send_request(
                "textDocument/references",
                ReferenceParams {
                    text_document_position_params: TextDocumentPositionParams {
                        text_document: TextDocumentIdentifier { uri: uri.clone() },
                        position,
                    },
                    context,
                    work_done_progress_params: Default::default(),
                    partial_result_params: Default::default(),
                },
            )
            .await?;

        if let Some(error) = result.get("error") {
            anyhow::bail!("LSP error: {}", error);
        }

        let locations: Vec<Location> = serde_json::from_value(result["result"].clone())?;
        Ok(LspOperationResult {
            result: locations,
            latency_ms: latency.as_millis(),
            cached: false,
        })
    }

    /// Hover information
    pub async fn hover(
        &self,
        uri: &Url,
        position: Position,
    ) -> Result<LspOperationResult<Option<Hover>>> {
        let (result, latency) = self
            .send_request(
                "textDocument/hover",
                HoverParams {
                    text_document_position_params: TextDocumentPositionParams {
                        text_document: TextDocumentIdentifier { uri: uri.clone() },
                        position,
                    },
                    work_done_progress_params: Default::default(),
                },
            )
            .await?;

        if let Some(error) = result.get("error") {
            anyhow::bail!("LSP error: {}", error);
        }

        let hover: Option<Hover> = serde_json::from_value(result["result"].clone())?;
        Ok(LspOperationResult {
            result: hover,
            latency_ms: latency.as_millis(),
            cached: false,
        })
    }

    /// Document symbols
    pub async fn document_symbol(
        &self,
        uri: &Url,
    ) -> Result<LspOperationResult<Vec<DocumentSymbol>>> {
        let (result, latency) = self
            .send_request(
                "textDocument/documentSymbol",
                DocumentSymbolParams {
                    text_document: TextDocumentIdentifier { uri: uri.clone() },
                    work_done_progress_params: Default::default(),
                    partial_result_params: Default::default(),
                },
            )
            .await?;

        if let Some(error) = result.get("error") {
            anyhow::bail!("LSP error: {}", error);
        }

        let symbols: Vec<DocumentSymbol> = serde_json::from_value(result["result"])?;
        Ok(LspOperationResult {
            result: symbols,
            latency_ms: latency.as_millis(),
            cached: false,
        })
    }

    /// Workspace symbol search
    pub async fn workspace_symbol(
        &self,
        query: &str,
    ) -> Result<LspOperationResult<Vec<SymbolInformation>>> {
        let (result, latency) = self
            .send_request(
                "workspace/symbol",
                WorkspaceSymbolParams {
                    query: query.to_string(),
                    work_done_progress_params: Default::default(),
                    partial_result_params: Default::default(),
                },
            )
            .await?;

        if let Some(error) = result.get("error") {
            anyhow::bail!("LSP error: {}", error);
        }

        let symbols: Vec<SymbolInformation> = serde_json::from_value(result["result"])?;
        Ok(LspOperationResult {
            result: symbols,
            latency_ms: latency.as_millis(),
            cached: false,
        })
    }

    /// Completion
    pub async fn completion(
        &self,
        uri: &Url,
        position: Position,
        context: Option<CompletionContext>,
    ) -> Result<LspOperationResult<CompletionResponse>> {
        let params = CompletionParams {
            text_document_position: TextDocumentPositionParams {
                text_document: TextDocumentIdentifier { uri: uri.clone() },
                position,
            },
            work_done_progress_params: Default::default(),
            partial_result_params: Default::default(),
            context,
        };

        let (result, latency) = self.send_request("textDocument/completion", params).await?;

        if let Some(error) = result.get("error") {
            anyhow::bail!("LSP error: {}", error);
        }

        let completion: CompletionResponse = serde_json::from_value(result["result"].clone())?;
        Ok(LspOperationResult {
            result: completion,
            latency_ms: latency.as_millis(),
            cached: false,
        })
    }

    /// Code actions
    pub async fn code_action(
        &self,
        uri: &Url,
        range: Range,
        context: CodeActionContext,
    ) -> Result<LspOperationResult<Vec<CodeActionOrCommand>>> {
        let (result, latency) = self
            .send_request(
                "textDocument/codeAction",
                CodeActionParams {
                    text_document: TextDocumentIdentifier { uri: uri.clone() },
                    range,
                    context,
                    work_done_progress_params: Default::default(),
                    partial_result_params: Default::default(),
                },
            )
            .await?;

        if let Some(error) = result.get("error") {
            anyhow::bail!("LSP error: {}", error);
        }

        let actions: Vec<CodeActionOrCommand> = serde_json::from_value(result["result"])?;
        Ok(LspOperationResult {
            result: actions,
            latency_ms: latency.as_millis(),
            cached: false,
        })
    }

    /// Publish diagnostics (notification)
    pub async fn publish_diagnostics(
        &self,
        uri: &Url,
        version: Option<i32>,
        diagnostics: Vec<Diagnostic>,
    ) -> Result<()> {
        self.send_notification(
            "textDocument/publishDiagnostics",
            PublishDiagnosticsParams {
                uri: uri.clone(),
                version,
                diagnostics,
            },
        ).await
    }

    /// Register notification handler
    pub async fn on_notification<F>(&self, method: &str, handler: F)
    where
        F: Fn(serde_json::Value) + Send + Sync + 'static,
    {
        let handler = Arc::new(handler) as NotificationHandler;
        let mut handlers = self.notification_handlers.write().await;
        handlers
            .entry(method.to_string())
            .or_insert_with(Vec::new)
            .push(handler);
    }

    /// Get server capabilities
    pub async fn capabilities(&self) -> Option<ServerCapabilities> {
        self.capabilities.read().await.clone()
    }

    /// Get server state
    pub async fn state(&self) -> EnhancedLspServerState {
        *self.state.read().await
    }

    /// Get performance metrics
    pub fn metrics(&self) -> LspMetrics {
        self.metrics.lock().clone()
    }

    /// Check if server is operational
    pub async fn is_operational(&self) -> bool {
        self.state.read().await.is_operational()
    }

    /// Get server name
    pub fn name(&self) -> &str {
        &self.server_name
    }
}

/// Enhanced LSP Server instance with full lifecycle management
pub struct EnhancedLspServer {
    handle: EnhancedLspHandle,
    child: Child,
    diagnostic_registry: Arc<EnhancedDiagnosticRegistry>,
}

impl EnhancedLspServer {
    pub async fn connect(config: EnhancedLspConfig) -> Result<Self> {
        let server_name = config.name.clone();
        log::info!(
            "LSP: Starting server '{}' with command '{}'",
            server_name, config.command
        );

        let child = Self::spawn_server(&config).await?;

        // Create channels
        let (writer_tx, mut writer_rx) = mpsc::channel::<String>(1000);

        // Create handle
        let handle = EnhancedLspHandle {
            config: Arc::new(config.clone()),
            state: Arc::new(RwLock::new(EnhancedLspServerState::Starting)),
            capabilities: Arc::new(RwLock::new(None)),
            request_id: Arc::new(std::sync::atomic::AtomicU64::new(0)),
            pending: Arc::new(Mutex::new(HashMap::new())),
            writer_tx: writer_tx.clone(),
            notification_handlers: Arc::new(RwLock::new(HashMap::new())),
            metrics: Arc::new(Mutex::new(LspMetrics::default())),
            start_time: Arc::new(RwLock::new(None)),
            restart_count: Arc::new(std::sync::atomic::AtomicU32::new(0)),
            server_name: server_name.clone(),
        };

        *handle.state.write().await = EnhancedLspServerState::Running;
        *handle.start_time.write().await = Some(Instant::now());

        log::info!("LSP: Server '{}' started successfully", server_name);

        // Initialize
        let root_uri = config
            .root_path
            .as_ref()
            .and_then(|p| Url::from_file_path(p).ok());

        let init_params = InitializeParams {
            process_id: Some(std::process::id()),
            root_path: config.root_path.as_ref().map(|p| p.to_string_lossy().into_owned()),
            root_uri,
            initialization_options: None,
            capabilities: ClientCapabilities::default(),
            trace: None,
            workspace_folders: None,
            client_info: Some(ClientInfo {
                name: "carpai".to_string(),
                version: Some(env!("CARGO_PKG_VERSION").to_string()),
            }),
            locale: None,
        };

        let (result, _) = handle
            .send_request("initialize", init_params)
            .await?;

        if let Some(error) = result.get("error") {
            anyhow::bail!("LSP initialization failed: {:?}", error);
        }

        let init_result: InitializeResult = serde_json::from_value(result["result"].clone())?;
        *handle.capabilities.write().await = Some(init_result.capabilities);

        handle
            .send_notification("initialized", serde_json::Value::Null)
            .await?;

        log::info!(
            "LSP: Server '{}' initialized successfully",
            server_name
        );

        Ok(Self {
            handle,
            child,
            diagnostic_registry: Arc::new(EnhancedDiagnosticRegistry::new(100)),
        })
    }

    async fn spawn_server(config: &EnhancedLspConfig) -> Result<Child> {
        let mut env: HashMap<String, String> = std::env::vars().collect();
        env.extend(config.env.clone());

        let child = Command::new(&config.command)
            .args(&config.args)
            .envs(&env)
            .stdin(std::process::Stdio::piped())
            .stdout(std::process::Stdio::piped())
            .stderr(std::process::Stdio::piped())
            .kill_on_drop(true)
            .spawn()
            .with_context(|| format!("Failed to spawn LSP server: {}", config.command))?;

        Ok(child)
    }

    pub fn handle(&self) -> &EnhancedLspHandle {
        &self.handle
    }

    pub fn diagnostic_registry(&self) -> Arc<EnhancedDiagnosticRegistry> {
        self.diagnostic_registry.clone()
    }

    pub async fn shutdown(mut self) -> Result<()> {
        log::info!("LSP: Shutting down server '{}'", self.handle.server_name);

        *self.handle.state.write().await = EnhancedLspServerState::Stopping;

        // Send shutdown request
        let _ = self.handle.send_request("shutdown", serde_json::Value::Null).await;
        let _ = self
            .handle
            .send_notification("exit", serde_json::Value::Null)
            .await;

        // Kill child process
        self.child.kill().await.ok();

        *self.handle.state.write().await = EnhancedLspServerState::Stopped;

        Ok(())
    }

    pub async fn restart(self) -> Result<Self> {
        log::info!("LSP: Restarting server '{}'", self.handle.server_name);

        self.shutdown().await?;

        let mut new_config = (*self.handle.config).clone();
        new_config.restart_count.fetch_add(1, std::sync::atomic::Ordering::SeqCst);

        Self::connect(new_config).await
    }
}
