//! On Directory Delete Hook Handler
//! Category: File Events

use anyhow::Result;
use tracing;

/// On Directory Delete hook implementation
pub struct OnDirectoryDeleteHook;

impl OnDirectoryDeleteHook {
    pub fn new() -> Self {
        Self
    }
}

#[async_trait::async_trait]
impl crate::hooks::HookHandler for OnDirectoryDeleteHook {
    async fn handle(&self, event: &crate::hooks::HookEvent) -> Result<()> {
        tracing::info!("Handling event in on_directory_delete hook");

        // TODO: Implement on_directory_delete hook logic
        match event {
            // Handle specific event types
            _ => {
                tracing::debug!("on_directory_delete received generic event");
            }
        }

        Ok(())
    }

    fn name(&self) -> &str {
        "on_directory_delete"
    }

    fn priority(&self) -> u32 {
        100  // Default priority, adjust as needed
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[tokio::test]
    async fn test_on_directory_delete_basic() {
        let hook = OnDirectoryDeleteHook::new();
        assert_eq!(hook.name(), "on_directory_delete");
        assert_eq!(hook.priority(), 100);
    }
}
