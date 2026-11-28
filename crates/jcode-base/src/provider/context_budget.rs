use super::*;

impl MultiProvider {
    pub(super) fn context_budget_provider(&self) -> Option<Arc<dyn Provider>> {
        match self.active_provider() {
            ActiveProvider::Claude => self.anthropic_provider(),
            ActiveProvider::OpenAI => self.openai_provider(),
            ActiveProvider::Copilot => self.copilot_provider(),
            ActiveProvider::Antigravity => self.antigravity_provider(),
            ActiveProvider::Gemini => self.gemini_provider(),
            ActiveProvider::Cursor => self.cursor_provider(),
            ActiveProvider::Bedrock => self
                .bedrock_provider()
                .map(|provider| provider as Arc<dyn Provider>),
            ActiveProvider::OpenRouter => self.active_openrouter_execution_provider(),
        }
    }
}

#[cfg(test)]
#[path = "context_budget_tests.rs"]
mod tests;
