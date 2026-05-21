//! Tool hooks - 工具执行前后处理

use crate::hooks::{HookHandler, HookEvent};
use anyhow::Result;

// Tool Before Execute Hook
pub struct ToolBeforeExecuteHook;

#[async_trait::async_trait]
impl HookHandler for ToolBeforeExecuteHook {
    async fn handle(&self, event: &HookEvent) -> Result<()> {
        if let HookEvent::ToolBeforeExecute { tool_name, args } = event {
            tracing::debug!("Tool executing: {} with {:?}", tool_name, args);
            // TODO: Permission check, logging, validation
        }
        Ok(())
    }

    fn name(&self) -> &str {
        "tool_before_execute"
    }

    fn priority(&self) -> u32 {
        5
    }
}

// Tool After Execute Hook
pub struct ToolAfterExecuteHook;

#[async_trait::async_trait]
impl HookHandler for ToolAfterExecuteHook {
    async fn handle(&self, event: &HookEvent) -> Result<()> {
        if let HookEvent::ToolAfterExecute { tool_name, result } = event {
            tracing::debug!("Tool executed: {} with result {:?}", tool_name, result);
            // TODO: Cache results, update state, metrics
        }
        Ok(())
    }

    fn name(&self) -> &str {
        "tool_after_execute"
    }

    fn priority(&self) -> u32 {
        100
    }
}
