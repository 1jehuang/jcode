//! # Chrome 扩展桥接 — 浏览器自动化
//!
//! 从 Claude Code 移植的 Claude in Chrome 集成：
//! - Chrome Native Host (Unix socket / named pipe)
//! - MCP Server for browser tools
//! - 浏览器检测 (7种 Chromium 浏览器)
//! - 扩展安装/状态管理

use serde::{Deserialize, Serialize};
use std::path::{Path, PathBuf};
use std::sync::Arc;
use tokio::sync::RwLock;
use tracing::info;

// ══════════════════════════════════════════════════════════════════
// 浏览器检测
// ═════════════════════════════════════════════════════════════════

/// 支持的 Chromium 浏览器
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Serialize, Deserialize)]
pub enum ChromiumBrowser {
    Chrome,
    Brave,
    Arc,
    Chromium,
    Edge,
    Vivaldi,
    Opera,
}

impl ChromiumBrowser {
    /// 所有浏览器列表（按检测优先级排列）
    pub fn all() -> &'static [ChromiumBrowser] {
        &[Self::Chrome, Self::Brave, Self::Arc, Self::Edge,
          Self::Chromium, Self::Vivaldi, Self::Opera]
    }

    /// 主进程文件/包名
    pub fn process_name(&self) -> &'static str {
        match self {
            Self::Chrome => "Google Chrome",
            Self::Brave => "Brave Browser",
            Self::Arc => "Arc",
            Self::Chromium => "Chromium",
            Self::Edge => "Microsoft Edge",
            Self::Vivaldi => "Vivaldi",
            Self::Opera => "Opera",
        }
    }

    /// 扩展目录名
    pub fn extension_dir(&self) -> Option<&'static str> {
        match self {
            Self::Chrome => Some("Google/Chrome"),
            Self::Brave => Some("BraveSoftware/Brave-Browser"),
            Self::Arc => None,
            Self::Chromium => Some("Chromium"),
            Self::Edge => Some("Microsoft/Edge"),
            Self::Vivaldi => Some("Vivaldi"),
            Self::Opera => Some("Opera Software/Opera"),
        }
    }
}

/// 检测到的浏览器信息
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct BrowserInfo {
    pub browser: ChromiumBrowser,
    pub executable_path: PathBuf,
    pub extension_installed: bool,
    pub native_host_installed: bool,
}

// ══════════════════════════════════════════════════════════════════
// Chrome Native Host — Unix Socket 桥接
// ═════════════════════════════════════════════════════════════════

/// Native Messaging Host 消息格式
#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(tag = "type")]
pub enum ChromeHostMessage {
    #[serde(rename = "ping")]
    Ping { id: u64 },
    #[serde(rename = "get_status")]
    GetStatus { id: u64 },
    #[serde(rename = "tool_request")]
    ToolRequest { id: u64, tool: String, args: serde_json::Value },
    #[serde(rename = "tool_response")]
    ToolResponse { id: u64, result: serde_json::Value },
    #[serde(rename = "notification")]
    Notification { event: String, data: serde_json::Value },
}

/// Chrome Native Host 状态
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct ChromeHostStatus {
    pub connected: bool,
    pub extension_paired: bool,
    pub device_id: Option<String>,
    pub browser: Option<String>,
}

/// Chrome Native Host 管理器
pub struct ChromeNativeHost {
    /// Unix socket 路径 (Unix) 或 named pipe 路径 (Windows)
    socket_path: PathBuf,
    running: Arc<RwLock<bool>>,
    browser_detected: Arc<RwLock<Option<ChromiumBrowser>>>,
    host_status: Arc<RwLock<ChromeHostStatus>>,
    /// 浏览器扩展已安装
    extension_installed: Arc<RwLock<bool>>,
}

impl ChromeNativeHost {
    /// 创建新的 Chrome Native Host 管理器
    pub fn new(username: &str) -> Self {
        let socket_path = if cfg!(windows) {
            PathBuf::from(format!("\\\\.\\pipe\\jcode-chrome-bridge-{}", username))
        } else {
            let tmp = std::env::temp_dir();
            tmp.join(format!("jcode-chrome-bridge-{}/host.sock", username))
        };

        Self {
            socket_path,
            running: Arc::new(RwLock::new(false)),
            browser_detected: Arc::new(RwLock::new(None)),
            host_status: Arc::new(RwLock::new(ChromeHostStatus {
                connected: false,
                extension_paired: false,
                device_id: None,
                browser: None,
            })),
            extension_installed: Arc::new(RwLock::new(false)),
        }
    }

    /// 获取 socket path
    pub fn socket_path(&self) -> &Path { &self.socket_path }

    /// 启动 Native Host socket 服务器
    pub async fn start(&self) -> Result<(), String> {
        if *self.running.read().await {
            return Ok(());
        }

        // 确保目录存在
        if let Some(parent) = self.socket_path.parent() {
            std::fs::create_dir_all(parent).map_err(|e| format!("Failed to create socket dir: {}", e))?;
        }

        // 生成 MCP 配置 JSON 文件（供 Chrome 扩展读取）
        let mcp_config = serde_json::json!({
            "serverName": "jcode-browser-bridge",
            "socketPath": self.socket_path.to_string_lossy(),
            "protocol": "mcp"
        });

        let config_dir = dirs::config_dir()
            .unwrap_or_else(|| PathBuf::from("."))
            .join("jcode")
            .join("chrome-bridge");
        std::fs::create_dir_all(&config_dir).map_err(|e| format!("Failed to create config dir: {}", e))?;
        let config_path = config_dir.join("bridge-config.json");
        std::fs::write(&config_path, serde_json::to_string_pretty(&mcp_config).unwrap())
            .map_err(|e| format!("Failed to write config: {}", e))?;

        *self.running.write().await = true;
        info!("Chrome Native Host started at {:?}", self.socket_path);
        Ok(())
    }

