pub mod claude;
pub mod codex;
pub mod oauth;

/// Authentication status for all supported providers
#[derive(Debug, Clone, Default)]
pub struct AuthStatus {
    /// Anthropic provider (Claude models) - via OAuth or API key
    pub anthropic: ProviderAuth,
    /// OpenRouter provider - via API key
    pub openrouter: AuthState,
    /// OpenAI provider - via API key
    pub openai: AuthState,
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
#[derive(Debug, Clone, Copy, Default, PartialEq, Eq)]
pub enum AuthState {
    /// Credential is available and valid
    Available,
    /// Credential exists but is expired (may still work with refresh)
    Expired,
    /// Credential is not configured
    #[default]
    NotConfigured,
}

impl AuthStatus {
    /// Check all authentication sources and return their status
    pub fn check() -> Self {
        let mut status = Self::default();

        // Check Anthropic (OAuth or API key)
        let mut anthropic = ProviderAuth::default();

        // Check OAuth
        match claude::load_credentials() {
            Ok(creds) => {
                let now_ms = chrono::Utc::now().timestamp_millis();
                anthropic.has_oauth = true;
                if creds.expires_at > now_ms {
                    anthropic.state = AuthState::Available;
                } else {
                    anthropic.state = AuthState::Expired;
                }
            }
            Err(_) => {}
        }

        // Check API key (overrides expired OAuth)
        if std::env::var("ANTHROPIC_API_KEY").is_ok() {
            anthropic.has_api_key = true;
            anthropic.state = AuthState::Available;
        }

        status.anthropic = anthropic;

        // Check OpenRouter API key
        if std::env::var("OPENROUTER_API_KEY").is_ok() {
            status.openrouter = AuthState::Available;
        }

        // Check OpenAI API key
        if std::env::var("OPENAI_API_KEY").is_ok() {
            status.openai = AuthState::Available;
        }

        status
    }
}
