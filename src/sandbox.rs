//! # Cloud Sandbox — Gvisor/沙箱集成
//!
//! 从 Claude Code 移植的沙箱系统，提供：
//! - SandboxManager: 沙箱生命周期管理
//! - SandboxConfig: 文件系统/网络隔离配置
//! - 命令沙箱包装: 与 bash tool 集成
//! - 依赖检查: bubblewrap/socat 等

use serde::{Deserialize, Serialize};
use std::collections::HashSet;
use std::sync::Arc;
use tokio::sync::RwLock;
use tracing::{info, warn};

// ══════════════════════════════════════════════════════════════════
// 沙箱配置
// ═════════════════════════════════════════════════════════════════

/// 沙箱文件系统配置
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct SandboxFilesystemConfig {
    /// 允许写入的路径
    #[serde(default)]
    pub allow_write_paths: Vec<String>,
    /// 禁止写入的路径
    #[serde(default)]
    pub deny_write_paths: Vec<String>,
    /// 禁止读取的路径
    #[serde(default)]
    pub deny_read_paths: Vec<String>,
    /// 允许读取的路径
    #[serde(default)]
    pub allow_read_paths: Vec<String>,
    /// 是否仅允许受管读取路径
    #[serde(default)]
    pub allow_managed_read_paths_only: bool,
}

impl Default for SandboxFilesystemConfig {
    fn default() -> Self {
        Self {
            allow_write_paths: vec![],
            deny_write_paths: vec![".git/".to_string()],
            deny_read_paths: vec!["/etc/shadow".to_string(), "/etc/ssh/".to_string()],
            allow_read_paths: vec![],
            allow_managed_read_paths_only: false,
        }
    }
}

/// 沙箱网络配置
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct SandboxNetworkConfig {
    /// 允许访问的域名列表
    #[serde(default)]
    pub allowed_domains: Vec<String>,
    /// 禁止访问的域名列表
    #[serde(default)]
    pub denied_domains: Vec<String>,
    /// 是否允许 Unix Socket
    #[serde(default = "default_true")]
    pub allow_unix_sockets: bool,
    /// 是否允许本地绑定
    #[serde(default)]
    pub allow_local_binding: bool,
}

fn default_true() -> bool { true }

impl Default for SandboxNetworkConfig {
    fn default() -> Self {
        Self {
            allowed_domains: vec![],
            denied_domains: vec![],
            allow_unix_sockets: true,
            allow_local_binding: false,
        }
    }
}

/// 完整沙箱设置
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct SandboxConfig {
    /// 是否启用沙箱
    #[serde(default)]
    pub enabled: bool,
    /// 沙箱不可用时是否报错退出
    #[serde(default)]
    pub fail_if_unavailable: bool,
    /// 沙箱启用时自动允许 bash
    #[serde(default)]
    pub auto_allow_bash_if_sandboxed: bool,
    /// 允许无沙箱运行命令
    #[serde(default)]
    pub allow_unsandboxed_commands: bool,
    /// 排除的命令列表（不运行沙箱）
    #[serde(default)]
    pub excluded_commands: Vec<String>,
    /// 忽略违规
    #[serde(default)]
    pub ignore_violations: bool,
    /// 文件系统配置
    #[serde(default)]
    pub filesystem: SandboxFilesystemConfig,
    /// 网络配置
    #[serde(default)]
    pub network: SandboxNetworkConfig,
}

impl Default for SandboxConfig {
    fn default() -> Self {
        Self {
            enabled: false,
            fail_if_unavailable: false,
            auto_allow_bash_if_sandboxed: false,
            allow_unsandboxed_commands: false,
            excluded_commands: vec!["git".to_string(), "which".to_string(), "echo".to_string()],
            ignore_violations: false,
            filesystem: SandboxFilesystemConfig::default(),
            network: SandboxNetworkConfig::default(),
        }
    }
}

// ══════════════════════════════════════════════════════════════════
// 沙箱违规记录
// ═════════════════════════════════════════════════════════════════

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct SandboxViolation {
    pub violation_type: SandboxViolationType,
    pub command: String,
    pub message: String,
    pub timestamp: chrono::DateTime<chrono::Utc>,
    pub allowed: bool,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Serialize, Deserialize)]
pub enum SandboxViolationType {
    FilesystemWrite,
    FilesystemRead,
    NetworkConnect,
    NetworkBind,
    ProcessSpawn,
    NestedSandbox,
}

