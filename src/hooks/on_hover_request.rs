//! On Hover Request Hook Handler
//! Category: Editor Events

use anyhow::Result;
use tracing;

/// On Hover Request hook implementation
pub struct OnHoverRequestHook;

impl OnHoverRequestHook {
    pub fn new() -> Self {
        Self
    }
}

#[async_trait::async_trait]
impl crate::hooks::HookHandler for OnHoverRequestHook {
    async fn handle(&self, event: &crate::hooks::HookEvent) -> Result<()> {
        tracing::info!("Handling event in on_hover_request hook");

        // TODO: Implement on_hover_request hook logic
        match event {
            // Handle specific event types
            _ => {
                tracing::debug!("on_hover_request received generic event");
            }
        }

        Ok(())
    }

    fn name(&self) -> &str {
        "on_hover_request"
    }

    fn priority(&self) -> u32 {
        100  // Default priority, adjust as needed
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[tokio::test]
    async fn test_on_hover_request_basic() {
        let hook = OnHoverRequestHook::new();
        assert_eq!(hook.name(), "on_hover_request");
        assert_eq!(hook.priority(), 100);
    }
}
