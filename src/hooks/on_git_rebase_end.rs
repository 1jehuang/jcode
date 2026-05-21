//! On Git Rebase End Hook Handler
//! Category: Git Events

use anyhow::Result;
use tracing;

/// On Git Rebase End hook implementation
pub struct OnGitRebaseEndHook;

impl OnGitRebaseEndHook {
    pub fn new() -> Self {
        Self
    }
}

#[async_trait::async_trait]
impl crate::hooks::HookHandler for OnGitRebaseEndHook {
    async fn handle(&self, event: &crate::hooks::HookEvent) -> Result<()> {
        tracing::info!("Handling event in on_git_rebase_end hook");

        // TODO: Implement on_git_rebase_end hook logic
        match event {
            // Handle specific event types
            _ => {
                tracing::debug!("on_git_rebase_end received generic event");
            }
        }

        Ok(())
    }

    fn name(&self) -> &str {
        "on_git_rebase_end"
    }

    fn priority(&self) -> u32 {
        100  // Default priority, adjust as needed
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[tokio::test]
    async fn test_on_git_rebase_end_basic() {
        let hook = OnGitRebaseEndHook::new();
        assert_eq!(hook.name(), "on_git_rebase_end");
        assert_eq!(hook.priority(), 100);
    }
}
