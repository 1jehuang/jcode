//! LSP 语言服务客户端
//!
//! 移植自 Claude Code:
//! - `src/services/lsp/LSPClient.ts` (14KB) - LSP 客户端封装
//! - `src/services/lsp/LSPServerManager.ts` (13KB) - 多服务器实例管理
//! - `src/services/lsp/LSPServerInstance.ts` (16KB) - 单服务器生命周期
//! - `src/tools/LSPTool/LSPTool.ts` (25KB) - LSP 工具暴露给 AI
//!
//! 设计模式 (来自 Claude Code):
//! - **工厂函数模式** (`createLSPClient()`): 用闭包替代 class, 实现状态封装
//! - **延迟初始化**: Handler 注册在连接建立前排队, 连接后批量应用
//! - **扩展名路由**: 根据文件扩展名自动选择合适的 LSP 服务器

use crate::types::{LspDiagnostic, LspSeverity, LspReference, LspStartOptions};
use anyhow::{Context, Result};
use async_trait::async_trait;
use lsp_types::*;
use serde_json::Value as JsonValue;
use std::collections::HashMap;
use std::path::PathBuf;
use std::sync::Arc;
use tokio::io::{AsyncRead, AsyncWrite};
use tokio::process::Child;
use tower_lsp::lsp_types;
use tracing::{debug, error, info, warn};

// ============================================================================
// LSP Client Trait 定义 - 对应 Claude Code LSPClient 接口
// ============================================================================

/// LSP 客户端 trait
/// 移植自 Claude Code `LSPClient.ts:21-41`:
/// ```typescript
/// export type LSPClient = {
///   capabilities: ServerCapabilities
///   isInitialized: boolean
///   start(command, args, options?): Promise<void>
///   initialize(params): Promise<InitializeResult>
///   sendRequest<T>(method, params): Promise<T>
///   sendNotification(method, params): Promise<void>
///   onNotification(method, handler): void
///   onRequest<TParams, TResult>(method, handler): void
///   stop(): Promise<void>
/// }
/// ```
#[async_trait]
pub trait LspClient: Send + Sync {
    /// 获取服务器能力声明
    fn capabilities(&self) -> Option<ServerCapabilities>;

    /// 是否已完成初始化
    fn is_initialized(&self) -> bool;

    /// 启动 LSP 服务器进程
    async fn start(
        &mut self,
        command: &str,
        args: &[String],
        options: Option<LspStartOptions>,
    ) -> Result<()>;

    /// 发送 initialize 请求
    async fn initialize(&self, params: InitializeParams) -> Result<InitializeResult>;

    /// 发送通用请求
    async fn send_request<T: serde::de::DeserializeOwned>(
        &self,
        method: &str,
        params: impl Into<JsonValue>,
    ) -> Result<T>;

    /// 发送通知 (无需响应)
    async fn send_notification(
        &self,
        method: &str,
        params: impl Into<JsonValue>,
    ) -> Result<()>;

    /// 注册通知处理器
    fn on_notification(
        &self,
        method: &str,
        handler: Box<dyn Fn(JsonValue) + Send + Sync>,
    );

    /// 注册请求处理器 (需要返回值)
    fn on_request<TParams, TResult>(
        &self,
        method: &str,
        handler: Box<dyn Fn(TParams) -> TResult + Send + Sync>,
    ) where
        TParams: serde::de::DeserializeOwned + Send + 'static,
        TResult: serde::Serialize + Send + 'static;

    /// 停止 LSP 服务器
    async fn stop(&mut self) -> Result<()>;
}

// ============================================================================
// 进程型 LSP 客户端实现 - 基于 stdio 通信
// ============================================================================

type NotificationHandler = Box<dyn Fn(JsonValue) + Send + Sync>;
type RequestHandlerInner = Box<dyn erased_serde::Serialize + Send + Sync>;

/// 基于 JSON-RPC over stdio 的 LSP 客户端实现
/// 
/// 移植自 Claude Code `createLSPClient()` 工厂函数:
/// - 闭包状态管理模式
/// - Handler 延迟排队机制
/// - 进程崩溃回调
///
/// ## 架构
/// ```text
/// +------------------------------+
///     StdioLspClient              |
/// +------------------------------+
///  process: Child (stdio pipe)    | <- LSP server 进程
///  connection: jsonrpc Connection |
///  capabilities: ServerCaps       |
///  pending_handlers: Vec          | <- 延迟注册队列
///  crash_callback: Option         |
/// +------------------------------+
/// ```
pub struct StdioLspClient {
    /// LSP 服务器子进程
    process: Option<Child>,

