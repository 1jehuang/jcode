pub use jcode_auth_types::{
    AuthCredentialSource, AuthExpiryConfidence, AuthReadinessLevel, AuthRefreshSupport, AuthState,
    AuthValidationMethod,
};

use serde::Serialize;

/// Cached low-level authentication snapshot for all supported providers.
///
/// This is the probe/cache substrate. New CLI and UI surfaces should prefer
/// `AuthStatus::assessment_for_provider`, which normalizes these raw fields into
/// the canonical provider auth contract (`ProviderAuthAssessment`).
#[derive(Debug, Clone, Default)]
pub struct AuthStatus {
    /// Jcode subscription router credentials
    pub jcode: AuthState,
    /// Anthropic provider (Claude models) - via OAuth or API key
    pub anthropic: ProviderAuth,
    /// OpenRouter provider - via API key
    pub openrouter: AuthState,
    /// Azure OpenAI provider - via Entra ID or API key
    pub azure: AuthState,
    /// AWS Bedrock provider - via Bedrock API key or AWS credentials
    pub bedrock: AuthState,
    /// OpenAI provider - via OAuth or API key
    pub openai: AuthState,
    /// OpenAI has OAuth credentials
    pub openai_has_oauth: bool,
    /// OpenAI has API key available
    pub openai_has_api_key: bool,
    /// Azure OpenAI has API key available
    pub azure_has_api_key: bool,
    /// Azure OpenAI is configured for Entra ID authentication
    pub azure_uses_entra: bool,
    /// Copilot API available (GitHub OAuth token found)
    pub copilot: AuthState,
    /// Copilot has API token (from hosts.json/apps.json/GITHUB_TOKEN)
    pub copilot_has_api_token: bool,
    /// Antigravity OAuth configured
    pub antigravity: AuthState,
    /// Gemini CLI available
    pub gemini: AuthState,
    /// Cursor provider configured via Cursor Agent plus API key or CLI session
    pub cursor: AuthState,
    /// Google/Gmail OAuth configured
    pub google: AuthState,
    /// Google Gmail has send capability (Full tier)
    pub google_can_send: bool,
}

/// Auth state for Anthropic which has multiple auth methods
#[derive(Debug, Clone, Copy, Default)]
pub struct ProviderAuth {
    /// Overall state (best of available methods)
    pub state: AuthState,
    /// Has OAuth credentials
    pub has_oauth: bool,
    /// Has API key
    pub has_api_key: bool,
}

/// Canonical auth contract for one login provider.
///
/// This is the single structured answer that UI, CLI reports, diagnostics, and
/// provider setup should consume when they need to explain or act on auth state.
/// It combines the cached credential probe, source attribution, refresh metadata,
/// and runtime validation records into one provider-scoped assessment.
#[derive(Debug, Clone, Serialize)]
pub struct ProviderAuthAssessment {
    pub state: AuthState,
    pub readiness: AuthReadinessLevel,
    pub method_detail: String,
    pub credential_source: AuthCredentialSource,
    pub credential_source_detail: String,
    pub expiry_confidence: AuthExpiryConfidence,
    pub refresh_support: AuthRefreshSupport,
    pub validation_method: AuthValidationMethod,
    pub last_validation: Option<crate::auth::validation::ProviderValidationRecord>,
    pub last_refresh: Option<crate::auth::refresh_state::ProviderRefreshRecord>,
}

impl ProviderAuthAssessment {
    pub fn is_available(&self) -> bool {
        self.state == AuthState::Available
    }

    pub fn is_configured(&self) -> bool {
        self.state != AuthState::NotConfigured
    }

    pub fn health_summary(&self) -> String {
        let mut parts = vec![
            format!("readiness: {}", self.readiness.label()),
            format!("source: {}", self.credential_source_detail),
            format!("expiry: {}", self.expiry_confidence.label()),
            format!("refresh: {}", self.refresh_support.label()),
            format!("probe: {}", self.validation_method.label()),
        ];

        if let Some(record) = self.last_refresh.as_ref() {
            parts.push(format!(
                "last refresh: {}",
                crate::auth::refresh_state::format_record_label(record)
            ));
        }

        parts.join(" · ")
    }
}
