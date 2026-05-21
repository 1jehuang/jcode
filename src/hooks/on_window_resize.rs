//! On Window Resize Hook Handler
//! Category: Editor Events

use anyhow::Result;
use tracing;

/// On Window Resize hook implementation
pub struct OnWindowResizeHook;

impl OnWindowResizeHook {
    pub fn new() -> Self {
        Self
    }
}

#[async_trait::async_trait]
impl crate::hooks::HookHandler for OnWindowResizeHook {
    async fn handle(&self, event: &crate::hooks::HookEvent) -> Result<()> {
        tracing::info!("Handling event in on_window_resize hook");

        // TODO: Implement on_window_resize hook logic
        match event {
            // Handle specific event types
            _ => {
                tracing::debug!("on_window_resize received generic event");
            }
        }

        Ok(())
    }

    fn name(&self) -> &str {
        "on_window_resize"
    }

    fn priority(&self) -> u32 {
        100  // Default priority, adjust as needed
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[tokio::test]
    async fn test_on_window_resize_basic() {
        let hook = OnWindowResizeHook::new();
        assert_eq!(hook.name(), "on_window_resize");
        assert_eq!(hook.priority(), 100);
    }
}
