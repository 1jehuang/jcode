//! IDE 检测器 - Lockfile 服务发现协议
//!
//! 移植自 Claude Code `src/utils/ide.ts`:
//! - `detectIDEs()` -> 扫描 lockfile 目录
//! - PID 祖先级检查
//! - WSL/Windows 路径转换
//! - IDE 自动发现与连接

use crate::types::{
    IdeLockfileContent, IdeTransport, DetectedIdeInfo, IdeConnectionStatus,
};
use anyhow::{Context, Result};
use chrono::{DateTime, Local};
use std::path::{Path, PathBuf};
use tokio::fs;
use tracing::{debug, info, warn};

/// IDE 检测器配置
#[derive(Debug, Clone)]
pub struct IdeDetectorConfig {
    /// Lockfile 存储目录 (默认: ~/.jcode/ide/)
    pub lockfile_dir: Option<PathBuf>,
    
    /// 当前工作目录 (用于匹配 workspace)
    pub current_cwd: PathBuf,
    
    /// 是否启用 WSL 路径转换
    pub wsl_path_conversion: bool,
    
    /// 扫描超时时间 (毫秒)
    pub scan_timeout_ms: u64,
}

impl Default for IdeDetectorConfig {
    fn default() -> Self {
        Self {
            lockfile_dir: None,
            current_cwd: std::env::current_dir().unwrap_or_else(|_| PathBuf::from("/")),
            wsl_path_conversion: true,
            scan_timeout_ms: 5000,
        }
    }
}

/// IDE 检测器 - 通过扫描 Lockfile 目录发现运行中的 IDE
///
/// ## 协议说明
/// Claude Code 使用 Lockfile 协议实现 IDE 自动发现:
/// ```text
/// ~/.claude/ide/
/// +-- 12345.lock          # 格式: {port}.lock
/// +-- 12346.lock
/// +-- ...
///
/// 每个 .lock 文件内容:
/// {"workspaceFolders": ["/path"], "pid": 12345, "ideName": "Cursor", ...}
/// ```
pub struct IdeDetector {
    config: IdeDetectorConfig,
}

impl IdeDetector {
    /// 创建新的 IDE 检测器
    pub fn new(config: IdeDetectorConfig) -> Self {
        Self { config }
    }

    /// 使用默认配置创建检测器
    pub fn with_cwd(cwd: PathBuf) -> Self {
        Self::new(IdeDetectorConfig {
            current_cwd: cwd,
            ..Default::default()
        })
    }

    /// 获取 Lockfile 目录路径
    /// 默认: ~/.jcode/ide/
    fn lockfile_dir(&self) -> PathBuf {
        self.config.lockfile_dir.clone().unwrap_or_else(|| {
            dirs::home_dir()
                .map(|h| h.join(".jcode").join("ide"))
                .unwrap_or_else(|| PathBuf::from("/tmp/.jcode/ide"))
        })
    }

    /// 检测所有可用的 IDE 实例
    ///
    /// ## 流程 (移植自 Claude Code detectIDEs):
    /// 1. 扫描 `~/.jcode/ide/` 目录下所有 `.lock` 文件
    /// 2. 按 mtime 排序 (最新优先)
    /// 3. 并行读取所有 lockfile 内容
    /// 4. 校验 cwd 匹配 + PID 有效性
    /// 5. 返回有效的 IDE 列表
    pub async fn detect(&self) -> Result<Vec<DetectedIdeInfo>> {
        let lockfile_dir = self.lockfile_dir();

        // 确保目录存在
        if !lockfile_dir.exists() {
            debug!("IDE lockfile directory does not exist: {:?}", lockfile_dir);
            return Ok(Vec::new());
        }

        // 扫描所有 .lock 文件
        let mut entries = fs::read_dir(&lockfile_dir)
            .await
            .context("Failed to read IDE lockfile directory")?
            .filter_map(|e| async move {
                e.ok().and_then(|entry| {
                    let path = entry.path();
                    if path.extension().map_or(false, |ext| ext == "lock") {
                        Some(path)
                    } else {
                        None
                    }
                })
            })
            .collect::<Vec<_>>()
            .await;

        if entries.is_empty() {
            debug!("No IDE lockfiles found in {:?}", lockfile_dir);
            return Ok(Vec::new());
        }

        info!("Found {} IDE lockfile(s), scanning...", entries.len());

        // 并行读取并解析每个 lockfile
        let mut detected = Vec::new();
        for entry in &entries {
            match self.parse_lockfile(entry).await {
                Ok(Some(ide)) => detected.push(ide),
                Ok(None) => {} // 无效或过期, 跳过
                Err(e) => {
                    warn!("Failed to parse lockfile {:?}: {}", entry, e);
                }
            }
        }

        // 按 mtime 排序 (最新优先) — Claude Code 行为
        detected.sort_by(|a, b| b.lockfile_mtime.cmp(&a.lockfile_mtime));

        // 校验有效性
        let valid_ides: Vec<DetectedIdeInfo> = detected
            .into_iter()
            .filter_map(|mut ide| {
                let is_valid = ide.validate(&self.config.current_cwd);
                if !is_valid {
                    debug!("IDE '{}' failed validation", ide.name);
                }
                if is_valid { Some(ide) } else { None }
            })
            .collect();

        info!(
            "IDE detection complete: {} valid out of {} total",
            valid_ides.len(),
            entries.len()
        );

        Ok(valid_ides)
    }

