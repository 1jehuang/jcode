//! On Tool Stream End Hook Handler
//! Category: Tool Execution

use anyhow::Result;
use tracing;

/// On Tool Stream End hook implementation
pub struct OnToolStreamEndHook;

impl OnToolStreamEndHook {
    pub fn new() -> Self {
        Self
    }
}

#[async_trait::async_trait]
impl crate::hooks::HookHandler for OnToolStreamEndHook {
    async fn handle(&self, event: &crate::hooks::HookEvent) -> Result<()> {
        tracing::info!("Handling event in on_tool_stream_end hook");

        // TODO: Implement on_tool_stream_end hook logic
        match event {
            // Handle specific event types
            _ => {
                tracing::debug!("on_tool_stream_end received generic event");
            }
        }

        Ok(())
    }

    fn name(&self) -> &str {
        "on_tool_stream_end"
    }

    fn priority(&self) -> u32 {
        100  // Default priority, adjust as needed
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[tokio::test]
    async fn test_on_tool_stream_end_basic() {
        let hook = OnToolStreamEndHook::new();
        assert_eq!(hook.name(), "on_tool_stream_end");
        assert_eq!(hook.priority(), 100);
    }
}