// ══════════════════════════════════════════════════════════════════
// 沙箱管理器
// ═════════════════════════════════════════════════════════════════

/// 沙箱依赖检查结果
#[derive(Debug, Clone)]
pub struct SandboxDependencyStatus {
    pub bubblewrap_available: bool,
    pub socat_available: bool,
    pub ripgrep_available: bool,
    pub seatbelt_available: bool,
    pub error_message: Option<String>,
}

impl SandboxDependencyStatus {
    /// 所有必须依赖是否可用
    pub fn all_available(&self) -> bool {
        #[cfg(target_os = "macos")]
        { self.seatbelt_available }
        #[cfg(not(target_os = "macos"))]
        { self.bubblewrap_available && self.socat_available }
    }
}

/// ISandboxManager trait — 沙箱管理器接口
pub trait ISandboxManager: Send + Sync {
    /// 初始化沙箱
    fn initialize(&self) -> impl std::future::Future<Output = Result<(), String>> + Send;
    /// 检查沙箱是否启用
    fn is_sandboxing_enabled(&self) -> bool;
    /// 用沙箱包装命令
    fn wrap_with_sandbox(&self, command: &str, args: &[&str]) -> impl std::future::Future<Output = Result<(String, Vec<String>), String>> + Send;
    /// 检查依赖
    fn check_dependencies(&self) -> impl std::future::Future<Output = SandboxDependencyStatus> + Send;
    /// 刷新配置
    fn refresh_config(&self, config: SandboxConfig) -> impl std::future::Future<Output = ()> + Send;
    /// 重置沙箱
    fn reset(&self) -> impl std::future::Future<Output = Result<(), String>> + Send;
    /// 记录违规
    fn record_violation(&self, violation: SandboxViolation);
    /// 获取违规列表
    fn get_violations(&self) -> Vec<SandboxViolation>;
}

/// 沙箱管理器 — 默认实现
pub struct SandboxManager {
    config: Arc<RwLock<SandboxConfig>>,
    violations: Arc<RwLock<Vec<SandboxViolation>>>,
    initialized: Arc<RwLock<bool>>,
    /// 排除命令集合（快速查找）
    excluded_cmds: Arc<RwLock<HashSet<String>>>,
}

impl SandboxManager {
    pub fn new(config: SandboxConfig) -> Self {
        let excluded: HashSet<String> = config.excluded_commands.iter().cloned().collect();
        Self {
            config: Arc::new(RwLock::new(config)),
            violations: Arc::new(RwLock::new(Vec::new())),
            initialized: Arc::new(RwLock::new(false)),
            excluded_cmds: Arc::new(RwLock::new(excluded)),
        }
    }

    /// 决定是否应对命令使用沙箱
    pub fn should_use_sandbox(&self, command: &str) -> bool {
        if !self.is_sandboxing_enabled() {
            return false;
        }
        // 检查排除命令
        let excluded = self.excluded_cmds.blocking_read();
        if excluded.contains(command) {
            return false;
        }
        true
    }

    /// 检查命令是否为排除命令（支持前缀和通配符）
    pub fn contains_excluded_command(&self, command: &str) -> bool {
        let excluded = self.excluded_cmds.blocking_read();
        if excluded.contains(command) {
            return true;
        }
        // 前缀匹配
        for excl in excluded.iter() {
            if command.starts_with(excl) {
                return true;
            }
        }
        // 通配符匹配（简化版）
        for excl in excluded.iter() {
            if excl.contains('*') {
                let pattern = excl.replace('*', ".*");
                if let Ok(re) = regex::Regex::new(&pattern) {
                    if re.is_match(command) {
                        return true;
                    }
                }
            }
        }
        false
    }
}

impl ISandboxManager for SandboxManager {
    async fn initialize(&self) -> Result<(), String> {
        let deps = self.check_dependencies().await;
        if !deps.all_available() {
            let msg = deps.error_message.unwrap_or_else(|| "Sandbox dependencies not available".to_string());
            let config = self.config.read().await;
            if config.fail_if_unavailable {
                return Err(msg);
            }
            warn!("Sandbox dependencies not available: {}", msg);
        }
        *self.initialized.write().await = true;
        info!("Sandbox initialized");
        Ok(())
    }

    fn is_sandboxing_enabled(&self) -> bool {
        let config = futures::executor::block_on(self.config.read());
        if !config.enabled {
            return false;
        }
        if !config.allow_unsandboxed_commands {
            return true;
        }
        config.enabled
    }

