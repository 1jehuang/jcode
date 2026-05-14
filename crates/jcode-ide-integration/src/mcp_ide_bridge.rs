//! MCP IDE 桥接层
//!
//! 移植自 Claude Code:
//! - `src/hooks/useIDEIntegration.tsx` - IDE 作为 MCP Server 的生命周期管理
//! - `src/services/mcp/client.ts` - MCP 客户端调用 IDE RPC
//!
//! 核心概念:
//! IDE 被注册为**特殊的 MCP Server**, 通过统一的 MCP 协议与 AI Agent 通信:
//! ```typescript
//! // Claude Code 中的注册方式
//! dynamicMcpConfig.ide = {
//!   type: url.startsWith("ws:") ? "ws-ide" : "sse-ide",
//!   url: ide.url,
//!   ideName: ide.name,
//!   authToken: ide.authToken,
//!   scope: "dynamic"
//! }
//! ```

use crate::types::{DetectedIdeInfo, McpIdeConfig, IdeType};
use anyhow::Result;
use serde::{Deserialize, Serialize};
use serde_json::Value;
use std::sync::Arc;
use tokio::sync::RwLock;

// ============================================================================
// MCP IDE 配置管理
// ============================================================================

/// 动态 MCP 配置 (包含 IDE 桥接)
/// 
/// 对应 Claude Code 的 `dynamicMCP` 配置结构
#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(default)]
pub struct DynamicMcpConfig {
    /// IDE 桥接配置 (运行时动态注册)
    #[serde(skip_serializing_if = "Option::is_none")]
    pub ide: Option<McpIdeConfig>,
    
    /// 其他 MCP servers (静态配置)
    #[serde(default)]
    pub servers: std::collections::HashMap<String, McpServerConfig>,
}

/// MCP Server 通用配置
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct McpServerConfig {
    /// server 类型: stdio / sse / ws-ide
    #[serde(rename = "type")]
    pub mcp_type: String,
    
    /// 连接 URL 或命令
    pub url: Option<String>,
    pub command: Option<String>,
    pub args: Option<Vec<String>>,
    
    /// 环境变量
    #[serde(default)]
    pub env: std::collections::HashMap<String, String>,
}

impl Default for DynamicMcpConfig {
    fn default() -> Self {
        Self {
            ide: None,
            servers: std::collections::HashMap::new(),
        }
    }
}

// ============================================================================
// IDE RPC 调用封装
// ============================================================================

/// IDE RPC 调用方法列表
/// 移植自 Claude Code `callIdeRpc()` 在 `src/services/mcp/client.ts`
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Serialize, Deserialize)]
pub enum IdeRpcMethod {
    /// 获取编辑器中打开的文件列表
    GetOpenFiles,
    
    /// 获取当前活动文件的 URI
    GetActiveFile,
    
    /// 获取选中的文本范围
    GetSelection,
    
    /// 获取光标位置
    GetCursorLocation,
    
    /// 获取扩展列表
    GetExtensions,
    
    /// 安装扩展
    InstallExtension,
    
    /// 显示信息消息
    ShowInformationMessage,
    
    /// 显示警告消息
    ShowWarningMessage,
    
    /// 显示错误消息
    ShowErrorMessage,
    
    /// 显示输入框
    ShowInputBox,
    
    /// 打开文件并定位到指定位置
    OpenFileAtLocation,
    
    /// 应用文本编辑 (diff 格式)
    ApplyEdit,
    
    /// 执行命令 (如 format document)
    ExecuteCommand,
    
    /// 获取文档诊断
    GetDiagnostics,
    
    /// 获取符号引用
    FindReferences,
}

impl IdeRpcMethod {
    /// 转换为实际的 RPC 方法名字符串
    pub fn as_str(&self) -> &'static str {
        match self {
            Self::GetOpenFiles => "getOpenFiles",
            Self::GetActiveFile => "getActiveFile",
            Self::GetSelection => "getSelection",
            Self::GetCursorLocation => "getCursorLocation",
            Self::GetExtensions => "getExtensions",
            Self::InstallExtension => "installExtension",
            Self::ShowInformationMessage => "showInformationMessage",
            Self::ShowWarningMessage => "showWarningMessage",
            Self::ShowErrorMessage => "showErrorMessage",
            Self::ShowInputBox => "showInputBox",
            Self::OpenFileAtLocation => "openFileAtLocation",
            Self::ApplyEdit => "applyEdit",
            Self::ExecuteCommand => "executeCommand",
            Self::GetDiagnostics => "getDiagnostics",
            Self::FindReferences => "findReferences",
        }
    }

    /// 所有可用方法的列表 (用于能力声明)
    pub fn all() -> &'static [Self] {
        &[
            Self::GetOpenFiles,
            Self::GetActiveFile,
            Self::GetSelection,
            Self::GetCursorLocation,
            Self::GetExtensions,
            Self::InstallExtension,
            Self::ShowInformationMessage,
            Self::ShowWarningMessage,
            Self::ShowErrorMessage,
            Self::ShowInputBox,
            Self::OpenFileAtLocation,
            Self::ApplyEdit,
            Self::ExecuteCommand,
            Self::GetDiagnostics,
            Self::FindReferences,
        ]
    }
}