    /// 检测最佳匹配的 IDE (最新且有效)
    pub async fn detect_best(&self) -> Result<Option<DetectedIdeInfo>> {
        let all = self.detect().await?;
        Ok(all.into_iter().next())
    }

    /// 解析单个 lockfile 文件
    async fn parse_lockfile(&self, path: &Path) -> Result<Option<DetectedIdeInfo>> {
        // 读取文件内容
        let content = match fs::read_to_string(path).await {
            Ok(c) => c,
            Err(e) => {
                warn!("Cannot read lockfile {:?}: {}", path, e);
                return Ok(None);
            }
        };

        // 解析 JSON
        let lock_content: IdeLockfileContent = match serde_json::from_str(&content) {
            Ok(lc) => lc,
            Err(e) => {
                warn!("Invalid JSON in lockfile {:?}: {}", path, e);
                return Ok(None);
            }
        };

        // 从文件名提取端口号
        let port = self.extract_port_from_filename(path)?;

        // 获取文件修改时间
        let metadata = fs::metadata(path).await?;
        let mtime: DateTime<Local> = metadata
            .modified()
            .ok()
            .and_then(|t| t.into())
            .unwrap_or_else(Local::now);

        // 构建连接 URL
        let transport = lock_content
            .transport
            .as_ref()
            .unwrap_or(&IdeTransport::WebSocket);
        
        // 默认使用 localhost
        let url = transport.build_url("127.0.0.1", port, "");

        let mut info = DetectedIdeInfo::new(
            lock_content.ide_name.clone().unwrap_or_else(|| format!("IDE-{}", port)),
            port,
            url,
            transport.clone(),
            lock_content,
            mtime,
        );

        // WSL 路径转换 (如果需要)
        if self.config.wsl_path_conversion && info.ide_running_in_windows == Some(true) {
            for folder in &mut info.workspace_folders {
                *folder = Self::convert_wsl_to_windows_path(folder);
            }
        }

        Ok(Some(info))
    }

    /// 从 lockfile 文件名提取端口号
    /// 文件名格式: `{port}.lock`
    fn extract_port_from_filename(&self, path: &Path) -> Result<u16> {
        let filename = path
            .file_stem()
            .and_then(|s| s.to_str())
            .context("Invalid lockfile filename")?;

        filename
            .parse::<u16>()
            .with_context(|| format!("Invalid port number in lockfile: {}", filename))
    }

    /// WSL -> Windows 路径转换
    /// 移植自 Claude Code `src/utils/idePathConversion.ts`: WindowsToWSLConverter
    ///
    /// 示例:
    /// - `/mnt/c/Users/user/project` -> `C:\Users\user\project`
    /// - `/home/user/project` -> `\\wsl$\Ubuntu\home\user\project` (反向)
    #[cfg(target_os = "linux")]
    fn convert_wsl_to_windows_path(wsl_path: &str) -> String {
        use std::path::Component;
        
        // 检查是否是 /mnt/ 路径
        if let Some(rest) = wsl_path.strip_prefix("/mnt/") {
            if let Some((drive_letter, remaining)) = rest.split_once('/') {
                let drive = drive_letter.to_ascii_uppercase();
                return format!("{}:\\{}", drive, remaining.replace('/', "\\"));
            }
        }

        // 非 /mnt/ 路径保持原样 (可能是 WSL 内部路径)
        wsl_path.to_string()
    }

