//! Session hooks - 会话生命周期处理

use crate::hooks::{HookHandler, HookEvent};
use anyhow::Result;

// Session Started Hook
pub struct SessionStartedHook;

#[async_trait::async_trait]
impl HookHandler for SessionStartedHook {
    async fn handle(&self, event: &HookEvent) -> Result<()> {
        if let HookEvent::SessionStarted { session_id } = event {
            tracing::info!("Session started: {}", session_id);
            // TODO: Initialize session state, load context
        }
        Ok(())
    }

    fn name(&self) -> &str {
        "session_started"
    }

    fn priority(&self) -> u32 {
        1
    }
}

// Session Ended Hook
pub struct SessionEndedHook;

#[async_trait::async_trait]
impl HookHandler for SessionEndedHook {
    async fn handle(&self, event: &HookEvent) -> Result<()> {
        if let HookEvent::SessionEnded { session_id } = event {
            tracing::info!("Session ended: {}", session_id);
            // TODO: Save session state, cleanup, analytics
        }
        Ok(())
    }

    fn name(&self) -> &str {
        "session_ended"
    }

    fn priority(&self) -> u32 {
        999
    }
}
