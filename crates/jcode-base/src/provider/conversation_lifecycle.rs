//! Conversation lifecycle operations shared by all instantiated runtime slots.
use super::*;

impl MultiProvider {
    pub(super) async fn reset_existing_conversation_state(&self) -> Result<()> {
        // Do not create lazy providers or hold synchronous locks across awaits.
        let mut providers = Vec::new();
        for slot in [
            &self.anthropic,
            &self.openai,
            &self.copilot_api,
            &self.antigravity,
            &self.gemini,
            &self.cursor,
            &self.claude_code,
            &self.openrouter,
        ] {
            if let Some(provider) = slot.read().unwrap_or_else(|p| p.into_inner()).clone() {
                providers.push(provider);
            }
        }
        if let Some(provider) = self
            .bedrock
            .read()
            .unwrap_or_else(|p| p.into_inner())
            .clone()
        {
            providers.push(provider as Arc<dyn Provider>);
        }
        providers.extend(
            self.openai_compatible_profiles
                .read()
                .unwrap_or_else(|p| p.into_inner())
                .values()
                .cloned(),
        );
        // Reset every slot even on failure: inactive children must not keep the
        // discarded future alive until the user switches back to that provider.
        let mut errors = Vec::new();
        for provider in providers {
            if let Err(error) = provider.reset_conversation_state().await {
                errors.push(format!("{}: {error:#}", provider.name()));
            }
        }
        if !errors.is_empty() {
            anyhow::bail!(
                "Could not reset provider conversation state: {}",
                errors.join("; ")
            );
        }
        Ok(())
    }
}
