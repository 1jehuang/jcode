use super::*;

/// Centralizes runtime lookup and lifecycle decisions for provider slots that
/// can have more than one concrete runtime behind a single public provider.
///
/// Today the main case is `ActiveProvider::OpenRouter`: real OpenRouter and
/// direct OpenAI-compatible profiles share the OpenAI-compatible wire protocol,
/// but they are distinct runtime identities and must not overwrite each other.
pub(super) struct ProviderRegistry<'a> {
    provider: &'a MultiProvider,
}

impl<'a> ProviderRegistry<'a> {
    pub(super) fn new(provider: &'a MultiProvider) -> Self {
        Self { provider }
    }

    pub(super) fn real_openrouter(&self) -> Option<Arc<dyn Provider>> {
        self.provider
            .openrouter
            .read()
            .unwrap_or_else(|poisoned| poisoned.into_inner())
            .clone()
    }

    pub(super) fn compatible_profile(&self, profile_id: &str) -> Option<Arc<dyn Provider>> {
        self.provider
            .openai_compatible_profiles
            .read()
            .unwrap_or_else(|poisoned| poisoned.into_inner())
            .get(profile_id)
            .cloned()
    }

    pub(super) fn install_compatible_profile(
        &self,
        profile_id: impl Into<String>,
        runtime: Arc<dyn Provider>,
    ) {
        self.provider
            .openai_compatible_profiles
            .write()
            .unwrap_or_else(|poisoned| poisoned.into_inner())
            .insert(profile_id.into(), runtime);
    }

    pub(super) fn active_compatible_profile_id(&self) -> Option<String> {
        self.provider
            .active_named_provider_profiles
            .read()
            .unwrap_or_else(|poisoned| poisoned.into_inner())
            .as_ref()?
            .openai_compatible
            .clone()
    }

    pub(super) fn active_anthropic_profile_id(&self) -> Option<String> {
        self.provider
            .active_named_provider_profiles
            .read()
            .unwrap_or_else(|poisoned| poisoned.into_inner())
            .as_ref()?
            .anthropic_compatible
            .clone()
    }

    pub(super) fn set_active_compatible_profile(&self, profile_id: impl Into<String>) {
        let mut profiles = self
            .provider
            .active_named_provider_profiles
            .write()
            .unwrap_or_else(|poisoned| poisoned.into_inner());
        profiles
            .get_or_insert_with(ActiveNamedProviderProfiles::default)
            .openai_compatible = Some(profile_id.into());
    }

    pub(super) fn set_active_anthropic_profile(&self, profile_id: impl Into<String>) {
        let mut profiles = self
            .provider
            .active_named_provider_profiles
            .write()
            .unwrap_or_else(|poisoned| poisoned.into_inner());
        let profiles = profiles.get_or_insert_with(ActiveNamedProviderProfiles::default);
        profiles.openai_compatible = None;
        profiles.anthropic_compatible = Some(profile_id.into());
    }

    pub(super) fn clear_active_compatible_profile(&self) {
        let mut profiles = self
            .provider
            .active_named_provider_profiles
            .write()
            .unwrap_or_else(|poisoned| poisoned.into_inner());
        if let Some(active) = profiles.as_mut() {
            active.openai_compatible = None;
            if active.anthropic_compatible.is_none() {
                *profiles = None;
            }
        }
    }

    pub(super) fn clear_active_anthropic_profile(&self) {
        let mut profiles = self
            .provider
            .active_named_provider_profiles
            .write()
            .unwrap_or_else(|poisoned| poisoned.into_inner());
        if let Some(active) = profiles.as_mut() {
            active.anthropic_compatible = None;
            if active.openai_compatible.is_none() {
                *profiles = None;
            }
        }
    }

    pub(super) fn active_compatible_profile(&self) -> Option<Arc<dyn Provider>> {
        let profile_id = self.active_compatible_profile_id()?;
        self.compatible_profile(&profile_id)
    }

    /// Runtime that should execute requests for the public OpenRouter slot.
    pub(super) fn active_openrouter_execution(&self) -> Option<Arc<dyn Provider>> {
        self.active_compatible_profile()
            .or_else(|| self.real_openrouter())
    }
}
