use anyhow::Result;
use jcode_provider_core::{ActiveProvider, provider_key};

/// Stable product/runtime identity selected by login or provider initialization.
///
/// This intentionally differs from the lower-level [`ActiveProvider`] execution slot.
/// For example Azure OpenAI currently reuses the OpenAI-compatible/OpenRouter HTTP
/// transport, but its runtime identity is still Azure OpenAI.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum RuntimeProviderId {
    Jcode,
    Claude,
    OpenAi,
    OpenAiApiKey,
    OpenRouter,
    OpenAiCompatible,
    AzureOpenAi,
    Bedrock,
    Cursor,
    Copilot,
    Gemini,
    Antigravity,
    AutoImport,
}

impl RuntimeProviderId {
    pub const fn key(self) -> &'static str {
        match self {
            Self::Jcode => "jcode",
            Self::Claude => "claude",
            Self::OpenAi => "openai",
            Self::OpenAiApiKey => "openai-api",
            Self::OpenRouter => "openrouter",
            Self::OpenAiCompatible => "openai-compatible",
            Self::AzureOpenAi => "azure-openai",
            Self::Bedrock => "bedrock",
            Self::Cursor => "cursor",
            Self::Copilot => "copilot",
            Self::Gemini => "gemini",
            Self::Antigravity => "antigravity",
            Self::AutoImport => "auto-import",
        }
    }

    pub const fn label(self) -> &'static str {
        match self {
            Self::Jcode => "Jcode Subscription",
            Self::Claude => "Anthropic/Claude",
            Self::OpenAi => "OpenAI",
            Self::OpenAiApiKey => "OpenAI API",
            Self::OpenRouter => "OpenRouter",
            Self::OpenAiCompatible => "OpenAI-compatible",
            Self::AzureOpenAi => "Azure OpenAI",
            Self::Bedrock => "AWS Bedrock",
            Self::Cursor => "Cursor",
            Self::Copilot => "GitHub Copilot",
            Self::Gemini => "Gemini",
            Self::Antigravity => "Antigravity",
            Self::AutoImport => "Auto Import",
        }
    }
}

/// How model routing should be represented in the current process.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum RuntimeSelection {
    /// Force the multi-provider router onto a concrete execution slot.
    Locked(ActiveProvider),
    /// Do not force routing. Optionally set a preferred active provider for UI/session context.
    Unlocked { active_hint: Option<ActiveProvider> },
    /// Leave existing routing env untouched.
    Unchanged,
}

impl RuntimeSelection {
    fn log_value(self) -> &'static str {
        match self {
            Self::Locked(_) => "locked",
            Self::Unlocked { .. } => "unlocked",
            Self::Unchanged => "unchanged",
        }
    }
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub struct RuntimeModelHint {
    pub env_key: &'static str,
    pub model: String,
}

impl RuntimeModelHint {
    pub fn new(env_key: &'static str, model: impl Into<String>) -> Self {
        Self {
            env_key,
            model: model.into(),
        }
    }
}

/// Typed activation plan shared by CLI, TUI, and bootstrap code.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct ProviderActivation {
    pub runtime_id: RuntimeProviderId,
    pub selection: RuntimeSelection,
    pub model_hint: Option<RuntimeModelHint>,
}

impl ProviderActivation {
    pub fn new(runtime_id: RuntimeProviderId, selection: RuntimeSelection) -> Self {
        Self {
            runtime_id,
            selection,
            model_hint: None,
        }
    }

    pub fn with_model_hint(mut self, env_key: &'static str, model: impl Into<String>) -> Self {
        self.model_hint = Some(RuntimeModelHint::new(env_key, model));
        self
    }

    pub fn locked(runtime_id: RuntimeProviderId, active_provider: ActiveProvider) -> Self {
        Self::new(runtime_id, RuntimeSelection::Locked(active_provider))
    }

    pub fn unlocked(runtime_id: RuntimeProviderId, active_hint: Option<ActiveProvider>) -> Self {
        Self::new(runtime_id, RuntimeSelection::Unlocked { active_hint })
    }

    pub fn azure_openai(model: Option<String>) -> Self {
        let activation = Self::locked(RuntimeProviderId::AzureOpenAi, ActiveProvider::OpenRouter);
        if let Some(model) = model.filter(|value| !value.trim().is_empty()) {
            activation.with_model_hint("JCODE_OPENROUTER_MODEL", model)
        } else {
            activation
        }
    }

    pub fn openai_compatible(model: Option<String>) -> Self {
        let activation = Self::locked(
            RuntimeProviderId::OpenAiCompatible,
            ActiveProvider::OpenRouter,
        );
        if let Some(model) = model.filter(|value| !value.trim().is_empty()) {
            activation.with_model_hint("JCODE_OPENROUTER_MODEL", model)
        } else {
            activation
        }
    }

