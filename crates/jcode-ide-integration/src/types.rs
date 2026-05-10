//! IDE 深度集成类型定义
//!
//! 移植自 Claude Code:
//! - src/utils/ide.ts (Lockfile 协议, IDE 类型系统)
//! - src/services/lsp/LSPClient.ts (LSP 接口定义)
//! - src/hooks/useIDEIntegration.tsx (MCP IDE 桥接)
//!
//! 设计原则:
//! - Lockfile 服务发现协议: 扫描 ~/.jcode/ide/*.lock 发现运行中的 IDE
//! - 双协议支持: WebSocket (实时) + SSE (服务端推送)
//! - MCP 抽象: IDE 被注册为特殊的 MCP Server

use serde::{Deserialize, Serialize};

// ============================================================================
// IDE 类型系统 - 移植自 Claude Code ide.ts:22 (IdeType, 22种IDE支持)
// ============================================================================

/// 支持的 IDE 类型
/// 移植自 Claude Code: `type IdeType = 'cursor' | 'windsurf' | 'vscode' | 'pycharm' | ...`
#[derive(Debug, Clone, PartialEq, Eq, Hash, Serialize, Deserialize)]
#[serde(rename_all = "kebab-case")]
pub enum IdeType {
    // === VSCode 系列 (共享 vscode 协议) ===
    /// Visual Studio Code
    VsCode,
    /// Cursor AI IDE (最常用)
    Cursor,
    /// Windsurf AI IDE
    Windsurf,

    // === JetBrains 系列 (19种) ===
    /// PyCharm (Python)
    PyCharm,
    /// IntelliJ IDEA (Java/Kotlin)
    IntelliJ,
    /// WebStorm (JavaScript/TypeScript)
    WebStorm,
    /// PhpStorm (PHP)
    PhpStorm,
    /// GoLand (Go)
    GoLand,
    /// RustAnalyzer (Rust)
    RustRover,
    /// DataGrip (SQL)
    DataGrip,
    /// RubyMine (Ruby)
    RubyMine,
    /// CLion (C/C++)
    CLion,
    /// AppCode (Objective-C/Swift)
    AppCode,

    // === 其他 ===
    /// Neovim (通过扩展)
    Neovim,
    /// 自定义/未知
    Unknown(String),
}

impl IdeType {
    /// 返回 IDE 显示名称
    pub fn display_name(&self) -> &str {
        match self {
            Self::VsCode => "Visual Studio Code",
            Self::Cursor => "Cursor",
            Self::Windsurf => "Windsurf",
            Self::PyCharm => "PyCharm",
            Self::IntelliJ => "IntelliJ IDEA",
            Self::WebStorm => "WebStorm",
            Self::PhpStorm => "PhpStorm",
            Self::GoLand => "GoLand",
            Self::RustRover => "RustRover",
            Self::DataGrip => "DataGrip",
            Self::RubyMine => "RubyMine",
            Self::CLion => "CLion",
            Self::AppCode => "AppCode",
            Self::Neovim => "Neovim",
            Self::Unknown(name) => name.as_str(),
        }
    }

    /// 判断是否属于 VSCode 系列（共享 vscode 协议）
    pub fn is_vscode_family(&self) -> bool {
        matches!(self, Self::VsCode | Self::Cursor | Self::Windsurf)
    }

    /// 判断是否属于 JetBrains 系列
    pub fn is_jetbrains_family(&self) -> bool {
        matches!(
            self,
            Self::PyCharm
                | Self::IntelliJ
                | Self::WebStorm
                | Self::PhpStorm
                | Self::GoLand
                | Self::RustRover
                | Self::DataGrip
                | Self::RubyMine
                | Self::CLion
                | Self::AppCode
        )
    }