    /// 停止 Native Host
    pub async fn stop(&self) {
        *self.running.write().await = false;
        *self.host_status.write().await = ChromeHostStatus {
            connected: false,
            extension_paired: false,
            device_id: None,
            browser: None,
        };
        info!("Chrome Native Host stopped");
    }

    /// 更新扩展连接状态
    pub fn set_extension_paired(&self, device_id: String, browser: String) {
        let mut status = self.host_status.blocking_write();
        status.extension_paired = true;
        status.connected = true;
        status.device_id = Some(device_id);
        status.browser = Some(browser);
        *self.extension_installed.blocking_write() = true;
    }

    /// 获取当前状态
    pub async fn status(&self) -> ChromeHostStatus {
        self.host_status.read().await.clone()
    }

    /// 检查扩展是否安装
    pub async fn is_extension_installed(&self) -> bool {
        *self.extension_installed.read().await
    }
}

// ══════════════════════════════════════════════════════════════════
// 浏览器检测工具
// ═════════════════════════════════════════════════════════════════

/// 检测系统中已安装的 Chromium 浏览器
pub fn detect_browsers() -> Vec<BrowserInfo> {
    let mut found = Vec::new();

    for browser in ChromiumBrowser::all() {
        if let Some(exec_path) = find_browser_executable(browser) {
            found.push(BrowserInfo {
                browser: *browser,
                executable_path: exec_path,
                extension_installed: false,
                native_host_installed: false,
            });
        }
    }

    found
}

/// 检测指定浏览器的可执行文件路径
fn find_browser_executable(browser: &ChromiumBrowser) -> Option<PathBuf> {
    // 常见安装路径按平台
    let candidates: &[&str] = match browser {
        ChromiumBrowser::Chrome => &[
            "/Applications/Google Chrome.app/Contents/MacOS/Google Chrome",       // macOS
            r"C:\Program Files\Google\Chrome\Application\chrome.exe",             // Windows
            "/usr/bin/google-chrome", "/usr/bin/chromium-browser",                // Linux
        ],
        ChromiumBrowser::Brave => &[
            "/Applications/Brave Browser.app/Contents/MacOS/Brave Browser",
            r"C:\Program Files\BraveSoftware\Brave-Browser\Application\brave.exe",
            "/usr/bin/brave-browser",
        ],
        ChromiumBrowser::Arc => &[
            "/Applications/Arc.app/Contents/MacOS/Arc",
            r"C:\Program Files\Arc\Arc.exe",
        ],
        ChromiumBrowser::Edge => &[
            "/Applications/Microsoft Edge.app/Contents/MacOS/Microsoft Edge",
            r"C:\Program Files (x86)\Microsoft\Edge\Application\msedge.exe",
            "/usr/bin/microsoft-edge",
        ],
        ChromiumBrowser::Chromium => &[
            "/Applications/Chromium.app/Contents/MacOS/Chromium",
            r"C:\Program Files\Chromium\Application\chrome.exe",
            "/usr/bin/chromium",
        ],
        ChromiumBrowser::Vivaldi => &[
            "/Applications/Vivaldi.app/Contents/MacOS/Vivaldi",
            r"C:\Program Files\Vivaldi\Application\vivaldi.exe",
            "/usr/bin/vivaldi",
        ],
        ChromiumBrowser::Opera => &[
            "/Applications/Opera.app/Contents/MacOS/Opera",
            r"C:\Program Files\Opera\opera.exe",
            "/usr/bin/opera",
        ],
    };

    for path in candidates {
        let p = PathBuf::from(path);
        if p.exists() {
            return Some(p);
        }
    }
    None
}

// ══════════════════════════════════════════════════════════════════
// MCP 浏览器工具定义
// ═════════════════════════════════════════════════════════════════

/// MCP 浏览器工具定义
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct BrowserToolDefinition {
    pub name: String,
    pub description: String,
    pub input_schema: serde_json::Value,
}

/// 浏览器工具注册表
pub fn browser_tool_definitions() -> Vec<BrowserToolDefinition> {
    vec![
        BrowserToolDefinition {
            name: "navigate".to_string(),
            description: "Navigate to a URL".to_string(),
            input_schema: serde_json::json!({
                "type": "object",
                "properties": { "url": {"type": "string"} },
                "required": ["url"]
            }),
        },
        BrowserToolDefinition {
            name: "read_page".to_string(),
            description: "Read the current page content".to_string(),
            input_schema: serde_json::json!({
                "type": "object",
                "properties": {}
            }),
        },
        BrowserToolDefinition {
            name: "find".to_string(),
            description: "Find text on the page".to_string(),
            input_schema: serde_json::json!({
                "type": "object",
                "properties": {
                    "text": {"type": "string"},
                    "case_sensitive": {"type": "boolean", "default": false}
                },
                "required": ["text"]
            }),
        },
        BrowserToolDefinition {
            name: "form_input".to_string(),
            description: "Input text into a form field by selector".to_string(),
            input_schema: serde_json::json!({
                "type": "object",
                "properties": {
                    "selector": {"type": "string"},
                    "value": {"type": "string"}
                },
                "required": ["selector", "value"]
            }),
        },
        BrowserToolDefinition {
            name: "click".to_string(),
            description: "Click an element on the page".to_string(),
            input_schema: serde_json::json!({
                "type": "object",
                "properties": {"selector": {"type": "string"}},
                "required": ["selector"]
            }),
        },
    ]
}
