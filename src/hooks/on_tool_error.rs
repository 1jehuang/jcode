//! On Tool Error Hook Handler
//! Category: Tool Execution

use anyhow::Result;
use tracing;

/// On Tool Error hook implementation
pub struct OnToolErrorHook;

impl OnToolErrorHook {
    pub fn new() -> Self {
        Self
    }
}

#[async_trait::async_trait]
impl crate::hooks::HookHandler for OnToolErrorHook {
    async fn handle(&self, event: &crate::hooks::HookEvent) -> Result<()> {
        tracing::info!("Handling event in on_tool_error hook");

        // TODO: Implement on_tool_error hook logic
        match event {
            // Handle specific event types
            _ => {
                tracing::debug!("on_tool_error received generic event");
            }
        }

        Ok(())
    }

    fn name(&self) -> &str {
        "on_tool_error"
    }

    fn priority(&self) -> u32 {
        100  // Default priority, adjust as needed
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[tokio::test]
    async fn test_on_tool_error_basic() {
        let hook = OnToolErrorHook::new();
        assert_eq!(hook.name(), "on_tool_error");
        assert_eq!(hook.priority(), 100);
    }
}
