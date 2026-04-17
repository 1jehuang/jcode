use super::*;

impl MultiProvider {
    pub(super) fn claude_provider(&self) -> Option<Arc<claude::ClaudeProvider>> {
        self.claude
            .read()
            .unwrap_or_else(|poisoned| poisoned.into_inner())
            .clone()
    }

    pub(super) fn anthropic_provider(&self) -> Option<Arc<anthropic::AnthropicProvider>> {
        self.anthropic
            .read()
            .unwrap_or_else(|poisoned| poisoned.into_inner())
            .clone()
    }

    pub(super) fn openai_provider(&self) -> Option<Arc<openai::OpenAIProvider>> {
        self.openai
            .read()
            .unwrap_or_else(|poisoned| poisoned.into_inner())
            .clone()
    }

    pub(super) fn antigravity_provider(&self) -> Option<Arc<antigravity::AntigravityCliProvider>> {
        self.antigravity
            .read()
            .unwrap_or_else(|poisoned| poisoned.into_inner())
            .clone()
    }

    pub(super) fn gemini_provider(&self) -> Option<Arc<gemini::GeminiProvider>> {
        self.gemini
            .read()
            .unwrap_or_else(|poisoned| poisoned.into_inner())
            .clone()
    }

    pub(super) fn copilot_provider(&self) -> Option<Arc<copilot::CopilotApiProvider>> {
        self.copilot_api
            .read()
            .unwrap_or_else(|poisoned| poisoned.into_inner())
            .clone()
    }

    pub(super) fn cursor_provider(&self) -> Option<Arc<cursor::CursorCliProvider>> {
        self.cursor
            .read()
            .unwrap_or_else(|poisoned| poisoned.into_inner())
            .clone()
    }

    pub(super) fn openrouter_provider(&self) -> Option<Arc<openrouter::OpenRouterProvider>> {
        self.openrouter
            .read()
            .unwrap_or_else(|poisoned| poisoned.into_inner())
            .clone()
    }

    pub(super) fn has_claude_runtime(&self) -> bool {
        self.anthropic_provider().is_some() || self.claude_provider().is_some()
    }
}
