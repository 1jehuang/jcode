//! # Buddy — 桌面伴侣应用（借鉴 Claude Code buddy/ 目录）
//!
//! 提供系统托盘驻留、Git 钩子通知、跨 IDE 协作等功能。
//! 作为 CarpAI Server 的桌面守护进程运行。

use serde::{Deserialize, Serialize};

/// Buddy 配置
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct BuddyConfig {
    /// 是否开机自启
    pub auto_start: bool,
    /// 是否显示系统托盘图标
    pub show_tray: bool,
    /// 通知类型
    pub notifications: NotificationConfig,
    /// Git 钩子路径
    pub git_hooks_dir: Option<String>,
    /// IDE 集成端口
    pub ide_port: u16,
}

impl Default for BuddyConfig {
    fn default() -> Self {
        Self {
            auto_start: false,
            show_tray: true,
            notifications: NotificationConfig::default(),
            git_hooks_dir: None,
            ide_port: 0,
        }
    }
}

/// 通知配置
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct NotificationConfig {
    /// 代码审查完成通知
    pub review_complete: bool,
    /// 构建完成通知
    pub build_complete: bool,
    /// 任务完成通知
    pub task_complete: bool,
    /// 错误通知
    pub errors: bool,
}

impl Default for NotificationConfig {
    fn default() -> Self {
        Self {
            review_complete: true,
            build_complete: true,
            task_complete: true,
            errors: true,
        }
    }
}

/// Buddy 管理器
pub struct BuddyManager {
    config: std::sync::RwLock<BuddyConfig>,
}

impl BuddyManager {
    pub fn new() -> Self {
        Self {
            config: std::sync::RwLock::new(BuddyConfig::default()),
        }
    }

    /// 初始化系统托盘
    #[cfg(target_os = "windows")]
    pub fn init_tray(&self) -> Result<(), String> {
        // Windows 系统托盘启用
        Ok(())
    }

    /// 初始化系统托盘
    #[cfg(target_os = "macos")]
    pub fn init_tray(&self) -> Result<(), String> {
        // macOS 菜单栏启用
        Ok(())
    }

    /// 初始化系统托盘
    #[cfg(not(any(target_os = "windows", target_os = "macos")))]
    pub fn init_tray(&self) -> Result<(), String> {
        // Linux 系统托盘启用
        Ok(())
    }

    /// 发送桌面通知
    pub fn notify(&self, title: &str, message: &str) {
        let _ = (title, message);
        #[cfg(target_os = "windows")]
        { /* Windows 通知 API */ }
        #[cfg(target_os = "macos")]
        { /* macOS 通知 */ }
    }
}