impl std::fmt::Display for IdeRpcMethod {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        write!(f, "{}", self.as_str())
    }
}

/// IDE RPC 响应
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct IdeRpcResponse<T> {
    #[serde(default)]
    pub success: bool,
    pub data: Option<T>,
    #[serde(default)]
    pub error: Option<String>,
}

/// 文件位置信息 (用于 openFileAtLocation 等)
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct FileLocation {
    pub uri: String,
    pub line: u32,
    pub column: u32,
}

/// 文本编辑操作 (用于 applyEdit)
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct TextEditOperation {
    pub file_uri: String,
    pub old_string: String,
    pub new_string: String,
}

// ============================================================================
// MCP IDE Bridge 主结构
// ============================================================================

/// MCP IDE 桥接器 - 管理 IDE 与 MCP 系统的集成
///
/// ## 架构
/// ```text
/// ┌──────────────────────────────────────┐
/// │         McpIdeBridge                 │
/// ├──────────────────────────────────────┤
/// │  ide_config: Option<McpIdeConfig>    │ ← 当前连接的 IDE
/// │  mcp_config: DynamicMcpConfig       │ ← 完整 MCP 配置
/// │  http_client: reqwest::Client       │ ← HTTP RPC 客户端
/// │  available_methods: HashSet<Method> │ ← 能力集
/// └──────────────────────────────────────┘
///
///          ↕ MCP Protocol over HTTP/SSE
/// ┌──────────────────────────────────────┐
/// │     AI Agent (JCode Agent)          │
/// │   → 通过 MCP Tool 调用 IDE 功能      │
/// └──────────────────────────────────────┘
/// ```
pub struct McpIdeBridge {
    /// 当前已桥接的 IDE 信息
    ide_info: Arc<RwLock<Option<DetectedIdeInfo>>>,
    
    /// MCP 动态配置
    mcp_config: Arc<RwLock<DynamicMcpConfig>>,
    
    /// 可用的 RPC 方法集合
    available_methods: Arc<RwLock<std::collections::HashSet<IdeRpcMethod>>>,
    
    /// HTTP 客户端 (用于向 IDE 发送 JSON-RPC 请求)
    http_client: Arc<reqwest::Client>,
}

impl McpIdeBridge {
    /// 创建新的 MCP IDE 桥接器
    pub fn new() -> Self {
        Self {
            ide_info: Arc::new(RwLock::new(None)),
            mcp_config: Arc::new(RwLock::new(DynamicMcpConfig::default())),
            available_methods: Arc::new(RwLock::new(
                IdeRpcMethod::all().iter().copied().collect()
            )),
            http_client: Arc::new(
                reqwest::Client::builder()
                    .timeout(std::time::Duration::from_secs(30))
                    .build()
                    .expect("Failed to create HTTP client")
            ),
        }
    }

    /// 注册 IDE 到 MCP 系统
    ///
    /// 这是核心方法 - 将检测到的 IDE 注册为 MCP Server:
    /// ```text
    /// Before: dynamicMcpConfig.ide = null
    /// After:  dynamicMcpConfig.ide = { type: "ws-ide", url: "...", ... }
    /// ```
    pub async fn register_ide(&self, ide: DetectedIdeInfo) -> Result<()> {
        info!("Registering IDE as MCP Server: {} ({})", ide.name, ide.url);

        let mcp_ide_config = McpIdeConfig::from_detected_ide(&ide);

        // 更新 MCP 配置
        let mut config = self.mcp_config.write().await;
        config.ide = Some(mcp_ide_config);

        // 更新 IDE 信息
        let mut info = self.ide_info.write().await;
        *info = Some(ide);

        info!("IDE registered as MCP Server successfully");
        Ok(())
    }

    /// 注销当前 IDE
    pub async fn unregister_ide(&self) -> Result<()> {
        info!("Unregistering IDE from MCP");

        let mut config = self.mcp_config.write().await;
        config.ide = None;

        let mut info = self.ide_info.write().await;
        *info = None;

        Ok(())
    }

