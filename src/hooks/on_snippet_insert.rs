//! On Snippet Insert Hook Handler
//! Category: Editor Events

use anyhow::Result;
use tracing;

/// On Snippet Insert hook implementation
pub struct OnSnippetInsertHook;

impl OnSnippetInsertHook {
    pub fn new() -> Self {
        Self
    }
}

#[async_trait::async_trait]
impl crate::hooks::HookHandler for OnSnippetInsertHook {
    async fn handle(&self, event: &crate::hooks::HookEvent) -> Result<()> {
        tracing::info!("Handling event in on_snippet_insert hook");

        // TODO: Implement on_snippet_insert hook logic
        match event {
            // Handle specific event types
            _ => {
                tracing::debug!("on_snippet_insert received generic event");
            }
        }

        Ok(())
    }

    fn name(&self) -> &str {
        "on_snippet_insert"
    }

    fn priority(&self) -> u32 {
        100  // Default priority, adjust as needed
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[tokio::test]
    async fn test_on_snippet_insert_basic() {
        let hook = OnSnippetInsertHook::new();
        assert_eq!(hook.name(), "on_snippet_insert");
        assert_eq!(hook.priority(), 100);
    }
}
