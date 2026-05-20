#[derive(Debug, Clone)]
pub(crate) enum PendingLogin {
    /// Waiting for user to paste Claude OAuth code for a specific stored account
    ClaudeAccount {
        verifier: String,
        label: String,
        redirect_uri: Option<String>,
    },
    /// Waiting for user to paste an OpenAI OAuth callback URL/query for a specific stored account.
    OpenAiAccount {
        verifier: String,
        label: String,
        expected_state: String,
        redirect_uri: String,
    },
    /// Waiting for user to paste a Gemini OAuth callback URL/query or auth code.
    Gemini {
        verifier: String,
        expected_state: Option<String>,
        redirect_uri: String,
    },
    /// Waiting for user to paste an Antigravity OAuth callback URL/query.
    Antigravity {
        verifier: String,
        expected_state: String,
        redirect_uri: String,
    },
    /// Waiting for user to paste an API key for an OpenAI-compatible provider.
    ApiKeyProfile {
        provider_id: String,
        provider: String,
        auth_method: String,
        docs_url: String,
        env_file: String,
        key_name: String,
        default_model: Option<String>,
        endpoint: Option<String>,
        api_key_optional: bool,
        openai_compatible_profile: Option<crate::provider_catalog::OpenAiCompatibleProfile>,
    },
    /// Waiting for the user to paste a custom OpenAI-compatible API base.
    OpenAiCompatibleApiBase {
        profile: crate::provider_catalog::OpenAiCompatibleProfile,
    },
    /// Waiting for user to paste a Cursor API key.
    CursorApiKey,
    /// GitHub Copilot device flow in progress (polling in background)
    Copilot,
    /// Waiting for the user to choose which external auth sources to import.
    AutoImportSelection {
        candidates: Vec<crate::cli::provider_init::ExternalAuthReviewCandidate>,
    },
    /// Waiting for Azure OpenAI endpoint.
    AzureEndpoint,
    /// Waiting for Azure OpenAI deployment/model name.
    AzureModel { endpoint: String },
    /// Waiting for Azure OpenAI auth method choice.
    AzureAuthChoice { endpoint: String, model: String },
    /// Waiting for Azure OpenAI API key.
    AzureApiKey { endpoint: String, model: String },
}

impl PendingLogin {
    pub(crate) fn telemetry_context(&self) -> Option<(String, String)> {
        match self {
            Self::ClaudeAccount { .. } => Some(("claude".to_string(), "oauth".to_string())),
            Self::OpenAiAccount { .. } => Some(("openai".to_string(), "oauth".to_string())),
            Self::Gemini { .. } => Some(("gemini".to_string(), "oauth".to_string())),
            Self::Antigravity { .. } => Some(("antigravity".to_string(), "oauth".to_string())),
            Self::ApiKeyProfile {
                provider_id,
                auth_method,
                ..
            } => Some((provider_id.clone(), auth_method.clone())),
            Self::OpenAiCompatibleApiBase { profile } => {
                let resolved = crate::provider_catalog::resolve_openai_compatible_profile(*profile);
                Some((
                    resolved.id,
                    if resolved.requires_api_key {
                        "api_key".to_string()
                    } else {
                        "local_endpoint".to_string()
                    },
                ))
            }
            Self::CursorApiKey => Some(("cursor".to_string(), "api_key".to_string())),
            Self::Copilot => Some(("copilot".to_string(), "device_code".to_string())),
            Self::AutoImportSelection { .. } => None,
            Self::AzureEndpoint | Self::AzureModel { .. } | Self::AzureAuthChoice { .. } => {
                Some(("azure".to_string(), "hybrid".to_string()))
            }
            Self::AzureApiKey { .. } => Some(("azure".to_string(), "api_key".to_string())),
        }
    }
}

#[derive(Debug, Clone)]
pub(crate) enum PendingAccountInput {
    NewAccountLabel {
        provider_id: String,
        display_name: String,
    },
    CommandValue {
        prompt: String,
        command_prefix: String,
        empty_value: Option<String>,
        status_notice: String,
    },
}

#[derive(Debug, Clone)]
pub(crate) enum AccountCommand {
    OpenOverlay {
        provider_filter: Option<String>,
    },
    Doctor {
        provider_id: Option<String>,
    },
    ShowSettings {
        provider_id: String,
    },
    Login {
        provider_id: String,
    },
    Add {
        provider_id: String,
        label: Option<String>,
    },
    Switch {
        provider_id: String,
        label: String,
    },
    SwitchShorthand {
        label: String,
    },
    Remove {
        provider_id: String,
        label: String,
    },
    SetDefaultProvider(Option<String>),
    SetDefaultModel(Option<String>),
    SetOpenAiTransport(Option<String>),
    SetOpenAiEffort(Option<String>),
    SetOpenAiFast(bool),
    SetCopilotPremium(Option<String>),
    SetOpenAiCompatApiBase(Option<String>),
    SetOpenAiCompatApiKeyName(Option<String>),
    SetOpenAiCompatEnvFile(Option<String>),
    SetOpenAiCompatDefaultModel(Option<String>),
}