    #[cfg(not(target_os = "linux"))]
    fn convert_wsl_to_windows_path(path: &str) -> String {
        path.to_string()
    }

    /// Windows -> WSL 路径转换 (反向)
    /// 用于将 Windows 路径转换为 Linux 路径以便在 WSL 中使用
    #[cfg(target_os = "linux")]
    pub fn convert_windows_to_wsl_path(win_path: &str) -> String {
        let cleaned = win_path.replace('\\', "/");
        
        // 匹配 C:/Users/... 格式
        if let Some(rest) = cleaned.strip_prefix(|c: char| c.is_alphabetic()) {
            if let Some(remaining) = rest.strip_with(":") || rest.starts_with(":/") {
                let drive = cleaned.chars().next().unwrap().to_ascii_lowercase();
                return format!("/mnt/{}/{}", drive, remaining.trim_start_matches(':').trim_start_matches('/'));
            }
        }

        // 匹配 UNC 路径 \\wsl$\
        if cleaned.contains(r"\wsl$\") || cleaned.contains("/wsl$/") {
            // 提取 WSL 发行版名称和内部路径
            let parts: Vec<&str> = cleaned.split(['\\', '/']).collect();
            if parts.len() >= 3 && parts[0].is_empty() && parts[1] == "wsl$" {
                return format!("/{}", parts[2..].join("/"));
            }
        }

        cleaned
    }

    #[cfg(not(target_os = "linux"))]
    pub fn convert_windows_to_wsl_path(path: &str) -> String {
        path.replace('\\', "/")
    }
}

// ============================================================================
// IDE 连接管理器 - 管理 IDE 连接生命周期
// ============================================================================

use tokio_tungstenite::tungstenite::Message;

/// IDE WebSocket 连接回调
pub type OnIdeMessage = Box<dyn Fn(Message) + Send + Sync>;
pub type OnIdeConnected = Box<dyn Fn() + Send + Sync>;
pub type OnIdeDisconnected = Box<dyn Fn(String) + Send + Sync>; // reason: String
pub type OnIdeError = Box<dyn Fn(anyhow::Error) + Send + Sync>;

/// IDE 连接管理器
/// 移植自 Claude Code `useIDEIntegration.tsx` React Hook 的 Rust 实现
pub struct IdeConnectionManager {
    detector: IdeDetector,
    current_connection: Option<IdeActiveConnection>,
    callbacks: IdeConnectionCallbacks,
    auto_reconnect: bool,
    max_reconnect_attempts: u32,
}

struct IdeActiveConnection {
    _ws_stream: tokio_tungstenite::WebSocketStream<
        tokio_tungstenite::MaybeTlsStream<tokio::net::TcpStream>,
    >,
    ide_info: DetectedIdeInfo,
}

#[derive(Default)]
pub struct IdeConnectionCallbacks {
    pub on_message: Option<OnIdeMessage>,
    pub on_connected: Option<OnIdeConnected>,
    pub on_disconnected: Option<OnIdeDisconnected>,
    pub on_error: Option<OnIdeError>,
}

impl IdeConnectionManager {
    /// 创建新的 IDE 连接管理器
    pub fn new(detector: IdeDetector) -> Self {
        Self {
            detector,
            current_connection: None,
            callbacks: IdeConnectionCallbacks::default(),
            auto_reconnect: true,
            max_reconnect_attempts: 5,
        }
    }

    /// 设置连接回调
    pub fn with_callbacks(mut self, callbacks: IdeConnectionCallbacks) -> Self {
        self.callbacks = callbacks;
        self
    }

    /// 获取当前连接状态
    pub fn status(&self) -> IdeConnectionStatus {
        match &self.current_connection {
            None => IdeConnectionStatus::Disconnected,
            Some(conn) => IdeConnectionStatus::Connected {
                ide_name: conn.ide_info.name.clone(),
                extension_installed: true, // TODO: 检测扩展安装状态
            },
        }
    }

