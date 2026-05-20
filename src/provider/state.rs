use super::MultiProvider;
use super::selection::{ActiveProvider, ConfigProviderSelection, ProviderAvailability};
use crate::auth::AuthStatus;
use crate::config::Config;

/// Canonical aggregate view of provider-related state.
///
/// This is intentionally not the durable storage for auth or config. Credentials
/// still live in auth/env files, and preferences still live in config.toml. This
/// facade is the single in-process place that combines those persisted sources
/// with provider catalog identity/resolution so CLI, TUI, and runtime code do not
/// each reinterpret provider strings differently.
pub(crate) struct ProviderState<'a> {
    config: &'a Config,
    auth_status: &'a AuthStatus,
}

/// The source of the currently selected runtime model.
///
/// This is deliberately small: login/config/env/model-picker code should not each
/// encode ad-hoc precedence rules. They should report what happened, and the
/// reducer decides how later auth/catalog work may reconcile with that choice.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub(crate) enum ProviderModelSelectionSource {
    Startup,
    User,
    Auth,
}

#[derive(Clone, Debug, Eq, PartialEq)]
pub(crate) enum ProviderStateEvent {
    RuntimeModelObserved { model: String },
    UserSelectedModel { model: String },
    AuthSelectedModel { model: String },
}

#[derive(Clone, Debug, Eq, PartialEq)]
pub(crate) struct ProviderRuntimeState {
    selected_model: Option<String>,
    selection_source: ProviderModelSelectionSource,
    selection_generation: u64,
}

impl Default for ProviderRuntimeState {
    fn default() -> Self {
        Self {
            selected_model: None,
            selection_source: ProviderModelSelectionSource::Startup,
            selection_generation: 0,
        }
    }
}

impl ProviderStateEvent {
    pub(crate) fn selected_model(
        source: ProviderModelSelectionSource,
        model: impl Into<String>,
    ) -> Self {
        match source {
            ProviderModelSelectionSource::Startup => Self::RuntimeModelObserved {
                model: model.into(),
            },
            ProviderModelSelectionSource::User => Self::UserSelectedModel {
                model: model.into(),
            },
            ProviderModelSelectionSource::Auth => Self::AuthSelectedModel {
                model: model.into(),
            },
        }
    }
}

impl ProviderRuntimeState {
    pub(crate) fn observed(model: impl Into<String>) -> Self {
        let mut state = Self::default();
        state.apply(ProviderStateEvent::RuntimeModelObserved {
            model: model.into(),
        });
        state
    }

    pub(crate) fn selection_generation(&self) -> u64 {
        self.selection_generation
    }

    pub(crate) fn user_selected_after(&self, generation: u64) -> bool {
        self.selection_generation > generation
            && self.selection_source == ProviderModelSelectionSource::User
    }

    pub(crate) fn apply(&mut self, event: ProviderStateEvent) {
        match event {
            ProviderStateEvent::RuntimeModelObserved { model } => {
                self.selected_model = Some(model);
                self.selection_source = ProviderModelSelectionSource::Startup;
            }
            ProviderStateEvent::UserSelectedModel { model } => {
                self.record_selection(model, ProviderModelSelectionSource::User);
            }
            ProviderStateEvent::AuthSelectedModel { model } => {
                self.record_selection(model, ProviderModelSelectionSource::Auth);
            }
        }
    }

    fn record_selection(&mut self, model: String, source: ProviderModelSelectionSource) {
        self.selected_model = Some(model);
        self.selection_source = source;
        self.selection_generation = self.selection_generation.saturating_add(1);
    }
}

impl<'a> ProviderState<'a> {
    pub(crate) fn from_parts(config: &'a Config, auth_status: &'a AuthStatus) -> Self {
        Self {
            config,
            auth_status,
        }
    }

    pub(crate) fn auth_status(&self) -> &'a AuthStatus {
        self.auth_status
    }

    pub(crate) fn default_model(&self) -> Option<&'a str> {
        self.config.provider.default_model.as_deref()
    }

    pub(crate) fn default_provider_key(&self) -> Option<&'a str> {
        self.config.provider.default_provider.as_deref()
    }

    pub(crate) fn default_provider_selection(&self) -> Option<ConfigProviderSelection> {
        self.default_provider_key().and_then(|provider| {
            MultiProvider::resolve_config_provider_selection(provider, self.config)
        })
    }

    pub(crate) fn preferred_active_provider(&self) -> Option<ActiveProvider> {
        self.default_provider_selection()
            .map(|selection| selection.active_provider())
    }

    pub(crate) fn preferred_provider_display_label(&self) -> Option<String> {
        self.default_provider_selection()
            .map(|selection| selection.display_label())
    }

    pub(crate) fn preferred_provider_is_configured(
        &self,
        availability: ProviderAvailability,
    ) -> Option<bool> {
        self.preferred_active_provider()
            .map(|provider| availability.is_configured(provider))
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn provider_state_resolves_default_provider_through_canonical_selection() {
        let mut cfg = Config::default();
        cfg.provider.default_provider = Some("kimi".to_string());
        cfg.provider.default_model = Some("moonshot-v1-8k".to_string());
        let auth = AuthStatus::default();
        let state = ProviderState::from_parts(&cfg, &auth);

        assert_eq!(state.default_provider_key(), Some("kimi"));
        assert_eq!(state.default_model(), Some("moonshot-v1-8k"));
        assert_eq!(
            state.preferred_active_provider(),
            Some(ActiveProvider::OpenRouter)
        );
        assert_eq!(
            state.preferred_provider_is_configured(ProviderAvailability {
                openrouter: true,
                ..ProviderAvailability::default()
            }),
            Some(true)
        );
    }

    #[test]
    fn runtime_state_tracks_user_selection_after_auth_generation() {
        let mut state = ProviderRuntimeState::observed("startup-model");
        assert_eq!(state.selection_generation(), 0);
        assert_eq!(
            state.selection_source,
            ProviderModelSelectionSource::Startup
        );

        state.apply(ProviderStateEvent::AuthSelectedModel {
            model: "auth-model".to_string(),
        });
        let auth_generation = state.selection_generation();
        assert_eq!(auth_generation, 1);
        assert_eq!(state.selected_model.as_deref(), Some("auth-model"));
        assert!(!state.user_selected_after(auth_generation));

        state.apply(ProviderStateEvent::UserSelectedModel {
            model: "manual-model".to_string(),
        });
        assert!(state.user_selected_after(auth_generation));
        assert_eq!(state.selected_model.as_deref(), Some("manual-model"));
        assert_eq!(state.selection_source, ProviderModelSelectionSource::User);
    }
}