    /// 服务器能力声明 (initialize 后填充)
    capabilities: Option<ServerCapabilities>,

    /// 是否已初始化
    initialized: bool,

    /// 启动是否失败 (防止重用崩溃的客户端)
    start_failed: bool,

    /// 启动失败错误信息
    start_error: Option<anyhow::Error>,

    /// 是否正在执行 intentional shutdown
    is_stopping: bool,

    /// 服务器名称 (用于日志)
    server_name: String,

    /// 延迟排队的通知 handlers (初始化前注册的)
    pending_notification_handlers: Vec<(String, NotificationHandler)>,

    /// 延迟排队的 request handlers
    pending_request_handlers: Vec<(String, RequestHandlerInner)>,

    /// 已激活的通知 handlers
    active_notification_handlers:
        HashMap<String, Vec<Box<dyn Fn(JsonValue) + Send + Sync>>>,

    /// 崩溃回调
    on_crash: Option<Box<dyn Fn(anyhow::Error) + Send + Sync>>,
}

impl StdioLspClient {
    /// 创建新的 stdio LSP 客户端实例
    /// 
    /// 对应 Claude Code: `export function createLSPClient(serverName, onCrash?)`
    pub fn new(server_name: &str) -> Self {
        Self {
            process: None,
            capabilities: None,
            initialized: false,
            start_failed: false,
            start_error: None,
            is_stopping: false,
            server_name: server_name.to_string(),
            pending_notification_handlers: Vec::new(),
            pending_request_handlers: Vec::new(),
            active_notification_handlers: HashMap::new(),
            on_crash: None,
        }
    }

    /// 设置进程崩溃回调
    /// 
    /// Claude Code 注释: "Called when the server process exits unexpectedly 
    /// (non-zero exit code during operation, not during intentional stop)"
    pub fn on_crash<F>(mut self, callback: F) -> Self
    where
        F: Fn(anyhow::Error) + Send + Sync + 'static,
    {
        self.on_crash = Some(Box::new(callback));
        self
    }

    /// 检查启动状态 (对应 Claude Code checkStartFailed)
    fn check_start_failed(&self) -> Result<()> {
        if self.start_failed {
            Err(self
                .start_error
                .clone()
                .unwrap_or_else(|| anyhow::anyhow!("LSP server {} failed to start", self.server_name)))
        } else {
            Ok(())
        }
    }

    /// 将延迟注册的 handlers 应用到活跃集合
    fn flush_pending_handlers(&mut self) {
        // 刷新通知 handlers
        for (method, handler) in self.pending_notification_handlers.drain(..) {
            self.active_notification_handlers
                .entry(method)
                .or_default()
                .push(handler);
        }

        // 刷新请求 handlers
        self.pending_request_handlers.clear();
    }
}

#[async_trait]
impl LspClient for StdioLspClient {
    fn capabilities(&self) -> Option<ServerCapabilities> {
        self.capabilities.clone()
    }

    fn is_initialized(&self) -> bool {
        self.initialized
    }

    async fn start(
        &mut self,
        command: &str,
        args: &[String],
        options: Option<LspStartOptions>,
    ) -> Result<()> {
        self.check_start_failed()?;

        info!("Starting LSP server: {} with args {:?}", command, args);

        // 构建 spawn 环境
        let mut spawn_env = std::env::vars().collect::<HashMap<String, String>>();
        if let Some(opts) = &options {
            if let Some(env) = &opts.env {
                spawn_env.extend(env.clone());
            }
        }

        // Spawn LSP 服务器进程
        // Claude Code 关键注释:
        // "spawn() returns immediately, but the 'error' event fires asynchronously.
        // If we use streams before confirming spawn succeeded, we get unhandled promise rejections."
        let child = tokio::process::Command::new(command)
            .args(args)
            .stdin(std::process::Stdio::piped())
            .stdout(std::process::Stdio::piped())
            .stderr(std::process::Stdio::piped()) // 捕获 stderr 用于日志
            .current_dir(if let Some(ref opts) = options {
                opts.cwd.clone().unwrap_or_else(|| std::env::current_dir().unwrap_or_default())
            } else {
                std::env::current_dir().unwrap_or_default()
            })
            .envs(&spawn_env)
            // Windows 上隐藏控制台窗口
            .creation_flags(cfg_if::cfg_if! {
                if #[cfg(windows)] {
                    0x08000000 // CREATE_NO_WINDOW
                } else {
                    0
                }
            })
            .spawn()
            .with_context(|| format!("Failed to spawn LSP server: {}", command))?;

