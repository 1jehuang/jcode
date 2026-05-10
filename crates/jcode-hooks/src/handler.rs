// ════════════════════════════════════════════════════════════════
// Hook Handler trait + Action 类型
// ════════════════════════════════════════════════════════════════

use crate::events::HookEventData;
use std::any::Any;

/// Hook 处理器的返回动作
#[derive(Debug)]
pub enum HookAction {
    /// 允许事件继续传递 (默认行为)
    Allow,
    /// 修改事件数据后继续传递
    Modify(Box<dyn Any + Send + Sync>),
    /// 阻止事件继续传递
    Block(String),
}

impl std::fmt::Display for HookAction {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            Self::Allow => write!(f, "Allow"),
            Self::Modify(_) => write!(f, "Modify"),
            Self::Block(reason) => write!(f, "Block({})", reason),
        }
    }
}

/// Hook Handler trait — 所有 Hook 处理器必须实现此接口
///
/// # 实现示例
///
/// ```ignore
/// struct LogHandler;
///
/// #[async_trait::async_trait]
/// impl HookHandler for LogHandler {
///     async fn handle(&self, event: &HookEventData) -> HookAction {
///         println!("Event: {:?}", event.event_type());
///         HookAction::Allow
///     }
///
///     fn name(&self) -> &str { "LogHandler" }
///     fn priority(&self) -> i32 { 0 }
/// }
/// ```
#[async_trait::async_trait]
pub trait HookHandler: Send + Sync {
    /// 处理 Hook 事件
    async fn handle(&self, event: &HookEventData) -> HookAction;

    /// Handler 名称 (用于调试/日志)
    fn name(&self) -> &str;

    /// 优先级 (数值越小越先执行)
    fn priority(&self) -> i32 {
        0
    }

    /// 是否只运行一次 (执行后自动移除)
    fn once(&self) -> bool {
        false
    }
}

/// 便捷的闭包包装 Handler
pub struct ClosureHandler<F>
where
    F: Fn(&HookEventData) -> HookAction + Send + Sync,
{
    name: String,
    priority: i32,
    handler: F,
}

impl<F> ClosureHandler<F>
where
    F: Fn(&HookEventData) -> HookAction + Send + Sync,
{
    pub fn new(name: &str, handler: F) -> Self {
        Self { name: name.to_string(), priority: 0, handler }
    }

    pub fn with_priority(mut self, priority: i32) -> Self {
        self.priority = priority;
        self
    }
}

#[async_trait::async_trait]
impl<F> HookHandler for ClosureHandler<F>
where
    F: Fn(&HookEventData) -> HookAction + Send + Sync,
{
    async fn handle(&self, event: &HookEventData) -> HookAction {
        (self.handler)(event)
    }

    fn name(&self) -> &str {
        &self.name
    }

    fn priority(&self) -> i32 {
        self.priority
    }
}
