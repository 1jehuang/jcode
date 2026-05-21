//! OAuth2 authentication provider implementation
//!
//! Supports multiple OAuth2 providers (Google, GitHub, Azure AD, etc.)
//! with PKCE flow for enhanced security.

use async_trait::async_trait;
use oauth2::{
    basic::BasicClient, AuthUrl, AuthorizationCode, ClientId, ClientSecret, CsrfToken,
    PkceCodeChallenge, RedirectUrl, Scope, TokenResponse, TokenUrl,
};
use serde::{Deserialize, Serialize};
use std::collections::HashMap;
use thiserror::Error;

#[derive(Error, Debug)]
pub enum OAuthError {
    #[error("OAuth2 error: {0}")]
    OAuth2(#[from] oauth2::RequestTokenError),

    #[error("Configuration error: {0}")]
    Config(String),

    #[error("Token validation failed: {0}")]
    TokenValidation(String),

    #[error("HTTP error: {0}")]
    Http(#[from] reqwest::Error),
}

pub type Result<T> = std::result::Result<T, OAuthError>;

/// OAuth2 token structure
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct OAuthToken {
    pub access_token: String,
    pub refresh_token: Option<String>,
    pub token_type: String,
    pub expires_in: Option<u64>,
    pub scope: Vec<String>,
    pub id_token: Option<String>, // For OIDC
}

/// OAuth2 provider configuration
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct OAuthConfig {
    pub client_id: String,
    pub client_secret: Option<String>,
    pub auth_url: String,
    pub token_url: String,
    pub redirect_uri: String,
    pub scopes: Vec<String>,
    pub provider_type: ProviderType,
}

/// Supported OAuth2 provider types
#[derive(Debug, Clone, Serialize, Deserialize, PartialEq)]
pub enum ProviderType {
    Google,
    GitHub,
    AzureAD,
    Okta,
    Generic,
}

impl ProviderType {
    /// Get default configuration for known providers
    pub fn default_config(&self, client_id: &str, client_secret: &str) -> OAuthConfig {
        match self {
            ProviderType::Google => OAuthConfig {
                client_id: client_id.to_string(),
                client_secret: Some(client_secret.to_string()),
                auth_url: "https://accounts.google.com/o/oauth2/v2/auth".to_string(),
                token_url: "https://oauth2.googleapis.com/token".to_string(),
                redirect_uri: "http://localhost:8080/oauth/callback".to_string(),
                scopes: vec!["openid".to_string(), "profile".to_string(), "email".to_string()],
                provider_type: ProviderType::Google,
            },
            ProviderType::GitHub => OAuthConfig {
                client_id: client_id.to_string(),
                client_secret: Some(client_secret.to_string()),
                auth_url: "https://github.com/login/oauth/authorize".to_string(),
                token_url: "https://github.com/login/oauth/access_token".to_string(),
                redirect_uri: "http://localhost:8080/oauth/callback".to_string(),
                scopes: vec!["user:email".to_string(), "read:user".to_string()],
                provider_type: ProviderType::GitHub,
            },
            ProviderType::AzureAD => OAuthConfig {
                client_id: client_id.to_string(),
                client_secret: Some(client_secret.to_string()),
                auth_url: "https://login.microsoftonline.com/common/oauth2/v2.0/authorize".to_string(),
                token_url: "https://login.microsoftonline.com/common/oauth2/v2.0/token".to_string(),
                redirect_uri: "http://localhost:8080/oauth/callback".to_string(),
                scopes: vec![
                    "openid".to_string(),
                    "profile".to_string(),
                    "offline_access".to_string(),
                ],
                provider_type: ProviderType::AzureAD,
            },
            _ => panic!("No default config for provider type {:?}", self),
        }
    }
}

/// OAuth2 provider trait for extensibility
#[async_trait]
pub trait OAuthProvider: Send + Sync {
    /// Generate authorization URL with PKCE
    fn get_authorization_url(&self) -> Result<(String, CsrfToken, String)>;

    /// Exchange authorization code for tokens
    async fn exchange_code(&self, code: String, pkce_verifier: String) -> Result<OAuthToken>;

    /// Refresh access token
    async fn refresh_token(&self, refresh_token: &str) -> Result<OAuthToken>;

    /// Validate token and get user info
    async fn validate_token(&self, token: &str) -> Result<HashMap<String, serde_json::Value>>;

    /// Revoke token
    async fn revoke_token(&self, token: &str) -> Result<()>;
}

/// Standard OAuth2 provider implementation
pub struct StandardOAuthProvider {
    client: BasicClient,
    config: OAuthConfig,
}

impl StandardOAuthProvider {
    pub fn new(config: OAuthConfig) -> Result<Self> {
        let client = BasicClient::new(ClientId::new(config.client_id.clone()))
            .set_client_secret(config.client_secret.clone().map(ClientSecret::new))
            .set_auth_uri(AuthUrl::new(config.auth_url.clone())?)
            .set_token_uri(TokenUrl::new(config.token_url.clone())?)
            .set_redirect_uri(RedirectUrl::new(config.redirect_uri.clone())?);

        Ok(Self { client, config })
    }
}

#[async_trait]
impl OAuthProvider for StandardOAuthProvider {
    fn get_authorization_url(&self) -> Result<(String, CsrfToken, String)> {
        let (pkce_challenge, pkce_verifier) = PkceCodeChallenge::new_random_sha256();

        let (auth_url, csrf_token) = self
            .client
            .authorize_url(CsrfToken::new_random)
            .add_scopes(
                self.config
                    .scopes
                    .iter()
                    .map(|s| Scope::new(s.clone()))
                    .collect::<Vec<_>>(),
            )
            .set_pkce_challenge(pkce_challenge)
            .url();

        Ok((
            auth_url.to_string(),
            csrf_token,
            pkce_verifier.secret().clone(),
        ))
    }

    async fn exchange_code(&self, code: String, pkce_verifier: String) -> Result<OAuthToken> {
        let token_response = self
            .client
            .exchange_code(AuthorizationCode::new(code))
            .set_pkce_verifier(oauth2::PkceCodeVerifier::new(pkce_verifier))
            .request_async(oauth2::reqwest::async_http_client)
            .await?;

        Ok(OAuthToken {
            access_token: token_response.access_token().secret().clone(),
            refresh_token: token_response
                .refresh_token()
                .map(|t| t.secret().clone()),
            token_type: token_response.token_type().to_string(),
            expires_in: token_response.expires_in().map(|d| d.as_secs()),
            scope: token_response
                .scopes()
                .map(|s| s.iter().map(|s| s.to_string()).collect())
                .unwrap_or_default(),
            id_token: None, // Would need to extract from extra fields
        })
    }

    async fn refresh_token(&self, refresh_token: &str) -> Result<OAuthToken> {
        let token_response = self
            .client
            .refresh_token(oauth2::RefreshToken::new(refresh_token.to_string()))
            .request_async(oauth2::reqwest::async_http_client)
            .await?;

        Ok(OAuthToken {
            access_token: token_response.access_token().secret().clone(),
            refresh_token: token_response
                .refresh_token()
                .map(|t| t.secret().clone()),
            token_type: token_response.token_type().to_string(),
            expires_in: token_response.expires_in().map(|d| d.as_secs()),
            scope: token_response
                .scopes()
                .map(|s| s.iter().map(|s| s.to_string()).collect())
                .unwrap_or_default(),
            id_token: None,
        })
    }

    async fn validate_token(&self, _token: &str) -> Result<HashMap<String, serde_json::Value>> {
        // Implementation depends on provider's userinfo endpoint
        // This is a placeholder - real implementation would call the userinfo endpoint
        Err(OAuthError::TokenValidation(
            "Not implemented for generic provider".to_string(),
        ))
    }

    async fn revoke_token(&self, _token: &str) -> Result<()> {
        // Not all providers support token revocation
        Ok(())
    }
}

/// OAuth2 session manager
pub struct OAuthSessionManager {
    providers: HashMap<String, Box<dyn OAuthProvider>>,
}

impl OAuthSessionManager {
    pub fn new() -> Self {
        Self {
            providers: HashMap::new(),
        }
    }

    pub fn register_provider(&mut self, name: String, provider: Box<dyn OAuthProvider>) {
        self.providers.insert(name, provider);
    }

    pub fn get_provider(&self, name: &str) -> Option<&dyn OAuthProvider> {
        self.providers.get(name).map(|p| p.as_ref())
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_oauth_config_creation() {
        let config = ProviderType::Google.default_config("test_client", "test_secret");
        assert_eq!(config.client_id, "test_client");
        assert_eq!(config.provider_type, ProviderType::Google);
        assert!(config.scopes.contains(&"openid".to_string()));
    }

    #[tokio::test]
    async fn test_standard_oauth_provider() {
        let config = ProviderType::GitHub.default_config("test_client", "test_secret");
        let provider = StandardOAuthProvider::new(config).unwrap();

        let (auth_url, _csrf, _verifier) = provider.get_authorization_url().unwrap();
        assert!(auth_url.starts_with("https://github.com/login/oauth/authorize"));
    }
}
