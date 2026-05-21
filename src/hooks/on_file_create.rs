//! On File Create Hook Handler
//! Category: File Events

use anyhow::Result;
use tracing;

/// On File Create hook implementation
pub struct OnFileCreateHook;

impl OnFileCreateHook {
    pub fn new() -> Self {
        Self
    }
}

#[async_trait::async_trait]
impl crate::hooks::HookHandler for OnFileCreateHook {
    async fn handle(&self, event: &crate::hooks::HookEvent) -> Result<()> {
        tracing::info!("Handling event in on_file_create hook");

        // TODO: Implement on_file_create hook logic
        match event {
            // Handle specific event types
            _ => {
                tracing::debug!("on_file_create received generic event");
            }
        }

        Ok(())
    }

    fn name(&self) -> &str {
        "on_file_create"
    }

    fn priority(&self) -> u32 {
        100  // Default priority, adjust as needed
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[tokio::test]
    async fn test_on_file_create_basic() {
        let hook = OnFileCreateHook::new();
        assert_eq!(hook.name(), "on_file_create");
        assert_eq!(hook.priority(), 100);
    }
}
