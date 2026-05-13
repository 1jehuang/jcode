//! OAuth Authentication Service
//!
//! Provides OAuth 2.0 authentication support for MCP and LSP servers
//! that require OAuth-based authentication.
//!
//! Features:
//! - Token management (acquisition, refresh, caching)
//! - Multiple OAuth provider support
//! - Secure token storage
//! - Automatic token refresh on expiration
//! - PKCE support for security

use anyhow::{Context, Result};
use async_trait::async_trait;
use serde::{Deserialize, Serialize};
use std::collections::HashMap;
use std::path::PathBuf;
use std::sync::Arc;
use std::time::{Duration, Instant};
use tokio::sync::RwLock;
use tracing::{debug, error, info, warn};

/// OAuth token response from authorization server
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct OAuthToken {
    pub access_token: String,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub refresh_token: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub id_token: Option<String>,
    pub token_type: String,
    pub expires_in: u64,
    #[serde(skip_serializing)]
    obtained_at: Instant,
}

impl OAuthToken {
    pub fn new(
        access_token: String,
        refresh_token: Option<String>,
        id_token: Option<String>,
        expires_in: u64,
    ) -> Self {
        Self {
            access_token,
            refresh_token,
            id_token,
            token_type: "Bearer".to_string(),
            expires_in,
            obtained_at: Instant::now(),
        }
    }

    /// Check if the token is expired or will expire soon
    pub fn is_expired(&self) -> bool {
        let elapsed = self.obtained_at.elapsed().as_secs();
        // Consider expired if within 60 seconds of expiry
        elapsed >= self.expires_in.saturating_sub(60)
    }

    /// Get remaining lifetime in seconds
    pub fn remaining_lifetime(&self) -> u64 {
        let elapsed = self.obtained_at.elapsed().as_secs();
        self.expires_in.saturating_sub(elapsed)
    }

    /// Time until expiration
    pub fn expires_at(&self) -> Instant {
        self.obtained_at + Duration::from_secs(self.expires_in)
    }
}

/// OAuth configuration for a specific provider
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct OAuthProviderConfig {
    pub client_id: String,
    pub client_secret: Option<String>,
    pub authorization_url: String,
    pub token_url: String,
    pub refresh_url: Option<String>,
    pub scopes: Vec<String>,
    #[serde(default)]
    pub use_pkce: bool,
    #[serde(default = "default_redirect_uri")]
    pub redirect_uri: String,
}

fn default_redirect_uri() -> String {
    "http://localhost:8080/callback".to_string()
}

/// OAuth provider trait for extensibility
#[async_trait]
pub trait OAuthProvider: Send + Sync {
    fn name(&self) -> &str;

    async fn get_authorization_url(&self, state: &str) -> Result<String>;

    async fn exchange_code_for_token(
        &self,
        code: &str,
        code_verifier: Option<&str>,
    ) -> Result<OAuthToken>;

    async fn refresh_token(&self, refresh_token: &str) -> Result<OAuthToken>;

    fn config(&self) -> &OAuthProviderConfig;
}

/// Generic OAuth 2.0 provider implementation
pub struct GenericOAuthProvider {
    config: OAuthProviderConfig,
    http_client: reqwest::Client,
}

impl GenericOAuthProvider {
    pub fn new(config: OAuthProviderConfig) -> Self {
        let http_client = reqwest::Client::builder()
            .timeout(Duration::from_secs(30))
            .build()
            .expect("Failed to create HTTP client");

        Self { config, http_client }
    }
}

#[async_trait]
impl OAuthProvider for GenericOAuthProvider {
    fn name(&self) -> &str {
        &"generic"
    }

    async fn get_authorization_url(&self, state: &str) -> Result<String> {
        let url = format!(
            "{}?response_type=code&client_id={}&redirect_uri={}&scope={}&state={}",
            self.config.authorization_url,
            urlencoding::encode(&self.config.client_id),
            urlencoding::encode(&self.config.redirect_uri),
            urlencoding::encode(&self.config.scopes.join(" ")),
            urlencoding::encode(state),
        );

        if self.config.use_pkce {
            // PKCE parameters will be added by the caller
        }

        Ok(url)
    }