    /// 从字符串解析 IDE 类型
    /// 兼容多种命名格式: "VSCode", "cursor", "Visual Studio Code"
    pub fn from_str_flexible(s: &str) -> Self {
        match s.to_ascii_lowercase().trim() {
            "vscode" | "visual studio code" | "code" => Self::VsCode,
            "cursor" => Self::Cursor,
            "windsurf" => Self::Windsurf,
            "pycharm" => Self::PyCharm,
            "intellij" | "intellij idea" | "idea" => Self::IntelliJ,
            "webstorm" => Self::WebStorm,
            "phpstorm" => Self::PhpStorm,
            "goland" | "go land" => Self::GoLand,
            "rustrover" | "rust rover" => Self::RustRover,
            "datagrip" | "data grip" => Self::DataGrip,
            "rubymine" | "ruby mine" => Self::RubyMine,
            "clion" => Self::CLion,
            "appcode" | "app code" => Self::AppCode,
            "neovim" | "nvim" => Self::Neovim,
            other => Self::Unknown(other.to_string()),
        }
    }
}

impl std::fmt::Display for IdeType {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        write!(f, "{}", self.display_name())
    }
}

// ============================================================================
// Lockfile 协议 - 移植自 Claude Code ide.ts:73-90
// ============================================================================

/// Lockfile JSON 内容结构
/// 对应 Claude Code: `type LockfileJsonContent`
///
/// 存储位置: `~/.jcode/ide/{port}.lock`
/// 格式示例: {"workspaceFolders": ["/home/user/project"], "pid": 12345, "ideName": "Cursor"}
#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(default)]
pub struct IdeLockfileContent {
    /// IDE 工作区路径列表
    #[serde(skip_serializing_if = "Vec::is_empty")]
    pub workspace_folders: Vec<String>,

    /// IDE 进程 PID
    #[serde(skip_serializing_if = "Option::is_none")]
    pub pid: Option<u32>,

    /// IDE 显示名称
    #[serde(skip_serializing_if = "Option::is_none")]
    pub ide_name: Option<String>,

    /// 通信协议类型
    #[serde(skip_serializing_if = "Option::is_none")]
    pub transport: Option<IdeTransport>,

    /// IDE 是否运行在 Windows 上 (WSL 兼容标记)
    #[serde(skip_serializing_if = "Option::is_none")]
    pub running_in_windows: Option<bool>,

    /// OAuth 认证令牌
    #[serde(skip_serializing_if = "Option::is_none")]
    pub auth_token: Option<String>,
}

impl Default for IdeLockfileContent {
    fn default() -> Self {
        Self {
            workspace_folders: Vec::new(),
            pid: None,
            ide_name: None,
            transport: None,
            running_in_windows: None,
            auth_token: None,
        }
    }
}

/// IDE 通信传输协议
/// 移植自 Claude Code: `transport?: 'ws' | 'sse'`
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "lowercase")]
pub enum IdeTransport {
    /// WebSocket 实时双向通信
    WebSocket,
    /// SSE (Server-Sent Events) 单向推送
    Sse,
}

impl IdeTransport {
    /// 根据URL自动判断传输协议
    pub fn from_url(url: &str) -> Self {
        if url.starts_with("ws:") || url.starts_with("wss:") {
            Self::WebSocket
        } else {
            Self::Sse
        }
    }

    /// 构建连接 URL
    pub fn build_url(&self, host: &str, port: u16, path: &str) -> String {
        match self {
            Self::WebSocket => format!("ws://{}:{}{}", host, port, path),
            Self::Sse => format!("http://{}:{}{}", host, port, path),
        }
    }
}

// ============================================================================
// IDE 检测信息 - 移植自 Claude Code ide.ts:92-99
// ============================================================================

/// IDE 检测结果信息
/// 对应 Claude Code: `export type DetectedIDEInfo`
#[derive(Debug, Clone)]
pub struct DetectedIdeInfo {
    /// IDE 名称
    pub name: String,

    /// IDE 监听端口
    pub port: u16,

    /// 工作区路径列表
    pub workspace_folders: Vec<String>,

    /// 连接 URL (ws:// 或 http://)
    pub url: String,

