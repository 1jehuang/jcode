//! # IDE 集成 — JetBrains 插件 + VS Code 扩展
//!
//! 从 Claude Code 移植的 IDE 集成系统：
//! - IDE 检测: 锁文件/进程名 发现运行的 IDE
//! - JetBrains 插件检测: 扫描插件目录
//! - VS Code 扩展安装: 自动安装
//! - IDE RPC 通信: MCP over SSE/WebSocket
//! - Diff 显示: 在 IDE 中打开 diff 标签

use serde::{Deserialize, Serialize};
use std::collections::HashMap;
use std::path::PathBuf;
use tokio::sync::RwLock;
use tracing::info;

// ══════════════════════════════════════════════════════════════════
// IDE 类型检测
// ═════════════════════════════════════════════════════════════════

/// 支持的 IDE 类型
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Serialize, Deserialize)]
pub enum IdeType {
    // VS Code 系列
    VSCode,
    Cursor,
    Windsurf,
    // JetBrains 系列
    IntelliJ,
    PyCharm,
    WebStorm,
    PhpStorm,
    RubyMine,
    CLion,
    GoLand,
    Rider,
    DataGrip,
    AppCode,
    DataSpell,
    Aqua,
    Gateway,
    Fleet,
    AndroidStudio,
}

impl IdeType {
    /// 显示名称
    pub fn display_name(&self) -> &'static str {
        match self {
            Self::VSCode => "VS Code",
            Self::Cursor => "Cursor",
            Self::Windsurf => "Windsurf",
            Self::IntelliJ => "IntelliJ IDEA",
            Self::PyCharm => "PyCharm",
            Self::WebStorm => "WebStorm",
            Self::PhpStorm => "PhpStorm",
            Self::RubyMine => "RubyMine",
            Self::CLion => "CLion",
            Self::GoLand => "GoLand",
            Self::Rider => "Rider",
            Self::DataGrip => "DataGrip",
            Self::AppCode => "AppCode",
            Self::DataSpell => "DataSpell",
            Self::Aqua => "Aqua",
            Self::Gateway => "Gateway",
            Self::Fleet => "Fleet",
            Self::AndroidStudio => "Android Studio",
        }
    }

    /// 进程匹配关键字列表
    pub fn process_keywords(&self) -> &[&'static str] {
        match self {
            Self::VSCode => &["Code", "code-oss", "vscode"],
            Self::Cursor => &["cursor"],
            Self::Windsurf => &["windsurf"],
            Self::IntelliJ => &["idea"],
            Self::PyCharm => &["pycharm"],
            Self::WebStorm => &["webstorm"],
            Self::PhpStorm => &["phpstorm"],
            Self::RubyMine => &["rubymine"],
            Self::CLion => &["clion"],
            Self::GoLand => &["goland"],
            Self::Rider => &["rider"],
            Self::DataGrip => &["datagrip"],
            Self::AppCode => &["appcode"],
            Self::DataSpell => &["dataspell"],
            Self::Aqua => &["aqua"],
            Self::Gateway => &["gateway"],
            Self::Fleet => &["fleet"],
            Self::AndroidStudio => &["android-studio", "studio"],
        }
    }

    /// 是否为 JetBrains IDE
    pub fn is_jetbrains(&self) -> bool {
        matches!(self,
            Self::IntelliJ | Self::PyCharm | Self::WebStorm | Self::PhpStorm |
            Self::RubyMine | Self::CLion | Self::GoLand | Self::Rider | Self::DataGrip |
            Self::AppCode | Self::DataSpell | Self::Aqua | Self::Gateway | Self::Fleet |
            Self::AndroidStudio
        )
    }

    /// 是否为 VS Code 系列
    pub fn is_vscode_family(&self) -> bool {
        matches!(self, Self::VSCode | Self::Cursor | Self::Windsurf)
    }
}

// ══════════════════════════════════════════════════════════════════
// IDE 检测
// ═════════════════════════════════════════════════════════════════

/// 检测到的 IDE 信息
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct IdeInfo {
    pub ide_type: IdeType,
    pub port: Option<u16>,
    pub url: Option<String>,
    pub workspace_folders: Vec<String>,
    pub is_valid: bool,
    pub auth_token: Option<String>,
    pub extension_installed: bool,
}

/// IDE 检测器
pub struct IdeDetector;