    async fn wrap_with_sandbox(&self, command: &str, args: &[&str]) -> Result<(String, Vec<String>), String> {
        if !self.should_use_sandbox(command) {
            return Ok((command.to_string(), args.iter().map(|s| s.to_string()).collect()));
        }

        if cfg!(target_os = "macos") {
            // macOS: 使用 seatbelt (sandbox-exec)
            let mut sbox_args = vec![
                "-n".to_string(),
                "-f".to_string(),
                "/tmp/jcode-sandbox.sb".to_string(),
            ];
            sbox_args.push(command.to_string());
            sbox_args.extend(args.iter().map(|s| s.to_string()));
            Ok(("sandbox-exec".to_string(), sbox_args))
        } else {
            // Linux/Unix: 使用 bubblewrap (bwrap)
            let config = self.config.read().await;
            let mut bwrap_args = vec![
                "--unshare-all".to_string(),
                "--new-session".to_string(),
                "--ro-bind".to_string(), "/usr".to_string(), "/usr".to_string(),
                "--ro-bind".to_string(), "/lib".to_string(), "/lib".to_string(),
                "--ro-bind".to_string(), "/bin".to_string(), "/bin".to_string(),
                "--proc".to_string(), "/proc".to_string(),
                "--dev".to_string(), "/dev".to_string(),
                "--tmpfs".to_string(), "/tmp".to_string(),
            ];

            if let Ok(cwd) = std::env::current_dir() {
                let cwd_str = cwd.to_string_lossy().to_string();
                bwrap_args.push("--bind".to_string());
                bwrap_args.push(cwd_str.clone());
                bwrap_args.push(cwd_str);
            }

            if !config.network.allow_unix_sockets {
                bwrap_args.push("--unshare-net".to_string());
            }

            for path in &config.filesystem.deny_write_paths {
                bwrap_args.push("--ro-bind".to_string());
                bwrap_args.push(path.clone());
                bwrap_args.push(path.clone());
            }

            for path in &config.filesystem.allow_write_paths {
                bwrap_args.push("--bind".to_string());
                bwrap_args.push(path.clone());
                bwrap_args.push(path.clone());
            }

            bwrap_args.push(command.to_string());
            bwrap_args.extend(args.iter().map(|s| s.to_string()));

            Ok(("bwrap".to_string(), bwrap_args))
        }
    }

    async fn check_dependencies(&self) -> SandboxDependencyStatus {
        let mut status = SandboxDependencyStatus {
            bubblewrap_available: false,
            socat_available: false,
            ripgrep_available: false,
            seatbelt_available: false,
            error_message: None,
        };

        #[cfg(target_os = "macos")]
        {
            status.seatbelt_available = which_cmd("sandbox-exec");
        }

        #[cfg(not(target_os = "macos"))]
        {
            status.bubblewrap_available = which_cmd("bwrap");
            status.socat_available = which_cmd("socat");
        }

        status.ripgrep_available = which_cmd("rg");

        if !status.all_available() {
            status.error_message = Some(format!(
                "Missing sandbox dependencies. Required: bwrap/sandbox-exec, socat. Available: bwrap={}, socat={}, seatbelt={}, rg={}",
                status.bubblewrap_available, status.socat_available, status.seatbelt_available, status.ripgrep_available
            ));
        }

        status
    }

    async fn refresh_config(&self, config: SandboxConfig) {
        let excluded: HashSet<String> = config.excluded_commands.iter().cloned().collect();
        *self.excluded_cmds.write().await = excluded;
        *self.config.write().await = config;
        info!("Sandbox config refreshed");
    }

    async fn reset(&self) -> Result<(), String> {
        *self.initialized.write().await = false;
        self.violations.write().await.clear();
        self.initialize().await
    }

    fn record_violation(&self, violation: SandboxViolation) {
        self.violations.blocking_write().push(violation);
    }

    fn get_violations(&self) -> Vec<SandboxViolation> {
        self.violations.blocking_read().clone()
    }
}

/// 运行时可读的沙箱状态摘要
#[derive(Debug, Clone, Serialize)]
pub struct SandboxStatus {
    pub enabled: bool,
    pub initialized: bool,
    pub dependencies_ok: bool,
    pub violations_count: usize,
    pub excluded_commands: Vec<String>,
}

fn which_cmd(name: &str) -> bool {
    std::process::Command::new("which")
        .arg(name)
        .output()
        .map(|o| o.status.success())
        .unwrap_or(false)
}