    /// 检查是否已有 IDE 注册
    pub async fn is_registered(&self) -> bool {
        let config = self.mcp_config.read().await;
        config.ide.is_some()
    }

    /// 获取当前 IDE 配置
    pub async fn get_ide_config(&self) -> Option<McpIdeConfig> {
        let config = self.mcp_config.read().await;
        config.ide.clone()
    }

    /// 获取当前 IDE 信息
    pub async fn get_ide_info(&self) -> Option<DetectedIdeInfo> {
        let info = self.ide_info.read().await;
        info.clone()
    }

    /// 调用 IDE RPC 方法
    ///
    /// 通过 MCP JSON-RPC 协议向已注册的 IDE 发送请求。
    /// 支持三种传输模式:
    /// - SSE/HTTP: POST /message?sessionId=<id> (MCP 标准)
    /// - WebSocket: ws://<url>/ws
    /// - 直接 HTTP: POST <url>/<method>
    ///
    /// # Example
    /// ```ignore
    /// let result: IdeRpcResponse<Vec<String>> = bridge.call_rpc(
    ///     IdeRpcMethod::GetOpenFiles,
    ///     json!({}),
    /// ).await?;
    /// ```
    pub async fn call_rpc<T: serde::de::DeserializeOwned>(
        &self,
        method: IdeRpcMethod,
        params: serde_json::Value,
    ) -> Result<IdeRpcResponse<T>> {
        // 检查是否有已注册的 IDE
        let config = self.mcp_config.read().await;
        let ide_config = config.ide.as_ref().ok_or_else(|| {
            anyhow::anyhow!("No IDE registered. Call register_ide() first.")
        })?;

        let method_str = method.as_str();
        let url = ide_config.url.trim_end_matches('/').to_string();

        tracing::debug!(
            "IDE RPC call -> {}: {:?} via {}",
            method_str,
            params,
            url
        );

        // 根据 IDE 类型选择传输方式
        let response = match ide_config.mcp_type.as_str() {
            t if t.contains("sse") || t.contains("http") => {
                // SSE/HTTP 传输: POST /message?sessionId=<id>
                let message_url = format!("{}/message", url);
                
                let request_body = serde_json::json!({
                    "jsonrpc": "2.0",
                    "id": 1,
                    "method": method_str,
                    "params": params,
                });

                let mut req = self.http_client.post(&message_url)
                    .json(&request_body);

                // Add auth token if available
                if let Some(token) = &ide_config.auth_token {
                    req = req.header("Authorization", format!("Bearer {}", token));
                }

                let resp = req.send().await.map_err(|e| {
                    anyhow::anyhow!("IDE RPC HTTP request failed for '{}': {}", method_str, e)
                })?;

                if !resp.status().is_success() {
                    return Err(anyhow::anyhow!(
                        "IDE RPC '{}' returned HTTP {}",
                        method_str,
                        resp.status()
                    ));
                }

                let value: serde_json::Value = resp.json().await.map_err(|e| {
                    anyhow::anyhow!("IDE RPC '{}' response parse error: {}", method_str, e)
                })?;

                value
            }
            t if t.contains("ws") || t.contains("websocket") => {
                // WebSocket 传输 — 通过 WebSocket 发送 JSON-RPC 消息
                // 注意: tokio-tungstenite 或类似库需要作为依赖添加
                // 当前实现使用 HTTP 回退
                tracing::warn!(
                    "WebSocket transport not fully implemented for '{}', falling back to HTTP",
                    method_str
                );

                let message_url = format!("{}/{}", url, method_str);
                let request_body = serde_json::json!({
                    "jsonrpc": "2.0",
                    "id": 1,
                    "method": method_str,
                    "params": params,
                });

                let resp = self.http_client.post(&message_url)
                    .json(&request_body)
                    .send()
                    .await
                    .map_err(|e| {
                        anyhow::anyhow!("IDE RPC (WS fallback) failed: {}", e)
                    })?;

                resp.json::<serde_json::Value>().await
                    .map_err(|e| anyhow::anyhow!("IDE RPC response parse error: {}", e))?
            }
            _ => {
                // 直接 HTTP POST 到 IDE 端点
                let full_url = format!("{}/{}", url, method_str);
                let request_body = serde_json::json!({
                    "jsonrpc": "2.0",
                    "id": 1,
                    "method": method_str,
                    "params": params,
                });

                let resp = self.http_client.post(&full_url)
                    .json(&request_body)
                    .send()
                    .await
                    .map_err(|e| {
                        anyhow::anyhow!("IDE RPC HTTP call to '{}' failed: {}", full_url, e)
                    })?;

                resp.json::<serde_json::Value>().await
                    .map_err(|e| anyhow::anyhow!("IDE RPC response parse error: {}", e))?
            }
        };

        // 解析 JSON-RPC 响应
        if let Some(error) = response.get("error") {
            let code = error.get("code").and_then(|c| c.as_i64()).unwrap_or(-1);
            let message = error.get("message").and_then(|m| m.as_str()).unwrap_or("Unknown error");
            return Ok(IdeRpcResponse {
                success: false,
                data: None,
                error: Some(format!("JSON-RPC error {}: {}", code, message)),
            });
        }

        if let Some(result) = response.get("result") {
            let data: T = serde_json::from_value(result.clone())
                .map_err(|e| anyhow::anyhow!("Failed to deserialize RPC result for '{}': {}", method_str, e))?;
            return Ok(IdeRpcResponse {
                success: true,
                data: Some(data),
                error: None,
            });
        }

        // Unexpected response format
        Err(anyhow::anyhow!(
            "IDE RPC '{}' returned unexpected response: {:?}",
            method_str,
            response
        ))
    }