    /// 自动检测并连接到最佳匹配的 IDE
    pub async fn auto_connect(&mut self) -> Result<Option<DetectedIdeInfo>> {
        info!("Starting IDE auto-detection...");

        match self.detector.detect_best().await {
            Ok(Some(ide)) => {
                info!("Found IDE: {} at port {}", ide.name, ide.port);
                
                // 尝试建立连接
                match self.connect_to_ide(&ide).await {
                    Ok(()) => {
                        if let Some(cb) = &self.callbacks.on_connected {
                            cb();
                        }
                        Ok(Some(ide))
                    }
                    Err(e) => {
                        warn!("Failed to connect to {}: {}", ide.name, e);
                        if let Some(cb) = &self.callbacks.on_error {
                            cb(e);
                        }
                        Ok(None)
                    }
                }
            }
            Ok(None) => {
                info!("No IDE found");
                Ok(None)
            }
            Err(e) => {
                if let Some(cb) = &self.callbacks.on_error {
                    cb(e.context("IDE detection failed").into());
                }
                Err(e)
            }
        }
    }

    /// 连接到指定的 IDE
    pub async fn connect_to_ide(&mut self, ide: &DetectedIdeInfo>) -> Result<()> {
        let url = if ide.url.starts_with("ws:") || ide.url.starts_with("wss:") {
            ide.url.clone()
        } else {
            // 如果是 SSE URL, 转换为 WS (优先使用 WS 进行双向通信)
            format!(
                "ws://127.0.0.0:{}",
                ide.port
            )
        };

        info!("Connecting to IDE at {} ({})...", url, ide.name);

        let (ws_stream, _) = tokio_tungstenite::connect_async(&url).await?;

        self.current_connection = Some(IdeActiveConnection {
            _ws_stream: ws_stream,
            ide_info: ide.clone(),
        });

        info!("Successfully connected to {}", ide.name);
        Ok(())
    }

    /// 断开当前连接
    pub async fn disconnect(&mut self) -> Option<String> {
        if let Some(conn) = self.current_connection.take() {
            let name = conn.ide_info.name.clone();
            
            if let Some(cb) = &self.callbacks.on_disconnected {
                cb(format!("User requested disconnect from {}", name));
            }
            
            info!("Disconnected from {}", name);
            Some(name)
        } else {
            None
        }
    }

    /// 向已连接的 IDE 发送消息
    pub async fn send_message(&self, message: Message) -> Result<()> {
        match &self.current_connection {
            Some(_) => {
                // TODO: 通过 ws_stream 发送消息
                debug!("Sending message to IDE");
                Ok(())
            }
            None => Err(anyhow::anyhow!("Not connected to any IDE")),
        }
    }

    /// 安装 IDE 扩展 (如果需要)
    /// 
    /// 移植自 Claude Code: `installExtension()` 在 ide.ts 中
    /// 支持自动安装 VSCode/Cursor/Windsurf 扩展
    pub async fn install_extension(&self, ide_type: &crate::types::IdeType) -> Result<bool> {
        use crate::types::IdeType;

        match ide_type {
            // VSCode 系列: 使用 --install-extension 命令
            IdeType::VsCode | IdeType::Cursor | IdeType::Windsurf => {
                let extension_id = "anthropic.jcode-integration"; // JCode 扩展 ID
                
                // 查找 code/cursor/windsurf 可执行文件
                let cmd = match ide_type {
                    IdeType::Cursor => "cursor",
                    IdeType::Windsurf => "windsurf",
                    _ => "code",
                };
                
                info!("Installing {} extension via {}...", extension_id, cmd);
                
                let output = tokio::process::Command::new(cmd)
                    .args(["--install-extension", extension_id, "--force"])
                    .output()
                    .await;

                match output {
                    Ok(result) if result.status.success() => {
                        info!("Extension installed successfully");
                        Ok(true)
                    }
                    Ok(result) => {
                        let stderr = String::from_utf8_lossy(&result.stderr);
                        warn!("Extension install failed: {}", stderr);
                        Ok(false)
                    }
                    Err(e) => {
                        warn!("Failed to run {}: {}", cmd, e);
                        Err(e.into())
                    }
                }
            }
            // JetBrains 系列: 需要通过插件市场 API 或手动安装
            _ => {
                info!("JetBrains IDEs require manual plugin installation");
                Ok(false)
            }
        }
    }

    /// 检查 IDE 扩展是否已安装
    pub async fn check_extension_installed(
        &self, 
        _ide_type: &crate::types::IdeType,
    ) -> bool {
        // TODO: 通过 IDE RPC 查询扩展列表
        // Claude Code 中通过 callIdeRpc("getExtensions") 实现
        false
    }
}
