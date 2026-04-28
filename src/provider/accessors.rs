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

    /// Lazy hot-init: if `self.openai` is currently `None` but valid OpenAI
    /// credentials exist on disk (or via `OPENAI_API_KEY`), attach a fresh
    /// `OpenAIProvider`. Used by credential-check paths in `set_model` and
    /// `complete*` so the dispatcher recovers when credentials become valid
    /// after startup without an in-process `LoginCompleted` event reaching
    /// this server (e.g. the user ran `jcode login --provider openai` from a
    /// separate shell).
    pub(super) fn try_hot_init_openai_from_disk(&self) {
        if self.openai_provider().is_some() {
            return;
        }
        crate::auth::AuthStatus::invalidate_cache();
        let Ok(credentials) = crate::auth::codex::load_credentials() else {
            return;
        };
        crate::logging::info("Hot-initialized OpenAI provider lazily on demand");
        *self
            .openai
            .write()
            .unwrap_or_else(|poisoned| poisoned.into_inner()) =
            Some(Arc::new(openai::OpenAIProvider::new(credentials)));
    }
}