impl IdeDetector {
    /// 通过锁文件检测 IDE
    /// 格式: ~/.claude/ide/<port>.lock
    pub fn detect_from_lockfiles() -> Vec<IdeInfo> {
        let mut found = Vec::new();
        let lock_dir = dirs::home_dir()
            .unwrap_or_else(|| PathBuf::from("."))
            .join(".claude")
            .join("ide");

        if !lock_dir.exists() {
            return found;
        }

        if let Ok(entries) = std::fs::read_dir(&lock_dir) {
            for entry in entries.flatten() {
                let path = entry.path();
                if path.extension().and_then(|e| e.to_str()) != Some("lock") {
                    continue;
                }
                // 文件名: <port>.lock
                let stem = path.file_stem().and_then(|s| s.to_str());
                let port: u16 = match stem.and_then(|s| s.parse().ok()) {
                    Some(p) => p,
                    None => continue,
                };

                // 读取锁文件内容
                if let Ok(content) = std::fs::read_to_string(&path)
                    && let Ok(lock_info) = serde_json::from_str::<IdeLockInfo>(&content) {
                        found.push(IdeInfo {
                            ide_type: lock_info.ide_type,
                            port: Some(port),
                            url: Some(format!("http://127.0.0.1:{}", port)),
                            workspace_folders: lock_info.workspace_folders,
                            is_valid: lock_info.is_valid,
                            auth_token: lock_info.auth_token,
                            extension_installed: false,
                        });
                    }
            }
        }

        found
    }

    /// 通过进程名检测运行中的 IDE
    pub fn detect_from_processes() -> Vec<IdeType> {
        let mut found = Vec::new();

        // 遍历所有 IDE 类型，检查进程
        for ide_type in ALL_IDE_TYPES {
            let keywords = ide_type.process_keywords();
            for keyword in keywords {
                if process_exists(keyword) {
                    found.push(*ide_type);
                    break;
                }
            }
        }

        found
    }

    /// 检测所有可用的 IDE
    pub fn detect_all() -> Vec<IdeInfo> {
        let mut results = Self::detect_from_lockfiles();

        // 补充通过进程名检测到的
        let from_processes = Self::detect_from_processes();
        for ide_type in from_processes {
            if !results.iter().any(|r| r.ide_type == ide_type) {
                results.push(IdeInfo {
                    ide_type,
                    port: None,
                    url: None,
                    workspace_folders: vec![],
                    is_valid: true,
                    auth_token: None,
                    extension_installed: false,
                });
            }
        }

        results
    }
}

/// 锁文件内容格式
#[derive(Debug, Clone, Serialize, Deserialize)]
struct IdeLockInfo {
    ide_type: IdeType,
    port: Option<u16>,
    workspace_folders: Vec<String>,
    is_valid: bool,
    auth_token: Option<String>,
}

/// 所有 IDE 类型列表
const ALL_IDE_TYPES: &[IdeType] = &[
    IdeType::VSCode, IdeType::Cursor, IdeType::Windsurf,
    IdeType::IntelliJ, IdeType::PyCharm, IdeType::WebStorm,
    IdeType::PhpStorm, IdeType::RubyMine, IdeType::CLion,
    IdeType::GoLand, IdeType::Rider, IdeType::DataGrip,
    IdeType::AppCode, IdeType::DataSpell, IdeType::Aqua,
    IdeType::Gateway, IdeType::Fleet, IdeType::AndroidStudio,
];

// ══════════════════════════════════════════════════════════════════
// JetBrains 插件检测
// ═════════════════════════════════════════════════════════════════

/// JetBrains 插件检测器
pub struct JetBrainsPluginDetector;

impl JetBrainsPluginDetector {
    /// 插件 ID
    const PLUGIN_ID: &'static str = "claude-code-jetbrains-plugin";

