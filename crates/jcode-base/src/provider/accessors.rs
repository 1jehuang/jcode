use super::*;

impl MultiProvider {
    pub(super) fn anthropic_provider(&self) -> Option<Arc<dyn Provider>> {
        self.anthropic
            .read()
            .unwrap_or_else(|poisoned| poisoned.into_inner())
            .clone()
    }

    pub(super) fn openai_provider(&self) -> Option<Arc<dyn Provider>> {
        self.openai
            .read()
            .unwrap_or_else(|poisoned| poisoned.into_inner())
            .clone()
    }

    pub(super) fn antigravity_provider(&self) -> Option<Arc<dyn Provider>> {
        self.antigravity
            .read()
            .unwrap_or_else(|poisoned| poisoned.into_inner())
            .clone()
    }

    pub(super) fn gemini_provider(&self) -> Option<Arc<dyn Provider>> {
        self.gemini
            .read()
            .unwrap_or_else(|poisoned| poisoned.into_inner())
            .clone()
    }

    pub(super) fn copilot_provider(&self) -> Option<Arc<dyn Provider>> {
        self.copilot_api
            .read()
            .unwrap_or_else(|poisoned| poisoned.into_inner())
            .clone()
    }

    pub(super) fn cursor_provider(&self) -> Option<Arc<dyn Provider>> {
        self.cursor
            .read()
            .unwrap_or_else(|poisoned| poisoned.into_inner())
            .clone()
    }

    pub(super) fn cursor_acp_provider(&self) -> Option<Arc<dyn Provider>> {
        self.cursor_acp
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

    pub(super) fn openrouter_provider(&self) -> Option<Arc<dyn Provider>> {
        ProviderRegistry::new(self).real_openrouter()
    }

    pub(super) fn active_openrouter_execution_provider(&self) -> Option<Arc<dyn Provider>> {
        ProviderRegistry::new(self).active_openrouter_execution()
    }

    pub(super) fn clear_active_openai_compatible_profile(&self) {
        ProviderRegistry::new(self).clear_active_compatible_profile();
    }

    pub(super) fn has_claude_runtime(&self) -> bool {
        self.anthropic_provider().is_some()
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
            // The OpenRouter slot executes through the *active* runtime: a
            // direct OpenAI-compatible profile when one is active, else real
            // OpenRouter. Checking only the real slot here made dispatch treat
            // an active compat profile (e.g. minimax) as "not configured"
            // whenever no OPENROUTER_API_KEY existed, and the failover loop
            // then silently rerouted the request to another provider such as
            // OpenAI (issue #358).
            ActiveProvider::OpenRouter => self.active_openrouter_execution_provider().is_some(),
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