    pub fn jcode_subscription(model: impl Into<String>) -> Self {
        Self::locked(RuntimeProviderId::Jcode, ActiveProvider::OpenRouter)
            .with_model_hint("JCODE_OPENROUTER_MODEL", model)
    }

    pub fn apply_env(&self) -> Result<()> {
        crate::env::set_var("JCODE_RUNTIME_PROVIDER", self.runtime_id.key());

        let mut active_key_for_log = "";
        match self.selection {
            RuntimeSelection::Locked(active_provider) => {
                active_key_for_log = provider_key(active_provider);
                crate::env::set_var("JCODE_ACTIVE_PROVIDER", active_key_for_log);
                crate::env::set_var("JCODE_FORCE_PROVIDER", "1");
            }
            RuntimeSelection::Unlocked { active_hint } => {
                crate::env::remove_var("JCODE_FORCE_PROVIDER");
                if let Some(active_provider) = active_hint {
                    active_key_for_log = provider_key(active_provider);
                    crate::env::set_var("JCODE_ACTIVE_PROVIDER", active_key_for_log);
                }
            }
            RuntimeSelection::Unchanged => {}
        }

        if let Some(model_hint) = &self.model_hint {
            crate::env::set_var(model_hint.env_key, &model_hint.model);
        }

        let model_env = self
            .model_hint
            .as_ref()
            .map(|hint| hint.env_key)
            .unwrap_or("");
        crate::logging::auth_event(
            "runtime_activation",
            self.runtime_id.key(),
            &[
                ("label", self.runtime_id.label()),
                ("selection", self.selection.log_value()),
                ("active_provider", active_key_for_log),
                ("model_env", model_env),
            ],
        );
        Ok(())
    }
}

/// Backwards-compatible adapter for existing string-based call sites.
pub fn lock_runtime_provider_key(provider_key_raw: &str) {
    crate::env::set_var("JCODE_ACTIVE_PROVIDER", provider_key_raw);
    crate::env::set_var("JCODE_FORCE_PROVIDER", "1");
    crate::logging::auth_event(
        "runtime_activation_legacy_lock",
        provider_key_raw,
        &[("selection", "locked")],
    );
}

pub fn unlock_runtime_provider() {
    crate::env::remove_var("JCODE_FORCE_PROVIDER");
    crate::logging::auth_event(
        "runtime_activation_unlock",
        "runtime",
        &[("selection", "unlocked")],
    );
}

pub fn apply_azure_openai_runtime() -> Result<Option<String>> {
    crate::auth::azure::apply_runtime_env()?;
    let model = crate::auth::azure::load_model();
    ProviderActivation::azure_openai(model.clone()).apply_env()?;
    Ok(model)
}

pub fn apply_openai_compatible_runtime(default_model: Option<String>) -> Result<()> {
    ProviderActivation::openai_compatible(default_model).apply_env()
}

#[cfg(test)]
mod tests {
    use super::*;

    struct EnvGuard {
        saved: Vec<(&'static str, Option<String>)>,
    }

    impl EnvGuard {
        fn new(keys: &[&'static str]) -> Self {
            let saved = keys
                .iter()
                .map(|key| (*key, std::env::var(key).ok()))
                .collect();
            for key in keys {
                crate::env::remove_var(key);
            }
            Self { saved }
        }
    }

    impl Drop for EnvGuard {
        fn drop(&mut self) {
            for (key, value) in self.saved.drain(..) {
                if let Some(value) = value {
                    crate::env::set_var(key, value);
                } else {
                    crate::env::remove_var(key);
                }
            }
        }
    }

    #[test]
    fn azure_activation_preserves_identity_while_using_openrouter_slot() {
        let _guard = EnvGuard::new(&[
            "JCODE_RUNTIME_PROVIDER",
            "JCODE_ACTIVE_PROVIDER",
            "JCODE_FORCE_PROVIDER",
            "JCODE_OPENROUTER_MODEL",
        ]);

        ProviderActivation::azure_openai(Some("gpt-4.1-mini".to_string()))
            .apply_env()
            .unwrap();

        assert_eq!(
            std::env::var("JCODE_RUNTIME_PROVIDER").as_deref(),
            Ok("azure-openai")
        );
        assert_eq!(
            std::env::var("JCODE_ACTIVE_PROVIDER").as_deref(),
            Ok("openrouter")
        );
        assert_eq!(std::env::var("JCODE_FORCE_PROVIDER").as_deref(), Ok("1"));
        assert_eq!(
            std::env::var("JCODE_OPENROUTER_MODEL").as_deref(),
            Ok("gpt-4.1-mini")
        );
    }
}
