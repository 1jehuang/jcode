use super::*;

impl MultiProvider {
    pub(super) fn bound_session_id(&self) -> Option<String> {
        self.session_id
            .read()
            .unwrap_or_else(|e| e.into_inner())
            .clone()
    }

    pub(super) fn bound_working_dir(&self) -> Option<std::path::PathBuf> {
        self.working_dir
            .read()
            .unwrap_or_else(|e| e.into_inner())
            .clone()
    }

    pub(super) fn bind_provider_session(&self, provider: &dyn Provider) {
        if let Some(session_id) = self.bound_session_id() {
            provider.set_session_id(&session_id);
        }
        if let Some(working_dir) = self.bound_working_dir() {
            provider.set_working_dir(&working_dir);
        }
    }

    pub(super) fn bind_session_id(&self, session_id: &str) {
        *self.session_id.write().unwrap_or_else(|e| e.into_inner()) = Some(session_id.to_owned());
        self.bind_instantiated_providers();
    }

    pub(super) fn bind_working_dir(&self, working_dir: &std::path::Path) {
        *self.working_dir.write().unwrap_or_else(|e| e.into_inner()) =
            Some(working_dir.to_path_buf());
        self.bind_instantiated_providers();
    }

    pub(super) fn bind_instantiated_providers(&self) {
        let profiles: Vec<_> = self
            .openai_compatible_profiles
            .read()
            .unwrap_or_else(|e| e.into_inner())
            .values()
            .cloned()
            .collect();
        for provider in [
            self.anthropic_provider(),
            self.openai_provider(),
            self.copilot_provider(),
            self.antigravity_provider(),
            self.gemini_provider(),
            self.cursor_provider(),
            self.cursor_acp_provider(),
            self.openrouter_provider(),
            self.bedrock_provider()
                .map(|provider| provider as Arc<dyn Provider>),
        ]
        .into_iter()
        .flatten()
        .chain(profiles)
        {
            self.bind_provider_session(provider.as_ref());
        }
    }
}
