//! On Split View Hook Handler
//! Category: Editor Events

use anyhow::Result;
use tracing;

/// On Split View hook implementation
pub struct OnSplitViewHook;

impl OnSplitViewHook {
    pub fn new() -> Self {
        Self
    }
}

#[async_trait::async_trait]
impl crate::hooks::HookHandler for OnSplitViewHook {
    async fn handle(&self, event: &crate::hooks::HookEvent) -> Result<()> {
        tracing::info!("Handling event in on_split_view hook");

        // TODO: Implement on_split_view hook logic
        match event {
            // Handle specific event types
            _ => {
                tracing::debug!("on_split_view received generic event");
            }
        }

        Ok(())
    }

    fn name(&self) -> &str {
        "on_split_view"
    }

    fn priority(&self) -> u32 {
        100  // Default priority, adjust as needed
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[tokio::test]
    async fn test_on_split_view_basic() {
        let hook = OnSplitViewHook::new();
        assert_eq!(hook.name(), "on_split_view");
        assert_eq!(hook.priority(), 100);
    }
}
