//! On Font Change Hook Handler
//! Category: Editor Events

use anyhow::Result;
use tracing;

/// On Font Change hook implementation
pub struct OnFontChangeHook;

impl OnFontChangeHook {
    pub fn new() -> Self {
        Self
    }
}

#[async_trait::async_trait]
impl crate::hooks::HookHandler for OnFontChangeHook {
    async fn handle(&self, event: &crate::hooks::HookEvent) -> Result<()> {
        tracing::info!("Handling event in on_font_change hook");

        // TODO: Implement on_font_change hook logic
        match event {
            // Handle specific event types
            _ => {
                tracing::debug!("on_font_change received generic event");
            }
        }

        Ok(())
    }

    fn name(&self) -> &str {
        "on_font_change"
    }

    fn priority(&self) -> u32 {
        100  // Default priority, adjust as needed
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[tokio::test]
    async fn test_on_font_change_basic() {
        let hook = OnFontChangeHook::new();
        assert_eq!(hook.name(), "on_font_change");
        assert_eq!(hook.priority(), 100);
    }
}