    /// 获取可用的 IDE RPC 方法列表
    pub async fn get_available_methods(&self) -> Vec<IdeRpcMethod> {
        let methods = self.available_methods.read().await;
        methods.iter().copied().collect()
    }

    /// 将 IDE 功能导出为 MCP Tool 定义
    ///
    /// 这些工具可以被 JCode Agent 直接使用:
    /// - `ide_get_open_files` → 获取打开的文件
    /// - `ide_apply_edit` → 应用代码修改
    /// - `ide_find_references` → 查找引用
    /// - etc.
    pub fn export_as_mcp_tools(&self) -> Vec<McpToolDefinition> {
        IdeRpcMethod::all()
            .iter()
            .map(|method| McpToolDefinition {
                name: format!("ide_{}", method.as_str().to_lowercase()),
                description: Self::tool_description_for_method(method),
                input_schema: Self::input_schema_for_method(method),
            })
            .collect()
    }

    fn tool_description_for_method(method: &IdeRpcMethod) -> &'static str {
        match method {
            IdeRpcMethod::GetOpenFiles => "Get the list of currently open files in the IDE",
            IdeRpcMethod::GetActiveFile => "Get the URI of the currently active/visible file",
            IdeRpcMethod::GetSelection => "Get the currently selected text range in the editor",
            IdeRpcMethod::GetCursorLocation => "Get the cursor position (line, column) in the active file",
            IdeRpcMethod::GetExtensions => "List installed IDE extensions and their status",
            IdeRpcMethod::InstallExtension => "Install an extension by ID into the connected IDE",
            IdeRpcMethod::ShowInformationMessage => "Display an informational message/notification in the IDE",
            IdeRpcMethod::ShowWarningMessage => "Display a warning message in the IDE",
            IdeRpcMethod::ShowErrorMessage => "Display an error message in the IDE",
            IdeRpcMethod::ShowInputBox => "Show an input box/prompt to the user in the IDE",
            IdeRpcMethod::OpenFileAtLocation => "Open a specific file and navigate to a line/column position",
            IdeRpcMethod::ApplyEdit => "Apply a text edit (old_string → new_string) to a file",
            IdeRpcMethod::ExecuteCommand => "Execute an IDE command (e.g., 'editor.action.formatDocument')",
            IdeRpcMethod::GetDiagnostics => "Get diagnostics (errors/warnings) for a specific file",
            IdeRpcMethod::FindReferences => "Find all references to a symbol at a given location",
        }
    }

    fn input_schema_for_method(_method: &IdeRpcMethod) -> serde_json::Value {
        // TODO: 为每个方法定义详细的 JSON Schema 输入参数
        serde_json::json!({
            "type": "object",
            "properties": {},
            "additionalProperties": true
        })
    }
}

impl Default for McpIdeBridge {
    fn default() -> Self {
        Self::new()
    }
}

// ============================================================================
// MCP Tool 定义 (暴露给 Agent)
// ============================================================================

/// MCP Tool 定义 (供 Agent 使用)
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct McpToolDefinition {
    /// 工具名称 (如 "ide_get_open_files")
    pub name: String,
    
    /// 工具描述
    pub description: &'static str,
    
    /// 输入参数 JSON Schema
    pub input_schema: serde_json::Value,
}
