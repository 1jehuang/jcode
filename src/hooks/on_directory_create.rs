//! On Directory Create Hook Handler
//! Category: File Events

use anyhow::Result;
use tracing;

/// On Directory Create hook implementation
pub struct OnDirectoryCreateHook;

impl OnDirectoryCreateHook {
    pub fn new() -> Self {
        Self
    }
}

#[async_trait::async_trait]
impl crate::hooks::HookHandler for OnDirectoryCreateHook {
    async fn handle(&self, event: &crate::hooks::HookEvent) -> Result<()> {
        tracing::info!("Handling event in on_directory_create hook");

        // TODO: Implement on_directory_create hook logic
        match event {
            // Handle specific event types
            _ => {
                tracing::debug!("on_directory_create received generic event");
            }
        }

        Ok(())
    }

    fn name(&self) -> &str {
        "on_directory_create"
    }

    fn priority(&self) -> u32 {
        100  // Default priority, adjust as needed
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[tokio::test]
    async fn test_on_directory_create_basic() {
        let hook = OnDirectoryCreateHook::new();
        assert_eq!(hook.name(), "on_directory_create");
        assert_eq!(hook.priority(), 100);
    }
}
