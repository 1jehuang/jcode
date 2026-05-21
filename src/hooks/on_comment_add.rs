//! On Comment Add Hook Handler
//! Category: Collaboration Events

use anyhow::Result;
use tracing;

/// On Comment Add hook implementation
pub struct OnCommentAddHook;

impl OnCommentAddHook {
    pub fn new() -> Self {
        Self
    }
}

#[async_trait::async_trait]
impl crate::hooks::HookHandler for OnCommentAddHook {
    async fn handle(&self, event: &crate::hooks::HookEvent) -> Result<()> {
        tracing::info!("Handling event in on_comment_add hook");

        // TODO: Implement on_comment_add hook logic
        match event {
            // Handle specific event types
            _ => {
                tracing::debug!("on_comment_add received generic event");
            }
        }

        Ok(())
    }

    fn name(&self) -> &str {
        "on_comment_add"
    }

    fn priority(&self) -> u32 {
        100  // Default priority, adjust as needed
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[tokio::test]
    async fn test_on_comment_add_basic() {
        let hook = OnCommentAddHook::new();
        assert_eq!(hook.name(), "on_comment_add");
        assert_eq!(hook.priority(), 100);
    }
}
