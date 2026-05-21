//! On Tool Fallback Hook Handler
//! Category: Tool Execution

use anyhow::Result;
use tracing;

/// On Tool Fallback hook implementation
pub struct OnToolFallbackHook;

impl OnToolFallbackHook {
    pub fn new() -> Self {
        Self
    }
}

#[async_trait::async_trait]
impl crate::hooks::HookHandler for OnToolFallbackHook {
    async fn handle(&self, event: &crate::hooks::HookEvent) -> Result<()> {
        tracing::info!("Handling event in on_tool_fallback hook");

        // TODO: Implement on_tool_fallback hook logic
        match event {
            // Handle specific event types
            _ => {
                tracing::debug!("on_tool_fallback received generic event");
            }
        }

        Ok(())
    }

    fn name(&self) -> &str {
        "on_tool_fallback"
    }

    fn priority(&self) -> u32 {
        100  // Default priority, adjust as needed
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[tokio::test]
    async fn test_on_tool_fallback_basic() {
        let hook = OnToolFallbackHook::new();
        assert_eq!(hook.name(), "on_tool_fallback");
        assert_eq!(hook.priority(), 100);
    }
}