    async fn exchange_code_for_token(
        &self,
        code: &str,
        code_verifier: Option<&str>,
    ) -> Result<OAuthToken> {
        let mut params = HashMap::new();
        params.insert("grant_type", "authorization_code");
        params.insert("code", code);
        params.insert("redirect_uri", &self.config.redirect_uri);
        params.insert("client_id", &self.config.client_id);

        if let Some(secret) = &self.config.client_secret {
            params.insert("client_secret", secret);
        }

        if let Some(verifier) = code_verifier {
            params.insert("code_verifier", verifier);
        }

        let response = self
            .http_client
            .post(&self.config.token_url)
            .form(&params)
            .send()
            .await
            .context("Failed to send token request")?;

        if !response.status().is_success() {
            let status = response.status();
            let body = response.text().await.unwrap_or_default();
            anyhow::bail!(
                "Token request failed with status {}: {}",
                status,
                body
            );
        }

        let token_response: TokenResponse = response.json().await?;
        Ok(OAuthToken::new(
            token_response.access_token,
            token_response.refresh_token,
            token_response.id_token,
            token_response.expires_in,
        ))
    }

    async fn refresh_token(&self, refresh_token: &str) -> Result<OAuthToken> {
        let refresh_url = self
            .config
            .refresh_url
            .as_ref()
            .unwrap_or(&self.config.token_url);

        let mut params = HashMap::new();
        params.insert("grant_type", "refresh_token");
        params.insert("refresh_token", refresh_token);
        params.insert("client_id", &self.config.client_id);

        if let Some(secret) = &self.config.client_secret {
            params.insert("client_secret", secret);
        }

        let response = self
            .http_client
            .post(refresh_url)
            .form(&params)
            .send()
            .await
            .context("Failed to send refresh request")?;

        if !response.status().is_success() {
            let status = response.status();
            let body = response.text().await.unwrap_or_default();
            anyhow::bail!(
                "Refresh token request failed with status {}: {}",
                status,
                body
            );
        }

        let token_response: TokenResponse = response.json().await?;
        Ok(OAuthToken::new(
            token_response.access_token,
            token_response.refresh_token,
            token_response.id_token,
            token_response.expires_in,
        ))
    }

    fn config(&self) -> &OAuthProviderConfig {
        &self.config
    }
}

/// Token response from OAuth server
#[derive(Debug, Clone, Serialize, Deserialize)]
struct TokenResponse {
    access_token: String,
    #[serde(default)]
    refresh_token: Option<String>,
    #[serde(default)]
    id_token: Option<String>,
    expires_in: u64,
    #[serde(default = "default_token_type")]
    token_type: String,
}

fn default_token_type() -> String {
    "Bearer".to_string()
}

/// PKCE code challenge pair
#[derive(Debug, Clone)]
pub struct PkcePair {
    pub code_verifier: String,
    pub code_challenge: String,
}

impl PkcePair {
    pub fn generate() -> Result<Self> {
        use base64::{engine::general_purpose::URL_SAFE_NO_PAD, Engine as _};
        use rand::RngCore;
        use sha2::{Digest, Sha256};

        let mut bytes = [0u8; 32];
        rand::rngs::OsRng.fill_bytes(&mut bytes);

        let code_verifier = URL_SAFE_NO_PAD.encode(bytes);

        let mut hasher = Sha256::new();
        hasher.update(code_verifier.as_bytes());
        let hash = hasher.finalize();
        let code_challenge = URL_SAFE_NO_PAD.encode(hash);

        Ok(Self {
            code_verifier,
            code_challenge,
        })
    }
}

/// OAuth session manager for handling multiple providers
pub struct OAuthSessionManager {
    providers: RwLock<HashMap<String, Arc<dyn OAuthProvider>>>,
    tokens: RwLock<HashMap<String, OAuthToken>>,
    token_cache_path: Option<PathBuf>,
}

impl OAuthSessionManager {
    pub fn new(token_cache_path: Option<PathBuf>) -> Self {
        Self {
            providers: RwLock::new(HashMap::new()),
            tokens: RwLock::new(HashMap::new()),
            token_cache_path,
        }
    }

    /// Register an OAuth provider
    pub async fn register_provider(&self, name: String, provider: Arc<dyn OAuthProvider>) {
        let mut providers = self.providers.write().await;
        providers.insert(name.clone(), provider);
        info!("OAuth: Provider '{}' registered", name);
    }

    /// Get a valid access token, refreshing if necessary
    pub async fn get_valid_token(&self, provider_name: &str) -> Result<String> {
        let tokens = self.tokens.read().await;

        if let Some(token) = tokens.get(provider_name) {
            if !token.is_expired() {
                return Ok(token.access_token.clone());
            }
        }
        drop(tokens);

        // Need to refresh
        self.refresh_provider_token(provider_name).await
    }