    /// 是否有效 (PID 校验 + cwd 匹配)
    pub is_valid: bool,

    /// 认证令牌
    pub auth_token: Option<String>,

    /// IDE 运行在 Windows (从 WSL 视角)
    pub ide_running_in_windows: Option<bool>,

    /// IDE 类型
    pub ide_type: IdeType,

    /// Lockfile 文件修改时间 (用于排序)
    pub lockfile_mtime: chrono::DateTime<chrono::Local>,
}

impl DetectedIdeInfo {
    /// 创建检测信息
    pub fn new(
        name: String,
        port: u16,
        url: String,
        transport: IdeTransport,
        lockfile_content: IdeLockfileContent,
        mtime: chrono::DateTime<chrono::Local>,
    ) -> Self {
        let ide_type = lockfile_content
            .ide_name
            .as_deref()
            .map(IdeType::from_str_flexible)
            .unwrap_or(IdeType::Unknown(name.clone()));

        Self {
            name,
            port,
            workspace_folders: lockfile_content.workspace_folders,
            url,
            is_valid: false, // 需要后续校验
            auth_token: lockfile_content.auth_token,
            ide_running_in_windows: lockfile_content.running_in_windows,
            ide_type,
            lockfile_mtime: mtime,
        }
    }

    /// 校验此 IDE 信息是否有效:
    /// 1. PID 进程是否仍在运行
    /// 2. 工作目录是否与当前 cwd 匹配 (或为子目录)
    pub fn validate(&mut self, current_cwd: &std::path::Path) -> bool {
        // PID 校验
        if let Some(pid) = self.pid_from_lockfile() {
            if !Self::is_process_running(pid) {
                self.is_valid = false;
                return false;
            }
        }

        // 工作区校验: 至少一个工作文件夹匹配当前 cwd 或其父目录
        let cwd_str = current_cwd.to_string_lossy().to_string();
        let has_matching_workspace = self.workspace_folders.iter().any(|folder| {
            cwd_str.starts_with(folder.as_str()) || folder.starts_with(cwd_str.as_str())
        });

        self.is_valid = has_matching_workspace || self.workspace_folders.is_empty();
        self.is_valid
    }

    /// 从 lockfile 内容获取 PID
    fn pid_from_lockfile(&self) -> Option<u32> {
        // 此处需要从原始 lockfile 读取，简化版直接返回 None
        // 实际实现应在 detect 时保存 pid
        None
    }

    /// 检查指定 PID 的进程是否仍在运行
    /// 移植自 Claude Code ide.ts:49-56 `isProcessRunning()`
    #[cfg(unix)]
    fn is_process_running(pid: u32) -> bool {
        unsafe { libc::kill(pid as i32, 0) == 0 }
    }

    #[cfg(windows)]
    fn is_process_running(pid: u32) -> bool {
        use windows_sys::Win32::Foundation::{CloseHandle, OpenProcess};
        use windows_sys::Win32::System::Threading::PROCESS_QUERY_LIMITED_INFORMATION;
        unsafe {
            let handle = OpenProcess(PROCESS_QUERY_LIMITED_INFORMATION, false, pid);
            if handle != 0 {
                CloseHandle(handle);
                true
            } else {
                false
            }
        }
    }
}

// ============================================================================
// MCP IDE 桥接配置 - 移植自 Claude Code useIDEIntegration.tsx (MCP 抽象)
// ============================================================================

/// MCP IDE 配置 (将 IDE 注册为特殊 MCP Server)
/// 移植自 Claude Code: `dynamicMcpConfig.ide = { type: "ws-ide", ... }`
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct McpIdeConfig {
    /// MCP Server 类型: ws-ide 或 sse-ide
    #[serde(rename = "type")]
    pub mcp_type: String,

    /// IDE 连接 URL
    pub url: String,

    /// IDE 显示名称
    pub ide_name: String,

    /// 认证令牌
    #[serde(skip_serializing_if = "Option::is_none")]
    pub auth_token: Option<String>,

    /// 作用域: "dynamic" 表示运行时动态注册
    pub scope: String,
}

