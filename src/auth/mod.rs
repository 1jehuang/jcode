pub mod claude;
pub mod codex;
pub mod oauth;

/// Authentication status for all supported providers
#[derive(Debug, Clone, Default)]
pub struct AuthStatus {
    /// Claude Code OAuth credentials (from ~/.claude/.credentials.json)
    pub claude_oauth: AuthState,
    /// Direct Anthropic API key (ANTHROPIC_API_KEY env var)
    pub anthropic_api_key: AuthState,
    /// OpenRouter API key (OPENROUTER_API_KEY env var)
    pub openrouter_api_key: AuthState,
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

        // Check Claude OAuth
        match claude::load_credentials() {
            Ok(creds) => {
                let now_ms = chrono::Utc::now().timestamp_millis();
                if creds.expires_at > now_ms {
                    status.claude_oauth = AuthState::Available;
                } else {
                    status.claude_oauth = AuthState::Expired;
                }
            }
            Err(_) => {
                status.claude_oauth = AuthState::NotConfigured;
            }
        }

        // Check Anthropic API key
        if std::env::var("ANTHROPIC_API_KEY").is_ok() {
            status.anthropic_api_key = AuthState::Available;
        }

        // Check OpenRouter API key
        if std::env::var("OPENROUTER_API_KEY").is_ok() {
            status.openrouter_api_key = AuthState::Available;
        }

        status
    }
}