        // 验证进程成功启动
        // Claude Code: 等待一小段时间确认 spawn 成功
        tokio::time::sleep(tokio::time::Duration::from_millis(100)).await;

        // 检查进程是否仍在运行 (ENOENT 等错误会在此处体现)
        match child.try_wait() {
            Ok(Some(status)) => {
                let err = anyhow::anyhow!(
                    "LSP server {} exited immediately with status {}",
                    self.server_name,
                    status
                );
                self.start_failed = true;
                self.start_error = Some(err.clone());
                return Err(err);
            }
            Ok(None) => {
                // 进程仍在运行, 正常
            }
            Err(e) => {
                // 无法获取状态, 但进程可能正常启动
                warn!("Could not query LSP process status: {}", e);
            }
        }

        self.process = Some(child);
        info!("LSP server {} spawned successfully", self.server_name);
        Ok(())
    }

    async fn initialize(&self, params: InitializeParams) -> Result<InitializeResult> {
        self.check_start_failed()?;
        
        info!("Initializing LSP server: {}", self.server_name);
        
        // TODO: 实现实际的 JSON-RPC initialize 请求
        // 这需要建立基于 stdin/stdout 的 jsonrpc 通信层
        // Claude Code 使用 vscode-jsonrpc 库
        
        // 返回模拟的能力声明
        // 实际实现应从服务器的 InitializeResult 中获取
        let result = InitializeResult {
            capabilities: ServerCapabilities {
                text_document_sync: Some(TextDocumentSyncCapability::Kind(TextDocumentSyncKind::INCREMENTAL)),
                completion_provider: Some(CompletionOptions {
                    resolve_provider: Some(true),
                    trigger_characters: Some(vec![".".to_string(), ":".to_string(), "<".to_string()]),
                    ..Default::default()
                }),
                hover_provider: Some(HoverCapability::Simple(true)),
                definition_provider: Some(OneOf::Left(true)),
                references_provider: Some(OneOf::Left(true)),
                document_symbol_provider: Some(OneOf::Left(true)),
                workspace_symbol_provider: Some(OneOf::Left(true)),
                code_action_provider: Some(CodeActionCapability::Simple(CodeActionOptions {
                    code_action_kinds: Some(vec![
                        CodeActionKind::QUICKFIX,
                        CodeActionKind::REFACTOR,
                        CodeActionKind::SOURCE_FIX_ALL,
                    ]),
                    ..Default::default()
                })),
                diagnostic_provider: Some(DiagnosticRegistrationOptions {
                    identifier: Some("jcode".to_string()),
                    inter_file_dependencies: Some(true),
                    workspace_diagnostics: Some(true),
                    ..Default::default()
                }),
                ..Default::default()
            },
            server_info: Some(ServerInfo {
                name: self.server_name.clone(),
                version: Some("1.0.0".to_string()),
            }),
        };

        // 标记为已初始化, 并刷新延迟 handlers
        // 注意: 由于 Rust borrow checker, 这里无法直接修改 self
        // 实际实现中应该使用 Arc<Mutex<Self>> 包装
        
        info!("LSP server {} initialized successfully", self.server_name);
        Ok(result)
    }

    async fn send_request<T: serde::de::DeserializeOwned>(
        &self,
        method: &str,
        params: impl Into<JsonValue>,
    ) -> Result<T> {
        self.check_start_failed()?;

        debug!("LSP request -> {}: {}", self.server_name, method);
        
        // TODO: 实际发送 JSON-RPC 请求到 stdin
        // 并从 stdout 读取响应
        
        // 返回类型占位符
        Err(anyhow::anyhow!("send_request not yet fully implemented"))
    }

    async fn send_notification(
        &self,
        method: &str,
        params: impl Into<JsonValue>,
    ) -> Result<()> {
        self.check_start_failed()?;

        debug!("LSP notification -> {}: {}", self.server_name, method);
        
        // TODO: 实际发送通知
        Ok(())
    }

    fn on_notification(
        &self,
        method: &str,
        handler: Box<dyn Fn(JsonValue) + Send + Sync>,
    ) {
        // 如果尚未初始化, 加入延迟队列 (Claude Code 设计)
        if !self.initialized {
            // 注意: 需要 &mut self, 这里简化处理
            warn!("Notification handler registered before initialization for {}", method);
        }
        // 否则直接加入活跃集合
        // self.active_notification_handlers.entry(method).or_default().push(handler);
    }

    fn on_request<TParams, TResult>(
        &self,
        method: &str,
        handler: Box<dyn Fn(TParams) -> TResult + Send + Sync>,
    )
    where
        TParams: serde::de::DeserializeOwned + Send + 'static,
        TResult: serde::Serialize + Send + 'static,
    {
        debug!("Registering LSP request handler: {}", method);
        // TODO: 类型擦除存储 handler
    }

    async fn stop(&mut self) -> Result<()> {
        if let Some(mut child) = self.process.take() {
            self.is_stopping = true;

            info!("Stopping LSP server: {}", self.server_name);
            
            // 发送 shutdown 请求
            if let Err(e) = self.send_notification("shutdown", serde_json::json!(null)).await {
                warn!("Failed to send shutdown notification: {}", e);
            }
            
            // 发送 exit 通知
            if let Err(e) = self.send_notification("exit", serde_json::json!(null)).await {
                warn!("Failed to send exit notification: {}", e);
            }

            // 等待进程退出 (最多 5 秒)
            match tokio::time::timeout(
                std::time::Duration::from_secs(5),
                child.wait(),
            ).await {
                Ok(Ok(status)) => {
                    info!("LSP server {} exited with status: {}", self.server_name, status);
                }
                Ok(Err(e)) => {
                    warn!("Error waiting for LSP server exit: {}", e);
                }
                Err(_) => {
                    warn!("LPS server {} did not exit gracefully, killing...", self.server_name);
                    child.kill().ok();
                }
            }
        }

        self.process = None;
        self.capabilities = None;
        self.initialized = false;
        self.is_stopping = false;
        Ok(())
    }
}

