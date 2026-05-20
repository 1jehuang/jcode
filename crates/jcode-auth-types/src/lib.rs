use serde::{Deserialize, Serialize};

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

#[derive(Debug, Clone, Copy, Default, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum AuthReadinessLevel {
    #[default]
    None,
    CredentialPresent,
    Authenticated,
    RequestValid,
    DeploymentValid,
}

impl AuthReadinessLevel {
    pub fn label(self) -> &'static str {
        match self {
            Self::None => "not configured",
            Self::CredentialPresent => "credential present",
            Self::Authenticated => "authenticated",
            Self::RequestValid => "request valid",
            Self::DeploymentValid => "deployment valid",
        }
    }
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
pub struct ProviderValidationRecord {
    pub checked_at_ms: i64,
    pub success: bool,
    pub provider_smoke_ok: Option<bool>,
    pub tool_smoke_ok: Option<bool>,
    pub summary: String,
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
pub struct ProviderRefreshRecord {
    pub last_attempt_ms: i64,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub last_success_ms: Option<i64>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub last_error: Option<String>,
}
