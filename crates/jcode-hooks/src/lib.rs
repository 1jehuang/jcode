// jcode-hooks
// ════════════════════════════════════════════════════════════════
// Hook 事件广播系统 — 移植自 Claude Code hooks/ 目录
//
// 核心能力:
//
//   7 类 Hook 点:
//   ┌──────────────────────────────────────────────────┐
//   │ 1. Session 级别: Start / End                     │
//   │ 2. Agent 执行: PreAgentExecute / PostAgentExecute│
//   │ 3. Prompt 注入: PrePrompt (修改 system prompt)   │
//   │ 4. 工具调用: PreToolCall / PostToolCall          │
//   │ 5. HTTP 请求: PreHttpRequest / PostHttpResponse  │
//   │ 6. 安全检查: SsrfCheck                           │
//   │ 7. 自定义: Custom(event_name, payload)            │
//   └──────────────────────────────────────────────────┘
//
// 架构模式:
//
//   Publisher → EventBus → [Handler1, Handler2, ...] → Action(Allow|Modify|Block)
//
// 使用示例:
//
// ```ignore
// let bus = HookEventBus::new();
//
// // 注册 Handler
// bus.register(HookEventType::PreToolCall, |event| {
//     if event.tool_name() == "Bash" { return HookAction::Block("No bash allowed".into()); }
//     HookAction::Allow
// });
//
// // 发布事件
// let action = bus.emit(HookEvent::tool_call_pre("Bash", "rm -rf /")).await;
// assert!(action.is_blocked());
// ```
// ════════════════════════════════════════════════════════════════

mod events;
mod handler;
mod bus;

pub use events::*;
pub use handler::{HookHandler, HookAction};
pub use bus::{HookEventBus};

/// 便捷的 Hook 注册宏
#[macro_export]
macro_rules! register_hook {
    ($bus:expr, $event_type:path, $handler:expr) => {
        $bus.register($event_type, std::sync::Arc::new($handler))
    };
}