// ============================================================================
// LSP 服务器管理器 - 多实例管理
// ============================================================================

/// LSP 服务器注册条目
#[derive(Debug, Clone)]
pub struct LspServerEntry {
    /// 服务器唯一标识
    pub id: String,
    
    /// 显示名称
    pub name: String,
    
    /// 启动命令
    pub command: String,
    
    /// 启动参数模板
    pub args_template: Vec<String>,
    
    /// 支持的文件扩展名
    pub extensions: Vec<String>,

    /// 是否按需启动 (懒加载)
    pub lazy_start: bool,
}

/// LSP 服务器管理器
/// 
/// 移植自 Claude Code `LSPServerManager.ts`:
/// - 管理多个 LSP 服务器实例
/// - 按文件扩展名路由到正确的服务器
/// - 懒加载 + 生命周期管理
pub struct LspServerManager {
    /// 已注册的服务器
    servers: HashMap<String, LspServerEntry>,
    
    /// 活跃的服务器客户端实例
    active_clients: HashMap<String, Arc<std::sync::Mutex<Box<dyn LspClient>>>>,
    
    /// 扩展名 -> 服务器 ID 映射
    extension_map: HashMap<String, String>,
}

impl LspServerManager {
    /// 创建新的 LSP 服务器管理器
    pub fn new() -> Self {
        Self {
            servers: HashMap::new(),
            active_clients: HashMap::new(),
            extension_map: HashMap::new(),
        }
    }

    /// 注册一个 LSP 服务器
    pub fn register_server(&mut self, entry: LspServerEntry) {
        // 构建扩展名映射
        for ext in &entry.extensions {
            self.extension_map.insert(ext.to_lowercase(), entry.id.clone());
        }
        self.servers.insert(entry.id.clone(), entry);
        info!("Registered LSP server: {} (extensions: {:?})", entry.name, entry.extensions);
    }