    /// 检测所有 JetBrains IDE 的插件目录
    pub fn detect_plugin_directories() -> Vec<PathBuf> {
        let mut dirs = Vec::new();

        // 按平台构建标准插件目录
        #[cfg(target_os = "macos")]
        let base = dirs::home_dir().map(|h| h.join("Library").join("Application Support"));

        #[cfg(target_os = "windows")]
        let base = std::env::var("APPDATA").ok().map(PathBuf::from);

        #[cfg(target_os = "linux")]
        let base = dirs::home_dir().map(|h| h.join(".local").join("share"));

        let Some(base) = base else { return dirs; };

        // JetBrains IDE 目录映射
        let ide_dirs = [
            "JetBrains/IntelliJIdea*", "JetBrains/PyCharm*", "JetBrains/WebStorm*",
            "JetBrains/PhpStorm*", "JetBrains/RubyMine*", "JetBrains/CLion*",
            "JetBrains/GoLand*", "JetBrains/Rider*", "JetBrains/DataGrip*",
        ];

        for pattern in &ide_dirs {
            let plugin_dir = base.join(pattern).join("plugins");
            // 展开通配符（简化处理）
            if let Some(parent) = plugin_dir.parent() {
                if parent.exists() {
                    if let Ok(entries) = std::fs::read_dir(parent) {
                        for entry in entries.flatten() {
                            let path = entry.path();
                            if path.is_dir() {
                                let plugins_path = path.join("plugins");
                                if plugins_path.exists() {
                                    dirs.push(plugins_path);
                                }
                            }
                        }
                    }
                }
            }
        }

        dirs
    }

    /// 检查插件是否已安装
    pub fn is_plugin_installed() -> bool {
        let dirs = Self::detect_plugin_directories();
        for dir in dirs {
            let plugin_path = dir.join(Self::PLUGIN_ID);
            if plugin_path.exists() {
                return true;
            }
            // 也检查 lib 子目录
            let lib_path = dir.join(Self::PLUGIN_ID).join("lib");
            if lib_path.exists() {
                return true;
            }
        }
        false
    }
}

// ══════════════════════════════════════════════════════════════════
// VS Code 扩展管理
// ═════════════════════════════════════════════════════════════════

pub struct VSCodeExtensionManager;

impl VSCodeExtensionManager {
    /// 扩展 ID
    const EXTENSION_ID: &'static str = "anthropic.claude-code";

    /// 获取 VS Code CLI 命令
    fn get_cli_command() -> Option<String> {
        let commands = ["code", "cursor", "windsurf"];
        for cmd in &commands {
            if which::which(cmd).is_ok() {
                return Some(cmd.to_string());
            }
        }
        None
    }

    /// 检查扩展是否已安装
    pub fn is_extension_installed() -> bool {
        let cmd = match Self::get_cli_command() {
            Some(c) => c,
            None => return false,
        };

        let output = match std::process::Command::new(&cmd)
            .args(["--list-extensions"])
            .output()
        {
            Ok(o) => o,
            Err(_) => return false,
        };

        let stdout = String::from_utf8_lossy(&output.stdout);
        stdout.contains(Self::EXTENSION_ID)
    }

    /// 安装扩展
    pub async fn install_extension() -> Result<bool, String> {
        let cmd = match Self::get_cli_command() {
            Some(c) => c,
            None => return Err("No VS Code CLI found".to_string()),
        };

        let output = tokio::process::Command::new(&cmd)
            .args(["--force", "--install-extension"])
            .arg(Self::EXTENSION_ID)
            .output()
            .await
            .map_err(|e| format!("Failed to install extension: {}", e))?;

        if output.status.success() {
            info!("VS Code extension '{}' installed", Self::EXTENSION_ID);
            Ok(true)
        } else {
            let err = String::from_utf8_lossy(&output.stderr);
            Err(format!("Extension install failed: {}", err))
        }
    }
}

// ══════════════════════════════════════════════════════════════════
// IDE RPC 通信
// ═════════════════════════════════════════════════════════════════

/// IDE RPC 请求
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct IdeRpcRequest {
    pub method: String,
    pub params: serde_json::Value,
    pub id: String,
}

/// IDE RPC 响应
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct IdeRpcResponse {
    pub id: String,
    pub result: Option<serde_json::Value>,
    pub error: Option<String>,
}

/// IDE RPC 客户端
pub struct IdeRpcClient {
    base_url: String,
    auth_token: Option<String>,
    client: reqwest::Client,
}

impl IdeRpcClient {
    pub fn new(ide_info: &IdeInfo) -> Self {
        let base_url = ide_info.url.clone().unwrap_or_else(|| "http://127.0.0.1:27131".to_string());
        Self {
            base_url,
            auth_token: ide_info.auth_token.clone(),
            client: reqwest::Client::new(),
        }
    }

