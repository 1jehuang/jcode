//! On Git Tag Create Hook Handler
//! Category: Git Events

use anyhow::Result;
use tracing;

/// On Git Tag Create hook implementation
pub struct OnGitTagCreateHook;

impl OnGitTagCreateHook {
    pub fn new() -> Self {
        Self
    }
}

#[async_trait::async_trait]
impl crate::hooks::HookHandler for OnGitTagCreateHook {
    async fn handle(&self, event: &crate::hooks::HookEvent) -> Result<()> {
        tracing::info!("Handling event in on_git_tag_create hook");

        // TODO: Implement on_git_tag_create hook logic
        match event {
            // Handle specific event types
            _ => {
                tracing::debug!("on_git_tag_create received generic event");
            }
        }

        Ok(())
    }

    fn name(&self) -> &str {
        "on_git_tag_create"
    }

    fn priority(&self) -> u32 {
        100  // Default priority, adjust as needed
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[tokio::test]
    async fn test_on_git_tag_create_basic() {
        let hook = OnGitTagCreateHook::new();
        assert_eq!(hook.name(), "on_git_tag_create");
        assert_eq!(hook.priority(), 100);
    }
}