    /// Refresh the token for a specific provider
    async fn refresh_provider_token(&self, provider_name: &str) -> Result<String> {
        let providers = self.providers.read().await;
        let provider = providers.get(provider_name).context(format!(
            "OAuth provider '{}' not found",
            provider_name
        ))?;

        let tokens = self.tokens.read().await;
        let old_token = tokens.get(provider_name).cloned();
        drop(tokens);

        if let Some(old_token) = old_token {
            if let Some(refresh_token) = old_token.refresh_token {
                match provider.refresh_token(&refresh_token).await {
                    Ok(new_token) => {
                        self.store_token(provider_name, new_token.clone()).await;
                        return Ok(new_token.access_token);
                    }
                    Err(e) => {
                        warn!(
                            "OAuth: Failed to refresh token for '{}': {}",
                            provider_name, e
                        );
                        // Continue to try getting new token
                    }
                }
            }
        }

        anyhow::bail!(
            "No valid token available for provider '{}'. User re-authentication required.",
            provider_name
        )
    }

    /// Store a new token
    async fn store_token(&self, provider_name: &str, token: OAuthToken) {
        let mut tokens = self.tokens.write().await;
        tokens.insert(provider_name.to_string(), token);

        // Persist to disk if cache path is set
        if let Some(cache_path) = &self.token_cache_path {
            if let Err(e) = self.persist_tokens_to_disk(cache_path).await {
                warn!("OAuth: Failed to persist tokens: {}", e);
            }
        }
    }

    /// Store token after successful authentication
    pub async fn store_new_token(&self, provider_name: &str, token: OAuthToken) {
        info!("OAuth: New token stored for '{}'", provider_name);
        self.store_token(provider_name, token).await;
    }

    /// Persist tokens to disk
    async fn persist_tokens_to_disk(&self, path: &PathBuf) -> Result<()> {
        let tokens = self.tokens.read().await;
        let data = serde_json::to_vec(&*tokens)?;
        tokio::fs::write(path, data).await?;
        Ok(())
    }

    /// Load tokens from disk
    pub async fn load_tokens_from_disk(&self) -> Result<()> {
        if let Some(cache_path) = &self.token_cache_path {
            if cache_path.exists() {
                let data = tokio::fs::read(cache_path).await?;
                let tokens: HashMap<String, OAuthToken> = serde_json::from_slice(&data)?;
                *self.tokens.write().await = tokens.clone();
                info!(
                    "OAuth: Loaded {} cached tokens from disk",
                    tokens.len()
                );
            }
        }
        Ok(())
    }

    /// Clear all tokens (for logout)
    pub async fn clear_all_tokens(&self) {
        let mut tokens = self.tokens.write().await;
        tokens.clear();

        if let Some(cache_path) = &self.token_cache_path {
            let _ = tokio::fs::remove_file(cache_path).await;
        }

        info!("OAuth: All tokens cleared");
    }

    /// Clear token for a specific provider
    pub async fn clear_token(&self, provider_name: &str) {
        let mut tokens = self.tokens.write().await;
        tokens.remove(provider_name);
        info!("OAuth: Token cleared for '{}'", provider_name);
    }

    /// Check if we have a valid token for a provider
    pub async fn has_valid_token(&self, provider_name: &str) -> bool {
        let tokens = self.tokens.read().await;
        match tokens.get(provider_name) {
            Some(token) => !token.is_expired(),
            None => false,
        }
    }

    /// Get token information (without exposing the actual token)
    pub async fn get_token_info(&self, provider_name: &str) -> Option<TokenInfo> {
        let tokens = self.tokens.read().await;
        tokens.get(provider_name).map(|t| TokenInfo {
            has_refresh_token: t.refresh_token.is_some(),
            remaining_seconds: t.remaining_lifetime(),
            is_expired: t.is_expired(),
        })
    }

    /// List all registered providers
    pub async fn list_providers(&self) -> Vec<String> {
        let providers = self.providers.read().await;
        providers.keys().cloned().collect()
    }

    /// Generate PKCE pair for authorization flow
    pub fn generate_pkce_pair() -> Result<PkcePair> {
        PkcePair::generate()
    }
}

/// Token information for display (no sensitive data)
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct TokenInfo {
    pub has_refresh_token: bool,
    pub remaining_seconds: u64,
    pub is_expired: bool,
}

/// Helper function to check and refresh OAuth token before MCP/LSP operations
pub async fn ensure_valid_oauth_token(
    oauth_manager: &OAuthSessionManager,
    provider_name: &str,
) -> Result<String> {
    debug!("OAuth: Checking token validity for '{}'", provider_name);

    match oauth_manager.get_valid_token(provider_name).await {
        Ok(token) => {
            debug!("OAuth: Valid token available for '{}'", provider_name);
            Ok(token)
        }
        Err(e) => {
            error!("OAuth: No valid token for '{}': {}", provider_name, e);
            Err(e)
        }
    }
}

/// Default token cache path
pub fn default_token_cache_path() -> PathBuf {
    dirs::cache_dir()
        .unwrap_or_else(|| PathBuf::from("."))
        .join("carpai")
        .join("oauth_tokens.json")
}
