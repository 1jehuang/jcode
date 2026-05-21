//! On Access Denied Hook Handler
//! Category: Security Events

use anyhow::Result;
use tracing;

/// On Access Denied hook implementation
pub struct OnAccessDeniedHook;

impl OnAccessDeniedHook {
    pub fn new() -> Self {
        Self
    }
}

#[async_trait::async_trait]
impl crate::hooks::HookHandler for OnAccessDeniedHook {
    async fn handle(&self, event: &crate::hooks::HookEvent) -> Result<()> {
        tracing::info!("Handling event in on_access_denied hook");

        // TODO: Implement on_access_denied hook logic
        match event {
            // Handle specific event types
            _ => {
                tracing::debug!("on_access_denied received generic event");
            }
        }

        Ok(())
    }

    fn name(&self) -> &str {
        "on_access_denied"
    }

    fn priority(&self) -> u32 {
        100  // Default priority, adjust as needed
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[tokio::test]
    async fn test_on_access_denied_basic() {
        let hook = OnAccessDeniedHook::new();
        assert_eq!(hook.name(), "on_access_denied");
        assert_eq!(hook.priority(), 100);
    }
}
