// ════════════════════════════════════════════════════════════════
// LSP Client — JSON-RPC 通信 + 文档同步
// ════════════════════════════════════════════════════════════════

use lsp_types::*;
use serde_json::{json, Value};
use std::collections::HashMap;
use std::sync::atomic::{AtomicU64, Ordering};

use crate::server_manager::LanguageId;

/// 通用 LSP 结果类型
pub type LspResult<T> = Result<T, LspError>;

#[derive(Debug, thiserror::Error)]
pub enum LspError {
    #[error("IO error: {0}")]
    Io(#[from] std::io::Error),
    #[error("JSON error: {0}")]
    Json(#[from] serde_json::Error),
    #[error("Server error: code={code}, message='{message}'")]
    Server { code: i32, message: String },
    #[error("Request timeout")]
    Timeout,
    #[error("Not initialized")]
    NotInitialized,
    #[error("No server running")]
    NoServer,
}

/// JSON-RPC ID (自增)
static NEXT_ID: AtomicU64 = AtomicU64::new(1);

fn next_id() -> u64 {
    NEXT_ID.fetch_add(1, Ordering::SeqCst)
}

/// 单个 LSP Server 的客户端
pub struct LspClient {
    /// Server 进程
    process: Option<tokio::process::Child>,
    
    /// stdin/stdout
    // Note: In a full implementation these would be actual handles
    server_name: String,
    root_uri: Url,
    initialized: bool,
    open_documents: HashMap<Url, i32>, // document version tracking
    capabilities: Option<ServerCapabilities>,
}

impl LspClient {
    pub fn new(server_name: String) -> Self {
        Self {
            process: None,
            server_name,
            root_uri: Url::parse("file:///workspace").unwrap(),
            initialized: false,
            open_documents: HashMap::new(),
            capabilities: None,
        }
    }

    /// 初始化 LSP Server
    pub async fn initialize(&mut self) -> LspResult<InitializeResult> {
        if self.initialized {
            return Ok(InitializeResult {
                capabilities: self.capabilities.clone().unwrap_or_default(),
                server_info: Some(ServerInfo {
                    name: self.server_name.clone(),
                    version: Some("0.0.0".into()),
                }),
            });
        }

        let _id = next_id();

        let params = InitializeParams {
            process_id: None,
            root_uri: Some(self.root_uri.clone()),
            initialization_options: None,
            capabilities: ClientCapabilities::default(),
            trace: None,
            workspace_folders: None,
            client_info: Some(ClientInfo { name: "jcode-lsp".into(), version: Some("0.1.0".into()) }),
            locale: Some("zh-CN".into()),
            work_done_progress_params: Default::default(),
            ..Default::default()
        };

        // 发送 initialize request
        let result: InitializeResult = self.send_request("initialize", json!(params)).await?;

        // 发送 initialized notification
        self.send_notification("initialized", json!({})).await;

        self.capabilities = Some(result.capabilities.clone());
        self.initialized = true;

        tracing::info!(
            server = %self.server_name,
            "LSP server initialized"
        );

        Ok(result)
    }

    /// 打开文档 (通知 Server 开始追踪)
    pub async fn open_document(&mut self, uri: &str, language_id: &str, content: &str) -> LspResult<()> {
        let url = Url::parse(uri).map_err(|e| LspError::Server { code: -32600, message: format!("Invalid URI: {}", e) })?;
        
        let params = DidOpenTextDocumentParams {
            text_document: TextDocumentItem {
                uri: url.clone(),
                language_id: language_id.to_string(),
                version: 1,
                text: content.to_string(),
            },
        };

        self.open_documents.insert(url, 1);
        self.send_notification("textDocument/didOpen", json!(params)).await
    }

    /// 更新文档内容 (增量或全量)
    pub async fn update_document(&mut self, uri: &str, content: &str) -> LspResult<()> {
        let url = Url::parse(uri).ok();
        
        if let Some(url) = url {
            let new_version = self.open_documents.get(&url).copied().unwrap_or(0) + 1;

            let params = DidChangeTextDocumentParams {
                text_document: VersionedTextDocumentIdentifier {
                    uri: url.clone(),
                    version: new_version,
                },
                content_changes: vec![TextDocumentContentChangeEvent {
                    range: None,
                    range_length: None,
                    text: content.to_string(),
                }],
            };

            self.open_documents.insert(url, new_version);
            self.send_notification("textDocument/didChange", json!(params)).await
        } else {
            Err(LspError::Server { code: -32600, message: "Invalid URI".into() })
        }
    }

    // ─── 核心功能方法 ──────────────────────────────

    /// 跳转到定义
    pub async fn goto_definition(
        &self,
        file: &str,
        line: u32,
        character: u32,
    ) -> LspResult<Vec<Location>> {
        let params = GotoDefinitionParams {
            text_document_position_params: TextDocumentPositionParams {
                text_document: TextDocumentIdentifier { uri: Url::parse(file).unwrap() },
                position: Position::new(line, character),
            },
            work_done_progress_params: Default::default(),
            partial_result_params: Default::default(),
        };

        let response: Value = self.send_request("textDocument/definition", json!(params)).await?;
        
        match response {
            Value::Array(locations) => {
                let locs: Vec<Location> = locations.into_iter()
                    .filter_map(|v| serde_json::from_value(v).ok())
                    .collect();
                Ok(locs)
            }
            Value::Null => Ok(vec![]),
            other => {
                // Single location
                match serde_json::from_value(other) {
                    Ok(loc) => Ok(vec![loc]),
                    Err(_) => Ok(vec![]), // NullLink
                }
            }
        }
    }

    /// 查找所有引用
    pub async fn find_references(
        &self,
        file: &str,
        line: u32,
        character: u32,
    ) -> LspResult<Vec<Location>> {
        let params = ReferenceParams {
            text_document_position: TextDocumentPositionParams {
                text_document: TextDocumentIdentifier { uri: Url::parse(file).unwrap() },
                position: Position::new(line, character),
            },
            context: ReferenceContext { include_declaration: true },
            work_done_progress_params: Default::default(),
            partial_result_params: Default::default(),
        };

        let response: Value = self.send_request("textDocument/references", json!(params)).await?;
        
        match response {
            Value::Array(locations) => {
                let locs: Vec<Location> = locations.into_iter()
                    .filter_map(|v| serde_json::from_value(v).ok())
                    .collect();
                Ok(locs)
            }
            _ => Ok(vec![])
        }
    }

    /// 获取诊断信息
    pub async fn get_diagnostics(&self, _file: &str) -> LspResult<Vec<Diagnostic>> {
        // Diagnostics are pushed from server via textDocument/publishDiagnostics
        // Here we return cached diagnostics or trigger a refresh
        // For now, return empty as real implementation needs diagnostic cache
        Ok(vec![])
    }

    /// 获取补全建议
    pub async fn get_completion(
        &self,
        file: &str,
        line: u32,
        character: u32,
    ) -> LspResult<Vec<CompletionItem>> {
        let params = CompletionParams {
            text_document_position: TextDocumentPositionParams {
                text_document: TextDocumentIdentifier { uri: Url::parse(file).unwrap() },
                position: Position::new(line, character),
            },
            work_done_progress_params: Default::default(),
            partial_result_params: Default::default(),
            context: None,
        };

        let response: Value = self.send_request("textDocument/completion", json!(params)).await?;
        
        match response {
            Value::Object(obj) => {
                if let Some(items) = obj.get("items") {
                    let items: Vec<CompletionItem> = serde_json::from_value(items.clone())?;
                    Ok(items)
                } else {
                    Ok(vec![])
                }
            }
            _ => Ok(vec![])
        }
    }

    /// 获取悬停文档
    pub async fn hover(
        &self,
        file: &str,
        line: u32,
        character: u32,
    ) -> LspResult<Option<Hover>> {
        let params: HoverParams = HoverParams {
            text_document_position_params: TextDocumentPositionParams {
                text_document: TextDocumentIdentifier { uri: Url::parse(file).unwrap() },
                position: Position::new(line, character),
            },
            work_done_progress_params: Default::default(),
        };

        let response: Value = self.send_request("textDocument/hover", json!(params)).await?;
        
        match response {
            Value::Null => Ok(None),
            other => {
                let hover: Hover = serde_json::from_value(other)?;
                Ok(Some(hover))
            }
        }
    }

    /// 重命名符号
    pub async fn rename_symbol(
        &self,
        file: &str,
        line: u32,
        character: u32,
        new_name: &str,
    ) -> LspResult<WorkspaceEdit> {
        let params = RenameParams {
            text_document_position: TextDocumentPositionParams {
                text_document: TextDocumentIdentifier { uri: Url::parse(file).unwrap() },
                position: Position::new(line, character),
            },
            new_name: new_name.to_string(),
            work_done_progress_params: Default::default(),
        };

        let response: WorkspaceEdit = self.send_request("textDocument/rename", json!(params)).await?;
        Ok(response)
    }

    // ─── 内部 JSON-RPC 方法 ───────────────────────

    async fn send_request<T: serde::de::DeserializeOwned>(
        &self,
        method: &str,
        params: Value,
    ) -> LspResult<T> {
        // Placeholder: 实际实现需要与 stdio 通信
        // 这里模拟一个基本的请求/响应循环
        
        if !self.initialized && method != "initialize" {
            return Err(LspError::NotInitialized);
        }

        let id = next_id();
        let _request = json!({
            "jsonrpc": "2.0",
            "id": id,
            "method": method,
            "params": params,
        });

        tracing::debug!(method = method, id = id, "LSP request");

        // TODO: 写入 stdin, 从 stdout 读取响应
        // 格式: Content-Length: ...\r\n\r\n{...json...}
        
        // 返回空结果作为占位符 — 真实连接需要子进程 stdio
        Err(LspError::NoServer)
    }

    async fn send_notification(&self, method: &str, params: Value) -> LspResult<()> {
        let _notification = json!({
            "jsonrpc": "2.0",
            "method": method,
            "params": params,
        });

        tracing::debug!(method = method, "LSP notification");
        
        // TODO: 写入 stdin (无 ID, 无响应预期)
        Ok(())
    }

    /// 关闭并退出
    pub async fn shutdown(&mut self) -> LspResult<()> {
        if !self.initialized {
            return Ok(());
        }

        // 发送 shutdown request
        let _: Value = self.send_request("shutdown", json!({})).await.map_err(|_| LspError::NoServer)?;

        // 发送 exit notification
        self.send_notification("exit", json!({})).await;

        self.initialized = false;
        self.capabilities = None;
        self.open_documents.clear();

        // Kill child process if exists
        if let Some(mut child) = self.process.take() {
            child.kill().await.ok();
        }

        Ok(())
    }
}

type LpResult<T> = Result<T, ()>;