    /// 调用 IDE RPC 方法
    pub async fn call(&self, method: &str, params: serde_json::Value) -> Result<serde_json::Value, String> {
        let request = IdeRpcRequest {
            method: method.to_string(),
            params,
            id: uuid::Uuid::new_v4().to_string(),
        };

        let url = format!("{}/rpc", self.base_url);
        let mut req = self.client.post(&url).json(&request);

        if let Some(ref token) = self.auth_token {
            req = req.header("Authorization", format!("Bearer {}", token));
        }

        let resp = req.send().await.map_err(|e| format!("RPC call failed: {}", e))?;
        let rpc_resp: IdeRpcResponse = resp.json().await.map_err(|e| format!("Failed to parse RPC response: {}", e))?;

        if let Some(err) = rpc_resp.error {
            Err(err)
        } else {
            Ok(rpc_resp.result.unwrap_or(serde_json::Value::Null))
        }
    }

    /// 在 IDE 中打开 diff
    pub async fn open_diff(&self, file_path: &str, new_content: &str) -> Result<serde_json::Value, String> {
        self.call("openDiff", serde_json::json!({
            "filePath": file_path,
            "newContent": new_content,
        })).await
    }

    /// 关闭 IDE 中所有 diff 标签
    pub async fn close_all_diffs(&self) -> Result<serde_json::Value, String> {
        self.call("closeAllDiffTabs", serde_json::json!({})).await
    }
}

// ══════════════════════════════════════════════════════════════════
// IDE 集成管理器
// ═════════════════════════════════════════════════════════════════

pub struct IdeIntegrationManager {
    /// 检测到的 IDE
    detected_ides: RwLock<Vec<IdeInfo>>,
    /// 当前活跃的 IDE
    active_ide: RwLock<Option<IdeInfo>>,
    /// RPC 客户端
    rpc_client: RwLock<Option<IdeRpcClient>>,
}

impl Default for IdeIntegrationManager {
    fn default() -> Self {
        Self::new()
    }
}

impl IdeIntegrationManager {
    pub fn new() -> Self {
        Self {
            detected_ides: RwLock::new(Vec::new()),
            active_ide: RwLock::new(None),
            rpc_client: RwLock::new(None),
        }
    }

    /// 初始化和检测所有 IDE
    pub async fn initialize(&self) {
        let detected = IdeDetector::detect_all();

        // 检查扩展/插件安装状态
        let mut ides = Vec::new();
        for mut ide in detected {
            if ide.ide_type.is_vscode_family() {
                ide.extension_installed = VSCodeExtensionManager::is_extension_installed();
            } else if ide.ide_type.is_jetbrains() {
                ide.extension_installed = JetBrainsPluginDetector::is_plugin_installed();
            }
            ides.push(ide);
        }

        *self.detected_ides.write().await = ides;
        info!("IDE integration initialized");
    }

    /// 获取所有检测到的 IDE
    pub async fn get_detected_ides(&self) -> Vec<IdeInfo> {
        self.detected_ides.read().await.clone()
    }

    /// 设置活跃 IDE
    pub async fn set_active_ide(&self, ide_info: IdeInfo) {
        let client = IdeRpcClient::new(&ide_info);
        *self.active_ide.write().await = Some(ide_info);
        *self.rpc_client.write().await = Some(client);
        info!("Active IDE set");
    }

    /// 获取活跃 IDE 的 RPC 客户端
    pub async fn get_rpc_client(&self) -> Option<IdeRpcClient> {
        if self.rpc_client.read().await.is_some() {
            let ide = self.active_ide.read().await;
            ide.as_ref().map(IdeRpcClient::new)
        } else {
            None
        }
    }

    /// 在 IDE 中显示 diff
    pub async fn show_diff(&self, file_path: &str, new_content: &str) -> Result<serde_json::Value, String> {
        let client = self.get_rpc_client().await
            .ok_or_else(|| "No active IDE".to_string())?;
        client.open_diff(file_path, new_content).await
    }
}

// ══════════════════════════════════════════════════════════════════
// 辅助函数
// ═════════════════════════════════════════════════════════════════