    /// 根据文件路径获取或创建对应的 LSP 客户端
    /// 
    /// 这是核心路由方法 - 根据扩展名自动选择合适的 LSP 服务器
    pub async fn get_client_for_file(&mut self, file_path: &PathBuf) -> Result<Arc<std::sync::Mutex<Box<dyn LspClient>>>> {
        // 从文件路径提取扩展名
        let extension = file_path
            .extension()
            .and_then(|e| e.to_str())
            .unwrap_or("")
            .to_lowercase();

        if extension.is_empty() {
            return Err(anyhow::anyhow!("File has no extension: {:?}", file_path));
        }

        // 查找对应的服务器 ID
        let server_id = self.extension_map.get(&extension).cloned().ok_or_else(|| {
            anyhow::anyhow!("No LSP server registered for extension: .{}", extension)
        })?;

        // 返回已有或创建新实例
        if let Some(client) = self.active_clients.get(&server_id) {
            Ok(client.clone())
        } else {
            self.start_server(&server_id).await
        }
    }

    /// 启动指定服务器并返回客户端
    async fn start_server(&mut self, server_id: &str) -> Result<Arc<std::sync::Mutex<Box<dyn LspClient>>>> {
        let entry = self.servers.get(server_id).cloned().ok_or_else(|| {
            anyhow::anyhow!("Unknown LSP server: {}", server_id)
        })?;

        info!("Starting LSP server: {} ({})", entry.name, server_id);

        let mut client = Box::new(StdioLspClient::new(&entry.name)) as Box<dyn LspClient>;
        
        client.start(
            &entry.command,
            &entry.args_template,
            None, // TODO: 从配置传入 options
        ).await?;

        // 执行 initialize
        let init_params = InitializeParams {
            process_id: Some(std::process::id()),
            root_uri: Some(Url::from_file_path(std::env::current_dir().unwrap_or_default()).unwrap()),
            capabilities: ClientCapabilities {
                text_document: Some(TextDocumentClientCapabilities {
                    completion: Some(CompletionClientCapabilities {
                        completion_item: Some(CompletionClientCapabilitiesCompletionItem {
                            snippet_support: Some(true),
                            documentation_format: Some(MarkupKind::Markdown),
                            ..Default::default()
                        }),
                        ..Default::default()
                    }),
                    ..Default::default()
                }),
                window: Some(WindowClientCapabilities {
                    work_done_progress: Some(true),
                    ..Default::default()
                }),
                ..Default::default()
            },
            ..Default::default()
        };

        client.initialize(init_params).await?;

        let client_arc = Arc::new(std::sync::Mutex::new(client));
        self.active_clients.insert(server_id.to_string(), client_arc.clone());

        Ok(client_arc)
    }

    /// 获取文件的诊断信息
    /// 
    /// 移植自 Claude Code LSP 功能: diagnostics/references/symbols
    pub async fn get_diagnostics(&self, _file_path: &PathBuf) -> Result<Vec<LspDiagnostic>> {
        // TODO: 调用 textDocument/diagnostic 请求
        // 返回格式化的诊断结果
        Ok(vec![])
    }

    /// 获取符号引用位置
    pub async fn get_references(
        &self, 
        _file_path: &PathBuf, 
        _line: u32, 
        _column: u32,
    ) -> Result<Vec<LspReference>> {
        // TODO: 调用 textDocument/references 请求
        Ok(vec![])
    }

    /// 获取文档符号
    pub async fn get_document_symbols(&self, _file_path: &PathBuf) -> Result<Vec<DocumentSymbol>> {
        // TODO: 调用 textDocument/documentSymbol 请求
        Ok(vec![])
    }

    /// 停止所有活跃的 LSP 服务器
    pub async fn stop_all(&mut self) -> Result<()> {
        for (id, client) in &mut self.active_clients {
            info!("Stopping LSP server: {}", id);
            let mut guard = client.lock().await;
            if let Err(e) = guard.stop().await {
                warn!("Error stopping LSP server {}: {}", id, e);
            }
        }
        self.active_clients.clear();
        Ok(())
    }

    /// 获取已注册的服务器数量
    pub fn server_count(&self) -> usize {
        self.servers.len()
    }

    /// 获取活跃的客户端数量
    pub fn active_count(&self) -> usize {
        self.active_clients.len()
    }
}

impl Default for LspServerManager {
    fn default() -> Self {
        Self::new()
    }
}
