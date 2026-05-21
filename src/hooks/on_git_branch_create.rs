//! On Git Branch Create Hook Handler
//! Category: Git Events

use anyhow::Result;
use tracing;

/// On Git Branch Create hook implementation
pub struct OnGitBranchCreateHook;

impl OnGitBranchCreateHook {
    pub fn new() -> Self {
        Self
    }
}

#[async_trait::async_trait]
impl crate::hooks::HookHandler for OnGitBranchCreateHook {
    async fn handle(&self, event: &crate::hooks::HookEvent) -> Result<()> {
        tracing::info!("Handling event in on_git_branch_create hook");

        // TODO: Implement on_git_branch_create hook logic
        match event {
            // Handle specific event types
            _ => {
                tracing::debug!("on_git_branch_create received generic event");
            }
        }

        Ok(())
    }

    fn name(&self) -> &str {
        "on_git_branch_create"
    }

    fn priority(&self) -> u32 {
        100  // Default priority, adjust as needed
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[tokio::test]
    async fn test_on_git_branch_create_basic() {
        let hook = OnGitBranchCreateHook::new();
        assert_eq!(hook.name(), "on_git_branch_create");
        assert_eq!(hook.priority(), 100);
    }
}