impl McpIdeConfig {
    /// 从 DetectedIdeInfo 创建 MCP 配置
    pub fn from_detected_ide(ide: &DetectedIdeInfo) -> Self {
        let mcp_type = if ide.url.starts_with("ws:") || ide.url.starts_with("wss:") {
            "ws-ide".to_string()
        } else {
            "sse-ide".to_string()
        };

        Self {
            mcp_type,
            url: ide.url.clone(),
            ide_name: ide.name.clone(),
            auth_token: ide.auth_token.clone(),
            scope: "dynamic".to_string(),
        }
    }
}

// ============================================================================
// LSP 相关类型 - 移植自 Claude Code LSPClient.ts
// ============================================================================

/// LSP 启动选项
#[derive(Debug, Clone)]
pub struct LspStartOptions {
    /// 环境变量
    pub env: Option<std::collections::HashMap<String, String>>,

    /// 工作目录
    pub cwd: Option<std::path::PathBuf>,
}

/// LSP 诊断信息
/// 移植自 Claude Code lsp-types 封装
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct LspDiagnostic {
    /// 文件路径
    pub file_path: String,

    /// 严重级别: Error / Warning / Hint / Information
    pub severity: LspSeverity,

    /// 行号 (1-based)
    pub line: u32,

    /// 列号 (1-based)
    pub column: u32,

    /// 诊断消息
    pub message: String,

    /// 诊断代码 (如 "unused_variable")
    pub code: Option<String>,

    /// 来源 (如 "rustc", "typescript")
    pub source: Option<String>,
}

/// LSP 诊断严重级别
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "lowercase")]
pub enum LspSeverity {
    Error,
    Warning,
    Information,
    Hint,
}

impl From<lsp_types::DiagnosticSeverity> for LspSeverity {
    fn from(severity: lsp_types::DiagnosticSeverity) -> Self {
        match severity {
            lsp_types::DiagnosticSeverity::ERROR => Self::Error,
            lsp_types::DiagnosticSeverity::WARNING => Self::Warning,
            lsp_types::DiagnosticSeverity::INFORMATION => Self::Information,
            lsp_types::DiagnosticSeverity::HINT => Self::Hint,
            _ => Self::Information, // 未知值默认为 Information
        }
    }
}

/// LSP 符号引用信息
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct LspReference {
    /// 引用所在文件路径
    pub file_path: String,

    /// 行号 (1-based)
    pub line: u32,

    /// 列号 (1-based)
    pub column: u32,
}

// ============================================================================
// IDE 连接状态 - 用于 TUI 状态管理
// ============================================================================

/// IDE 连接状态枚举
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub enum IdeConnectionStatus {
    /// 未连接
    Disconnected,
    /// 正在检测 IDE
    Detecting,
    /// 正在建立连接
    Connecting,
    /// 已连接
    Connected {
        ide_name: String,
        /// 是否已安装扩展
        extension_installed: bool,
    },
    /// 连接断开 (含原因)
    DisconnectedWithReason(String),
    /// 错误状态
    Error(String),
}

impl Default for IdeConnectionStatus {
    fn default() -> Self {
        Self::Disconnected
    }
}

impl std::fmt::Display for IdeConnectionStatus {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            Self::Disconnected => write!(f, "未连接"),
            Self::Detecting => write!(f, "正在检测 IDE..."),
            Self::Connecting => write!(f, "正在连接..."),
            Self::Connected {
                ide_name,
                extension_installed,
            } => {
                if *extension_installed {
                    write!(f, "已连接到 {} ✓", ide_name)
                } else {
                    write!(f, "已连接到 {} (需安装扩展)", ide_name)
                }
            }
            Self::DisconnectedWithReason(reason) => {
                write!(f, "已断开: {}", reason)
            }
            Self::Error(err) => write!(f, "IDE 错误: {}", err),
        }
    }
}