/// 检查进程是否存在（跨平台）
fn process_exists(keyword: &str) -> bool {
    let lower_kw = keyword.to_lowercase();

    let output = if cfg!(target_os = "windows") {
        std::process::Command::new("tasklist")
            .args(["/FO", "CSV", "/NH"])
            .output()
            .ok()
    } else {
        std::process::Command::new("ps")
            .args(["aux"])
            .output()
            .ok()
    };

    match output {
        Some(out) => {
            let stdout = String::from_utf8_lossy(&out.stdout);
            stdout.to_lowercase().contains(&lower_kw)
        }
        None => false,
    }
}

// ══════════════════════════════════════════════════════════════════
// MCP 传输层 — SSE/WebSocket/Stdio
// ═════════════════════════════════════════════════════════════════

/// MCP 传输类型
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum McpTransportType {
    Stdio, SSE, WebSocket, StreamableHttp,
}

impl McpTransportType {
    pub fn as_str(&self) -> &'static str {
        match self { Self::Stdio => "stdio", Self::SSE => "sse", Self::WebSocket => "ws", Self::StreamableHttp => "http" }
    }
}

/// MCP 服务器配置
#[derive(Debug, Clone)]
pub struct McpServerConfig {
    pub name: String,
    pub transport: McpTransportType,
    pub command: Option<String>,
    pub args: Vec<String>,
    pub url: Option<String>,
    pub headers: HashMap<String, String>,
    pub env: HashMap<String, String>,
}

/// MCP 连接状态
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum McpConnectionStatus {
    Disconnected, Connecting, Connected, Failed,
}

/// MCP 客户端
pub struct McpClient {
    config: McpServerConfig,
    status: McpConnectionStatus,
}

impl McpClient {
    pub fn new(config: McpServerConfig) -> Self {
        Self { config, status: McpConnectionStatus::Disconnected }
    }

    pub fn status(&self) -> McpConnectionStatus { self.status }

    /// 调用 IDE RPC 方法
    pub async fn call_ide_rpc(&self, method: &str, params: serde_json::Value) -> Result<serde_json::Value, String> {
        let url = self.config.url.as_ref().ok_or("No URL configured for MCP client")?;
        let client = reqwest::Client::new();
        let body = serde_json::json!({
            "jsonrpc": "2.0",
            "method": method,
            "params": params,
            "id": uuid::Uuid::new_v4().to_string(),
        });
        let resp = client.post(url).json(&body)
            .send().await.map_err(|e| format!("MCP RPC call failed: {}", e))?;
        let result: serde_json::Value = resp.json().await.map_err(|e| format!("Parse failed: {}", e))?;
        Ok(result)
    }

    /// 在 IDE 中打开 diff 标签
    pub async fn open_diff(&self, file_path: &str, old_content: &str, new_content: &str) -> Result<serde_json::Value, String> {
        self.call_ide_rpc("openDiff", serde_json::json!({
            "filePath": file_path,
            "oldContent": old_content,
            "newContent": new_content,
        })).await
    }

    /// 发送 @mentioned 通知到 IDE
    pub async fn notify_at_mentioned(&self, file_path: &str, line: u32) -> Result<serde_json::Value, String> {
        self.call_ide_rpc("at_mentioned", serde_json::json!({
            "filePath": file_path,
            "line": line,
        })).await
    }
}

/// MCP 连接管理器
pub struct McpConnectionManager {
    clients: RwLock<HashMap<String, McpClient>>,
}

impl McpConnectionManager {
    pub fn new() -> Self { Self { clients: RwLock::new(HashMap::new()) } }

    pub async fn register(&self, config: McpServerConfig) {
        let name = config.name.clone();
        self.clients.write().await.insert(name, McpClient::new(config));
    }

    pub async fn call_rpc(&self, server: &str, method: &str, params: serde_json::Value) -> Result<serde_json::Value, String> {
        let clients = self.clients.read().await;
        clients.get(server).ok_or_else(|| format!("MCP server '{}' not found", server))?.call_ide_rpc(method, params).await
    }

    pub async fn open_diff_ide(&self, file_path: &str, old_content: &str, new_content: &str) -> Result<serde_json::Value, String> {
        for client in self.clients.read().await.values() {
            if let Ok(r) = client.open_diff(file_path, old_content, new_content).await { return Ok(r); }
        }
        Err("No IDE connected to show diff".to_string())
    }
}

impl Default for McpConnectionManager { fn default() -> Self { Self::new() } }
