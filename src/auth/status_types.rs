use serde::{Deserialize, Serialize};

/// Authentication status for all supported providers
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

/// State of a single auth credential
#[derive(Debug, Clone, Copy, Default, PartialEq, Eq, Serialize, Deserialize)]
pub enum AuthState {
    /// Credential is available and valid
    Available,
    /// Partial configuration exists (or OAuth may be expired)
    Expired,
    /// Credential is not configured
    #[default]
    NotConfigured,
}

#[derive(Debug, Clone, Copy, Default, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum AuthCredentialSource {
    #[default]
    None,
    EnvironmentVariable,
    AppConfigFile,
    JcodeManagedFile,
    TrustedExternalFile,
    TrustedExternalAppState,
    LocalCliSession,
    AzureDefaultCredential,
    Mixed,
}

impl AuthCredentialSource {
    pub fn label(self) -> &'static str {
        match self {
            Self::None => "none",
            Self::EnvironmentVariable => "environment variable",
            Self::AppConfigFile => "app config file",
            Self::JcodeManagedFile => "jcode-managed file",
            Self::TrustedExternalFile => "trusted external file",
            Self::TrustedExternalAppState => "trusted external app state",
            Self::LocalCliSession => "local CLI session",
            Self::AzureDefaultCredential => "Azure DefaultAzureCredential",
            Self::Mixed => "mixed",
        }
    }
}

#[derive(Debug, Clone, Copy, Default, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum AuthExpiryConfidence {
    #[default]
    Unknown,
    Exact,
    PresenceOnly,
    ConfigurationOnly,
    NotApplicable,
}

impl AuthExpiryConfidence {
    pub fn label(self) -> &'static str {
        match self {
            Self::Unknown => "unknown",
            Self::Exact => "exact timestamp",
            Self::PresenceOnly => "presence only",
            Self::ConfigurationOnly => "configuration only",
            Self::NotApplicable => "not applicable",
        }
    }
}

#[derive(Debug, Clone, Copy, Default, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum AuthRefreshSupport {
    #[default]
    Unknown,
    Automatic,
    Conditional,
    ManualRelogin,
    ExternalManaged,
    NotApplicable,
}

impl AuthRefreshSupport {
    pub fn label(self) -> &'static str {
        match self {
            Self::Unknown => "unknown",
            Self::Automatic => "automatic",
            Self::Conditional => "conditional",
            Self::ManualRelogin => "manual re-login",
            Self::ExternalManaged => "external/manual",
            Self::NotApplicable => "not applicable",
        }
    }
}

#[derive(Debug, Clone, Copy, Default, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum AuthValidationMethod {
    #[default]
    Unknown,
    PresenceCheck,
    TimestampCheck,
    ConfigurationCheck,
    TrustedImportScan,
    CommandProbe,
    CompositeProbe,
}

impl AuthValidationMethod {
    pub fn label(self) -> &'static str {
        match self {
            Self::Unknown => "unknown",
            Self::PresenceCheck => "presence check",
            Self::TimestampCheck => "timestamp check",
            Self::ConfigurationCheck => "configuration check",
            Self::TrustedImportScan => "trusted import scan",
            Self::CommandProbe => "command probe",
            Self::CompositeProbe => "composite probe",
        }
    }
}

#[derive(Debug, Clone, Serialize)]
pub struct ProviderAuthAssessment {
    pub state: AuthState,
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
    pub fn health_summary(&self) -> String {
        let mut parts = vec![
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
