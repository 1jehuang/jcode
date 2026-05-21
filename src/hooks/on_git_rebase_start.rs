//! On Git Rebase Start Hook Handler
//! Category: Git Events

use anyhow::Result;
use tracing;

/// On Git Rebase Start hook implementation
pub struct OnGitRebaseStartHook;

impl OnGitRebaseStartHook {
    pub fn new() -> Self {
        Self
    }
}

#[async_trait::async_trait]
impl crate::hooks::HookHandler for OnGitRebaseStartHook {
    async fn handle(&self, event: &crate::hooks::HookEvent) -> Result<()> {
        tracing::info!("Handling event in on_git_rebase_start hook");

        // TODO: Implement on_git_rebase_start hook logic
        match event {
            // Handle specific event types
            _ => {
                tracing::debug!("on_git_rebase_start received generic event");
            }
        }

        Ok(())
    }

    fn name(&self) -> &str {
        "on_git_rebase_start"
    }

    fn priority(&self) -> u32 {
        100  // Default priority, adjust as needed
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[tokio::test]
    async fn test_on_git_rebase_start_basic() {
        let hook = OnGitRebaseStartHook::new();
        assert_eq!(hook.name(), "on_git_rebase_start");
        assert_eq!(hook.priority(), 100);
    }
}
