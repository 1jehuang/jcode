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

    pub(super) fn antigravity_provider(&self) -> Option<Arc<antigravity::AntigravityProvider>> {
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

    pub(super) fn bedrock_provider(&self) -> Option<Arc<bedrock::BedrockProvider>> {
        self.bedrock
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

    pub(super) fn provider_slot_available(&self, provider: ActiveProvider) -> bool {
        match provider {
            ActiveProvider::Claude => self.has_claude_runtime(),
            ActiveProvider::OpenAI => self.openai_provider().is_some(),
            ActiveProvider::Copilot => self.copilot_provider().is_some(),
            ActiveProvider::Antigravity => self.antigravity_provider().is_some(),
            ActiveProvider::Gemini => self.gemini_provider().is_some(),
            ActiveProvider::Cursor => self.cursor_provider().is_some(),
            ActiveProvider::Bedrock => self.bedrock_provider().is_some(),
            ActiveProvider::OpenRouter => self.openrouter_provider().is_some(),
        }
    }

    pub(super) fn reconcile_auth_if_provider_missing(&self, provider: ActiveProvider) -> bool {
        if self.provider_slot_available(provider) {
            return true;
        }

        crate::logging::info(&format!(
            "Provider {} missing at use site; reconciling auth from disk",
            Self::provider_label(provider)
        ));
        Provider::on_auth_changed(self);
        self.provider_slot_available(provider)
    }
}
