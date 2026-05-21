//! On Session End Hook Handler
//! Category: Session Management

use anyhow::Result;
use tracing;

/// On Session End hook implementation
pub struct OnSessionEndHook;

impl OnSessionEndHook {
    pub fn new() -> Self {
        Self
    }
}

#[async_trait::async_trait]
impl crate::hooks::HookHandler for OnSessionEndHook {
    async fn handle(&self, event: &crate::hooks::HookEvent) -> Result<()> {
        tracing::info!("Handling event in on_session_end hook");

        // TODO: Implement on_session_end hook logic
        match event {
            // Handle specific event types
            _ => {
                tracing::debug!("on_session_end received generic event");
            }
        }

        Ok(())
    }

    fn name(&self) -> &str {
        "on_session_end"
    }

    fn priority(&self) -> u32 {
        100  // Default priority, adjust as needed
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[tokio::test]
    async fn test_on_session_end_basic() {
        let hook = OnSessionEndHook::new();
        assert_eq!(hook.name(), "on_session_end");
        assert_eq!(hook.priority(), 100);
    }
}
