//! # Vim — Vim 模式（借鉴 Claude Code vim/ 目录）
//!
//! 在 TUI 中启用 Vim 风格键绑定和模式编辑。
//! 支持 Normal/Insert/Visual/Command 四种模式。

use std::sync::Arc;
use tokio::sync::RwLock;

/// Vim 模式
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum VimMode {
    Normal,
    Insert,
    Visual,
    Command,
}

/// Vim 配置
#[derive(Debug, Clone)]
pub struct VimConfig {
    /// 是否启用 Vim 模式
    pub enabled: bool,
    /// 是否启用相对行号
    pub relativenumber: bool,
    /// 是否启用语法高亮
    pub syntax_highlight: bool,
    /// 是否启用鼠标支持
    pub mouse_support: bool,
    /// Tab 宽度
    pub tab_width: u8,
    /// 是否自动缩进
    pub auto_indent: bool,
}

impl Default for VimConfig {
    fn default() -> Self {
        Self {
            enabled: false,
            relativenumber: true,
            syntax_highlight: true,
            mouse_support: true,
            tab_width: 4,
            auto_indent: true,
        }
    }
}

/// Vim 状态管理器
pub struct VimManager {
    state: Arc<RwLock<VimConfig>>,
    mode: Arc<RwLock<VimMode>>,
}

impl VimManager {
    pub fn new() -> Self {
        Self {
            state: Arc::new(RwLock::new(VimConfig::default())),
            mode: Arc::new(RwLock::new(VimMode::Normal)),
        }
    }

    /// 获取当前模式
    pub async fn current_mode(&self) -> VimMode {
        *self.mode.read().await
    }

    /// 切换模式
    pub async fn set_mode(&self, mode: VimMode) {
        let mut m = self.mode.write().await;
        *m = mode;
    }

    /// 启用/禁用 Vim 模式
    pub async fn toggle(&self) -> bool {
        let mut state = self.state.write().await;
        state.enabled = !state.enabled;
        state.enabled
    }

    /// 处理按键事件
    pub async fn handle_key(&self, key: &str) -> Option<String> {
        let mode = *self.mode.read().await;
        match mode {
            VimMode::Normal => self.handle_normal_mode(key),
            VimMode::Insert => None, // 透传
            VimMode::Visual => self.handle_visual_mode(key),
            VimMode::Command => self.handle_command_mode(key),
        }
    }

    fn handle_normal_mode(&self, key: &str) -> Option<String> {
        match key {
            "i" => { self.mode.try_write().map(|mut m| *m = VimMode::Insert); None }
            "v" => { self.mode.try_write().map(|mut m| *m = VimMode::Visual); None }
            ":" => { self.mode.try_write().map(|mut m| *m = VimMode::Command); None }
            "u" => Some("undo".into()),
            "dd" => Some("delete_line".into()),
            "yy" => Some("yank_line".into()),
            "p" => Some("paste".into()),
            _ => None,
        }
    }

    fn handle_visual_mode(&self, _key: &str) -> Option<String> {
        None
    }

    fn handle_command_mode(&self, key: &str) -> Option<String> {
        match key {
            "w" => { self.mode.try_write().map(|mut m| *m = VimMode::Normal); Some("write".into()) }
            "q" => { self.mode.try_write().map(|mut m| *m = VimMode::Normal); Some("quit".into()) }
            "wq" => { self.mode.try_write().map(|mut m| *m = VimMode::Normal); Some("write_quit".into()) }
            _ => None,
        }
    }

    /// 获取 Vim 模式的状态提示符
    pub fn mode_prompt(&self) -> String {
        let mode = self.mode.try_read().map(|m| *m).unwrap_or(VimMode::Normal);
        match mode {
            VimMode::Normal => "-- NORMAL --",
            VimMode::Insert => "-- INSERT --",
            VimMode::Visual => "-- VISUAL --",
            VimMode::Command => ":",
        }.to_string()
    }
}
