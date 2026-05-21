//! On Git Merge Conflict Hook Handler
//! Category: Git Events

use anyhow::Result;
use tracing;

/// On Git Merge Conflict hook implementation
pub struct OnGitMergeConflictHook;

impl OnGitMergeConflictHook {
    pub fn new() -> Self {
        Self
    }
}

#[async_trait::async_trait]
impl crate::hooks::HookHandler for OnGitMergeConflictHook {
    async fn handle(&self, event: &crate::hooks::HookEvent) -> Result<()> {
        tracing::info!("Handling event in on_git_merge_conflict hook");

        // TODO: Implement on_git_merge_conflict hook logic
        match event {
            // Handle specific event types
            _ => {
                tracing::debug!("on_git_merge_conflict received generic event");
            }
        }

        Ok(())
    }

    fn name(&self) -> &str {
        "on_git_merge_conflict"
    }

    fn priority(&self) -> u32 {
        100  // Default priority, adjust as needed
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[tokio::test]
    async fn test_on_git_merge_conflict_basic() {
        let hook = OnGitMergeConflictHook::new();
        assert_eq!(hook.name(), "on_git_merge_conflict");
        assert_eq!(hook.priority(), 100);
    }
}
