//! Hook系统 - 事件驱动架构
//!
//! 对标: Claude Code `src/hooks/` 104个文件
//!
//! Hook类型:
//! - File Hooks: 文件打开/保存/关闭
//! - Editor Hooks: 光标移动/选择变化
//! - Tool Hooks: 工具执行前/后
//! - Session Hooks: 会话开始/结束
//! - Git Hooks: 提交/推送/合并

pub mod file_hooks;
pub mod editor_hooks;
pub mod tool_hooks;
pub mod session_hooks;
pub mod git_hooks;

use std::sync::Arc;
use tokio::sync::RwLock;
use serde::{Deserialize, Serialize};

/// Hook事件类型
#[derive(Debug, Clone, Serialize, Deserialize)]
pub enum HookEvent {
    // File events
    FileOpened { path: String },
    FileSaved { path: String },
    FileClosed { path: String },

    // Editor events
    CursorMoved { line: u32, col: u32 },
    SelectionChanged { start: (u32, u32), end: (u32, u32) },

    // Tool events
    ToolBeforeExecute { tool_name: String, args: serde_json::Value },
    ToolAfterExecute { tool_name: String, result: serde_json::Value },

    // Session events
    SessionStarted { session_id: String },
    SessionEnded { session_id: String },

    // Git events
    GitPreCommit,
    GitPostCommit,
    GitPrePush,
    GitPostPush,
}

/// Hook处理器trait
#[async_trait::async_trait]
pub trait HookHandler: Send + Sync {
    /// 处理Hook事件
    async fn handle(&self, event: &HookEvent) -> anyhow::Result<()>;

    /// Hook名称
    fn name(&self) -> &str;

    /// 优先级（数字越小优先级越高）
    fn priority(&self) -> u32 {
        100
    }
}

/// Hook管理器
pub struct HookManager {
    handlers: Arc<RwLock<Vec<Box<dyn HookHandler>>>>,
}

impl HookManager {
    pub fn new() -> Self {
        Self {
            handlers: Arc::new(RwLock::new(Vec::new())),
        }
    }

    /// 注册Hook处理器
    pub async fn register(&self, handler: Box<dyn HookHandler>) {
        let mut handlers = self.handlers.write().await;
        handlers.push(handler);
        handlers.sort_by_key(|h| h.priority());
    }

    /// 触发Hook事件
    pub async fn trigger(&self, event: &HookEvent) -> anyhow::Result<()> {
        let handlers = self.handlers.read().await;
        for handler in handlers.iter() {
            if let Err(e) = handler.handle(event).await {
                tracing::warn!("Hook {} failed: {}", handler.name(), e);
            }
        }
        Ok(())
    }

    /// 获取已注册的Hook数量
    pub async fn handler_count(&self) -> usize {
        self.handlers.read().await.len()
    }
}

impl Default for HookManager {
    fn default() -> Self {
        Self::new()
    }
}

/// 全局Hook管理器
static GLOBAL_HOOK_MANAGER: std::sync::LazyLock<std::sync::Mutex<Option<HookManager>>> =
    std::sync::LazyLock::new(|| std::sync::Mutex::new(None));

/// 获取全局Hook管理器
pub fn global_hook_manager() -> std::sync::MutexGuard<'static, Option<HookManager>> {
    GLOBAL_HOOK_MANAGER.lock().unwrap()
}

/// 初始化全局Hook管理器
pub fn init_global_hook_manager() {
    let mut guard = GLOBAL_HOOK_MANAGER.lock().unwrap();
    *guard = Some(HookManager::new());
}
